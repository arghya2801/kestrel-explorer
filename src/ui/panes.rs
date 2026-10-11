//! Each folder view owns its control, navigation, workers, and drag target.
use super::{Event, child, drag, with_app};
use crate::{
    browser::Browser,
    events::Events,
    icons::{self, Icons},
    settings::Settings,
    sort::SortKey,
    watch::Watcher,
};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::thread::JoinHandle;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::HFONT;
use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PWSTR, Result, w};

pub(super) struct FolderView {
    pub id: u64,
    pub list: HWND,
    pub label: HWND,
    pub browser: Browser,
    pub lister: Option<JoinHandle<()>>,
    pub lister_cancel: Arc<std::sync::atomic::AtomicBool>,
    pub icons: Icons,
    pub watcher: Option<(u64, Watcher)>,
    pub device_note: Option<(isize, isize)>,
    pub edit_pending: Option<String>,
    pub renaming: Option<PathBuf>,
    pub resync_after_rename: bool,
    pub drop_row: Option<usize>,
    pub drop_target: Option<IDropTarget>,
}

impl Drop for FolderView {
    fn drop(&mut self) {
        self.lister_cancel.store(true, Ordering::SeqCst);
        unsafe {
            let _ = RevokeDragDrop(self.list);
            if let Some((note, _)) = self.device_note.take() {
                let _ = UnregisterDeviceNotification(HDEVNOTIFY(note as _));
            }
            let _ = DestroyWindow(self.list);
            let _ = DestroyWindow(self.label);
        }
    }
}

pub(super) struct Workspace {
    pub view: FolderView,
    pub other: Option<FolderView>,
    pub split: bool,
    pub right_active: bool,
    pub ratio: i32,
}

pub(super) unsafe fn create(
    parent: HWND,
    settings: &Settings,
    id: u64,
    events: Arc<Events<Event>>,
    font: HFONT,
) -> Result<FolderView> {
    unsafe {
        let list = child(
            parent,
            WC_LISTVIEWW,
            w!(""),
            WS_TABSTOP
                | WINDOW_STYLE(
                    LVS_REPORT | LVS_OWNERDATA | LVS_SHOWSELALWAYS | LVS_SHAREIMAGELISTS | LVS_EDITLABELS,
                ),
            WINDOW_EX_STYLE(0),
            0,
        )?;
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        SendMessageW(
            list,
            LVM_SETEXTENDEDLISTVIEWSTYLE,
            Some(WPARAM(ex as usize)),
            Some(LPARAM(ex as isize)),
        );
        for (i, (title, width, key)) in super::COLUMNS.iter().enumerate() {
            let col = LVCOLUMNW {
                mask: LVCF_TEXT | LVCF_WIDTH | LVCF_FMT,
                fmt: if *key == SortKey::Size { LVCFMT_RIGHT } else { LVCFMT_LEFT },
                cx: settings.columns.get(i).copied().unwrap_or_else(|| super::scale(parent, *width)),
                pszText: PWSTR(title.0 as *mut _),
                ..Default::default()
            };
            SendMessageW(list, LVM_INSERTCOLUMNW, Some(WPARAM(i)), Some(LPARAM(&col as *const _ as isize)));
        }
        let label = match child(parent, w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), 0) {
            Ok(label) => label,
            Err(e) => {
                let _ = DestroyWindow(list);
                return Err(e);
            }
        };
        super::set_font(&[list, label], font);
        let (tx, rx) = mpsc::channel();
        let mut icons = Icons::new(tx, 0);
        if let Err(e) = icons::spawn_worker(rx, icons.latest.clone(), move |done| {
            events.send(Event::Icon { view: id, done })
        }) {
            crate::crash::log(&format!("icons disabled: {e}"));
        }
        if with_app(|a| !a.nodes.is_empty()) == Some(true) {
            let (image, folder) = icons::system_image_list().unwrap_or_default();
            icons.folder = folder;
            SendMessageW(list, LVM_SETIMAGELIST, Some(WPARAM(LVSIL_SMALL as usize)), Some(LPARAM(image.0)));
        }
        let target = drag::DropTarget::create(list);
        if let Err(e) = RegisterDragDrop(list, &target) {
            crate::crash::log(&format!("drag and drop disabled: {e}"));
        }
        SendMessageW(list, LVM_SETCALLBACKMASK, Some(WPARAM(LVIS_DROPHILITED.0 as usize)), None);
        let mut browser = Browser::with_generation_base(crate::tabs::generation_base(id));
        (browser.sort, browser.show_hidden) = (settings.sort, settings.show_hidden);
        Ok(FolderView {
            id,
            list,
            label,
            browser,
            lister: None,
            lister_cancel: Arc::default(),
            icons,
            watcher: None,
            device_note: None,
            edit_pending: None,
            renaming: None,
            resync_after_rename: false,
            drop_row: None,
            drop_target: Some(target),
        })
    }
}

