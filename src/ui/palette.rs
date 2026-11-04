//! Finding commands and their keys: the command palette (Ctrl+Shift+P), a searchable list of
//! every command with its shortcut, and the toolbar's tooltips, which name the same shortcuts.
//! Both show keys.txt bindings in place of the built-in keys they add to.
use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PWSTR, w};

use super::commands::*;
use super::toolbar::*;
use super::{ID_BACK, ID_FORWARD, ID_UP, child, scale, with_app};

/// A command: where it belongs, what it's called, its built-in keys, and the id that runs it.
pub(super) struct Entry {
    pub group: &'static str,
    pub label: &'static str,
    pub keys: &'static str,
    pub id: usize,
}

const fn e(group: &'static str, label: &'static str, keys: &'static str, id: usize) -> Entry {
    Entry { group, label, keys, id }
}

pub(super) const ENTRIES: &[Entry] = &[
    e("Go", "Back", "Alt+Left", ID_BACK),
    e("Go", "Forward", "Alt+Right", ID_FORWARD),
    e("Go", "Up to the parent folder", "Alt+Up", ID_UP),
    e("Go", "Edit the address", "Ctrl+L", super::breadcrumbs::EDIT),
    e("Go", "Filter or search this folder", "Ctrl+F", FOCUS_FILTER),
    e("Go", "Refresh", "F5", REFRESH),
    e("Go", "Show the open folder in the sidebar", "Ctrl+Shift+E", REVEAL),
    e("Go", "Pin this folder to Quick access", "", PIN),
    e("Go", "Open a terminal here", "", TERMINAL),
    e("Files", "Open", "Enter", CMD_OPEN),
    e("Files", "Open in a new tab", "Middle-click", CMD_OPEN_NEW_TAB),
    e("Files", "Open in the other pane", "", CMD_OPEN_OTHER_PANE),
    e("Files", "New folder", "Ctrl+Shift+N", CMD_NEW_FOLDER),
    e("Files", "Cut", "Ctrl+X", CMD_CUT),
    e("Files", "Copy", "Ctrl+C", CMD_COPY),
    e("Files", "Paste", "Ctrl+V", CMD_PASTE),
    e("Files", "Rename", "F2", CMD_RENAME),
    e("Files", "Delete (to the Recycle Bin)", "Delete", CMD_DELETE),
    e("Files", "Delete permanently", "Shift+Delete", CMD_DELETE_PERMANENTLY),
    e("Files", "Copy as path", "Ctrl+Shift+C", CMD_COPY_PATH),
    e("Files", "Select all", "Ctrl+A", CMD_SELECT_ALL),
    e("Files", "Undo", "Ctrl+Z", CMD_UNDO),
    e("Files", "Redo", "Ctrl+Y", CMD_REDO),
    e("Files", "Properties", "Alt+Enter", CMD_PROPERTIES),
    e("Files", "Show in Windows Explorer", "", CMD_SHOW_IN_EXPLORER),
    e("Files", "Windows' full right-click menu", "Shift+right-click", CMD_MORE_OPTIONS),
    e("Files", "Pin the selected folder to Quick access", "", CMD_PIN),
    e("Files", "Copy to the other pane", "Shift+F5", CMD_COPY_TO_OTHER),
    e("Files", "Move to the other pane", "Shift+F6", CMD_MOVE_TO_OTHER),
    e("Files", "Calculate folder sizes", "", FOLDER_SIZES),
    e("Files", "File operations", "", OPERATIONS),
    e("View", "Details", "", DETAILS),
    e("View", "Thumbnails", "Ctrl+mouse wheel", GRID),
    e("View", "Hidden items", "Ctrl+H", HIDDEN),
    e("View", "Preview pane", "Alt+P", PREVIEW),
    e("View", "Group by date", "", GROUP),
    e("View", "Sort by name", "", SORT_NAME),
    e("View", "Sort by date modified", "", SORT_DATE),
    e("View", "Sort by type", "", SORT_TYPE),
    e("View", "Sort by size", "", SORT_SIZE),
    e("View", "Reverse the sort order", "", DESCENDING),
    e("Panes", "Split into two panes", "Ctrl+Shift+S", SPLIT),
    e("Panes", "Focus the other pane", "F6", FOCUS_OTHER_PANE),
    e("Panes", "Swap panes", "Ctrl+U", SWAP_PANES),
    e("Panes", "Show this folder in the other pane", "Ctrl+Shift+U", MIRROR_PANE),
    e("Panes", "Sync pane navigation", "", SYNC_PANES),
    e("Panes", "Compare panes", "", COMPARE),
    e("Tabs", "New tab", "Ctrl+T", super::tabbar::NEW_TAB),
    e("Tabs", "Close tab", "Ctrl+W", CLOSE_TAB),
    e("Tabs", "Reopen closed tab", "Ctrl+Shift+T", REOPEN_TAB),
    e("Tabs", "Duplicate tab", "Ctrl+Shift+K", DUPLICATE_TAB),
    e("Tabs", "Next tab", "Ctrl+Tab", NEXT_TAB),
    e("Tabs", "Previous tab", "Ctrl+Shift+Tab", PREV_TAB),
    e("Tabs", "Go to tab 1 to 8, or the last", "Ctrl+1 to Ctrl+9", 0),
    e("Tabs", "Close other tabs", "", CLOSE_OTHER_TABS),
    e("Tabs", "Close tabs to the right", "", CLOSE_TABS_AFTER),
    e("Tabs", "Move tab left", "", MOVE_TAB_BACK),
    e("Tabs", "Move tab right", "", MOVE_TAB_FORWARD),
    e("Tabs", "List all tabs", "", TAB_LIST),
    e("Tabs", "Tabs on the left", "", VERTICAL_TABS),
    e("Window", "New window", "Ctrl+N", NEW_WINDOW),
    e("Window", "Settings", "", SETTINGS),
    e("Window", "Keyboard shortcuts", "F1", SHORTCUTS),
    e("Window", "Command palette", "Ctrl+Shift+P", PALETTE),
    e("Window", "Check for updates", "", UPDATES),
];

