//! Sidebar: Quick access folders and drives, folders loaded on expand by a worker.

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WINDOW_EX_STYLE, WINDOW_STYLE, WS_TABSTOP};
use windows::core::{HSTRING, PWSTR, Result, w};

use super::{Event, child, go, set_status, spawn, with_app};
use crate::places::{self, DriveKind};

/// Add children in batches so a large folder does not monopolize the UI.
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
    /// 0 once the item is deleted (a removed drive): late results are ignored.
    item: isize,
    state: State,
    parent: Option<usize>,
    pending: std::collections::VecDeque<String>,
    more: bool,
    network: bool,
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

/// Adds the root groups and returns the "This PC" node. Call once the app state exists.
pub(super) fn populate() -> usize {
    let quick = insert(None, "Quick access", None, true);
    for (name, path) in places::known_folders() {
        let node = insert(Some(quick), name, Some(path.clone()), true);
        with_app(|a| a.view.icons.request_node((a.tree_epoch << 32) | node, path));
    }
    for path in with_app(|a| a.pins.clone()).unwrap_or_default() {
        let node = insert(Some(quick), &path.display().to_string(), Some(path.clone()), true);
        with_app(|a| a.view.icons.request_node((a.tree_epoch << 32) | node, path));
    }
    let pc = insert(None, "This PC", None, true);
    add_drives(pc);
    for group in [quick, pc] {
        expand(group);
    }
    let network = insert(None, "Network", None, true);
    with_app(|a| {
        a.nodes[network].network = true;
        a.nodes[network].state = State::Unloaded;
    });
    pc
}

fn add_drives(pc: usize) {
    for d in places::drives() {
        let node = insert(Some(pc), &places::drive_name(d.letter, d.kind, None), Some(d.root.clone()), true);
        with_app(|a| a.view.icons.request_node((a.tree_epoch << 32) | node, d.root.clone()));
        // Network drives can take a long time to answer; their generic name is good enough.
        if d.kind != DriveKind::Network
            && let Some(events) = with_app(|a| a.events.clone())
        {
            let encoded = token(node);
            spawn("label", move || {
                if let Some(label) = places::volume_label(&d.root) {
                    let text = places::drive_name(d.letter, d.kind, Some(&label));
                    events.send(Event::DriveLabel { node: encoded, text });
                }
            });
        }
    }
}

/// Drives came or went: rebuild the drive nodes.
pub(super) fn refresh_drives() {
    let Some((tree, pc, gone)) = with_app(|a| {
        let pc = a.this_pc;
        let under_pc = |mut n: usize| loop {
            match a.nodes[n].parent {
                Some(p) if p == pc => return true,
                Some(p) => n = p,
                None => return false,
            }
        };
        let doomed: Vec<usize> =
            (0..a.nodes.len()).filter(|&n| a.nodes[n].item != 0 && under_pc(n)).collect();
        let drives: Vec<isize> =
            doomed.iter().filter(|&&n| a.nodes[n].parent == Some(pc)).map(|&n| a.nodes[n].item).collect();
        for n in doomed {
            a.nodes[n].item = 0;
        }
        (a.tree, pc, drives)
    }) else {
        return;
    };
    for item in gone {
        unsafe { SendMessageW(tree, TVM_DELETEITEM, None, Some(LPARAM(item))) };
    }
    add_drives(pc);
}

/// The live tree item of `node`, if it still exists.
fn item_of(node: usize) -> Option<(HWND, isize)> {
    with_app(|a| a.nodes.get(node).map(|n| (a.tree, n.item)).filter(|&(_, item)| item != 0)).flatten()
}

