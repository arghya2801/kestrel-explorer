//! What the preview pane shows for a file: our own text view for text files,
//! otherwise Windows' registered preview handler (run out of process by the
//! system), otherwise a thumbnail.

use std::io::Read;
use std::path::Path;

use windows::Win32::System::Com::CLSIDFromString;
use windows::Win32::UI::Shell::{ASSOCF_INIT_DEFAULTTOSTAR, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW};
use windows::core::{GUID, HSTRING, PCWSTR, PWSTR, w};

/// Read at most this much of a text file for the preview.
pub const MAX_TEXT: u64 = 64 * 1024;

const TEXT_EXTENSIONS: [&str; 36] = [
    "txt",
    "md",
    "log",
    "json",
    "xml",
    "csv",
    "tsv",
    "ini",
    "cfg",
    "conf",
    "rs",
    "py",
    "js",
    "ts",
    "tsx",
    "jsx",
    "c",
    "cpp",
    "h",
    "hpp",
    "cs",
    "java",
    "go",
    "toml",
    "yaml",
    "yml",
    "html",
    "htm",
    "css",
    "bat",
    "cmd",
    "ps1",
    "sh",
    "sql",
    "gitignore",
    "lock",
];

pub fn is_text(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|e| TEXT_EXTENSIONS.contains(&e.as_str()))
}

/// Text for the preview, or `None` if the bytes look binary. Handles UTF-8 and
/// UTF-16 byte-order marks; newlines become CRLF for the edit control.
pub fn decode_text(bytes: &[u8]) -> Option<String> {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(
            &rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect::<Vec<_>>(),
        )
    } else if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        String::from_utf16_lossy(
            &rest.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect::<Vec<_>>(),
        )
    } else {
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
        if bytes[..bytes.len().min(8192)].contains(&0) {
            return None;
        }
        String::from_utf8_lossy(bytes).into_owned()
    };
    Some(text.replace("\r\n", "\n").replace('\n', "\r\n"))
}

/// The start of a text file for the preview. Worker threads only.
pub fn read_text(path: &Path) -> std::io::Result<Option<String>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(MAX_TEXT).read_to_end(&mut bytes)?;
    Ok(decode_text(&bytes))
}

/// The preview handler registered for an extension (".pdf"), if any.
pub fn handler_for(ext: &str) -> Option<GUID> {
    // The shell-extension key Windows uses for preview handlers (IPreviewHandler).
    let preview = w!("{8895b1c6-b41f-4c1c-a562-0d564250836f}");
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    let found = unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_DEFAULTTOSTAR,
            ASSOCSTR_SHELLEXTENSION,
            &HSTRING::from(ext),
            preview,
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    };
    if found.is_err() {
        return None;
    }
    unsafe { CLSIDFromString(PCWSTR(buf.as_ptr())) }.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_files_are_recognised_by_extension() {
        assert!(is_text(Path::new(r"C:\a\notes.TXT")));
        assert!(is_text(Path::new(r"C:\a\main.rs")));
        assert!(!is_text(Path::new(r"C:\a\photo.jpg")));
        assert!(!is_text(Path::new(r"C:\a\Makefile")));
    }

    #[test]
    fn text_decodes_utf8_and_utf16_with_bom() {
        assert_eq!(decode_text("héllo\r\nworld".as_bytes()).as_deref(), Some("héllo\r\nworld"));
        let mut le = vec![0xFF, 0xFE];
        le.extend("hi ünï".encode_utf16().flat_map(u16::to_le_bytes));
        assert_eq!(decode_text(&le).as_deref(), Some("hi ünï"));
        let mut bom8 = vec![0xEF, 0xBB, 0xBF];
        bom8.extend(b"x");
        assert_eq!(decode_text(&bom8).as_deref(), Some("x"));
    }

    #[test]
    fn binary_content_is_not_shown_as_text() {
        assert_eq!(decode_text(&[0x4D, 0x5A, 0x90, 0x00, 0x03, 0x00]), None);
    }

    #[test]
    fn bare_newlines_become_windows_newlines_for_the_edit_box() {
        assert_eq!(decode_text(b"a\nb\r\nc").as_deref(), Some("a\r\nb\r\nc"));
    }

    #[test]
    fn handlers_are_found_for_known_types_only() {
        let _com = crate::shell::Com::init();
        assert!(handler_for(".fep-no-such-type").is_none());
        // Windows ships a previewer for plain text.
        assert!(handler_for(".txt").is_some());
    }
}
