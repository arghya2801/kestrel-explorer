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
pub const REVEAL: usize = 234;
pub const GROUP: usize = 235;
pub const SWAP_PANES: usize = 236;
pub const MIRROR_PANE: usize = 237;
pub const VERTICAL_TABS: usize = 238;
// 240 is the "+" (new tab) button.
/// The tab strip's list of every tab, by the "+" button.
pub const TAB_LIST: usize = 241;
/// Tab commands; each acts on the active tab.
pub const REOPEN_TAB: usize = 242;
pub const CLOSE_OTHER_TABS: usize = 243;
pub const CLOSE_TABS_AFTER: usize = 244;
pub const MOVE_TAB_BACK: usize = 245;
pub const MOVE_TAB_FORWARD: usize = 246;
pub const CLOSE_TAB: usize = 247;
pub const DUPLICATE_TAB: usize = 248;
pub const NEXT_TAB: usize = 249;
pub const PREV_TAB: usize = 250;
/// Window and focus commands, so the palette and keys.txt can run them.
pub const NEW_WINDOW: usize = 251;
pub const FOCUS_FILTER: usize = 252;
pub const FOCUS_OTHER_PANE: usize = 253;
pub const PALETTE: usize = 254;
/// The Sort button, and sort and group choices that set rather than toggle.
pub const SORT_MENU: usize = 255;
pub const SORT_CREATED: usize = 256;
pub const SORT_UP: usize = 257;
pub const SORT_DOWN: usize = 258;
pub const GROUP_NONE: usize = 259;
pub const GROUP_DATE: usize = 260;
/// Narrows the vertical tab column to badges, or widens it again.
pub const TABS_COLLAPSE: usize = 261;
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
    (SORT_MENU, "Sort", 2),
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
    // The last group always stays; Sort and View too while there's room, else they move into
    // More with the other buttons that don't fit. `reserve` is the room the pinned ones need.
    let room = |ids: &[usize]| {
        BUTTONS.iter().zip(&widths).filter(|(b, _)| ids.contains(&b.0)).map(|(_, w)| w + gap).sum::<i32>()
    };
    let last = [MORE, SETTINGS, HELP];
    let sort_view = room(&[SORT_MENU, VIEW]) + room(&last) + 2 * group_gap + scale(parent, 6) <= width;
    let pinned = |id: usize| last.contains(&id) || (sort_view && matches!(id, SORT_MENU | VIEW));
    let reserve = room(&last) + group_gap + if sort_view { room(&[SORT_MENU, VIEW]) + group_gap } else { 0 };
    let mut x = scale(parent, 6);
    let mut group = BUTTONS[0].2;
    for (i, &(id, _, g)) in BUTTONS.iter().enumerate() {
        let pinned = pinned(id);
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
    let Some((list, dir)) = with_app(|a| (a.view.list, a.view.browser.dir().map(std::path::PathBuf::from)))
    else {
        return false;
    };
    // This PC lists drives: nothing there to cut, paste into, or measure.
    let has_folder = dir.as_deref().is_some_and(|d| !crate::places::is_this_pc(d));
    let selected = unsafe { SendMessageW(list, LVM_GETSELECTEDCOUNT, None, None).0 > 0 };
    match id {
        commands::CMD_CUT | commands::CMD_COPY | commands::CMD_RENAME | commands::CMD_DELETE => {
            has_folder && selected
        }
        commands::CMD_COPY_PATH | commands::CMD_OPEN => selected,
        commands::CMD_PASTE => has_folder && unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok(),
        commands::CMD_NEW_FOLDER | FOLDER_SIZES | TERMINAL => has_folder,
        REFRESH => dir.is_some(),
        SYNC_PANES | COMPARE | SWAP_PANES | MIRROR_PANE | FOCUS_OTHER_PANE => {
            with_app(|a| a.split_view) == Some(true)
        }
        REOPEN_TAB => with_app(|a| !a.closed_tabs.is_empty()) == Some(true),
        CLOSE_OTHER_TABS => with_app(|a| a.tabs.len() > 1) == Some(true),
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
            if let Some(dir) = with_app(|a| a.view.browser.dir().map(std::path::PathBuf::from)).flatten() {
                super::tree::set_pinned(&dir, !super::tree::is_pinned(&dir));
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
            if let Some((dir, events)) = with_app(|a| {
                let dir = a.view.browser.dir().filter(|d| !crate::places::is_this_pc(d))?;
                Some((dir.to_path_buf(), a.events.clone()))
            })
            .flatten()
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
        SWAP_PANES => super::panes::swap(),
        MIRROR_PANE => super::panes::mirror(),
        VERTICAL_TABS => super::tabbar::set_vertical(with_app(|a| !a.vertical_tabs).unwrap_or(false)),
        COMPARE => super::panes::compare(),
        REVEAL => super::tree::reveal(),
        GROUP => {
            // Every pane and tab showing this folder re-sorts into (or out of) date groups.
            let ids = with_app(|a| {
                let dir = a.view.browser.dir().map(std::path::PathBuf::from)?;
                if a.grouped.contains(&dir) {
                    a.grouped.retain(|p| p != &dir);
                } else {
                    a.grouped.push(dir.clone());
                }
                Some(
                    a.views()
                        .filter(|v| v.browser.dir() == Some(dir.as_path()))
                        .map(|v| v.id)
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .unwrap_or_default();
            for id in ids {
                if let Some(job) = super::with_view(id, |v| v.browser.resort_job()).flatten() {
                    spawn_sort(job);
                }
            }
            if let Some(s) = super::current_settings(hwnd) {
                super::save_preferences(s);
            }
        }
        SORT_NAME..=SORT_SIZE | DESCENDING | SORT_CREATED | SORT_UP | SORT_DOWN => {
            let job = with_app(|a| {
                let key = match id {
                    SORT_NAME => SortKey::Name,
                    SORT_DATE => SortKey::Modified,
                    SORT_TYPE => SortKey::Type,
                    SORT_SIZE => SortKey::Size,
                    SORT_CREATED => SortKey::Created,
                    _ => a.view.browser.sort.key,
                };
                let descending = match id {
                    DESCENDING => !a.view.browser.sort.descending,
                    SORT_UP => false,
                    SORT_DOWN => true,
                    _ => a.view.browser.sort.descending,
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
        PALETTE => super::palette::open(hwnd),
        NEW_WINDOW => super::open_side_by_side(hwnd),
        FOCUS_OTHER_PANE => super::panes::focus_other(),
        FOCUS_FILTER => {
            if let Some(filter) = with_app(|a| a.filter) {
                unsafe {
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(filter));
                    SendMessageW(filter, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
                }
            }
        }
        VIEW | MORE | HELP | SORT_MENU => popup(hwnd, id),
        GROUP_NONE | GROUP_DATE => {
            let grouped =
                with_app(|a| a.view.browser.dir().is_some_and(|d| a.grouped.iter().any(|g| g == d)));
            if grouped == Some(id == GROUP_NONE) {
                return execute(GROUP);
            }
        }
        _ => return false,
    }
    true
}

/// Adds item `id` to `menu`, with its shortcut, grayed when it doesn't apply, and checked.
fn item(menu: HMENU, id: usize, text: &str, checked: bool) {
    let flags = MF_STRING
        | if enabled(id) { MF_ENABLED } else { MF_GRAYED }
        | if checked { MF_CHECKED } else { MF_UNCHECKED };
    unsafe {
        let _ = AppendMenuW(menu, flags, id, &HSTRING::from(super::palette::menu_text(text, id)));
    }
}

fn separator(menu: HMENU) {
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    }
}

/// A submenu called `text` in `menu`, filled by `fill`.
fn submenu(menu: HMENU, text: &str, fill: impl FnOnce(HMENU)) {
    unsafe {
        let Ok(sub) = CreatePopupMenu() else { return };
        fill(sub);
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, &HSTRING::from(text));
    }
}

/// The Sort, View, More and Help menus, laid out like Windows 11 Explorer's: Sort has the
/// sort key, its direction and grouping; View has the layout, then what to show, the panes and
/// the tabs. Each item names its shortcut.
fn popup(hwnd: HWND, kind: usize) {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        fill(menu, kind);
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

/// Puts the items of menu `kind` (Sort, View, More or Help) into `menu`.
fn fill(menu: HMENU, kind: usize) {
    let Some((sort, hidden, preview, grid, size, grouped, split, sync, vertical)) = with_app(|a| {
        (
            a.view.browser.sort,
            a.view.browser.show_hidden,
            preview::is_visible_in(a),
            a.view.grid.enabled,
            a.view.grid.size,
            a.view.browser.dir().is_some_and(|d| a.grouped.iter().any(|g| g == d)),
            a.split_view,
            a.sync_panes,
            a.vertical_tabs,
        )
    }) else {
        return;
    };
    unsafe {
        match kind {
            SORT_MENU => {
                for (id, label, key) in [
                    (SORT_NAME, "&Name", SortKey::Name),
                    (SORT_DATE, "Date &modified", SortKey::Modified),
                    (SORT_TYPE, "&Type", SortKey::Type),
                    (SORT_SIZE, "&Size", SortKey::Size),
                    (SORT_CREATED, "Date &created", SortKey::Created),
                ] {
                    item(menu, id, label, sort.key == key);
                }
                separator(menu);
                item(menu, SORT_UP, "&Ascending", !sort.descending);
                item(menu, SORT_DOWN, "&Descending", sort.descending);
                separator(menu);
                submenu(menu, "&Group by", |sub| {
                    item(sub, GROUP_NONE, "(&None)", !grouped);
                    item(sub, GROUP_DATE, "Date &modified", grouped);
                });
            }
            VIEW => {
                item(menu, DETAILS, "&Details", !grid);
                for (i, &(px, label)) in THUMB_SIZES.iter().enumerate() {
                    item(menu, THUMBS + i, label, grid && size == px);
                }
                separator(menu);
                submenu(menu, "&Show", |sub| {
                    item(sub, HIDDEN, "&Hidden items", hidden);
                    item(sub, PREVIEW, "&Preview pane", preview);
                    item(sub, FOLDER_SIZES, "Folder &sizes (calculate now)", false);
                });
                submenu(menu, "P&anes", |sub| {
                    item(sub, SPLIT, "&Two panes", split);
                    separator(sub);
                    item(sub, FOCUS_OTHER_PANE, "&Focus the other pane", false);
                    item(sub, SWAP_PANES, "S&wap panes", false);
                    item(sub, MIRROR_PANE, "Show this folder in the &other pane", false);
                    item(sub, SYNC_PANES, "S&ync pane navigation", sync);
                    item(sub, COMPARE, "&Compare panes", false);
                });
                submenu(menu, "&Tabs", |sub| {
                    item(sub, VERTICAL_TABS, "Tabs on the &left", vertical);
                    item(sub, TAB_LIST, "List &all tabs", false);
                    separator(sub);
                    item(sub, REOPEN_TAB, "&Reopen closed tab", false);
                    item(sub, CLOSE_OTHER_TABS, "Close &other tabs", false);
                });
                separator(menu);
                item(menu, REVEAL, "Show in s&idebar", false);
            }
            HELP => {
                item(menu, PALETTE, "&Find a command or shortcut", false);
                item(menu, SHORTCUTS, "&Keyboard shortcuts", false);
                separator(menu);
                item(
                    menu,
                    UPDATES,
                    &format!("Check for &updates (this is {})", env!("CARGO_PKG_VERSION")),
                    false,
                );
            }
            _ => {
                // The toolbar buttons that didn't fit come first.
                let hidden_buttons: Vec<_> = with_app(|a| {
                    a.toolbar
                        .iter()
                        .zip(BUTTONS)
                        .filter(|(b, _)| GetWindowLongPtrW(**b, GWL_STYLE) as u32 & WS_VISIBLE.0 == 0)
                        .map(|(_, &(id, label, _))| (id, label))
                        .collect()
                })
                .unwrap_or_default();
                for &(id, label) in &hidden_buttons {
                    match id {
                        VIEW => submenu(menu, "&View", |sub| fill(sub, VIEW)),
                        SORT_MENU => submenu(menu, "&Sort", |sub| fill(sub, SORT_MENU)),
                        _ => item(menu, id, label, false),
                    }
                }
                if !hidden_buttons.is_empty() {
                    separator(menu);
                }
                let pinned = with_app(|a| a.view.browser.dir().map(std::path::PathBuf::from))
                    .flatten()
                    .is_some_and(|d| super::tree::is_pinned(&d));
                item(
                    menu,
                    PIN,
                    if pinned { "Unpin from &Quick access" } else { "Pin to &Quick access" },
                    pinned,
                );
                item(menu, RECENT, "&Recent folders", false);
                item(menu, PARENTS, "&Parent folders", false);
                separator(menu);
                item(menu, commands::CMD_COPY_PATH, "Copy as pa&th", false);
                item(menu, commands::CMD_SELECT_ALL, "Select &all", false);
                item(menu, commands::CMD_SHOW_IN_EXPLORER, "Show in &Explorer", false);
                item(menu, commands::CMD_MORE_OPTIONS, "&Windows context menu", false);
                separator(menu);
                item(menu, TERMINAL, "Open &terminal here", false);
                item(menu, OPERATIONS, "File &operations", false);
                item(menu, NEW_WINDOW, "New &window", false);
            }
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
