//! Main window. Rules:
//! - The UI thread never touches disk, network or shell; workers report via `Events`.
//! - Never call into Win32 while `APP` is borrowed: controls send messages
//!   back synchronously, and a re-entrant borrow would fail.

use std::cell::RefCell;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, GetMonitorInfoW, HBRUSH, HDC, HFONT, HGDIOBJ,
    InvalidateRect, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect, MonitorFromWindow,
    ScreenToClient,
};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::SystemServices::{
    APPCOMMAND_BROWSER_BACKWARD, APPCOMMAND_BROWSER_FORWARD, APPCOMMAND_BROWSER_REFRESH, APPCOMMAND_ID,
};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};

use crate::browser::{Applied, Browser, ListJob, SortJob, parse_location};
use crate::events::Events;
use crate::icons::{self, Done, Icons};
use crate::listing::Snapshot;
use crate::search;
use crate::settings::Settings;
use crate::sort::{Sort, SortKey, filter_order, sorted_order};
use crate::tabs::Tabs;
use crate::theme::{Palette, ThemeChoice, system_prefers_dark};
use crate::watch::Watcher;
use crate::{crash, format, shell};

mod commands;
mod dark;
mod dialogs;
mod drag;
mod preview;
mod tabbar;
pub mod toolbar;
pub use commands::{
    CMD_COPY, CMD_COPY_PATH, CMD_CUT, CMD_DELETE, CMD_DELETE_PERMANENTLY, CMD_MORE_OPTIONS, CMD_NEW_FOLDER,
    CMD_OPEN, CMD_OPEN_NEW_TAB, CMD_PASTE, CMD_RENAME, CMD_SELECT_ALL, CMD_SHOW_IN_EXPLORER,
};
mod tree;

const CLASS: PCWSTR = w!("Kestrel");
const WM_APP_EVENTS: u32 = WM_APP + 1;
const WM_APP_LATE_INIT: u32 = WM_APP + 2;
const ID_BACK: usize = 1;
const ID_FORWARD: usize = 2;
const ID_UP: usize = 3;
/// Listing threads stuck on dead network paths we tolerate before refusing new ones.
const MAX_LISTERS: usize = 8;
const COLUMNS: [(PCWSTR, i32, SortKey); 4] = [
    (w!("Name"), 320, SortKey::Name),
    (w!("Date modified"), 160, SortKey::Modified),
    (w!("Type"), 150, SortKey::Type),
    (w!("Size"), 100, SortKey::Size),
];

enum Event {
    Listed {
        generation: u64,
        result: io::Result<(Snapshot, Vec<u32>)>,
    },
    Sorted {
        generation: u64,
        sort: Sort,
        show_hidden: bool,
        filter: Vec<u16>,
        order: Vec<u32>,
    },
    /// A shortcut resolved to a folder: show it.
    Navigate(PathBuf),
    /// One-off status bar message.
    Message(String),
    /// A preview finished loading.
    Preview {
        token: u64,
        shown: preview::Shown,
    },
    /// A search is running; this many matches so far.
    SearchProgress(usize),
    TreeChildren {
        node: usize,
        result: io::Result<Vec<String>>,
    },
    DriveLabel {
        node: usize,
        text: String,
    },
    Icon(Done),
    /// The watched folder changed (`id` names the watcher).
    DirChanged(u64),
    /// A watcher opened its folder; `handle` is for device-removal notifications.
    WatchReady {
        id: u64,
        handle: isize,
    },
    /// The list should catch up with the browser (after a rename box closed).
    Resync,
    /// A file operation ended: the item to select, or an error.
    OpDone {
        result: std::result::Result<Option<String>, String>,
        edit: bool,
    },
}

struct App {
    hwnd: HWND,
    list: HWND,
    address: HWND,
    /// Filter box: narrows the list as you type; Enter searches subfolders.
    filter: HWND,
    buttons: [HWND; 3],
    toolbar: Vec<HWND>,
    startup_override: Option<Option<PathBuf>>,
    status: HWND,
    font: HFONT,
    browser: Browser,
    events: Arc<Events<Event>>,
    /// Thread listing the folder being loaded, so Esc can cancel its blocking I/O.
    lister: Option<JoinHandle<()>>,
    /// Tells a running search to stop.
    lister_cancel: Arc<std::sync::atomic::AtomicBool>,
    tabs: Tabs<tabbar::TabSlot>,
    tab_strip: HWND,
    next_tab_id: u64,
    theme: ThemeChoice,
    palette: Palette,
    brushes: Option<dark::Brushes>,
    preview: preview::Pane,
    live_listers: Arc<AtomicUsize>,
    tree: HWND,
    nodes: Vec<tree::Node>,
    /// Sidebar width in pixels.
    split: i32,
    /// While dragging the splitter: where in the gap it was grabbed.
    drag_offset: Option<i32>,
    icons: Icons,
    /// Watcher for the folder on screen, with its id; events from older ids are ignored.
    watcher: Option<(u64, Watcher)>,
    watch_ids: u64,
    /// Device notification for the watched folder's handle, and that handle.
    device_note: Option<(isize, isize)>,
    /// The sidebar's "This PC" node, whose children are the drives.
    this_pc: usize,
    /// Name of a just-created item to start renaming once it shows up.
    edit_pending: Option<String>,
    /// The item whose name is being edited in place. Kept as a path: rows can
    /// move under the edit box if the folder changes meanwhile.
    renaming: Option<PathBuf>,
    /// The folder changed during the rename; sync the list once the box closes.
    resync_after_rename: bool,
    /// Folder row highlighted as the destination of a drag in progress.
    drop_row: Option<usize>,
    drop_target: Option<windows::Win32::System::Ole::IDropTarget>,
}

impl App {
    fn controls(&self) -> Vec<HWND> {
        let [b0, b1, b2] = self.buttons;
        let mut controls =
            vec![self.list, self.status, self.address, self.filter, self.tree, self.tab_strip, b0, b1, b2];
        controls.extend(&self.toolbar);
        controls.extend(preview::controls(&self.preview));
        controls
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// View state captured as the window closes, for saving.
    static FINAL_SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
}

/// File operations still running. The process must not exit under them:
/// a copy cut off halfway leaves a broken file.
static RUNNING_OPS: AtomicUsize = AtomicUsize::new(0);

pub fn running_operations() -> usize {
    RUNNING_OPS.load(Ordering::SeqCst)
}

/// Counts one running operation for as long as it lives.
pub(crate) struct OpGuard;
impl OpGuard {
    pub(crate) fn new() -> OpGuard {
        RUNNING_OPS.fetch_add(1, Ordering::SeqCst);
        OpGuard
    }
}
impl Drop for OpGuard {
    fn drop(&mut self) {
        RUNNING_OPS.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The list's drop target, for tests that simulate a drop.
pub fn drop_target() -> Option<windows::Win32::System::Ole::IDropTarget> {
    with_app(|a| a.drop_target.clone()).flatten()
}

/// The settings captured when the main window closed on this thread.
pub fn take_final_settings() -> Option<Settings> {
    FINAL_SETTINGS.with(|s| s.borrow_mut().take())
}

/// Runs `f` on the app state; `None` if there is none yet or it is already borrowed.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => app.as_mut().map(f),
        Err(_) => {
            crash::log("re-entrant access to UI state");
            None
        }
    })
}

