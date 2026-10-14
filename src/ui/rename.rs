//! Batch rename (F2 with several items selected): find/replace in the names, with a live preview.
use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{DeleteObject, HDC, HFONT, HGDIOBJ};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

use super::{child, dark, scale, set_font, window_text};
use crate::ops::{Op, batch_names};

const FIND: usize = 10;
const REPLACE: usize = 11;
const PREVIEW: usize = 12;
const HINT: usize = 13;
const FIND_LABEL: usize = 20;
const REPLACE_LABEL: usize = 21;

struct State {
    paths: Vec<PathBuf>,
    names: Vec<String>,
    font: HFONT,
}

/// Opens the batch rename window for `paths` (two or more items).
pub(super) fn open(owner: HWND, paths: Vec<PathBuf>) {
    if let Err(e) = unsafe { create(owner, paths) } {
        super::set_status(&format!("Couldn't open Rename: {e}"));
    }
}

unsafe fn create(owner: HWND, paths: Vec<PathBuf>) -> windows::core::Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?.into();
        let class = w!("KestrelRename");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let mut r = RECT::default();
        GetWindowRect(owner, &mut r)?;
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            class,
            &HSTRING::from(format!("Rename {} items - Kestrel", paths.len())),
            WS_OVERLAPPEDWINDOW,
            r.left + scale(owner, 60),
            r.top + scale(owner, 60),
            scale(owner, 560),
            scale(owner, 480),
            Some(owner),
            None,
            Some(instance),
            None,
        )?;
        let made = (|| -> windows::core::Result<()> {
            for (id, text) in [
                (FIND_LABEL, "&Find (empty: whole name)"),
                (REPLACE_LABEL, "&Replace with (# numbers 1, 2, 3...)"),
            ] {
                child(hwnd, w!("STATIC"), &HSTRING::from(text), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), id)?;
            }
            for id in [FIND, REPLACE] {
                let style = WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
                child(hwnd, w!("EDIT"), w!(""), style, WS_EX_CLIENTEDGE, id)?;
            }
            let style =
                WS_TABSTOP | WS_VSCROLL | WINDOW_STYLE((ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32);
            child(hwnd, w!("EDIT"), w!(""), style, WS_EX_CLIENTEDGE, PREVIEW)?;
            child(hwnd, w!("STATIC"), w!(""), WINDOW_STYLE(0), WINDOW_EX_STYLE(0), HINT)?;
            let default = WS_TABSTOP | WINDOW_STYLE(BS_DEFPUSHBUTTON as u32);
            child(hwnd, w!("BUTTON"), w!("Rename"), default, WINDOW_EX_STYLE(0), IDOK.0 as usize)?;
            let plain = WS_TABSTOP | WINDOW_STYLE(BS_PUSHBUTTON as u32);
            child(hwnd, w!("BUTTON"), w!("Cancel"), plain, WINDOW_EX_STYLE(0), IDCANCEL.0 as usize)?;
            Ok(())
        })();
        if let Err(e) = made {
            let _ = DestroyWindow(hwnd);
            return Err(e);
        }
        let names =
            paths.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
        let font = super::message_font(hwnd);
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            Box::into_raw(Box::new(State { paths, names, font })) as isize,
        );
        let mut c = GetWindow(hwnd, GW_CHILD).unwrap_or_default();
        while !c.is_invalid() {
            set_font(&[c], font);
            c = GetWindow(c, GW_HWNDNEXT).unwrap_or_default();
        }
        super::dialogs::track(hwnd);
        layout(hwnd);
        update(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetFocus(GetDlgItem(Some(hwnd), FIND as i32).ok());
        Ok(())
    }
}

fn item(hwnd: HWND, id: usize) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id as i32) }.unwrap_or_default()
}

fn state(hwnd: HWND) -> Option<&'static State> {
    unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() }
}

/// The new names, or why they can't be used.
fn proposed(hwnd: HWND) -> Option<Result<Vec<String>, String>> {
    let s = state(hwnd)?;
    Some(batch_names(&s.names, &window_text(item(hwnd, FIND)), &window_text(item(hwnd, REPLACE))))
}

