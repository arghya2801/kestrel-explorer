//! Things the user does to files: the right-click menu, its keyboard
//! shortcuts, rename in place, and the clipboard. File work runs on its own
//! thread through `ops`; results come back as `Event::OpDone`.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR, w};

use super::{Event, go, open_file, selected_rows, set_status, spawn, spawn_list, with_app};
use crate::ops::{self, Mode, Op};
use crate::{clip, crash};

pub const CMD_OPEN: usize = 100;
pub const CMD_CUT: usize = 101;
pub const CMD_COPY: usize = 102;
pub const CMD_PASTE: usize = 103;
pub const CMD_RENAME: usize = 104;
pub const CMD_DELETE: usize = 105;
pub const CMD_DELETE_PERMANENTLY: usize = 106;
pub const CMD_NEW_FOLDER: usize = 107;
pub const CMD_COPY_PATH: usize = 108;
pub const CMD_SHOW_IN_EXPLORER: usize = 109;
pub const CMD_SELECT_ALL: usize = 110;
pub const CMD_MORE_OPTIONS: usize = 111;
pub const CMD_OPEN_NEW_TAB: usize = 112;
pub const CMD_PROPERTIES: usize = 113;
pub const CMD_UNDO: usize = 114;
pub const CMD_REDO: usize = 115;
pub const CMD_PIN: usize = 116;
pub const CMD_OPEN_OTHER_PANE: usize = 117;
pub const CMD_COPY_TO_OTHER: usize = 118;
pub const CMD_MOVE_TO_OTHER: usize = 119;
/// First of the 7-Zip commands, one per `sevenzip::ACTIONS` entry (120..=127).
pub const CMD_7ZIP: usize = 120;

/// Commands that change files. This PC lists drives, not files: they don't apply there.
const CHANGES_FILES: [usize; 9] = [
    CMD_CUT,
    CMD_COPY,
    CMD_PASTE,
    CMD_RENAME,
    CMD_DELETE,
    CMD_DELETE_PERMANENTLY,
    CMD_NEW_FOLDER,
    CMD_COPY_TO_OTHER,
    CMD_MOVE_TO_OTHER,
];

pub(super) fn on_this_pc() -> bool {
    current_dir().is_some_and(|d| crate::places::is_this_pc(&d))
}

/// Runs a menu command. Returns false for ids that aren't ours.
pub(super) fn run(cmd: usize) -> bool {
    let seven_zip = cmd.checked_sub(CMD_7ZIP).and_then(|i| crate::sevenzip::ACTIONS.get(i).copied());
    if (CHANGES_FILES.contains(&cmd) || seven_zip.is_some()) && on_this_pc() {
        return true;
    }
    if let Some(action) = seven_zip {
        run_seven_zip(action);
        return true;
    }
    match cmd {
        CMD_OPEN => open_selected(),
        CMD_CUT => copy_selection(true),
        CMD_COPY => copy_selection(false),
        CMD_PASTE => paste(),
        CMD_RENAME => rename_selected(),
        CMD_DELETE => delete_selected(false),
        CMD_DELETE_PERMANENTLY => delete_selected(true),
        CMD_NEW_FOLDER => {
            if let Some(dir) = current_dir() {
                start_op(Op::NewFolder { dir }, true);
            }
        }
        CMD_COPY_PATH => copy_paths(),
        CMD_SHOW_IN_EXPLORER => show_in_explorer(),
        CMD_SELECT_ALL => select_all(),
        CMD_MORE_OPTIONS => more_options(None, false),
        CMD_PROPERTIES => more_options(None, true),
        CMD_UNDO => start_history(false),
        CMD_REDO => start_history(true),
        CMD_OPEN_NEW_TAB => {
            if let Some((dir, true)) = selected_paths().into_iter().next() {
                super::tabbar::new_tab(Some(dir));
            }
        }
        CMD_OPEN_OTHER_PANE => {
            if let Some((dir, true)) = selected_paths().into_iter().next() {
                super::panes::open_in_other(dir);
            }
        }
        CMD_PIN => {
            if let Some(dir) = pin_target() {
                super::tree::set_pinned(&dir, !super::tree::is_pinned(&dir));
            }
        }
        CMD_COPY_TO_OTHER | CMD_MOVE_TO_OTHER => to_other_pane(cmd == CMD_MOVE_TO_OTHER),
        _ => return false,
    }
    true
}

