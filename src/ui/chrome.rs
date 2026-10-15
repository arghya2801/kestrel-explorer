//! The top bar's look: flat icon buttons, rounded address and filter fields with an
//! accent line while focused, and toolbar separators. Buttons stay ordinary themed
//! buttons (keyboard, hot tracking, screen readers); NM_CUSTOMDRAW paints them.
//! High contrast keeps Windows' own drawing.

use std::cell::Cell;

use windows::Win32::Foundation::{COLORREF, HWND, LRESULT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{ID_BACK, ID_FORWARD, ID_UP, breadcrumbs, scale, toolbar, with_app};
use crate::theme::{Palette, high_contrast, mix};

const SEARCH: char = '\u{E721}';
/// Space for a glyph from the icon font, before its gap to the label.
const ICON: i32 = 16;
/// Horizontal padding inside a button.
const PAD: i32 = 10;

/// A command's glyph in Windows' icon font (Segoe Fluent Icons / MDL2 Assets).
fn glyph(id: usize) -> Option<char> {
    use super::commands::*;
    Some(match id {
        ID_BACK => '\u{E72B}',
        ID_FORWARD => '\u{E72A}',
        ID_UP => '\u{E74A}',
        breadcrumbs::EDIT => '\u{E70F}',
        CMD_NEW_FOLDER => '\u{E8F4}',
        CMD_CUT => '\u{E8C6}',
        CMD_COPY => '\u{E8C8}',
        CMD_PASTE => '\u{E77F}',
        CMD_RENAME => '\u{E8AC}',
        CMD_DELETE => '\u{E74D}',
        toolbar::REFRESH => '\u{E72C}',
        toolbar::SPLIT => '\u{E89A}',
        toolbar::VIEW => '\u{E8A9}',
        toolbar::MORE => '\u{E712}',
        toolbar::SETTINGS => '\u{E713}',
        toolbar::HELP => '\u{E897}',
        _ => return None,
    })
}

/// Buttons that open a menu get a small arrow after the label.
fn opens_menu(id: usize) -> bool {
    matches!(id, toolbar::VIEW | toolbar::MORE)
}

/// Icon-only buttons: the label is still the window text, for screen readers.
fn icon_only(id: usize) -> bool {
    matches!(id, ID_BACK | ID_FORWARD | ID_UP | breadcrumbs::EDIT)
}

thread_local! {
    /// The icon font and the DPI it was made for.
    static ICONS: Cell<(u32, HFONT)> = const { Cell::new((0, HFONT(std::ptr::null_mut()))) };
}

/// Windows 11's icon font, or Windows 10's, sized for `hwnd`'s DPI.
/// ponytail: one font per thread; windows on monitors with different DPIs re-create it as they paint.
fn icon_font(hwnd: HWND) -> HFONT {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    ICONS.with(|cell| {
        let (at, font) = cell.get();
        if at == dpi && !font.is_invalid() {
            return font;
        }
        if !font.is_invalid() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(font.0));
            }
        }
        let font = ["Segoe Fluent Icons", "Segoe MDL2 Assets"]
            .into_iter()
            .map(|face| unsafe { create_font(face, dpi) })
            .find(|f| !f.is_invalid())
            .unwrap_or_default();
        cell.set((dpi, font));
        font
    })
}

/// The named font, or an invalid handle when Windows would substitute another face.
unsafe fn create_font(face: &str, dpi: u32) -> HFONT {
    unsafe {
        let mut lf = LOGFONTW {
            lfHeight: -(ICON * dpi as i32 / 96),
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: CLEARTYPE_QUALITY,
            ..Default::default()
        };
        for (slot, c) in lf.lfFaceName.iter_mut().zip(face.encode_utf16()) {
            *slot = c;
        }
        let font = CreateFontIndirectW(&lf);
        let dc = GetDC(None);
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let mut got = [0u16; 32];
        let n = GetTextFaceW(dc, Some(&mut got)).max(1) as usize - 1;
        SelectObject(dc, old);
        ReleaseDC(None, dc);
        if String::from_utf16_lossy(&got[..n.min(got.len())]) == face {
            font
        } else {
            let _ = DeleteObject(HGDIOBJ(font.0));
            HFONT::default()
        }
    }
}

fn text_of(hwnd: HWND) -> Vec<u16> {
    let mut buf = [0u16; 260];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) }.max(0) as usize;
    buf[..n].to_vec()
}

/// Measures with the window's UI font; `f` gets a DC with it selected.
fn measure<R>(hwnd: HWND, f: impl FnOnce(HDC) -> R) -> R {
    let font = with_app(|a| a.font).unwrap_or_default();
    unsafe {
        let dc = GetDC(Some(hwnd));
        let old = SelectObject(dc, HGDIOBJ(font.0));
        let r = f(dc);
        SelectObject(dc, old);
        ReleaseDC(Some(hwnd), dc);
        r
    }
}

