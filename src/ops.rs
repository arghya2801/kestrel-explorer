//! File operations through Windows' own engine (`IFileOperation`): Recycle Bin,
//! undo records, progress and conflict dialogs. Each call blocks until the
//! operation ends, so it runs on its own worker thread.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR, FOF_NOERRORUI,
    FOF_RENAMEONCOLLISION, FOF_SILENT, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
    IFileOperationProgressSink, IShellItem, SHCreateItemFromParsingName,
};
use windows::core::{HSTRING, PCWSTR};

use crate::shell::Com;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Delete { paths: Vec<PathBuf>, recycle: bool },
    Rename { path: PathBuf, name: String },
    NewFolder { dir: PathBuf },
    Copy { paths: Vec<PathBuf>, dest: PathBuf },
    Move { paths: Vec<PathBuf>, dest: PathBuf },
}

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    /// Windows shows progress, confirmations and errors, owned by this window.
    Interactive(isize),
    /// No UI at all; failures come back as errors. For tests.
    Silent,
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
    "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A file name as Windows will store it (outer spaces and trailing dots
/// dropped), or why it can't be used.
pub fn validate_name(name: &str) -> Result<String, String> {
    let name = name.trim_matches(' ').trim_end_matches(['.', ' ']);
    if name.is_empty() {
        return Err("A name can't be empty.".into());
    }
    if name.chars().any(|c| (c as u32) < 32 || r#"\/:*?"<>|"#.contains(c)) {
        return Err(r#"A name can't contain any of these characters: \ / : * ? " < > |"#.into());
    }
    if name.encode_utf16().count() > 255 {
        return Err("That name is too long.".into());
    }
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return Err(format!("\"{name}\" is reserved by Windows."));
    }
    Ok(name.to_string())
}

/// `base`, or `base (2)`, `base (3)`, ... whichever doesn't exist yet.
pub fn unique_name(base: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base} ({n})")).find(|n| !exists(n)).unwrap_or_default()
}

fn item(path: &Path) -> Result<IShellItem, String> {
    unsafe { SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None) }
        .map_err(|e| format!("{}: {}", path.display(), e.message()))
}

const NO_SINK: Option<&IFileOperationProgressSink> = None;

