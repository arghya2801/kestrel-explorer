//! Clickable ancestors in the address row; Ctrl+L keeps the ordinary editable path.
use super::{child, with_app};
use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};
use windows::core::{HSTRING, w};
pub(super) const EDIT: usize = 399;
const FIRST: usize = 400;
pub(super) fn owns(hwnd: HWND) -> bool {
    with_app(|a| a.breadcrumbs.iter().any(|(h, _)| *h == hwnd)) == Some(true)
}
pub(super) fn execute(id: usize) -> bool {
    if id == EDIT {
        super::focus_address();
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
        if paths.len() > 5 {
            paths.drain(1..paths.len() - 4);
        }
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
                &HSTRING::from(format!("{} ›", text.replace('&', "&&"))),
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
    let Some((hwnd, address, edit, editing, buttons)) = with_app(|a| {
        (
            a.hwnd,
            a.address,
            a.address_edit_button,
            a.address_editing,
            a.breadcrumbs.iter().map(|p| p.0).collect::<Vec<_>>(),
        )
    }) else {
        return;
    };
    let r = super::client_rect_of(hwnd, address);
    let edit_width = super::scale(hwnd, 48).min(r.right - r.left);
    let available = (r.right - r.left - edit_width).max(0);
    let count = buttons.len().min((available / super::scale(hwnd, 64).max(1)) as usize);
    let first = buttons.len().saturating_sub(count);
    unsafe {
        let _ = ShowWindow(address, if editing || buttons.is_empty() { SW_SHOWNA } else { SW_HIDE });
        let _ = ShowWindow(edit, if editing || buttons.is_empty() { SW_HIDE } else { SW_SHOWNA });
        let _ = MoveWindow(edit, r.right - edit_width, r.top, edit_width, r.bottom - r.top, true);
        for (i, button) in buttons.into_iter().enumerate() {
            let show = !editing && i >= first && count > 0;
            let _ = ShowWindow(button, if show { SW_SHOWNA } else { SW_HIDE });
            if show {
                let width = available / count as i32;
                let _ = MoveWindow(
                    button,
                    r.left + (i - first) as i32 * width,
                    r.top,
                    width,
                    r.bottom - r.top,
                    true,
                );
            }
        }
    }
}
