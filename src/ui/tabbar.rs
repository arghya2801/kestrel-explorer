//! Tabs. The active tab's browser lives in `App::browser` like before; the
//! others wait in their `TabSlot`. Switching swaps them, saving the outgoing
//! tab's selection and scroll position.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PWSTR, Result, w};

use super::{
    child, go, refresh, selected_rows, spawn_list, stop_watching, update_chrome, update_sort_arrows,
    watch_current, with_app,
};
use crate::browser::{Applied, Browser};
use crate::listing::Snapshot;
use crate::tabs::{generation_base, tab_of_generation};

pub(super) struct TabSlot {
    pub(super) id: u64,
    /// `None` for the active tab, whose browser is `App::browser`.
    browser: Option<Browser>,
    lister: Option<JoinHandle<()>>,
    lister_cancel: Arc<AtomicBool>,
    selected: Vec<usize>,
    top: usize,
}

impl TabSlot {
    pub(super) fn active(id: u64) -> TabSlot {
        TabSlot {
            id,
            browser: None,
            lister: None,
            lister_cancel: Arc::default(),
            selected: Vec::new(),
            top: 0,
        }
    }
}

pub(super) unsafe fn create(parent: HWND) -> Result<HWND> {
    let strip = unsafe {
        child(parent, WC_TABCONTROLW, w!(""), WINDOW_STYLE(TCS_FOCUSNEVER), WINDOW_EX_STYLE(0), 0)?
    };
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

/// Short name for a tab: the folder's name, or the search.
fn label_for(b: &Browser) -> String {
    if let Some(q) = b.query() {
        return format!("Search: {q}");
    }
    match b.dir() {
        Some(d) => {
            d.file_name().map_or_else(|| d.display().to_string(), |n| n.to_string_lossy().into_owned())
        }
        None => "Loading…".into(),
    }
}

/// Refreshes the active tab's label.
pub(super) fn update_label() {
    if let Some((strip, index, text)) =
        with_app(|a| (a.tab_strip, a.tabs.active(), label_for(&a.view.browser)))
    {
        set_label(strip, index, &text);
    }
}

/// Opens a tab right of the active one showing `dir` (default: the current folder).
pub(super) fn new_tab(dir: Option<PathBuf>) {
    let Some((index, strip, dir)) = with_app(|a| {
        let id = a.next_tab_id;
        a.next_tab_id += 1;
        let mut b = Browser::with_generation_base(generation_base(id));
        (b.sort, b.show_hidden) = (a.view.browser.sort, a.view.browser.show_hidden);
        let dir = dir.or_else(|| a.view.browser.dir().map(Path::to_path_buf));
        let from = a.tabs.active();
        let slot = TabSlot { browser: Some(b), ..TabSlot::active(id) };
        let index = a.tabs.open(slot);
        a.tabs.select(from); // `switch_to` does the activating
        (index, a.tab_strip, dir)
    }) else {
        return;
    };
    insert_label(strip, index, "Loading…");
    switch_to(index);
    if let Some(dir) = dir {
        go(dir);
    }
}

/// Closes tab `index`; the last tab stays.
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
    if let Some(slot) = with_app(|a| a.tabs.close(index)).flatten() {
        slot.lister_cancel.store(true, Ordering::SeqCst);
    }
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

/// Moves to the next (or previous) tab, wrapping around.
pub(super) fn cycle(back: bool) {
    let Some(to) = with_app(|a| if back { a.tabs.prev_index() } else { a.tabs.next_index() }) else { return };
    switch_to(to);
}

pub(super) fn switch_to(to: usize) {
    let Some((list, strip, from, len)) =
        with_app(|a| (a.view.list, a.tab_strip, a.tabs.active(), a.tabs.len()))
    else {
        return;
    };
    if from == to || to >= len {
        return;
    }
    let selected = selected_rows();
    let top = unsafe { SendMessageW(list, LVM_GETTOPINDEX, None, None) }.0.max(0) as usize;
    stop_watching();
    let restored = with_app(|a| {
        let incoming = &mut a.tabs.items_mut()[to];
        let browser = incoming.browser.take()?;
        let lister = incoming.lister.take();
        let cancel = incoming.lister_cancel.clone();
        let (in_sel, in_top) = (std::mem::take(&mut incoming.selected), incoming.top);
        let out_browser = std::mem::replace(&mut a.view.browser, browser);
        let out_lister = std::mem::replace(&mut a.view.lister, lister);
        let out_cancel = std::mem::replace(&mut a.view.lister_cancel, cancel);
        let outgoing = &mut a.tabs.items_mut()[from];
        (outgoing.browser, outgoing.lister, outgoing.lister_cancel) =
            (Some(out_browser), out_lister, out_cancel);
        (outgoing.selected, outgoing.top) = (selected, top);
        a.tabs.select(to);
        Some((in_sel, in_top))
    })
    .flatten();
    let Some((sel, top)) = restored else { return };
    unsafe { SendMessageW(strip, TCM_SETCURSEL, Some(WPARAM(to)), None) };
    refresh(&sel, true);
    unsafe {
        // Put the tab's old first visible row back at the top.
        let per_page = SendMessageW(list, LVM_GETCOUNTPERPAGE, None, None).0.max(1) as usize;
        let count = SendMessageW(list, LVM_GETITEMCOUNT, None, None).0.max(0) as usize;
        if count > 0 {
            let last = (top + per_page - 1).min(count - 1);
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(last)), Some(LPARAM(0)));
            SendMessageW(list, LVM_ENSUREVISIBLE, Some(WPARAM(top.min(count - 1))), Some(LPARAM(0)));
        }
    }
    if with_app(|a| a.view.browser.dir().is_some()) == Some(true) {
        watch_current();
    }
    update_sort_arrows();
    update_chrome();
    // Catch up on changes made while the tab was in the background.
    if let Some(job) = with_app(|a| a.view.browser.on_dir_changed()).flatten() {
        spawn_list(job);
    }
}

/// True if `generation` belongs to a tab other than the active one.
pub(super) fn is_background(generation: u64) -> bool {
    with_app(|a| a.tabs.items()[a.tabs.active()].id != tab_of_generation(generation)) == Some(true)
}

/// A listing finished for a background tab: store it there; the tab shows it when selected.
pub(super) fn route_listed(generation: u64, result: std::io::Result<(Snapshot, Vec<u32>)>) {
    let Some((strip, labelled)) = with_app(|a| {
        let id = tab_of_generation(generation);
        let index = a.tabs.items().iter().position(|t| t.id == id)?;
        let slot = &mut a.tabs.items_mut()[index];
        let browser = slot.browser.as_mut()?;
        // A vanished folder is handled when the tab is next shown.
        if browser.on_listed(generation, result, &[]) != Applied::Stale {
            slot.lister = None;
        }
        Some((a.tab_strip, (index, label_for(slot.browser.as_ref()?))))
    })
    .flatten() else {
        return;
    };
    set_label(strip, labelled.0, &labelled.1);
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
