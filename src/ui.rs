//! Main window. Rules:
//! - The UI thread never touches disk, network or shell; workers report via `Events`.
//! - Never call into Win32 while `APP` is borrowed: controls send messages
//!   back synchronously, and a re-entrant borrow would fail.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateFontIndirectW, DeleteObject, HFONT, HGDIOBJ};
use windows::Win32::System::Diagnostics::Debug::{
    SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetErrorMode,
};
use windows::Win32::System::LibraryLoader::{
    GetModuleHandleW, LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SystemParametersInfoForDpi};
use windows::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_BACK};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};

use crate::browser::{Browser, SortJob};
use crate::events::Events;
use crate::listing::{Snapshot, list_dir};
use crate::sort::{Sort, SortKey, sorted_order};
use crate::{crash, format};

const CLASS: PCWSTR = w!("FileExplorerPro");
const WM_APP_EVENTS: u32 = WM_APP + 1;
const COLUMNS: [(PCWSTR, i32, SortKey); 3] = [
    (w!("Name"), 320, SortKey::Name),
    (w!("Date modified"), 160, SortKey::Modified),
    (w!("Size"), 100, SortKey::Size),
];

enum Event {
    Listed { generation: u64, result: io::Result<(Snapshot, Vec<u32>)> },
    Sorted { generation: u64, sort: Sort, order: Vec<u32> },
}

struct App {
    list: HWND,
    status: HWND,
    font: HFONT,
    browser: Browser,
    events: Arc<Events<Event>>,
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
            dwICC: ICC_STANDARD_CLASSES | ICC_LISTVIEW_CLASSES | ICC_BAR_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);
    }
    Ok(())
}

pub fn create_main_window(start: &Path) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?.into();
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinstance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc); // fails harmlessly if already registered
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            CLASS,
            w!("File Explorer Pro"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1100,
            700,
            None,
            None,
            Some(hinstance),
            None,
        )?;

        let list = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            WC_LISTVIEWW,
            None,
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS),
            0,
            0,
            0,
            0,
            Some(hwnd),
            None,
            Some(hinstance),
            None,
        )?;
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(ex as usize)),
            Some(LPARAM(ex as isize)),
        );
        let _ = SetWindowTheme(list, w!("Explorer"), None);
        let scale = |px: i32| px * GetDpiForWindow(hwnd) as i32 / 96;
        for (i, (title, width, key)) in COLUMNS.iter().enumerate() {
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if *key == SortKey::Size { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                cx: scale(*width),
                pszText: PWSTR(title.0 as *mut _),
                ..Default::default()
            };
            SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }

        let status = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            STATUSCLASSNAMEW,
            None,
            WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SBARS_SIZEGRIP),
            0,
            0,
            0,
            0,
            Some(hwnd),
            None,
            Some(hinstance),
            None,
        )?;

        let font = message_font(hwnd);
        set_font(&[list, status], font);

        let target = hwnd.0 as isize;
        let events = Arc::new(Events::new(move || {
            PostMessageW(Some(HWND(target as _)), WM_APP_EVENTS, WPARAM(0), LPARAM(0)).is_ok()
        }));
        APP.with(|cell| {
            *cell.borrow_mut() = Some(App { list, status, font, browser: Browser::default(), events })
        });

        layout(hwnd);
        update_sort_arrows();
        let _ = SetFocus(Some(list));
        navigate(start.to_path_buf());
        Ok(hwnd)
    }
}