/// The keys for command `id`: a keys.txt binding first, then the built-in keys, if any.
pub(super) fn shortcut(id: usize) -> String {
    let custom = crate::keys::bindings(super::COMMAND_NAMES)
        .iter()
        .find(|b| b.command == id)
        .and_then(|b| b.text.split("    ").next().map(str::to_string));
    custom
        .or_else(|| ENTRIES.iter().find(|e| e.id == id && !e.keys.is_empty()).map(|e| e.keys.into()))
        .unwrap_or_default()
}

/// A menu item's text with its shortcut after a tab, as menus show them.
pub(super) fn menu_text(text: &str, id: usize) -> String {
    match shortcut(id) {
        keys if keys.is_empty() => text.to_string(),
        keys => format!("{text}\t{keys}"),
    }
}

/// Whether every word of `query` appears in the entry's group, label or keys.
fn matches(entry: &Entry, query: &str) -> bool {
    let hay = format!("{} {} {}", entry.group, entry.label, shortcut(entry.id)).to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| hay.contains(word))
}

/// The open palette: its window, search box and list, and the entries the list shows.
type Open = (HWND, HWND, HWND, Vec<usize>);

thread_local! {
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
}

const SEARCH: usize = 1;
const LIST: usize = 2;

/// Opens the palette over `owner`, or closes it if it's open.
pub(super) fn open(owner: HWND) {
    if let Some((hwnd, ..)) = OPEN.with(|o| o.borrow().clone()) {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        return;
    }
    unsafe {
        let class = w!("KestrelPalette");
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let mut r = RECT::default();
        let _ = GetWindowRect(owner, &mut r);
        let width = scale(owner, 600).min(r.right - r.left - scale(owner, 40));
        let height = scale(owner, 420);
        let x = r.left + (r.right - r.left - width) / 2;
        let y = r.top + scale(owner, 70);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class,
            w!("Command palette"),
            WS_POPUP | WS_BORDER,
            x,
            y,
            width,
            height,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ) else {
            return;
        };
        let pad = scale(owner, 8);
        let field = scale(owner, 30);
        let search = child(
            hwnd,
            w!("EDIT"),
            w!(""),
            WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
            WS_EX_CLIENTEDGE,
            SEARCH,
        )
        .unwrap_or_default();
        let style = LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_NOCOLUMNHEADER;
        let list = child(hwnd, WC_LISTVIEWW, w!(""), WINDOW_STYLE(style), WINDOW_EX_STYLE(0), LIST)
            .unwrap_or_default();
        let inner = width - 2;
        let _ = MoveWindow(search, pad, pad, inner - 2 * pad, field, true);
        let _ = MoveWindow(list, pad, 2 * pad + field, inner - 2 * pad, height - 3 * pad - field - 2, true);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(ex as usize)),
            Some(LPARAM(ex as isize)),
        );
        let keys_width = scale(owner, 170);
        for (i, cx) in [inner - 2 * pad - keys_width - scale(owner, 24), keys_width].into_iter().enumerate() {
            let col = LVCOLUMNW { mask: LVCF_WIDTH, cx, ..Default::default() };
            SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }
        if let Some(font) = with_app(|a| a.font) {
            super::set_font(&[search, list], font);
        }
        SendMessageW(
            search,
            EM_SETCUEBANNER,
            Some(WPARAM(1)),
            Some(LPARAM(w!("Type a command or a key, such as \"tab\" or \"Ctrl+W\"").as_ptr() as isize)),
        );
        super::set_accessible_name(list, w!("Commands"));
        super::dark::style_new_list(list);
        super::dark::apply_dialog(hwnd);
        let _ = SetWindowSubclass(search, Some(search_proc), 1, 0);
        OPEN.with(|o| *o.borrow_mut() = Some((hwnd, search, list, Vec::new())));
        fill("");
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(Some(search));
    }
}

