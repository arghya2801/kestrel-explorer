//! Light and dark appearance. Uses the documented dark title bar attribute,
//! control color messages, and the "DarkMode_*" visual-style names (which
//! fall back to the normal look if Windows ever drops them). Controls with no
//! dark style are painted here: the status bar in dark mode, the tab strip always
//! (except in high contrast).

use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
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
    /// Behind the active pane's path label.
    active: HBRUSH,
}

impl Brushes {
    fn new(p: &Palette) -> Brushes {
        unsafe {
            Brushes {
                chrome: CreateSolidBrush(COLORREF(p.chrome)),
                field: CreateSolidBrush(COLORREF(p.field)),
                active: CreateSolidBrush(COLORREF(active_label(p))),
            }
        }
    }
}

impl Drop for Brushes {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.chrome.0));
            let _ = DeleteObject(HGDIOBJ(self.field.0));
            let _ = DeleteObject(HGDIOBJ(self.active.0));
        }
    }
}

/// A tint of the accent color, so the active pane stands out from the other.
fn active_label(p: &Palette) -> u32 {
    crate::theme::mix(p.window, p.accent, if p.dark { 35 } else { 22 })
}

/// Applies `palette` to the window and its controls.
pub(super) fn apply(hwnd: HWND, palette: Palette) {
    let Some((lists, tree, strip, status, address, filter, buttons)) = with_app(|a| {
        (
            a.views().map(|v| v.list).chain(std::iter::once(a.tab_list)).collect::<Vec<_>>(),
            a.tree,
            a.tab_strip,
            a.status,
            a.address,
            a.filter,
            a.buttons
                .into_iter()
                .chain(a.toolbar.iter().copied())
                .chain(std::iter::once(a.address_edit_button))
                .chain(std::iter::once(a.crumb_more))
                .chain(a.tab_buttons)
                .chain(a.breadcrumbs.iter().map(|p| p.0))
                .collect::<Vec<_>>(),
        )
    }) else {
        return;
    };
    // Settings can pick an accent instead of Windows' own.
    let mut palette = palette;
    let accent = crate::prefs::get().accent;
    if accent > 0 && !crate::theme::high_contrast() {
        palette.accent = crate::prefs::ACCENTS[accent.min(crate::prefs::ACCENTS.len() - 1)].1;
    }
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

        for &list in &lists {
            style_list(list, palette);
        }
        let _ = SetWindowTheme(tree, style(w!("Explorer"), w!("DarkMode_Explorer")), None);
        if let Some(tips) = with_app(|a| a.tips) {
            let _ = SetWindowTheme(tips, style(PCWSTR::null(), w!("DarkMode_Explorer")), None);
        }
        for edit in [address, filter] {
            let _ = SetWindowTheme(edit, style(PCWSTR::null(), w!("DarkMode_CFD")), None);
        }
        for b in buttons {
            let _ = SetWindowTheme(b, style(PCWSTR::null(), w!("DarkMode_Explorer")), None);
        }

        let (bg, fg) = (LPARAM(palette.window as isize), LPARAM(palette.text as isize));
        // -1: the tree's own default colors.
        let (tbg, tfg) = if dark { (bg, fg) } else { (LPARAM(-1), LPARAM(-1)) };
        SendMessageW(tree, TVM_SETBKCOLOR, None, Some(tbg));
        SendMessageW(tree, TVM_SETTEXTCOLOR, None, Some(tfg));

        for c in [strip, status] {
            let _ = SetWindowSubclass(c, Some(paint_dark), 1, 0);
        }
        let _ =
            RedrawWindow(Some(hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_FRAME | RDW_ALLCHILDREN);
    }
}

