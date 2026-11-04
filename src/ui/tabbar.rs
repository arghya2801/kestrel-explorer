//! A tab owns its folder views; switching tabs preserves both panes. Tabs show along the
//! top in a tab strip, or listed above the sidebar ("vertical tabs").
use super::toolbar::TAB_LIST;
use super::{
    child,
    panes::{self, Workspace},
    with_app,
};
use crate::settings::Settings;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};

/// The "+" button's control id; distinct from every top-bar command, as it shares their drawing.
/// Also the command the tab list posts for its "New tab" row.
pub(super) const NEW_TAB: usize = 240;
/// Command for switching to tab `n`: `SELECT_TAB + n`, posted by the tab list.
const SELECT_TAB: usize = 0x1000;

thread_local! {
    /// Text handed to a tooltip; it reads it after the notification returns.
    static TIP: RefCell<Vec<u16>> = const { RefCell::new(Vec::new()) };
    /// The tab under the mouse, and whether the mouse is on its close button.
    static HOT: Cell<(Option<usize>, bool)> = const { Cell::new((None, false)) };
    /// A tab being dragged: its index, where the press started, and whether it has moved enough
    /// to count as a drag.
    static DRAG: Cell<Option<(usize, POINT, bool)>> = const { Cell::new(None) };
    /// The tab list's row under the mouse, and whether the mouse is on its close button.
    static LIST_HOT: Cell<(Option<usize>, bool)> = const { Cell::new((None, false)) };
    /// The tab list's row being dragged.
    static LIST_DRAG: Cell<Option<usize>> = const { Cell::new(None) };
}

pub(super) struct TabSlot {
    pub workspace: Option<Workspace>,
}
impl TabSlot {
    pub(super) fn active(_id: u64) -> Self {
        Self { workspace: None }
    }
}

/// The tab strip, and its "+" (new tab) and "all tabs" buttons. The buttons are the window's
/// own children so they stay put when the strip scrolls: "+" follows the last tab until the
/// strip fills, then waits beside "all tabs" right of the strip.
pub(super) unsafe fn create(parent: HWND) -> Result<(HWND, [HWND; 2])> {
    let strip = unsafe {
        // Clips the "+" button, which sits over the strip's empty end.
        let style = WINDOW_STYLE(TCS_FOCUSNEVER | TCS_TOOLTIPS) | WS_CLIPCHILDREN | WS_CLIPSIBLINGS;
        child(parent, WC_TABCONTROLW, w!(""), style, WINDOW_EX_STYLE(0), 0)?
    };
    // Roomier tabs than the control's default, with space at the right for the close button.
    let (x, y) = (super::scale(parent, PAD_X), super::scale(parent, 4));
    let buttons = unsafe {
        SendMessageW(strip, TCM_SETPADDING, None, Some(LPARAM(((y << 16) | x) as isize)));
        SendMessageW(strip, TCM_SETMINTABWIDTH, None, Some(LPARAM(super::scale(parent, 96) as isize)));
        let _ = windows::Win32::UI::Shell::SetWindowSubclass(strip, Some(strip_proc), 3, 0);
        let plus = child(parent, w!("BUTTON"), w!("New tab"), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), NEW_TAB)?;
        let all = child(parent, w!("BUTTON"), w!("All tabs"), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), TAB_LIST)?;
        for b in [plus, all] {
            let _ = SetWindowPos(b, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        [plus, all]
    };
    insert_label(strip, 0, "");
    Ok((strip, buttons))
}

/// Horizontal padding each side of a tab's text: the left inset plus room for the close button.
const PAD_X: i32 = 24;
/// The close button's size, and its gap from the tab's right edge.
const CLOSE: i32 = 16;
const CLOSE_GAP: i32 = 8;

/// Tab `i`'s rectangle as drawn: from just below the strip's top to its bottom.
pub(super) fn tab_rect(strip: HWND, i: usize) -> RECT {
    let mut r = RECT::default();
    unsafe {
        SendMessageW(strip, TCM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize)));
        let mut client = RECT::default();
        let _ = GetClientRect(strip, &mut client);
        r.top = client.top + super::scale(strip, 4);
        r.bottom = client.bottom;
    }
    r
}

/// The close button inside tab rectangle `r`.
pub(super) fn close_rect(strip: HWND, r: &RECT) -> RECT {
    let (size, gap) = (super::scale(strip, CLOSE), super::scale(strip, CLOSE_GAP));
    let mid = (r.top + r.bottom) / 2;
    RECT {
        left: r.right - gap - size,
        top: mid - size / 2,
        right: r.right - gap,
        bottom: mid - size / 2 + size,
    }
}

/// Where a tab's text goes inside `r`: after the left inset, before the close button.
pub(super) fn text_rect(strip: HWND, r: &RECT) -> RECT {
    let close = close_rect(strip, r);
    RECT { left: r.left + super::scale(strip, 12), right: close.left - super::scale(strip, 4), ..*r }
}

/// The tab under the mouse, and whether the mouse is on its close button.
pub(super) fn hot() -> (Option<usize>, bool) {
    HOT.with(Cell::get)
}

/// Whether tab `i` shows its close button: the active tab, the tab under the mouse, and not in
/// high contrast, where Windows draws the strip.
pub(super) fn shows_close(strip: HWND, i: usize) -> bool {
    let active = unsafe { SendMessageW(strip, TCM_GETCURSEL, None, None) }.0;
    !crate::theme::high_contrast() && (i as isize == active || hot().0 == Some(i))
}

/// Space right of the strip for the "+" and "all tabs" buttons once the strip fills.
pub(super) fn reserve(hwnd: HWND) -> i32 {
    let size = super::scale(hwnd, 24);
    2 * size + 3 * super::scale(hwnd, 4)
}

/// The vertical tab list: one row per tab, under a "Tabs (n)" heading with the "+" button.
/// Hidden until vertical tabs are on.
pub(super) unsafe fn create_list(parent: HWND) -> Result<HWND> {
    unsafe {
        let style = LVS_REPORT | LVS_NOCOLUMNHEADER | LVS_SINGLESEL | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS;
        let list = child(parent, WC_LISTVIEWW, w!(""), WINDOW_STYLE(style), WINDOW_EX_STYLE(0), 0)?;
        let _ = ShowWindow(list, SW_HIDE);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER | LVS_EX_INFOTIP;
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(ex as usize)),
            Some(LPARAM(ex as isize)),
        );
        let col = LVCOLUMNW { mask: LVCF_WIDTH, cx: 100, ..Default::default() };
        SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(0)), Some(LPARAM(&col as *const _ as isize)));
        let _ = windows::Win32::UI::Shell::SetWindowSubclass(list, Some(list_proc), 4, 0);
        Ok(list)
    }
}

