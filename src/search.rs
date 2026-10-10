//! Search below a folder for names containing some text. Worker threads only:
//! it reads every folder under the root.

use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use crate::listing::{Snapshot, list_dir};
use crate::sort::fold;

/// Enough to be useful; a search matching more than this should be narrowed.
pub const MAX_RESULTS: usize = 100_000;

/// Everything under `root` whose name contains `query` (ignoring case), as a
/// snapshot of `root` with names relative to it (`sub\file.txt`). Junctions and
/// other reparse points aren't entered, so links can't make it loop. Folders
/// that can't be read are skipped. `cancelled` is checked before each folder;
/// `progress` gets the number of matches so far.
pub fn search(
    root: &Path,
    query: &str,
    show_hidden: bool,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(usize),
) -> io::Result<Snapshot> {
    let needle = fold(&query.encode_utf16().collect::<Vec<_>>());
    let mut out = Snapshot::new(root.to_path_buf());
    let mut queue = VecDeque::from([(root.to_path_buf(), Vec::<u16>::new())]);
    let mut first = true;
    while let Some((dir, prefix)) = queue.pop_front() {
        if cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let listed = match list_dir(&dir) {
            Ok(s) => s,
            Err(e) if first => return Err(e),
            Err(_) => continue, // access denied etc.: skip that folder
        };
        first = false;
        for i in 0..listed.len() {
            if listed.is_hidden(i) && !show_hidden {
                continue;
            }
            let name = listed.name(i);
            let mut rel = prefix.clone();
            if !rel.is_empty() {
                rel.push(b'\\' as u16);
            }
            rel.extend_from_slice(name);
            let e = &listed.entries[i];
            if needle.is_empty() || fold(name).windows(needle.len()).any(|w| w == needle.as_slice()) {
                out.add(&rel, e.attrs, e.size, e.modified);
                if out.len() >= MAX_RESULTS {
                    return Ok(out);
                }
            }
            if listed.is_dir(i) && e.attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
                queue.push_back((dir.join(String::from_utf16_lossy(name)), rel));
            }
        }
        progress(out.len());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::ffi::OsStrExt;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fep-search-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn found(s: &Snapshot) -> Vec<String> {
        let mut v: Vec<String> = (0..s.len()).map(|i| s.name_string(i)).collect();
        v.sort();
        v
    }

    fn run(root: &Path, q: &str, hidden: bool) -> std::io::Result<Snapshot> {
        search(root, q, hidden, &|| false, &mut |_| {})
    }

    #[test]
    fn finds_matches_in_subfolders_as_relative_names() {
        let d = temp("nested");
        fs::create_dir_all(d.join("a").join("b")).unwrap();
        fs::create_dir_all(d.join("reports")).unwrap();
        fs::write(d.join("a").join("b").join("report1.txt"), "").unwrap();
        fs::write(d.join("Report2.txt"), "").unwrap();
        fs::write(d.join("a").join("other.txt"), "").unwrap();
        let s = run(&d, "REPORT", false).unwrap();
        assert_eq!(found(&s), ["Report2.txt", r"a\b\report1.txt", "reports"]);
        let i = (0..s.len()).find(|&i| s.name_string(i) == "reports").unwrap();
        assert!(s.is_dir(i));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn does_not_follow_junctions() {
        let d = temp("loop");
        fs::write(d.join("target.txt"), "").unwrap();
        let link = d.join("loop");
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&d)
            .output()
            .unwrap();
        assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stdout));
        assert_eq!(found(&run(&d, "target", false).unwrap()), ["target.txt"]);
        fs::remove_dir(&link).unwrap();
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn hidden_items_are_skipped_unless_shown() {
        let d = temp("hidden");
        let h = d.join("secret match.txt");
        fs::write(&h, "").unwrap();
        let wide: Vec<u16> = h.as_os_str().encode_wide().chain([0]).collect();
        unsafe {
            windows::Win32::Storage::FileSystem::SetFileAttributesW(
                windows::core::PCWSTR(wide.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN,
            )
        }
        .unwrap();
        assert!(run(&d, "match", false).unwrap().is_empty());
        assert_eq!(run(&d, "match", true).unwrap().len(), 1);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cancelling_stops_the_search() {
        let d = temp("cancel");
        fs::create_dir_all(d.join("x")).unwrap();
        let err = search(&d, "x", false, &|| true, &mut |_| {}).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Interrupted);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn missing_root_is_an_error() {
        let d = temp("missing");
        assert_eq!(run(&d.join("nope"), "x", false).unwrap_err().kind(), std::io::ErrorKind::NotFound);
        fs::remove_dir_all(d).unwrap();
    }
}
