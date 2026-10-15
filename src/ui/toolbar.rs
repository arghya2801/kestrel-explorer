//! File commands below the address bar, with overflow on narrow windows.
use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Result, w};

use super::{child, commands, dialogs, preview, run, scale, spawn_sort, update_sort_arrows, with_app};
use crate::browser::Browser;
use crate::settings::THUMB_SIZES;
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
pub const PIN: usize = 216;
pub const RECENT: usize = 217;
pub const PARENTS: usize = 218;
pub const TERMINAL: usize = 219;
pub const OPERATIONS: usize = 220;
/// First of the thumbnail size commands, one per `THUMB_SIZES` entry.
pub const THUMBS: usize = 221;
// 221..=224 are the thumbnail sizes.
pub const UPDATES: usize = 230;
pub const FOLDER_SIZES: usize = 231;
pub const SYNC_PANES: usize = 232;
pub const COMPARE: usize = 233;
const RELEASES: &str = "https://github.com/arghya2801/kestrel-explorer/releases/latest";

/// Logical y where the toolbar row ends and the tab strip starts.
pub(super) const BOTTOM: i32 = 74;
const TOP: i32 = 40;
const HEIGHT: i32 = 32;

/// Command, label, and group: a separator is drawn between groups.
const BUTTONS: &[(usize, &str, u8)] = &[
    (commands::CMD_NEW_FOLDER, "New folder", 0),
    (commands::CMD_CUT, "Cut", 1),
    (commands::CMD_COPY, "Copy", 1),
    (commands::CMD_PASTE, "Paste", 1),
    (commands::CMD_RENAME, "Rename", 1),
    (commands::CMD_DELETE, "Delete", 1),
    (REFRESH, "Refresh", 2),
    (SPLIT, "Split", 2),
    (VIEW, "View", 2),
    (MORE, "More", 3),
    (SETTINGS, "Settings", 3),
    (HELP, "Help", 3),
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
    let (gap, group_gap) = (scale(parent, 2), scale(parent, 13));
    let widths: Vec<i32> = buttons
        .iter()
        .zip(BUTTONS)
        .map(|(&b, &(id, _, _))| super::chrome::button_width(parent, b, id))
        .collect();
    // The pinned last group, plus the separator before it.
    let reserve: i32 = widths.iter().rev().take(3).map(|w| w + gap).sum::<i32>() + group_gap;
    let mut x = scale(parent, 6);
    let mut group = BUTTONS[0].2;
    for (i, &(id, _, g)) in BUTTONS.iter().enumerate() {
        let pinned = matches!(id, MORE | SETTINGS | HELP);
        if g != group {
            x += group_gap - gap;
        }
        let visible = pinned || x + widths[i] + reserve <= width;
        unsafe {
            let _ = ShowWindow(buttons[i], if visible { SW_SHOWNA } else { SW_HIDE });
            if visible {
                let _ = MoveWindow(buttons[i], x, scale(parent, TOP), widths[i], scale(parent, HEIGHT), true);
            }
        }
        if visible {
            x += widths[i] + gap;
            group = g;
        } else if g != group {
            x -= group_gap - gap;
        }
    }
}