/// The "Tabs (n)" heading's height above the tab list.
pub(super) fn heading_height(hwnd: HWND) -> i32 {
    super::scale(hwnd, 30)
}

/// Height for the tab list within `space`. `chosen` (96-DPI pixels) is the height the
/// divider was dragged to; otherwise every row, if they fit in two fifths of `space`.
/// Either way at least one row, and the sidebar keeps some room.
pub(super) fn list_height(list: HWND, space: i32, chosen: Option<i32>) -> i32 {
    let rows = unsafe { SendMessageW(list, LVM_GETITEMCOUNT, None, None) }.0.max(1) as usize;
    let size = |rows: usize| {
        let r =
            unsafe { SendMessageW(list, LVM_APPROXIMATEVIEWRECT, Some(WPARAM(rows)), Some(LPARAM(-1))) }.0;
        ((r >> 16) & 0xFFFF) as i32 + super::scale(list, 4)
    };
    let one = size(1);
    let height = match chosen {
        Some(h) => super::scale(list, h),
        None => size(rows).min(space * 2 / 5),
    };
    height.min(space - super::scale(list, 100)).max(one)
}

/// Where the "Tabs (n)" heading is drawn, in the window's coordinates.
pub(super) fn heading_rect(hwnd: HWND) -> Option<RECT> {
    let (vertical, split) = with_app(|a| (a.vertical_tabs, a.split))?;
    let top = super::content_top(hwnd);
    vertical.then(|| RECT { left: 0, top, right: split, bottom: top + heading_height(hwnd) })
}

/// The heading's text: "Tabs (6)".
pub(super) fn heading_text() -> String {
    format!("Tabs ({})", with_app(|a| a.tabs.len()).unwrap_or(0))
}

/// Shows the tabs above the sidebar (`on`) or along the top, and remembers the choice.
pub(super) fn set_vertical(on: bool) {
    let Some((hwnd, strip, list)) = with_app(|a| {
        a.vertical_tabs = on;
        (a.hwnd, a.tab_strip, a.tab_list)
    }) else {
        return;
    };
    unsafe {
        let _ = ShowWindow(strip, if on { SW_HIDE } else { SW_SHOWNA });
        let _ = ShowWindow(list, if on { SW_SHOWNA } else { SW_HIDE });
    }
    refresh();
    super::layout(hwnd);
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, true);
    }
    if let Some(s) = super::current_settings(hwnd) {
        super::save_preferences(s);
    }
}
/// Puts "+" just right of the last tab, or right of the strip once the tabs fill it (when the
/// strip's scroll arrows appear), and "all tabs" at the far right. With the tabs above the
/// sidebar, both leave the top row.
pub(super) fn place_buttons() {
    let Some((hwnd, strip, [plus, all], vertical)) =
        with_app(|a| (a.hwnd, a.tab_strip, a.tab_buttons, a.vertical_tabs))
    else {
        return;
    };
    unsafe {
        if vertical {
            let _ = ShowWindow(all, SW_HIDE);
            if let Some(head) = heading_rect(hwnd) {
                let size = super::scale(hwnd, 24);
                let (x, y) = (
                    head.right - size - super::scale(hwnd, 6),
                    head.top + (head.bottom - head.top - size) / 2,
                );
                let _ = MoveWindow(plus, x, y, size, size, true);
                let _ = ShowWindow(plus, SW_SHOWNA);
            }
            return;
        }
        let bounds = super::client_rect_of(hwnd, strip);
        let (gap, size) = (super::scale(hwnd, 4), super::scale(hwnd, 24));
        let top = bounds.top + super::scale(hwnd, 4);
        let count = SendMessageW(strip, TCM_GETITEMCOUNT, None, None).0.max(1) as usize;
        let last = tab_rect(strip, count - 1);
        // The strip's own scroll arrows mean it's full. Their style, not IsWindowVisible, which
        // is false while the window itself is hidden.
        let arrows = FindWindowExW(Some(strip), None, w!("msctls_updown32"), None)
            .ok()
            .filter(|&u| GetWindowLongPtrW(u, GWL_STYLE) as u32 & WS_VISIBLE.0 != 0);
        let scrolled = tab_rect(strip, 0).left < 0;
        let mut x = bounds.left + last.right + gap;
        if arrows.is_some() || scrolled || x + size > bounds.right {
            x = bounds.right + gap;
        }
        let _ = MoveWindow(plus, x, top, size, size, true);
        let _ = MoveWindow(all, bounds.right + 2 * gap + size, top, size, size, true);
        for b in [plus, all] {
            let _ = ShowWindow(b, SW_SHOWNA);
        }
    }
}

/// The menu under "all tabs": every tab, the active one checked, and Reopen closed tab.
fn all_tabs_menu() {
    let Some((hwnd, all, active, can_reopen)) =
        with_app(|a| (a.hwnd, a.tab_buttons[1], a.tabs.active(), !a.closed_tabs.is_empty()))
    else {
        return;
    };
    let labels = labels();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (i, (label, tip)) in labels.iter().enumerate() {
            let check = if i == active { MF_CHECKED } else { MF_UNCHECKED };
            let text = HSTRING::from(format!("{}\t{}", label.replace('&', "&&"), tip.replace('&', "&&")));
            let _ = AppendMenuW(menu, MF_STRING | check, i + 1, &text);
        }
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let reopen_flag = if can_reopen { MF_ENABLED } else { MF_GRAYED };
        let reopen_id = labels.len() + 1;
        let _ = AppendMenuW(menu, MF_STRING | reopen_flag, reopen_id, w!("&Reopen closed tab\tCtrl+Shift+T"));
        let mut r = RECT::default();
        let _ = GetWindowRect(all, &mut r);
        let flags = TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_RIGHTALIGN;
        let cmd = TrackPopupMenu(menu, flags, r.right, r.bottom, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        match cmd {
            0 => {}
            c if c == reopen_id => reopen(),
            c => switch_to(c - 1),
        }
    }
}
fn insert_label(strip: HWND, index: usize, text: &str) {
    let text = HSTRING::from(text);
    let item = TCITEMW { mask: TCIF_TEXT, pszText: PWSTR(text.as_ptr() as *mut _), ..Default::default() };
    unsafe {
        SendMessageW(strip, TCM_INSERTITEMW, Some(WPARAM(index)), Some(LPARAM(&item as *const _ as isize)))
    };
}
fn set_label(strip: HWND, index: usize, text: &str) {
    let text = HSTRING::from(text);
    let item = TCITEMW { mask: TCIF_TEXT, pszText: PWSTR(text.as_ptr() as *mut _), ..Default::default() };
    unsafe {
        SendMessageW(strip, TCM_SETITEMW, Some(WPARAM(index)), Some(LPARAM(&item as *const _ as isize)))
    };
}
fn label_of(strip: HWND, index: usize) -> String {
    let mut buf = [0u16; 260];
    let mut item =
        TCITEMW { mask: TCIF_TEXT, pszText: PWSTR(buf.as_mut_ptr()), cchTextMax: 260, ..Default::default() };
    unsafe {
        SendMessageW(strip, TCM_GETITEMW, Some(WPARAM(index)), Some(LPARAM(&mut item as *mut _ as isize)))
    };
    String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)])
}

