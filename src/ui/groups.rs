//! Group headers on the virtual file list. Windows documents groups only for lists that
//! hold their own items; Explorer's virtual lists use the undocumented IOwnerDataCallback
//! (COMCTL32 6.10+, registered with message 0x10BB), which maps rows to groups. Rows stay
//! display rows, so selection, rename and drag need no changes.

use std::cell::RefCell;

use crate::theme::{Palette, high_contrast, mix};

use windows::Win32::Foundation::{
    COLORREF, E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, LRESULT, POINT, RECT, S_OK, WPARAM,
};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_GETFONT};
use windows::core::{
    ComObject, HRESULT, HSTRING, IUnknown, IUnknown_Vtbl, Interface, PWSTR, implement, interface,
};

const LVM_SETOWNERDATACALLBACK: u32 = 0x10BB;

#[interface("44C09D56-8D3B-419D-A462-7B956B105B47")]
unsafe trait IOwnerDataCallback: IUnknown {
    fn get_item_position(&self, item: i32, position: *mut POINT) -> HRESULT;
    fn set_item_position(&self, item: i32, position: POINT) -> HRESULT;
    fn get_item_in_group(&self, group: i32, index: i32, item: *mut i32) -> HRESULT;
    fn get_item_group(&self, item: i32, occurrence: i32, group: *mut i32) -> HRESULT;
    fn get_item_group_count(&self, item: i32, count: *mut i32) -> HRESULT;
    fn on_cache_hint(&self, first: LVITEMINDEX, last: LVITEMINDEX) -> HRESULT;
}

/// Each group's rows, and each row's group. Group ids are their indices.
#[implement(IOwnerDataCallback)]
#[derive(Default)]
pub(super) struct Groups {
    labels: RefCell<Vec<&'static str>>,
    rows: RefCell<Vec<Vec<u32>>>,
    group_of: RefCell<Vec<u32>>,
}

impl IOwnerDataCallback_Impl for Groups_Impl {
    unsafe fn get_item_position(&self, _: i32, _: *mut POINT) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn set_item_position(&self, _: i32, _: POINT) -> HRESULT {
        E_NOTIMPL
    }
    unsafe fn get_item_in_group(&self, group: i32, index: i32, item: *mut i32) -> HRESULT {
        let rows = self.rows.borrow();
        let Some(&row) = rows.get(group as usize).and_then(|r| r.get(index as usize)) else {
            return E_INVALIDARG;
        };
        unsafe { *item = row as i32 };
        S_OK
    }
    unsafe fn get_item_group(&self, item: i32, _: i32, group: *mut i32) -> HRESULT {
        let Some(&g) = self.group_of.borrow().get(item as usize) else { return E_INVALIDARG };
        unsafe { *group = g as i32 };
        S_OK
    }
    unsafe fn get_item_group_count(&self, _: i32, count: *mut i32) -> HRESULT {
        unsafe { *count = 1 };
        S_OK
    }
    unsafe fn on_cache_hint(&self, _: LVITEMINDEX, _: LVITEMINDEX) -> HRESULT {
        S_OK
    }
}

/// Gives `list` its group callback. Call once, after creating the list.
pub(super) fn attach(list: HWND) -> ComObject<Groups> {
    let groups = ComObject::new(Groups::default());
    let callback: IOwnerDataCallback = groups.to_interface();
    unsafe {
        SendMessageW(list, LVM_SETOWNERDATACALLBACK, Some(WPARAM(callback.as_raw() as usize)), None);
    }
    groups
}

/// Shows `sections` (label and rows, newest first) as headed groups of `list`, or plain rows
/// for `None`. Call before setting the item count.
pub(super) fn show(list: HWND, groups: &Groups, sections: Option<Vec<(&'static str, Vec<u32>)>>) {
    let Some(sections) = sections else {
        groups.rows.borrow_mut().clear();
        unsafe { SendMessageW(list, LVM_ENABLEGROUPVIEW, Some(WPARAM(0)), None) };
        return;
    };
    let mut group_of = vec![0; sections.iter().map(|s| s.1.len()).sum()];
    for (g, (_, rows)) in sections.iter().enumerate() {
        for &row in rows {
            group_of[row as usize] = g as u32;
        }
    }
    let labels: Vec<HSTRING> = sections.iter().map(|s| HSTRING::from(s.0)).collect();
    let counts: Vec<usize> = sections.iter().map(|s| s.1.len()).collect();
    // The list asks for rows as soon as groups exist: the mapping goes first.
    *groups.labels.borrow_mut() = sections.iter().map(|s| s.0).collect();
    *groups.rows.borrow_mut() = sections.into_iter().map(|s| s.1).collect();
    *groups.group_of.borrow_mut() = group_of;
    unsafe { SendMessageW(list, LVM_REMOVEALLGROUPS, None, None) };
    for (g, (header, count)) in labels.iter().zip(counts).enumerate() {
        let group = LVGROUP {
            cbSize: size_of::<LVGROUP>() as u32,
            mask: LVGF_HEADER | LVGF_GROUPID | LVGF_ITEMS,
            pszHeader: PWSTR(header.as_ptr() as *mut _),
            iGroupId: g as i32,
            cItems: count as u32,
            ..Default::default()
        };
        unsafe {
            SendMessageW(
                list,
                LVM_INSERTGROUP,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(&group as *const _ as isize)),
            )
        };
    }
    unsafe { SendMessageW(list, LVM_ENABLEGROUPVIEW, Some(WPARAM(1)), None) };
}

/// In dark mode the themed header text is dark blue on dark grey: paint the label and its
/// rule in the palette's colors instead. Light mode and high contrast keep Windows' drawing.
pub(super) unsafe fn draw_header(cd: &NMLVCUSTOMDRAW, groups: &Groups, p: Palette) -> Option<LRESULT> {
    if !p.dark || high_contrast() || cd.dwItemType != LVCDI_GROUP || cd.nmcd.dwDrawStage != CDDS_PREPAINT {
        return None;
    }
    let mut label: Vec<u16> = groups.labels.borrow().get(cd.nmcd.dwItemSpec)?.encode_utf16().collect();
    // `rc` spans the whole group; `rcText` is its header row.
    let (hdc, list) = (cd.nmcd.hdc, cd.nmcd.hdr.hwndFrom);
    let r = RECT { left: cd.nmcd.rc.left, right: cd.nmcd.rc.right, ..cd.rcText };
    unsafe {
        let font = SendMessageW(list, WM_GETFONT, None, None).0;
        let old = SelectObject(hdc, HGDIOBJ(font as _));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(mix(p.window, p.text, 80)));
        let mut t = RECT { left: r.left + super::scale(list, 10), ..r };
        let format = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX;
        DrawTextW(hdc, &mut label, &mut t, format | DT_CALCRECT);
        t.top = r.top;
        t.bottom = r.bottom;
        DrawTextW(hdc, &mut label, &mut t, format);
        let mid = (r.top + r.bottom) / 2;
        let gap = super::scale(list, 8);
        let rule = RECT { left: t.right + gap, top: mid, right: r.right - gap, bottom: mid + 1 };
        let brush = CreateSolidBrush(COLORREF(super::chrome::stroke(&p)));
        FillRect(hdc, &rule, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        SelectObject(hdc, old);
    }
    Some(LRESULT(CDRF_SKIPDEFAULT as isize))
}
