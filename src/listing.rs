//! Directory listing. Runs on worker threads only: it can block for a long time
//! on network paths or slow media.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_FILES, HANDLE};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN, FIND_FIRST_EX_LARGE_FETCH, FindClose, FindExInfoBasic,
    FindExSearchNameMatch, FindFirstFileExW, FindNextFileW, WIN32_FIND_DATAW,
};
use windows::core::PCWSTR;

#[derive(Debug)]
pub struct Entry {
    name_start: u32,
    name_len: u32,
    pub attrs: u32,
    pub size: u64,
    /// FILETIME as u64 (100ns ticks since 1601, UTC).
    pub modified: u64,
}

/// One folder's contents. Immutable once built; names live in one shared buffer.
#[derive(Debug)]
pub struct Snapshot {
    pub dir: PathBuf,
    pub entries: Vec<Entry>,
    names: Vec<u16>,
}

impl Snapshot {
    pub fn new(dir: PathBuf) -> Self {
        Snapshot { dir, entries: Vec::new(), names: Vec::new() }
    }
    pub fn add(&mut self, name: &[u16], attrs: u32, size: u64, modified: u64) {
        self.entries.push(Entry {
            name_start: self.names.len() as u32,
            name_len: name.len() as u32,
            attrs,
            size,
            modified,
        });
        self.names.extend_from_slice(name);
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn name(&self, i: usize) -> &[u16] {
        let e = &self.entries[i];
        &self.names[e.name_start as usize..(e.name_start + e.name_len) as usize]
    }
    pub fn name_string(&self, i: usize) -> String {
        String::from_utf16_lossy(self.name(i))
    }
    pub fn is_dir(&self, i: usize) -> bool {
        self.entries[i].attrs & FILE_ATTRIBUTE_DIRECTORY.0 != 0
    }
    pub fn is_hidden(&self, i: usize) -> bool {
        self.entries[i].attrs & FILE_ATTRIBUTE_HIDDEN.0 != 0
    }
}

/// `\\?\`-prefixed form of an absolute path, so >MAX_PATH paths work without
/// the LongPathsEnabled registry switch.
pub fn verbatim(path: &Path) -> Vec<u16> {
    let s: Vec<u16> = path.as_os_str().encode_wide().collect();
    let starts = |p: &str| s.starts_with(&p.encode_utf16().collect::<Vec<_>>());
    if starts(r"\\?\") {
        s
    } else if starts(r"\\") {
        r"\\?\UNC".encode_utf16().chain(s[1..].iter().copied()).collect()
    } else {
        r"\\?\".encode_utf16().chain(s).collect()
    }
}

struct FindHandle(HANDLE);
impl Drop for FindHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = FindClose(self.0);
        }
    }
}

pub fn list_dir(dir: &Path) -> io::Result<Snapshot> {
    let dir = std::path::absolute(dir)?;
    let mut pattern = verbatim(&dir.join("*"));
    pattern.push(0);

    let mut snap = Snapshot::new(dir);
    let mut data = WIN32_FIND_DATAW::default();
    let handle = unsafe {
        FindFirstFileExW(
            PCWSTR(pattern.as_ptr()),
            FindExInfoBasic,
            &mut data as *mut _ as *mut _,
            FindExSearchNameMatch,
            None,
            FIND_FIRST_EX_LARGE_FETCH,
        )
    };
    let handle = match handle {
        Ok(h) => FindHandle(h),
        // A drive root has no "." / "..", so an empty root reports "file not found".
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() && snap.dir.parent().is_none() => {
            return Ok(snap);
        }
        Err(e) => return Err(to_io(e)),
    };
    loop {
        push(&mut snap, &data);
        if let Err(e) = unsafe { FindNextFileW(handle.0, &mut data) } {
            if e.code() == ERROR_NO_MORE_FILES.to_hresult() {
                return Ok(snap);
            }
            return Err(to_io(e));
        }
    }
}

/// Keeps the Win32 error code so `io::ErrorKind` (NotFound, PermissionDenied, ...) is right.
fn to_io(e: windows::core::Error) -> io::Error {
    let hr = e.code().0 as u32;
    if hr & 0xFFFF_0000 == 0x8007_0000 {
        io::Error::from_raw_os_error((hr & 0xFFFF) as i32)
    } else {
        io::Error::other(e)
    }
}

fn push(snap: &mut Snapshot, d: &WIN32_FIND_DATAW) {
    let len = d.cFileName.iter().position(|&c| c == 0).unwrap_or(d.cFileName.len());
    let name = &d.cFileName[..len];
    if name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16] {
        return;
    }
    snap.add(
        name,
        d.dwFileAttributes,
        (d.nFileSizeHigh as u64) << 32 | d.nFileSizeLow as u64,
        (d.ftLastWriteTime.dwHighDateTime as u64) << 32 | d.ftLastWriteTime.dwLowDateTime as u64,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Fresh empty dir under %TEMP%, removed on drop.
    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("fep-test-{}-{n}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(s: &Snapshot) -> Vec<String> {
        let mut v: Vec<String> = (0..s.len()).map(|i| s.name_string(i)).collect();
        v.sort();
        v
    }

    #[test]
    fn verbatim_prefixes_drive_and_unc_paths() {
        assert_eq!(String::from_utf16(&verbatim(Path::new(r"C:\x"))).unwrap(), r"\\?\C:\x");
        assert_eq!(
            String::from_utf16(&verbatim(Path::new(r"\\srv\share\d"))).unwrap(),
            r"\\?\UNC\srv\share\d"
        );
        assert_eq!(String::from_utf16(&verbatim(Path::new(r"\\?\C:\x"))).unwrap(), r"\\?\C:\x");
    }

    #[test]
    fn lists_files_and_dirs_without_dot_entries() {
        let t = TempDir::new();
        fs::write(t.0.join("a.txt"), "abc").unwrap();
        fs::create_dir(t.0.join("sub")).unwrap();
        fs::write(t.0.join("ünï😀.txt"), "").unwrap();

        let s = list_dir(&t.0).unwrap();
        assert_eq!(names(&s), ["a.txt", "sub", "ünï😀.txt"]);
        let a = (0..s.len()).find(|&i| s.name_string(i) == "a.txt").unwrap();
        let sub = (0..s.len()).find(|&i| s.name_string(i) == "sub").unwrap();
        assert_eq!(s.entries[a].size, 3);
        assert!(!s.is_dir(a) && s.is_dir(sub));
        assert!(s.entries[a].modified > 0);
    }

    #[test]
    fn empty_dir_is_ok_and_empty() {
        let t = TempDir::new();
        assert_eq!(list_dir(&t.0).unwrap().len(), 0);
    }

    #[test]
    fn missing_dir_is_not_found() {
        let t = TempDir::new();
        let err = list_dir(&t.0.join("nope")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn protected_dir_is_permission_denied() {
        let err = list_dir(Path::new(r"C:\System Volume Information")).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn lists_paths_longer_than_max_path() {
        let t = TempDir::new();
        let deep = t.0.join("d".repeat(120)).join("e".repeat(120)).join("f".repeat(120));
        fs::create_dir_all(&deep).unwrap();
        fs::write(deep.join("x"), "").unwrap();
        assert!(deep.as_os_str().len() > 260);
        assert_eq!(names(&list_dir(&deep).unwrap()), ["x"]);
    }
}
