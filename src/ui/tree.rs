//! Sidebar: Quick access folders and drives, folders loaded on expand by a worker.

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WS_TABSTOP};
use windows::core::{HSTRING, PWSTR, Result, w};

use super::{Event, child, go, set_status, spawn, with_app};
use crate::places::{self, DriveKind};

/// Children shown per folder. A folder with more sub-folders than this is
/// better browsed in the list.
// ponytail: hard cap, page in more on demand if anyone hits it.
const MAX_CHILDREN: usize = 5000;

#[derive(PartialEq)]
enum State {
    Unloaded,
    Loading,
    Loaded,
}

pub(super) struct Node {
    /// `None` for group headings ("Quick access", "This PC").
    path: Option<PathBuf>,
    item: isize,
    state: State,
}

pub(super) unsafe fn create(parent: HWND) -> Result<HWND> {
    unsafe {
        let style = TVS_HASBUTTONS | TVS_LINESATROOT | TVS_SHOWSELALWAYS | TVS_FULLROWSELECT;
        let tree =
            child(parent, WC_TREEVIEWW, w!(""), WS_TABSTOP | WINDOW_STYLE(style), WINDOW_EX_STYLE(0), 0)?;
        let ex = TVS_EX_DOUBLEBUFFER as usize;
        SendMessageW(tree, TVM_SETEXTENDEDSTYLE, Some(WPARAM(ex)), Some(LPARAM(ex as isize)));
        let _ = SetWindowTheme(tree, w!("Explorer"), None);
        Ok(tree)
    }
}

/// Adds the root groups. Call once the app state exists.
pub(super) fn populate() {
    let quick = insert(None, "Quick access", None, true);
    for (name, path) in places::known_folders() {
        let node = insert(Some(quick), name, Some(path.clone()), true);
        with_app(|a| a.icons.request_node(node, path));
    }
    let pc = insert(None, "This PC", None, true);
    for d in places::drives() {
        let node = insert(Some(pc), &places::drive_name(d.letter, d.kind, None), Some(d.root.clone()), true);
        with_app(|a| a.icons.request_node(node, d.root.clone()));
        // Network drives can take a long time to answer; their generic name is good enough.
        if d.kind != DriveKind::Network
            && let Some(events) = with_app(|a| a.events.clone())
        {
            spawn("label", move || {
                if let Some(label) = places::volume_label(&d.root) {
                    let text = places::drive_name(d.letter, d.kind, Some(&label));
                    events.send(Event::DriveLabel { node, text });
                }
            });
        }
    }
    for group in [quick, pc] {
        expand(group);
    }
}

/// Adds a node and returns its index (stored as the item's lParam).
fn insert(parent: Option<usize>, text: &str, path: Option<PathBuf>, expandable: bool) -> usize {
    let loaded = path.is_none();
    let Some((tree, idx, parent_item, folder)) = with_app(|a| {
        let parent_item = parent.map_or(TVI_ROOT.0, |p| a.nodes[p].item);
        a.nodes.push(Node { path, item: 0, state: if loaded { State::Loaded } else { State::Unloaded } });
        (a.tree, a.nodes.len() - 1, parent_item, a.icons.folder)
    }) else {
        return usize::MAX;
    };
    let text = HSTRING::from(text);
    let ins = TVINSERTSTRUCTW {
        hParent: HTREEITEM(parent_item),
        hInsertAfter: TVI_LAST,
        Anonymous: TVINSERTSTRUCTW_0 {
            itemex: TVITEMEXW {
                mask: TVIF_TEXT | TVIF_PARAM | TVIF_CHILDREN | TVIF_IMAGE | TVIF_SELECTEDIMAGE,
                iImage: folder,
                iSelectedImage: folder,
                pszText: PWSTR(text.as_ptr() as *mut _),
                cChildren: TVITEMEXW_CHILDREN(expandable as i32),
                lParam: LPARAM(idx as isize),
                ..Default::default()
            },
        },
    };
    let item =
        unsafe { SendMessageW(tree, TVM_INSERTITEMW, None, Some(LPARAM(&ins as *const _ as isize))) }.0;
    with_app(|a| a.nodes[idx].item = item);
    idx
}

fn expand(node: usize) {
    if let Some((tree, item)) = with_app(|a| (a.tree, a.nodes[node].item)) {
        unsafe { SendMessageW(tree, TVM_EXPAND, Some(WPARAM(TVE_EXPAND.0 as usize)), Some(LPARAM(item))) };
    }
}