/// Process-wide setup. Call on each UI thread before creating windows.
pub fn init_process() -> Result<()> {
    crash::install();
    unsafe {
        // No "insert a disk" dialogs when touching empty card readers / DVD drives.
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX);
        SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)?;
        OleInitialize(None)?;
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES
                | ICC_LISTVIEW_CLASSES
                | ICC_BAR_CLASSES
                | ICC_TREEVIEW_CLASSES
                | ICC_TAB_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    Ok(())
}

unsafe fn child(
    parent: HWND,
    class: PCWSTR,
    text: impl windows::core::Param<PCWSTR>,
    style: WINDOW_STYLE,
    ex: WINDOW_EX_STYLE,
    id: usize,
) -> Result<HWND> {
    unsafe {
        CreateWindowExW(
            ex,
            class,
            text,
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            Some(parent),
            Some(HMENU(id as _)),
            Some(GetModuleHandleW(None)?.into()),
            None,
        )
    }
}

pub fn create_main_window(start: &Path, settings: &Settings) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            // Resource 1 from res/app.rc; absent in test binaries, which is fine.
            hIcon: LoadIconW(Some(hinstance), PCWSTR(1 as _)).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as _),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly if already registered
        // A saved position on a monitor that's no longer there would open the window off-screen.
        // Fit a saved position onto the nearest monitor: screens change between runs.
        let (x, y, w, h) = settings
            .window
            .map(|(x, y, w, h)| {
                let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
                let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
                let monitor = MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST);
                if GetMonitorInfoW(monitor, &mut info).as_bool() {
                    let wa = info.rcWork;
                    crate::settings::fit_window((x, y, w, h), (wa.left, wa.top, wa.right, wa.bottom))
                } else {
                    (x, y, w, h)
                }
            })
            .unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT, 1100, 700));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("Kestrel"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinstance),
            None,
        )?;

        let none = WINDOW_EX_STYLE::default();
        let button = |text, id| child(hwnd, WC_BUTTONW, text, WS_TABSTOP | WS_DISABLED, none, id);
        let buttons = [button(w!("←"), ID_BACK)?, button(w!("→"), ID_FORWARD)?, button(w!("↑"), ID_UP)?];
        let address = child(
            hwnd,
            WC_EDITW,
            w!(""),
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            0,
        )?;
        let filter = child(
            hwnd,
            WC_EDITW,
            w!(""),
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            0,
        )?;
        // Grey hint text while empty; shown even when focused.
        SendMessageW(
            filter,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(w!("Filter  (Enter: search subfolders)").0 as isize)),
        );
        let list = child(
            hwnd,
            WC_LISTVIEWW,
            w!(""),
            WS_TABSTOP
                | WINDOW_STYLE(
                    LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS | LVS_EDITLABELS,
                ),
            none,
            0,
        )?;
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(ex as usize)),
            Some(LPARAM(ex as isize)),
        );
        let _ = SetWindowTheme(list, w!("Explorer"), None);
        for (i, (title, width, key)) in COLUMNS.iter().enumerate() {
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if *key == SortKey::Size { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                cx: settings.columns.get(i).copied().unwrap_or_else(|| scale(hwnd, *width)),
                pszText: PWSTR(title.0 as *mut _),
                ..Default::default()
            };
            SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }
        let status = child(hwnd, STATUSCLASSNAMEW, w!(""), WINDOW_STYLE(SBARS_SIZEGRIP), none, 0)?;
        let tree = tree::create(hwnd)?;
        let tab_strip = tabbar::create(hwnd)?;
        let preview_pane = preview::create(hwnd)?;
        let toolbar_buttons = toolbar::create(hwnd)?;

        let font = message_font(hwnd);
        set_font(&[list, status, address, filter, tree, tab_strip, buttons[0], buttons[1], buttons[2]], font);
        set_font(&preview::controls(&preview_pane), font);
        set_font(&toolbar_buttons, font);

        let target = hwnd.0 as isize;
        let events = Arc::new(Events::new(move || {
            PostMessageW(Some(HWND(target as _)), WM_APP_EVENTS, WPARAM(0), LPARAM(0)).is_ok()
        }));
        startup_mark("controls");
        let (icon_jobs, icon_rx) = mpsc::channel();
        let icons = Icons::new(icon_jobs, 0); // folder icon known after `late_init`
        let icon_events = events.clone();
        // The worker stops when `icons` (the only sender) is dropped with the window.
        if let Err(e) =
            icons::spawn_worker(icon_rx, icons.latest.clone(), move |d| icon_events.send(Event::Icon(d)))
        {
            crash::log(&format!("icons disabled: {e}"));
        }
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                list,
                address,
                buttons,
                toolbar: toolbar_buttons,
                startup_override: None,
                status,
                font,
                browser: {
                    let mut b = Browser::default();
                    (b.sort, b.show_hidden) = (settings.sort, settings.show_hidden);
                    b
                },
                events,
                lister: None,
                lister_cancel: Arc::default(),
                tabs: Tabs::new(tabbar::TabSlot::active(0)),
                tab_strip,
                next_tab_id: 1,
                theme: settings.theme,
                palette: Palette::new(false),
                brushes: None,
                preview: preview_pane,
                filter,
                live_listers: Arc::new(AtomicUsize::new(0)),
                tree,
                nodes: Vec::new(),
                split: settings.split.unwrap_or_else(|| scale(hwnd, 240)),
                drag_offset: None,
                icons,
                watcher: None,
                watch_ids: 0,
                device_note: None,
                this_pc: 0,
                edit_pending: None,
                renaming: None,
                resync_after_rename: false,
                drop_row: None,
                drop_target: None,
            })
        });

        // Drops from other apps (and from this list onto its own folders).
        let target = drag::DropTarget::create(list);
        if let Err(e) = windows::Win32::System::Ole::RegisterDragDrop(list, &target) {
            crash::log(&format!("drag and drop into the list disabled: {e}"));
        }
        with_app(|a| a.drop_target = Some(target));
        // Owner-data lists keep only selection and focus; the drop highlight comes from `fill`.
        SendMessageW(list, LVM_SETCALLBACKMASK, Some(WPARAM(LVIS_DROPHILITED.0 as usize)), None);
        let _ = windows::Win32::System::DataExchange::AddClipboardFormatListener(hwnd);
        layout(hwnd);
        update_sort_arrows();
        preview::restore(hwnd, settings.preview);
        let _ = SetFocus(Some(list));
        go(start.to_path_buf());
        // Icons and the sidebar wait until the window has painted once.
        dark::apply(hwnd, Palette::new(settings.theme.is_dark(system_prefers_dark())));
        let _ = PostMessageW(Some(hwnd), WM_APP_LATE_INIT, WPARAM(0), LPARAM(0));
        Ok(hwnd)
    }
}