/// A list's theme, colors and header drawing for `palette`.
fn style_list(list: HWND, palette: Palette) {
    let dark = palette.dark;
    unsafe {
        let _ = SetWindowTheme(list, if dark { w!("DarkMode_Explorer") } else { w!("Explorer") }, None);
        let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as _);
        let _ = SetWindowTheme(header, if dark { w!("DarkMode_ItemsView") } else { PCWSTR::null() }, None);
        let (bg, fg) = (LPARAM(palette.window as isize), LPARAM(palette.text as isize));
        SendMessageW(list, LVM_SETBKCOLOR, None, Some(bg));
        SendMessageW(list, LVM_SETTEXTBKCOLOR, None, Some(bg));
        SendMessageW(list, LVM_SETTEXTCOLOR, None, Some(fg));
        // Column header text: the header tells the list (not us) when it draws.
        let _ = SetWindowSubclass(list, Some(header_text), 2, 0);
    }
}

/// Gives a list made after the window was themed (a new tab's or the second pane's) the
/// current look, without repainting the rest of the window.
pub(super) fn style_new_list(list: HWND) {
    if let Some(palette) = with_app(|a| a.palette) {
        style_list(list, palette);
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

/// Pane path labels (WM_CTLCOLORSTATIC): the active pane's on an accent tint, the other's
/// dimmed. `None` in high contrast: Windows' colors.
pub(super) fn label_colors(hdc: HDC, active: bool) -> Option<LRESULT> {
    let (palette, brushes) = with_app(|a| (a.palette, a.brushes.as_ref().map(|b| (b.active, b.chrome))))?;
    let (active_brush, chrome) = brushes?;
    if crate::theme::high_contrast() {
        return None;
    }
    unsafe {
        let (back, text) = if active {
            (active_label(&palette), palette.text)
        } else {
            (palette.chrome, crate::theme::mix(palette.chrome, palette.text, 60))
        };
        SetTextColor(hdc, COLORREF(text));
        SetBkColor(hdc, COLORREF(back));
    }
    Some(LRESULT(if active { active_brush.0 } else { chrome.0 } as isize))
}

/// Dark mode for a dialog (Settings, the guides): a dark title bar and dark-styled controls.
/// Its page tabs are painted like the window's tab strip; checkbox labels by `checkbox`.
pub(super) fn apply_dialog(hwnd: HWND) {
    let Some(palette) = with_app(|a| a.palette) else { return };
    if !palette.dark || crate::theme::high_contrast() {
        return;
    }
    unsafe {
        let flag = BOOL::from(true);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &flag as *const _ as *const _, 4);
        let mut c = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
        while !c.is_invalid() {
            let mut class = [0u16; 32];
            let n = GetClassNameW(c, &mut class) as usize;
            match String::from_utf16_lossy(&class[..n]).as_str() {
                "Button" => _ = SetWindowTheme(c, w!("DarkMode_Explorer"), None),
                "Edit" if GetWindowLongPtrW(c, GWL_STYLE) as u32 & ES_MULTILINE as u32 != 0 => {
                    _ = SetWindowTheme(c, w!("DarkMode_Explorer"), None)
                }
                "Edit" | "ComboBox" => _ = SetWindowTheme(c, w!("DarkMode_CFD"), None),
                "SysTabControl32" => _ = SetWindowSubclass(c, Some(paint_dark), 1, 0),
                _ => {}
            }
            c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
        }
    }
}

/// A dialog's labels and checkboxes (WM_CTLCOLORSTATIC) on the dialog's own background, in
/// either theme. `None` in high contrast: Windows' colors.
pub(super) fn static_colors(hdc: HDC) -> Option<LRESULT> {
    let (palette, brush) = with_app(|a| (a.palette, a.brushes.as_ref().map(|b| b.chrome)))?;
    if crate::theme::high_contrast() {
        return None;
    }
    unsafe {
        SetTextColor(hdc, COLORREF(palette.text));
        SetBkColor(hdc, COLORREF(palette.chrome));
    }
    Some(LRESULT(brush?.0 as isize))
}

/// Dark mode checkboxes: the theme draws their labels black whatever the text color, so draw
/// the box from the theme and the label ourselves. `None`: let Windows draw it.
pub(super) unsafe fn checkbox(nm: &NMCUSTOMDRAW) -> Option<LRESULT> {
    let palette = with_app(|a| a.palette)?;
    let button = nm.hdr.hwndFrom;
    let style = unsafe { GetWindowLongPtrW(button, GWL_STYLE) } as u32 & BS_TYPEMASK as u32;
    if !palette.dark
        || crate::theme::high_contrast()
        || style != BS_AUTOCHECKBOX as u32
        || nm.dwDrawStage != CDDS_PREPAINT
    {
        return None;
    }
    unsafe {
        let hdc = nm.hdc;
        let back = CreateSolidBrush(COLORREF(palette.chrome));
        FillRect(hdc, &nm.rc, back);
        let _ = DeleteObject(HGDIOBJ(back.0));
        let checked = SendMessageW(button, BM_GETCHECK, None, None).0 == BST_CHECKED.0 as isize;
        let state = nm.uItemState;
        let offset = if state.contains(CDIS_DISABLED) {
            3
        } else if state.contains(CDIS_SELECTED) {
            2
        } else if state.contains(CDIS_HOT) {
            1
        } else {
            0
        };
        let part_state = if checked { CBS_CHECKEDNORMAL.0 } else { CBS_UNCHECKEDNORMAL.0 } + offset;
        let theme = OpenThemeData(Some(button), w!("Button"));
        let size =
            GetThemePartSize(theme, Some(hdc), BP_CHECKBOX.0, part_state, None, TS_TRUE).unwrap_or_default();
        let top = (nm.rc.top + nm.rc.bottom - size.cy) / 2;
        let boxed = RECT { left: nm.rc.left, top, right: nm.rc.left + size.cx, bottom: top + size.cy };
        let _ = DrawThemeBackground(theme, hdc, BP_CHECKBOX.0, part_state, &boxed, None);
        let _ = CloseThemeData(theme);
        let mut text = [0u16; 260];
        let len = GetWindowTextW(button, &mut text) as usize;
        let mut r = RECT { left: boxed.right + super::scale(button, 6), ..nm.rc };
        SetBkMode(hdc, TRANSPARENT);
        let ink = if state.contains(CDIS_DISABLED) {
            crate::theme::mix(palette.chrome, palette.text, 45)
        } else {
            palette.text
        };
        SetTextColor(hdc, COLORREF(ink));
        let hide_prefix =
            if state.contains(CDIS_SHOWKEYBOARDCUES) { DRAW_TEXT_FORMAT(0) } else { DT_HIDEPREFIX };
        DrawTextW(hdc, &mut text[..len], &mut r, DT_LEFT | DT_VCENTER | DT_SINGLELINE | hide_prefix);
        if state.contains(CDIS_FOCUS) && state.contains(CDIS_SHOWKEYBOARDCUES) {
            let mut measured = r;
            DrawTextW(hdc, &mut text[..len], &mut measured, DT_LEFT | DT_SINGLELINE | DT_CALCRECT);
            let h = measured.bottom - measured.top;
            let focus = RECT {
                left: r.left - 1,
                top: (r.top + r.bottom - h) / 2 - 1,
                right: measured.right + 1,
                bottom: (r.top + r.bottom + h) / 2 + 1,
            };
            let _ = DrawFocusRect(hdc, &focus);
        }
    }
    Some(LRESULT(CDRF_SKIPDEFAULT as isize))
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
                    unsafe { paint_header(nm, palette.unwrap()) };
                    LRESULT(CDRF_SKIPDEFAULT as isize)
                }
                _ => LRESULT(CDRF_DODEFAULT as isize),
            };
        }
    }
    unsafe { DefSubclassProc(hwnd, msg, wp, lp) }
}

