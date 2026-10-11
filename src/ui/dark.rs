//! Light and dark appearance. Uses the documented dark title bar attribute,
//! control color messages, and the "DarkMode_*" visual-style names (which
//! fall back to the normal look if Windows ever drops them). Controls with no
//! dark style (tab strip, status bar) are painted here in dark mode.

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{BOOL, PCWSTR, PWSTR, w};

use super::with_app;
use crate::theme::Palette;

/// Brushes for the current palette, deleted when replaced.
pub(super) struct Brushes {
    pub(super) chrome: HBRUSH,
    pub(super) field: HBRUSH,
}

impl Brushes {
    fn new(p: &Palette) -> Brushes {
        unsafe {
            Brushes {
                chrome: CreateSolidBrush(COLORREF(p.chrome)),
                field: CreateSolidBrush(COLORREF(p.field)),
            }
        }
    }
}

impl Drop for Brushes {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.chrome.0));
            let _ = DeleteObject(HGDIOBJ(self.field.0));
        }
    }
}

/// Applies `palette` to the window and its controls.
pub(super) fn apply(hwnd: HWND, palette: Palette) {
    let Some((list, tree, strip, status, address, filter, buttons)) = with_app(|a| {
        (
            a.list,
            a.tree,
            a.tab_strip,
            a.status,
            a.address,
            a.filter,
            a.buttons.into_iter().chain(a.toolbar.iter().copied()).collect::<Vec<_>>(),
        )
    }) else {
        return;
    };
    let brushes = Brushes::new(&palette);
    with_app(|a| {
        a.palette = palette;
        a.brushes = Some(brushes);
    });
    let dark = palette.dark;
    let style = |light: PCWSTR, dark_name: PCWSTR| if dark { dark_name } else { light };
    unsafe {
        let flag = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &flag as *const _ as *const _, 4);

        let _ = SetWindowTheme(list, style(w!("Explorer"), w!("DarkMode_Explorer")), None);
        let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as _);
        let _ = SetWindowTheme(header, style(PCWSTR::null(), w!("DarkMode_ItemsView")), None);
        let _ = SetWindowTheme(tree, style(w!("Explorer"), w!("DarkMode_Explorer")), None);
        for edit in [address, filter] {
            let _ = SetWindowTheme(edit, style(PCWSTR::null(), w!("DarkMode_CFD")), None);
        }
        for b in buttons {
            let _ = SetWindowTheme(b, style(PCWSTR::null(), w!("DarkMode_Explorer")), None);
        }

        let (bg, fg) = (LPARAM(palette.window as isize), LPARAM(palette.text as isize));
        SendMessageW(list, LVM_SETBKCOLOR, None, Some(bg));
        SendMessageW(list, LVM_SETTEXTBKCOLOR, None, Some(bg));
        SendMessageW(list, LVM_SETTEXTCOLOR, None, Some(fg));
        // -1: the tree's own default colors.
        let (tbg, tfg) = if dark { (bg, fg) } else { (LPARAM(-1), LPARAM(-1)) };
        SendMessageW(tree, TVM_SETBKCOLOR, None, Some(tbg));
        SendMessageW(tree, TVM_SETTEXTCOLOR, None, Some(tfg));

        for c in [strip, status] {
            let _ = SetWindowSubclass(c, Some(paint_dark), 1, 0);
        }
        // Column header text: the header tells the list (not us) when it draws.
        let _ = SetWindowSubclass(list, Some(header_text), 2, 0);
        let _ =
            RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
    }
}

/// Text box colors (WM_CTLCOLOREDIT). `None` in light mode: Windows' defaults.
pub(super) fn edit_colors(hdc: HDC) -> Option<LRESULT> {
    let (palette, brush) = with_app(|a| (a.palette, a.brushes.as_ref().map(|b| b.field)))?;
    if !palette.dark {
        return None;
    }
    unsafe {
        SetTextColor(hdc, COLORREF(palette.text));
        SetBkColor(hdc, COLORREF(palette.field));
    }
    Some(LRESULT(brush?.0 as isize))
}

