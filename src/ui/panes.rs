//! State owned by one folder view.
use crate::{browser::Browser, icons::Icons, watch::Watcher};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Ole::IDropTarget;

pub(super) struct FolderView {
    pub list: HWND,
    pub browser: Browser,
    pub lister: Option<JoinHandle<()>>,
    pub lister_cancel: Arc<std::sync::atomic::AtomicBool>,
    pub icons: Icons,
    pub watcher: Option<(u64, Watcher)>,
    pub device_note: Option<(isize, isize)>,
    pub edit_pending: Option<String>,
    pub renaming: Option<PathBuf>,
    pub resync_after_rename: bool,
    pub drop_row: Option<usize>,
    pub drop_target: Option<IDropTarget>,
}