fn width_of(dc: HDC, text: &[u16]) -> i32 {
    let mut size = SIZE::default();
    unsafe {
        let _ = GetTextExtentPoint32W(dc, text, &mut size);
    }
    size.cx
}

/// The UI font's line height, for centering text boxes in their fields.
pub(super) fn line_height(hwnd: HWND) -> i32 {
    measure(hwnd, |dc| {
        let mut tm = TEXTMETRICW::default();
        unsafe {
            let _ = GetTextMetricsW(dc, &mut tm);
        }
        tm.tmHeight
    })
}

/// The width a toolbar button needs for its icon and label.
pub(super) fn button_width(parent: HWND, button: HWND, id: usize) -> i32 {
    let text = text_of(button);
    let arrow = if opens_menu(id) { "\u{25BE}".encode_utf16().collect() } else { Vec::new() };
    let (label, arrow) = measure(parent, |dc| (width_of(dc, &text), width_of(dc, &arrow)));
    let icon = if glyph(id).is_some() { scale(parent, ICON + 8) } else { 0 };
    let arrow = if arrow > 0 { arrow + scale(parent, 6) } else { 0 };
    scale(parent, PAD) * 2 + icon + label + arrow
}

/// The width a breadcrumb needs; all but the last carry a separator.
pub(super) fn crumb_width(parent: HWND, button: HWND, last: bool) -> i32 {
    let text = text_of(button);
    let sep: Vec<u16> = "\u{203A}".encode_utf16().collect();
    let (label, sep) = measure(parent, |dc| (width_of(dc, &text), width_of(dc, &sep)));
    scale(parent, 8) * 2 + label + if last { 0 } else { sep + scale(parent, 8) }
}

/// Whether `hwnd` is one of the top bar's buttons.
pub(super) fn is_top_button(hwnd: HWND) -> bool {
    with_app(|a| {
        a.buttons.contains(&hwnd)
            || a.toolbar.contains(&hwnd)
            || a.address_edit_button == hwnd
            || a.breadcrumbs.iter().any(|p| p.0 == hwnd)
    }) == Some(true)
}

