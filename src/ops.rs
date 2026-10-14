//! File operations through Windows' own engine (`IFileOperation`): Recycle Bin,
//! undo records, progress and conflict dialogs. Each call blocks until the
//! operation ends, so it runs on its own worker thread.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use windows::Win32::Foundation::HWND;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR, FOF_NOERRORUI,
    FOF_RENAMEONCOLLISION, FOF_SILENT, FOFX_RECYCLEONDELETE, FileOperation, IFileOperation,
    IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem, SHCreateItemFromParsingName,
    SIGDN, SIGDN_DESKTOPABSOLUTEPARSING, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, Ref, implement};

use crate::shell::Com;

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Delete {
        paths: Vec<PathBuf>,
        recycle: bool,
    },
    Rename {
        path: PathBuf,
        name: String,
    },
    NewFolder {
        dir: PathBuf,
    },
    Copy {
        paths: Vec<PathBuf>,
        dest: PathBuf,
    },
    Move {
        paths: Vec<PathBuf>,
        dest: PathBuf,
    },
    /// Undo of a move, rename or recycle: each item (by parsing name; Recycle Bin
    /// items have no file path) goes back to its original path.
    Restore {
        items: Vec<(String, PathBuf)>,
    },
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

/// Runs `op` to completion and records it for Undo. Returns the name of the item to
/// select afterwards (new or renamed), or `None`. Cancelled by the user counts as success.
pub fn run(op: &Op, mode: Mode) -> Result<Option<String>, String> {
    run_recorded(op, mode, true)
}

/// `fresh`: a new operation, which ends what Redo could repeat.
fn run_recorded(op: &Op, mode: Mode, fresh: bool) -> Result<Option<String>, String> {
    let job = crate::activity::Job::start(label(op));
    let result = run_inner(op, mode);
    job.finish(&result.as_ref().map(|r| r.0.clone()).map_err(Clone::clone));
    let (select, undo) = result?;
    let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(undo) = undo {
        history.undo.push((undo, op.clone()));
    }
    if fresh {
        history.redo.clear();
    }
    Ok(select)
}

/// The operation as the File operations window lists it.
fn label(op: &Op) -> String {
    match op {
        Op::Copy { paths, dest } => format!("Copy {} items to {}", paths.len(), dest.display()),
        Op::Move { paths, dest } => format!("Move {} items to {}", paths.len(), dest.display()),
        Op::Delete { paths, recycle } => format!(
            "{} {} items\r\n{}",
            if *recycle { "Recycle" } else { "Delete permanently" },
            paths.len(),
            paths.first().map(|p| p.display().to_string()).unwrap_or_default()
        ),
        Op::Rename { path, name } => format!("Rename {} to {name}", path.display()),
        Op::NewFolder { dir } => format!("New folder in {}", dir.display()),
        Op::Restore { items } => format!("Put back {} items", items.len()),
    }
}

/// This process's Undo and Redo stacks, like Explorer's: (undo, original) pairs, and originals.
struct History {
    undo: Vec<(Op, Op)>,
    redo: Vec<Op>,
}
static HISTORY: Mutex<History> = Mutex::new(History { undo: Vec::new(), redo: Vec::new() });

/// Whether Undo and Redo have anything to do.
pub fn can_undo_redo() -> (bool, bool) {
    let h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    (!h.undo.is_empty(), !h.redo.is_empty())
}

/// Reverses the latest operation. `None`: nothing to undo.
pub fn undo(mode: Mode) -> Option<Result<Option<String>, String>> {
    let (undo, original) = HISTORY.lock().unwrap_or_else(|e| e.into_inner()).undo.pop()?;
    let job = crate::activity::Job::start(format!("Undo {}", label(&original)));
    let result = run_inner(&undo, mode).map(|r| r.0);
    job.finish(&result);
    let mut h = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    match &result {
        Ok(_) => h.redo.push(original),
        Err(_) => h.undo.push((undo, original)),
    }
    Some(result)
}

/// Repeats the latest undone operation. `None`: nothing to redo.
pub fn redo(mode: Mode) -> Option<Result<Option<String>, String>> {
    let op = HISTORY.lock().unwrap_or_else(|e| e.into_inner()).redo.pop()?;
    let result = run_recorded(&op, mode, false);
    if result.is_err() {
        HISTORY.lock().unwrap_or_else(|e| e.into_inner()).redo.push(op);
    }
    Some(result)
}

/// What each item became, collected while Windows works, so Undo knows the real names.
#[implement(IFileOperationProgressSink)]
struct Sink(std::sync::Arc<Mutex<Vec<Made>>>);

/// An item's source path (none for a new item) and what it became, as a parsing name.
type Made = (Option<PathBuf>, String);

impl Sink {
    fn record(&self, from: Ref<IShellItem>, hr: HRESULT, made: Ref<IShellItem>) {
        if let (true, Some(made)) = (hr.is_ok(), made.as_ref())
            && let Some(to) = name_of(made, SIGDN_DESKTOPABSOLUTEPARSING)
        {
            let from = from.as_ref().and_then(|f| name_of(f, SIGDN_FILESYSPATH)).map(PathBuf::from);
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push((from, to));
        }
    }
}

