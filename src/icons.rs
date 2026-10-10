//! Icons and type names. The UI asks `Icons` while painting (only visible rows
//! are painted, so only visible rows are looked up); anything unknown is
//! queued for one STA worker thread that calls the shell.

use std::collections::{HashMap, HashSet};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
};
use windows::Win32::UI::Controls::HIMAGELIST;
use windows::Win32::UI::Shell::{
    SHFILEINFOW, SHGFI_FLAGS, SHGFI_SMALLICON, SHGFI_SYSICONINDEX, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW,
};
use windows::core::PCWSTR;

/// Types whose icon differs per file, so the extension's icon isn't enough.
const OWN_ICON: [&str; 6] = [".exe", ".ico", ".lnk", ".url", ".cpl", ".scr"];

#[derive(Debug, PartialEq)]
pub enum Job {
    Ext(String),
    File {
        generation: u64,
        entry: u32,
        path: PathBuf,
    },
    /// A sidebar node's own icon (drives, special folders).
    Node {
        node: usize,
        path: PathBuf,
    },
}

#[derive(Debug, PartialEq)]
pub enum Done {
    Ext { ext: String, icon: i32, type_name: String },
    File { generation: u64, entry: u32, icon: i32 },
    Node { node: usize, icon: i32 },
}

/// Lowercase extension including the dot, Windows-style: ".gitignore" is all extension.
pub fn extension(name: &str) -> Option<String> {
    let dot = name.rfind('.')?;
    (dot + 1 < name.len()).then(|| name[dot..].to_lowercase())
}

pub fn needs_own_icon(ext: &str) -> bool {
    OWN_ICON.contains(&ext)
}

/// UI-thread side: what is known so far, and what has been asked for.
pub struct Icons {
    tx: Sender<Job>,
    pub folder: i32,
    ext: HashMap<String, (i32, String)>,
    pending_ext: HashSet<String>,
    files: HashMap<u32, i32>,
    pending_files: HashSet<u32>,
    /// Generation the per-file caches belong to; the worker skips older jobs.
    pub latest: Arc<AtomicU64>,
}