fn set_expandable(node: usize, expandable: bool) {
    let Some((tree, item)) = with_app(|a| (a.tree, a.nodes[node].item)) else { return };
    let tv = TVITEMW {
        mask: TVIF_CHILDREN,
        hItem: HTREEITEM(item),
        cChildren: TVITEMEXW_CHILDREN(expandable as i32),
        ..Default::default()
    };
    unsafe { SendMessageW(tree, TVM_SETITEMW, None, Some(LPARAM(&tv as *const _ as isize))) };
}

/// `None` = not handled.
pub(super) unsafe fn on_notify(hdr: &NMHDR, lp: LPARAM) -> Option<LRESULT> {
    let nm = unsafe { &*(lp.0 as *const NMTREEVIEWW) };
    let node = nm.itemNew.lParam.0 as usize;
    match hdr.code {
        TVN_ITEMEXPANDINGW if nm.action.0 & TVE_EXPAND.0 != 0 => {
            start_loading(node);
            Some(LRESULT(0))
        }
        TVN_SELCHANGEDW if nm.action == TVC_BYMOUSE || nm.action == TVC_BYKEYBOARD => {
            if let Some(path) = with_app(|a| a.nodes.get(node).and_then(|n| n.path.clone())).flatten() {
                go(path);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn start_loading(node: usize) {
    let Some((path, show_hidden, events)) = with_app(|a| {
        let n = a.nodes.get_mut(node)?;
        if n.state != State::Unloaded {
            return None;
        }
        n.state = State::Loading;
        Some((n.path.clone()?, a.browser.show_hidden, a.events.clone()))
    })
    .flatten() else {
        return;
    };
    spawn("tree", move || {
        let result = crate::crash::guard(|| places::subfolders(&path, show_hidden))
            .unwrap_or_else(|| Err(std::io::Error::other("internal error")));
        events.send(Event::TreeChildren { node, result });
    });
}

pub(super) fn on_children(node: usize, result: std::io::Result<Vec<String>>) {
    let Some(path) = with_app(|a| {
        a.nodes[node].state = State::Loaded;
        a.nodes[node].path.clone()
    })
    .flatten() else {
        return;
    };
    match result {
        Ok(names) if !names.is_empty() => {
            for name in names.iter().take(MAX_CHILDREN) {
                insert(Some(node), name, Some(path.join(name)), true);
            }
            expand(node);
        }
        Ok(_) => set_expandable(node, false),
        Err(e) => {
            set_expandable(node, false);
            set_status(&format!("Can't open {}: {e}", path.display()));
        }
    }
}

pub(super) fn on_label(node: usize, text: String) {
    let Some((tree, item)) = with_app(|a| (a.tree, a.nodes[node].item)) else { return };
    let text = HSTRING::from(text);
    let tv = TVITEMW {
        mask: TVIF_TEXT,
        hItem: HTREEITEM(item),
        pszText: PWSTR(text.as_ptr() as *mut _),
        ..Default::default()
    };
    unsafe { SendMessageW(tree, TVM_SETITEMW, None, Some(LPARAM(&tv as *const _ as isize))) };
}

pub(super) fn on_icon(node: usize, icon: i32) {
    let Some((tree, item)) = with_app(|a| (a.tree, a.nodes[node].item)) else { return };
    let tv = TVITEMW {
        mask: TVIF_IMAGE | TVIF_SELECTEDIMAGE,
        hItem: HTREEITEM(item),
        iImage: icon,
        iSelectedImage: icon,
        ..Default::default()
    };
    unsafe { SendMessageW(tree, TVM_SETITEMW, None, Some(LPARAM(&tv as *const _ as isize))) };
}

/// Sidebar width for a drag to `x`: keeps both panes usable.
pub(super) fn clamp_split(x: i32, client_width: i32, dpi: u32) -> i32 {
    let s = |px: i32| px * dpi as i32 / 96;
    x.min(client_width - s(200)).max(s(120))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_both_panes_usable() {
        assert_eq!(clamp_split(300, 1000, 96), 300);
        assert_eq!(clamp_split(10, 1000, 96), 120);
        assert_eq!(clamp_split(950, 1000, 96), 800);
        assert_eq!(clamp_split(10, 1000, 192), 240);
        // Window narrower than both minimums: the sidebar keeps its minimum.
        assert_eq!(clamp_split(500, 250, 96), 120);
    }
}
