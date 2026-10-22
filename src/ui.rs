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
use windows::core::{HSTRING, PCWSTR, Result, w};

use crate::browser::{Applied, Browser, ListJob, SortJob, parse_location};
use crate::events::Events;
use crate::icons::{self, Done};
use crate::listing::Snapshot;
use crate::search;
use crate::settings::Settings;
use crate::sort::{Sort, SortKey, filter_order, sorted_order};
use crate::tabs::Tabs;
use crate::theme::{Palette, ThemeChoice, system_prefers_dark};
use crate::watch::Watcher;
use crate::{crash, format, shell};

mod breadcrumbs;
mod chrome;
mod commands;
mod dark;
mod dialogs;
mod drag;
mod grid;
mod groups;
mod panes;
mod preview;
mod rename;
mod tabbar;
pub mod toolbar;
pub use commands::{
    CMD_COPY, CMD_COPY_PATH, CMD_COPY_TO_OTHER, CMD_CUT, CMD_DELETE, CMD_DELETE_PERMANENTLY,
    CMD_MORE_OPTIONS, CMD_MOVE_TO_OTHER, CMD_NEW_FOLDER, CMD_OPEN, CMD_OPEN_NEW_TAB, CMD_OPEN_OTHER_PANE,
    CMD_PASTE, CMD_PIN, CMD_RENAME, CMD_SELECT_ALL, CMD_SHOW_IN_EXPLORER,
};
mod tree;

/// Command names for keys.txt; each runs exactly like its menu item.
const COMMAND_NAMES: &[(&str, usize)] = &[
    ("open", commands::CMD_OPEN),
    ("cut", commands::CMD_CUT),
    ("copy", commands::CMD_COPY),
    ("paste", commands::CMD_PASTE),
    ("rename", commands::CMD_RENAME),
    ("delete", commands::CMD_DELETE),
    ("delete-permanently", commands::CMD_DELETE_PERMANENTLY),
    ("new-folder", commands::CMD_NEW_FOLDER),
    ("copy-path", commands::CMD_COPY_PATH),
    ("show-in-explorer", commands::CMD_SHOW_IN_EXPLORER),
    ("select-all", commands::CMD_SELECT_ALL),
    ("windows-menu", commands::CMD_MORE_OPTIONS),
    ("open-in-new-tab", commands::CMD_OPEN_NEW_TAB),
    ("properties", commands::CMD_PROPERTIES),
    ("undo", commands::CMD_UNDO),
    ("redo", commands::CMD_REDO),
    ("pin-selected", commands::CMD_PIN),
    ("open-in-other-pane", commands::CMD_OPEN_OTHER_PANE),
    ("copy-to-other-pane", commands::CMD_COPY_TO_OTHER),
    ("move-to-other-pane", commands::CMD_MOVE_TO_OTHER),
    ("swap-panes", toolbar::SWAP_PANES),
    ("mirror-pane", toolbar::MIRROR_PANE),
    ("vertical-tabs", toolbar::VERTICAL_TABS),
    ("refresh", toolbar::REFRESH),
    ("hidden-items", toolbar::HIDDEN),
    ("preview", toolbar::PREVIEW),
    ("split", toolbar::SPLIT),
    ("settings", toolbar::SETTINGS),
    ("shortcuts", toolbar::SHORTCUTS),
    ("details", toolbar::DETAILS),
    ("thumbnails", toolbar::GRID),
    ("pin", toolbar::PIN),
    ("terminal", toolbar::TERMINAL),
    ("file-operations", toolbar::OPERATIONS),
    ("folder-sizes", toolbar::FOLDER_SIZES),
    ("group-by-date", toolbar::GROUP),
    ("reveal-in-sidebar", toolbar::REVEAL),
    ("sync-panes", toolbar::SYNC_PANES),
    ("compare-panes", toolbar::COMPARE),
    ("check-for-updates", toolbar::UPDATES),
];

const CLASS: PCWSTR = w!("Kestrel");
const WM_APP_EVENTS: u32 = WM_APP + 1;
const WM_APP_LATE_INIT: u32 = WM_APP + 2;
const WM_APP_PREFERENCES: u32 = WM_APP + 3;
/// Checkpoints the session to disk; preview owns timer 7.
const TIMER_SESSION: usize = 8;
const ID_BACK: usize = 1;
const ID_FORWARD: usize = 2;
const ID_UP: usize = 3;
/// Listing threads stuck on dead network paths we tolerate before refusing new ones.
const MAX_LISTERS: usize = 8;
/// Details columns: title, width when shown (at 100% scaling) and sort key.
/// Columns from `SHOWN_COLUMNS` on start hidden (width 0); the header menu shows them.
const COLUMNS: [(PCWSTR, i32, SortKey); 6] = [
    (w!("Name"), 320, SortKey::Name),
    (w!("Date modified"), 160, SortKey::Modified),
    (w!("Type"), 150, SortKey::Type),
    (w!("Size"), 100, SortKey::Size),
    (w!("Date created"), 160, SortKey::Created),
    (w!("Attributes"), 80, SortKey::Attributes),
];
const SHOWN_COLUMNS: usize = 4;

enum Event {
    Thumbnail {
        view: u64,
        generation: u64,
        entry: u32,
        attempted: bool,
        image: Option<grid::Bitmap>,
    },
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
    Navigate {
        generation: u64,
        dir: PathBuf,
    },
    /// One-off status bar message.
    Message(String),
    /// A preview finished loading.
    Preview {
        token: u64,
        shown: preview::Shown,
    },
    /// A search is running; this many matches so far.
    SearchProgress {
        generation: u64,
        found: usize,
    },
    TreeChildren {
        node: usize,
        result: io::Result<Vec<String>>,
    },
    DriveLabel {
        node: usize,
        text: String,
    },
    Icon {
        view: u64,
        done: Done,
    },
    /// The watched folder changed (`id` names the watcher).
    DirChanged(u64),
    /// A watcher opened its folder; `handle` is for device-removal notifications.
    WatchReady {
        id: u64,
        handle: isize,
    },
    /// The list should catch up with the browser (after a rename box closed).
    Resync(u64),
    /// A file operation ended: the item to select, or an error.
    /// One folder's total from Calculate folder sizes.
    FolderSize {
        view: u64,
        dir: PathBuf,
        name: Vec<u16>,
        bytes: u64,
        done: usize,
        of: usize,
    },
    OpDone {
        view: u64,
        dir: Option<PathBuf>,
        result: std::result::Result<Option<String>, String>,
        edit: bool,
    },
}

struct App {
    hwnd: HWND,
    view: panes::FolderView,
    other: Option<panes::FolderView>,
    split_view: bool,
    /// View > Sync pane navigation: the other pane follows into subfolders and up.
    sync_panes: bool,
    right_active: bool,
    pane_ratio: i32,
    preview_width: Option<i32>,
    drag_kind: SplitKind,
    address: HWND,
    address_edit_button: HWND,
    address_editing: bool,
    breadcrumbs: Vec<(HWND, PathBuf)>,
    /// The address and filter fields drawn around their controls (`chrome::paint`).
    fields: [RECT; 2],
    filter: HWND,
    syncing_filter: bool,
    buttons: [HWND; 3],
    toolbar: Vec<HWND>,
    startup_override: Option<Option<PathBuf>>,
    restore_session: bool,
    pins: Vec<PathBuf>,
    /// Known folders taken out of Quick access.
    unpinned: Vec<PathBuf>,
    recent: Vec<PathBuf>,
    /// Folders shown grouped by date (View > Group by date).
    grouped: Vec<PathBuf>,
    /// The default sort and each folder's own, shared by every view's browser.
    sorts: std::rc::Rc<std::cell::RefCell<crate::browser::FolderSorts>>,
    status: HWND,
    font: HFONT,
    events: Arc<Events<Event>>,
    tabs: Tabs<tabbar::TabSlot>,
    tab_strip: HWND,
    /// The tabs listed above the sidebar, shown instead of the strip with `vertical_tabs`.
    tab_list: HWND,
    vertical_tabs: bool,
    next_tab_id: u64,
    theme: ThemeChoice,
    palette: Palette,
    brushes: Option<dark::Brushes>,
    preview: preview::Pane,
    live_listers: Arc<AtomicUsize>,
    tree: HWND,
    nodes: Vec<tree::Node>,
    tree_epoch: usize,
    tree_hidden: bool,
    /// A reveal (Ctrl+Shift+E) is expanding the sidebar towards the open folder.
    tree_revealing: bool,
    split: i32,
    drag_offset: Option<i32>,
    watch_ids: u64,
    this_pc: usize,
}

impl App {
    fn views(&self) -> impl Iterator<Item = &panes::FolderView> {
        std::iter::once(&self.view).chain(self.other.iter()).chain(
            self.tabs
                .items()
                .iter()
                .filter_map(|t| t.workspace.as_ref())
                .flat_map(|w| std::iter::once(&w.view).chain(w.other.iter())),
        )
    }
    fn view_mut(&mut self, id: u64) -> Option<&mut panes::FolderView> {
        std::iter::once(&mut self.view)
            .chain(self.other.iter_mut())
            .chain(
                self.tabs
                    .items_mut()
                    .iter_mut()
                    .filter_map(|t| t.workspace.as_mut())
                    .flat_map(|w| std::iter::once(&mut w.view).chain(w.other.iter_mut())),
            )
            .find(|v| v.id == id)
    }