/// Hairlines between visible groups, in the parent's client coordinates.
pub(super) fn separators(parent: HWND) -> Vec<RECT> {
    let Some(buttons) = with_app(|a| a.toolbar.clone()) else { return Vec::new() };
    let visible: Vec<(RECT, u8)> = buttons
        .iter()
        .zip(BUTTONS)
        .filter(|(b, _)| unsafe { IsWindowVisible(**b) }.as_bool())
        .map(|(&b, &(_, _, g))| (super::client_rect_of(parent, b), g))
        .collect();
    let inset = scale(parent, 8);
    visible
        .windows(2)
        .filter(|w| w[0].1 != w[1].1)
        .map(|w| {
            let x = (w[0].0.right + w[1].0.left) / 2;
            RECT { left: x, top: w[0].0.top + inset, right: x + 1, bottom: w[0].0.bottom - inset }
        })
        .collect()
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
        SYNC_PANES | COMPARE => with_app(|a| a.split_view) == Some(true),
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
        PIN => {
            with_app(|a| {
                if let Some(dir) = a
                    .view
                    .browser
                    .dir()
                    .map(std::path::PathBuf::from)
                    .filter(|p| crate::settings::rememberable(p))
                {
                    if a.pins.contains(&dir) {
                        a.pins.retain(|p| p != &dir);
                    } else {
                        a.pins.push(dir);
                    }
                }
            });
            super::tree::reload();
            if let Some(s) = super::current_settings(hwnd) {
                super::save_preferences(s);
            }
        }
        RECENT | PARENTS => locations(hwnd, id == RECENT),
        // The releases page shows what's newer; nothing is sent until the user asks.
        UPDATES => {
            let events = with_app(|a| a.events.clone());
            super::spawn("updates", move || {
                if let Err(e) = crate::shell::open(std::path::Path::new(RELEASES), HWND::default())
                    && let Some(events) = events
                {
                    events.send(super::Event::Message(e));
                }
            });
        }
        TERMINAL => {
            if let Some((dir, events)) =
                with_app(|a| a.view.browser.dir().map(|p| (p.to_path_buf(), a.events.clone()))).flatten()
            {
                super::spawn("terminal", move || {
                    if let Err(e) = crate::shell::terminal(&dir) {
                        events.send(super::Event::Message(format!("Couldn't open terminal: {e}")));
                    }
                });
            }
        }
        OPERATIONS => dialogs::operations(hwnd),
        DETAILS | GRID => super::grid::set(id == GRID),
        id if (THUMBS..THUMBS + THUMB_SIZES.len()).contains(&id) => {
            super::grid::set_size(THUMB_SIZES[id - THUMBS].0)
        }
        SPLIT => super::panes::toggle(),
        REFRESH => run(Browser::refresh),
        HIDDEN => {
            if let Some(job) = with_app(|a| a.view.browser.toggle_hidden()).flatten() {
                spawn_sort(job);
            }
        }
        PREVIEW => preview::toggle(hwnd),
        FOLDER_SIZES => super::calculate_folder_sizes(),
        SYNC_PANES => _ = with_app(|a| a.sync_panes = !a.sync_panes),
        COMPARE => super::panes::compare(),
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
                let (grid, size) =
                    with_app(|a| (a.view.grid.enabled, a.view.grid.size)).unwrap_or((false, 0));
                add(DETAILS, "Details", !grid);
                for (i, &(px, label)) in THUMB_SIZES.iter().enumerate() {
                    add(THUMBS + i, label, grid && size == px);
                }
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
                add(FOLDER_SIZES, "Calculate folder sizes", false);
                add(SYNC_PANES, "Sync pane navigation", with_app(|a| a.sync_panes).unwrap_or(false));
                add(COMPARE, "Compare panes", false);
            }
            HELP => {
                add(SHORTCUTS, "Keyboard shortcuts\tF1", false);
                add(UPDATES, &format!("Check for updates (this is {})", env!("CARGO_PKG_VERSION")), false);
            }
            _ => {
                for &(id, label, _) in BUTTONS.iter().filter(|b| !matches!(b.0, MORE | SETTINGS | HELP)) {
                    add(id, label, false);
                }
                let pinned =
                    with_app(|a| a.view.browser.dir().is_some_and(|p| a.pins.iter().any(|pin| pin == p)))
                        .unwrap_or(false);
                add(PIN, if pinned { "Unpin current folder" } else { "Pin current folder" }, pinned);
                add(RECENT, "Recent folders", false);
                add(PARENTS, "Parent folders", false);
                add(TERMINAL, "Open terminal here", false);
                add(OPERATIONS, "File operations", false);
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

fn locations(hwnd: HWND, recent: bool) {
    let paths = with_app(|a| {
        if recent {
            a.recent.clone()
        } else {
            a.view
                .browser
                .dir()
                .map(|p| p.ancestors().map(std::path::PathBuf::from).collect())
                .unwrap_or_default()
        }
    })
    .unwrap_or_default();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (i, path) in paths.iter().enumerate() {
            let _ = AppendMenuW(
                menu,
                MF_STRING,
                i + 1,
                &HSTRING::from(path.display().to_string().replace('&', "&&")),
            );
        }
        if paths.is_empty() {
            let _ = AppendMenuW(menu, MF_GRAYED, 0, w!("No recent folders"));
        }
        let mut point = windows::Win32::Foundation::POINT::default();
        let _ = GetCursorPos(&mut point);
        let selected = TrackPopupMenu(menu, TPM_RETURNCMD, point.x, point.y, None, hwnd, None).0 as usize;
        let _ = DestroyMenu(menu);
        if let Some(path) = selected.checked_sub(1).and_then(|i| paths.get(i)) {
            super::go(path.clone());
        }
    }
}
