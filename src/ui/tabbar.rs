//! A tab owns its folder views; switching tabs preserves both panes. Tabs show along the
//! top in a tab strip, or listed above the sidebar ("vertical tabs").
use super::{
    child,
    panes::{self, Workspace},
    with_app,
};
use crate::settings::Settings;
use std::cell::RefCell;
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
}

pub(super) struct TabSlot {
    pub workspace: Option<Workspace>,
}
impl TabSlot {
    pub(super) fn active(_id: u64) -> Self {
        Self { workspace: None }
    }
}

pub(super) unsafe fn create(parent: HWND) -> Result<HWND> {
    let strip = unsafe {
        let style = WINDOW_STYLE(TCS_FOCUSNEVER | TCS_TOOLTIPS) | WS_CLIPCHILDREN;
        child(parent, WC_TABCONTROLW, w!(""), style, WINDOW_EX_STYLE(0), 0)?
    };
    // Roomier tabs than the control's default.
    let (x, y) = (super::scale(parent, 16), super::scale(parent, 4));
    unsafe {
        SendMessageW(strip, TCM_SETPADDING, None, Some(LPARAM(((y << 16) | x) as isize)));
        SendMessageW(strip, TCM_SETMINTABWIDTH, None, Some(LPARAM(super::scale(parent, 96) as isize)));
        child(strip, w!("BUTTON"), w!("New tab"), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), NEW_TAB)?;
        let _ = windows::Win32::UI::Shell::SetWindowSubclass(strip, Some(strip_proc), 3, 0);
    }
    insert_label(strip, 0, "");
    Ok(strip)
}

/// The vertical tab list: one row per tab, then "New tab". Hidden until vertical tabs are on.
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

