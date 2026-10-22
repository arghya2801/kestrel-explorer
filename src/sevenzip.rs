//! 7-Zip's commands for the right-click menu, when 7-Zip is installed. They run 7-Zip's own
//! programs (7zG.exe shows progress, questions and errors; 7zFM.exe opens an archive), so
//! none of 7-Zip loads into this process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::core::{HSTRING, w};

/// Extensions 7-Zip opens as archives.
const ARCHIVES: [&str; 26] = [
    "7z", "zip", "rar", "tar", "gz", "tgz", "bz2", "tbz", "tbz2", "xz", "txz", "zst", "tzst", "lzma", "z",
    "cab", "iso", "wim", "arj", "lzh", "cpio", "rpm", "deb", "001", "zipx", "jar",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Open,
    ExtractFiles,
    ExtractHere,
    ExtractToFolder,
    Test,
    AddDialog,
    Add7z,
    AddZip,
}

/// In menu order. Actions before `AddDialog` work on archives only.
pub const ACTIONS: [Action; 8] = [
    Action::Open,
    Action::ExtractFiles,
    Action::ExtractHere,
    Action::ExtractToFolder,
    Action::Test,
    Action::AddDialog,
    Action::Add7z,
    Action::AddZip,
];

/// 7-Zip's folder, from the installer's registry entry. Read once; registry only.
pub fn install_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE].into_iter().find_map(read_path).map(PathBuf::from)
    })
    .as_deref()
}

fn read_path(root: HKEY) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = (buf.len() * 2) as u32;
    unsafe {
        RegGetValueW(
            root,
            w!(r"Software\7-Zip"),
            &HSTRING::from("Path"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    }
    .ok()
    .ok()?;
    let text = String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]);
    (!text.is_empty()).then_some(text)
}