/// Shows the entries matching `query`, the first selected.
fn fill(query: &str) {
    let Some((_, _, list, _)) = OPEN.with(|o| o.borrow().clone()) else { return };
    let shown: Vec<usize> = (0..ENTRIES.len()).filter(|&i| matches(&ENTRIES[i], query)).collect();
    unsafe {
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(0)), None);
        SendMessageW(list, LVM_DELETEALLITEMS, None, None);
        for (row, &i) in shown.iter().enumerate() {
            let entry = &ENTRIES[i];
            let label = HSTRING::from(format!("{}: {}", entry.group, entry.label));
            let item = LVITEMW {
                mask: LVIF_TEXT,
                iItem: row as i32,
                pszText: PWSTR(label.as_ptr() as *mut _),
                ..Default::default()
            };
            SendMessageW(list, LVM_INSERTITEMW, None, Some(LPARAM(&item as *const _ as isize)));
            let keys = HSTRING::from(shortcut(entry.id));
            let sub = LVITEMW { iSubItem: 1, pszText: PWSTR(keys.as_ptr() as *mut _), ..Default::default() };
            SendMessageW(list, LVM_SETITEMTEXTW, Some(WPARAM(row)), Some(LPARAM(&sub as *const _ as isize)));
        }
        select(list, 0);
        SendMessageW(list, WM_SETREDRAW, Some(WPARAM(1)), None);
    }
    OPEN.with(|o| {
        if let Some(open) = o.borrow_mut().as_mut() {
            open.3 = shown;
        }
    });
}

fn select(list: HWND, row: usize) {
    let on = LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0);
    let item = LVITEMW { state: on, stateMask: on, ..Default::default() };
    unsafe {
        SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(row)), Some(LPARAM(&item as *const _ as isize)));
        SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(row)), Some(LPARAM(0)));
    }
}

fn selected(list: HWND) -> Option<usize> {
    let row = unsafe {
        SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_SELECTED as isize)))
    }
    .0;
    usize::try_from(row).ok()
}

/// Closes the palette and runs the chosen command in the window.
fn run_selected() {
    let Some((hwnd, _, list, shown)) = OPEN.with(|o| o.borrow().clone()) else { return };
    let id = selected(list).and_then(|row| shown.get(row)).map(|&i| ENTRIES[i].id);
    let owner = unsafe { GetWindow(hwnd, GW_OWNER) }.unwrap_or_default();
    unsafe {
        let _ = DestroyWindow(hwnd);
        if let Some(id) = id.filter(|&id| id != 0) {
            let _ = PostMessageW(Some(owner), WM_COMMAND, WPARAM(id), LPARAM(0));
        }
    }
}

/// Up, Down, Page Up and Page Down move through the list while typing; Enter runs, Esc closes.
unsafe extern "system" fn search_proc(
    edit: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let list = OPEN.with(|o| o.borrow().as_ref().map(|o| o.2));
    if let Some(list) = list {
        let count = unsafe { SendMessageW(list, LVM_GETITEMCOUNT, None, None) }.0.max(0) as usize;
        let at = selected(list).unwrap_or(0);
        match (msg, VIRTUAL_KEY(wp.0 as u16)) {
            (WM_KEYDOWN, VK_DOWN) if count > 0 => return select_and(list, (at + 1).min(count - 1)),
            (WM_KEYDOWN, VK_UP) => return select_and(list, at.saturating_sub(1)),
            (WM_KEYDOWN, VK_NEXT) if count > 0 => return select_and(list, (at + 10).min(count - 1)),
            (WM_KEYDOWN, VK_PRIOR) => return select_and(list, at.saturating_sub(10)),
            (WM_KEYDOWN, VK_RETURN) => {
                crate::crash::guard(run_selected);
                return LRESULT(0);
            }
            (WM_KEYDOWN, VK_ESCAPE) => {
                if let Some((hwnd, ..)) = OPEN.with(|o| o.borrow().clone()) {
                    unsafe {
                        let _ = DestroyWindow(hwnd);
                    }
                }
                return LRESULT(0);
            }
            // No beep for the Enter and Esc that were handled above.
            (WM_CHAR, _) if matches!(wp.0, 0x0D | 0x1B) => return LRESULT(0),
            _ => {}
        }
    }
    unsafe { DefSubclassProc(edit, msg, wp, lp) }
}