/// The folder a view shows, or is on its way to.
fn shown_dir(view: &panes::FolderView) -> Option<PathBuf> {
    view.browser.dir().map(PathBuf::from).or_else(|| view.browser.requested_location().map(|l| l.dir))
}

/// Each tab includes its parent folder; deeper collisions add ancestors. A split tab is named
/// by its left pane, so clicking between panes doesn't rename it. The tooltip is the full path,
/// or both panes' paths.
fn labels() -> Vec<(String, String)> {
    let shown = with_app(|a| {
        (0..a.tabs.len())
            .map(|i| {
                let (view, other, split, right) = match a.tabs.items()[i].workspace.as_ref() {
                    Some(w) => (&w.view, w.other.as_ref(), w.split, w.right_active),
                    None => (&a.view, a.other.as_ref(), a.split_view, a.right_active),
                };
                let other = other.filter(|_| split);
                let (left, right) = match other {
                    Some(o) if right => (o, Some(view)),
                    Some(o) => (view, Some(o)),
                    None => (view, None),
                };
                (shown_dir(left), left.browser.query().map(String::from), right.and_then(shown_dir))
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let folders: Vec<PathBuf> = shown.iter().filter(|s| s.1.is_none()).filter_map(|s| s.0.clone()).collect();
    let mut names = crate::places::tab_names(&folders).into_iter();
    shown
        .into_iter()
        .map(|(dir, query, right)| {
            let (label, tip) = match (dir, query) {
                (Some(dir), Some(q)) => {
                    (format!("Search: {q}"), format!("Search for \"{q}\" in {}", dir.display()))
                }
                (Some(dir), None) => (names.next().unwrap_or_default(), dir.display().to_string()),
                (None, _) => ("Loading…".into(), String::new()),
            };
            match right {
                Some(right) => (label, format!("{tip}  |  {}", right.display())),
                None => (label, tip),
            }
        })
        .collect()
}

/// Brings the tab labels, and the tab list when shown, up to date.
pub(super) fn refresh() {
    let Some((strip, list, vertical, active)) =
        with_app(|a| (a.tab_strip, a.tab_list, a.vertical_tabs, a.tabs.active()))
    else {
        return;
    };
    let labels = labels();
    for (i, (text, _)) in labels.iter().enumerate() {
        if label_of(strip, i) != *text {
            set_label(strip, i, text);
        }
    }
    if vertical {
        fill_list(list, &labels, active);
    }
}

fn fill_list(list: HWND, labels: &[(String, String)], active: usize) {
    let folder = with_app(|a| a.view.icons.folder).unwrap_or(0);
    unsafe {
        let rows = labels.len();
        let selected = LVIS_SELECTED.0 | LVIS_FOCUSED.0;
        let mut count = SendMessageW(list, LVM_GETITEMCOUNT, None, None).0.max(0) as usize;
        let resized = count != rows;
        while count < rows {
            let item = LVITEMW { iItem: count as i32, ..Default::default() };
            SendMessageW(list, LVM_INSERTITEMW, None, Some(LPARAM(&item as *const _ as isize)));
            count += 1;
        }
        while count > rows {
            count -= 1;
            SendMessageW(list, LVM_DELETEITEM, Some(WPARAM(count)), None);
        }
        for (i, text) in labels.iter().map(|l| l.0.as_str()).enumerate() {
            let text = HSTRING::from(text);
            let item = LVITEMW {
                mask: LVIF_TEXT | LVIF_IMAGE | LVIF_STATE,
                iItem: i as i32,
                iImage: folder,
                pszText: PWSTR(text.as_ptr() as *mut _),
                state: LIST_VIEW_ITEM_STATE_FLAGS(if i == active { selected } else { 0 }),
                stateMask: LIST_VIEW_ITEM_STATE_FLAGS(selected),
                ..Default::default()
            };
            SendMessageW(list, LVM_SETITEMW, None, Some(LPARAM(&item as *const _ as isize)));
        }
        SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(active)), Some(LPARAM(0)));
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, false);
        if let Some(hwnd) = with_app(|a| a.hwnd) {
            if let Some(head) = heading_rect(hwnd) {
                let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), Some(&head), true);
            }
            if resized {
                super::layout(hwnd);
            }
        }
    }
}

/// Fits the list's one column to its width, after the list was sized.
pub(super) fn fit_list_column(list: HWND) {
    unsafe {
        SendMessageW(
            list,
            LVM_SETCOLUMNWIDTH,
            Some(WPARAM(0)),
            Some(LPARAM(LVSCW_AUTOSIZE_USEHEADER as isize)),
        );
    }
}

/// Commands the tab list posts to the window. Returns false for ids that aren't ours.
pub(super) fn execute(id: usize) -> bool {
    use super::toolbar::*;
    let (active, len) = with_app(|a| (a.tabs.active(), a.tabs.len())).unwrap_or_default();
    match id {
        NEW_TAB => new_tab(None),
        TAB_LIST => all_tabs_menu(),
        REOPEN_TAB => reopen(),
        CLOSE_TAB => close_tab(active),
        DUPLICATE_TAB => duplicate(active),
        NEXT_TAB => cycle(false),
        PREV_TAB => cycle(true),
        CLOSE_OTHER_TABS => close_others(active),
        CLOSE_TABS_AFTER => close_after(active),
        MOVE_TAB_BACK if active > 0 => move_tab(active, active - 1),
        MOVE_TAB_FORWARD if active + 1 < len => move_tab(active, active + 1),
        MOVE_TAB_BACK | MOVE_TAB_FORWARD => {}
        id if id >= SELECT_TAB && id - SELECT_TAB < with_app(|a| a.tabs.len()).unwrap_or(0) => {
            switch_to(id - SELECT_TAB)
        }
        _ => return false,
    }
    true
}

