//! Watches one folder for changes on a background thread.
//!
//! Overlapped `ReadDirectoryChangesW` plus a stop event: stopping never races
//! with the thread entering a blocking call. The folder is opened with
//! `FILE_SHARE_DELETE`, so watching never stops anyone renaming or deleting it.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_NOTIFY_ENUM_DIR, HANDLE, WAIT_OBJECT_0};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED, FILE_LIST_DIRECTORY,
    FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME,
    FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING, ReadDirectoryChangesW,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{
    CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects, WaitForSingleObject,
};
use windows::core::PCWSTR;

use crate::listing::verbatim;

/// Changes closer together than this are reported once.
const DEBOUNCE: Duration = Duration::from_millis(100);
/// 64 KB: the most `ReadDirectoryChangesW` accepts for network folders.
const BUFFER_WORDS: usize = 64 * 1024 / 4;

pub struct Watcher {
    stop: Arc<OwnedHandle>,
    finished: Receiver<()>,
}

impl Watcher {
    /// Watches `dir`. `on_change` runs on the watcher thread after each burst of
    /// changes, and once more if the folder disappears. `on_ready` gets the
    /// folder handle once it is open (for device-removal notifications).
    pub fn start(
        dir: PathBuf,
        on_change: impl Fn() + Send + 'static,
        on_ready: impl FnOnce(isize) + Send + 'static,
    ) -> Watcher {
        let stop = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
            .map(|h| Arc::new(unsafe { OwnedHandle::from_raw_handle(h.0) }));
        let (done_tx, finished) = mpsc::channel();
        let Ok(stop) = stop else {
            return Watcher { stop: Arc::new(null_handle()), finished };
        };
        let stop2 = stop.clone();
        let spawned = std::thread::Builder::new().name("watch".into()).spawn(move || {
            let _done = done_tx; // dropped when the thread ends: `finished` disconnects
            crate::crash::guard(|| run(&dir, &stop2, on_change, on_ready));
        });
        if let Err(e) = spawned {
            crate::crash::log(&format!("no live updates for this folder: {e}"));
        }
        Watcher { stop, finished }
    }

    /// Stops the thread and waits up to `timeout` for it to close the folder.
    /// Returns true if it did. Needed before a USB drive can be ejected.
    pub fn stop_and_wait(self, timeout: Duration) -> bool {
        self.signal_stop();
        matches!(self.finished.recv_timeout(timeout), Err(mpsc::RecvTimeoutError::Disconnected))
    }

