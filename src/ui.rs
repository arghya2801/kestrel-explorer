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
    COLOR_BTNFACE, CreateFontIndirectW, DeleteObject, HBRUSH, HFONT, HGDIOBJ, InvalidateRect, ScreenToClient,
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
use crate::listing::{Snapshot, list_dir};
use crate::sort::{Sort, SortKey, sorted_order};
use crate::{crash, format, shell};

mod tree;

const CLASS: PCWSTR = w!("FileExplorerPro");
const WM_APP_EVENTS: u32 = WM_APP + 1;
const ID_BACK: usize = 1;
const ID_FORWARD: usize = 2;
const ID_UP: usize = 3;
/// Listing threads stuck on dead network paths we tolerate before refusing new ones.
const MAX_LISTERS: usize = 8;
const COLUMNS: [(PCWSTR, i32, SortKey); 3] = [
    (w!("Name"), 320, SortKey::Name),
    (w!("Date modified"), 160, SortKey::Modified),
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
        order: Vec<u32>,
    },
    /// A shortcut resolved to a folder: show it.
    Navigate(PathBuf),
    /// One-off status bar message.
    Message(String),
    TreeChildren {
        node: usize,
        result: io::Result<Vec<String>>,
    },
    DriveLabel {
        node: usize,
        text: String,
    },
}

struct App {
    hwnd: HWND,
    list: HWND,
    address: HWND,
    buttons: [HWND; 3],
    status: HWND,
    font: HFONT,
    browser: Browser,
    events: Arc<Events<Event>>,
    /// Thread listing the folder being loaded, so Esc can cancel its blocking I/O.
    lister: Option<JoinHandle<()>>,
    live_listers: Arc<AtomicUsize>,
    tree: HWND,
    nodes: Vec<tree::Node>,
    /// Sidebar width in pixels.
    split: i32,
    /// While dragging the splitter: where in the gap it was grabbed.
    drag_offset: Option<i32>,
}

impl App {
    fn controls(&self) -> [HWND; 7] {
        let [b0, b1, b2] = self.buttons;
        [self.list, self.status, self.address, self.tree, b0, b1, b2]
    }
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
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
            dwICC: ICC_STANDARD_CLASSES | ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES | ICC_TREEVIEW_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    Ok(())
}

unsafe fn child(
    parent: HWND,
    class: PCWSTR,
    text: PCWSTR,
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

pub fn create_main_window(start: &Path) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as _),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly if already registered
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("File Explorer Pro"),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            700,
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
        let list = child(
            hwnd,
            WC_LISTVIEWW,
            w!(""),
            WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS),
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
                cx: scale(hwnd, *width),
                pszText: PWSTR(title.0 as *mut _),
                ..Default::default()
            };
            SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }
        let status = child(hwnd, STATUSCLASSNAMEW, w!(""), WINDOW_STYLE(SBARS_SIZEGRIP), none, 0)?;
        let tree = tree::create(hwnd)?;

        let font = message_font(hwnd);
        set_font(&[list, status, address, tree, buttons[0], buttons[1], buttons[2]], font);

        let target = hwnd.0 as isize;
        let events = Arc::new(Events::new(move || {
            PostMessageW(Some(HWND(target as _)), WM_APP_EVENTS, WPARAM(0), LPARAM(0)).is_ok()
        }));
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App {
                hwnd,
                list,
                address,
                buttons,
                status,
                font,
                browser: Browser::default(),
                events,
                lister: None,
                live_listers: Arc::new(AtomicUsize::new(0)),
                tree,
                nodes: Vec::new(),
                split: scale(hwnd, 240),
                drag_offset: None,
            })
        });

        tree::populate();
        layout(hwnd);
        update_sort_arrows();
        let _ = SetFocus(Some(list));
        go(start.to_path_buf());
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
    if msg.message != WM_KEYDOWN && msg.message != WM_SYSKEYDOWN {
        return false;
    }
    crash::guard(|| on_key(msg)).unwrap_or(false)
}