/// Fills `r` with rounded corners.
unsafe fn round(hdc: HDC, r: &RECT, color: u32, radius: i32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
        let (ob, op) = (SelectObject(hdc, HGDIOBJ(brush.0)), SelectObject(hdc, HGDIOBJ(pen.0)));
        let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

/// A rounded shape filled with `fill` and edged with a 1-pixel `edge`.
pub(super) unsafe fn round_outline(hdc: HDC, r: &RECT, fill: u32, edge: u32, radius: i32) {
    unsafe {
        round(hdc, r, edge, radius);
        let inner = RECT { left: r.left + 1, top: r.top + 1, right: r.right - 1, bottom: r.bottom - 1 };
        round(hdc, &inner, fill, (radius - 1).max(0));
    }
}

/// A rounded outline of `width` pixels.
unsafe fn outline(hdc: HDC, r: &RECT, color: u32, radius: i32, width: i32) {
    unsafe {
        let pen = CreatePen(PS_SOLID, width, COLORREF(color));
        let (ob, op) = (SelectObject(hdc, GetStockObject(NULL_BRUSH)), SelectObject(hdc, HGDIOBJ(pen.0)));
        let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
        SelectObject(hdc, ob);
        SelectObject(hdc, op);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }
}

unsafe fn fill(hdc: HDC, r: &RECT, color: u32) {
    unsafe {
        let brush = CreateSolidBrush(COLORREF(color));
        FillRect(hdc, r, brush);
        let _ = DeleteObject(HGDIOBJ(brush.0));
    }
}

unsafe fn text(hdc: HDC, s: &[u16], r: &mut RECT, color: u32, format: DRAW_TEXT_FORMAT) {
    unsafe {
        SetTextColor(hdc, COLORREF(color));
        let mut s = s.to_vec();
        DrawTextW(hdc, &mut s, r, format | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX);
    }
}

unsafe fn icon(hdc: HDC, hwnd: HWND, c: char, r: &mut RECT, color: u32, format: DRAW_TEXT_FORMAT) {
    let mut buf = [0u16; 2];
    let font = icon_font(hwnd);
    if font.is_invalid() {
        return;
    }
    unsafe {
        let old = SelectObject(hdc, HGDIOBJ(font.0));
        text(hdc, c.encode_utf16(&mut buf), r, color, format);
        SelectObject(hdc, old);
    }
}

/// Paints a top-bar button. `None`: let Windows draw it.
pub(super) unsafe fn draw_button(nm: &NMCUSTOMDRAW) -> Option<LRESULT> {
    if nm.dwDrawStage != CDDS_PREPAINT || high_contrast() {
        return None;
    }
    let hwnd = nm.hdr.hwndFrom;
    let id = unsafe { GetDlgCtrlID(hwnd) } as usize;
    let (p, toggled, crumb, last_crumb) = with_app(|a| {
        let at = a.breadcrumbs.iter().position(|c| c.0 == hwnd);
        (
            a.palette,
            id == toolbar::SPLIT && a.split_view,
            at,
            at.is_some_and(|i| i + 1 == a.breadcrumbs.len()),
        )
    })?;
    let in_field = crumb.is_some() || id == breadcrumbs::EDIT;
    let base = if in_field { p.field } else { p.chrome };
    let state = nm.uItemState;
    let disabled = state.contains(CDIS_DISABLED);
    let fill_color = if disabled {
        None
    } else if state.contains(CDIS_SELECTED) {
        Some(mix(base, p.text, 16))
    } else if toggled {
        Some(mix(base, p.text, 12))
    } else if state.contains(CDIS_HOT) {
        Some(mix(base, p.text, 8))
    } else {
        None
    };
    let ink = if disabled { mix(base, p.text, 38) } else { p.text };
    let quiet = if disabled { ink } else { mix(base, p.text, 70) };
    let hdc = nm.hdc;
    let radius = scale(hwnd, 4);
    unsafe {
        fill(hdc, &nm.rc, base);
        if let Some(c) = fill_color {
            round(hdc, &nm.rc, c, radius);
        }
        SetBkMode(hdc, TRANSPARENT);
        let label = text_of(hwnd);
        let mut r = RECT { left: nm.rc.left + scale(hwnd, if in_field { 8 } else { PAD }), ..nm.rc };
        if icon_only(id) {
            let mut r = nm.rc;
            let color = if id == breadcrumbs::EDIT { quiet } else { ink };
            icon(hdc, hwnd, glyph(id).unwrap_or(' '), &mut r, color, DT_CENTER);
        } else if crumb.is_some() {
            let color = if last_crumb || fill_color.is_some() { ink } else { quiet };
            text(hdc, &label, &mut r, color, DT_LEFT | DT_END_ELLIPSIS);
            if !last_crumb {
                let mut s = RECT { right: nm.rc.right - scale(hwnd, 8), ..nm.rc };
                text(hdc, &"\u{203A}".encode_utf16().collect::<Vec<_>>(), &mut s, quiet, DT_RIGHT);
            }
        } else {
            if let Some(g) = glyph(id) {
                let mut g_rect = RECT { right: r.left + scale(hwnd, ICON), ..r };
                icon(hdc, hwnd, g, &mut g_rect, ink, DT_CENTER);
                r.left += scale(hwnd, ICON + 8);
            }
            text(hdc, &label, &mut r, ink, DT_LEFT);
            if opens_menu(id) {
                let mut a = RECT { right: nm.rc.right - scale(hwnd, PAD), ..nm.rc };
                text(hdc, &"\u{25BE}".encode_utf16().collect::<Vec<_>>(), &mut a, quiet, DT_RIGHT);
            }
        }
        if state.contains(CDIS_FOCUS) && state.contains(CDIS_SHOWKEYBOARDCUES) {
            outline(hdc, &nm.rc, p.text, radius, scale(hwnd, 2).max(1));
        }
    }
    Some(LRESULT(CDRF_SKIPDEFAULT as isize))
}

/// The address and filter fields and the toolbar separators, painted on the window
/// background around their child controls.
pub(super) fn paint(hwnd: HWND, hdc: HDC) {
    let Some((p, fields, focused)) = with_app(|a| {
        let focus = unsafe { GetFocus() };
        (a.palette, a.fields, [focus == a.address, focus == a.filter])
    }) else {
        return;
    };
    let contrast = high_contrast();
    let stroke = if contrast { p.text } else { stroke(&p) };
    let radius = scale(hwnd, 4);
    unsafe {
        for (field, focused) in fields.iter().zip(focused) {
            if field.right <= field.left {
                continue;
            }
            round_outline(hdc, field, p.field, stroke, radius);
            if focused {
                // WinUI's focused text box: a thicker accent line along the bottom.
                let line = scale(hwnd, 2);
                let r = RECT {
                    left: field.left + radius,
                    top: field.bottom - line,
                    right: field.right - radius,
                    bottom: field.bottom,
                };
                fill(hdc, &r, p.accent);
            }
        }
        let filter = fields[1];
        if filter.right > filter.left {
            SetBkMode(hdc, TRANSPARENT);
            let mut r = RECT {
                left: filter.left + scale(hwnd, 8),
                right: filter.left + scale(hwnd, 8 + ICON),
                ..filter
            };
            icon(hdc, hwnd, SEARCH, &mut r, mix(p.field, p.text, 60), DT_CENTER);
        }
        for r in toolbar::separators(hwnd) {
            fill(hdc, &r, stroke);
        }
    }
}

/// Hairlines: field borders, separators, the tab strip's base line.
pub(super) fn stroke(p: &Palette) -> u32 {
    mix(p.chrome, p.text, if p.dark { 16 } else { 14 })
}

/// Where the filter's text starts, after its search icon.
pub(super) fn filter_inset(hwnd: HWND) -> i32 {
    scale(hwnd, 8 + ICON + 6)
}
