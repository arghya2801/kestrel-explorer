//! Clipboard file lists in the format Explorer uses (`CF_HDROP` plus
//! "Preferred DropEffect" for cut vs copy), so copy/paste works both ways.
//!
//! Write from the UI thread (the clipboard needs an owner window). Read from a
//! worker thread: reading can make Windows ask the app that copied the data to
//! render it, and a hung app would otherwise hang us.

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;
use std::time::Duration;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW,
    SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT};
use windows::core::w;

/// Size of the DROPFILES header: offset, point, non-client flag, wide flag.
const HEADER: usize = 20;
const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
const DROPEFFECT_LINK: u32 = 4;

pub fn encode_hdrop(paths: &[PathBuf]) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend((HEADER as u32).to_le_bytes());
    v.extend([0u8; 12]); // pt.x, pt.y, fNC
    v.extend(1u32.to_le_bytes()); // fWide
    for p in paths {
        v.extend(p.as_os_str().encode_wide().chain([0]).flat_map(u16::to_le_bytes));
    }
    v.extend([0, 0]);
    v
}

/// Paths in a DROPFILES block. Anything malformed (bad offset, a name cut
/// off before its terminator) is dropped rather than guessed at.
pub fn decode_hdrop(b: &[u8]) -> Vec<PathBuf> {
    let at = |i: usize| b.get(i..i + 4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
    let (Some(offset), Some(wide)) = (at(0), at(16)) else { return Vec::new() };
    let Some(body) = b.get(offset as usize..) else { return Vec::new() };
    let mut out = Vec::new();
    if wide != 0 {
        let units: Vec<u16> = body.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let mut start = 0;
        for (i, &u) in units.iter().enumerate() {
            if u == 0 {
                if i == start {
                    break;
                }
                out.push(PathBuf::from(OsString::from_wide(&units[start..i])));
                start = i + 1;
            }
        }
    } else {
        let mut start = 0;
        for (i, &c) in body.iter().enumerate() {
            if c == 0 {
                if i == start {
                    break;
                }
                out.push(PathBuf::from(String::from_utf8_lossy(&body[start..i]).into_owned()));
                start = i + 1;
            }
        }
    }
    out
}

/// Open clipboard; closed on drop.
struct Opened;
impl Drop for Opened {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Another program may have the clipboard open for a moment: retry briefly.
fn open(owner: Option<HWND>) -> Result<Opened, String> {
    for _ in 0..20 {
        if unsafe { OpenClipboard(owner) }.is_ok() {
            return Ok(Opened);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("The clipboard is busy. Try again.".into())
}

fn drop_effect_format() -> u32 {
    unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) }
}

fn put(format: u32, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|e| e.message())?;
        let p = GlobalLock(mem);
        if p.is_null() {
            let _ = GlobalFree(Some(mem));
            return Err("Out of memory.".into());
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast(), bytes.len());
        let _ = GlobalUnlock(mem);
        // On success the clipboard owns the memory.
        if let Err(e) = SetClipboardData(format, Some(HANDLE(mem.0))) {
            let _ = GlobalFree(Some(mem));
            return Err(e.message());
        }
    }
    Ok(())
}

fn take(format: u32) -> Option<Vec<u8>> {
    unsafe {
        let mem = HGLOBAL(GetClipboardData(format).ok()?.0);
        let p = GlobalLock(mem);
        if p.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(p as *const u8, GlobalSize(mem)).to_vec();
        let _ = GlobalUnlock(mem);
        Some(bytes)
    }
}

/// Puts files on the clipboard; `cut` marks them to be moved on paste. UI thread.
pub fn set_files(owner: HWND, paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let _open = open(Some(owner))?;
    unsafe { EmptyClipboard() }.map_err(|e| e.message())?;
    put(CF_HDROP.0 as u32, &encode_hdrop(paths))?;
    let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY | DROPEFFECT_LINK };
    put(drop_effect_format(), &effect.to_le_bytes())
}

/// Puts text on the clipboard. UI thread.
pub fn set_text(owner: HWND, text: &str) -> Result<(), String> {
    let _open = open(Some(owner))?;
    unsafe { EmptyClipboard() }.map_err(|e| e.message())?;
    let bytes: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    put(CF_UNICODETEXT.0 as u32, &bytes)
}

/// Files on the clipboard and whether they were cut. Worker threads only.
pub fn get_files() -> Option<(Vec<PathBuf>, bool)> {
    let _open = open(None).ok()?;
    let files = decode_hdrop(&take(CF_HDROP.0 as u32)?);
    if files.is_empty() {
        return None;
    }
    let effect = take(drop_effect_format())
        .and_then(|b| b.get(..4).map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]])))
        .unwrap_or(DROPEFFECT_COPY);
    Some((files, effect & DROPEFFECT_MOVE != 0 && effect & DROPEFFECT_COPY == 0))
}

/// Text on the clipboard. Worker threads only.
pub fn get_text() -> Option<String> {
    let _open = open(None).ok()?;
    let bytes = take(CF_UNICODETEXT.0 as u32)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let len = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..len]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, HWND_MESSAGE, WINDOW_EX_STYLE, WINDOW_STYLE,
    };
    use windows::core::w;

    #[test]
    fn file_lists_round_trip() {
        let long = PathBuf::from(format!(r"C:\{}\file.txt", "d".repeat(300)));
        let paths = vec![PathBuf::from(r"C:\a b\ünï 😀.txt"), PathBuf::from(r"\\srv\share\x"), long];
        assert_eq!(decode_hdrop(&encode_hdrop(&paths)), paths);
    }

    #[test]
    fn broken_file_lists_decode_to_what_is_readable() {
        assert!(decode_hdrop(&[]).is_empty());
        assert!(decode_hdrop(&[1, 2, 3]).is_empty());
        let mut bytes = encode_hdrop(&[PathBuf::from(r"C:\ok")]);
        bytes[0] = 0xFF; // offset past the end
        assert!(decode_hdrop(&bytes).is_empty());
        let mut cut = encode_hdrop(&[PathBuf::from(r"C:\one"), PathBuf::from(r"C:\two")]);
        cut.truncate(cut.len() - 6); // second name loses its end and the list terminator
        assert_eq!(decode_hdrop(&cut), [PathBuf::from(r"C:\one")]);
    }

    /// Uses the real clipboard, restoring any text that was on it.
    #[test]
    fn files_survive_a_trip_through_the_clipboard() {
        let owner = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .unwrap();
        let saved = get_text();
        let paths = vec![PathBuf::from(r"C:\x\one.txt"), PathBuf::from(r"C:\x\two.txt")];
        set_files(owner, &paths, true).unwrap();
        assert_eq!(get_files(), Some((paths.clone(), true)));
        set_files(owner, &paths, false).unwrap();
        assert_eq!(get_files(), Some((paths, false)));
        set_text(owner, "hello").unwrap();
        assert_eq!(get_files(), None);
        assert_eq!(get_text().as_deref(), Some("hello"));
        if let Some(t) = saved {
            set_text(owner, &t).unwrap();
        }
        unsafe { DestroyWindow(owner) }.unwrap();
    }
}