/// Adds a node and returns its index (stored as the item's lParam).
fn insert(parent: Option<usize>, text: &str, path: Option<PathBuf>, expandable: bool) -> usize {
    let loaded = path.is_none();
    let Some((tree, idx, parent_item, folder)) = with_app(|a| {
        let parent_item = parent.map_or(TVI_ROOT.0, |p| a.nodes[p].item);
        let state = if loaded { State::Loaded } else { State::Unloaded };
        a.nodes.push(Node {
            path,
            item: 0,
            state,
            parent,
            pending: Default::default(),
            more: false,
            network: false,
        });
        (a.tree, a.nodes.len() - 1, parent_item, a.view.icons.folder)
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
    if let Some((tree, item)) = item_of(node) {
        unsafe { SendMessageW(tree, TVM_EXPAND, Some(WPARAM(TVE_EXPAND.0 as usize)), Some(LPARAM(item))) };
    }
}

fn set_expandable(node: usize, expandable: bool) {
    let Some((tree, item)) = item_of(node) else { return };
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
            if let Some(parent) =
                with_app(|a| a.nodes.get(node).filter(|n| n.more).and_then(|n| n.parent)).flatten()
            {
                show_page(parent);
                return Some(LRESULT(0));
            }
            if let Some(path) = with_app(|a| a.nodes.get(node).and_then(|n| n.path.clone())).flatten() {
                go(path);
            }
            Some(LRESULT(0))
        }
        _ => None,
    }
}

fn start_loading(node: usize) {
    let Some((path, network, show_hidden, events)) = with_app(|a| {
        let n = a.nodes.get_mut(node)?;
        if n.state != State::Unloaded {
            return None;
        }
        n.state = State::Loading;
        Some((n.path.clone(), n.network, a.view.browser.show_hidden, a.events.clone()))
    })
    .flatten() else {
        return;
    };
    let encoded = token(node);
    spawn("tree", move || {
        let result = crate::crash::guard(|| {
            if network {
                crate::network::computers()
            } else if let Some(path) = &path {
                if crate::network::server_of(path).is_some() {
                    crate::network::list_location(path).map(|s| {
                        (0..s.len())
                            .filter(|&i| show_hidden || !s.is_hidden(i))
                            .map(|i| s.name_string(i))
                            .collect()
                    })
                } else {
                    places::subfolders(path, show_hidden)
                }
            } else {
                Ok(Vec::new())
            }
        })
        .unwrap_or_else(|| Err(std::io::Error::other("internal error")));
        events.send(Event::TreeChildren { node: encoded, result });
    });
}

pub(super) fn on_children(encoded: usize, result: std::io::Result<Vec<String>>) {
    let Some(node) = resolve(encoded) else { return };
    if item_of(node).is_none() {
        return;
    }
    with_app(|a| a.nodes[node].state = State::Loaded);
    match result {
        Ok(names) if !names.is_empty() => {
            with_app(|a| a.nodes[node].pending = names.into());
            show_page(node);
            sync();
        }
        Ok(_) => {
            set_expandable(node, false);
        }
        Err(e) => {
            with_app(|a| a.nodes[node].state = State::Unloaded);
            set_status(&format!(
                "Sidebar unavailable: {e}. You can still enter a folder or server in the address bar."
            ));
        }
    }
}

pub(super) fn on_label(encoded: usize, text: String) {
    let Some(node) = resolve(encoded) else { return };
    let Some((tree, item)) = item_of(node) else { return };
    let text = HSTRING::from(text);
    let tv = TVITEMW {
        mask: TVIF_TEXT,
        hItem: HTREEITEM(item),
        pszText: PWSTR(text.as_ptr() as *mut _),
        ..Default::default()
    };
    unsafe { SendMessageW(tree, TVM_SETITEMW, None, Some(LPARAM(&tv as *const _ as isize))) };
}

