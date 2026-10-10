use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use file_explorer_pro::ui;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::{LVITEMW, LVM_GETITEMCOUNT, LVM_GETITEMTEXTW};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{PWSTR, w};

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("fep-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Runs this thread's message loop until `done` or the timeout.
fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let mut msg = MSG::default();
    while start.elapsed() < timeout {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn list_of(main: HWND) -> HWND {
    unsafe { FindWindowExW(Some(main), None, w!("SysListView32"), None) }.unwrap()
}

fn item_count(list: HWND) -> usize {
    unsafe { SendMessageW(list, LVM_GETITEMCOUNT, None, None) }.0 as usize
}

fn item_text(list: HWND, row: usize) -> String {
    let mut buf = [0u16; 260];
    let mut item =
        LVITEMW { pszText: PWSTR(buf.as_mut_ptr()), cchTextMax: buf.len() as i32, ..Default::default() };
    let n = unsafe {
        SendMessageW(list, LVM_GETITEMTEXTW, Some(WPARAM(row)), Some(LPARAM(&mut item as *mut _ as isize)))
    };
    String::from_utf16_lossy(&buf[..n.0 as usize])
}

fn open(dir: &Path) -> HWND {
    ui::init_process().unwrap();
    ui::create_main_window(dir).unwrap()
}

#[test]
fn main_window_creates_and_destroys() {
    let t = TempDir::new("create");
    let hwnd = open(&t.0);
    assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
    unsafe { DestroyWindow(hwnd) }.unwrap();
    assert!(!unsafe { IsWindow(Some(hwnd)) }.as_bool());
}

#[test]
fn shows_folder_contents_folders_first() {
    let t = TempDir::new("list");
    std::fs::write(t.0.join("b.txt"), "x").unwrap();
    std::fs::write(t.0.join("a.txt"), "x").unwrap();
    std::fs::create_dir(t.0.join("zdir")).unwrap();

    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(Duration::from_secs(5), || item_count(list) == 3), "folder never loaded");
    assert_eq!((0..3).map(|r| item_text(list, r)).collect::<Vec<_>>(), ["zdir", "a.txt", "b.txt"]);
    unsafe { DestroyWindow(hwnd) }.unwrap();
}