/// Runs a 7-Zip command on the selection, on a worker: 7-Zip shows its own progress and errors.
fn run_seven_zip(action: crate::sevenzip::Action) {
    let selected = selected_paths();
    let Some(dir) = crate::sevenzip::install_dir() else { return };
    if !crate::sevenzip::applies(action, &selected) {
        return;
    }
    let items: Vec<PathBuf> = selected.into_iter().map(|p| p.0).collect();
    let Some(events) = with_app(|a| a.events.clone()) else { return };
    spawn("7-zip", move || {
        if let Err(e) = crash::guard(|| crate::sevenzip::run(dir, action, &items))
            .unwrap_or_else(|| Err("Internal error while running 7-Zip.".into()))
        {
            events.send(Event::Message(e));
        }
    });
}

/// Adds the 7-Zip submenu for the selection, when 7-Zip is installed and the items are files
/// on disk (not drives, nor the inside of a .zip shown as a folder).
unsafe fn add_seven_zip(menu: HMENU, selected: &[(PathBuf, bool)]) {
    let in_zip = current_dir().is_some_and(|d| d.ancestors().any(crate::shell::is_zip));
    if crate::sevenzip::install_dir().is_none() || selected.is_empty() || on_this_pc() || in_zip {
        return;
    }
    let items: Vec<PathBuf> = selected.iter().map(|p| p.0.clone()).collect();
    unsafe {
        let Ok(sub) = CreatePopupMenu() else { return };
        for (i, &action) in crate::sevenzip::ACTIONS.iter().enumerate() {
            if action == crate::sevenzip::Action::AddDialog && GetMenuItemCount(Some(sub)) > 0 {
                let _ = AppendMenuW(sub, MF_SEPARATOR, 0, PCWSTR::null());
            }
            if crate::sevenzip::applies(action, selected) {
                let text = HSTRING::from(crate::sevenzip::label(action, &items));
                let _ = AppendMenuW(sub, MF_STRING, CMD_7ZIP + i, &text);
            }
        }
        let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("7-&Zip"));
    }
}

/// The folder Pin acts on: the selected folder, or with nothing selected, the open one.
fn pin_target() -> Option<PathBuf> {
    match selected_paths().first() {
        Some((dir, true)) => Some(dir.clone()),
        Some(_) => None,
        None => current_dir(),
    }
}

/// Copies or moves the selection into the folder shown in the other pane.
fn to_other_pane(move_: bool) {
    let paths: Vec<PathBuf> = selected_paths().into_iter().map(|p| p.0).collect();
    let Some(dest) = super::panes::other_dir() else {
        return set_status("Open a second pane (Split) to copy or move there.");
    };
    if !paths.is_empty() {
        transfer(paths, dest, move_);
    }
}

fn current_dir() -> Option<PathBuf> {
    with_app(|a| a.view.browser.dir().map(|d| d.to_path_buf())).flatten()
}

fn selected_paths() -> Vec<(PathBuf, bool)> {
    let rows = selected_rows();
    with_app(|a| rows.iter().filter_map(|&r| a.view.browser.path_at(r)).collect()).unwrap_or_default()
}

fn list() -> Option<HWND> {
    with_app(|a| a.view.list)
}