pub(super) fn on_icon(encoded: usize, icon: i32) {
    let Some(node) = resolve(encoded) else { return };
    let Some((tree, item)) = item_of(node) else { return };
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

fn token(node: usize) -> usize {
    with_app(|a| (a.tree_epoch << 32) | node).unwrap_or(node)
}
fn resolve(token: usize) -> Option<usize> {
    with_app(|a| ((token >> 32) == a.tree_epoch).then_some(token & 0xffff_ffff)).flatten()
}

fn show_page(node: usize) {
    let Some((path, network, names, more, remaining)) = with_app(|a| {
        let n = a.nodes.get_mut(node)?;
        let names = n.pending.drain(..MAX_CHILDREN.min(n.pending.len())).collect::<Vec<_>>();
        let (path, network, remaining) = (n.path.clone(), n.network, n.pending.len());
        let more = a
            .nodes
            .iter_mut()
            .find(|n| n.parent == Some(node) && n.more && n.item != 0)
            .map(|n| std::mem::take(&mut n.item));
        Some((path, network, names, more, remaining))
    })
    .flatten() else {
        return;
    };
    if let Some(item) = more
        && let Some(tree) = with_app(|a| a.tree)
    {
        unsafe {
            SendMessageW(tree, TVM_DELETEITEM, None, Some(LPARAM(item)));
        }
    }
    for name in names {
        let path = if network {
            PathBuf::from(format!(r"\\{name}"))
        } else if let Some(base) = &path {
            base.join(&name)
        } else {
            continue;
        };
        insert(Some(node), &name, Some(path), true);
    }
    if remaining > 0 {
        let more = insert(Some(node), &format!("Load more… ({remaining} remaining)"), None, false);
        with_app(|a| a.nodes[more].more = true);
    }
    expand(node);
}

pub(super) fn reload() {
    with_app(|a| a.tree_hidden = !a.view.browser.show_hidden);
    sync();
}

/// Rebuild once when the active view's hidden setting changes; old worker results carry an epoch.
pub(super) fn sync() {
    let Some((tree, rebuild, dir)) = with_app(|a| {
        if a.nodes.is_empty() {
            return None;
        }
        let rebuild = a.tree_hidden != a.view.browser.show_hidden;
        if rebuild {
            a.tree_hidden = a.view.browser.show_hidden;
            a.tree_epoch += 1;
            a.nodes.clear();
        }
        Some((a.tree, rebuild, a.view.browser.dir().map(PathBuf::from)))
    })
    .flatten() else {
        return;
    };
    if rebuild {
        unsafe {
            SendMessageW(tree, TVM_DELETEITEM, None, Some(LPARAM(TVI_ROOT.0)));
        }
        let pc = populate();
        with_app(|a| a.this_pc = pc);
    }
    let Some(dir) = dir else { return };
    let target = dir.to_string_lossy().to_lowercase();
    if let Some(rest) = target.strip_prefix(r"\\").filter(|s| !s.starts_with("?\\"))
        && let Some(server) = rest.split('\\').next()
    {
        let server_path = PathBuf::from(format!(r"\\{server}"));
        let missing =
            with_app(|a| !a.nodes.iter().any(|n| n.path.as_ref() == Some(&server_path))).unwrap_or(false);
        if missing && let Some(network) = with_app(|a| a.nodes.iter().position(|n| n.network)).flatten() {
            insert(Some(network), server, Some(server_path), true);
        }
    }
    let closest = with_app(|a| {
        a.nodes
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                let path = n.path.as_ref()?.to_string_lossy().to_lowercase();
                let prefix = path.trim_end_matches('\\');
                (n.item != 0
                    && (target == path
                        || target.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('\\'))))
                .then_some((i, path.len(), target == path))
            })
            .max_by_key(|&(_, len, _)| len)
    })
    .flatten();
    if let Some((node, _, exact)) = closest {
        if !exact
            && let Some(parent) = with_app(|a| {
                (a.nodes[node].state == State::Loaded).then(|| a.nodes[node].path.clone()).flatten()
            })
            .flatten()
            && let Some(next) = dir.ancestors().find(|p| {
                p.parent()
                    .is_some_and(|p| p.to_string_lossy().eq_ignore_ascii_case(&parent.to_string_lossy()))
            })
            && let Some(name) = next.file_name()
        {
            let name = name.to_string_lossy();
            with_app(|a| a.nodes[node].pending.retain(|n| !n.eq_ignore_ascii_case(&name)));
            insert(Some(node), &name, Some(next.to_path_buf()), true);
            sync();
            return;
        }
        if let Some((tree, item)) = item_of(node) {
            unsafe {
                SendMessageW(tree, TVM_SELECTITEM, Some(WPARAM(TVGN_CARET as usize)), Some(LPARAM(item)));
                SendMessageW(tree, TVM_ENSUREVISIBLE, None, Some(LPARAM(item)));
            }
        }
        if !exact {
            start_loading(node);
            expand(node);
        }
    }
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
