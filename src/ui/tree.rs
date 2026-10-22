//! Sidebar: Quick access folders and drives, folders loaded on expand by a worker.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, MF_SEPARATOR, MF_STRING, SendMessageW,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WINDOW_EX_STYLE, WINDOW_STYLE, WS_TABSTOP,
};
use windows::core::{HSTRING, PCWSTR, PWSTR, Result, w};

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
        let style = TVS_HASBUTTONS | TVS_LINESATROOT | TVS_SHOWSELALWAYS | TVS_FULLROWSELECT | TVS_INFOTIP;
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
    let shown = quick_access();
    for (path, name) in shown.iter().zip(places::short_names(&shown)) {
        let node = insert(Some(quick), &name, Some(path.clone()), true);
        with_app(|a| a.view.icons.request_node((a.tree_epoch << 32) | node, path.clone()));
    }
    // This PC opens the drives page; its drives are already below it, so it never loads.
    let pc = insert(None, places::THIS_PC, Some(places::this_pc()), true);
    with_app(|a| a.nodes[pc].state = State::Loaded);
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
    // Only expanding and selecting carry this larger structure.
    let tree_view = || unsafe { &*(lp.0 as *const NMTREEVIEWW) };
    let nm = matches!(hdr.code, TVN_ITEMEXPANDINGW | TVN_SELCHANGEDW).then(tree_view);
    let node = nm.map_or(0, |nm| nm.itemNew.lParam.0 as usize);
    let action = nm.map(|nm| nm.action);
    match hdr.code {
        TVN_ITEMEXPANDINGW if action.is_some_and(|a| a.0 & TVE_EXPAND.0 != 0) => {
            start_loading(node);
            Some(LRESULT(0))
        }
        // Hovering an item shows its full path.
        TVN_GETINFOTIPW => {
            let tip = unsafe { &mut *(lp.0 as *mut NMTVGETINFOTIPW) };
            let path =
                with_app(|a| a.nodes.get(tip.lParam.0 as usize).and_then(|n| n.path.clone())).flatten();
            if let Some(path) = path
                && tip.cchTextMax > 0
                && !tip.pszText.is_null()
            {
                let text: Vec<u16> = path.display().to_string().encode_utf16().collect();
                let n = text.len().min(tip.cchTextMax as usize - 1);
                unsafe {
                    std::ptr::copy_nonoverlapping(text.as_ptr(), tip.pszText.0, n);
                    *tip.pszText.0.add(n) = 0;
                }
            }
            Some(LRESULT(0))
        }
        TVN_SELCHANGEDW if action == Some(TVC_BYMOUSE) || action == Some(TVC_BYKEYBOARD) => {
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

/// The folders Quick access shows, in order.
fn quick_access() -> Vec<PathBuf> {
    let known: Vec<PathBuf> = places::known_folders().into_iter().map(|k| k.1).collect();
    with_app(|a| places::quick_access(&known, &a.pins, &a.unpinned)).unwrap_or_default()
}

pub(super) fn is_pinned(dir: &Path) -> bool {
    quick_access().iter().any(|p| crate::browser::same_dir(p, dir))
}

/// Adds `dir` to Quick access or takes it out, in every window, and saves the change.
pub(super) fn set_pinned(dir: &Path, on: bool) {
    if !crate::settings::rememberable(dir) {
        return;
    }
    let known: Vec<PathBuf> = places::known_folders().into_iter().map(|k| k.1).collect();
    let Some(hwnd) = with_app(|a| {
        places::set_quick_access(dir, on, &known, &mut a.pins, &mut a.unpinned);
        a.hwnd
    }) else {
        return;
    };
    reload();
    if let Some(s) = super::current_settings(hwnd) {
        super::save_preferences(s);
    }
}

/// Right-click (or Shift+F10) on the sidebar. `lp` is the screen point, or -1 from the keyboard.
pub(super) fn context_menu(hwnd: HWND, lp: LPARAM) {
    let Some(tree) = with_app(|a| a.tree) else { return };
    let mut at = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
    let item = unsafe {
        if lp.0 == -1 {
            let _ = GetCursorPos(&mut at);
            SendMessageW(tree, TVM_GETNEXTITEM, Some(WPARAM(TVGN_CARET as usize)), None).0
        } else {
            let mut hit = TVHITTESTINFO { pt: at, ..Default::default() };
            let _ = ScreenToClient(tree, &mut hit.pt);
            SendMessageW(tree, TVM_HITTEST, None, Some(LPARAM(&mut hit as *mut _ as isize))).0
        }
    };
    if item == 0 {
        return;
    }
    let Some(path) =
        with_app(|a| a.nodes.iter().find(|n| n.item == item).and_then(|n| n.path.clone())).flatten()
    else {
        return;
    };
    let pinned = is_pinned(&path);
    const NEW_TAB: usize = 1;
    const OTHER_PANE: usize = 2;
    const PIN: usize = 3;
    const PROPERTIES: usize = 4;
    let picked = unsafe {
        // Marks the item the menu is for, as Explorer does.
        SendMessageW(tree, TVM_SELECTITEM, Some(WPARAM(TVGN_DROPHILITE as usize)), Some(LPARAM(item)));
        let Ok(menu) = CreatePopupMenu() else { return };
        let _ = AppendMenuW(menu, MF_STRING, NEW_TAB, w!("Open in new &tab"));
        let _ = AppendMenuW(menu, MF_STRING, OTHER_PANE, w!("Open in other &pane"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let pin = if pinned { w!("&Unpin from Quick access") } else { w!("P&in to Quick access") };
        let _ = AppendMenuW(menu, MF_STRING, PIN, pin);
        if !places::is_this_pc(&path) {
            let _ = AppendMenuW(menu, MF_STRING, PROPERTIES, w!("P&roperties"));
        }
        let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, at.x, at.y, None, hwnd, None).0;
        let _ = DestroyMenu(menu);
        SendMessageW(tree, TVM_SELECTITEM, Some(WPARAM(TVGN_DROPHILITE as usize)), Some(LPARAM(0)));
        picked as usize
    };
    match picked {
        NEW_TAB => super::tabbar::new_tab(Some(path)),
        OTHER_PANE => super::panes::open_in_other(path),
        PIN => set_pinned(&path, !pinned),
        PROPERTIES => {
            if let Err(e) = crate::shellmenu::launch(at.x, at.y, &[path], true) {
                set_status(&format!("Couldn't show properties: {e}"));
            }
        }
        _ => {}
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
/// Then take the next step of a `reveal`, if one is under way.
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
    if with_app(|a| a.tree_revealing) != Some(true) {
        return deselect_elsewhere(tree, dir);
    }
    // Before the first listing there is nothing to reveal yet; the next sync carries on.
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
    let Some((node, _, exact)) = closest else { return stop_revealing() };
    if !exact
        && let Some(parent) =
            with_app(|a| (a.nodes[node].state == State::Loaded).then(|| a.nodes[node].path.clone()).flatten())
                .flatten()
        && let Some(next) = dir.ancestors().find(|p| {
            p.parent().is_some_and(|p| p.to_string_lossy().eq_ignore_ascii_case(&parent.to_string_lossy()))
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
    if exact {
        stop_revealing();
    } else {
        start_loading(node);
        expand(node);
    }
}

/// Expands the sidebar down to the open folder. Folders load on a worker, so this
/// continues from `on_children` until it gets there.
pub(super) fn reveal() {
    with_app(|a| a.tree_revealing = true);
    sync();
}

fn stop_revealing() {
    with_app(|a| a.tree_revealing = false);
}

/// The sidebar doesn't follow navigation; drop a selection that no longer matches
/// the open folder so clicking that item navigates again.
fn deselect_elsewhere(tree: HWND, dir: Option<PathBuf>) {
    let item = unsafe { SendMessageW(tree, TVM_GETNEXTITEM, Some(WPARAM(TVGN_CARET as usize)), None) }.0;
    if item == 0 {
        return;
    }
    let mut tv = TVITEMW { mask: TVIF_PARAM, hItem: HTREEITEM(item), ..Default::default() };
    unsafe { SendMessageW(tree, TVM_GETITEMW, None, Some(LPARAM(&mut tv as *mut _ as isize))) };
    let selected = with_app(|a| a.nodes.get(tv.lParam.0 as usize).and_then(|n| n.path.clone())).flatten();
    let same = selected
        .zip(dir)
        .is_some_and(|(s, d)| s.to_string_lossy().eq_ignore_ascii_case(&d.to_string_lossy()));
    if !same {
        unsafe { SendMessageW(tree, TVM_SELECTITEM, Some(WPARAM(TVGN_CARET as usize)), Some(LPARAM(0))) };
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