/// Writes `text` where a tooltip will read it.
fn tip(text: &str) -> PWSTR {
    TIP.with(|t| {
        let mut t = t.borrow_mut();
        *t = text.encode_utf16().chain([0]).collect();
        PWSTR(t.as_mut_ptr())
    })
}

/// The tab strip's tooltip asking for a tab's text: its full path.
pub(super) fn tooltip(hdr: &NMHDR, lp: LPARAM) -> Option<LRESULT> {
    let strip = with_app(|a| a.tab_strip)?;
    let tips = unsafe { SendMessageW(strip, TCM_GETTOOLTIPS, None, None) }.0;
    if hdr.code != TTN_GETDISPINFOW || tips == 0 || hdr.hwndFrom.0 as isize != tips {
        return None;
    }
    let info = unsafe { &mut *(lp.0 as *mut NMTTDISPINFOW) };
    let text = labels().get(hdr.idFrom).map(|l| l.1.clone()).unwrap_or_default();
    info.lpszText = tip(&text);
    Some(LRESULT(0))
}

/// Posts the command for choosing row `row` of the tab list: switching to that tab, or a
/// new tab from the last row. Posted, so the list isn't changed while it handles the click.
fn choose_row(row: usize) {
    let Some((hwnd, count, active)) = with_app(|a| (a.hwnd, a.tabs.len(), a.tabs.active())) else {
        return;
    };
    let id = match row {
        row if row == active => return,
        row if row < count => SELECT_TAB + row,
        _ => NEW_TAB,
    };
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(id), LPARAM(0));
    }
}

/// Notifications from the tab list. A click chooses a row; moving the selection with the
/// arrow keys only moves the cursor, and Enter chooses (see `on_key`).
pub(super) fn on_list_notify(hdr: &NMHDR, lp: LPARAM) -> Option<LRESULT> {
    let (hwnd, count) = with_app(|a| (a.hwnd, a.tabs.len()))?;
    match hdr.code {
        NM_CLICK => {
            let nm = unsafe { &*(lp.0 as *const NMITEMACTIVATE) };
            if let Ok(row) = usize::try_from(nm.iItem) {
                choose_row(row);
            }
            Some(LRESULT(0))
        }
        NM_CUSTOMDRAW if !crate::theme::high_contrast() => {
            let cd = unsafe { &*(lp.0 as *const NMLVCUSTOMDRAW) };
            Some(LRESULT(match cd.nmcd.dwDrawStage {
                CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as isize,
                CDDS_ITEMPREPAINT => {
                    unsafe { draw_row(cd) };
                    CDRF_SKIPDEFAULT as isize
                }
                _ => CDRF_DODEFAULT as isize,
            }))
        }
        LVN_BEGINDRAG => {
            let nm = unsafe { &*(lp.0 as *const NMLISTVIEW) };
            if let Ok(row) = usize::try_from(nm.iItem) {
                LIST_DRAG.with(|d| d.set(Some(row)));
                unsafe {
                    windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(hdr.hwndFrom);
                }
            }
            Some(LRESULT(0))
        }
        LVN_GETINFOTIPW => {
            let nm = unsafe { &mut *(lp.0 as *mut NMLVGETINFOTIPW) };
            let text = labels().get(nm.iItem as usize).map(|l| l.1.clone()).unwrap_or_default();
            let wide: Vec<u16> = text.encode_utf16().collect();
            if nm.cchTextMax > 0 && !nm.pszText.is_null() {
                let n = wide.len().min(nm.cchTextMax as usize - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), nm.pszText.0, n);
                    *nm.pszText.0.add(n) = 0;
                }
            }
            Some(LRESULT(0))
        }
        NM_RCLICK => {
            let nm = unsafe { &*(lp.0 as *const NMITEMACTIVATE) };
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            let tab = usize::try_from(nm.iItem).ok().filter(|&t| t < count);
            menu(hwnd, tab, pt);
            Some(LRESULT(1))
        }
        _ => None,
    }
}

/// Row `row`'s rectangle in the tab list.
fn row_rect(list: HWND, row: usize) -> RECT {
    let mut r = RECT { left: LVIR_BOUNDS as i32, ..Default::default() };
    unsafe {
        SendMessageW(list, LVM_GETITEMRECT, Some(WPARAM(row)), Some(LPARAM(&mut r as *mut _ as isize)))
    };
    r
}

/// The close button inside row rectangle `r`.
fn row_close_rect(list: HWND, r: &RECT) -> RECT {
    let (size, gap) = (super::scale(list, CLOSE), super::scale(list, 6));
    let mid = (r.top + r.bottom) / 2;
    RECT {
        left: r.right - gap - size,
        top: mid - size / 2,
        right: r.right - gap,
        bottom: mid - size / 2 + size,
    }
}

/// Whether row `row` shows its close button: the active tab and the row under the mouse.
fn row_shows_close(row: usize) -> bool {
    !crate::theme::high_contrast()
        && (with_app(|a| a.tabs.active()) == Some(row) || LIST_HOT.with(Cell::get).0 == Some(row))
}

/// The tab list's row at `pt`.
fn row_at(list: HWND, pt: POINT) -> Option<usize> {
    let mut hit = LVHITTESTINFO { pt, ..Default::default() };
    let row = unsafe { SendMessageW(list, LVM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))) }.0;
    usize::try_from(row).ok()
}

/// A bold copy of the window font, for the active tab's row; made once per font.
fn bold_font() -> Option<windows::Win32::Graphics::Gdi::HFONT> {
    use windows::Win32::Graphics::Gdi::*;
    with_app(|a| {
        if let Some((base, bold)) = a.tab_bold
            && base == a.font
        {
            return Some(bold);
        }
        let mut lf = LOGFONTW::default();
        unsafe {
            GetObjectW(HGDIOBJ(a.font.0), size_of::<LOGFONTW>() as i32, Some(&mut lf as *mut _ as *mut _));
        }
        lf.lfWeight = FW_SEMIBOLD.0 as i32;
        let bold = unsafe { CreateFontIndirectW(&lf) };
        if let Some((_, old)) = a.tab_bold.replace((a.font, bold)) {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(old.0));
            }
        }
        Some(bold)
    })
    .flatten()
}