    fn controls(&self) -> Vec<HWND> {
        let [b0, b1, b2] = self.buttons;
        let mut controls = vec![
            self.view.list,
            self.status,
            self.address,
            self.filter,
            self.tree,
            self.tab_strip,
            self.tab_list,
            b0,
            b1,
            b2,
        ];
        controls.extend(self.views().filter(|v| v.id != self.view.id).map(|v| v.list));
        controls.extend(self.views().map(|v| v.label));
        controls.extend(&self.toolbar);
        controls.push(self.address_edit_button);
        controls.extend(self.breadcrumbs.iter().map(|p| p.0));
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
    with_app(|a| a.view.drop_target.clone()).flatten()
}

/// The settings captured when the main window closed on this thread.
pub fn take_final_settings() -> Option<Settings> {
    FINAL_SETTINGS.with(|s| s.borrow_mut().take())
}

/// Runs `f` on the app state; `None` if there is none yet or it is already borrowed.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.try_with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => app.as_mut().map(f),
        Err(_) => {
            crash::log("re-entrant access to UI state");
            None
        }
    })
    .ok()
    .flatten()
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
        let buttons =
            [button(w!("Back"), ID_BACK)?, button(w!("Forward"), ID_FORWARD)?, button(w!("Up"), ID_UP)?];
        let address =
            child(hwnd, WC_EDITW, w!(""), WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), none, 0)?;
        let filter =
            child(hwnd, WC_EDITW, w!(""), WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), none, 0)?;
        // Grey hint text while empty; shown even when focused.
        SendMessageW(
            filter,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(w!("Filter (Enter searches subfolders)").0 as isize)),
        );
        let font = message_font(hwnd);
        let target = hwnd.0 as isize;
        let events = Arc::new(Events::new(move || {
            PostMessageW(Some(HWND(target as _)), WM_APP_EVENTS, WPARAM(0), LPARAM(0)).is_ok()
        }));
        startup_mark("address controls");
        let mut view = panes::create(hwnd, settings, 0, events.clone(), font)?;
        let sorts = std::rc::Rc::new(std::cell::RefCell::new(crate::browser::FolderSorts {
            default: settings.sort,
            folders: settings.folder_sorts.clone(),
        }));
        view.browser.sorts = sorts.clone();
        startup_mark("list control");
        let list = view.list;
        let status = child(hwnd, STATUSCLASSNAMEW, w!(""), WINDOW_STYLE(SBARS_SIZEGRIP), none, 0)?;
        let tree = tree::create(hwnd)?;
        let tab_strip = tabbar::create(hwnd)?;
        let tab_list = tabbar::create_list(hwnd)?;
        startup_mark("sidebar controls");
        let preview_pane = preview::create(hwnd)?;
        startup_mark("preview controls");
        let toolbar_buttons = toolbar::create(hwnd)?;
        startup_mark("toolbar controls");

        set_font(
            &[list, status, address, filter, tree, tab_strip, tab_list, buttons[0], buttons[1], buttons[2]],
            font,
        );
        set_font(&preview::controls(&preview_pane), font);
        set_font(&toolbar_buttons, font);

        startup_mark("controls");
        let address_edit_button =
            child(hwnd, w!("BUTTON"), w!("Edit"), WS_TABSTOP, WINDOW_EX_STYLE(0), breadcrumbs::EDIT)?;
        set_font(&[address_edit_button], font);
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                view,
                other: None,
                split_view: false,
                sync_panes: false,
                right_active: false,
                pane_ratio: settings.pane_ratio,
                preview_width: settings.preview_width,
                drag_kind: SplitKind::Sidebar,
                address,
                address_edit_button,
                address_editing: false,
                breadcrumbs: Vec::new(),
                fields: [RECT::default(); 2],
                buttons,
                toolbar: toolbar_buttons,
                startup_override: None,
                restore_session: settings.restore_session,
                pins: settings.pins.clone(),
                unpinned: settings.unpinned.clone(),
                recent: settings.recent.clone(),
                grouped: settings.grouped.clone(),
                sorts,
                status,
                font,
                events,
                tabs: Tabs::new(tabbar::TabSlot::active(0)),
                tab_strip,
                tab_list,
                vertical_tabs: false,
                next_tab_id: 1,
                theme: settings.theme,
                palette: Palette::new(false),
                brushes: None,
                preview: preview_pane,
                filter,
                syncing_filter: false,
                live_listers: Arc::new(AtomicUsize::new(0)),
                tree,
                nodes: Vec::new(),
                tree_epoch: 0,
                tree_hidden: settings.show_hidden,
                tree_revealing: false,
                split: settings.split.unwrap_or_else(|| scale(hwnd, 240)),
                drag_offset: None,
                watch_ids: 0,
                this_pc: 0,
            })
        });

        let _ = windows::Win32::System::DataExchange::AddClipboardFormatListener(hwnd);
        if settings.vertical_tabs {
            with_app(|a| a.vertical_tabs = true);
            let _ = ShowWindow(tab_strip, SW_HIDE);
            let _ = ShowWindow(tab_list, SW_SHOWNA);
        }
        layout(hwnd);
        update_sort_arrows();
        preview::restore(hwnd, settings.preview);
        grid::set(settings.grid);
        let _ = SetFocus(Some(list));
        go(start.to_path_buf());
        if settings.dual_pane && settings.tabs.is_empty() {
            panes::toggle();
        }
        if !settings.tabs.is_empty() {
            tabbar::restore(settings);
        }
        crate::session::register(hwnd.0 as isize, settings.clone());
        SetTimer(Some(hwnd), TIMER_SESSION, 30_000, None);
        // Icons and the sidebar wait until the window has painted once.
        startup_mark("before theme");
        dark::apply(hwnd, Palette::new(settings.theme.is_dark(system_prefers_dark())));
        startup_mark("after theme");
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
    // Ctrl+mouse wheel over the file list changes the thumbnail size.
    if msg.message == WM_MOUSEWHEEL
        && msg.wParam.0 & windows::Win32::System::SystemServices::MK_CONTROL.0 as usize != 0
        && with_app(|a| a.view.list) == Some(msg.hwnd)
    {
        grid::zoom((msg.wParam.0 >> 16) as i16 > 0);
        return true;
    }
    // Shift+mouse wheel over a file list scrolls it sideways, as in Explorer.
    if msg.message == WM_MOUSEWHEEL
        && msg.wParam.0 & windows::Win32::System::SystemServices::MK_SHIFT.0 as usize != 0
        && with_app(|a| a.views().any(|v| v.list == msg.hwnd)) == Some(true)
    {
        let delta = (msg.wParam.0 >> 16) as i16 as isize;
        unsafe { SendMessageW(msg.hwnd, LVM_SCROLL, Some(WPARAM(-delta as usize)), Some(LPARAM(0))) };
        return true;
    }
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    crash::guard(|| on_key(msg)).unwrap_or(false)
}