pub fn run_message_loop() {
    let mut msg = MSG::default();
    unsafe {
        // GetMessageW returns -1 on error, 0 on WM_QUIT.
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
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
            WM_SIZE => {
                layout(hwnd);
                LRESULT(0)
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
                if let Some((old, list, status)) =
                    with_app(|a| (std::mem::replace(&mut a.font, font), a.list, a.status))
                {
                    set_font(&[list, status], font);
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
    let list = with_app(|a| a.list)?;
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
                let target = with_app(|a| {
                    let s = a.browser.snapshot()?;
                    let i = *a.browser.order.get(usize::try_from(row).ok()?)? as usize;
                    s.is_dir(i).then(|| s.dir.join(s.name_string(i)))
                });
                if let Some(dir) = target.flatten() {
                    navigate(dir);
                }
                Some(LRESULT(0))
            }
            LVN_KEYDOWN => {
                if (*(lp.0 as *const NMLVKEYDOWN)).wVKey == VK_BACK.0 {
                    let parent = with_app(|a| a.browser.dir().and_then(Path::parent).map(Path::to_path_buf));
                    if let Some(p) = parent.flatten() {
                        navigate(p);
                    }
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
            if with_app(|a| a.browser.on_listed(generation, result)) == Some(true) {
                refresh(true);
                if let Some(job) = with_app(|a| a.browser.sort_job()).flatten() {
                    spawn_sort(job);
                }
            }
        }
        Event::Sorted { generation, sort, order } => {
            if with_app(|a| a.browser.on_sorted(generation, sort, order)) == Some(true) {
                refresh(false);
            }
        }
    }
}

fn navigate(dir: PathBuf) {
    let Some((generation, sort, show_hidden, events)) = with_app(|a| {
        let g = a.browser.navigate(dir.clone());
        (g, a.browser.sort, a.browser.show_hidden, a.events.clone())
    }) else {
        return;
    };
    spawn("list", move || {
        let result = crash::guard(|| {
            list_dir(&dir).map(|s| {
                let order = sorted_order(&s, sort, show_hidden);
                (s, order)
            })
        })
        .unwrap_or_else(|| Err(io::Error::other("internal error while listing")));
        events.send(Event::Listed { generation, result });
    });
    update_status();
}

fn spawn_sort(job: SortJob) {
    let Some((show_hidden, events)) = with_app(|a| (a.browser.show_hidden, a.events.clone())) else { return };
    spawn("sort", move || {
        if let Some(order) = crash::guard(|| sorted_order(&job.snapshot, job.sort, show_hidden)) {
            events.send(Event::Sorted { generation: job.generation, sort: job.sort, order });
        }
    });
}

/// Starts a worker thread. If the OS refuses, the failure is logged; the UI stays as is.
fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        crash::log(&format!("could not start {name} thread: {e}"));
    }
}

/// Pushes browser state to the controls. `new_folder` resets scroll and selection.
fn refresh(new_folder: bool) {
    let Some((list, count, title)) = with_app(|a| {
        let title = a.browser.dir().map(|d| d.display().to_string()).unwrap_or_default();
        (a.list, a.browser.order.len(), title)
    }) else {
        return;
    };
    unsafe {
        // Selection is by row index, so it is meaningless after any reorder.
        // ponytail: selection dropped on re-sort; remap by name if it annoys.
        let clear = LVITEMW {
            stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
            ..Default::default()
        };
        SendMessageW(
            list,
            LVM_SETITEMSTATE,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(&clear as *const _ as isize)),
        );
        SendMessageW(list, LVM_SETITEMCOUNT, Some(WPARAM(count)), Some(LPARAM(0)));
        if new_folder && count > 0 {
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(0)), Some(LPARAM(0)));
        }
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, false);
        if let Ok(parent) = GetParent(list) {
            let _ = SetWindowTextW(parent, &HSTRING::from(format!("{title} - File Explorer Pro")));
        }
    }
    update_status();
}

fn update_status() {
    let Some((status, text)) = with_app(|a| (a.status, a.browser.status())) else { return };
    let text = HSTRING::from(text);
    unsafe {
        SendMessageW(status, SB_SETTEXTW, Some(WPARAM(0)), Some(LPARAM(text.as_ptr() as isize)));
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

fn layout(hwnd: HWND) {
    let Some((list, status)) = with_app(|a| (a.list, a.status)) else { return };
    unsafe {
        SendMessageW(status, WM_SIZE, None, None);
        let (mut client, mut sr) = (RECT::default(), RECT::default());
        let _ = GetClientRect(hwnd, &mut client);
        let _ = GetWindowRect(status, &mut sr);
        let h = client.bottom - (sr.bottom - sr.top);
        let _ = MoveWindow(list, 0, 0, client.right, h.max(0), true);
    }
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