impl Icons {
    pub fn new(tx: Sender<Job>, folder: i32) -> Self {
        Icons {
            tx,
            folder,
            ext: HashMap::new(),
            pending_ext: HashSet::new(),
            files: HashMap::new(),
            pending_files: HashSet::new(),
            latest: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Icon for a row; queues lookups the first time something is unknown.
    pub fn icon(&mut self, generation: u64, entry: u32, name: &[u16], is_dir: bool, dir: &Path) -> i32 {
        if is_dir {
            return self.folder;
        }
        if generation != self.latest.load(Ordering::SeqCst) {
            self.latest.store(generation, Ordering::SeqCst);
            self.files.clear();
            self.pending_files.clear();
        }
        let name = String::from_utf16_lossy(name);
        let Some(ext) = extension(&name) else { return 0 };
        let base = self.ext_entry(&ext).map_or(0, |e| e.0);
        if !needs_own_icon(&ext) {
            return base;
        }
        if let Some(&icon) = self.files.get(&entry) {
            return icon;
        }
        if self.pending_files.insert(entry) {
            let _ = self.tx.send(Job::File { generation, entry, path: dir.join(&name) });
        }
        base
    }

    pub fn type_name(&mut self, name: &[u16], is_dir: bool) -> Option<String> {
        if is_dir {
            return Some("File folder".into());
        }
        let ext = extension(&String::from_utf16_lossy(name))?;
        self.ext_entry(&ext).map(|e| e.1.clone())
    }

    fn ext_entry(&mut self, ext: &str) -> Option<&(i32, String)> {
        if !self.ext.contains_key(ext) && self.pending_ext.insert(ext.to_string()) {
            let _ = self.tx.send(Job::Ext(ext.to_string()));
        }
        self.ext.get(ext)
    }

    /// Stores a worker result. Returns true if the list should repaint.
    pub fn on_done(&mut self, done: Done) -> bool {
        match done {
            Done::Ext { ext, icon, type_name } => {
                self.ext.insert(ext, (icon, type_name));
                true
            }
            Done::File { generation, entry, icon } if generation == self.latest.load(Ordering::SeqCst) => {
                self.files.insert(entry, icon);
                true
            }
            Done::File { .. } | Done::Node { .. } => false,
        }
    }
}

fn shfileinfo(
    path: &[u16],
    attrs: FILE_FLAGS_AND_ATTRIBUTES,
    flags: SHGFI_FLAGS,
) -> Option<(usize, SHFILEINFOW)> {
    let mut sfi = SHFILEINFOW::default();
    let path: Vec<u16> = path.iter().copied().chain([0]).collect();
    let r = unsafe {
        SHGetFileInfoW(PCWSTR(path.as_ptr()), attrs, Some(&mut sfi), size_of::<SHFILEINFOW>() as u32, flags)
    };
    (r != 0).then_some((r, sfi))
}

/// Generic icon and type name for an extension. Never opens a file.
pub fn ext_info(ext: &str) -> Option<(i32, String)> {
    let name: Vec<u16> = format!("x{ext}").encode_utf16().collect();
    let flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES;
    let (_, sfi) = shfileinfo(&name, FILE_ATTRIBUTE_NORMAL, flags)?;
    let len = sfi.szTypeName.iter().position(|&c| c == 0).unwrap_or(0);
    Some((sfi.iIcon, String::from_utf16_lossy(&sfi.szTypeName[..len])))
}

/// The icon of this particular file or folder. Reads it: worker threads only.
pub fn file_icon(path: &Path) -> Option<i32> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    let (_, sfi) = shfileinfo(&wide, FILE_FLAGS_AND_ATTRIBUTES(0), SHGFI_SYSICONINDEX | SHGFI_SMALLICON)?;
    Some(sfi.iIcon)
}

/// The process's small system image list and the generic folder icon in it.
/// Set up once per process: the shell's first-use setup isn't safe to race.
pub fn system_image_list() -> Option<(HIMAGELIST, i32)> {
    static LIST: std::sync::OnceLock<Option<(isize, i32)>> = std::sync::OnceLock::new();
    let (list, folder) = (*LIST.get_or_init(|| {
        let name: Vec<u16> = "folder".encode_utf16().collect();
        let flags = SHGFI_SYSICONINDEX | SHGFI_SMALLICON | SHGFI_USEFILEATTRIBUTES;
        let (list, sfi) = shfileinfo(&name, FILE_ATTRIBUTE_DIRECTORY, flags)?;
        Some((list as isize, sfi.iIcon))
    }))?;
    Some((HIMAGELIST(list), folder))
}

/// Runs jobs until every sender is gone. Per-file jobs for an older folder are skipped.
// ponytail: FIFO; if scrolling a huge folder of .exe files lags, drop jobs for rows no longer visible.
pub fn spawn_worker(
    rx: Receiver<Job>,
    latest: Arc<AtomicU64>,
    send: impl Fn(Done) + Send + 'static,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("icons".into())
        .spawn(move || {
            let _com = crate::shell::Com::init();
            for job in rx {
                let done = crate::crash::guard(|| match job {
                    Job::Ext(ext) => {
                        let (icon, type_name) = ext_info(&ext).unwrap_or((0, String::new()));
                        Some(Done::Ext { ext, icon, type_name })
                    }
                    Job::File { generation, .. } if generation != latest.load(Ordering::SeqCst) => None,
                    Job::File { generation, entry, path } => {
                        file_icon(&path).map(|icon| Done::File { generation, entry, icon })
                    }
                    Job::Node { node, path } => file_icon(&path).map(|icon| Done::Node { node, icon }),
                });
                if let Some(Some(d)) = done {
                    send(d);
                }
            }
        })
        .expect("start icon thread")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn extensions_are_lowercase_after_the_last_dot() {
        assert_eq!(extension("Report.TXT").as_deref(), Some(".txt"));
        assert_eq!(extension("a.tar.gz").as_deref(), Some(".gz"));
        assert_eq!(extension(".gitignore").as_deref(), Some(".gitignore"));
        assert_eq!(extension("Makefile"), None);
        assert_eq!(extension("odd."), None);
    }

    #[test]
    fn only_some_types_have_per_file_icons() {
        assert!(needs_own_icon(".exe") && needs_own_icon(".lnk") && needs_own_icon(".ico"));
        assert!(!needs_own_icon(".txt") && !needs_own_icon(".exe.txt"));
    }

    #[test]
    fn same_extension_is_looked_up_once() {
        let (tx, rx) = mpsc::channel();
        let mut icons = Icons::new(tx, 7);
        let dir = Path::new(r"C:\d");
        assert_eq!(icons.icon(1, 0, &w("a.txt"), false, dir), 0);
        assert_eq!(icons.icon(1, 1, &w("b.TXT"), false, dir), 0);
        assert_eq!(icons.icon(1, 2, &w("sub"), true, dir), 7);
        assert_eq!(icons.type_name(&w("c.txt"), false), None);
        assert_eq!(icons.type_name(&w("sub"), true).as_deref(), Some("File folder"));
        let jobs: Vec<Job> = rx.try_iter().collect();
        assert_eq!(jobs, [Job::Ext(".txt".into())]);

        assert!(icons.on_done(Done::Ext { ext: ".txt".into(), icon: 42, type_name: "Text Document".into() }));
        assert_eq!(icons.icon(1, 0, &w("a.txt"), false, dir), 42);
        assert_eq!(icons.type_name(&w("c.txt"), false).as_deref(), Some("Text Document"));
    }

    #[test]
    fn programs_get_their_own_icon_per_generation() {
        let (tx, rx) = mpsc::channel();
        let mut icons = Icons::new(tx, 7);
        let dir = Path::new(r"C:\d");
        icons.icon(1, 5, &w("app.exe"), false, dir);
        icons.icon(1, 5, &w("app.exe"), false, dir);
        let jobs: Vec<Job> = rx.try_iter().collect();
        let file = Job::File { generation: 1, entry: 5, path: dir.join("app.exe") };
        assert_eq!(jobs, [Job::Ext(".exe".into()), file]);

        assert!(icons.on_done(Done::File { generation: 1, entry: 5, icon: 99 }));
        assert_eq!(icons.icon(1, 5, &w("app.exe"), false, dir), 99);
        // A stale result is ignored, and a new folder forgets per-file icons.
        assert!(!icons.on_done(Done::File { generation: 0, entry: 6, icon: 1 }));
        assert_ne!(icons.icon(2, 5, &w("app.exe"), false, dir), 99);
        assert_eq!(icons.latest.load(Ordering::SeqCst), 2);
    }

    /// The shell sets up its image list on first use, and two threads doing that
    /// at once can fail. The app does it on the UI thread before starting the
    /// worker; tests do the same once.
    fn shell_ready() -> crate::shell::Com {
        static INIT: std::sync::Once = std::sync::Once::new();
        let com = crate::shell::Com::init();
        INIT.call_once(|| {
            system_image_list().unwrap();
        });
        com
    }

    #[test]
    fn shell_lookups_return_real_data() {
        let _com = shell_ready();
        let (icon, type_name) = ext_info(".txt").unwrap();
        assert!(icon > 0 && !type_name.is_empty(), "{icon} {type_name}");
        let notepad = Path::new(&std::env::var("SystemRoot").unwrap()).join("notepad.exe");
        assert!(file_icon(&notepad).unwrap() > 0);
        let (list, folder) = system_image_list().unwrap();
        assert!(!list.is_invalid() && folder > 0);
    }

    #[test]
    fn worker_answers_jobs_and_skips_stale_files() {
        let (tx, rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let _com = shell_ready();
        let latest = Arc::new(AtomicU64::new(3));
        let worker = spawn_worker(rx, latest, move |d| done_tx.send(d).unwrap());
        tx.send(Job::File { generation: 2, entry: 0, path: r"C:\Windows\notepad.exe".into() }).unwrap();
        tx.send(Job::Ext(".txt".into())).unwrap();
        drop(tx);
        worker.join().unwrap();
        let done: Vec<Done> = done_rx.try_iter().collect();
        assert_eq!(done.len(), 1, "{done:?}");
        assert!(matches!(&done[0], Done::Ext { ext, .. } if ext == ".txt"));
    }
}