fn on_key(msg: &MSG) -> bool {
    panes::activate(msg.hwnd);
    let Some((list, address, tree, filter)) = with_app(|a| (a.view.list, a.address, a.tree, a.filter)) else {
        return false;
    };
    if msg.message == WM_KEYDOWN && msg.wParam.0 == VK_F1.0 as usize {
        if let Some(hwnd) = with_app(|a| a.hwnd) {
            dialogs::shortcuts(hwnd);
        }
        return true;
    }
    if ![list, address, tree, filter].contains(&msg.hwnd)
        && !toolbar::owns(msg.hwnd)
        && !breadcrumbs::owns(msg.hwnd)
        && with_app(|a| a.address_edit_button) != Some(msg.hwnd)
    {
        return false;
    }
    let key = VIRTUAL_KEY(msg.wParam.0 as u16);
    let in_address = msg.hwnd == address;
    let in_list = msg.hwnd == list;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    // keys.txt bindings win over the built-in keys, except plain typing keys in text boxes.
    let alt = unsafe { GetKeyState(VK_MENU.0 as i32) } < 0;
    let chord = crate::keys::Chord { vk: key.0, ctrl, shift, alt };
    let typing =
        (in_address || msg.hwnd == filter) && !ctrl && !alt && !(VK_F1.0..=VK_F24.0).contains(&key.0);
    if !typing
        && let Some(binding) = crate::keys::bindings(COMMAND_NAMES).iter().find(|b| b.chord == chord)
        && let Some(hwnd) = with_app(|a| a.hwnd)
    {
        unsafe { SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(binding.command)), Some(LPARAM(0))) };
        return true;
    }
    if msg.message == WM_SYSKEYDOWN {
        match key {
            VK_LEFT => run(Browser::back),
            VK_RIGHT => run(Browser::forward),
            VK_UP => run(Browser::up),
            VK_D => focus_address(),
            VK_P => preview::toggle(with_app(|a| a.hwnd).unwrap_or_default()),
            VK_RETURN if in_list => _ = commands::run(commands::CMD_PROPERTIES),
            _ => return false,
        }
        return true;
    }
    match key {
        VK_BACK if !in_address && msg.hwnd != filter => run(Browser::back),
        VK_F5 if in_list && shift => _ = commands::run(commands::CMD_COPY_TO_OTHER),
        VK_F6 if in_list && shift => _ = commands::run(commands::CMD_MOVE_TO_OTHER),
        VK_F6 => panes::focus_other(),
        VK_U if ctrl && !in_address && msg.hwnd != filter => {
            if shift {
                panes::mirror()
            } else {
                panes::swap()
            }
        }
        VK_S if ctrl && shift => panes::toggle(),
        VK_F2 if in_list => _ = commands::run(commands::CMD_RENAME),
        VK_DELETE if in_list && shift => _ = commands::run(commands::CMD_DELETE_PERMANENTLY),
        VK_DELETE if in_list => _ = commands::run(commands::CMD_DELETE),
        VK_C if in_list && ctrl && shift => _ = commands::run(commands::CMD_COPY_PATH),
        VK_C if in_list && ctrl => _ = commands::run(commands::CMD_COPY),
        VK_X if in_list && ctrl => _ = commands::run(commands::CMD_CUT),
        VK_V if in_list && ctrl => _ = commands::run(commands::CMD_PASTE),
        VK_A if in_list && ctrl => _ = commands::run(commands::CMD_SELECT_ALL),
        VK_Z if in_list && ctrl => _ = commands::run(commands::CMD_UNDO),
        VK_Y if in_list && ctrl => _ = commands::run(commands::CMD_REDO),
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
            if let Some(job) = with_app(|a| a.view.browser.toggle_hidden()).flatten() {
                spawn_sort(job);
            }
        }
        VK_RETURN if in_address => {
            let text = window_text(address);
            let current = with_app(|a| a.view.browser.dir().map(Path::to_path_buf)).flatten();
            match parse_location(&text, current.as_deref()) {
                Some(dir) => {
                    go(dir);
                    with_app(|a| a.address_editing = false);
                    breadcrumbs::layout();
                    unsafe {
                        let _ = SetFocus(Some(list));
                    }
                }
                None => set_status(&format!("Not a location: {text}")),
            }
        }
        VK_ESCAPE if in_address => {
            with_app(|a| a.address_editing = false);
            breadcrumbs::layout();
            show_location();
            unsafe {
                let _ = SetFocus(Some(list));
            }
        }
        VK_ESCAPE => cancel_loading(),
        VK_T if ctrl => tabbar::new_tab(None),
        VK_E if ctrl && shift => tree::reveal(),
        VK_K if ctrl && shift => tabbar::duplicate(with_app(|a| a.tabs.active()).unwrap_or(0)),
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
            if let Some(other) =
                with_app(|a| a.split_view.then(|| a.other.as_ref().map(|v| v.list)).flatten()).flatten()
            {
                order.push(other);
            }
            let at = order.iter().position(|&h| h == msg.hwnd).unwrap_or(0);
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let next = if shift { (at + order.len() - 1) % order.len() } else { (at + 1) % order.len() };
            if order[next] == address {
                focus_address();
            } else {
                let _ = SetFocus(Some(order[next]));
            }
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
            WM_APP_PREFERENCES => {
                if let Some((pins, unpinned)) = crate::session::pins() {
                    let changed = with_app(|a| {
                        let changed = a.pins != pins || a.unpinned != unpinned;
                        a.pins = pins;
                        a.unpinned = unpinned;
                        changed
                    });
                    if changed == Some(true) {
                        tree::reload();
                    }
                }
                if let Some(theme) = crate::session::theme() {
                    with_app(|a| a.theme = theme);
                    dark::apply(hwnd, Palette::new(theme.is_dark(system_prefers_dark())));
                }
                LRESULT(0)
            }
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
                // The focused field's accent line comes and goes.
                if matches!(code as u32, EN_SETFOCUS | EN_KILLFOCUS)
                    && let Some(fields) = with_app(|a| a.fields)
                {
                    for f in fields {
                        let _ = InvalidateRect(Some(hwnd), Some(&f), true);
                    }
                }
                // Typing in the filter box narrows the list at once.
                if code == EN_CHANGE as usize && with_app(|a| a.filter.0 as isize) == Some(from) {
                    if with_app(|a| a.syncing_filter) == Some(true) {
                        return LRESULT(0);
                    }
                    let text = window_text(HWND(from as _));
                    if let Some(job) = with_app(|a| a.view.browser.set_filter(&text)).flatten() {
                        spawn_sort(job);
                    }
                    update_chrome();
                    return LRESULT(0);
                }
                let from_button = with_app(|a| {
                    a.buttons
                        .iter()
                        .chain(a.toolbar.iter())
                        .chain(std::iter::once(&a.address_edit_button))
                        .any(|b| b.0 as isize == from)
                        || a.breadcrumbs.iter().any(|p| p.0.0 as isize == from)
                }) == Some(true);
                let ours = (from == 0 && code == 0) || (from_button && code == BN_CLICKED as usize);
                if !ours {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                }
                match id {
                    ID_BACK if from_button => run(Browser::back),
                    ID_FORWARD if from_button => run(Browser::forward),
                    ID_UP if from_button => run(Browser::up),
                    id if (from == 0 || from_button)
                        && (breadcrumbs::execute(id)
                            || toolbar::execute(id)
                            || commands::run(id)
                            || tabbar::execute(id)) => {}
                    _ => return DefWindowProcW(hwnd, msg, wp, lp),
                }
                if !matches!(id, toolbar::SETTINGS | toolbar::SHORTCUTS | toolbar::HELP | breadcrumbs::EDIT)
                    && let Some(list) = with_app(|a| a.view.list)
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
            WM_CTLCOLORSTATIC if let Some(active) = panes::label_is_active(HWND(lp.0 as _)) => {
                dark::label_colors(HDC(wp.0 as _), active)
                    .unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                dark::edit_colors(HDC(wp.0 as _)).unwrap_or_else(|| DefWindowProcW(hwnd, msg, wp, lp))
            }
            WM_ERASEBKGND if dark::erase(hwnd, HDC(wp.0 as _)) => {
                chrome::paint(hwnd, HDC(wp.0 as _));
                LRESULT(1)
            }
            // Windows' light/dark app setting changed.
            WM_SETTINGCHANGE => {
                let area = if lp.0 == 0 {
                    String::new()
                } else {
                    PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default()
                };
                if (area == "ImmersiveColorSet" || wp.0 == SPI_SETHIGHCONTRAST.0 as usize)
                    && let Some(choice) = with_app(|a| a.theme)
                {
                    dark::apply(hwnd, Palette::new(choice.is_dark(system_prefers_dark())));
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize.x = scale(hwnd, 360);
                info.ptMinTrackSize.y = scale(hwnd, 240);
                LRESULT(0)
            }
            WM_SIZE => {
                layout(hwnd);
                LRESULT(0)
            }
            // The only bare client area below the toolbar is the splitter gap.
            WM_LBUTTONDOWN => {
                if let Some((kind, edge)) = splitter_at(hwnd, lp) {
                    let x = (lp.0 & 0xFFFF) as i16 as i32;
                    with_app(|a| {
                        a.drag_offset = Some(x - edge);
                        a.drag_kind = kind;
                    });
                    SetCapture(hwnd);
                }
                LRESULT(0)
            }
            WM_MOUSEMOVE => {
                if let Some((offset, kind)) = with_app(|a| a.drag_offset.map(|o| (o, a.drag_kind))).flatten()
                {
                    let x = (lp.0 & 0xFFFF) as i16 as i32;
                    drag_splitter(hwnd, kind, x - offset);
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
                if let Some(list) = with_app(|a| a.view.list) {
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
                // Thumbnails re-render for the new scale. ponytail: active pane only; others on next View change.
                if with_app(|a| a.view.grid.enabled) == Some(true) {
                    grid::set(true);
                }
                layout(hwnd);
                LRESULT(0)
            }
            WM_CONTEXTMENU if header_list(HWND(wp.0 as _)).is_some() => {
                if let Some(list) = header_list(HWND(wp.0 as _)) {
                    column_menu(hwnd, list, lp);
                }
                LRESULT(0)
            }
            WM_CONTEXTMENU if with_app(|a| a.tree.0 as usize == wp.0) == Some(true) => {
                tree::context_menu(hwnd, lp);
                LRESULT(0)
            }
            WM_CONTEXTMENU if with_app(|a| a.views().any(|v| v.list.0 as usize == wp.0)) == Some(true) => {
                panes::activate(HWND(wp.0 as _));
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
            WM_TIMER if wp.0 == TIMER_SESSION => {
                save_session(hwnd, false);
                LRESULT(0)
            }
            // Sign-out and restart end the process without a normal exit: save now.
            WM_QUERYENDSESSION => {
                save_session(hwnd, false);
                LRESULT(1)
            }
            WM_ENDSESSION if wp.0 != 0 => {
                save_session(hwnd, true);
                LRESULT(0)
            }
            WM_PAINT => {
                log_first_paint();
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_DESTROY => {
                with_app(|a| a.events.close());
                let _ = windows::Win32::System::DataExchange::RemoveClipboardFormatListener(hwnd);
                preview::shutdown();
                if let Some(list) = with_app(|a| a.view.list) {
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
    if hdr.code == NM_CUSTOMDRAW && chrome::is_top_button(hdr.hwndFrom) {
        return unsafe { chrome::draw_button(&*(lp.0 as *const NMCUSTOMDRAW)) };
    }
    if hdr.code == TTN_GETDISPINFOW
        && let Some(done) = tabbar::tooltip(hdr, lp)
    {
        return Some(done);
    }
    let (_list, tree, strip, tab_list) = with_app(|a| (a.view.list, a.tree, a.tab_strip, a.tab_list))?;
    if hdr.hwndFrom == strip {
        return tabbar::on_notify(hdr);
    }
    if hdr.hwndFrom == tab_list {
        return tabbar::on_list_notify(hdr, lp);
    }
    if hdr.hwndFrom == tree {
        return unsafe { tree::on_notify(hdr, lp) };
    }
    let view = with_app(|a| a.views().find(|v| v.list == hdr.hwndFrom).map(|v| v.id)).flatten()?;
    if hdr.code == NM_CUSTOMDRAW {
        let (groups, palette) =
            with_app(|a| Some((a.view_mut(view)?.groups.clone(), a.palette))).flatten()?;
        let cd = unsafe { &*(lp.0 as *const NMLVCUSTOMDRAW) };
        return unsafe {
            groups::draw_header(cd, &groups, palette).or_else(|| dark::column_lines(cd, palette))
        };
    }
    if hdr.code == LVN_ODCACHEHINT {
        let hint = unsafe { &*(lp.0 as *const NMLVCACHEHINT) };
        with_view(view, |v| {
            let from = hint.iFrom.max(0) as usize;
            let to = hint.iTo.max(0) as usize;
            v.grid.hint(from, to);
            v.icons.hint(v.browser.order.get(from..(to + 1).min(v.browser.order.len())).unwrap_or_default());
        });
        return Some(LRESULT(0));
    }
    if hdr.code == LVN_GETDISPINFOW {
        let item = unsafe { &mut (*(lp.0 as *mut NMLVDISPINFOW)).item };
        with_view(view, |v| fill(v, item));
        return Some(LRESULT(0));
    }
    if matches!(
        hdr.code,
        NM_SETFOCUS
            | NM_CLICK
            | LVN_COLUMNCLICK
            | LVN_ITEMACTIVATE
            | LVN_BEGINDRAG
            | LVN_BEGINRDRAG
            | LVN_BEGINLABELEDITW
    ) {
        panes::activate(hdr.hwndFrom);
    }
    if with_app(|a| a.view.list) != Some(hdr.hwndFrom) {
        return None;
    }
    unsafe {
        match hdr.code {
            LVN_ODFINDITEMW => {
                let find = &*(lp.0 as *const NMLVFINDITEMW);
                if !find.lvfi.flags.contains(LVFI_STRING) || find.lvfi.psz.is_null() {
                    return Some(LRESULT(-1));
                }
                let prefix = find.lvfi.psz.as_wide();
                let row =
                    with_app(|a| a.view.browser.find_prefix(find.iStart.max(0) as usize, prefix)).flatten();
                Some(LRESULT(row.map_or(-1, |r| r as isize)))
            }
            LVN_COLUMNCLICK => {
                let col = (*(lp.0 as *const NMLISTVIEW)).iSubItem as usize;
                if let Some((_, _, key)) = COLUMNS.get(col) {
                    if let Some(job) =
                        with_app(|a| a.view.browser.set_sort(a.view.browser.sort_for_column(*key))).flatten()
                    {
                        spawn_sort(job);
                    }
                    update_sort_arrows();
                }
                Some(LRESULT(0))
            }
            LVN_ITEMCHANGED | LVN_ODSTATECHANGED => {
                if with_app(|a| a.view.restoring_selection) == Some(true) {
                    return Some(LRESULT(0));
                }
                toolbar::update();
                show_status();
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
                    usize::try_from(row).ok().and_then(|r| with_app(|a| a.view.browser.path_at(r)).flatten());
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
fn fill(v: &mut panes::FolderView, item: &mut LVITEMW) {
    let (b, icons, grid, drop_row) = (&v.browser, &mut v.icons, &mut v.grid, v.drop_row);
    let (Some(s), Ok(row)) = (b.snapshot(), usize::try_from(item.iItem)) else { return write(item, &[]) };
    let Some(&entry) = b.order.get(row) else { return write(item, &[]) };
    let i = entry as usize;
    if item.mask.contains(LVIF_STATE) && Some(row) == drop_row {
        item.state.0 |= LVIS_DROPHILITED.0;
        item.stateMask.0 |= LVIS_DROPHILITED.0;
    }
    let drive = s.drives.get(i);
    if item.mask.contains(LVIF_IMAGE) {
        item.iImage = if let Some(d) = drive.filter(|_| !grid.enabled) {
            d.icon
        } else if grid.enabled {
            grid.icon(b.shown_generation(), entry, row, b.path_at(row).map(|p| p.0).unwrap_or_default())
        } else {
            icons.icon(b.shown_generation(), entry, s.name(i), s.is_dir(i), &s.dir)
        };
    }
    let wide = |t: String| t.encode_utf16().collect::<Vec<_>>();
    match COLUMNS.get(item.iSubItem as usize).map(|c| c.2) {
        Some(SortKey::Name) => write(item, s.name(i)),
        Some(SortKey::Modified) => write(item, &wide(format::time(s.entries[i].modified))),
        Some(SortKey::Type) if drive.is_some() => write(item, &wide(drive.unwrap().detail.clone())),
        Some(SortKey::Type) => {
            write(item, &wide(icons.type_name(s.name(i), s.is_dir(i)).unwrap_or_default()))
        }
        Some(SortKey::Size) if drive.is_some_and(|_| s.entries[i].size > 0) => {
            write(item, &wide(format::bytes(s.entries[i].size)))
        }
        Some(SortKey::Size) if !s.is_dir(i) => write(item, &wide(format::size(s.entries[i].size))),
        Some(SortKey::Size) => match v.sizes.get(s.name(i)) {
            Some(&bytes) => write(item, &wide(format::size(bytes))),
            None => write(item, &[]),
        },
        Some(SortKey::Created) => write(item, &wide(format::time(s.entries[i].created))),
        Some(SortKey::Attributes) => write(item, &wide(format::attributes(s.entries[i].attrs))),
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
        Event::Thumbnail { view, generation, entry, attempted, image } => {
            if let Some((list, true)) =
                with_view(view, |v| (v.list, v.grid.finish(generation, entry, attempted, image)))
            {
                unsafe {
                    let _ = InvalidateRect(Some(list), None, false);
                }
            }
        }
        Event::Listed { generation, result } => {
            let id = crate::tabs::tab_of_generation(generation);
            let sel = selected_rows_for(id);
            let focus = focused_name(id);
            let applied = with_view(id, |v| {
                let r = v.browser.on_listed(generation, result, &sel);
                if r != Applied::Stale {
                    v.lister = None;
                }
                r
            });
            match applied {
                Some(Applied::Shown { select, new_folder }) => {
                    if new_folder {
                        with_view(id, |v| {
                            v.sizes_cancel.store(true, Ordering::SeqCst);
                            v.sizes.clear();
                        });
                    }
                    if new_folder
                        && let Some(dir) = with_view(id, |v| v.browser.dir().map(PathBuf::from)).flatten()
                    {
                        with_app(|a| crate::settings::remember(&mut a.recent, dir));
                    }
                    refresh_view(id, &select, new_folder, focus.as_deref());
                    if new_folder && view_visible(id) {
                        watch_view(id);
                    }
                    if with_app(|a| a.view.id == id) == Some(true) {
                        commands::after_refresh();
                    }
                    if let Some(job) = with_view(id, |v| v.browser.sort_job()).flatten() {
                        spawn_sort(job);
                    }
                }
                Some(Applied::Failed)
                    if with_view(id, |v| {
                        let first = v.browser.dir().is_none() && !v.fallback_attempted;
                        v.fallback_attempted = true;
                        first
                    }) == Some(true) =>
                {
                    if let Some(home) = std::env::var_os("USERPROFILE")
                        && let Some(job) = with_view(id, |v| v.browser.go(PathBuf::from(home)))
                    {
                        spawn_list(job);
                    }
                }
                Some(Applied::Failed) => {
                    if with_app(|a| a.view.id == id) == Some(true) {
                        update_chrome();
                    }
                }
                Some(Applied::Gone(job)) => spawn_list(job),
                _ => {}
            }
            start_queued_lists();
            tabbar::refresh();
            panes::update_labels();
            if let Some(job) = with_view(id, |v| v.browser.follow_up()).flatten() {
                spawn_list(job);
            }
        }
        Event::DirChanged(watch_id) => {
            let id = with_app(|a| {
                a.views().find(|v| v.watcher.as_ref().is_some_and(|w| w.0 == watch_id)).map(|v| v.id)
            })
            .flatten();
            if let Some(job) = id.and_then(|id| with_view(id, |v| v.browser.on_dir_changed()).flatten()) {
                spawn_list(job);
            }
        }
        Event::WatchReady { id, handle } => on_watch_ready(id, handle),
        Event::FolderSize { view, dir, name, bytes, done, of } => {
            let list = with_view(view, |v| {
                (v.browser.dir() == Some(dir.as_path())).then(|| {
                    v.sizes.insert(name, bytes);
                    v.list
                })
            });
            if let Some(Some(list)) = list {
                if done == of
                    && let Some(job) = with_view(view, |v| {
                        (v.browser.sort.key == SortKey::Size).then(|| v.browser.resort_job()).flatten()
                    })
                    .flatten()
                {
                    spawn_sort(job);
                }
                unsafe {
                    let _ = InvalidateRect(Some(list), None, false);
                }
                if with_app(|a| a.view.id == view) == Some(true) {
                    set_status(&if done == of {
                        format!("Folder sizes calculated ({of})")
                    } else {
                        format!("Calculating folder sizes… {done} of {of}  (Esc to stop)")
                    });
                }
            }
        }
        Event::OpDone { view, dir, result, edit } => commands::on_op_done(view, dir, result, edit),
        Event::Resync(id) => refresh_view(id, &selected_rows_for(id), false, focused_name(id).as_deref()),
        Event::Sorted { generation, sort, show_hidden, filter, order } => {
            let id = crate::tabs::tab_of_generation(generation);
            let sel = selected_rows_for(id);
            let focus = focused_name(id);
            if let Some(Applied::Shown { select, .. }) =
                with_view(id, |v| v.browser.on_sorted(generation, sort, show_hidden, &filter, order, &sel))
            {
                refresh_view(id, &select, false, focus.as_deref());
            }
        }
        Event::Navigate { generation, dir } => {
            let id = crate::tabs::tab_of_generation(generation);
            if let Some(job) =
                with_view(id, |v| (v.browser.generation() == generation).then(|| v.browser.go(dir))).flatten()
            {
                spawn_list(job);
            }
        }
        Event::Message(text) => set_status(&text),
        Event::Preview { token, shown } => preview::finish(token, shown),
        Event::SearchProgress { generation, found } => {
            if with_app(|a| a.view.browser.generation() == generation && a.view.browser.is_loading())
                == Some(true)
            {
                set_status(&format!("Searching… {found} found so far  (Esc to stop)"));
            }
        }
        Event::TreeChildren { node, result } => tree::on_children(node, result),
        Event::DriveLabel { node, text } => tree::on_label(node, text),
        Event::Icon { done: Done::Node { node, icon }, .. } => tree::on_icon(node, icon),
        Event::Icon { view, done } => {
            if let Some((list, true)) = with_view(view, |v| (v.list, v.icons.on_done(done))) {
                unsafe {
                    let _ = InvalidateRect(Some(list), None, false);
                }
            }
        }
    }
}

/// Runs a browser navigation (`Browser::back`, `up`, ...) and lists its target.
fn run(nav: impl FnOnce(&mut Browser) -> Option<ListJob>) {
    let Some((job, from)) = with_app(|a| {
        let from = a.view.browser.requested_location().map(|l| l.dir);
        nav(&mut a.view.browser).map(|job| (job, from))
    })
    .flatten() else {
        return;
    };
    let to = job.dir.clone();
    spawn_list(job);
    panes::follow(from, &to);
}

fn go(dir: PathBuf) {
    run(|b| Some(b.go(dir)));
}

fn start_queued_lists() {
    loop {
        let job = with_app(|a| {
            if a.live_listers.load(Ordering::SeqCst) >= MAX_LISTERS {
                return None;
            }
            let id = a.views().find(|v| v.queued_list.is_some())?.id;
            a.view_mut(id)?.queued_list.take()
        })
        .flatten();
        let Some(job) = job else { break };
        let id = crate::tabs::tab_of_generation(job.generation);
        if with_view(id, |v| v.browser.generation() == job.generation) == Some(true) {
            spawn_list(job);
        }
    }
}

fn spawn_list(job: ListJob) {
    let id = crate::tabs::tab_of_generation(job.generation);
    if with_app(|a| a.live_listers.load(Ordering::SeqCst) >= MAX_LISTERS) == Some(true) {
        with_view(id, |v| {
            v.lister_cancel.store(true, Ordering::SeqCst);
            v.queued_list = Some(job);
        });
        update_chrome();
        return;
    }
    with_view(id, |v| v.queued_list = None);
    // A new listing supersedes any search still running.
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let Some((events, live)) = with_app(|a| {
        if let Some(v) = a.view_mut(id) {
            std::mem::replace(&mut v.lister_cancel, cancel.clone()).store(true, Ordering::SeqCst);
        }
        (a.events.clone(), a.live_listers.clone())
    }) else {
        return;
    };
    let grouped = is_grouped(&job.dir);
    // A refresh keeps the folder sizes already calculated; a new folder starts without them.
    let sizes = with_view(id, |v| {
        (job.query.is_none() && v.browser.dir() == Some(job.dir.as_path())).then(|| v.sizes.clone())
    })
    .flatten()
    .unwrap_or_default();
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
                            events.send(Event::SearchProgress { generation, found });
                        }
                    };
                    search::search(&dir, q, show_hidden, &|| cancel.load(Ordering::SeqCst), &mut progress)
                }
                None => crate::network::list_location(&dir).map(|mut s| {
                    s.free = crate::listing::free_space(&dir);
                    s
                }),
            };
            snap.map(|s| {
                let mut order = filter_order(&s, &sorted_order(&s, sort, show_hidden, &sizes), &filter);
                if grouped {
                    crate::groups::group_order(&s, &mut order, crate::groups::today());
                }
                (s, order)
            })
        })
        .unwrap_or_else(|| Err(io::Error::other("internal error while listing")));
        drop(_done);
        events.send(Event::Listed { generation, result });
    });
    if worker.is_none() {
        with_app(|a| a.live_listers.fetch_sub(1, Ordering::SeqCst));
    }
    with_view(id, |v| v.lister = worker);
    if with_app(|a| a.view.id == id) == Some(true) {
        update_chrome();
    }
}

/// Startup work that can wait for the first paint: the shell's icon list
/// (~20 ms the first time) and the sidebar.
fn late_init() {
    let Some((lists, tree)) = with_app(|a| (a.views().map(|v| v.list).collect::<Vec<_>>(), a.tree)) else {
        return;
    };
    panes::enable_drop();
    // Names for screen readers; after first paint because it loads oleacc.
    if let Some((address, filter, strip, tab_list)) =
        with_app(|a| (a.address, a.filter, a.tab_strip, a.tab_list))
    {
        for (control, name) in [
            (address, w!("Address")),
            (filter, w!("Filter")),
            (tree, w!("Folders")),
            (strip, w!("Tabs")),
            (tab_list, w!("Tabs")),
        ] {
            set_accessible_name(control, name);
        }
        for &list in &lists {
            set_accessible_name(list, w!("Files"));
        }
    }
    let (image_list, folder_icon) = icons::system_image_list().unwrap_or_default();
    startup_mark("image list");
    if !image_list.is_invalid() {
        unsafe {
            for &list in &lists {
                SendMessageW(
                    list,
                    LVM_SETIMAGELIST,
                    Some(WPARAM(LVSIL_SMALL as usize)),
                    Some(LPARAM(image_list.0)),
                );
            }
            SendMessageW(
                tree,
                TVM_SETIMAGELIST,
                Some(WPARAM(TVSIL_NORMAL as usize)),
                Some(LPARAM(image_list.0)),
            );
            if let Some(tab_list) = with_app(|a| a.tab_list) {
                SendMessageW(
                    tab_list,
                    LVM_SETIMAGELIST,
                    Some(WPARAM(LVSIL_SMALL as usize)),
                    Some(LPARAM(image_list.0)),
                );
            }
        }
    }
    let ids = with_app(|a| a.views().map(|v| v.id).collect::<Vec<_>>()).unwrap_or_default();
    for id in ids {
        with_view(id, |v| v.icons.folder = folder_icon);
    }
    let this_pc = tree::populate();
    startup_mark("sidebar");
    with_app(|a| a.this_pc = this_pc);
    tree::sync();
    tabbar::refresh();
    for list in lists {
        unsafe {
            let _ = InvalidateRect(Some(list), None, false);
        }
    }
}

/// A screen-reader name for a control that has no visible label (Dynamic Annotation).
fn set_accessible_name(control: HWND, name: PCWSTR) {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Accessibility::{CLSID_AccPropServices, IAccPropServices, PROPID_ACC_NAME};
    unsafe {
        if let Ok(services) =
            CoCreateInstance::<_, IAccPropServices>(&CLSID_AccPropServices, None, CLSCTX_INPROC_SERVER)
        {
            let _ = services.SetHwndPropStr(control, OBJID_CLIENT.0 as u32, 0, PROPID_ACC_NAME, name);
        }
    }
}

/// Records the view state for saving once the window is gone.
fn capture_settings(hwnd: HWND) {
    if let Some(s) = current_settings(hwnd) {
        crate::session::closing(hwnd.0 as isize, s.clone());
        FINAL_SETTINGS.with(|f| *f.borrow_mut() = Some(s));
    }
}

/// Records the open window's state and writes the session; `all` also saves preferences.
fn save_session(hwnd: HWND, all: bool) {
    if let Some(s) = current_settings(hwnd) {
        crate::session::update(hwnd.0 as isize, s);
    }
    let saved = if all { crate::session::finish() } else { crate::session::checkpoint() };
    if let Err(e) = saved {
        crash::log(&format!("Session not saved: {e}"));
    }
}

/// The window's view state as settings.
fn current_settings(hwnd: HWND) -> Option<Settings> {
    let (list, mut s) = with_app(|a| {
        let s = Settings {
            grid: a.view.grid.enabled,
            thumb_size: a.view.grid.size,
            sort: a.sorts.borrow().default,
            folder_sorts: a.sorts.borrow().folders.clone(),
            show_hidden: a.view.browser.show_hidden,
            split: Some(a.split),
            preview: preview::is_visible_in(a),
            dual_pane: a.split_view,
            pane_ratio: a.pane_ratio,
            preview_width: a.preview_width,
            last_folder: a
                .startup_override
                .clone()
                .unwrap_or_else(|| a.view.browser.dir().map(Path::to_path_buf)),
            theme: a.theme,
            restore_session: a.restore_session,
            pins: a.pins.clone(),
            unpinned: a.unpinned.clone(),
            vertical_tabs: a.vertical_tabs,
            recent: a.recent.clone(),
            grouped: a.grouped.clone(),
            ..Default::default()
        };
        (a.view.list, s)
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
    (s.tabs, s.active_tab) = tabbar::saved();
    Some(s)
}

/// Windows opened with Ctrl+N, each on its own UI thread with its own state.
static OTHER_WINDOWS: std::sync::Mutex<Vec<JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

pub fn other_window_count() -> usize {
    OTHER_WINDOWS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|w| !w.is_finished()).count()
}

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
    s.tabs.clear();
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
    open_restored_window(s);
}

/// Opens a restored window on its own UI thread.
pub fn open_restored_window(s: Settings) {
    let start = s.last_folder.clone().unwrap_or_else(|| PathBuf::from(r"C:\"));
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
        Ok(h) => {
            let mut windows = OTHER_WINDOWS.lock().unwrap_or_else(|e| e.into_inner());
            windows.retain(|w| !w.is_finished());
            windows.push(h);
        }
        Err(e) => set_status(&format!("Couldn't open a new window: {e}")),
    }
}

/// Logs how long after process start the window first painted.
fn log_first_paint() {
    if !STARTUP_DONE.swap(true, Ordering::SeqCst) {
        record_startup("first paint");
        let marks = STARTUP_MARKS.lock().map(|m| m.join(", ")).unwrap_or_default();
        crash::log(&format!("startup (ms after process start): {marks}"));
    }
}

static STARTUP_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static STARTUP_MARKS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Notes how long after process creation `what` happened; logged at first paint.
pub fn startup_mark(what: &str) {
    if !STARTUP_DONE.load(Ordering::Relaxed) {
        record_startup(what);
    }
}
fn record_startup(what: &str) {
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
fn watch_view(view: u64) {
    stop_watching_view(view);
    if !view_visible(view) {
        return;
    }
    let Some((id, dir, events)) = with_app(|a| {
        a.watch_ids += 1;
        Some((a.watch_ids, a.view_mut(view)?.browser.dir()?.to_path_buf(), a.events.clone()))
    })
    .flatten() else {
        return;
    };
    // This PC is no folder; drives coming and going refresh it instead (`on_device_change`).
    if crate::places::is_this_pc(&dir) {
        return;
    }
    let ready = events.clone();
    let watcher = Watcher::start(
        dir,
        move || events.send(Event::DirChanged(id)),
        move |handle| ready.send(Event::WatchReady { id, handle }),
    );
    with_view(view, |v| v.watcher = Some((id, watcher)));
}
fn stop_watching() {
    if let Some(id) = with_app(|a| a.view.id) {
        stop_watching_view(id);
    }
}
fn stop_watching_view(view: u64) {
    let Some((watcher, note)) = with_view(view, |v| (v.watcher.take(), v.device_note.take())) else { return };
    drop(watcher);
    if let Some((note, _)) = note {
        unsafe {
            let _ = UnregisterDeviceNotification(HDEVNOTIFY(note as _));
        }
    }
}
fn on_watch_ready(id: u64, handle: isize) {
    let Some((hwnd, view)) = with_app(|a| {
        a.views().find(|v| v.watcher.as_ref().is_some_and(|w| w.0 == id)).map(|v| (a.hwnd, v.id))
    })
    .flatten() else {
        return;
    };
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
        with_view(view, |v| v.device_note = Some((note.0 as isize, handle)));
    }
}

fn on_device_change(event: u32, lp: LPARAM) {
    if lp.0 == 0 {
        return;
    }
    let hdr = unsafe { &*(lp.0 as *const DEV_BROADCAST_HDR) };
    if hdr.dbch_devicetype == DBT_DEVTYP_HANDLE {
        let handle = unsafe { (*(lp.0 as *const DEV_BROADCAST_HANDLE)).dbch_handle.0 as isize };
        let Some(view) =
            with_app(|a| a.views().find(|v| v.device_note.is_some_and(|n| n.1 == handle)).map(|v| v.id))
                .flatten()
        else {
            return;
        };
        match event {
            DBT_DEVICEQUERYREMOVE => {
                // The eject goes ahead only if nobody holds a handle on the drive: close ours now.
                if let Some((_, w)) = with_view(view, |v| v.watcher.take()).flatten() {
                    w.stop_and_wait(std::time::Duration::from_millis(500));
                }
            }
            DBT_DEVICEREMOVECOMPLETE => {
                stop_watching_view(view);
                if let Some(job) = with_view(view, |v| v.browser.on_dir_changed()).flatten() {
                    spawn_list(job);
                }
            }
            _ => {}
        }
    } else if hdr.dbch_devicetype == DBT_DEVTYP_VOLUME
        && (event == DBT_DEVICEARRIVAL || event == DBT_DEVICEREMOVECOMPLETE)
    {
        tree::refresh_drives();
        let pcs = with_app(|a| {
            a.views()
                .filter(|v| v.browser.dir().is_some_and(crate::places::is_this_pc))
                .map(|v| v.id)
                .collect()
        })
        .unwrap_or_else(Vec::new);
        for id in pcs {
            if let Some(job) = with_view(id, |v| v.browser.refresh()).flatten() {
                spawn_list(job);
            }
        }
    }
}

/// View > Calculate folder sizes: totals for every folder shown, one at a time on a worker.
fn calculate_folder_sizes() {
    let Some((view, dir, names, events, cancel)) = with_app(|a| {
        let v = &mut a.view;
        let s = v.browser.snapshot().filter(|s| !crate::places::is_this_pc(&s.dir))?;
        let names: Vec<Vec<u16>> = v
            .browser
            .order
            .iter()
            .map(|&i| i as usize)
            .filter(|&i| s.is_dir(i))
            .map(|i| s.name(i).to_vec())
            .collect();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        std::mem::replace(&mut v.sizes_cancel, cancel.clone()).store(true, Ordering::SeqCst);
        Some((v.id, s.dir.clone(), names, a.events.clone(), cancel))
    })
    .flatten() else {
        return;
    };
    if names.is_empty() {
        return set_status("No folders here to measure");
    }
    let of = names.len();
    spawn("folder sizes", move || {
        for (done, name) in names.into_iter().enumerate() {
            let path = dir.join(String::from_utf16_lossy(&name));
            let Some(bytes) = search::folder_size(&path, &|| cancel.load(Ordering::SeqCst)) else { return };
            events.send(Event::FolderSize { view, dir: dir.clone(), name, bytes, done: done + 1, of });
        }
    });
}

fn cancel_loading() {
    with_app(|a| {
        a.view.lister_cancel.store(true, Ordering::SeqCst);
        a.view.sizes_cancel.store(true, Ordering::SeqCst);
    });
    let Some(worker) =
        with_app(|a| a.view.browser.cancel().then(|| a.view.lister.take()).flatten()).flatten()
    else {
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
    let sizes =
        with_view(crate::tabs::tab_of_generation(job.generation), |v| v.sizes.clone()).unwrap_or_default();
    let grouped = is_grouped(&job.snapshot.dir);
    spawn("sort", move || {
        let order = crash::guard(|| {
            let s = &job.snapshot;
            let mut order = filter_order(s, &sorted_order(s, job.sort, job.show_hidden, &sizes), &job.filter);
            if grouped {
                crate::groups::group_order(s, &mut order, crate::groups::today());
            }
            order
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

/// Whether `dir` is shown grouped by date (View > Group by date).
fn is_grouped(dir: &Path) -> bool {
    with_app(|a| a.grouped.iter().any(|g| g == dir)) == Some(true)
}

fn open_file(path: PathBuf) {
    let Some((events, hwnd, generation)) =
        with_app(|a| (a.events.clone(), a.hwnd.0 as isize, a.view.browser.generation()))
    else {
        return;
    };
    spawn("open", move || {
        let ev = match crash::guard(|| shell::open(&path, HWND(hwnd as _))) {
            Some(Ok(Some(dir))) => Event::Navigate { generation, dir },
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
    let list = with_app(|a| a.view.list)?;
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
    with_app(|a| a.view.browser.path_at(row)).flatten()
}

fn selected_rows() -> Vec<usize> {
    with_app(|a| a.view.id).map(selected_rows_for).unwrap_or_default()
}

fn selected_rows_for(id: u64) -> Vec<usize> {
    let Some(list) = with_view(id, |v| v.list) else { return Vec::new() };
    let count = unsafe { SendMessageW(list, LVM_GETITEMCOUNT, None, None).0 as usize };
    if unsafe { SendMessageW(list, LVM_GETSELECTEDCOUNT, None, None).0 as usize } == count {
        return (0..count).collect();
    }
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
fn focused_name(id: u64) -> Option<Vec<u16>> {
    let list = with_view(id, |v| v.list)?;
    let row = unsafe {
        SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize))).0
    };
    with_view(id, |v| {
        let entry = *v.browser.order.get(usize::try_from(row).ok()?)?;
        Some(v.browser.snapshot()?.name(entry as usize).to_vec())
    })
    .flatten()
}

fn refresh_view(id: u64, select: &[usize], new_folder: bool, focus: Option<&[u16]>) {
    let Some((list, count, renaming)) =
        with_view(id, |v| (v.list, v.browser.order.len(), v.renaming.is_some()))
    else {
        return;
    };
    if renaming {
        if !new_folder {
            // Resetting the list now would cancel the user's typing.
            with_view(id, |v| v.resync_after_rename = true);
            return;
        }
        unsafe { SendMessageW(list, LVM_CANCELEDITLABEL, None, None) };
    }
    // Group by date, in folders that asked for it.
    let Some((sections, view_groups)) = with_app(|a| {
        let v = a.views().find(|v| v.id == id)?;
        let grouped = v.browser.dir().is_some_and(|d| a.grouped.iter().any(|g| g == d));
        let sections = grouped.then(|| {
            let today = crate::groups::today();
            let s = v.browser.snapshot();
            let days =
                v.browser.order.iter().map(|&e| crate::groups::local_day(s?.entries[e as usize].modified));
            let groups = crate::groups::by_date(days, today);
            groups.into_iter().map(|(b, rows)| (crate::groups::LABELS[b], rows)).collect()
        });
        Some((sections, v.groups.clone()))
    })
    .flatten() else {
        return;
    };
    with_view(id, |v| v.restoring_selection = true);
    unsafe {
        let visible = IsWindowVisible(list).as_bool();
        if visible {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        }
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
        groups::show(list, &view_groups, sections);
        let flags = if new_folder { 0 } else { LVSICF_NOSCROLL };
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(flags as isize)));
        if select.len() == count && count > 0 {
            set(-1, LVIS_SELECTED.0);
        } else {
            for &row in select {
                set(row as isize, LVIS_SELECTED.0);
            }
        }
        let focused = if new_folder {
            None
        } else {
            focus.and_then(|n| with_view(id, |v| v.browser.row_named(n)).flatten())
        };
        if let Some(first) = focused.or_else(|| select.first().copied()) {
            set(first as isize, LVIS_FOCUSED.0 | if select.contains(&first) { LVIS_SELECTED.0 } else { 0 });
            if new_folder {
                SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(first)), Some(LPARAM(0)));
            }
        } else if new_folder && count > 0 {
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        if visible {
            SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
        }
        let _ = InvalidateRect(Some(list), None, false);
    }
    with_view(id, |v| v.restoring_selection = false);
    if with_app(|a| a.view.id == id) == Some(true) {
        update_chrome();
    }
}

/// Title, address bar, button states and status text from the browser.
fn update_chrome() {
    let Some((hwnd, buttons, enabled, title, filter, filter_text)) = with_app(|a| {
        let b = &a.view.browser;
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
    if unsafe { GetFocus() } != filter && !window_text(filter).eq_ignore_ascii_case(&filter_text) {
        with_app(|a| a.syncing_filter = true);
        unsafe {
            let _ = SetWindowTextW(filter, &HSTRING::from(filter_text));
        }
        with_app(|a| a.syncing_filter = false);
    }
    unsafe {
        for (button, on) in buttons.iter().zip(enabled) {
            let _ = EnableWindow(*button, on);
        }
        let _ = SetWindowTextW(hwnd, &HSTRING::from(format!("{title} - Kestrel")));
    }
    toolbar::update();
    panes::update_labels();
    // A folder can bring its own sort; the header arrow follows.
    update_sort_arrows();
    tree::sync();
    show_location();
    breadcrumbs::update();
    tabbar::refresh();
    show_status();
}

fn show_status() {
    let rows = selected_rows();
    if let Some(text) = with_app(|a| a.view.browser.status_line(&rows)) {
        set_status(&text);
    }
}

/// Puts the current folder in the address bar, unless the user is typing there.
fn show_location() {
    let Some((address, dir)) = with_app(|a| (a.address, a.view.browser.dir().map(Path::to_path_buf))) else {
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
    with_app(|a| a.address_editing = true);
    breadcrumbs::layout();
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
    let Some((list, sort)) = with_app(|a| (a.view.list, a.view.browser.sort)) else { return };
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

/// The file list whose column header is `header`.
fn header_list(header: HWND) -> Option<HWND> {
    let lists = with_app(|a| a.views().map(|v| v.list).collect::<Vec<_>>())?;
    lists.into_iter().find(|&l| unsafe { SendMessageW(l, LVM_GETHEADER, None, None) }.0 == header.0 as isize)
}

/// Right-click on the column headers: show or hide every column but Name.
fn column_menu(hwnd: HWND, list: HWND, lp: LPARAM) {
    let width = |i: usize| unsafe { SendMessageW(list, LVM_GETCOLUMNWIDTH, Some(WPARAM(i)), None) }.0;
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (i, (title, _, _)) in COLUMNS.iter().enumerate().skip(1) {
            let _ = AppendMenuW(
                menu,
                MF_STRING | if width(i) > 0 { MF_CHECKED } else { MF_UNCHECKED },
                i,
                *title,
            );
        }
        let mut at = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
        if lp.0 == -1 {
            let _ = GetCursorPos(&mut at);
        }
        let picked =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        if let Some(&(_, shown_width, _)) = COLUMNS.get(picked).filter(|_| picked > 0) {
            let cx = if width(picked) > 0 { 0 } else { scale(hwnd, shown_width) };
            SendMessageW(list, LVM_SETCOLUMNWIDTH, Some(WPARAM(picked)), Some(LPARAM(cx as isize)));
        }
    }
}

fn scale(hwnd: HWND, px: i32) -> i32 {
    px * unsafe { GetDpiForWindow(hwnd) } as i32 / 96
}

fn layout(hwnd: HWND) {
    let Some((list, status, address, buttons, tree, split, filter, strip)) =
        with_app(|a| (a.view.list, a.status, a.address, a.buttons, a.tree, a.split, a.filter, a.tab_strip))
    else {
        return;
    };
    unsafe {
        SendMessageW(status, WM_SIZE, None, None);
        let (mut client, mut sr) = (RECT::default(), RECT::default());
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(status, &mut sr);
        let (pad, bar, bw) = (scale(hwnd, 6), scale(hwnd, 32), scale(hwnd, 36));
        for (i, b) in buttons.iter().enumerate() {
            let _ = MoveWindow(*b, pad + i as i32 * (bw + scale(hwnd, 2)), pad, bw, bar, true);
        }
        let ax = pad + 3 * (bw + scale(hwnd, 2)) + scale(hwnd, 4);
        let fw = scale(hwnd, 260).min((client.right / 3).max(0));
        let field =
            |left: i32, right: i32| RECT { left, top: pad, right: right.max(left), bottom: pad + bar };
        let fields =
            [field(ax, client.right - fw - 2 * pad), field(client.right - fw - pad, client.right - pad)];
        with_app(|a| a.fields = fields);
        // Text boxes sit borderless inside their fields, vertically centered.
        let line = chrome::line_height(hwnd) + 2;
        let text_top = pad + (bar - line) / 2;
        let f = fields[1];
        let inset = chrome::filter_inset(hwnd);
        let _ = MoveWindow(
            filter,
            f.left + inset,
            text_top,
            (f.right - f.left - inset - scale(hwnd, 8)).max(0),
            line,
            true,
        );
        let a = fields[0];
        let _ = MoveWindow(
            address,
            a.left + scale(hwnd, 10),
            text_top,
            (a.right - a.left - scale(hwnd, 20)).max(0),
            line,
            true,
        );
        toolbar::layout(hwnd, client.right);
        let _ = MoveWindow(strip, 0, scale(hwnd, toolbar::BOTTOM), client.right, tab_height(hwnd), true);
        let top = content_top(hwnd);
        let h = (client.bottom - (sr.bottom - sr.top) - top).max(0);
        let split = tree::clamp_split(split, client.right, GetDpiForWindow(hwnd));
        let gap = splitter_width(hwnd);
        // Vertical tabs sit above the sidebar, a gap apart.
        let tabs_h = match with_app(|a| a.vertical_tabs.then_some(a.tab_list)).flatten() {
            Some(tab_list) => {
                let tabs_h = tabbar::list_height(tab_list, h);
                let _ = MoveWindow(tab_list, 0, top, split, tabs_h, true);
                tabbar::fit_list_column(tab_list);
                tabs_h + gap
            }
            None => 0,
        };
        let _ = MoveWindow(tree, 0, top + tabs_h, split, (h - tabs_h).max(0), true);
        let rest = (client.right - split - gap).max(0);
        // Preview pane, when shown: a third of the space right of the sidebar.
        let preview_w = if preview::is_visible() {
            with_app(|a| a.preview_width).flatten().unwrap_or(rest / 3).max(scale(hwnd, 100)).min(rest / 2)
        } else {
            0
        };
        let preview_space = if preview_w > 0 { preview_w + gap } else { 0 };
        let _ = ShowWindow(list, SW_SHOWNA);
        if let Some((other, enabled, right, ratio)) =
            with_app(|a| a.other.as_ref().map(|v| (v.list, a.split_view, a.right_active, a.pane_ratio)))
                .flatten()
        {
            let _ = ShowWindow(other, if enabled { SW_SHOWNA } else { SW_HIDE });
            if enabled {
                let width = (rest - preview_space - gap).max(0);
                let left_width = width * ratio.clamp(100, 900) / 1000;
                let (left, right) = if right { (other, list) } else { (list, other) };
                let _ = MoveWindow(left, split + gap, top, left_width, h, true);
                let _ = MoveWindow(right, split + gap + left_width + gap, top, width - left_width, h, true);
            } else {
                let _ = MoveWindow(list, split + gap, top, rest - preview_space, h, true);
            }
        } else {
            let _ = MoveWindow(list, split + gap, top, rest - preview_space, h, true);
        }
        if let Some(pane) = with_app(|a| a.preview.hwnd) {
            let _ = MoveWindow(pane, client.right - preview_w, top, preview_w, h, true);
        }
    }
    panes::layout_labels(hwnd);
    breadcrumbs::layout();
    if preview::is_visible() {
        preview::resize();
    }
}

fn splitter_width(hwnd: HWND) -> i32 {
    scale(hwnd, 5)
}

#[derive(Clone, Copy)]
enum SplitKind {
    Sidebar,
    Panes,
    Preview,
}

fn client_rect_of(parent: HWND, child: HWND) -> RECT {
    let mut rect = RECT::default();
    unsafe {
        let _ = GetWindowRect(child, &mut rect);
        let mut point = POINT { x: rect.left, y: rect.top };
        let _ = ScreenToClient(parent, &mut point);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        rect = RECT { left: point.x, top: point.y, right: point.x + width, bottom: point.y + height };
    }
    rect
}
fn splitter_at(hwnd: HWND, lp: LPARAM) -> Option<(SplitKind, i32)> {
    let x = (lp.0 & 0xffff) as i16 as i32;
    let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
    if y < content_top(hwnd) {
        return None;
    }
    let (tree, list, other, right, split, preview) = with_app(|a| {
        (a.tree, a.view.list, a.other.as_ref().map(|v| v.list), a.right_active, a.split_view, a.preview.hwnd)
    })?;
    let gap = splitter_width(hwnd);
    let sidebar = client_rect_of(hwnd, tree).right;
    if (sidebar..sidebar + gap).contains(&x) {
        return Some((SplitKind::Sidebar, sidebar));
    }
    if split && let Some(other) = other {
        let edge = client_rect_of(hwnd, if right { other } else { list }).right;
        if (edge..edge + gap).contains(&x) {
            return Some((SplitKind::Panes, edge));
        }
    }
    if preview::is_visible() {
        let edge = client_rect_of(hwnd, preview).left - gap;
        if (edge..edge + gap).contains(&x) {
            return Some((SplitKind::Preview, edge));
        }
    }
    None
}
fn over_splitter(hwnd: HWND, lp: LPARAM) -> bool {
    splitter_at(hwnd, lp).is_some()
}
fn drag_splitter(hwnd: HWND, kind: SplitKind, x: i32) {
    let mut client = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut client);
    }
    match kind {
        SplitKind::Sidebar => {
            let dpi = unsafe { GetDpiForWindow(hwnd) };
            with_app(|a| a.split = tree::clamp_split(x, client.right, dpi));
        }
        SplitKind::Preview => {
            with_app(|a| a.preview_width = Some((client.right - x - splitter_width(hwnd)).max(100)));
        }
        SplitKind::Panes => {
            if let Some((left, right)) = with_app(|a| {
                a.other
                    .as_ref()
                    .map(|v| if a.right_active { (v.list, a.view.list) } else { (a.view.list, v.list) })
            })
            .flatten()
            {
                let left = client_rect_of(hwnd, left).left;
                let width = (client_rect_of(hwnd, right).right - left - splitter_width(hwnd)).max(1);
                with_app(|a| a.pane_ratio = ((x - left) * 1000 / width).clamp(100, 900));
            }
        }
    }
}

/// The tab strip's height; none with the tabs above the sidebar.
fn tab_height(hwnd: HWND) -> i32 {
    if with_app(|a| a.vertical_tabs) == Some(true) { 0 } else { scale(hwnd, 30) }
}

/// Where the sidebar and list start: below the toolbar and the tab strip.
fn content_top(hwnd: HWND) -> i32 {
    scale(hwnd, toolbar::BOTTOM) + tab_height(hwnd)
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
        a.restore_session = settings.restore_session;
        a.split = settings.split.unwrap_or(240);
        a.pane_ratio = settings.pane_ratio;
        a.preview_width = settings.preview_width;
        a.startup_override = Some(settings.last_folder.clone());
        a.view.browser.show_hidden = settings.show_hidden;
        *a.sorts.borrow_mut() =
            crate::browser::FolderSorts { default: settings.sort, folders: settings.folder_sorts.clone() };
        // The active folder takes the new default unless it has a sort of its own.
        let sort = a.view.browser.dir().map_or(settings.sort, |d| a.sorts.borrow().get(d));
        let job = a.view.browser.set_sort(sort);
        (a.view.list, job)
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
    if with_app(|a| a.split_view) != Some(settings.dual_pane) {
        panes::toggle();
    }
    if preview::is_visible() != settings.preview {
        preview::toggle(hwnd);
    }
    if let Some(job) = job {
        spawn_sort(job);
    }
    grid::set(settings.grid);
    update_sort_arrows();
    layout(hwnd);
    save_preferences(settings);
}

fn save_preferences(settings: Settings) {
    let Some(events) = with_app(|a| a.events.clone()) else { return };
    for other in crate::session::preferences(settings) {
        unsafe {
            let _ = PostMessageW(Some(HWND(other as _)), WM_APP_PREFERENCES, WPARAM(0), LPARAM(0));
        }
    }
    let saving = OpGuard::new();
    spawn("settings", move || {
        let _saving = saving;
        if let Err(e) = crate::session::flush_preferences() {
            events.send(Event::Message(format!("Settings could not be saved: {e}")));
        }
    });
}

fn with_view<R>(id: u64, f: impl FnOnce(&mut panes::FolderView) -> R) -> Option<R> {
    with_app(|a| a.view_mut(id).map(f)).flatten()
}
fn view_visible(id: u64) -> bool {
    with_app(|a| a.view.id == id || (a.split_view && a.other.as_ref().is_some_and(|v| v.id == id)))
        == Some(true)
}

/// Active folder list, also used by the window tests and stress runner.
pub fn active_list() -> Option<HWND> {
    with_app(|a| a.view.list)
}

/// The tab list shown above the sidebar with vertical tabs, for the window tests.
pub fn tab_list() -> Option<HWND> {
    with_app(|a| a.tab_list)
}

#[cfg(test)]
mod listing_queue_tests {
    use super::*;

    #[test]
    fn busy_listings_keep_the_latest_request_and_discard_canceled_ones() {
        init_process().unwrap();
        let folder = std::env::temp_dir();
        let hwnd = create_main_window(&folder, &Settings::default()).unwrap();
        if let Some(worker) = with_app(|a| a.view.lister.take()).flatten() {
            worker.join().unwrap();
        }
        with_app(|a| a.live_listers.store(MAX_LISTERS, Ordering::SeqCst));
        go(folder.join("older"));
        go(folder.join("latest"));
        assert_eq!(
            with_app(|a| a.view.queued_list.as_ref().map(|j| j.dir.clone())).flatten(),
            Some(folder.join("latest"))
        );
        assert_eq!(
            with_app(|a| a.view.browser.requested_location().map(|l| l.dir)).flatten(),
            Some(folder.join("latest"))
        );
        with_app(|a| {
            a.view.browser.cancel();
            a.live_listers.store(0, Ordering::SeqCst);
        });
        start_queued_lists();
        assert!(with_app(|a| a.view.queued_list.is_none()).unwrap());
        assert_eq!(with_app(|a| a.live_listers.load(Ordering::SeqCst)), Some(0));
        with_app(|a| a.live_listers.store(MAX_LISTERS, Ordering::SeqCst));
        go(folder);
        with_app(|a| a.live_listers.store(0, Ordering::SeqCst));
        start_queued_lists();
        assert!(with_app(|a| a.view.queued_list.is_none() && a.view.lister.is_some()).unwrap());
        if let Some(worker) = with_app(|a| a.view.lister.take()).flatten() {
            worker.join().unwrap();
        }
        unsafe {
            DestroyWindow(hwnd).unwrap();
        }
    }
}