pub fn run_message_loop() {
    let mut msg = MSG::default();
    unsafe {
        // GetMessageW returns -1 on error, 0 on WM_QUIT.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if !translate(&msg) {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

/// App-wide keyboard shortcuts, checked before a message is dispatched.
/// Returns true if the message was consumed. Call from every message loop.
pub fn translate(msg: &MSG) -> bool {
    if dialogs::translate(msg) {
        return true;
    }
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    crash::guard(|| on_key(msg)).unwrap_or(false)
}

fn on_key(msg: &MSG) -> bool {
    let Some((list, address, tree, filter)) = with_app(|a| (a.list, a.address, a.tree, a.filter)) else {
        return false;
    };
    if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_F1.0 as usize {
        if let Some(hwnd) = with_app(|a| a.hwnd) {
            dialogs::shortcuts(hwnd);
        }
        return true;
    }
    if ![list, address, tree, filter].contains(&msg.hwnd) && !toolbar::owns(msg.hwnd) {
        return false;
    }
    let key = VIRTUAL_KEY(msg.wParam.0 as u16);
    let in_address = msg.hwnd == address;
    let in_list = msg.hwnd == list;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    if msg.message == WM_SYSKEYDOWN {
        match key {
            VK_LEFT => run(Browser::back),
            VK_RIGHT => run(Browser::forward),
            VK_UP => run(Browser::up),
            VK_D => focus_address(),
            VK_P => preview::toggle(with_app(|a| a.hwnd).unwrap_or_default()),
            _ => return false,
        }
        return true;
    }
    match key {
        VK_BACK if !in_address && msg.hwnd != filter => run(Browser::back),
        VK_F2 if in_list => _ = commands::run(commands::CMD_RENAME),
        VK_DELETE if in_list && shift => _ = commands::run(commands::CMD_DELETE_PERMANENTLY),
        VK_DELETE if in_list => _ = commands::run(commands::CMD_DELETE),
        VK_C if in_list && ctrl && shift => _ = commands::run(commands::CMD_COPY_PATH),
        VK_C if in_list && ctrl => _ = commands::run(commands::CMD_COPY),
        VK_X if in_list && ctrl => _ = commands::run(commands::CMD_CUT),
        VK_V if in_list && ctrl => _ = commands::run(commands::CMD_PASTE),
        VK_A if in_list && ctrl => _ = commands::run(commands::CMD_SELECT_ALL),
        VK_N if !in_address && ctrl && shift => _ = commands::run(commands::CMD_NEW_FOLDER),
        VK_F5 => run(Browser::refresh),
        VK_F4 => focus_address(),
        VK_F if ctrl => unsafe {
            let _ = SetFocus(Some(filter));
            SendMessageW(filter, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
        },
        VK_RETURN if msg.hwnd == filter => {
            let text = window_text(filter);
            run(|b| b.search(&text));
            unsafe {
                let _ = SetFocus(Some(list));
            }
        }
        VK_ESCAPE if msg.hwnd == filter => unsafe {
            let _ = SetWindowTextW(filter, w!("")); // EN_CHANGE clears the filter
            let _ = SetFocus(Some(list));
        },
        // Keys with their own meaning in a text box.
        VK_BACK | VK_DELETE | VK_A | VK_C | VK_X | VK_V | VK_N | VK_H if msg.hwnd == filter => return false,
        VK_L if ctrl => focus_address(),
        VK_H if ctrl && !in_address => {
            if let Some(job) = with_app(|a| a.browser.toggle_hidden()).flatten() {
                spawn_sort(job);
            }
        }
        VK_RETURN if in_address => {
            let text = window_text(address);
            let current = with_app(|a| a.browser.dir().map(Path::to_path_buf)).flatten();
            match parse_location(&text, current.as_deref()) {
                Some(dir) => {
                    go(dir);
                    unsafe {
                        let _ = SetFocus(Some(list));
                    }
                }
                None => set_status(&format!("Not a location: {text}")),
            }
        }
        VK_ESCAPE if in_address => {
            show_location();
            unsafe {
                let _ = SetFocus(Some(list));
            }
        }
        VK_ESCAPE => cancel_loading(),
        VK_T if ctrl => tabbar::new_tab(None),
        VK_N if ctrl && !shift => open_side_by_side(with_app(|a| a.hwnd).unwrap_or_default()),
        VK_W if ctrl => tabbar::close_active(),
        VK_TAB if ctrl => tabbar::cycle(shift_held()),
        VK_TAB => unsafe {
            // Focus order: address bar, sidebar, list.
            let mut order = vec![address, filter];
            order.extend(
                with_app(|a| a.toolbar.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|&h| IsWindowVisible(h).as_bool() && IsWindowEnabled(h).as_bool()),
            );
            order.extend([tree, list]);
            let at = order.iter().position(|&h| h == msg.hwnd).unwrap_or(0);
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let next = if shift { (at + order.len() - 1) % order.len() } else { (at + 1) % order.len() };
            let _ = SetFocus(Some(order[next]));
        },
        _ => return false,
    }
    true
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crash::guard(|| handle(hwnd, msg, wp, lp)).unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

fn handle(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_APP_LATE_INIT => {
                late_init();
                LRESULT(0)
            }
            WM_APP_EVENTS => {
                for ev in with_app(|a| a.events.drain()).unwrap_or_default() {
                    apply(ev);
                }
                LRESULT(0)
            }
            WM_NOTIFY => {
                on_notify(&*(lp.0 as *const NMHDR), lp).unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
            }
            WM_COMMAND => {
                // Only clicks on our buttons and menu picks. Edit controls (the address
                // bar, the rename box) also send WM_COMMAND, with ids that can collide.
                let (id, code, from) = (wp.0 & 0xFFFF, (wp.0 >> 16) & 0xFFFF, lp.0);
                // Typing in the filter box narrows the list at once.
                if code == EN_CHANGE as usize && with_app(|a| a.filter.0 as isize) == Some(from) {
                    let text = window_text(HWND(from as _));
                    if let Some(job) = with_app(|a| a.browser.set_filter(&text)).flatten() {
                        spawn_sort(job);
                    }
                    update_chrome();
                    return LRESULT(0);
                }
                let from_button =
                    with_app(|a| a.buttons.iter().chain(a.toolbar.iter()).any(|b| b.0 as isize == from))
                        == Some(true);
                let ours = (from == 0 && code == 0) || (from_button && code == BN_CLICKED as usize);
                if !ours {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                match id {
                    ID_BACK if from_button => run(Browser::back),
                    ID_FORWARD if from_button => run(Browser::forward),
                    ID_UP if from_button => run(Browser::up),
                    id if (from == 0 || from_button) && (toolbar::execute(id) || commands::run(id)) => {}
                    _ => return DefWindowProcW(hwnd, msg, wp, lp),
                }
                if !matches!(id, toolbar::SETTINGS | toolbar::SHORTCUTS | toolbar::HELP)
                    && let Some(list) = with_app(|a| a.list)
                {
                    let _ = SetFocus(Some(list));
                }
                LRESULT(0)
            }
            // Mouse back/forward buttons and keyboard browser keys arrive here.
            WM_APPCOMMAND => {
                match APPCOMMAND_ID(((lp.0 >> 16) as u32) & !FAPPCOMMAND_MASK & 0xFFFF) {
                    APPCOMMAND_BROWSER_BACKWARD => run(Browser::back),
                    APPCOMMAND_BROWSER_FORWARD => run(Browser::forward),
                    APPCOMMAND_BROWSER_REFRESH => run(Browser::refresh),
                    _ => return DefWindowProcW(hwnd, msg, wp, lp),
                }
                LRESULT(1)
            }
            WM_CTLCOLOREDIT => {
                dark::edit_colors(HDC(wp.0 as _)).unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
            }
            WM_ERASEBKGND if dark::erase(hwnd, HDC(wp.0 as _)) => LRESULT(1),
            // Windows' light/dark app setting changed.
            WM_SETTINGCHANGE => {
                let area = if lp.0 == 0 {
                    String::new()
                } else {
                    PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default()
                };
                if area == "ImmersiveColorSet"
                    && let Some(choice) = with_app(|a| a.theme)
                {
                    dark::apply(hwnd, Palette::new(choice.is_dark(system_prefers_dark())));
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SIZE => {
                layout(hwnd);
                LRESULT(0)
            }
            // The only bare client area below the toolbar is the splitter gap.
            WM_LBUTTONDOWN => {
                if over_splitter(hwnd, lp) {
                    let x = (lp.0 & 0xFFFF) as i16 as i32;
                    with_app(|a| a.drag_offset = Some(x - a.split));
                    SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some(offset) = with_app(|a| a.drag_offset).flatten() {
                    let mut client = RECT::default();
                    let _ = GetClientRect(hwnd, &mut client);
                    let x = (lp.0 & 0xFFFF) as i16 as i32;
                    let dpi = GetDpiForWindow(hwnd);
                    with_app(|a| a.split = tree::clamp_split(x - offset, client.right, dpi));
                    layout(hwnd);
                }
                LRESULT(0)
            }
            WM_LBUTTONUP => {
                let _ = ReleaseCapture();
                LRESULT(0)
            }
            WM_CAPTURECHANGED => {
                with_app(|a| a.drag_offset = None);
                LRESULT(0)
            }
            WM_SETCURSOR if wp.0 == hwnd.0 as usize => {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = ScreenToClient(hwnd, &mut pt);
                let at = LPARAM(((pt.y as isize & 0xFFFF) << 16) | (pt.x as isize & 0xFFFF));
                if over_splitter(hwnd, at) {
                    SetCursor(LoadCursorW(None, IDC_SIZEWE).ok());
                    return LRESULT(1);
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_SETFOCUS => {
                if let Some(list) = with_app(|a| a.list) {
                    let _ = SetFocus(Some(list));
                }
                LRESULT(0)
            }
            WM_DPICHANGED => {
                let r = &*(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                let font = message_font(hwnd);
                let new_dpi = (wp.0 & 0xFFFF) as i32;
                if let Some((old, controls)) = with_app(|a| {
                    let old_dpi = GetDpiForWindow(hwnd) as i32;
                    a.split = a.split * new_dpi / old_dpi.max(1);
                    (std::mem::replace(&mut a.font, font), a.controls())
                }) {
                    set_font(&controls, font);
                    let _ = DeleteObject(HGDIOBJ(old.0));
                }
                layout(hwnd);
                LRESULT(0)
            }
            WM_CONTEXTMENU if with_app(|a| a.list.0 as usize) == Some(wp.0) => {
                commands::context_menu(hwnd, lp);
                LRESULT(0)
            }
            WM_CLIPBOARDUPDATE => {
                toolbar::update();
                LRESULT(0)
            }
            WM_DEVICECHANGE => {
                on_device_change(wp.0 as u32, lp);
                LRESULT(1)
            }
            WM_TIMER if preview::on_timer(hwnd, wp.0) => LRESULT(0),
            WM_PAINT => {
                log_first_paint();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DESTROY => {
                let _ = windows::Win32::System::DataExchange::RemoveClipboardFormatListener(hwnd);
                preview::shutdown();
                if let Some(list) = with_app(|a| a.list) {
                    let _ = windows::Win32::System::Ole::RevokeDragDrop(list);
                }
                capture_settings(hwnd);
                stop_watching();
                if let Some(app) = APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut a| a.take())) {
                    let _ = DeleteObject(HGDIOBJ(app.font.0));
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// `None` = not handled, let DefWindowProc run.
unsafe fn on_notify(hdr: &NMHDR, lp: LPARAM) -> Option<LRESULT> {
    let (list, tree, strip) = with_app(|a| (a.list, a.tree, a.tab_strip))?;
    if hdr.hwndFrom == strip {
        return tabbar::on_notify(hdr);
    }
    if hdr.hwndFrom == tree {
        return unsafe { tree::on_notify(hdr, lp) };
    }
    if hdr.hwndFrom != list {
        return None;
    }
    unsafe {
        match hdr.code {
            LVN_GETDISPINFOW => {
                let item = &mut (*(lp.0 as *mut NMLVDISPINFOW)).item;
                with_app(|a| fill(&a.browser, &mut a.icons, a.drop_row, item));
                Some(LRESULT(0))
            }
            LVN_ODFINDITEMW => {
                let find = &*(lp.0 as *const NMLVFINDITEMW);
                if !find.lvfi.flags.contains(LVFI_STRING) || find.lvfi.psz.is_null() {
                    return Some(LRESULT(-1));
                }
                let prefix = find.lvfi.psz.as_wide();
                let row = with_app(|a| a.browser.find_prefix(find.iStart.max(0) as usize, prefix)).flatten();
                Some(LRESULT(row.map_or(-1, |r| r as isize)))
            }
            LVN_COLUMNCLICK => {
                let col = (*(lp.0 as *const NMLISTVIEW)).iSubItem as usize;
                if let Some((_, _, key)) = COLUMNS.get(col) {
                    if let Some(job) =
                        with_app(|a| a.browser.set_sort(a.browser.sort_for_column(*key))).flatten()
                    {
                        spawn_sort(job);
                    }
                    update_sort_arrows();
                }
                Some(LRESULT(0))
            }
            LVN_ITEMCHANGED | LVN_ODSTATECHANGED => {
                toolbar::update();
                if let Some(main) = with_app(|a| a.hwnd) {
                    preview::selection_changed(main);
                }
                Some(LRESULT(0))
            }
            LVN_BEGINDRAG | LVN_BEGINRDRAG => {
                drag::begin_drag();
                Some(LRESULT(0))
            }
            LVN_BEGINLABELEDITW => Some(commands::begin_rename(lp)),
            LVN_ENDLABELEDITW => Some(commands::end_rename(lp)),
            LVN_ITEMACTIVATE => {
                let row = (*(lp.0 as *const NMITEMACTIVATE)).iItem;
                let target =
                    usize::try_from(row).ok().and_then(|r| with_app(|a| a.browser.path_at(r)).flatten());
                match target {
                    Some((dir, true)) => go(dir),
                    Some((file, false)) => open_file(file),
                    None => {}
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    }
}

/// Answers the list's request for a row's text and/or icon.
fn fill(b: &Browser, icons: &mut Icons, drop_row: Option<usize>, item: &mut LVITEMW) {
    let (Some(s), Ok(row)) = (b.snapshot(), usize::try_from(item.iItem)) else { return write(item, &[]) };
    let Some(&entry) = b.order.get(row) else { return write(item, &[]) };
    let i = entry as usize;
    if item.mask.contains(LVIF_STATE) && Some(row) == drop_row {
        item.state.0 |= LVIS_DROPHILITED.0;
        item.stateMask.0 |= LVIS_DROPHILITED.0;
    }
    if item.mask.contains(LVIF_IMAGE) {
        item.iImage = icons.icon(b.shown_generation(), entry, s.name(i), s.is_dir(i), &s.dir);
    }
    let wide = |t: String| t.encode_utf16().collect::<Vec<_>>();
    match COLUMNS.get(item.iSubItem as usize).map(|c| c.2) {
        Some(SortKey::Name) => write(item, s.name(i)),
        Some(SortKey::Modified) => write(item, &wide(format::time(s.entries[i].modified))),
        Some(SortKey::Type) => {
            write(item, &wide(icons.type_name(s.name(i), s.is_dir(i)).unwrap_or_default()))
        }
        Some(SortKey::Size) if !s.is_dir(i) => write(item, &wide(format::size(s.entries[i].size))),
        _ => write(item, &[]),
    }
}

/// Copies `text` into the control's buffer, truncating to fit. Only when the
/// control asked for text: otherwise `pszText` isn't ours to write.
fn write(item: &mut LVITEMW, text: &[u16]) {
    if !item.mask.contains(LVIF_TEXT) || item.pszText.is_null() || item.cchTextMax <= 0 {
        return;
    }
    let n = text.len().min(item.cchTextMax as usize - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), item.pszText.0, n);
        *item.pszText.0.add(n) = 0;
    }
}

fn apply(ev: Event) {
    match ev {
        Event::Listed { generation, result } if tabbar::is_background(generation) => {
            tabbar::route_listed(generation, result)
        }
        Event::Listed { generation, result } => {
            let sel = selected_rows();
            let applied = with_app(|a| {
                let r = a.browser.on_listed(generation, result, &sel);
                if r != Applied::Stale {
                    a.lister = None;
                }
                r
            });
            match applied {
                Some(Applied::Shown { select, new_folder }) => {
                    refresh(&select, new_folder);
                    if new_folder {
                        watch_current();
                    }
                    commands::after_refresh();
                    if let Some(job) = with_app(|a| a.browser.sort_job()).flatten() {
                        spawn_sort(job);
                    }
                }
                // The very first folder couldn't be opened (gone since last run?): show the user folder.
                Some(Applied::Failed) if with_app(|a| a.browser.dir().is_none()) == Some(true) => {
                    update_chrome();
                    if let Some(home) = std::env::var_os("USERPROFILE") {
                        go(PathBuf::from(home));
                    }
                }
                Some(Applied::Failed) => update_chrome(),
                Some(Applied::Gone(job)) => spawn_list(job),
                _ => {}
            }
            if let Some(job) = with_app(|a| a.browser.follow_up()).flatten() {
                spawn_list(job);
            }
        }
        Event::DirChanged(id) => {
            if let Some(job) = with_app(|a| {
                (a.watcher.as_ref().map(|w| w.0) == Some(id)).then(|| a.browser.on_dir_changed())
            })
            .flatten()
            .flatten()
            {
                spawn_list(job);
            }
        }
        Event::WatchReady { id, handle } => on_watch_ready(id, handle),
        Event::OpDone { result, edit } => commands::on_op_done(result, edit),
        Event::Resync => refresh(&selected_rows(), false),
        Event::Sorted { generation, sort, show_hidden, filter, order } => {
            let sel = selected_rows();
            if let Some(Applied::Shown { select, .. }) =
                with_app(|a| a.browser.on_sorted(generation, sort, show_hidden, &filter, order, &sel))
            {
                refresh(&select, false);
            }
        }
        Event::Navigate(dir) => go(dir),
        Event::Message(text) => set_status(&text),
        Event::Preview { token, shown } => preview::finish(token, shown),
        Event::SearchProgress(found) => {
            if with_app(|a| a.browser.is_loading()) == Some(true) {
                set_status(&format!("Searching… {found} found so far  (Esc to stop)"));
            }
        }
        Event::TreeChildren { node, result } => tree::on_children(node, result),
        Event::DriveLabel { node, text } => tree::on_label(node, text),
        Event::Icon(Done::Node { node, icon }) => tree::on_icon(node, icon),
        Event::Icon(done) => {
            if let Some((list, true)) = with_app(|a| (a.list, a.icons.on_done(done))) {
                unsafe {
                    let _ = InvalidateRect(Some(list), None, false);
                }
            }
        }
    }
}

/// Runs a browser navigation (`Browser::back`, `up`, ...) and lists its target.
fn run(nav: impl FnOnce(&mut Browser) -> Option<ListJob>) {
    if with_app(|a| a.live_listers.load(Ordering::SeqCst) >= MAX_LISTERS) == Some(true) {
        return set_status("Too many folders still loading (network?). Wait a moment and try again.");
    }
    if let Some(job) = with_app(|a| nav(&mut a.browser)).flatten() {
        spawn_list(job);
    }
}

fn go(dir: PathBuf) {
    run(|b| Some(b.go(dir)));
}

fn spawn_list(job: ListJob) {
    // A new listing supersedes any search still running.
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let Some((events, live)) = with_app(|a| {
        std::mem::replace(&mut a.lister_cancel, cancel.clone()).store(true, Ordering::SeqCst);
        (a.events.clone(), a.live_listers.clone())
    }) else {
        return;
    };
    live.fetch_add(1, Ordering::SeqCst);
    let worker = spawn("list", move || {
        struct Done(Arc<AtomicUsize>);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _done = Done(live);
        let ListJob { generation, dir, query, sort, show_hidden, filter } = job;
        let result = crash::guard(|| {
            let snap = match &query {
                Some(q) => {
                    let mut last = std::time::Instant::now();
                    let mut progress = |found: usize| {
                        if last.elapsed() > std::time::Duration::from_millis(250) {
                            last = std::time::Instant::now();
                            events.send(Event::SearchProgress(found));
                        }
                    };
                    search::search(&dir, q, show_hidden, &|| cancel.load(Ordering::SeqCst), &mut progress)
                }
                None => crate::network::list_location(&dir),
            };
            snap.map(|s| {
                let order = filter_order(&s, &sorted_order(&s, sort, show_hidden), &filter);
                (s, order)
            })
        })
        .unwrap_or_else(|| Err(io::Error::other("internal error while listing")));
        events.send(Event::Listed { generation, result });
    });
    if worker.is_none() {
        with_app(|a| a.live_listers.fetch_sub(1, Ordering::SeqCst));
    }
    with_app(|a| a.lister = worker);
    update_chrome();
}

/// Startup work that can wait for the first paint: the shell's icon list
/// (~20 ms the first time) and the sidebar.
fn late_init() {
    let Some((list, tree)) = with_app(|a| (a.list, a.tree)) else { return };
    let (image_list, folder_icon) = icons::system_image_list().unwrap_or_default();
    startup_mark("image list");
    if !image_list.is_invalid() {
        unsafe {
            SendMessageW(
                list,
                LVM_SETIMAGELIST,
                Some(WPARAM(LVSIL_SMALL as usize)),
                Some(LPARAM(image_list.0)),
            );
            SendMessageW(
                tree,
                TVM_SETIMAGELIST,
                Some(WPARAM(TVSIL_NORMAL as usize)),
                Some(LPARAM(image_list.0)),
            );
        }
    }
    with_app(|a| a.icons.folder = folder_icon);
    let this_pc = tree::populate();
    startup_mark("sidebar");
    with_app(|a| a.this_pc = this_pc);
    unsafe {
        let _ = InvalidateRect(Some(list), None, false);
    }
}

/// Records the view state for saving once the window is gone.
fn capture_settings(hwnd: HWND) {
    if let Some(s) = current_settings(hwnd) {
        FINAL_SETTINGS.with(|f| *f.borrow_mut() = Some(s));
    }
}

/// The window's view state as settings.
fn current_settings(hwnd: HWND) -> Option<Settings> {
    let (list, mut s) = with_app(|a| {
        let s = Settings {
            sort: a.browser.sort,
            show_hidden: a.browser.show_hidden,
            split: Some(a.split),
            preview: preview::is_visible_in(a),
            last_folder: a.startup_override.clone().unwrap_or_else(|| a.browser.dir().map(Path::to_path_buf)),
            theme: a.theme,
            ..Default::default()
        };
        (a.list, s)
    })?;
    unsafe {
        let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        if GetWindowPlacement(hwnd, &mut wp).is_ok() {
            let r = wp.rcNormalPosition;
            s.window = Some((r.left, r.top, r.right - r.left, r.bottom - r.top));
            s.maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32;
        }
        s.columns = (0..COLUMNS.len())
            .map(|i| SendMessageW(list, LVM_GETCOLUMNWIDTH, Some(WPARAM(i)), None).0 as i32)
            .collect();
    }
    Some(s)
}

/// Windows opened with Ctrl+N, each on its own UI thread with its own state.
static OTHER_WINDOWS: std::sync::Mutex<Vec<JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Waits until every window opened with Ctrl+N has been closed.
pub fn wait_for_other_windows() {
    loop {
        let next = OTHER_WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).pop();
        match next {
            Some(h) => {
                let _ = h.join();
            }
            None => return,
        }
    }
}

/// Ctrl+N: a second window on the same folder, the two tiled side by side.
fn open_side_by_side(hwnd: HWND) {
    let Some(mut s) = current_settings(hwnd) else { return };
    let start = s.last_folder.clone().unwrap_or_else(|| PathBuf::from(r"C:\"));
    unsafe {
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            let w = info.rcWork;
            let half = (w.right - w.left) / 2;
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetWindowPos(hwnd, None, w.left, w.top, half, w.bottom - w.top, SWP_NOZORDER);
            s.window = Some((w.left + half, w.top, w.right - w.left - half, w.bottom - w.top));
            s.maximized = false;
        }
    }
    let spawned = std::thread::Builder::new().name("window".into()).spawn(move || {
        crash::guard(|| {
            if init_process().is_err() {
                return;
            }
            if let Ok(w) = create_main_window(&start, &s) {
                unsafe {
                    let _ = ShowWindow(w, SW_SHOW);
                }
                run_message_loop();
            }
        });
    });
    match spawned {
        Ok(h) => OTHER_WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).push(h),
        Err(e) => set_status(&format!("Couldn't open a new window: {e}")),
    }
}

/// Logs how long after process start the window first painted.
fn log_first_paint() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !DONE.swap(true, Ordering::SeqCst) {
        startup_mark("first paint");
        let marks = STARTUP_MARKS.lock().map(|m| m.join(", ")).unwrap_or_default();
        crash::log(&format!("startup (ms after process start): {marks}"));
    }
}

static STARTUP_MARKS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Notes how long after process creation `what` happened; logged at first paint.
pub fn startup_mark(what: &str) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let ticks = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    let (mut created, mut x1, mut x2, mut x3) = Default::default();
    unsafe {
        if GetProcessTimes(GetCurrentProcess(), &mut created, &mut x1, &mut x2, &mut x3).is_ok()
            && let Ok(mut marks) = STARTUP_MARKS.lock()
        {
            let ms = (ticks(GetSystemTimeAsFileTime()) - ticks(created)) / 10_000;
            marks.push(format!("{what} {ms}"));
        }
    }
}

/// Starts watching the folder on screen, replacing any previous watcher.
fn watch_current() {
    stop_watching();
    let Some((id, dir, events)) = with_app(|a| {
        a.watch_ids += 1;
        Some((a.watch_ids, a.browser.dir()?.to_path_buf(), a.events.clone()))
    })
    .flatten() else {
        return;
    };
    let ready = events.clone();
    let watcher = Watcher::start(
        dir,
        move || events.send(Event::DirChanged(id)),
        move |handle| ready.send(Event::WatchReady { id, handle }),
    );
    with_app(|a| a.watcher = Some((id, watcher)));
}

/// Stops the watcher (it closes the folder on its own thread) and drops its device notification.
fn stop_watching() {
    let Some((watcher, note)) = with_app(|a| (a.watcher.take(), a.device_note.take())) else { return };
    drop(watcher);
    if let Some((note, _)) = note {
        unsafe {
            let _ = UnregisterDeviceNotification(HDEVNOTIFY(note as _));
        }
    }
}

fn on_watch_ready(id: u64, handle: isize) {
    let Some(hwnd) = with_app(|a| (a.watcher.as_ref().map(|w| w.0) == Some(id)).then_some(a.hwnd)).flatten()
    else {
        return;
    };
    // Lets us close the folder when someone asks to eject its drive.
    let filter = DEV_BROADCAST_HANDLE {
        dbch_size: size_of::<DEV_BROADCAST_HANDLE>() as u32,
        dbch_devicetype: DBT_DEVTYP_HANDLE.0,
        dbch_handle: HANDLE(handle as _),
        ..Default::default()
    };
    let note = unsafe {
        RegisterDeviceNotificationW(
            HANDLE(hwnd.0),
            &filter as *const _ as *const _,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
    };
    if let Ok(note) = note {
        with_app(|a| a.device_note = Some((note.0 as isize, handle)));
    }
}

fn on_device_change(event: u32, lp: LPARAM) {
    if lp.0 == 0 {
        return;
    }
    let hdr = unsafe { &*(lp.0 as *const DEV_BROADCAST_HDR) };
    if hdr.dbch_devicetype == DBT_DEVTYP_HANDLE {
        let handle = unsafe { (*(lp.0 as *const DEV_BROADCAST_HANDLE)).dbch_handle.0 as isize };
        if with_app(|a| a.device_note.map(|n| n.1) == Some(handle)) != Some(true) {
            return;
        }
        match event {
            DBT_DEVICEQUERYREMOVE => {
                // The eject goes ahead only if nobody holds a handle on the drive: close ours now.
                if let Some((_, w)) = with_app(|a| a.watcher.take()).flatten() {
                    w.stop_and_wait(std::time::Duration::from_millis(500));
                }
            }
            DBT_DEVICEREMOVECOMPLETE => {
                stop_watching();
                if let Some(job) = with_app(|a| a.browser.on_dir_changed()).flatten() {
                    spawn_list(job);
                }
            }
            _ => {}
        }
    } else if hdr.dbch_devicetype == DBT_DEVTYP_VOLUME
        && (event == DBT_DEVICEARRIVAL || event == DBT_DEVICEREMOVECOMPLETE)
    {
        tree::refresh_drives();
    }
}

fn cancel_loading() {
    with_app(|a| a.lister_cancel.store(true, Ordering::SeqCst));
    let Some(worker) = with_app(|a| a.browser.cancel().then(|| a.lister.take()).flatten()).flatten() else {
        return update_chrome();
    };
    // Wakes the thread if it's blocked in a synchronous call (e.g. a dead network path).
    // If it isn't, this fails harmlessly; either way its result is now stale.
    unsafe {
        let _ = CancelSynchronousIo(HANDLE(worker.as_raw_handle()));
    }
    update_chrome();
}

fn spawn_sort(job: SortJob) {
    let Some(events) = with_app(|a| a.events.clone()) else { return };
    spawn("sort", move || {
        let order = crash::guard(|| {
            let s = &job.snapshot;
            filter_order(s, &sorted_order(s, job.sort, job.show_hidden), &job.filter)
        });
        if let Some(order) = order {
            events.send(Event::Sorted {
                generation: job.generation,
                sort: job.sort,
                show_hidden: job.show_hidden,
                filter: job.filter,
                order,
            });
        }
    });
}

fn open_file(path: PathBuf) {
    let Some((events, hwnd)) = with_app(|a| (a.events.clone(), a.hwnd.0 as isize)) else { return };
    spawn("open", move || {
        let ev = match crash::guard(|| shell::open(&path, HWND(hwnd as _))) {
            Some(Ok(Some(dir))) => Event::Navigate(dir),
            Some(Ok(None)) => return,
            Some(Err(e)) => Event::Message(e),
            None => Event::Message(format!("Can't open {}", path.display())),
        };
        events.send(ev);
    });
}

/// Starts a worker thread. If the OS refuses, the failure is logged and shown.
fn spawn(name: &str, f: impl FnOnce() + Send + 'static) -> Option<JoinHandle<()>> {
    match std::thread::Builder::new().name(name.into()).spawn(f) {
        Ok(h) => Some(h),
        Err(e) => {
            crash::log(&format!("could not start {name} thread: {e}"));
            set_status("Out of resources: could not start a background task.");
            None
        }
    }
}

/// The focused row's item, else the first selected one.
fn focused_path() -> Option<(PathBuf, bool)> {
    let list = with_app(|a| a.list)?;
    let focused = unsafe {
        SendMessageW(
            list,
            LVM_GETNEXTITEM,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM((LVNI_FOCUSED | LVNI_SELECTED) as isize)),
        )
    }
    .0;
    let row = usize::try_from(focused).ok().or_else(|| selected_rows().first().copied())?;
    with_app(|a| a.browser.path_at(row)).flatten()
}

fn selected_rows() -> Vec<usize> {
    let Some(list) = with_app(|a| a.list) else { return Vec::new() };
    let mut rows = Vec::new();
    let mut i = -1isize;
    loop {
        i = unsafe {
            SendMessageW(
                list,
                LVM_GETNEXTITEM,
                Some(WPARAM(i as usize)),
                Some(LPARAM(LVNI_SELECTED as isize)),
            )
        }
        .0;
        if i < 0 {
            return rows;
        }
        rows.push(i as usize);
    }
}

/// Pushes the browser's contents to the list, selecting `select`.
fn refresh(select: &[usize], new_folder: bool) {
    let Some((list, count, renaming)) = with_app(|a| (a.list, a.browser.order.len(), a.renaming.is_some()))
    else {
        return;
    };
    if renaming {
        if !new_folder {
            // Resetting the list now would cancel the user's typing.
            with_app(|a| a.resync_after_rename = true);
            return;
        }
        unsafe { SendMessageW(list, LVM_CANCELEDITLABEL, None, None) };
    }
    unsafe {
        let set = |row: isize, state: u32| {
            let item = LVITEMW {
                state: LIST_VIEW_ITEM_STATE_FLAGS(state),
                stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
                ..Default::default()
            };
            SendMessageW(
                list,
                LVM_SETITEMSTATE,
                Some(WPARAM(row as usize)),
                Some(LPARAM(&item as *const _ as isize)),
            );
        };
        set(-1, 0);
        let flags = if new_folder { 0 } else { LVSICF_NOSCROLL };
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(flags as isize)));
        // ponytail: one message per selected row; slow only for "select all" in 100k+ folders.
        for &row in select {
            set(row as isize, LVIS_SELECTED.0);
        }
        if let Some(&first) = select.first() {
            set(first as isize, LVIS_SELECTED.0 | LVIS_FOCUSED.0);
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(first)), Some(LPARAM(0)));
        } else if new_folder && count > 0 {
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        let _ = InvalidateRect(Some(list), None, false);
    }
    update_chrome();
}

/// Title, address bar, button states and status text from the browser.
fn update_chrome() {
    let Some((hwnd, buttons, enabled, title, filter, filter_text)) = with_app(|a| {
        let b = &a.browser;
        let dir = b.dir().map(|d| d.display().to_string()).unwrap_or_default();
        let title = match b.query() {
            Some(q) => format!("\"{q}\" in {dir}"),
            None => dir,
        };
        (a.hwnd, a.buttons, [b.can_back(), b.can_forward(), b.can_up()], title, a.filter, b.filter())
    }) else {
        return;
    };
    // The browser drops the filter when going elsewhere: empty the box to match.
    if filter_text.is_empty() && unsafe { GetFocus() } != filter && !window_text(filter).is_empty() {
        unsafe {
            let _ = SetWindowTextW(filter, w!(""));
        }
    }
    unsafe {
        for (button, on) in buttons.iter().zip(enabled) {
            let _ = EnableWindow(*button, on);
        }
        let _ = SetWindowTextW(hwnd, &HSTRING::from(format!("{title} - Kestrel")));
    }
    toolbar::update();
    show_location();
    tabbar::update_label();
    if let Some(text) = with_app(|a| a.browser.status()) {
        set_status(&text);
    }
}

/// Puts the current folder in the address bar, unless the user is typing there.
fn show_location() {
    let Some((address, dir)) = with_app(|a| (a.address, a.browser.dir().map(Path::to_path_buf))) else {
        return;
    };
    unsafe {
        if GetFocus() != address {
            let text = dir.map(|d| d.display().to_string()).unwrap_or_default();
            let _ = SetWindowTextW(address, &HSTRING::from(text));
        }
    }
}

fn focus_address() {
    let Some(address) = with_app(|a| a.address) else { return };
    unsafe {
        let _ = SetFocus(Some(address));
        SendMessageW(address, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
    }
}

fn set_status(text: &str) {
    let Some(status) = with_app(|a| a.status) else { return };
    let text = HSTRING::from(text);
    unsafe {
        SendMessageW(status, SB_SETTEXTW, Some(WPARAM(0)), Some(LPARAM(text.as_ptr() as isize)));
    }
}

fn window_text(hwnd: HWND) -> String {
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(hwnd) as usize + 1];
        let n = GetWindowTextW(hwnd, &mut buf);
        String::from_utf16_lossy(&buf[..n as usize])
    }
}

fn update_sort_arrows() {
    let Some((list, sort)) = with_app(|a| (a.list, a.browser.sort)) else { return };
    unsafe {
        let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as _);
        for (i, (_, _, key)) in COLUMNS.iter().enumerate() {
            let mut item = HDITEMW { mask: HDI_FORMAT, ..Default::default() };
            SendMessageW(header, HDM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut item as *mut _ as isize)));
            item.fmt.0 &= !(HDF_SORTUP.0 | HDF_SORTDOWN.0);
            if *key == sort.key {
                item.fmt.0 |= if sort.descending { HDF_SORTDOWN.0 } else { HDF_SORTUP.0 };
            }
            SendMessageW(header, HDM_SETITEMW, Some(WPARAM(i)), Some(LPARAM(&item as *const _ as isize)));
        }
    }
}

fn scale(hwnd: HWND, px: i32) -> i32 {
    px * unsafe { GetDpiForWindow(hwnd) } as i32 / 96
}

fn layout(hwnd: HWND) {
    let Some((list, status, address, buttons, tree, split, filter, strip)) =
        with_app(|a| (a.list, a.status, a.address, a.buttons, a.tree, a.split, a.filter, a.tab_strip))
    else {
        return;
    };
    unsafe {
        SendMessageW(status, WM_SIZE, None, None);
        let (mut client, mut sr) = (RECT::default(), RECT::default());
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(status, &mut sr);
        let (pad, bar, bw) = (scale(hwnd, 4), scale(hwnd, 28), scale(hwnd, 32));
        for (i, b) in buttons.iter().enumerate() {
            let _ = MoveWindow(*b, pad + i as i32 * (bw + pad), pad, bw, bar, true);
        }
        let ax = pad + 3 * (bw + pad);
        let fw = scale(hwnd, 240).min((client.right / 3).max(0));
        let _ = MoveWindow(address, ax, pad, (client.right - ax - fw - 2 * pad).max(0), bar, true);
        let _ = MoveWindow(filter, client.right - fw - pad, pad, fw, bar, true);
        toolbar::layout(hwnd, client.right);
        let _ = MoveWindow(strip, 0, scale(hwnd, 68), client.right, tab_height(hwnd), true);
        let top = content_top(hwnd);
        let h = (client.bottom - (sr.bottom - sr.top) - top).max(0);
        let split = tree::clamp_split(split, client.right, GetDpiForWindow(hwnd));
        let gap = splitter_width(hwnd);
        let _ = MoveWindow(tree, 0, top, split, h, true);
        let rest = (client.right - split - gap).max(0);
        // Preview pane, when shown: a third of the space right of the sidebar.
        let preview_w =
            if preview::is_visible() { (rest / 3).max(scale(hwnd, 200)).min(rest / 2) } else { 0 };
        let _ = MoveWindow(list, split + gap, top, rest - preview_w, h, true);
        if let Some(pane) = with_app(|a| a.preview.hwnd) {
            let _ = MoveWindow(pane, client.right - preview_w, top, preview_w, h, true);
        }
    }
    if preview::is_visible() {
        preview::resize();
    }
}

fn splitter_width(hwnd: HWND) -> i32 {
    scale(hwnd, 5)
}

/// Is the client point in `lp` on the gap between sidebar and list?
fn over_splitter(hwnd: HWND, lp: LPARAM) -> bool {
    let x = (lp.0 & 0xFFFF) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xFFFF) as i16 as i32;
    let Some(split) = with_app(|a| a.split) else { return false };
    y >= content_top(hwnd) && x >= split && x < split + splitter_width(hwnd)
}

fn tab_height(hwnd: HWND) -> i32 {
    scale(hwnd, 26)
}

/// Where the sidebar and list start: below the toolbar and the tab strip.
fn content_top(hwnd: HWND) -> i32 {
    scale(hwnd, 68) + tab_height(hwnd)
}

fn shift_held() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}

fn message_font(hwnd: HWND) -> HFONT {
    unsafe {
        let mut ncm =
            NONCLIENTMETRICSW { cbSize: size_of::<NONCLIENTMETRICSW>() as u32, ..Default::default() };
        let _ = SystemParametersInfoForDpi(
            SPI_GETNONCLIENTMETRICS.0,
            ncm.cbSize,
            Some(&mut ncm as *mut _ as *mut _),
            0,
            GetDpiForWindow(hwnd),
        );
        CreateFontIndirectW(&ncm.lfMessageFont)
    }
}

fn set_font(controls: &[HWND], font: HFONT) {
    for &c in controls {
        unsafe { SendMessageW(c, WM_SETFONT, Some(WPARAM(font.0 as usize)), Some(LPARAM(1))) };
    }
}

fn apply_settings(hwnd: HWND, settings: Settings) {
    let Some((list, job)) = with_app(|a| {
        a.theme = settings.theme;
        a.split = settings.split.unwrap_or(240);
        a.startup_override = Some(settings.last_folder.clone());
        a.browser.show_hidden = settings.show_hidden;
        let job = a.browser.set_sort(settings.sort);
        (a.list, job)
    }) else {
        return;
    };
    unsafe {
        for (i, width) in settings.columns.iter().enumerate() {
            SendMessageW(list, LVM_SETCOLUMNWIDTH, Some(WPARAM(i)), Some(LPARAM(*width as isize)));
        }
        if let Some((x, y, w, h)) = settings.window {
            let r = RECT { left: x, top: y, right: x + w, bottom: y + h };
            let monitor = MonitorFromRect(&r, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
            let (x, y, w, h) = if GetMonitorInfoW(monitor, &mut info).as_bool() {
                let r = info.rcWork;
                crate::settings::fit_window((x, y, w, h), (r.left, r.top, r.right, r.bottom))
            } else {
                (x, y, w, h)
            };
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let _ = ShowWindow(hwnd, if settings.maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNOACTIVATE });
    }
    dark::apply(hwnd, Palette::new(settings.theme.is_dark(system_prefers_dark())));
    if preview::is_visible() != settings.preview {
        preview::toggle(hwnd);
    }
    if let Some(job) = job {
        spawn_sort(job);
    }
    update_sort_arrows();
    layout(hwnd);
    let Some(events) = with_app(|a| a.events.clone()) else { return };
    let saving = OpGuard::new();
    spawn("settings", move || {
        let _saving = saving;
        if let Err(e) = settings.save_to(&Settings::file()) {
            events.send(Event::Message(format!("Settings could not be saved: {e}")));
        }
    });
}
