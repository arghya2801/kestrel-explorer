//! Drag and drop with other apps. Dragging out hands the shell its own data
//! object for the selected files (same as dragging from Explorer). Dropping in
//! goes through `DropTarget`; the copy or move itself runs as a file operation.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, POINTL, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT, IDropTarget, IDropTarget_Impl};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{CLSID_DragDropHelper, IDropTargetHelper, SHDoDragDrop};
use windows::Win32::UI::WindowsAndMessaging::SendMessageW;
use windows::core::{Ref, Result, implement};

use super::{commands, selected_rows, with_app};
use crate::dnd::{self, MOVE, NONE};

/// Starts dragging the selected items out of the list.
pub(super) fn begin_drag() {
    // This PC's rows are whole drives: not something to drag around.
    if commands::on_this_pc() {
        return;
    }
    let rows = selected_rows();
    let Some((list, paths)) = with_app(|a| {
        (a.view.list, rows.iter().filter_map(|&r| a.view.browser.path_at(r).map(|p| p.0)).collect::<Vec<_>>())
    }) else {
        return;
    };
    if paths.is_empty() {
        return;
    }
    let Ok(data) = dnd::data_object(&paths) else { return };
    // The shell supplies the drag image and cursor feedback; the drop target does the work.
    unsafe {
        let _ = SHDoDragDrop(Some(list), &data, None, DROPEFFECT(dnd::COPY | dnd::MOVE | dnd::LINK));
    }
}

#[implement(IDropTarget)]
pub(super) struct DropTarget {
    list: HWND,
    /// Draws the dragged items' image over our window, like Explorer does.
    helper: Option<IDropTargetHelper>,
    paths: RefCell<Vec<PathBuf>>,
    allowed: Cell<u32>,
}

impl DropTarget {
    pub(super) fn create(list: HWND) -> IDropTarget {
        let helper = unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }.ok();
        DropTarget { list, helper, paths: RefCell::new(Vec::new()), allowed: Cell::new(NONE) }.into()
    }

    /// The effect and destination for the cursor at `pt` (screen coordinates).
    /// Over a folder row, the folder is the destination and gets highlighted.
    fn decide(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL) -> (u32, Option<PathBuf>) {
        super::panes::activate(self.list);
        let mut client = POINT { x: pt.x, y: pt.y };
        let _ = unsafe { ScreenToClient(self.list, &mut client) };
        let mut hit = LVHITTESTINFO { pt: client, ..Default::default() };
        let row = unsafe {
            SendMessageW(self.list, LVM_HITTEST, Some(WPARAM(0)), Some(LPARAM(&mut hit as *mut _ as isize)))
        }
        .0;
        let paths = self.paths.borrow();
        let (row, target) = with_app(|a| {
            let over = usize::try_from(row).ok().and_then(|r| a.view.browser.path_at(r).map(|p| (r, p)));
            match over {
                Some((r, (dir, true))) if !paths.contains(&dir) => (Some(r), Some(dir)),
                _ => (
                    None,
                    a.view.browser.dir().filter(|d| !crate::places::is_this_pc(d)).map(|d| d.to_path_buf()),
                ),
            }
        })
        .unwrap_or((None, None));
        set_drop_row(self.list, row);
        let effect =
            target.as_deref().map_or(NONE, |t| dnd::drop_effect(keys.0, self.allowed.get(), &paths, t));
        (effect, target)
    }
}

/// Highlights the folder row a drop would go into (`None`: no row).
fn set_drop_row(list: HWND, row: Option<usize>) {
    let Some(old) = with_app(|a| std::mem::replace(&mut a.view.drop_row, row)) else { return };
    if old == row {
        return;
    }
    for r in [old, row].into_iter().flatten() {
        unsafe { SendMessageW(list, LVM_REDRAWITEMS, Some(WPARAM(r)), Some(LPARAM(r as isize))) };
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let data = data.ok()?;
        *self.paths.borrow_mut() = dnd::paths_of(data);
        self.allowed.set(unsafe { (*effect).0 });
        let (e, _) = self.decide(keys, pt);
        unsafe {
            *effect = DROPEFFECT(e);
            if let Some(h) = &self.helper {
                let _ = h.DragEnter(self.list, data, &POINT { x: pt.x, y: pt.y }, DROPEFFECT(e));
            }
        }
        Ok(())
    }

    fn DragOver(&self, keys: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let (e, _) = self.decide(keys, pt);
        unsafe {
            *effect = DROPEFFECT(e);
            if let Some(h) = &self.helper {
                let _ = h.DragOver(&POINT { x: pt.x, y: pt.y }, DROPEFFECT(e));
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        set_drop_row(self.list, None);
        self.paths.borrow_mut().clear();
        if let Some(h) = &self.helper {
            unsafe {
                let _ = h.DragLeave();
            }
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let (e, target) = self.decide(keys, pt);
        set_drop_row(self.list, None);
        if let Some(h) = &self.helper
            && let Ok(data) = data.ok()
        {
            unsafe {
                let _ = h.Drop(data, &POINT { x: pt.x, y: pt.y }, DROPEFFECT(e));
            }
        }
        let paths = std::mem::take(&mut *self.paths.borrow_mut());
        if let (Some(dest), true) = (target, e != NONE) {
            commands::transfer(paths, dest, e == MOVE);
        }
        // We move the files ourselves. Reporting "moved" could let some drag
        // sources delete their originals before our copy finishes, so a move
        // is reported as "nothing for you to do".
        unsafe { *effect = DROPEFFECT(if e == MOVE { NONE } else { e }) };
        Ok(())
    }
}