pub(super) fn toggle() {
    let Some((hwnd, exists, settings, id, events, font, dir)) = with_app(|a| {
        let id = a.next_tab_id;
        if a.other.is_none() {
            a.next_tab_id += 1;
        }
        (
            a.hwnd,
            a.other.is_some(),
            Settings {
                sort: a.view.browser.sort,
                show_hidden: a.view.browser.show_hidden,
                ..Default::default()
            },
            id,
            a.events.clone(),
            a.font,
            a.view.browser.requested_location().map(|l| l.dir),
        )
    }) else {
        return;
    };
    let mut job = None;
    if !exists {
        let mut other = match unsafe { create(hwnd, &settings, id, events, font) } {
            Ok(other) => other,
            Err(e) => return super::set_status(&format!("Couldn't split the folder view: {e}")),
        };
        if let Some(dir) = dir {
            job = Some(other.browser.go(dir));
        }
        with_app(|a| a.other = Some(other));
    }
    with_app(|a| a.split_view = !a.split_view);
    let (palette, hidden) =
        with_app(|a| (a.palette, (!a.split_view).then(|| a.other.as_ref().map(|v| v.id)).flatten())).unwrap();
    if let Some(id) = hidden {
        super::stop_watching_view(id);
    }
    super::dark::apply(hwnd, palette);
    super::layout(hwnd);
    if let Some(job) = job {
        super::spawn_list(job);
    }
    if let Some(id) = with_app(|a| a.split_view.then(|| a.other.as_ref().map(|v| v.id)).flatten()).flatten() {
        super::watch_view(id);
    }
    super::update_chrome();
}

pub(super) fn activate(list: HWND) {
    let Some((hwnd, changed)) = with_app(|a| {
        if !a.split_view || a.other.as_ref().is_none_or(|v| v.list != list) {
            return (a.hwnd, false);
        }
        std::mem::swap(&mut a.view, a.other.as_mut().unwrap());
        a.right_active = !a.right_active;
        (a.hwnd, true)
    }) else {
        return;
    };
    if changed {
        super::preview::invalidate();
        super::layout(hwnd);
        super::update_chrome();
        super::update_sort_arrows();
    }
}

pub(super) fn focus_other() {
    if let Some(list) =
        with_app(|a| a.split_view.then(|| a.other.as_ref().map(|v| v.list)).flatten()).flatten()
    {
        activate(list);
        unsafe {
            let _ = SetFocus(Some(list));
        }
    }
}

pub(super) fn update_labels() {
    let Some(labels) = with_app(|a| {
        std::iter::once(&a.view)
            .chain(a.other.iter())
            .map(|v| {
                let path =
                    v.browser.requested_location().map(|l| l.dir.display().to_string()).unwrap_or_default();
                (v.label, format!("{}{}", if v.id == a.view.id { "Active: " } else { "" }, path))
            })
            .collect::<Vec<_>>()
    }) else {
        return;
    };
    for (label, text) in labels {
        unsafe {
            let _ = SetWindowTextW(label, &windows::core::HSTRING::from(text));
        }
    }
}
pub(super) fn layout_labels(hwnd: HWND) {
    let Some((split, views)) = with_app(|a| {
        (
            a.split_view,
            std::iter::once(&a.view).chain(a.other.iter()).map(|v| (v.list, v.label)).collect::<Vec<_>>(),
        )
    }) else {
        return;
    };
    for (list, label) in views {
        unsafe {
            let _ = ShowWindow(label, if split { SW_SHOWNA } else { SW_HIDE });
            if split {
                let mut rect = windows::Win32::Foundation::RECT::default();
                let _ = GetWindowRect(list, &mut rect);
                let mut point = windows::Win32::Foundation::POINT { x: rect.left, y: rect.top };
                let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
                let height = super::scale(hwnd, 24);
                let _ = MoveWindow(label, point.x, point.y, rect.right - rect.left, height, true);
                let _ = MoveWindow(
                    list,
                    point.x,
                    point.y + height,
                    rect.right - rect.left,
                    (rect.bottom - rect.top - height).max(0),
                    true,
                );
            }
        }
    }
    update_labels();
}