/// Paints one row of the tab list: the active tab tinted with an accent bar and bold name, the
/// parent dimmed, and a close button on the active and hovered rows.
unsafe fn draw_row(cd: &NMLVCUSTOMDRAW) {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::Graphics::Gdi::*;
    let (list, hdc, row) = (cd.nmcd.hdr.hwndFrom, cd.nmcd.hdc, cd.nmcd.dwItemSpec);
    let Some((p, active, font)) = with_app(|a| (a.palette, a.tabs.active(), a.font)) else { return };
    let Some(label) = labels().get(row).map(|l| l.0.clone()) else { return };
    let r = row_rect(list, row);
    let (hot, close_hot) = LIST_HOT.with(Cell::get);
    let is_active = row == active;
    let back = if is_active {
        crate::theme::mix(p.window, p.accent, if p.dark { 30 } else { 16 })
    } else if hot == Some(row) {
        crate::theme::mix(p.window, p.text, 7)
    } else {
        p.window
    };
    let s = |n| super::scale(list, n);
    unsafe {
        let brush = CreateSolidBrush(COLORREF(back));
        FillRect(hdc, &r, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        if is_active {
            let bar = CreateSolidBrush(COLORREF(p.accent));
            let mark = RECT {
                left: r.left + s(3),
                top: r.top + s(4),
                right: r.left + s(6),
                bottom: r.bottom - s(4),
            };
            FillRect(hdc, &mark, bar);
            let _ = DeleteObject(HGDIOBJ(bar.0));
        }
        let close = row_close_rect(list, &r);
        let mut area = RECT { left: r.left + s(12), right: close.left - s(4), ..r };
        let old = SelectObject(hdc, HGDIOBJ(if is_active { bold_font().unwrap_or(font) } else { font }.0));
        SetBkMode(hdc, TRANSPARENT);
        let mut text: Vec<u16> = label.encode_utf16().collect();
        let split = text.windows(3).position(|w| w == [0x20, 0xB7, 0x20]).unwrap_or(text.len());
        let mut size = windows::Win32::Foundation::SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &text[..split], &mut size);
        let (name, rest) = text.split_at_mut(split);
        let flags = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX;
        let mut name_area = RECT { right: (area.left + size.cx).min(area.right), ..area };
        SetTextColor(hdc, COLORREF(p.text));
        DrawTextW(hdc, name, &mut name_area, flags);
        SelectObject(hdc, HGDIOBJ(font.0));
        if !rest.is_empty() && name_area.right < area.right {
            area.left = name_area.right;
            SetTextColor(hdc, COLORREF(crate::theme::mix(back, p.text, 55)));
            DrawTextW(hdc, rest, &mut area, flags);
        }
        SelectObject(hdc, old);
        if row_shows_close(row) {
            let mut close = close;
            let pressing = hot == Some(row) && close_hot;
            if pressing {
                let ring = crate::theme::mix(back, p.text, 14);
                super::chrome::round_outline(hdc, &close, ring, ring, s(4));
            }
            super::chrome::icon(hdc, list, '\u{E711}', &mut close, p.text, DT_CENTER);
        }
        let focused = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == list;
        if focused && focused_row(list) == Some(row) {
            let ring = RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 };
            super::chrome::outline(hdc, &ring, p.text, s(3), 1);
        }
    }
}

/// Mouse handling for the tab list: close buttons, hover, dragging rows to reorder, and
/// middle-click to close.
unsafe extern "system" fn list_proc(
    list: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        ReleaseCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    let invalidate = || unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(list), None, false);
    };
    let pt = point(lp);
    match msg {
        WM_MBUTTONUP => {
            if let Some(row) = row_at(list, pt) {
                crate::crash::guard(|| close_tab(row));
                return LRESULT(0);
            }
        }
        WM_LBUTTONDOWN => {
            if let Some(row) = row_at(list, pt)
                && row_shows_close(row)
                && unsafe {
                    windows::Win32::Graphics::Gdi::PtInRect(&row_close_rect(list, &row_rect(list, row)), pt)
                }
                .as_bool()
            {
                crate::crash::guard(|| close_tab(row));
                return LRESULT(0);
            }
        }
        WM_MOUSEMOVE => {
            if let Some(from) = LIST_DRAG.with(Cell::get) {
                let count = with_app(|a| a.tabs.len()).unwrap_or(1).max(1);
                let to = row_at(list, POINT { x: 1, ..pt }).unwrap_or(if pt.y < 0 { 0 } else { count - 1 });
                if to != from {
                    LIST_DRAG.with(|d| d.set(Some(to)));
                    crate::crash::guard(|| move_tab(from, to));
                }
                return LRESULT(0);
            }
            let row = row_at(list, pt);
            let on_close = row.is_some_and(|r| {
                unsafe {
                    windows::Win32::Graphics::Gdi::PtInRect(&row_close_rect(list, &row_rect(list, r)), pt)
                }
                .as_bool()
            });
            if LIST_HOT.with(|h| h.replace((row, on_close))) != (row, on_close) {
                invalidate();
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: list,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
        }
        WM_MOUSELEAVE => {
            LIST_HOT.with(|h| h.set((None, false)));
            invalidate();
        }
        WM_LBUTTONUP if LIST_DRAG.with(Cell::get).is_some() => {
            unsafe {
                let _ = ReleaseCapture();
            }
            return LRESULT(0);
        }
        WM_CAPTURECHANGED => LIST_DRAG.with(|d| d.set(None)),
        WM_SETFOCUS | WM_KILLFOCUS => invalidate(),
        _ => {}
    }
    unsafe { windows::Win32::UI::Shell::DefSubclassProc(list, msg, wp, lp) }
}