/// Shows the right-click menu for the list at `lp` (screen coordinates, or -1 from the keyboard).
pub(super) fn context_menu(hwnd: HWND, lp: LPARAM) {
    let Some(list) = list() else { return };
    // Shift+right-click: straight to Windows' full menu, as in Explorer.
    if lp.0 != -1 && unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0 {
        let at = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
        return more_options(Some(at), false);
    }
    let has_selection = !selected_rows().is_empty();
    let files = !on_this_pc();
    // Only asks whether files are there; doesn't make the copying app render anything.
    let can_paste = files && unsafe { IsClipboardFormatAvailable(CF_HDROP.0 as u32) }.is_ok();
    let changes = has_selection && files;
    let other_pane = super::panes::other_dir().is_some();
    let (mut x, mut y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
    if lp.0 == -1 {
        // Shift+F10 / menu key: under the focused row, or the list's corner.
        let focused = unsafe {
            SendMessageW(list, LVM_GETNEXTITEM, Some(WPARAM(usize::MAX)), Some(LPARAM(LVNI_FOCUSED as isize)))
        }
        .0;
        let mut r = RECT { left: LVIR_LABEL as i32, ..Default::default() };
        if focused >= 0 {
            unsafe {
                SendMessageW(
                    list,
                    LVM_GETITEMRECT,
                    Some(WPARAM(focused as usize)),
                    Some(LPARAM(&mut r as *mut _ as isize)),
                )
            };
        }
        let mut pt = POINT { x: r.left, y: r.bottom };
        let _ = unsafe { ClientToScreen(list, &mut pt) };
        (x, y) = (pt.x, pt.y);
    }
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        let add = |id: usize, text: PCWSTR, on: bool| {
            let flags = MF_STRING | if on { MF_ENABLED } else { MF_GRAYED };
            let _ = AppendMenuW(menu, flags, id, text);
        };
        let line = || {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        };
        add(CMD_OPEN, w!("&Open\tEnter"), has_selection);
        let folder_selected = selected_paths().first().is_some_and(|p| p.1);
        add(CMD_OPEN_NEW_TAB, w!("Open in new &tab"), folder_selected);
        add(CMD_OPEN_OTHER_PANE, w!("Open in other pa&ne"), folder_selected);
        let pin = pin_target();
        let pin_text = match &pin {
            Some(dir) if super::tree::is_pinned(dir) => w!("Unpin from &Quick access"),
            _ => w!("Pin to &Quick access"),
        };
        add(CMD_PIN, pin_text, pin.is_some());
        add_seven_zip(menu, &selected_paths());
        line();
        add(CMD_CUT, w!("Cu&t\tCtrl+X"), changes);
        add(CMD_COPY, w!("&Copy\tCtrl+C"), changes);
        add(CMD_PASTE, w!("&Paste\tCtrl+V"), can_paste);
        if other_pane {
            add(CMD_COPY_TO_OTHER, w!("Copy to other pane\tShift+F5"), changes);
            add(CMD_MOVE_TO_OTHER, w!("Move to other pane\tShift+F6"), changes);
        }
        line();
        add(CMD_RENAME, w!("Rena&me\tF2"), changes);
        add(CMD_DELETE, w!("&Delete\tDel"), changes);
        add(CMD_NEW_FOLDER, w!("New &folder\tCtrl+Shift+N"), files);
        let (can_undo, can_redo) = ops::can_undo_redo();
        add(CMD_UNDO, w!("&Undo\tCtrl+Z"), can_undo);
        add(CMD_REDO, w!("Red&o\tCtrl+Y"), can_redo);
        line();
        add(CMD_COPY_PATH, w!("Copy &as path\tCtrl+Shift+C"), has_selection);
        add(CMD_SHOW_IN_EXPLORER, w!("Show in &Explorer"), true);
        add(CMD_MORE_OPTIONS, w!("&More options\tShift+right-click"), true);
        add(CMD_PROPERTIES, w!("P&roperties\tAlt+Enter"), true);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, None, hwnd, None).0;
        let _ = DestroyMenu(menu);
        if cmd > 0 {
            run(cmd as usize);
        }
    }
}

/// Windows' full menu (or Properties) for the selection or the open folder, in a
/// helper process so its add-ons can't crash or hang this window.
fn more_options(at: Option<(i32, i32)>, properties: bool) {
    let mut paths: Vec<PathBuf> = selected_paths().into_iter().map(|p| p.0).collect();
    if paths.is_empty() && !on_this_pc() {
        paths.extend(current_dir());
    }
    if paths.is_empty() {
        return;
    }
    let (x, y) = at.unwrap_or_else(|| {
        let mut pt = POINT::default();
        let _ = unsafe { GetCursorPos(&mut pt) };
        (pt.x, pt.y)
    });
    if let Err(e) = crate::shellmenu::launch(x, y, &paths, properties) {
        set_status(&format!("Couldn't show the full menu: {e}"));
    }
}

fn open_selected() {
    if let Some((path, is_dir)) = selected_paths().into_iter().next() {
        if is_dir { go(path) } else { open_file(path) }
    }
}

fn copy_selection(cut: bool) {
    let paths: Vec<PathBuf> = selected_paths().into_iter().map(|p| p.0).collect();
    let Some(hwnd) = with_app(|a| a.hwnd) else { return };
    if paths.is_empty() {
        return;
    }
    match clip::set_files(hwnd, &paths, cut) {
        Ok(()) => {
            let n = paths.len();
            let verb = if cut { "cut" } else { "copied" };
            set_status(&format!("{n} item{} {verb}", if n == 1 { "" } else { "s" }));
        }
        Err(e) => set_status(&e),
    }
}

