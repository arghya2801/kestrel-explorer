//! Search below a folder for names containing some text. Worker threads only:
//! it reads every folder under the root.

use std::collections::VecDeque;
use std::io;
use std::path::Path;

use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

use crate::listing::{Snapshot, list_dir};
use crate::sort::Query;

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
    let q = Query::parse(query, crate::sort::now());
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
            let found = q.matches(&listed, i)
                && q.content.as_deref().is_none_or(|text| {
                    !listed.is_dir(i) && contains_text(&dir.join(String::from_utf16_lossy(name)), text)
                });
            if found {
                out.add(&rel, e.attrs, e.size, e.modified);
                if let Some(last) = out.entries.last_mut() {
                    last.created = e.created;
                }
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

/// Files larger than this aren't read for `content:`.
const MAX_CONTENT: u64 = 16 << 20;

/// Whether the file holds `text` (lowercase UTF-8), ignoring ASCII case, as UTF-8 or UTF-16.
/// ponytail: a plain scan of the whole file; an index is the upgrade if searches get slow.
fn contains_text(path: &Path, text: &[u8]) -> bool {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_CONTENT) {
        return false;
    }
    let Ok(mut data) = std::fs::read(path) else { return false };
    data.make_ascii_lowercase();
    let wide: Vec<u8> = String::from_utf8_lossy(text).encode_utf16().flat_map(u16::to_le_bytes).collect();
    data.windows(text.len()).any(|w| w == text) || data.windows(wide.len()).any(|w| w == wide.as_slice())
}

/// Total bytes of every file below `dir`, hidden ones included, as Explorer's Properties counts.
/// Junctions aren't entered and unreadable folders are skipped. `None`: cancelled.
pub fn folder_size(dir: &Path, cancelled: &dyn Fn() -> bool) -> Option<u64> {
    let mut queue = vec![dir.to_path_buf()];
    let mut total = 0;
    while let Some(dir) = queue.pop() {
        if cancelled() {
            return None;
        }
        let Ok(listed) = list_dir(&dir) else { continue };
        for (i, e) in listed.entries.iter().enumerate() {
            if !listed.is_dir(i) {
                total += e.size;
            } else if e.attrs & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0 {
                queue.push(dir.join(listed.name_string(i)));
            }
        }
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::path::PathBuf;

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
    fn content_search_reads_utf8_and_utf16_files() {
        let d = temp("content");
        fs::write(d.join("a.txt"), "Invoice TOTAL: 42").unwrap();
        let utf16: Vec<u8> = "\u{feff}grand total".encode_utf16().flat_map(u16::to_le_bytes).collect();
        fs::write(d.join("b.txt"), utf16).unwrap();
        fs::write(d.join("c.txt"), "nothing here").unwrap();
        fs::create_dir(d.join("total folder")).unwrap();
        assert_eq!(found(&run(&d, "content:total", false).unwrap()), ["a.txt", "b.txt"]);
        assert_eq!(found(&run(&d, "b content:total", false).unwrap()), ["b.txt"]);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn folder_size_adds_up_nested_files_once() {
        let d = temp("size");
        fs::create_dir_all(d.join("a").join("b")).unwrap();
        fs::write(d.join("one.txt"), "x".repeat(10)).unwrap();
        fs::write(d.join("a").join("b").join("two.txt"), "x".repeat(32)).unwrap();
        let link = d.join("a").join("loop");
        let ok = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&d)
            .output()
            .unwrap();
        assert!(ok.status.success());
        assert_eq!(folder_size(&d, &|| false), Some(42), "the junction back to the root isn't counted");
        assert_eq!(folder_size(&d, &|| true), None);
        fs::remove_dir(&link).unwrap();
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