/// Height for the tab list within `space`: every row if they fit in two fifths of it.
pub(super) fn list_height(list: HWND, space: i32) -> i32 {
    let rows = unsafe { SendMessageW(list, LVM_GETITEMCOUNT, None, None) }.0.max(1) as usize;
    let size = unsafe { SendMessageW(list, LVM_APPROXIMATEVIEWRECT, Some(WPARAM(rows)), Some(LPARAM(-1))) }.0;
    (((size >> 16) & 0xFFFF) as i32 + super::scale(list, 4)).min(space * 2 / 5)
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
/// Keeps the "+" button just right of the last tab; hidden once the tabs fill the strip.
fn place_new_tab_button(strip: HWND) {
    unsafe {
        let Ok(button) = GetDlgItem(Some(strip), NEW_TAB as i32) else { return };
        let count = SendMessageW(strip, TCM_GETITEMCOUNT, None, None).0;
        let mut r = RECT::default();
        SendMessageW(
            strip,
            TCM_GETITEMRECT,
            Some(WPARAM(count.max(1) as usize - 1)),
            Some(LPARAM(&mut r as *mut _ as isize)),
        );
        let mut client = RECT::default();
        let _ = GetClientRect(strip, &mut client);
        let top = super::scale(strip, 4);
        let size = (client.bottom - top - super::scale(strip, 2)).max(0);
        let x = r.right + super::scale(strip, 4);
        // ponytail: with more tabs than fit, the strip's scroll arrows take this spot; Ctrl+T still works.
        let fits = x + size <= client.right;
        let _ = ShowWindow(button, if fits { SW_SHOWNA } else { SW_HIDE });
        let _ = MoveWindow(button, x, top, size, size, true);
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

/// Each tab's label and tooltip. Tabs on different folders of the same name add the parent
/// folders that tell them apart; the tooltip is the full path.
fn labels() -> Vec<(String, String)> {
    let shown = with_app(|a| {
        (0..a.tabs.len())
            .map(|i| {
                let view = match a.tabs.items()[i].workspace.as_ref() {
                    Some(w) => &w.view,
                    None => &a.view,
                };
                (view.browser.dir().map(PathBuf::from), view.browser.query().map(String::from))
            })
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let folders: Vec<PathBuf> = shown.iter().filter(|s| s.1.is_none()).filter_map(|s| s.0.clone()).collect();
    let mut names = crate::places::short_names(&folders).into_iter();
    shown
        .into_iter()
        .map(|(dir, query)| match (dir, query) {
            (Some(dir), Some(q)) => {
                (format!("Search: {q}"), format!("Search for \"{q}\" in {}", dir.display()))
            }
            (Some(dir), None) => (names.next().unwrap_or_default(), dir.display().to_string()),
            (None, _) => ("Loading…".into(), String::new()),
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
        let rows = labels.len() + 1;
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
        for (i, text) in labels.iter().map(|l| l.0.as_str()).chain(["+  New tab"]).enumerate() {
            let text = HSTRING::from(text);
            let new_row = i == labels.len();
            let item = LVITEMW {
                mask: LVIF_TEXT | LVIF_IMAGE | LVIF_STATE,
                iItem: i as i32,
                iImage: if new_row { I_IMAGENONE } else { folder },
                pszText: PWSTR(text.as_ptr() as *mut _),
                state: LIST_VIEW_ITEM_STATE_FLAGS(if i == active { selected } else { 0 }),
                stateMask: LIST_VIEW_ITEM_STATE_FLAGS(selected),
                ..Default::default()
            };
            SendMessageW(list, LVM_SETITEMW, None, Some(LPARAM(&item as *const _ as isize)));
        }
        SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(active)), Some(LPARAM(0)));
        if resized && let Some(hwnd) = with_app(|a| a.hwnd) {
            super::layout(hwnd);
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
    match id {
        NEW_TAB => new_tab(None),
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

/// Notifications from the tab list. Choosing a row is posted, so the list isn't changed
/// while it is still handling the click.
pub(super) fn on_list_notify(hdr: &NMHDR, lp: LPARAM) -> Option<LRESULT> {
    let (hwnd, count) = with_app(|a| (a.hwnd, a.tabs.len()))?;
    match hdr.code {
        LVN_ITEMCHANGED => {
            let nm = unsafe { &*(lp.0 as *const NMLISTVIEW) };
            let chosen = nm.uChanged.contains(LVIF_STATE)
                && nm.uNewState & LVIS_SELECTED.0 != 0
                && nm.uOldState & LVIS_SELECTED.0 == 0;
            let active = with_app(|a| a.tabs.active());
            if let Ok(row) = usize::try_from(nm.iItem)
                && chosen
                && Some(row) != active
            {
                let id = if row < count { SELECT_TAB + row } else { NEW_TAB };
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(id), LPARAM(0));
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

/// Middle-clicking a row in the tab list closes that tab.
unsafe extern "system" fn list_proc(
    list: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_MBUTTONUP {
        let pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
        let mut hit = LVHITTESTINFO { pt, ..Default::default() };
        let row =
            unsafe { SendMessageW(list, LVM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))) }.0;
        if let Ok(row) = usize::try_from(row) {
            crate::crash::guard(|| close_tab(row));
            return LRESULT(0);
        }
    }
    unsafe { windows::Win32::UI::Shell::DefSubclassProc(list, msg, wp, lp) }
}

/// The tab menu, from the strip or the list. `tab` is the tab right-clicked, if any.
fn menu(owner: HWND, tab: Option<usize>, at: POINT) {
    let vertical = with_app(|a| a.vertical_tabs).unwrap_or(false);
    let several = with_app(|a| a.tabs.len()).unwrap_or(1) > 1;
    let on = |yes: bool| if yes { MF_ENABLED } else { MF_GRAYED };
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, 1, w!("&New tab\tCtrl+T"));
        let _ = AppendMenuW(menu, MF_STRING | on(tab.is_some()), 3, w!("&Duplicate tab\tCtrl+Shift+K"));
        let _ = AppendMenuW(menu, MF_STRING | on(tab.is_some() && several), 2, w!("&Close tab\tCtrl+W"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let check = if vertical { MF_CHECKED } else { MF_UNCHECKED };
        let _ = AppendMenuW(menu, MF_STRING | check, 4, w!("Tabs &above the sidebar"));
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, owner, None).0;
        let _ = DestroyMenu(menu);
        match (cmd, tab) {
            (1, _) => new_tab(None),
            (2, Some(tab)) => close_tab(tab),
            (3, Some(tab)) => duplicate(tab),
            (4, _) => set_vertical(!vertical),
            _ => {}
        }
    }
}
pub(super) fn new_tab(dir: Option<PathBuf>) {
    let Some((hwnd, id, settings, events, font, dir)) = with_app(|a| {
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
            dir.or_else(|| a.view.browser.requested_location().map(|l| l.dir)),
        )
    }) else {
        return;
    };
    let view = match unsafe { panes::create(hwnd, &settings, id, events, font) } {
        Ok(v) => v,
        Err(e) => return super::set_status(&format!("Couldn't create tab: {e}")),
    };
    let Some((strip, index)) = with_app(|a| {
        let from = a.tabs.active();
        let index = a.tabs.open(TabSlot {
            workspace: Some(Workspace { view, other: None, split: false, right_active: false, ratio: 500 }),
        });
        a.tabs.select(from);
        (a.tab_strip, index)
    }) else {
        return;
    };
    insert_label(strip, index, "Loading…");
    switch_to(index);
    refresh();
    if let Some(dir) = dir {
        super::go(dir);
    }
}
/// Opens a new tab on the folder that tab `index` shows.
pub(super) fn duplicate(index: usize) {
    let dir = with_app(|a| {
        let view = match a.tabs.items().get(index)?.workspace.as_ref() {
            Some(w) => &w.view,
            None => &a.view,
        };
        view.browser.requested_location().map(|l| l.dir)
    })
    .flatten();
    if let Some(dir) = dir {
        new_tab(Some(dir));
    }
}
pub(super) fn close_tab(index: usize) {
    let Some((active, len, strip)) = with_app(|a| (a.tabs.active(), a.tabs.len(), a.tab_strip)) else {
        return;
    };
    if len <= 1 || index >= len {
        return;
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
    let Some((hwnd, strip, palette, list, ids)) = with_app(|a| {
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
        Some((a.hwnd, a.tab_strip, a.palette, a.view.list, ids))
    })
    .flatten() else {
        return;
    };
    unsafe {
        SendMessageW(strip, TCM_SETCURSEL, Some(WPARAM(to)), None);
    }
    super::preview::invalidate();
    super::dark::apply(hwnd, palette);
    super::layout(hwnd);
    for id in ids {
        super::watch_view(id);
        if let Some(job) = super::with_view(id, |v| v.browser.on_dir_changed()).flatten() {
            super::spawn_list(job);
        }
    }
    super::update_sort_arrows();
    super::update_chrome();
    unsafe {
        let _ = SetFocus(Some(list));
    }
}

/// Middle-clicking a tab closes it, as in browsers. Also runs the "+" button, a child
/// of the strip, and moves it whenever the tabs change.
unsafe extern "system" fn strip_proc(
    strip: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_MBUTTONUP => {
            let pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
            let mut hit = TCHITTESTINFO { pt, ..Default::default() };
            let tab =
                unsafe { SendMessageW(strip, TCM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))) }
                    .0;
            if let Ok(tab) = usize::try_from(tab) {
                crate::crash::guard(|| close_tab(tab));
                return LRESULT(0);
            }
        }
        WM_COMMAND if wp.0 & 0xFFFF == NEW_TAB => {
            crate::crash::guard(|| new_tab(None));
            return LRESULT(0);
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lp.0 as *const NMHDR) };
            if hdr.code == NM_CUSTOMDRAW
                && hdr.idFrom == NEW_TAB
                && let Some(r) = crate::crash::guard(|| unsafe {
                    super::chrome::draw_button(&*(lp.0 as *const NMCUSTOMDRAW))
                })
                .flatten()
            {
                return r;
            }
        }
        _ => {}
    }
    let result = unsafe { windows::Win32::UI::Shell::DefSubclassProc(strip, msg, wp, lp) };
    if matches!(msg, TCM_INSERTITEMW | TCM_DELETEITEM | TCM_SETITEMW | WM_SIZE | WM_SETFONT) {
        place_new_tab_button(strip);
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
    with_app(|a| {
        let tabs = a
            .tabs
            .items()
            .iter()
            .map(|t| {
                let (view, other, split, right_active, ratio) = match &t.workspace {
                    Some(w) => (&w.view, w.other.as_ref(), w.split, w.right_active, w.ratio),
                    None => (&a.view, a.other.as_ref(), a.split_view, a.right_active, a.pane_ratio),
                };
                crate::settings::SavedTab {
                    folder: view.browser.requested_location().map(|l| l.dir).unwrap_or_default(),
                    other: other.and_then(|v| v.browser.requested_location().map(|l| l.dir)),
                    split,
                    right_active,
                    ratio,
                }
            })
            .collect();
        (tabs, a.tabs.active())
    })
    .unwrap_or_default()
}
pub(super) fn restore(settings: &Settings) {
    for (i, tab) in settings.tabs.iter().enumerate() {
        if i > 0 {
            new_tab(Some(tab.folder.clone()));
        } else {
            super::go(tab.folder.clone());
        }
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
    switch_to(settings.active_tab.min(settings.tabs.len().saturating_sub(1)));
    if let Some(hwnd) = with_app(|a| a.hwnd) {
        super::layout(hwnd);
    }
}