fn update(hwnd: HWND) {
    let (Some(s), Some(result)) = (state(hwnd), proposed(hwnd)) else { return };
    let (preview, hint, ok) = match &result {
        Ok(new) => {
            let changed = s.names.iter().zip(new).filter(|(a, b)| a != b).count();
            let lines: Vec<String> = s.names.iter().zip(new).map(|(a, b)| format!("{a}  →  {b}")).collect();
            (lines.join("\r\n"), format!("{changed} of {} names change", s.names.len()), changed > 0)
        }
        Err(e) => (s.names.join("\r\n"), e.clone(), false),
    };
    unsafe {
        let _ = SetWindowTextW(item(hwnd, PREVIEW), &HSTRING::from(preview));
        let _ = SetWindowTextW(item(hwnd, HINT), &HSTRING::from(hint));
        let _ = EnableWindow(item(hwnd, IDOK.0 as usize), ok);
    }
}

fn layout(hwnd: HWND) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(hwnd, &mut r);
        let s = |n| scale(hwnd, n);
        let (w, h) = (r.right, r.bottom);
        for (row, (label, id)) in [(FIND_LABEL, FIND), (REPLACE_LABEL, REPLACE)].into_iter().enumerate() {
            let y = s(12 + row as i32 * 40);
            let _ = MoveWindow(item(hwnd, label), s(12), y + s(4), s(230), s(24), true);
            let _ = MoveWindow(item(hwnd, id), s(246), y, (w - s(258)).max(s(60)), s(28), true);
        }
        let _ = MoveWindow(item(hwnd, PREVIEW), s(12), s(96), (w - s(24)).max(0), (h - s(186)).max(0), true);
        let _ = MoveWindow(item(hwnd, HINT), s(12), h - s(82), (w - s(24)).max(0), s(28), true);
        let _ = MoveWindow(item(hwnd, IDOK.0 as usize), w - s(192), h - s(44), s(86), s(30), true);
        let _ = MoveWindow(item(hwnd, IDCANCEL.0 as usize), w - s(98), h - s(44), s(86), s(30), true);
    }
}

extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    crate::crash::guard(|| unsafe {
        match msg {
            WM_COMMAND => match (wp.0 & 0xffff, (wp.0 >> 16) as u32) {
                (FIND | REPLACE, EN_CHANGE) => update(hwnd),
                (id, _) if id == IDOK.0 as usize => {
                    if let (Some(s), Some(Ok(new))) = (state(hwnd), proposed(hwnd)) {
                        let items: Vec<(PathBuf, String)> = s
                            .paths
                            .iter()
                            .zip(s.names.iter().zip(new))
                            .filter(|(_, (old, new))| old != &new)
                            .map(|(p, (_, new))| (p.clone(), new))
                            .collect();
                        super::commands::start_op(Op::RenameMany { items }, false);
                        let _ = DestroyWindow(hwnd);
                    }
                    return LRESULT(0);
                }
                (id, _) if id == IDCANCEL.0 as usize => {
                    let _ = DestroyWindow(hwnd);
                    return LRESULT(0);
                }
                _ => {}
            },
            WM_SIZE => layout(hwnd),
            WM_GETMINMAXINFO => {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize.x = scale(hwnd, 420);
                info.ptMinTrackSize.y = scale(hwnd, 300);
                return LRESULT(0);
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => {
                if let Some(r) = dark::edit_colors(HDC(wp.0 as _)) {
                    return r;
                }
            }
            WM_ERASEBKGND if dark::erase(hwnd, HDC(wp.0 as _)) => return LRESULT(1),
            WM_NCDESTROY => {
                super::dialogs::untrack(hwnd);
                let ptr = SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) as *mut State;
                if !ptr.is_null() {
                    let state = Box::from_raw(ptr);
                    let _ = DeleteObject(HGDIOBJ(state.font.0));
                }
            }
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    })
    .unwrap_or(LRESULT(0))
}