    fn signal_stop(&self) {
        unsafe {
            let _ = SetEvent(HANDLE(self.stop.as_raw_handle()));
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.signal_stop();
    }
}

fn null_handle() -> OwnedHandle {
    // An event that is never used; keeps `Watcher` simple when CreateEventW fails.
    unsafe {
        OwnedHandle::from_raw_handle(CreateEventW(None, true, true, PCWSTR::null()).unwrap_or_default().0)
    }
}

fn run(dir: &Path, stop: &OwnedHandle, on_change: impl Fn(), on_ready: impl FnOnce(isize)) {
    let mut path = verbatim(dir);
    path.push(0);
    let Ok(folder) = (unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            FILE_LIST_DIRECTORY.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            None,
        )
    }) else {
        return;
    };
    let folder = unsafe { OwnedHandle::from_raw_handle(folder.0) };
    let Ok(done) = (unsafe { CreateEventW(None, true, false, PCWSTR::null()) }) else { return };
    let done = unsafe { OwnedHandle::from_raw_handle(done.0) };
    let (folder_h, done_h, stop_h) =
        (HANDLE(folder.as_raw_handle()), HANDLE(done.as_raw_handle()), HANDLE(stop.as_raw_handle()));
    // Reported only once the first request is queued: Windows buffers changes
    // from then on, so nothing that happens after "ready" can be missed.
    let mut on_ready = Some(on_ready);

    let filter = FILE_NOTIFY_CHANGE_FILE_NAME
        | FILE_NOTIFY_CHANGE_DIR_NAME
        | FILE_NOTIFY_CHANGE_ATTRIBUTES
        | FILE_NOTIFY_CHANGE_SIZE
        | FILE_NOTIFY_CHANGE_LAST_WRITE;
    // u32 elements: the buffer must be DWORD-aligned.
    let mut buf = vec![0u32; BUFFER_WORDS];
    loop {
        let mut ov = OVERLAPPED { hEvent: done_h, ..Default::default() };
        let started = unsafe {
            ReadDirectoryChangesW(
                folder_h,
                buf.as_mut_ptr().cast(),
                (buf.len() * 4) as u32,
                false,
                filter,
                None,
                Some(&mut ov),
                None,
            )
        };
        if started.is_err() {
            on_change(); // usually: the folder is being deleted
            return;
        }
        if let Some(ready) = on_ready.take() {
            ready(folder_h.0 as isize);
        }
        let mut bytes = 0u32;
        if unsafe { WaitForMultipleObjects(&[done_h, stop_h], false, INFINITE) } != WAIT_OBJECT_0 {
            // Stop requested (or the wait failed): cancel and let the I/O finish before the buffer goes away.
            unsafe {
                let _ = CancelIoEx(folder_h, Some(&ov));
                let _ = GetOverlappedResult(folder_h, &ov, &mut bytes, true);
            }
            return;
        }
        match unsafe { GetOverlappedResult(folder_h, &ov, &mut bytes, false) } {
            // Zero bytes or ENUM_DIR mean "too many changes to list": a re-list covers it either way.
            Ok(()) => {}
            Err(e) if e.code() == ERROR_NOTIFY_ENUM_DIR.to_hresult() => {}
            Err(_) => {
                on_change();
                return;
            }
        }
        // Collect the rest of the burst; the system keeps queueing changes meanwhile.
        if unsafe { WaitForSingleObject(stop_h, DEBOUNCE.as_millis() as u32) } == WAIT_OBJECT_0 {
            return;
        }
        on_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fep-watch-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Starts a watcher whose changes and readiness go to channels.
    fn start(dir: &Path) -> (Watcher, mpsc::Receiver<()>, mpsc::Receiver<isize>) {
        let (tx, rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let w = Watcher::start(
            dir.to_path_buf(),
            move || {
                let _ = tx.send(());
            },
            move |h| {
                let _ = ready_tx.send(h);
            },
        );
        (w, rx, ready_rx)
    }

    #[test]
    fn reports_a_new_file() {
        let d = temp("new");
        let (_w, changes, ready) = start(&d);
        ready.recv_timeout(Duration::from_secs(2)).expect("watcher opened the folder");
        fs::write(d.join("hello.txt"), "x").unwrap();
        changes.recv_timeout(Duration::from_secs(2)).expect("change reported");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn bursts_are_debounced() {
        let d = temp("burst");
        let (_w, changes, ready) = start(&d);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        for i in 0..300 {
            fs::write(d.join(format!("f{i}")), "").unwrap();
        }
        std::thread::sleep(Duration::from_millis(600));
        let n = changes.try_iter().count();
        assert!((1..=10).contains(&n), "{n} change signals for one burst");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn stopping_is_prompt() {
        let d = temp("stop");
        let (w, _changes, ready) = start(&d);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(w.stop_and_wait(Duration::from_secs(1)), "thread did not finish");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn watched_folder_can_still_be_deleted() {
        let d = temp("del");
        let (w, changes, ready) = start(&d);
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        fs::write(d.join("inside.txt"), "").unwrap();
        fs::remove_dir_all(&d).expect("watching must not block deletion");
        changes.recv_timeout(Duration::from_secs(2)).expect("deletion reported");
        assert!(w.stop_and_wait(Duration::from_secs(1)));
    }

    #[test]
    fn missing_folder_just_ends_quietly() {
        let d = temp("missing").join("nope");
        let (w, changes, ready) = start(&d);
        assert!(w.stop_and_wait(Duration::from_secs(1)));
        assert!(ready.try_recv().is_err() && changes.try_recv().is_err());
    }
}