unsafe fn paint_header(nm: &NMCUSTOMDRAW, palette: Palette) {
    unsafe {
        let mut text = [0u16; 260];
        let mut item = HDITEMW {
            mask: HDI_TEXT | HDI_FORMAT,
            pszText: PWSTR(text.as_mut_ptr()),
            cchTextMax: text.len() as i32,
            ..Default::default()
        };
        SendMessageW(
            nm.hdr.hwndFrom,
            HDM_GETITEMW,
            Some(WPARAM(nm.dwItemSpec)),
            Some(LPARAM(&mut item as *mut _ as isize)),
        );
        if let Some(brush) = with_app(|a| a.brushes.as_ref().map(|b| b.chrome)).flatten() {
            FillRect(nm.hdc, &nm.rc, brush);
        }
        SetTextColor(nm.hdc, COLORREF(palette.text));
        SetBkMode(nm.hdc, TRANSPARENT);
        let padding = super::scale(nm.hdr.hwndFrom, 6);
        let mut rect = RECT { left: nm.rc.left + padding, right: nm.rc.right - padding, ..nm.rc };
        if item.fmt.0 & (HDF_SORTUP.0 | HDF_SORTDOWN.0) != 0 {
            let mut arrow_rect = RECT { left: rect.right - padding * 2, ..rect };
            let mut arrow = [if item.fmt.0 & HDF_SORTUP.0 != 0 { 0x25b4 } else { 0x25be }];
            DrawTextW(nm.hdc, &mut arrow, &mut arrow_rect, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            rect.right = arrow_rect.left - padding;
        }
        let len = text.iter().position(|&c| c == 0).unwrap_or(0);
        let align = if item.fmt.0 & HDF_RIGHT.0 != 0 { DT_RIGHT } else { DT_LEFT };
        DrawTextW(
            nm.hdc,
            &mut text[..len],
            &mut rect,
            align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX,
        );
    }
}

/// Paints the tab strip, and in dark mode the status bar, ourselves.
unsafe extern "system" fn paint_dark(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let palette = crate::crash::guard(|| with_app(|a| (a.palette, a.tab_strip == hwnd))).flatten();
    let Some((palette, _)) =
        palette.filter(|&(p, strip)| if strip { !crate::theme::high_contrast() } else { p.dark })
    else {
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
            // A base line along the bottom; the selected tab opens into the content below it.
            let base = CreateSolidBrush(COLORREF(super::chrome::stroke(&p)));
            let line = RECT { top: client.bottom - 1, ..client };
            FillRect(hdc, &line, base);
            let _ = DeleteObject(HGDIOBJ(base.0));
            let count = SendMessageW(hwnd, TCM_GETITEMCOUNT, None, None).0.max(0) as usize;
            let selected = SendMessageW(hwnd, TCM_GETCURSEL, None, None).0;
            let radius = super::scale(hwnd, 6);
            // The window's tab strip has close buttons and hover; the Settings pages don't.
            let main = with_app(|a| a.tab_strip == hwnd).unwrap_or(false);
            let (hot, close_hot) = if main { super::tabbar::hot() } else { (None, false) };
            let focused = windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == hwnd;
            for i in 0..count {
                let mut r = super::tabbar::tab_rect(hwnd, i);
                let selected = i as isize == selected;
                let back = if selected { p.window } else { p.chrome };
                if selected {
                    let tab = RECT { bottom: r.bottom + radius, ..r };
                    super::chrome::round_outline(hdc, &tab, p.window, super::chrome::stroke(&p), radius);
                    let gap =
                        RECT { left: r.left + 1, top: r.bottom - 1, right: r.right - 1, bottom: r.bottom };
                    let fill = CreateSolidBrush(COLORREF(p.window));
                    FillRect(hdc, &gap, fill);
                    let _ = DeleteObject(HGDIOBJ(fill.0));
                } else if hot == Some(i) {
                    let hover = crate::theme::mix(p.chrome, p.text, 7);
                    let tab = RECT { bottom: r.bottom - super::scale(hwnd, 3), ..r };
                    super::chrome::round_outline(hdc, &tab, hover, hover, radius);
                }
                let ink = if selected { p.text } else { crate::theme::mix(p.chrome, p.text, 70) };
                let mut buf = [0u16; 260];
                let mut item = TCITEMW {
                    mask: TCIF_TEXT,
                    pszText: PWSTR(buf.as_mut_ptr()),
                    cchTextMax: buf.len() as i32,
                    ..Default::default()
                };
                SendMessageW(hwnd, TCM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut item as *mut _ as isize)));
                let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
                let flags = DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS | DT_NOPREFIX;
                if !main {
                    r.left += super::scale(hwnd, 6);
                    r.right -= super::scale(hwnd, 6);
                    SetTextColor(hdc, COLORREF(ink));
                    let centered = (flags & !DT_LEFT) | DT_CENTER;
                    DrawTextW(hdc, &mut buf[..len], &mut r, centered);
                    continue;
                }
                // "folder · parent": the folder in full color, the parent dimmed, and the
                // ellipsis takes the parent first.
                let text = &mut buf[..len];
                let split = text.windows(3).position(|w| w == [0x20, 0xB7, 0x20]).unwrap_or(len);
                let mut area = super::tabbar::text_rect(hwnd, &r);
                let mut size = windows::Win32::Foundation::SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, &text[..split], &mut size);
                let (name, rest) = text.split_at_mut(split);
                let mut name_area = RECT { right: (area.left + size.cx).min(area.right), ..area };
                SetTextColor(hdc, COLORREF(ink));
                DrawTextW(hdc, name, &mut name_area, flags);
                if !rest.is_empty() && name_area.right < area.right {
                    area.left = name_area.right;
                    SetTextColor(
                        hdc,
                        COLORREF(crate::theme::mix(back, p.text, if selected { 55 } else { 45 })),
                    );
                    DrawTextW(hdc, rest, &mut area, flags);
                }
                if super::tabbar::shows_close(hwnd, i) {
                    let mut close = super::tabbar::close_rect(hwnd, &r);
                    let pressing = hot == Some(i) && close_hot;
                    if pressing {
                        let ring = crate::theme::mix(back, p.text, 14);
                        super::chrome::round_outline(hdc, &close, ring, ring, super::scale(hwnd, 4));
                    }
                    let color = if pressing { p.text } else { ink };
                    super::chrome::icon(hdc, hwnd, '\u{E711}', &mut close, color, DT_CENTER);
                }
                if selected && focused {
                    let inset = super::scale(hwnd, 2);
                    let ring = RECT {
                        left: r.left + inset,
                        top: r.top + inset,
                        right: r.right - inset,
                        bottom: r.bottom - inset,
                    };
                    super::chrome::outline(hdc, &ring, p.text, radius - inset, 1);
                }
            }
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

