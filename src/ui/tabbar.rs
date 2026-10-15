//! A tab owns its folder views; switching tabs preserves both panes.
use super::{
    child,
    panes::{self, Workspace},
    with_app,
};
use crate::{browser::Browser, settings::Settings};
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PWSTR, Result, w};

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
        child(parent, WC_TABCONTROLW, w!(""), WINDOW_STYLE(TCS_FOCUSNEVER), WINDOW_EX_STYLE(0), 0)?
    };
    // Roomier tabs than the control's default.
    let (x, y) = (super::scale(parent, 16), super::scale(parent, 4));
    unsafe {
        SendMessageW(strip, TCM_SETPADDING, None, Some(LPARAM(((y << 16) | x) as isize)));
        SendMessageW(strip, TCM_SETMINTABWIDTH, None, Some(LPARAM(super::scale(parent, 96) as isize)));
        let _ = windows::Win32::UI::Shell::SetWindowSubclass(strip, Some(middle_click), 3, 0);
    }
    insert_label(strip, 0, "");
    Ok(strip)
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
fn label_for(b: &Browser) -> String {
    if let Some(q) = b.query() {
        return format!("Search: {q}");
    }
    b.dir()
        .map(|p| p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "Loading…".into())
}
pub(super) fn update_label() {
    if let Some((strip, index, text)) =
        with_app(|a| (a.tab_strip, a.tabs.active(), label_for(&a.view.browser)))
    {
        set_label(strip, index, &text);
    }
}
pub(super) fn update_label_for(id: u64) {
    if let Some((strip, index, text)) = with_app(|a| {
        if a.view.id == id {
            return Some((a.tab_strip, a.tabs.active(), label_for(&a.view.browser)));
        }
        a.tabs.items().iter().enumerate().find_map(|(i, t)| {
            t.workspace
                .as_ref()
                .filter(|w| w.view.id == id)
                .map(|w| (a.tab_strip, i, label_for(&w.view.browser)))
        })
    })
    .flatten()
    {
        set_label(strip, index, &text);
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
    if let Some(dir) = dir {
        super::go(dir);
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

/// Middle-clicking a tab closes it, as in browsers.
unsafe extern "system" fn middle_click(
    strip: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_MBUTTONUP {
        let pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
        let mut hit = TCHITTESTINFO { pt, ..Default::default() };
        let tab =
            unsafe { SendMessageW(strip, TCM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))) }.0;
        if let Ok(tab) = usize::try_from(tab) {
            crate::crash::guard(|| close_tab(tab));
            return LRESULT(0);
        }
    }
    unsafe { windows::Win32::UI::Shell::DefSubclassProc(strip, msg, wp, lp) }
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
                let Ok(menu) = CreatePopupMenu() else { return Some(LRESULT(1)) };
                let _ = AppendMenuW(menu, MF_STRING, 1, w!("&New tab\tCtrl+T"));
                let flags = if tab >= 0 && with_app(|a| a.tabs.len()).unwrap_or(1) > 1 {
                    MF_ENABLED
                } else {
                    MF_GRAYED
                };
                let _ = AppendMenuW(menu, MF_STRING | flags, 2, w!("&Close tab\tCtrl+W"));
                let owner = GetParent(strip).unwrap_or_default();
                let cmd = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    screen.x,
                    screen.y,
                    None,
                    owner,
                    None,
                )
                .0;
                let _ = DestroyMenu(menu);
                match cmd {
                    1 => new_tab(None),
                    2 => close_tab(tab as usize),
                    _ => {}
                }
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