pub fn is_archive(path: &Path) -> bool {
    path.extension().is_some_and(|e| ARCHIVES.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

/// Whether `action` applies to `items` (path, is a folder).
pub fn applies(action: Action, items: &[(PathBuf, bool)]) -> bool {
    let archives = !items.is_empty() && items.iter().all(|(p, dir)| !dir && is_archive(p));
    match action {
        Action::Open => archives && items.len() == 1,
        Action::ExtractFiles | Action::ExtractHere | Action::ExtractToFolder | Action::Test => archives,
        Action::AddDialog | Action::Add7z | Action::AddZip => !items.is_empty(),
    }
}

/// The name 7-Zip suggests: one item's own name, or for several, their folder's.
fn archive_name(items: &[PathBuf]) -> String {
    let name = match items {
        [one] => one.file_stem().or(one.file_name()),
        _ => items.first().and_then(|p| p.parent()).and_then(Path::file_name),
    };
    name.map_or_else(|| "Archive".into(), |n| n.to_string_lossy().into_owned())
}

fn stem(archive: &Path) -> String {
    archive.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned())
}

/// The menu text, as 7-Zip's own menu words it.
pub fn label(action: Action, items: &[PathBuf]) -> String {
    match action {
        Action::Open => "Open archive".into(),
        Action::ExtractFiles => "Extract files…".into(),
        Action::ExtractHere => "Extract Here".into(),
        Action::ExtractToFolder if items.len() == 1 => format!("Extract to \"{}\\\"", stem(&items[0])),
        Action::ExtractToFolder => "Extract each archive to its own folder".into(),
        Action::Test => "Test archive".into(),
        Action::AddDialog => "Add to archive…".into(),
        Action::Add7z => format!("Add to \"{}.7z\"", archive_name(items)),
        Action::AddZip => format!("Add to \"{}.zip\"", archive_name(items)),
    }
    .replace('&', "&&")
}

/// The programs to run for `action`, one after another: program file name and arguments.
/// `--` ends the switches, so names starting with `-` or `@` stay names.
pub fn commands(action: Action, items: &[PathBuf]) -> Vec<(&'static str, Vec<OsString>)> {
    let folder = |p: &Path| p.parent().map(Path::to_path_buf).unwrap_or_default();
    let output = |dir: PathBuf| {
        let mut o = OsString::from("-o");
        o.push(dir);
        o
    };
    let extract = |archive: &PathBuf, switches: Vec<OsString>| {
        let mut args: Vec<OsString> = vec!["x".into()];
        args.extend(switches);
        args.extend(["--".into(), archive.into()]);
        ("7zG.exe", args)
    };
    match action {
        Action::Open => items.iter().take(1).map(|a| ("7zFM.exe", vec![a.into()])).collect(),
        Action::ExtractFiles => {
            items.iter().map(|a| extract(a, vec!["-ad".into(), output(folder(a).join(stem(a)))])).collect()
        }
        Action::ExtractHere => items.iter().map(|a| extract(a, vec![output(folder(a))])).collect(),
        Action::ExtractToFolder => {
            items.iter().map(|a| extract(a, vec![output(folder(a).join(stem(a)))])).collect()
        }
        Action::Test => items.iter().map(|a| ("7zG.exe", vec!["t".into(), "--".into(), a.into()])).collect(),
        Action::AddDialog | Action::Add7z | Action::AddZip => {
            let Some(first) = items.first() else { return Vec::new() };
            let (switch, ext) = match action {
                Action::AddDialog => ("-ad", "7z"),
                Action::Add7z => ("-t7z", "7z"),
                _ => ("-tzip", "zip"),
            };
            let archive = folder(first).join(format!("{}.{ext}", archive_name(items)));
            let mut args: Vec<OsString> = vec!["a".into(), switch.into(), "--".into(), archive.into()];
            args.extend(items.iter().map(OsString::from));
            vec![("7zG.exe", args)]
        }
    }
}

/// Runs the commands one after another, each waiting for the last. 7-Zip reports its own
/// errors; this returns the first program that couldn't start. Waits: worker threads only.
pub fn run(dir: &Path, action: Action, items: &[PathBuf]) -> Result<(), String> {
    for (program, args) in commands(action, items) {
        let cwd = items.first().and_then(|p| p.parent()).unwrap_or(dir);
        std::process::Command::new(dir.join(program))
            .args(args)
            .current_dir(cwd)
            .spawn()
            .and_then(|mut child| child.wait())
            .map_err(|e| format!("Couldn't start 7-Zip ({program}): {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cmd: &(&str, Vec<OsString>)) -> Vec<String> {
        cmd.1.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn archive_actions_need_archives_only() {
        let zip = (PathBuf::from(r"C:\d\a.ZIP"), false);
        let txt = (PathBuf::from(r"C:\d\b.txt"), false);
        let dir = (PathBuf::from(r"C:\d\c.zip"), true);
        assert!(
            applies(Action::ExtractHere, std::slice::from_ref(&zip))
                && applies(Action::Open, std::slice::from_ref(&zip))
        );
        assert!(!applies(Action::ExtractHere, &[zip.clone(), txt.clone()]));
        assert!(!applies(Action::Open, &[zip.clone(), zip.clone()]));
        assert!(!applies(Action::Test, std::slice::from_ref(&dir)));
        assert!(applies(Action::AddZip, &[txt, dir]) && !applies(Action::AddZip, &[]));
    }

    #[test]
    fn names_follow_seven_zip() {
        let one = [PathBuf::from(r"C:\d\report.final.docx")];
        assert_eq!(label(Action::AddZip, &one), "Add to \"report.final.zip\"");
        let folder = [PathBuf::from(r"C:\d\Photos & more")];
        assert_eq!(label(Action::Add7z, &folder), "Add to \"Photos && more.7z\"");
        let two = [PathBuf::from(r"C:\work\a.txt"), PathBuf::from(r"C:\work\b.txt")];
        assert_eq!(label(Action::AddZip, &two), "Add to \"work.zip\"");
        assert_eq!(
            label(Action::ExtractToFolder, &[PathBuf::from(r"C:\d\x.tar.gz")]),
            "Extract to \"x.tar\\\""
        );
    }

    #[test]
    fn commands_put_names_after_the_switches() {
        let two = [PathBuf::from(r"C:\work\a.txt"), PathBuf::from(r"C:\work\-b.txt")];
        let add = commands(Action::AddZip, &two);
        assert_eq!(add.len(), 1);
        assert_eq!(add[0].0, "7zG.exe");
        assert_eq!(
            args(&add[0]),
            ["a", "-tzip", "--", r"C:\work\work.zip", r"C:\work\a.txt", r"C:\work\-b.txt"]
        );

        let archives = [PathBuf::from(r"C:\d\one.7z"), PathBuf::from(r"C:\d\two.zip")];
        let each = commands(Action::ExtractToFolder, &archives);
        assert_eq!(
            each.iter().map(args).collect::<Vec<_>>(),
            [["x", r"-oC:\d\one", "--", r"C:\d\one.7z"], ["x", r"-oC:\d\two", "--", r"C:\d\two.zip"],]
        );
        assert_eq!(
            args(&commands(Action::ExtractHere, &archives[..1])[0]),
            ["x", r"-oC:\d", "--", r"C:\d\one.7z"]
        );
        assert_eq!(args(&commands(Action::AddDialog, &two)[0])[..2], ["a", "-ad"]);
        assert_eq!(commands(Action::Open, &archives), [("7zFM.exe", vec![OsString::from(r"C:\d\one.7z")])]);
    }
}
