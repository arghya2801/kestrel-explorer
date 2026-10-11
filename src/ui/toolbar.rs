//! File commands below the address bar, with overflow on narrow windows.
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result, w};

use super::{child, commands, dialogs, preview, run, scale, spawn_sort, update_sort_arrows, with_app};
use crate::browser::Browser;
use crate::sort::SortKey;

pub const REFRESH: usize = 200;
pub const HIDDEN: usize = 201;
pub const PREVIEW: usize = 202;
pub const SORT_NAME: usize = 203;
pub const SORT_DATE: usize = 204;
pub const SORT_TYPE: usize = 205;
pub const SORT_SIZE: usize = 206;
pub const DESCENDING: usize = 207;
pub const VIEW: usize = 208;
pub const MORE: usize = 209;
pub const HELP: usize = 210;
pub const SHORTCUTS: usize = 211;
pub const SETTINGS: usize = 212;
pub const SPLIT: usize = 213;
pub const DETAILS: usize = 214;
pub const GRID: usize = 215;

const BUTTONS: &[(usize, &str, i32)] = &[
    (commands::CMD_NEW_FOLDER, "New folder", 94),
    (commands::CMD_CUT, "Cut", 44),
    (commands::CMD_COPY, "Copy", 52),
    (commands::CMD_PASTE, "Paste", 54),
    (commands::CMD_RENAME, "Rename", 70),
    (commands::CMD_DELETE, "Delete", 62),
    (REFRESH, "Refresh", 70),
    (SPLIT, "Split", 52),
    (VIEW, "View", 55),
    (MORE, "More", 58),
    (SETTINGS, "Settings", 72),
    (HELP, "Help", 50),
];

pub(super) unsafe fn create(parent: HWND) -> Result<Vec<HWND>> {
    BUTTONS
        .iter()
        .map(|&(id, text, _)| unsafe {
            child(parent, w!("BUTTON"), &HSTRING::from(text), WS_TABSTOP, WINDOW_EX_STYLE(0), id)
        })
        .collect()
}

pub(super) fn owns(hwnd: HWND) -> bool {
    with_app(|a| a.toolbar.contains(&hwnd)) == Some(true)
}

pub(super) fn layout(parent: HWND, width: i32) {
    let Some(buttons) = with_app(|a| a.toolbar.clone()) else { return };
    let pad = scale(parent, 4);
    let reserve: i32 = BUTTONS.iter().rev().take(3).map(|b| scale(parent, b.2) + pad).sum();
    let mut x = pad;
    for (i, &(id, _, logical_width)) in BUTTONS.iter().enumerate() {
        let w = scale(parent, logical_width);
        let pinned = matches!(id, MORE | SETTINGS | HELP);
        let visible = pinned || x + w + reserve <= width;
        unsafe {
            let _ = ShowWindow(buttons[i], if visible { SW_SHOWNA } else { SW_HIDE });
            if visible {
                let _ = MoveWindow(buttons[i], x, scale(parent, 36), w, scale(parent, 28), true);
            }
        }
        if visible {
            x += w + pad;
        }
    }
}

pub(super) fn enabled(id: usize) -> bool {
    use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
    use windows::Win32::System::Ole::CF_HDROP;
    let Some((list, has_folder)) = with_app(|a| (a.view.list, a.view.browser.dir().is_some())) else {
        return false;
    };
    match id {
        commands::CMD_CUT
        | commands::CMD_COPY
        | commands::CMD_RENAME
        | commands::CMD_DELETE
        | commands::CMD_COPY_PATH
        | commands::CMD_OPEN => unsafe { SendMessageW(list, LVM_GETSELECTEDCOUNT, None, None).0 > 0 },
        commands::CMD_PASTE => has_folder && unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok(),
        commands::CMD_NEW_FOLDER | REFRESH => has_folder,
        _ => true,
    }
}

pub(super) fn update() {
    let Some(buttons) = with_app(|a| a.toolbar.clone()) else { return };
    for (button, &(id, _, _)) in buttons.into_iter().zip(BUTTONS) {
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow(button, enabled(id));
        }
    }
}