fn copy_paths() {
    let text: Vec<String> = selected_paths().iter().map(|p| format!("\"{}\"", p.0.display())).collect();
    if let (Some(hwnd), false) = (with_app(|a| a.hwnd), text.is_empty())
        && let Err(e) = clip::set_text(hwnd, &text.join("\r\n"))
    {
        set_status(&e);
    }
}

fn paste() {
    let (Some(dest), Some((events, hwnd, view))) =
        (current_dir(), with_app(|a| (a.events.clone(), a.hwnd.0 as isize, a.view.id)))
    else {
        return;
    };
    // Reading the clipboard can wait on the app that copied: never on the UI thread.
    let running = super::OpGuard::new();
    spawn("paste", move || {
        let _running = running;
        let dir = Some(dest.clone());
        let ev = match crash::guard(clip::get_files).flatten() {
            None => Event::Message("Nothing to paste.".into()),
            Some((paths, cut)) => {
                let op = if cut { Op::Move { paths, dest } } else { Op::Copy { paths, dest } };
                let result = crash::guard(|| ops::run(&op, Mode::Interactive(hwnd)))
                    .unwrap_or_else(|| Err("Internal error while pasting.".into()));
                Event::OpDone { view, dir, result, edit: false }
            }
        };
        events.send(ev);
    });
}

fn delete_selected(permanent: bool) {
    let paths: Vec<PathBuf> = selected_paths().into_iter().map(|p| p.0).collect();
    if !paths.is_empty() {
        start_op(Op::Delete { paths, recycle: !permanent }, false);
    }
}

fn select_all() {
    let Some(list) = list() else { return };
    let item = LVITEMW { state: LVIS_SELECTED, stateMask: LVIS_SELECTED, ..Default::default() };
    unsafe {
        SendMessageW(
            list,
            LVM_SETITEMSTATE,
            Some(WPARAM(usize::MAX)),
            Some(LPARAM(&item as *const _ as isize)),
        )
    };
}

fn show_in_explorer() {
    let target = match selected_paths().into_iter().next() {
        Some((p, _)) => format!("/select,\"{}\"", p.display()),
        None if on_this_pc() => "shell:MyComputerFolder".into(),
        None => match current_dir() {
            Some(d) => format!("\"{}\"", d.display()),
            None => return,
        },
    };
    spawn("explorer", move || {
        let explorer = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
            .join("explorer.exe");
        if let Err(e) = std::process::Command::new(explorer).raw_arg(target).spawn() {
            crash::log(&format!("could not start Explorer: {e}"));
        }
    });
}

/// Copies or moves `paths` into `dest` (a drop).
pub(super) fn transfer(paths: Vec<PathBuf>, dest: PathBuf, move_: bool) {
    let op = if move_ { Op::Move { paths, dest } } else { Op::Copy { paths, dest } };
    start_op(op, false);
}

/// Runs `op` on its own thread; `edit` starts renaming the item it creates.
/// Undo (or Redo) of the latest file operation in this process, on a worker like any operation.
fn start_history(redo: bool) {
    let Some((events, hwnd, view, dir)) = with_app(|a| {
        (a.events.clone(), a.hwnd.0 as isize, a.view.id, a.view.browser.dir().map(PathBuf::from))
    }) else {
        return;
    };
    let running = super::OpGuard::new();
    spawn("undo", move || {
        let _running = running;
        let mode = Mode::Interactive(hwnd);
        match crash::guard(|| if redo { ops::redo(mode) } else { ops::undo(mode) }) {
            Some(Some(result)) => events.send(Event::OpDone { view, dir, result, edit: false }),
            Some(None) => {
                events.send(Event::Message(format!("Nothing to {}", if redo { "redo" } else { "undo" })))
            }
            None => events.send(Event::Message("Internal error during undo.".into())),
        }
    });
}

pub(super) fn start_op(op: Op, edit: bool) {
    let Some((events, hwnd, view, dir)) = with_app(|a| {
        (a.events.clone(), a.hwnd.0 as isize, a.view.id, a.view.browser.dir().map(PathBuf::from))
    }) else {
        return;
    };
    let running = super::OpGuard::new();
    spawn("op", move || {
        let _running = running;
        let result = crash::guard(|| ops::run(&op, Mode::Interactive(hwnd)))
            .unwrap_or_else(|| Err("Internal error during a file operation.".into()));
        events.send(Event::OpDone { view, dir, result, edit });
    });
}