/// The tab menu, from the strip or the list. `tab` is the tab right-clicked, if any.
fn menu(owner: HWND, tab: Option<usize>, at: POINT) {
    let Some((vertical, count, can_reopen)) =
        with_app(|a| (a.vertical_tabs, a.tabs.len(), !a.closed_tabs.is_empty()))
    else {
        return;
    };
    let on = |yes: bool| if yes { MF_ENABLED } else { MF_GRAYED };
    let (first, last) = (tab == Some(0), tab.is_some_and(|t| t + 1 >= count));
    let items: [(u32, &str, bool); 9] = if vertical {
        [
            (1, "&New tab\tCtrl+T", true),
            (3, "&Duplicate tab\tCtrl+Shift+K", tab.is_some()),
            (5, "&Reopen closed tab\tCtrl+Shift+T", can_reopen),
            (6, "Move &up", tab.is_some() && !first),
            (7, "Move do&wn", tab.is_some() && !last),
            (2, "&Close tab\tCtrl+W", tab.is_some() && count > 1),
            (8, "Close &other tabs", tab.is_some() && count > 1),
            (9, "Close tabs &below", tab.is_some() && !last),
            (4, "Tabs &above the sidebar", true),
        ]
    } else {
        [
            (1, "&New tab\tCtrl+T", true),
            (3, "&Duplicate tab\tCtrl+Shift+K", tab.is_some()),
            (5, "&Reopen closed tab\tCtrl+Shift+T", can_reopen),
            (6, "Move &left", tab.is_some() && !first),
            (7, "Move &right", tab.is_some() && !last),
            (2, "&Close tab\tCtrl+W", tab.is_some() && count > 1),
            (8, "Close &other tabs", tab.is_some() && count > 1),
            (9, "Close tabs to the r&ight", tab.is_some() && !last),
            (4, "Tabs &above the sidebar", true),
        ]
    };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (i, (id, text, enabled)) in items.into_iter().enumerate() {
            if matches!(i, 3 | 5 | 8) {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            }
            let check = if id == 4 && vertical { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | on(enabled) | check, id as usize, &HSTRING::from(text));
        }
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, owner, None).0;
        let _ = DestroyMenu(menu);
        match (cmd, tab) {
            (1, _) => new_tab(None),
            (2, Some(tab)) => close_tab(tab),
            (3, Some(tab)) => duplicate(tab),
            (4, _) => set_vertical(!vertical),
            (5, _) => reopen(),
            (6, Some(tab)) => move_tab(tab, tab - 1),
            (7, Some(tab)) => move_tab(tab, tab + 1),
            (8, Some(tab)) => close_others(tab),
            (9, Some(tab)) => close_after(tab),
            _ => {}
        }
    }
}

/// Shows the tab menu for the active tab under it, as Shift+F10 or the menu key would.
fn keyboard_menu(control: HWND) {
    let Some((hwnd, active, vertical)) = with_app(|a| (a.hwnd, a.tabs.active(), a.vertical_tabs)) else {
        return;
    };
    let tab = if vertical { focused_row(control) } else { Some(active) };
    let mut r = RECT { left: LVIR_BOUNDS as i32, ..Default::default() };
    let rect = Some(LPARAM(&mut r as *mut _ as isize));
    unsafe {
        let at_tab = Some(WPARAM(tab.unwrap_or(active)));
        SendMessageW(control, if vertical { LVM_GETITEMRECT } else { TCM_GETITEMRECT }, at_tab, rect);
        let mut at = POINT { x: r.left, y: r.bottom };
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(control, &mut at);
        menu(hwnd, tab.filter(|&t| t < with_app(|a| a.tabs.len()).unwrap_or(0)), at);
    }
}

/// The tab list's keyboard cursor.
fn focused_row(list: HWND) -> Option<usize> {
    let row = unsafe {
        SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize)))
    }
    .0;
    usize::try_from(row).ok()
}

/// Keys pressed while the tab strip or the tab list has the focus. `None`: not a tab key.
/// The strip's arrow keys stay with the control, which switches tabs as they move.
pub(super) fn on_key(
    control: HWND,
    key: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY,
    shift: bool,
) -> Option<bool> {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let list = with_app(|a| a.tab_list)?;
    let row = if control == list { focused_row(list) } else { with_app(|a| a.tabs.active()) };
    match key {
        VK_RETURN | VK_SPACE if control == list => choose_row(row?),
        VK_DELETE => close_tab(row?),
        VK_APPS => keyboard_menu(control),
        VK_F10 if shift => keyboard_menu(control),
        _ => return None,
    }
    Some(true)
}
/// Makes a folder view for a new tab, set up like the active one.
fn create_view() -> Option<panes::FolderView> {
    let (hwnd, id, settings, events, font) = with_app(|a| {
        let id = a.next_tab_id;
        a.next_tab_id += 1;
        (
            a.hwnd,
            id,
            Settings {
                grid: a.view.grid.enabled,
                thumb_size: a.view.grid.size,
                sort: a.view.browser.sort,
                show_hidden: a.view.browser.show_hidden,
                ..Default::default()
            },
            a.events.clone(),
            a.font,
        )
    })?;
    match unsafe { panes::create(hwnd, &settings, id, events, font) } {
        Ok(v) => Some(v),
        Err(e) => {
            super::set_status(&format!("Couldn't create tab: {e}"));
            None
        }
    }
}

/// Inserts a tab for `view` right of the active one, without switching to it. Returns its index.
fn insert(view: panes::FolderView) -> Option<usize> {
    let (strip, index) = with_app(|a| {
        let from = a.tabs.active();
        let index = a.tabs.open(TabSlot {
            workspace: Some(Workspace { view, other: None, split: false, right_active: false, ratio: 500 }),
        });
        a.tabs.select(from);
        (a.tab_strip, index)
    })?;
    insert_label(strip, index, "");
    Some(index)
}

pub(super) fn new_tab(dir: Option<PathBuf>) {
    if let Some(device) = dir.as_ref().filter(|p| crate::places::is_shell_device(p)) {
        super::go(device.clone());
        return;
    }
    let dir = dir.or_else(|| with_app(|a| a.view.browser.requested_location().map(|l| l.dir)).flatten());
    let Some(view) = create_view() else { return };
    let Some(index) = insert(view) else { return };
    switch_to(index);
    if let Some(dir) = dir {
        super::go(dir);
    }
    refresh();
}

/// Opens `dir` in a new tab right of the active one and stays on the active tab, as a
/// middle-click on a link does in a browser.
pub(super) fn open_in_background(dir: PathBuf) {
    if crate::places::is_shell_device(&dir) {
        return super::go(dir);
    }
    let Some(mut view) = create_view() else { return };
    let job = view.browser.go(dir);
    if insert(view).is_some() {
        super::spawn_list(job);
        refresh();
    }
}

/// Tab `index` as the session saves it: its folders and split.
fn saved_tab(index: usize) -> Option<crate::settings::SavedTab> {
    with_app(|a| {
        let (view, other, split, right_active, ratio) = match &a.tabs.items().get(index)?.workspace {
            Some(w) => (&w.view, w.other.as_ref(), w.split, w.right_active, w.ratio),
            None => (&a.view, a.other.as_ref(), a.split_view, a.right_active, a.pane_ratio),
        };
        Some(crate::settings::SavedTab {
            folder: view.browser.requested_location().map(|l| l.dir).unwrap_or_default(),
            other: other.and_then(|v| v.browser.requested_location().map(|l| l.dir)),
            split,
            right_active,
            ratio,
        })
    })
    .flatten()
}