pub(super) fn execute(id: usize) -> bool {
    let Some(hwnd) = with_app(|a| a.hwnd) else { return false };
    match id {
        DETAILS | GRID => super::grid::set(id == GRID),
        SPLIT => super::panes::toggle(),
        REFRESH => run(Browser::refresh),
        HIDDEN => {
            if let Some(job) = with_app(|a| a.view.browser.toggle_hidden()).flatten() {
                spawn_sort(job);
            }
        }
        PREVIEW => preview::toggle(hwnd),
        SORT_NAME..=SORT_SIZE | DESCENDING => {
            let job = with_app(|a| {
                let key = match id {
                    SORT_NAME => SortKey::Name,
                    SORT_DATE => SortKey::Modified,
                    SORT_TYPE => SortKey::Type,
                    SORT_SIZE => SortKey::Size,
                    _ => a.view.browser.sort.key,
                };
                let descending = if id == DESCENDING {
                    !a.view.browser.sort.descending
                } else {
                    a.view.browser.sort.descending
                };
                a.view.browser.set_sort(crate::sort::Sort { key, descending })
            })
            .flatten();
            if let Some(job) = job {
                spawn_sort(job);
            }
            update_sort_arrows();
        }
        SETTINGS => dialogs::settings(hwnd),
        SHORTCUTS => dialogs::shortcuts(hwnd),
        VIEW | MORE | HELP => popup(hwnd, id),
        _ => return false,
    }
    true
}

fn popup(hwnd: HWND, kind: usize) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: usize, text: &str, checked: bool| {
            let flags = MF_STRING
                | if enabled(id) { MF_ENABLED } else { MF_GRAYED }
                | if checked { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, flags, id, &HSTRING::from(text));
        };
        match kind {
            VIEW => {
                let grid = with_app(|a| a.view.grid.enabled).unwrap_or(false);
                add(DETAILS, "Details", !grid);
                add(GRID, "Thumbnail grid", grid);
                let Some((sort, hidden, preview)) = with_app(|a| {
                    (a.view.browser.sort, a.view.browser.show_hidden, preview::is_visible_in(a))
                }) else {
                    let _ = DestroyMenu(menu);
                    return;
                };
                for (id, label, key) in [
                    (SORT_NAME, "Sort by name", SortKey::Name),
                    (SORT_DATE, "Sort by date", SortKey::Modified),
                    (SORT_TYPE, "Sort by type", SortKey::Type),
                    (SORT_SIZE, "Sort by size", SortKey::Size),
                ] {
                    add(id, label, sort.key == key);
                }
                add(DESCENDING, "Descending", sort.descending);
                add(HIDDEN, "Hidden items\tCtrl+H", hidden);
                add(PREVIEW, "Preview pane\tAlt+P", preview);
            }
            HELP => add(SHORTCUTS, "Keyboard shortcuts\tF1", false),
            _ => {
                for &(id, label, _) in BUTTONS.iter().filter(|b| !matches!(b.0, MORE | SETTINGS | HELP)) {
                    add(id, label, false);
                }
                add(commands::CMD_COPY_PATH, "Copy as path\tCtrl+Shift+C", false);
                add(commands::CMD_SELECT_ALL, "Select all\tCtrl+A", false);
                add(commands::CMD_SHOW_IN_EXPLORER, "Show in Explorer", false);
                add(commands::CMD_MORE_OPTIONS, "Windows context menu\tShift+right-click", false);
            }
        }
        let button = GetDlgItem(Some(hwnd), kind as i32).unwrap_or(hwnd);
        let mut r = RECT::default();
        let _ = GetWindowRect(button, &mut r);
        let id = TrackPopupMenu(menu, TPM_RETURNCMD, r.left, r.bottom, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        if id != 0 {
            let _ = PostMessageW(Some(hwnd), WM_COMMAND, WPARAM(id), LPARAM(0));
        }
    }
}