/// In Details view the list theme draws a 2px line beside every column edge, far darker
/// than the rest of the list. Before the rows paint (so selection still covers it), replace
/// each with a faint 1px line. High contrast keeps Windows' lines.
pub(super) unsafe fn column_lines(cd: &NMLVCUSTOMDRAW, p: Palette) -> Option<LRESULT> {
    if crate::theme::high_contrast() || cd.nmcd.dwDrawStage != CDDS_PREPAINT || cd.dwItemType == LVCDI_GROUP {
        return None;
    }
    let (list, hdc) = (cd.nmcd.hdr.hwndFrom, cd.nmcd.hdc);
    unsafe {
        if SendMessageW(list, LVM_GETVIEW, None, None).0 as u32 != LV_VIEW_DETAILS {
            return None;
        }
        let header = HWND(SendMessageW(list, LVM_GETHEADER, None, None).0 as _);
        let count = SendMessageW(header, HDM_GETITEMCOUNT, None, None).0.max(0) as usize;
        let mut client = RECT::default();
        let _ = GetClientRect(list, &mut client);
        let (bg, line) =
            (CreateSolidBrush(COLORREF(p.window)), CreateSolidBrush(COLORREF(super::chrome::faint(&p))));
        for i in 0..count {
            let mut r = RECT::default();
            SendMessageW(header, HDM_GETITEMRECT, Some(WPARAM(i)), Some(LPARAM(&mut r as *mut _ as isize)));
            if r.right <= r.left {
                continue; // a hidden column
            }
            let mut edge = [POINT { x: r.right, y: 0 }];
            MapWindowPoints(Some(header), Some(list), &mut edge);
            let x = edge[0].x;
            FillRect(hdc, &RECT { left: x - 2, right: x + 2, ..client }, bg);
            FillRect(hdc, &RECT { left: x - 1, right: x, ..client }, line);
        }
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let _ = DeleteObject(HGDIOBJ(line.0));
    }
    None
}
