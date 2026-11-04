//! Clickable ancestors in the address row; Ctrl+L keeps the ordinary editable path.
use super::{child, with_app};
use windows::Win32::{
    Foundation::{HWND, RECT},
    UI::WindowsAndMessaging::*,
};
use windows::core::{HSTRING, w};
pub(super) const EDIT: usize = 399;
/// "…": the crumbs left out of a long path.
pub(super) const MORE: usize = 398;
const FIRST: usize = 400;
pub(super) fn owns(hwnd: HWND) -> bool {
    with_app(|a| a.crumb_more == hwnd || a.breadcrumbs.iter().any(|(h, _)| *h == hwnd)) == Some(true)
}
pub(super) fn execute(id: usize) -> bool {
    if id == EDIT {
        super::focus_address();
        return true;
    }
    if id == MORE {
        more_menu();
        return true;
    }
    if let Some(path) =
        id.checked_sub(FIRST).and_then(|i| with_app(|a| a.breadcrumbs.get(i).map(|p| p.1.clone())).flatten())
    {
        super::go(path);
        return true;
    }
    false
}
pub(super) fn update() {
    let Some((hwnd, font, old, paths)) = with_app(|a| {
        let dir = a.view.browser.dir()?.to_path_buf();
        if a.breadcrumbs.last().is_some_and(|p| p.1 == dir) {
            return None;
        }
        let mut paths = vec![dir];
        while let Some(parent) = paths.last().and_then(|p| crate::network::up_from(p)) {
            paths.push(parent);
        }
        paths.reverse();
        // Deep paths keep their drive rather than This PC.
        if paths.len() > 5 && paths.first().is_some_and(|p| crate::places::is_this_pc(p)) {
            paths.remove(0);
        }
        a.crumbs_drained =
            if paths.len() > 5 { paths.drain(1..paths.len() - 4).collect() } else { Vec::new() };
        Some((a.hwnd, a.font, std::mem::take(&mut a.breadcrumbs), paths))
    })
    .flatten() else {
        return;
    };
    for (control, _) in old {
        unsafe {
            let _ = DestroyWindow(control);
        }
    }
    let mut controls = Vec::new();
    for (i, path) in paths.into_iter().enumerate() {
        let text = path
            .file_name()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        match unsafe {
            child(
                hwnd,
                w!("BUTTON"),
                &HSTRING::from(text.replace('&', "&&")),
                WS_TABSTOP,
                WINDOW_EX_STYLE(0),
                FIRST + i,
            )
        } {
            Ok(button) => {
                super::set_font(&[button], font);
                let dark = with_app(|a| a.palette.dark).unwrap_or(false);
                unsafe {
                    let _ = windows::Win32::UI::Controls::SetWindowTheme(
                        button,
                        if dark { w!("DarkMode_Explorer") } else { w!("Explorer") },
                        None,
                    );
                }
                controls.push((button, path));
            }
            Err(e) => {
                super::set_status(&format!("Couldn't create address button: {e}"));
                break;
            }
        }
    }
    with_app(|a| a.breadcrumbs = controls);
    super::layout(hwnd);
}
pub(super) fn layout() {
    let Some((hwnd, address, edit, editing, buttons, field, more, drained)) = with_app(|a| {
        (
            a.hwnd,
            a.address,
            a.address_edit_button,
            a.address_editing,
            a.breadcrumbs.iter().map(|p| p.0).collect::<Vec<_>>(),
            a.fields[0],
            a.crumb_more,
            !a.crumbs_drained.is_empty(),
        )
    }) else {
        return;
    };
    // Crumbs and the edit button sit inside the address field, inset by `gap`.
    let gap = super::scale(hwnd, 2);
    let r = RECT {
        left: field.left + gap,
        top: field.top + gap,
        right: field.right - gap,
        bottom: field.bottom - gap,
    };
    let edit_width = super::scale(hwnd, 32).min(r.right - r.left);
    let available = (r.right - r.left - edit_width).max(0);
    let last = buttons.len().saturating_sub(1);
    let widths: Vec<i32> =
        buttons.iter().enumerate().map(|(i, &b)| super::chrome::crumb_width(hwnd, b, i == last)).collect();
    // The nearest ancestors that fit; the current folder always shows, squeezed if need be.
    let fit = |available: i32| {
        let mut first = buttons.len();
        let mut used = 0;
        while first > 0 && (first == buttons.len() || used + widths[first - 1] <= available) {
            first -= 1;
            used += widths[first];
        }
        first
    };
    // "…" stands for the folders left out: the middle of a deep path, or what doesn't fit.
    let more_width = super::chrome::crumb_width(hwnd, more, false);
    let mut first = fit(available);
    let show_more = drained || first > 0;
    if show_more {
        first = fit(available - more_width);
    }
    with_app(|a| a.crumbs_first = first);
    unsafe {
        let _ = ShowWindow(address, if editing || buttons.is_empty() { SW_SHOWNA } else { SW_HIDE });
        let _ = ShowWindow(edit, if editing || buttons.is_empty() { SW_HIDE } else { SW_SHOWNA });
        let _ = ShowWindow(more, if show_more && !editing { SW_SHOWNA } else { SW_HIDE });
        let _ = MoveWindow(edit, r.right - edit_width, r.top, edit_width, r.bottom - r.top, true);
        let mut x = r.left;
        let mut place = |button: HWND, width: i32| {
            let width = width.min(r.left + available - x).max(0);
            let _ = MoveWindow(button, x, r.top, width, r.bottom - r.top, true);
            x += width;
        };
        for (i, button) in buttons.into_iter().enumerate() {
            let show = !editing && i >= first;
            let _ = ShowWindow(button, if show { SW_SHOWNA } else { SW_HIDE });
            if !show {
                continue;
            }
            if i == first && first > 0 {
                place(more, more_width);
            }
            place(button, widths[i]);
            if i == 0 && drained {
                place(more, more_width);
            }
        }
    }
}

/// The folders "…" stands for, from the root down.
fn hidden() -> Vec<std::path::PathBuf> {
    with_app(|a| {
        let first = a.crumbs_first.min(a.breadcrumbs.len());
        let mut out = Vec::new();
        if first > 0 {
            out.push(a.breadcrumbs[0].1.clone());
        }
        out.extend(a.crumbs_drained.iter().cloned());
        out.extend(a.breadcrumbs.iter().take(first).skip(1).map(|c| c.1.clone()));
        out
    })
    .unwrap_or_default()
}

/// The menu under "…": each folder it stands for.
fn more_menu() {
    let Some((hwnd, more)) = with_app(|a| (a.hwnd, a.crumb_more)) else { return };
    let paths = hidden();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return };
        for (i, path) in paths.iter().enumerate() {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            let _ = AppendMenuW(menu, MF_STRING, i + 1, &HSTRING::from(name.replace('&', "&&")));
        }
        let mut r = RECT::default();
        let _ = GetWindowRect(more, &mut r);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, r.left, r.bottom, None, hwnd, None).0;
        let _ = DestroyMenu(menu);
        if let Some(path) = (cmd as usize).checked_sub(1).and_then(|i| paths.get(i)) {
            super::go(path.clone());
        }
    }
}