fn on_key(msg: &MSG) -> bool {
    let Some((list, address, tree)) = with_app(|a| (a.list, a.address, a.tree)) else { return false };
    if ![list, address, tree].contains(&msg.hwnd) {
        return false;
    }
    let key = VIRTUAL_KEY(msg.wParam.0 as u16);
    let in_address = msg.hwnd == address;
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    if msg.message == WM_SYSKEYDOWN {
        match key {
            VK_LEFT => run(Browser::back),
            VK_RIGHT => run(Browser::forward),
            VK_UP => run(Browser::up),
            VK_D => focus_address(),
            _ => return false,
        }
        return true;
    }
    match key {
        VK_BACK if !in_address => run(Browser::back),
        VK_F5 => run(Browser::refresh),
        VK_F4 => focus_address(),
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
        VK_TAB => unsafe {
            // Focus order: address bar, sidebar, list.
            let order = [address, tree, list];
            let at = order.iter().position(|&h| h == msg.hwnd).unwrap_or(0);
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let next = if shift { (at + 2) % 3 } else { (at + 1) % 3 };
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
                match wp.0 & 0xFFFF {
                    ID_BACK => run(Browser::back),
                    ID_FORWARD => run(Browser::forward),
                    ID_UP => run(Browser::up),
                    _ => return DefWindowProcW(hwnd, msg, wp, lp),
                }
                if let Some(list) = with_app(|a| a.list) {
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
            WM_DESTROY => {
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
    let (list, tree) = with_app(|a| (a.list, a.tree))?;
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
                if item.mask.contains(LVIF_TEXT) {
                    with_app(|a| fill_text(&a.browser, item));
                }
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

fn fill_text(b: &Browser, item: &mut LVITEMW) {
    let (Some(s), Ok(row)) = (b.snapshot(), usize::try_from(item.iItem)) else { return write(item, &[]) };
    let Some(&i) = b.order.get(row) else { return write(item, &[]) };
    let i = i as usize;
    match item.iSubItem {
        0 => write(item, s.name(i)),
        1 => write(item, &format::time(s.entries[i].modified).encode_utf16().collect::<Vec<_>>()),
        2 if !s.is_dir(i) => write(item, &format::size(s.entries[i].size).encode_utf16().collect::<Vec<_>>()),
        _ => write(item, &[]),
    }
}

/// Copies `text` into the control's buffer, truncating to fit.
fn write(item: &mut LVITEMW, text: &[u16]) {
    if item.pszText.is_null() || item.cchTextMax <= 0 {
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
                    if let Some(job) = with_app(|a| a.browser.sort_job()).flatten() {
                        spawn_sort(job);
                    }
                }
                Some(Applied::Failed) => update_chrome(),
                _ => {}
            }
        }
        Event::Sorted { generation, sort, show_hidden, order } => {
            let sel = selected_rows();
            if let Some(Applied::Shown { select, .. }) =
                with_app(|a| a.browser.on_sorted(generation, sort, show_hidden, order, &sel))
            {
                refresh(&select, false);
            }
        }
        Event::Navigate(dir) => go(dir),
        Event::Message(text) => set_status(&text),
        Event::TreeChildren { node, result } => tree::on_children(node, result),
        Event::DriveLabel { node, text } => tree::on_label(node, text),
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
    let Some((events, live)) = with_app(|a| (a.events.clone(), a.live_listers.clone())) else { return };
    live.fetch_add(1, Ordering::SeqCst);
    let worker = spawn("list", move || {
        struct Done(Arc<AtomicUsize>);
        impl Drop for Done {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _done = Done(live);
        let ListJob { generation, dir, sort, show_hidden } = job;
        let result = crash::guard(|| {
            list_dir(&dir).map(|s| {
                let order = sorted_order(&s, sort, show_hidden);
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

fn cancel_loading() {
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
        if let Some(order) = crash::guard(|| sorted_order(&job.snapshot, job.sort, job.show_hidden)) {
            events.send(Event::Sorted {
                generation: job.generation,
                sort: job.sort,
                show_hidden: job.show_hidden,
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
    let Some((list, count)) = with_app(|a| (a.list, a.browser.order.len())) else { return };
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
    let Some((hwnd, buttons, enabled, title)) = with_app(|a| {
        let b = &a.browser;
        let title = b.dir().map(|d| d.display().to_string()).unwrap_or_default();
        (a.hwnd, a.buttons, [b.can_back(), b.can_forward(), b.can_up()], title)
    }) else {
        return;
    };
    unsafe {
        for (button, on) in buttons.iter().zip(enabled) {
            let _ = EnableWindow(*button, on);
        }
        let _ = SetWindowTextW(hwnd, &HSTRING::from(format!("{title} - File Explorer Pro")));
    }
    show_location();
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
    let Some((list, status, address, buttons, tree, split)) =
        with_app(|a| (a.list, a.status, a.address, a.buttons, a.tree, a.split))
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
        let _ = MoveWindow(address, ax, pad, (client.right - ax - pad).max(0), bar, true);
        let top = bar + 2 * pad;
        let h = (client.bottom - (sr.bottom - sr.top) - top).max(0);
        let split = tree::clamp_split(split, client.right, GetDpiForWindow(hwnd));
        let gap = splitter_width(hwnd);
        let _ = MoveWindow(tree, 0, top, split, h, true);
        let _ = MoveWindow(list, split + gap, top, (client.right - split - gap).max(0), h, true);
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
    let top = scale(hwnd, 28) + 2 * scale(hwnd, 4);
    y >= top && x >= split && x < split + splitter_width(hwnd)
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
