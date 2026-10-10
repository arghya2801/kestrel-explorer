use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use file_explorer_pro::ui;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_BACK;
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

/// Sends the WM_NOTIFY a ListView would send to its parent.
fn notify<T>(main: HWND, mut payload: T) {
    unsafe { SendMessageW(main, WM_NOTIFY, Some(WPARAM(0)), Some(LPARAM(&mut payload as *mut T as isize))) };
}

fn hdr(list: HWND, code: u32) -> NMHDR {
    NMHDR { hwndFrom: list, idFrom: 0, code }
}

fn rows(list: HWND) -> Vec<String> {
    (0..item_count(list)).map(|r| item_text(list, r)).collect()
}

#[test]
fn column_click_sorts_by_size_then_toggles() {
    let t = TempDir::new("sort");
    std::fs::write(t.0.join("a.txt"), "xxx").unwrap();
    std::fs::write(t.0.join("b.txt"), "x").unwrap();
    std::fs::write(t.0.join("c.txt"), "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx".repeat(40))
        .unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(Duration::from_secs(5), || item_count(list) == 3));

    let click = |col| {
        notify(hwnd, NMLISTVIEW { hdr: hdr(list, LVN_COLUMNCLICK), iSubItem: col, ..Default::default() })
    };
    click(2); // Size ascending, by bytes: b (1) < a (3) < c
    assert!(
        pump_until(Duration::from_secs(5), || rows(list) == ["b.txt", "a.txt", "c.txt"]),
        "{:?}",
        rows(list)
    );
    click(2); // same column again: descending
    assert!(
        pump_until(Duration::from_secs(5), || rows(list) == ["c.txt", "a.txt", "b.txt"]),
        "{:?}",
        rows(list)
    );
    unsafe { DestroyWindow(hwnd) }.unwrap();
}

#[test]
fn activating_a_folder_opens_it_and_backspace_goes_up() {
    let t = TempDir::new("nav");
    std::fs::create_dir(t.0.join("inner")).unwrap();
    std::fs::write(t.0.join("inner").join("deep.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(Duration::from_secs(5), || rows(list) == ["inner"]));

    notify(hwnd, NMITEMACTIVATE { hdr: hdr(list, LVN_ITEMACTIVATE), iItem: 0, ..Default::default() });
    assert!(pump_until(Duration::from_secs(5), || rows(list) == ["deep.txt"]), "{:?}", rows(list));

    notify(hwnd, NMLVKEYDOWN { hdr: hdr(list, LVN_KEYDOWN), wVKey: VK_BACK.0, flags: 0 });
    assert!(pump_until(Duration::from_secs(5), || rows(list) == ["inner"]), "{:?}", rows(list));
    unsafe { DestroyWindow(hwnd) }.unwrap();
}
