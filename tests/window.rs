//! Drives the real window: creates it on a temp folder, sends the messages a
//! user's input would produce, and pumps until the background work shows up.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use file_explorer_pro::{places, ui};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PWSTR, w};

const WAIT: Duration = Duration::from_secs(5);

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("fep-ui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        // Long form, so it compares equal to paths the shell hands back.
        TempDir(dunce(std::fs::canonicalize(&p).unwrap()))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Strips the `\\?\` that canonicalize adds.
fn dunce(p: PathBuf) -> PathBuf {
    PathBuf::from(p.display().to_string().trim_start_matches(r"\\?\"))
}

/// Runs this thread's message loop (with the app's shortcuts) until `done` or the timeout.
fn pump_until(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    let mut msg = MSG::default();
    while start.elapsed() < timeout {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if !ui::translate(&msg) {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn find(main: HWND, class: windows::core::PCWSTR) -> HWND {
    unsafe { FindWindowExW(Some(main), None, class, None) }.unwrap()
}

fn list_of(main: HWND) -> HWND {
    find(main, w!("SysListView32"))
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

fn rows(list: HWND) -> Vec<String> {
    (0..item_count(list)).map(|r| item_text(list, r)).collect()
}

fn selected(list: HWND) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = -1isize;
    loop {
        i = unsafe {
            SendMessageW(
                list,
                LVM_GETNEXTITEM,
                Some(WPARAM(i as usize)),
                Some(LPARAM(LVNI_SELECTED as isize)),
            )
        }
        .0;
        if i < 0 {
            return out;
        }
        out.push(item_text(list, i as usize));
    }
}

fn status_text(main: HWND) -> String {
    let status = find(main, w!("msctls_statusbar32"));
    let mut buf = [0u16; 1024];
    unsafe { SendMessageW(status, SB_GETTEXTW, Some(WPARAM(0)), Some(LPARAM(buf.as_mut_ptr() as isize))) };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&buf[..n])
}

fn key(to: HWND, vk: VIRTUAL_KEY) {
    unsafe { PostMessageW(Some(to), WM_KEYDOWN, WPARAM(vk.0 as usize), LPARAM(0)) }.unwrap();
}

fn alt_key(to: HWND, vk: VIRTUAL_KEY) {
    unsafe { PostMessageW(Some(to), WM_SYSKEYDOWN, WPARAM(vk.0 as usize), LPARAM(1 << 29)) }.unwrap();
}

/// Sends the WM_NOTIFY a ListView would send to its parent.
fn notify<T>(main: HWND, mut payload: T) {
    unsafe { SendMessageW(main, WM_NOTIFY, Some(WPARAM(0)), Some(LPARAM(&mut payload as *mut T as isize))) };
}

fn hdr(list: HWND, code: u32) -> NMHDR {
    NMHDR { hwndFrom: list, idFrom: 0, code }
}

fn activate(main: HWND, list: HWND, row: i32) {
    notify(main, NMITEMACTIVATE { hdr: hdr(list, LVN_ITEMACTIVATE), iItem: row, ..Default::default() });
}

fn open(dir: &Path) -> HWND {
    ui::init_process().unwrap();
    ui::create_main_window(dir).unwrap()
}

fn close(hwnd: HWND) {
    unsafe { DestroyWindow(hwnd) }.unwrap();
}

#[test]
fn main_window_creates_and_destroys() {
    let t = TempDir::new("create");
    let hwnd = open(&t.0);
    assert!(unsafe { IsWindow(Some(hwnd)) }.as_bool());
    close(hwnd);
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
    assert!(pump_until(WAIT, || item_count(list) == 3), "folder never loaded");
    assert_eq!(rows(list), ["zdir", "a.txt", "b.txt"]);
    close(hwnd);
}

#[test]
fn column_click_sorts_by_size_then_toggles() {
    let t = TempDir::new("sort");
    std::fs::write(t.0.join("a.txt"), "xxx").unwrap();
    std::fs::write(t.0.join("b.txt"), "x").unwrap();
    std::fs::write(t.0.join("c.txt"), "x".repeat(5000)).unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || item_count(list) == 3));

    let click = |col| {
        notify(hwnd, NMLISTVIEW { hdr: hdr(list, LVN_COLUMNCLICK), iSubItem: col, ..Default::default() })
    };
    click(3); // Size ascending, by bytes: b (1) < a (3) < c
    assert!(pump_until(WAIT, || rows(list) == ["b.txt", "a.txt", "c.txt"]), "{:?}", rows(list));
    click(3); // same column again: descending
    assert!(pump_until(WAIT, || rows(list) == ["c.txt", "a.txt", "b.txt"]), "{:?}", rows(list));
    close(hwnd);
}

#[test]
fn opening_a_folder_then_backspace_returns_and_selects_it() {
    let t = TempDir::new("nav");
    std::fs::create_dir(t.0.join("first")).unwrap();
    std::fs::create_dir(t.0.join("inner")).unwrap();
    std::fs::write(t.0.join("inner").join("deep.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["first", "inner"]));

    activate(hwnd, list, 1);
    assert!(pump_until(WAIT, || rows(list) == ["deep.txt"]), "{:?}", rows(list));

    key(list, VK_BACK);
    assert!(pump_until(WAIT, || rows(list) == ["first", "inner"]), "{:?}", rows(list));
    assert_eq!(selected(list), ["inner"]);
    close(hwnd);
}

#[test]
fn alt_arrows_go_back_forward_and_up() {
    let t = TempDir::new("alt");
    std::fs::create_dir_all(t.0.join("a").join("b")).unwrap();
    let hwnd = open(&t.0.join("a"));
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["b"]));

    alt_key(list, VK_UP);
    assert!(pump_until(WAIT, || rows(list) == ["a"]), "up: {:?}", rows(list));
    assert_eq!(selected(list), ["a"]);
    alt_key(list, VK_LEFT);
    assert!(pump_until(WAIT, || rows(list) == ["b"]), "back: {:?}", rows(list));
    alt_key(list, VK_RIGHT);
    assert!(pump_until(WAIT, || rows(list) == ["a"]), "forward: {:?}", rows(list));
    close(hwnd);
}

#[test]
fn address_bar_enter_navigates_and_rejects_nonsense() {
    let t = TempDir::new("addr");
    std::fs::create_dir(t.0.join("there")).unwrap();
    std::fs::write(t.0.join("there").join("file.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    let edit = find(hwnd, w!("Edit"));
    assert!(pump_until(WAIT, || rows(list) == ["there"]));

    let type_and_enter = |text: &str| {
        unsafe { SetWindowTextW(edit, &HSTRING::from(text)) }.unwrap();
        key(edit, VK_RETURN);
    };
    type_and_enter(&format!("  \"{}\"  ", t.0.join("there").display()));
    assert!(pump_until(WAIT, || rows(list) == ["file.txt"]), "{:?}", rows(list));

    type_and_enter("..");
    assert!(pump_until(WAIT, || rows(list) == ["there"]), "{:?}", rows(list));

    type_and_enter("   ");
    assert!(pump_until(WAIT, || status_text(hwnd).starts_with("Not a location")), "{}", status_text(hwnd));
    close(hwnd);
}

#[test]
fn escape_abandons_a_hanging_network_folder() {
    let t = TempDir::new("esc");
    std::fs::write(t.0.join("still here.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    let edit = find(hwnd, w!("Edit"));
    assert!(pump_until(WAIT, || rows(list) == ["still here.txt"]));

    // Nothing answers at this address, so listing it blocks for a long time.
    unsafe { SetWindowTextW(edit, w!(r"\\10.255.255.1\nothing")) }.unwrap();
    key(edit, VK_RETURN);
    assert!(pump_until(WAIT, || status_text(hwnd).starts_with("Loading")), "{}", status_text(hwnd));

    key(list, VK_ESCAPE);
    assert!(pump_until(WAIT, || status_text(hwnd).starts_with("Stopped loading")), "{}", status_text(hwnd));
    assert_eq!(rows(list), ["still here.txt"]);
    close(hwnd);
}

#[test]
fn ctrl_h_toggles_hidden_files() {
    let t = TempDir::new("hidden");
    std::fs::write(t.0.join("seen.txt"), "").unwrap();
    let secret = t.0.join("secret.txt");
    std::fs::write(&secret, "").unwrap();
    unsafe { SetFileAttributesW(&HSTRING::from(secret.as_os_str()), FILE_ATTRIBUTE_HIDDEN) }.unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["seen.txt"]), "{:?}", rows(list));

    let ctrl_h = || unsafe {
        let mut keys = [0u8; 256];
        GetKeyboardState(&mut keys).unwrap();
        keys[VK_CONTROL.0 as usize] = 0x80;
        SetKeyboardState(&keys).unwrap();
        key(list, VK_H);
        pump_until(Duration::from_millis(50), || false);
        keys[VK_CONTROL.0 as usize] = 0;
        SetKeyboardState(&keys).unwrap();
    };
    ctrl_h();
    assert!(pump_until(WAIT, || rows(list) == ["secret.txt", "seen.txt"]), "{:?}", rows(list));
    ctrl_h();
    assert!(pump_until(WAIT, || rows(list) == ["seen.txt"]), "{:?}", rows(list));
    close(hwnd);
}

#[test]
fn shortcut_to_a_folder_opens_inside_the_app() {
    let t = TempDir::new("lnk");
    let target = t.0.join("target");
    std::fs::create_dir(&target).unwrap();
    std::fs::write(target.join("inside.txt"), "").unwrap();
    let lnk = t.0.join("go.lnk");
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath='{}'; $s.Save()",
        lnk.display(),
        target.display()
    );
    let ok = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).status();
    assert!(ok.unwrap().success() && lnk.exists());

    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["target", "go.lnk"]), "{:?}", rows(list));
    activate(hwnd, list, 1);
    assert!(pump_until(WAIT, || rows(list) == ["inside.txt"]), "{:?}", rows(list));
    close(hwnd);
}

// ---- sidebar ----

fn tree_of(main: HWND) -> HWND {
    find(main, w!("SysTreeView32"))
}

fn tv_next(tree: HWND, flag: u32, item: isize) -> isize {
    unsafe { SendMessageW(tree, TVM_GETNEXTITEM, Some(WPARAM(flag as usize)), Some(LPARAM(item))) }.0
}

fn tv_item(tree: HWND, item: isize) -> (String, isize) {
    let mut buf = [0u16; 260];
    let mut tv = TVITEMW {
        mask: TVIF_TEXT | TVIF_PARAM,
        hItem: HTREEITEM(item),
        pszText: PWSTR(buf.as_mut_ptr()),
        cchTextMax: buf.len() as i32,
        ..Default::default()
    };
    unsafe { SendMessageW(tree, TVM_GETITEMW, None, Some(LPARAM(&mut tv as *mut _ as isize))) };
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    (String::from_utf16_lossy(&buf[..n]), tv.lParam.0)
}

/// Child items of `parent` (0 = roots) as (text, handle).
fn tv_children(tree: HWND, parent: isize) -> Vec<(String, isize)> {
    let mut out = Vec::new();
    let mut it = if parent == 0 { tv_next(tree, TVGN_ROOT, 0) } else { tv_next(tree, TVGN_CHILD, parent) };
    while it != 0 {
        out.push((tv_item(tree, it).0, it));
        it = tv_next(tree, TVGN_NEXT, it);
    }
    out
}

fn system_drive_node(tree: HWND) -> isize {
    let pc = tv_children(tree, 0).into_iter().find(|(t, _)| t == "This PC").expect("This PC").1;
    let sys = format!("({})", std::env::var("SystemDrive").unwrap());
    tv_children(tree, pc).into_iter().find(|(t, _)| t.ends_with(&sys)).expect("system drive").1
}

#[test]
fn sidebar_lists_quick_access_and_drives_with_labels() {
    let t = TempDir::new("side");
    let hwnd = open(&t.0);
    let tree = tree_of(hwnd);
    let roots: Vec<String> = tv_children(tree, 0).into_iter().map(|r| r.0).collect();
    assert_eq!(roots, ["Quick access", "This PC"]);
    let quick = tv_children(tree, 0)[0].1;
    assert_eq!(tv_children(tree, quick)[0].0, "Desktop");
    let c = system_drive_node(tree);
    // The generic name is replaced by the volume label once the label thread answers.
    let sys = std::env::var("SystemDrive").unwrap();
    let label = places::volume_label(Path::new(&format!("{sys}\\"))).unwrap();
    let letter = sys.chars().next().unwrap();
    let expected = places::drive_name(letter, places::DriveKind::Fixed, Some(&label));
    assert!(pump_until(WAIT, || tv_item(tree, c).0 == expected), "{:?} != {expected}", tv_item(tree, c).0);
    close(hwnd);
}

#[test]
fn expanding_a_drive_loads_folders_and_clicking_one_shows_it() {
    let t = TempDir::new("expand");
    let hwnd = open(&t.0);
    let tree = tree_of(hwnd);
    let list = list_of(hwnd);
    let c = system_drive_node(tree);
    assert!(tv_children(tree, c).is_empty(), "loaded before expanding");

    unsafe { SendMessageW(tree, TVM_EXPAND, Some(WPARAM(TVE_EXPAND.0 as usize)), Some(LPARAM(c))) };
    let windows = || tv_children(tree, c).into_iter().find(|(t, _)| t.eq_ignore_ascii_case("Windows"));
    assert!(pump_until(WAIT, || windows().is_some()), "{:?}", tv_children(tree, c));

    let (_, item) = windows().unwrap();
    let (_, node) = tv_item(tree, item);
    let mut nm = NMTREEVIEWW { hdr: hdr(tree, TVN_SELCHANGEDW), action: TVC_BYMOUSE, ..Default::default() };
    nm.itemNew.hItem = HTREEITEM(item);
    nm.itemNew.lParam = LPARAM(node);
    notify(hwnd, nm);
    assert!(pump_until(WAIT, || rows(list).iter().any(|r| r.eq_ignore_ascii_case("System32"))));
    close(hwnd);
}

#[test]
fn dragging_the_splitter_resizes_the_sidebar() {
    let t = TempDir::new("split");
    let hwnd = open(&t.0);
    let tree = tree_of(hwnd);
    let width = || {
        let mut r = windows::Win32::Foundation::RECT::default();
        unsafe { GetWindowRect(tree, &mut r) }.unwrap();
        r.right - r.left
    };
    let start = width();
    let mut tr = windows::Win32::Foundation::RECT::default();
    unsafe { GetWindowRect(tree, &mut tr) }.unwrap();
    let mut pt = windows::Win32::Foundation::POINT { x: tr.right + 1, y: tr.top + 50 };
    unsafe { windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut pt) }.unwrap();
    let at = |x: i32, y: i32| LPARAM(((y as isize) << 16) | (x as isize & 0xFFFF));
    unsafe {
        SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(at(pt.x, pt.y)));
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(at(pt.x + 100, pt.y)));
        SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(at(pt.x + 100, pt.y)));
    }
    assert_eq!(width(), start + 100);
    unsafe {
        SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(at(pt.x + 100, pt.y)));
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(at(0, pt.y)));
        SendMessageW(hwnd, WM_LBUTTONUP, Some(WPARAM(0)), Some(at(0, pt.y)));
    }
    assert!(width() > 50, "sidebar collapsed to {}", width());
    close(hwnd);
}

#[test]
fn tab_cycles_address_sidebar_list() {
    let t = TempDir::new("tab");
    let hwnd = open(&t.0);
    let (edit, tree, list) = (find(hwnd, w!("Edit")), tree_of(hwnd), list_of(hwnd));
    unsafe { SetFocus(Some(edit)) }.unwrap();
    key(edit, VK_TAB);
    pump_until(Duration::from_millis(100), || false);
    assert_eq!(unsafe { GetFocus() }, tree);
    key(tree, VK_TAB);
    pump_until(Duration::from_millis(100), || false);
    assert_eq!(unsafe { GetFocus() }, list);
    key(list, VK_TAB);
    pump_until(Duration::from_millis(100), || false);
    assert_eq!(unsafe { GetFocus() }, edit);
    close(hwnd);
}

// ---- icons and types ----

fn item_image(list: HWND, row: usize) -> i32 {
    let mut item = LVITEMW { mask: LVIF_IMAGE, iItem: row as i32, ..Default::default() };
    unsafe { SendMessageW(list, LVM_GETITEMW, None, Some(LPARAM(&mut item as *mut _ as isize))) };
    item.iImage
}

fn cell(list: HWND, row: usize, col: i32) -> String {
    let mut buf = [0u16; 260];
    let mut item = LVITEMW {
        iSubItem: col,
        pszText: PWSTR(buf.as_mut_ptr()),
        cchTextMax: buf.len() as i32,
        ..Default::default()
    };
    let n = unsafe {
        SendMessageW(list, LVM_GETITEMTEXTW, Some(WPARAM(row)), Some(LPARAM(&mut item as *mut _ as isize)))
    };
    String::from_utf16_lossy(&buf[..n.0 as usize])
}

#[test]
fn rows_get_type_names_and_icons() {
    let t = TempDir::new("icons");
    std::fs::create_dir(t.0.join("folder")).unwrap();
    std::fs::write(t.0.join("notes.txt"), "").unwrap();
    std::fs::copy(Path::new(&std::env::var("SystemRoot").unwrap()).join("notepad.exe"), t.0.join("app.exe"))
        .unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["folder", "app.exe", "notes.txt"]), "{:?}", rows(list));

    assert_eq!(cell(list, 0, 2), "File folder");
    // Text Document's name is localized, so just wait for it to be filled in.
    assert!(pump_until(WAIT, || !cell(list, 2, 2).is_empty() && !cell(list, 1, 2).is_empty()));
    // Folder, program and text file each end up with a distinct icon; the .exe gets its own.
    assert!(
        pump_until(WAIT, || {
            let (f, a, n) = (item_image(list, 0), item_image(list, 1), item_image(list, 2));
            f != a && a != n && f != n && n > 0
        }),
        "{:?}",
        (0..3).map(|r| item_image(list, r)).collect::<Vec<_>>()
    );
    close(hwnd);
}