/// Gives the active tab `tab`'s second pane and split. Its first folder is already open.
fn apply_split(tab: &crate::settings::SavedTab) {
    if let Some(dir) = &tab.other {
        if with_app(|a| a.other.is_none()) == Some(true) {
            panes::toggle();
        }
        if let Some(job) = with_app(|a| a.other.as_mut().map(|v| v.browser.go(dir.clone()))).flatten() {
            super::spawn_list(job);
        }
    }
    with_app(|a| {
        a.split_view = tab.split && a.other.is_some();
        a.right_active = tab.right_active;
        a.pane_ratio = tab.ratio;
    });
}

/// Opens `tab` (folders and split) in a new tab and switches to it.
fn open_saved(tab: &crate::settings::SavedTab) {
    if tab.folder.as_os_str().is_empty() {
        return;
    }
    new_tab(Some(tab.folder.clone()));
    apply_split(tab);
    if let Some(hwnd) = with_app(|a| a.hwnd) {
        super::layout(hwnd);
    }
    super::update_chrome();
    refresh();
}

/// Opens a new tab with the same folders and split as tab `index`.
pub(super) fn duplicate(index: usize) {
    if let Some(tab) = saved_tab(index) {
        open_saved(&tab);
    }
}

/// How many closed tabs Ctrl+Shift+T can bring back.
const CLOSED_KEPT: usize = 10;

pub(super) fn close_tab(index: usize) {
    let Some((active, len, strip)) = with_app(|a| (a.tabs.active(), a.tabs.len(), a.tab_strip)) else {
        return;
    };
    if len <= 1 || index >= len {
        return;
    }
    if let Some(saved) = saved_tab(index) {
        with_app(|a| {
            a.closed_tabs.push(saved);
            if a.closed_tabs.len() > CLOSED_KEPT {
                a.closed_tabs.remove(0);
            }
        });
    }
    if index == active {
        switch_to(if index + 1 < len { index + 1 } else { index - 1 });
    }
    let closed = with_app(|a| a.tabs.close(index)).flatten();
    drop(closed);
    unsafe {
        SendMessageW(strip, TCM_DELETEITEM, Some(WPARAM(index)), None);
        let active = with_app(|a| a.tabs.active()).unwrap_or(0);
        SendMessageW(strip, TCM_SETCURSEL, Some(WPARAM(active)), None);
    }
    refresh();
}
pub(super) fn close_active() {
    if let Some(i) = with_app(|a| a.tabs.active()) {
        close_tab(i);
    }
}
/// Brings back the most recently closed tab (Ctrl+Shift+T).
pub(super) fn reopen() {
    if let Some(tab) = with_app(|a| a.closed_tabs.pop()).flatten() {
        open_saved(&tab);
    }
}
/// Closes every tab but `keep`.
pub(super) fn close_others(keep: usize) {
    close_after(keep);
    for i in (0..keep).rev() {
        close_tab(i);
    }
}
/// Closes the tabs right of (or below) `index`.
pub(super) fn close_after(index: usize) {
    let len = with_app(|a| a.tabs.len()).unwrap_or(0);
    for i in (index + 1..len).rev() {
        close_tab(i);
    }
}
/// Moves tab `from` to position `to`.
pub(super) fn move_tab(from: usize, to: usize) {
    let Some((strip, active)) = with_app(|a| {
        a.tabs.move_item(from, to);
        (a.tab_strip, a.tabs.active())
    }) else {
        return;
    };
    unsafe {
        SendMessageW(strip, TCM_SETCURSEL, Some(WPARAM(active)), None);
    }
    refresh();
}
/// Ctrl+1 to Ctrl+8 pick that tab; Ctrl+9 the last, as in browsers.
pub(super) fn select_number(n: usize) {
    let len = with_app(|a| a.tabs.len()).unwrap_or(0);
    let to = if n == 9 { len.saturating_sub(1) } else { n - 1 };
    if to < len {
        switch_to(to);
    }
}
pub(super) fn cycle(back: bool) {
    if let Some(i) = with_app(|a| if back { a.tabs.prev_index() } else { a.tabs.next_index() }) {
        switch_to(i);
    }
}
pub(super) fn switch_to(to: usize) {
    let Some((from, ids, lists)) = with_app(|a| {
        if to == a.tabs.active() || to >= a.tabs.len() {
            return None;
        }
        let views = std::iter::once(&a.view).chain(a.other.iter());
        Some((
            a.tabs.active(),
            views.clone().map(|v| v.id).collect::<Vec<_>>(),
            views.flat_map(|v| [v.list, v.label]).collect::<Vec<_>>(),
        ))
    })
    .flatten() else {
        return;
    };
    for id in ids {
        super::stop_watching_view(id);
    }
    for list in lists {
        unsafe {
            SendMessageW(list, LVM_CANCELEDITLABEL, None, None);
            let _ = ShowWindow(list, SW_HIDE);
        }
    }
    let Some((hwnd, strip, list, ids)) = with_app(|a| {
        let mut incoming = a.tabs.items_mut()[to].workspace.take()?;
        std::mem::swap(&mut a.view, &mut incoming.view);
        std::mem::swap(&mut a.other, &mut incoming.other);
        std::mem::swap(&mut a.split_view, &mut incoming.split);
        std::mem::swap(&mut a.right_active, &mut incoming.right_active);
        std::mem::swap(&mut a.pane_ratio, &mut incoming.ratio);
        a.tabs.items_mut()[from].workspace = Some(incoming);
        a.tabs.select(to);
        let mut ids = vec![a.view.id];
        if a.split_view
            && let Some(other) = &a.other
        {
            ids.push(other.id);
        }
        Some((a.hwnd, a.tab_strip, a.view.list, ids))
    })
    .flatten() else {
        return;
    };
    // One repaint for the whole switch instead of one per control moved, shown or hidden.
    let quiet = unsafe { IsWindowVisible(hwnd) }.as_bool();
    if quiet {
        unsafe { SendMessageW(hwnd, WM_SETREDRAW, Some(WPARAM(0)), None) };
    }
    unsafe {
        SendMessageW(strip, TCM_SETCURSEL, Some(WPARAM(to)), None);
    }
    super::preview::invalidate();
    super::layout(hwnd);
    for id in ids {
        super::watch_view(id);
        if let Some(job) = super::with_view(id, |v| v.browser.on_dir_changed()).flatten() {
            super::spawn_list(job);
        }
    }
    super::update_sort_arrows();
    super::update_chrome();
    if quiet {
        unsafe {
            use windows::Win32::Graphics::Gdi::*;
            SendMessageW(hwnd, WM_SETREDRAW, Some(WPARAM(1)), None);
            let _ = RedrawWindow(
                Some(hwnd),
                None,
                None,
                RDW_INVALIDATE | RDW_ERASE | RDW_ALLCHILDREN | RDW_UPDATENOW,
            );
        }
    }
    // Arrowing along a focused tab strip keeps the focus there.
    unsafe {
        if windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() != strip {
            let _ = SetFocus(Some(list));
        }
    }
}