/// Runs `op` to completion. Returns the name of the item to select afterwards
/// (new or renamed), or `None`. Cancelled by the user counts as success.
pub fn run(op: &Op, mode: Mode) -> Result<Option<String>, String> {
    let _com = Com::init();
    let fail = |what: &str, e: windows::core::Error| format!("Couldn't {what}: {}", e.message());
    unsafe {
        let fo: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_ALL).map_err(|e| fail("start the operation", e))?;
        let mut flags = FOF_NOCONFIRMMKDIR | FOF_ALLOWUNDO;
        match mode {
            Mode::Silent => flags |= FOF_SILENT | FOF_NOERRORUI | FOF_NOCONFIRMATION,
            Mode::Interactive(owner) => {
                let _ = fo.SetOwnerWindow(HWND(owner as _));
            }
        }
        let mut select = None;
        match op {
            Op::Delete { paths, recycle } => {
                flags = if *recycle {
                    // Explorer doesn't ask before recycling; the Recycle Bin is the undo.
                    flags | FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION
                } else {
                    FILEOPERATION_FLAGS(flags.0 & !FOF_ALLOWUNDO.0)
                };
                for p in paths {
                    fo.DeleteItem(&item(p)?, NO_SINK).map_err(|e| fail("delete", e))?;
                }
            }
            Op::Rename { path, name } => {
                let name = validate_name(name)?;
                fo.RenameItem(&item(path)?, &HSTRING::from(&name), NO_SINK).map_err(|e| fail("rename", e))?;
                select = Some(name);
            }
            Op::NewFolder { dir } => {
                let name = unique_name("New folder", |n| dir.join(n).exists());
                let attrs = FILE_ATTRIBUTE_DIRECTORY.0;
                fo.NewItem(&item(dir)?, attrs, &HSTRING::from(&name), PCWSTR::null(), NO_SINK)
                    .map_err(|e| fail("create the folder", e))?;
                select = Some(name);
            }
            Op::Copy { paths, dest } | Op::Move { paths, dest } => {
                // Pasting into the folder a file came from makes "name - Copy", like Explorer.
                if matches!(op, Op::Copy { .. }) && paths.iter().any(|p| p.parent() == Some(dest.as_path())) {
                    flags |= FOF_RENAMEONCOLLISION;
                }
                let dest_item = item(dest)?;
                for p in paths {
                    let r = if matches!(op, Op::Copy { .. }) {
                        fo.CopyItem(&item(p)?, &dest_item, PCWSTR::null(), NO_SINK)
                    } else {
                        fo.MoveItem(&item(p)?, &dest_item, PCWSTR::null(), NO_SINK)
                    };
                    r.map_err(|e| fail("copy", e))?;
                }
            }
        }
        fo.SetOperationFlags(flags).map_err(|e| fail("start the operation", e))?;
        fo.PerformOperations().map_err(|e| fail("finish", e))?;
        if fo.GetAnyOperationsAborted().is_ok_and(|b| b.as_bool()) {
            return Ok(None);
        }
        Ok(select)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fep-ops-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_are_cleaned_or_rejected() {
        assert_eq!(validate_name("  report.txt  ").as_deref(), Ok("report.txt"));
        assert_eq!(validate_name("trailing...").as_deref(), Ok("trailing"));
        for bad in ["", "   ", "...", "a/b", r"a\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b"] {
            assert!(validate_name(bad).is_err(), "{bad:?} accepted");
        }
        for reserved in ["CON", "con", "Con.txt", "NUL", "aux.log", "COM1", "lpt9.doc", "PRN"] {
            assert!(validate_name(reserved).is_err(), "{reserved:?} accepted");
        }
        assert!(validate_name("COM10").is_ok() && validate_name("console").is_ok());
        assert!(validate_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn new_names_count_up_past_existing_ones() {
        let taken = ["New folder", "New folder (2)"];
        let exists = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
        assert_eq!(unique_name("New folder", exists), "New folder (3)");
        assert_eq!(unique_name("Other", exists), "Other");
    }

    #[test]
    fn rename_new_folder_copy_move_and_delete() {
        let d = temp("all");
        fs::write(d.join("a.txt"), "aaa").unwrap();
        fs::write(d.join("b.txt"), "b").unwrap();

        run(&Op::Rename { path: d.join("a.txt"), name: "renamed.txt".into() }, Mode::Silent).unwrap();
        assert!(d.join("renamed.txt").exists() && !d.join("a.txt").exists());

        let made = run(&Op::NewFolder { dir: d.clone() }, Mode::Silent).unwrap();
        assert_eq!(made.as_deref(), Some("New folder"));
        assert!(d.join("New folder").is_dir());
        assert_eq!(
            run(&Op::NewFolder { dir: d.clone() }, Mode::Silent).unwrap().as_deref(),
            Some("New folder (2)")
        );

        let dest = d.join("New folder");
        let both = vec![d.join("renamed.txt"), d.join("b.txt")];
        run(&Op::Copy { paths: both.clone(), dest: dest.clone() }, Mode::Silent).unwrap();
        assert_eq!(fs::read_to_string(dest.join("renamed.txt")).unwrap(), "aaa");
        assert!(d.join("b.txt").exists(), "copy keeps the source");

        run(&Op::Move { paths: vec![d.join("b.txt")], dest: d.join("New folder (2)") }, Mode::Silent)
            .unwrap();
        assert!(!d.join("b.txt").exists() && d.join("New folder (2)").join("b.txt").exists());

        run(&Op::Delete { paths: vec![d.join("renamed.txt"), dest.clone()], recycle: false }, Mode::Silent)
            .unwrap();
        assert!(!d.join("renamed.txt").exists() && !dest.exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn copying_onto_the_same_folder_makes_a_copy() {
        let d = temp("dup");
        fs::write(d.join("x.txt"), "x").unwrap();
        run(&Op::Copy { paths: vec![d.join("x.txt")], dest: d.clone() }, Mode::Silent).unwrap();
        assert_eq!(fs::read_dir(&d).unwrap().count(), 2, "Windows names the copy \"x - Copy.txt\"");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn failures_come_back_as_errors() {
        let d = temp("fail");
        let r = run(&Op::Rename { path: d.join("missing.txt"), name: "y.txt".into() }, Mode::Silent);
        assert!(r.is_err(), "{r:?}");
        fs::remove_dir_all(d).unwrap();
    }
}