/// Main window background (WM_ERASEBKGND).
pub(super) fn erase(hwnd: HWND, hdc: HDC) -> bool {
    let Some(brush) = with_app(|a| a.brushes.as_ref().map(|b| b.chrome)).flatten() else { return false };
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
        FillRect(hdc, &r, brush);
    }
    true
}

/// Light header text in dark mode.
unsafe extern "system" fn header_text(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_NOTIFY && lp.0 != 0 {
        let nm = unsafe { &*(lp.0 as *const NMCUSTOMDRAW) };
        let header = unsafe { SendMessageW(hwnd, LVM_GETHEADER, None, None) }.0;
        let palette = crate::crash::guard(|| with_app(|a| a.palette)).flatten();
        if nm.hdr.code == NM_CUSTOMDRAW
            && nm.hdr.hwndFrom.0 as isize == header
            && palette.is_some_and(|p| p.dark)
        {
            return match nm.dwDrawStage {
                CDDS_PREPAINT => LRESULT(CDRF_NOTIFYITEMDRAW as isize),
                CDDS_ITEMPREPAINT => {
                    unsafe { SetTextColor(nm.hdc, COLORREF(palette.unwrap().text)) };
                    LRESULT(CDRF_DODEFAULT as isize)
                }
                _ => LRESULT(CDRF_DODEFAULT as isize),
            };
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

/// Paints the tab strip and status bar ourselves in dark mode; light mode is untouched.
unsafe extern "system" fn paint_dark(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let palette = crate::crash::guard(|| with_app(|a| a.palette)).flatten();
    let Some(palette) = palette.filter(|p| p.dark) else {
        return unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
    };
    match msg {
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            crate::crash::guard(|| unsafe { paint(hwnd, palette) });
            LRESULT(0)
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wp, lp) },
    }
}

unsafe fn paint(hwnd: HWND, p: Palette) {
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);
        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let chrome = CreateSolidBrush(COLORREF(p.chrome));
        FillRect(hdc, &client, chrome);
        let font = HFONT(SendMessageW(hwnd, WM_GETFONT, None, None).0 as _);
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(p.text));

        let mut class = [0u16; 32];
        let n = GetClassNameW(hwnd, &mut class) as usize;
        if String::from_utf16_lossy(&class[..n]) == "SysTabControl32" {
            let field = CreateSolidBrush(COLORREF(p.field));
            let count = SendMessageW(hwnd, TCM_GETITEMCOUNT, None, None).0.max(0) as usize;
            let selected = SendMessageW(hwnd, TCM_GETCURSEL, None, None).0;
            for i in 0..count {
                let mut r = RECT::default();
                SendMessageW(hwnd, TCM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize)));
                if i as isize == selected {
                    FillRect(hdc, &r, field);
                }
                let mut buf = [0u16; 260];
                let mut item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: PWSTR(buf.as_mut_ptr()),
                    cchTextMax: buf.len() as i32,
                    ..Default::default()
                };
                SendMessageW(hwnd, TCM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut item as *mut _ as isize)));
                let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
                DrawTextW(
                    hdc,
                    &mut buf[..len],
                    &mut r,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS,
                );
            }
            let _ = DeleteObject(HGDIOBJ(field.0));
        } else {
            // Status bar: one text part.
            let mut buf = [0u16; 1024];
            SendMessageW(hwnd, SB_GETTEXTW, Some(WPARAM(0)), Some(LPARAM(buf.as_mut_ptr() as isize)));
            let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
            let mut r = RECT { left: client.left + 6, ..client };
            DrawTextW(hdc, &mut buf[..len], &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
        }
        SelectObject(hdc, old_font);
        let _ = DeleteObject(HGDIOBJ(chrome.0));
        let _ = EndPaint(hwnd, &ps);
    }
}