fn point(lp: LPARAM) -> POINT {
    POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 }
}

/// The tab at `pt` (strip coordinates).
fn tab_at(strip: HWND, pt: POINT) -> Option<usize> {
    let mut hit = TCHITTESTINFO { pt, ..Default::default() };
    let tab = unsafe { SendMessageW(strip, TCM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))) }.0;
    usize::try_from(tab).ok()
}

/// Whether `pt` is on tab `tab`'s close button, while it shows one.
fn on_close(strip: HWND, tab: usize, pt: POINT) -> bool {
    let r = close_rect(strip, &tab_rect(strip, tab));
    shows_close(strip, tab) && unsafe { windows::Win32::Graphics::Gdi::PtInRect(&r, pt) }.as_bool()
}

/// Mouse handling for the strip: the close buttons, hover, dragging tabs to reorder them,
/// middle-click to close and the wheel to scroll, as in browsers.
unsafe extern "system" fn strip_proc(
    strip: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        ReleaseCapture, SetCapture, TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    let invalidate = || unsafe {
        let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(strip), None, false);
    };
    match msg {
        WM_MBUTTONUP => {
            if let Some(tab) = tab_at(strip, point(lp)) {
                crate::crash::guard(|| close_tab(tab));
                return LRESULT(0);
            }
        }
        WM_LBUTTONDOWN => {
            let pt = point(lp);
            if let Some(tab) = tab_at(strip, pt) {
                if on_close(strip, tab, pt) {
                    crate::crash::guard(|| close_tab(tab));
                    return LRESULT(0);
                }
                DRAG.with(|d| d.set(Some((tab, pt, false))));
            }
        }
        WM_MOUSEMOVE => {
            let pt = point(lp);
            let tab = tab_at(strip, pt);
            let hot = (tab, tab.is_some_and(|t| on_close(strip, t, pt)));
            if HOT.with(|h| h.replace(hot)) != hot {
                invalidate();
            }
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: strip,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
            if let Some((from, start, dragging)) = DRAG.with(Cell::get)
                && wp.0 & windows::Win32::System::SystemServices::MK_LBUTTON.0 as usize != 0
            {
                let far = unsafe { GetSystemMetrics(SM_CXDRAG) };
                if !dragging && (pt.x - start.x).abs() < far {
                    return LRESULT(0);
                }
                if !dragging {
                    unsafe {
                        SetCapture(strip);
                    }
                }
                let count = unsafe { SendMessageW(strip, TCM_GETITEMCOUNT, None, None) }.0.max(1) as usize;
                // Past either end of the strip counts as the first or last tab.
                let to = tab_at(strip, POINT { y: tab_rect(strip, 0).top + 1, ..pt })
                    .unwrap_or(if pt.x < tab_rect(strip, 0).left { 0 } else { count - 1 });
                DRAG.with(|d| d.set(Some((to, start, true))));
                if to != from {
                    crate::crash::guard(|| move_tab(from, to));
                }
                return LRESULT(0);
            }
        }
        WM_MOUSELEAVE => {
            HOT.with(|h| h.set((None, false)));
            invalidate();
        }
        WM_LBUTTONUP => {
            if DRAG.with(|d| d.take()).is_some_and(|(_, _, dragging)| dragging) {
                unsafe {
                    let _ = ReleaseCapture();
                }
                return LRESULT(0);
            }
        }
        WM_CAPTURECHANGED => DRAG.with(|d| d.set(None)),
        WM_SETFOCUS | WM_KILLFOCUS => invalidate(),
        // The wheel scrolls a strip too full to show every tab.
        WM_MOUSEWHEEL => {
            let count = unsafe { SendMessageW(strip, TCM_GETITEMCOUNT, None, None) }.0.max(0) as usize;
            let first = (0..count).find(|&i| tab_rect(strip, i).left >= 0).unwrap_or(0);
            let back = (wp.0 >> 16) as i16 > 0;
            let to = if back { first.saturating_sub(1) } else { (first + 1).min(count.saturating_sub(1)) };
            unsafe {
                SendMessageW(strip, WM_HSCROLL, Some(WPARAM((to << 16) | SB_THUMBPOSITION.0 as usize)), None);
            }
            invalidate();
            place_buttons();
            return LRESULT(0);
        }
        _ => {}
    }
    let result = unsafe { windows::Win32::UI::Shell::DefSubclassProc(strip, msg, wp, lp) };
    if matches!(msg, TCM_INSERTITEMW | TCM_DELETEITEM | TCM_SETITEMW | WM_SIZE | WM_SETFONT | WM_HSCROLL) {
        place_buttons();
    }
    result
}

/// Notifications from the tab strip.
pub(super) fn on_notify(hdr: &NMHDR) -> Option<LRESULT> {
    let strip = hdr.hwndFrom;
    match hdr.code {
        TCN_SELCHANGE => {
            let to = unsafe { SendMessageW(strip, TCM_GETCURSEL, None, None) }.0;
            if let Ok(to) = usize::try_from(to) {
                switch_to(to);
            }
            Some(LRESULT(0))
        }
        NM_RCLICK => {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
                let screen = pt;
                let _ = ScreenToClient(strip, &mut pt);
                let mut hit = TCHITTESTINFO { pt, ..Default::default() };
                let tab = SendMessageW(strip, TCM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))).0;
                menu(GetParent(strip).unwrap_or_default(), usize::try_from(tab).ok(), screen);
            }
            Some(LRESULT(1))
        }
        _ => None,
    }
}

pub(super) fn saved() -> (Vec<crate::settings::SavedTab>, usize) {
    let len = with_app(|a| a.tabs.len()).unwrap_or(0);
    ((0..len).filter_map(saved_tab).collect(), with_app(|a| a.tabs.active()).unwrap_or(0))
}
pub(super) fn restore(settings: &Settings) {
    for (i, tab) in settings.tabs.iter().enumerate() {
        if i > 0 {
            new_tab(Some(tab.folder.clone()));
        } else {
            super::go(tab.folder.clone());
        }
        apply_split(tab);
    }
    switch_to(settings.active_tab.min(settings.tabs.len().saturating_sub(1)));
    if let Some(hwnd) = with_app(|a| a.hwnd) {
        super::layout(hwnd);
    }
}