pub(super) fn on_op_done(
    view: u64,
    dir: Option<PathBuf>,
    result: Result<Option<String>, String>,
    edit: bool,
) {
    let active = with_app(|a| a.view.id == view) == Some(true);
    match result {
        Ok(name) => {
            // The watcher would notice too; refreshing now makes the result show up at once.
            let job = super::with_view(view, |v| {
                if v.browser.dir() != dir.as_deref() {
                    return None;
                }
                if let Some(name) = name {
                    v.browser.select_after_refresh(&name);
                    if edit && active {
                        v.edit_pending = Some(name);
                    }
                }
                v.browser.on_dir_changed()
            });
            if let Some(job) = job.flatten() {
                spawn_list(job);
            }
        }
        Err(e) => set_status(&e),
    }
}

/// After the list shows new contents: start renaming a just-created item.
pub(super) fn after_refresh() {
    let Some((list, row)) = with_app(|a| {
        let name: Vec<u16> = a.view.edit_pending.as_ref()?.encode_utf16().collect();
        let row = a.view.browser.row_named(&name)?;
        a.view.edit_pending = None;
        Some((a.view.list, row))
    })
    .flatten() else {
        return;
    };
    unsafe {
        let _ = SetFocus(Some(list));
        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(row)), None);
    }
}

fn rename_selected() {
    let paths: Vec<PathBuf> = selected_paths().into_iter().map(|p| p.0).collect();
    if paths.len() > 1 {
        if let Some(hwnd) = with_app(|a| a.hwnd) {
            super::rename::open(hwnd, paths);
        }
        return;
    }
    let (Some(list), Some(&row)) = (list(), selected_rows().first()) else { return };
    unsafe {
        let _ = SetFocus(Some(list));
        SendMessageW(list, LVM_EDITLABELW, Some(WPARAM(row)), None);
    }
}

/// Rename box opened: select the name without its extension, like Explorer.
pub(super) fn begin_rename(lp: LPARAM) -> LRESULT {
    let row = unsafe { (*(lp.0 as *const NMLVDISPINFOW)).item.iItem };
    let Some(list) = list() else { return LRESULT(1) };
    // Search results show paths relative to the searched folder: rename in the folder itself.
    if with_app(|a| a.view.browser.query().is_some()) == Some(true) {
        return LRESULT(1);
    }
    let Some((path, is_dir)) =
        usize::try_from(row).ok().and_then(|r| with_app(|a| a.view.browser.path_at(r)).flatten())
    else {
        return LRESULT(1); // nothing there: don't edit
    };
    with_app(|a| a.view.renaming = Some(path.clone()));
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 && !is_dir => name[..dot].encode_utf16().count(),
        _ => name.encode_utf16().count(),
    };
    unsafe {
        let edit = HWND(SendMessageW(list, LVM_GETEDITCONTROL, None, None).0 as _);
        // Posted: the list selects all text after this notification returns.
        let _ = PostMessageW(Some(edit), EM_SETSEL, WPARAM(0), LPARAM(stem as isize));
    }
    LRESULT(0)
}

/// Rename box closed. The list never takes the new text itself: the rename
/// runs as a file operation and the refresh shows the result.
pub(super) fn end_rename(lp: LPARAM) -> LRESULT {
    let item = unsafe { &(*(lp.0 as *const NMLVDISPINFOW)).item };
    let Some((path, resync, events)) = with_app(|a| {
        (a.view.renaming.take(), std::mem::take(&mut a.view.resync_after_rename), a.events.clone())
    }) else {
        return LRESULT(0);
    };
    if resync && let Some(id) = with_app(|a| a.view.id) {
        events.send(Event::Resync(id)); // after the box is gone
    }
    let Some(path) = path else { return LRESULT(0) };
    if item.pszText.is_null() {
        return LRESULT(0); // cancelled
    }
    let text = unsafe { item.pszText.to_string() }.unwrap_or_default();
    match ops::validate_name(&text) {
        Err(e) => set_status(&e),
        Ok(name) if path.file_name().is_some_and(|n| n.to_string_lossy() == name) => {}
        Ok(name) => {
            with_app(|a| a.view.browser.select_after_refresh(&name));
            start_op(Op::Rename { path, name }, false);
        }
    }
    LRESULT(0)
}