fn name_of(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as _));
        s
    }
}

type R = windows::core::Result<()>;

impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn PostRenameItem(
        &self,
        _: u32,
        i: Ref<IShellItem>,
        _: &PCWSTR,
        hr: HRESULT,
        made: Ref<IShellItem>,
    ) -> R {
        self.record(i, hr, made);
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        i: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        hr: HRESULT,
        made: Ref<IShellItem>,
    ) -> R {
        self.record(i, hr, made);
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        i: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        hr: HRESULT,
        made: Ref<IShellItem>,
    ) -> R {
        self.record(i, hr, made);
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, i: Ref<IShellItem>, hr: HRESULT, made: Ref<IShellItem>) -> R {
        self.record(i, hr, made);
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        hr: HRESULT,
        made: Ref<IShellItem>,
    ) -> R {
        self.record(Ref::default(), hr, made);
        Ok(())
    }
    fn StartOperations(&self) -> R {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> R {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> R {
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> R {
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> R {
        Ok(())
    }
    fn PreDeleteItem(&self, _: u32, _: Ref<IShellItem>) -> R {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> R {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> R {
        Ok(())
    }
    fn ResetTimer(&self) -> R {
        Ok(())
    }
    fn PauseTimer(&self) -> R {
        Ok(())
    }
    fn ResumeTimer(&self) -> R {
        Ok(())
    }
}

/// The operation that reverses `op`, given what it produced.
fn undo_of(op: &Op, made: Vec<Made>) -> Option<Op> {
    match op {
        _ if made.is_empty() => None,
        Op::Delete { recycle: false, .. } | Op::Restore { .. } => None,
        // Undoing a copy recycles the copies, as Explorer does.
        Op::Copy { .. } | Op::NewFolder { .. } => Some(Op::Delete {
            paths: made.into_iter().map(|(_, to)| PathBuf::from(to)).collect(),
            recycle: true,
        }),
        Op::Rename { .. } | Op::Move { .. } | Op::Delete { .. } => {
            Some(Op::Restore { items: made.into_iter().filter_map(|(from, to)| Some((to, from?))).collect() })
        }
    }
}

/// Runs `op` and returns the item to select and the operation that undoes it.
fn run_inner(op: &Op, mode: Mode) -> Result<(Option<String>, Option<Op>), String> {
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
            Op::Restore { items } => {
                for (now, original) in items {
                    let (Some(parent), Some(name)) = (original.parent(), original.file_name()) else {
                        continue;
                    };
                    fo.MoveItem(&item(Path::new(now))?, &item(parent)?, &HSTRING::from(name), NO_SINK)
                        .map_err(|e| fail("put the item back", e))?;
                }
            }
        }
        fo.SetOperationFlags(flags).map_err(|e| fail("start the operation", e))?;
        let made = std::sync::Arc::default();
        let sink: IFileOperationProgressSink = Sink(std::sync::Arc::clone(&made)).into();
        let advised = fo.Advise(&sink).map_err(|e| fail("start the operation", e))?;
        let performed = fo.PerformOperations().map_err(|e| fail("finish", e));
        let _ = fo.Unadvise(advised);
        // Whatever finished before a failure or cancel can still be undone.
        let undo = undo_of(op, std::mem::take(&mut *made.lock().unwrap_or_else(|e| e.into_inner())));
        performed?;
        if fo.GetAnyOperationsAborted().is_ok_and(|b| b.as_bool()) {
            return Ok((None, undo));
        }
        Ok((select, undo))
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
    fn every_recorded_operation_undoes() {
        let d = temp("undo");
        fs::write(d.join("x.txt"), "x").unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        let undo = |op: Op| {
            let (_, undo) = run_inner(&op, Mode::Silent).unwrap();
            // Same items, but kept out of the real Recycle Bin.
            let undo = match undo.expect("undoable") {
                Op::Delete { paths, .. } => Op::Delete { paths, recycle: false },
                undo => undo,
            };
            run_inner(&undo, Mode::Silent).unwrap();
        };
        let names = || {
            let mut n: Vec<_> = fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
            n.sort();
            n
        };
        let before = names();
        undo(Op::Rename { path: d.join("x.txt"), name: "y.txt".into() });
        assert_eq!(names(), before);
        // Windows names the copy "x - Copy.txt"; Undo must remove that one, not x.txt.
        undo(Op::Copy { paths: vec![d.join("x.txt")], dest: d.clone() });
        assert_eq!(names(), before);
        undo(Op::NewFolder { dir: d.clone() });
        assert_eq!(names(), before);
        undo(Op::Move { paths: vec![d.join("x.txt")], dest: d.join("sub") });
        assert_eq!(names(), before);
        undo(Op::Delete { paths: vec![d.join("x.txt")], recycle: true });
        assert_eq!(fs::read_to_string(d.join("x.txt")).unwrap(), "x", "restored from the Recycle Bin");
        let (_, none) =
            run_inner(&Op::Delete { paths: vec![d.join("x.txt")], recycle: false }, Mode::Silent).unwrap();
        assert_eq!(none, None, "permanent deletes can't be undone");
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