fn select_and(list: HWND, row: usize) -> LRESULT {
    let none = LVITEMW {
        stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            list,
            LVM_SETITEMSTATE,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(&none as *const _ as isize)),
        );
    }
    select(list, row);
    LRESULT(0)
}

extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::guard(|| unsafe {
        match msg {
            WM_COMMAND if wp.0 & 0xFFFF == SEARCH && ((wp.0 >> 16) & 0xFFFF) as u32 == EN_CHANGE => {
                fill(&super::window_text(HWND(lp.0 as _)));
                Some(LRESULT(0))
            }
            WM_NOTIFY => {
                let hdr = &*(lp.0 as *const NMHDR);
                if hdr.idFrom == LIST && matches!(hdr.code, NM_DBLCLK | LVN_ITEMACTIVATE) {
                    run_selected();
                    return Some(LRESULT(0));
                }
                None
            }
            // Clicking elsewhere closes the palette.
            WM_ACTIVATE if wp.0 & 0xFFFF == WA_INACTIVE as usize => {
                let _ = DestroyWindow(hwnd);
                Some(LRESULT(0))
            }
            WM_CTLCOLOREDIT => super::dark::edit_colors(windows::Win32::Graphics::Gdi::HDC(wp.0 as _)),
            WM_ERASEBKGND if super::dark::erase(hwnd, windows::Win32::Graphics::Gdi::HDC(wp.0 as _)) => {
                Some(LRESULT(1))
            }
            WM_DESTROY => {
                OPEN.with(|o| *o.borrow_mut() = None);
                None
            }
            _ => None,
        }
    })
    .flatten()
    .unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wp, lp) })
}

/// Toolbar tooltips: the button's command and its shortcut, such as "Copy (Ctrl+C)".
pub(super) fn tooltip(hdr: &NMHDR, lp: LPARAM) -> Option<LRESULT> {
    let tips = with_app(|a| a.tips)?;
    if hdr.code != TTN_GETDISPINFOW || hdr.hwndFrom != tips {
        return None;
    }
    let button = HWND(hdr.idFrom as _);
    let id = unsafe { GetDlgCtrlID(button) } as usize;
    let name = ENTRIES
        .iter()
        .find(|e| e.id == id)
        .map(|e| e.label.to_string())
        .unwrap_or_else(|| super::window_text(button).replace('&', ""));
    let text = match shortcut(id) {
        keys if keys.is_empty() => name,
        keys => format!("{name} ({keys})"),
    };
    TIP.with(|t| {
        let mut t = t.borrow_mut();
        *t = text.encode_utf16().chain([0]).collect();
        let info = unsafe { &mut *(lp.0 as *mut NMTTDISPINFOW) };
        info.lpszText = PWSTR(t.as_mut_ptr());
    });
    Some(LRESULT(0))
}

thread_local! {
    static TIP: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
}

/// Makes the tooltip window and gives each of `buttons` a tooltip.
pub(super) fn add_tooltips(owner: HWND, buttons: &[HWND]) -> HWND {
    unsafe {
        let Ok(tips) = CreateWindowExW(
            WS_EX_TOPMOST,
            TOOLTIPS_CLASSW,
            None,
            WINDOW_STYLE(TTS_ALWAYSTIP | TTS_NOPREFIX),
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(owner),
            None,
            None,
            None,
        ) else {
            return HWND::default();
        };
        for &b in buttons {
            let tool = TTTOOLINFOW {
                cbSize: size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_IDISHWND | TTF_SUBCLASS,
                hwnd: owner,
                uId: b.0 as usize,
                // LPSTR_TEXTCALLBACKW: ask with TTN_GETDISPINFO each time.
                lpszText: PWSTR(-1isize as *mut u16),
                ..Default::default()
            };
            SendMessageW(tips, TTM_ADDTOOLW, None, Some(LPARAM(&tool as *const _ as isize)));
        }
        tips
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_listed_once_and_names_its_keys() {
        let mut ids: Vec<usize> = ENTRIES.iter().map(|e| e.id).filter(|&id| id != 0).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "a command id is listed twice");
        assert_eq!(shortcut(CMD_COPY), "Ctrl+C");
        assert_eq!(menu_text("Copy", CMD_COPY), "Copy\tCtrl+C");
        assert_eq!(menu_text("Compare panes", COMPARE), "Compare panes");
        let close = ENTRIES.iter().find(|e| e.id == CLOSE_TAB).unwrap();
        assert!(matches(close, "ctrl+w") && matches(close, "TABS close") && !matches(close, "copy"));
    }
}
