//! Drives the real window: creates it on a temp folder, sends the messages a
//! user's input would produce, and pumps until the background work shows up.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kestrel::{places, ui};
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

/// Tests that show a window or depend on keyboard focus take this, so they
/// don't steal activation from each other.
static DESKTOP: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn desktop() -> std::sync::MutexGuard<'static, ()> {
    DESKTOP.lock().unwrap_or_else(|e| e.into_inner())
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

/// Puts `text` in an edit box as if typed (typing also sets the "modified" flag).
fn type_into(edit: HWND, text: &str) {
    unsafe {
        SetWindowTextW(edit, &HSTRING::from(text)).unwrap();
        SendMessageW(edit, EM_SETMODIFY, Some(WPARAM(1)), None);
    }
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
    let hwnd = ui::create_main_window(dir, &kestrel::settings::Settings::default()).unwrap();
    // The sidebar fills in right after startup.
    let tree = tree_of(hwnd);
    assert!(pump_until(WAIT, || tv_next(tree, TVGN_ROOT, 0) != 0), "sidebar never filled");
    hwnd
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

    // Nothing answers at this address, so listing it blocks for a long time. Windows
    // remembers failed hosts and then fails fast, so use a new address each run.
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos();
    let dead = format!(r"\\10.255.{}.{}\nothing", n % 250 + 1, (n / 250) % 250 + 1);
    unsafe { SetWindowTextW(edit, &HSTRING::from(dead)) }.unwrap();
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
fn tab_cycles_address_filter_sidebar_list() {
    let _desktop = desktop();
    let t = TempDir::new("tab");
    let hwnd = open(&t.0);
    let edit = find(hwnd, w!("Edit"));
    let filter = unsafe { FindWindowExW(Some(hwnd), Some(edit), w!("Edit"), None) }.unwrap();
    let order = [edit, filter, tree_of(hwnd), list_of(hwnd)];
    for i in 0..order.len() {
        // Something else on the desktop can take activation away for a moment, leaving
        // nothing focused. Retry then; focus landing on the wrong control fails at once.
        let mut landed = HWND::default();
        for _ in 0..5 {
            unsafe {
                let _ = SetActiveWindow(hwnd);
                let _ = SetFocus(Some(order[i]));
            }
            key(order[i], VK_TAB);
            pump_until(Duration::from_millis(100), || false);
            landed = unsafe { GetFocus() };
            if !landed.is_invalid() {
                break;
            }
        }
        assert_eq!(landed, order[(i + 1) % order.len()], "after Tab #{}", i + 1);
    }
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

// ---- live updates ----

fn select_row(list: HWND, row: usize) {
    let item = LVITEMW {
        state: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
        stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
        ..Default::default()
    };
    unsafe {
        SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(row)), Some(LPARAM(&item as *const _ as isize)))
    };
}

#[test]
fn changes_on_disk_show_up_and_keep_the_selection() {
    let t = TempDir::new("live");
    for n in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(t.0.join(n), "").unwrap();
    }
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["a.txt", "b.txt", "c.txt"]));
    pump_until(Duration::from_millis(300), || false); // let the watcher open the folder
    select_row(list, 2);

    std::fs::write(t.0.join("0new.txt"), "").unwrap();
    assert!(pump_until(WAIT, || rows(list) == ["0new.txt", "a.txt", "b.txt", "c.txt"]), "{:?}", rows(list));
    assert_eq!(selected(list), ["c.txt"]);

    std::fs::remove_file(t.0.join("a.txt")).unwrap();
    std::fs::rename(t.0.join("b.txt"), t.0.join("z.txt")).unwrap();
    assert!(pump_until(WAIT, || rows(list) == ["0new.txt", "c.txt", "z.txt"]), "{:?}", rows(list));
    assert_eq!(selected(list), ["c.txt"]);
    assert_eq!(status_text(hwnd), "3 items");
    close(hwnd);
}

#[test]
fn a_burst_of_new_files_all_arrives() {
    let t = TempDir::new("burst");
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || status_text(hwnd) == "0 items"));
    pump_until(Duration::from_millis(300), || false);
    let dir = t.0.clone();
    let writer = std::thread::spawn(move || {
        for i in 0..1000 {
            std::fs::write(dir.join(format!("f{i:04}")), "").unwrap();
        }
    });
    assert!(pump_until(Duration::from_secs(10), || item_count(list) == 1000), "{}", item_count(list));
    writer.join().unwrap();
    close(hwnd);
}

#[test]
fn deleting_the_open_folder_moves_to_its_parent() {
    let t = TempDir::new("vanish");
    let doomed = t.0.join("doomed").join("deeper");
    std::fs::create_dir_all(&doomed).unwrap();
    std::fs::write(t.0.join("keep.txt"), "").unwrap();
    let hwnd = open(&doomed);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || status_text(hwnd) == "0 items"));
    pump_until(Duration::from_millis(300), || false);

    std::fs::remove_dir_all(t.0.join("doomed")).expect("watching must not block deletion");
    assert!(pump_until(WAIT, || rows(list) == ["keep.txt"]), "{:?} / {}", rows(list), status_text(hwnd));
    close(hwnd);
}

#[test]
fn drive_changes_rebuild_the_drive_list_safely() {
    let t = TempDir::new("drives");
    let hwnd = open(&t.0);
    let tree = tree_of(hwnd);
    let pc = tv_children(tree, 0).into_iter().find(|(t, _)| t == "This PC").unwrap().1;
    let before = tv_children(tree, pc);
    // Start loading C:'s children, then pull the drive list out from under it.
    let c = system_drive_node(tree);
    unsafe { SendMessageW(tree, TVM_EXPAND, Some(WPARAM(TVE_EXPAND.0 as usize)), Some(LPARAM(c))) };
    let vol = DEV_BROADCAST_VOLUME {
        dbcv_size: size_of::<DEV_BROADCAST_VOLUME>() as u32,
        dbcv_devicetype: DBT_DEVTYP_VOLUME.0,
        ..Default::default()
    };
    unsafe {
        SendMessageW(
            hwnd,
            WM_DEVICECHANGE,
            Some(WPARAM(DBT_DEVICEARRIVAL as usize)),
            Some(LPARAM(&vol as *const _ as isize)),
        )
    };
    pump_until(Duration::from_millis(500), || false); // the old C: node's children arrive late and are dropped
    let after = tv_children(tree, pc);
    assert_eq!(before.len(), after.len());
    assert_ne!(before[0].1, after[0].1, "drive items were rebuilt");
    close(hwnd);
}

// ---- file operations ----

/// The clipboard is shared by the whole machine: one test at a time, and put back what was there.
static CLIPBOARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn command(main: HWND, id: usize) {
    unsafe { SendMessageW(main, WM_COMMAND, Some(WPARAM(id)), Some(LPARAM(0))) };
}

fn edit_control(list: HWND) -> HWND {
    HWND(unsafe { SendMessageW(list, LVM_GETEDITCONTROL, None, None) }.0 as _)
}

fn clipboard_text() -> Option<String> {
    kestrel::clip::get_text()
}

#[test]
fn f2_renames_in_place_and_rejects_bad_names() {
    let _desktop = desktop();
    let t = TempDir::new("rename");
    std::fs::write(t.0.join("report.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["report.txt"]));
    // The list only commits an in-place rename in a visible window.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    select_row(list, 0);
    unsafe { SetFocus(Some(list)) }.unwrap();

    key(list, VK_F2);
    assert!(pump_until(WAIT, || !edit_control(list).is_invalid()), "no rename box");
    let edit = edit_control(list);
    let (mut from, mut to) = (0u32, 0u32);
    unsafe {
        SendMessageW(
            edit,
            EM_GETSEL,
            Some(WPARAM(&mut from as *mut _ as usize)),
            Some(LPARAM(&mut to as *mut _ as isize)),
        )
    };
    assert_eq!((from, to), (0, 6), "only \"report\" is selected");
    type_into(edit, "bad:name.txt");
    key(edit, VK_RETURN);
    assert!(
        pump_until(WAIT, || status_text(hwnd).starts_with("A name can't contain")),
        "{}",
        status_text(hwnd)
    );
    assert!(t.0.join("report.txt").exists());

    key(list, VK_F2);
    assert!(pump_until(WAIT, || !edit_control(list).is_invalid()));
    type_into(edit_control(list), "final.md");
    key(edit_control(list), VK_RETURN);
    assert!(pump_until(WAIT, || rows(list) == ["final.md"]), "{:?}", rows(list));
    assert_eq!(selected(list), ["final.md"]);
    assert!(t.0.join("final.md").exists() && !t.0.join("report.txt").exists());
    close(hwnd);
}

#[test]
fn new_folder_appears_selected_and_ready_to_rename() {
    let t = TempDir::new("newdir");
    std::fs::write(t.0.join("x.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["x.txt"]));
    command(hwnd, ui::CMD_NEW_FOLDER);
    assert!(pump_until(WAIT, || rows(list) == ["New folder", "x.txt"]), "{:?}", rows(list));
    assert!(t.0.join("New folder").is_dir());
    // The watcher may show the folder a moment before the operation reports back.
    assert!(pump_until(WAIT, || selected(list) == ["New folder"]), "{:?}", selected(list));
    assert!(pump_until(WAIT, || !edit_control(list).is_invalid()), "rename box not opened");
    close(hwnd);
}

#[test]
fn copy_and_cut_then_paste_into_another_folder() {
    let _lock = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    let saved = clipboard_text();
    let t = TempDir::new("paste");
    std::fs::create_dir(t.0.join("sub")).unwrap();
    std::fs::write(t.0.join("a.txt"), "A").unwrap();
    std::fs::write(t.0.join("b.txt"), "B").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["sub", "a.txt", "b.txt"]));

    select_row(list, 1);
    command(hwnd, ui::CMD_COPY);
    activate(hwnd, list, 0);
    assert!(pump_until(WAIT, || status_text(hwnd) == "0 items"));
    command(hwnd, ui::CMD_PASTE);
    assert!(pump_until(WAIT, || rows(list) == ["a.txt"]), "{:?} / {}", rows(list), status_text(hwnd));
    assert!(t.0.join("a.txt").exists(), "copy keeps the original");

    key(list, VK_BACK);
    assert!(pump_until(WAIT, || rows(list) == ["sub", "a.txt", "b.txt"]));
    for r in 0..3 {
        let item = LVITEMW { stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0), ..Default::default() };
        unsafe {
            SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(r)), Some(LPARAM(&item as *const _ as isize)))
        };
    }
    select_row(list, 2);
    command(hwnd, ui::CMD_CUT);
    activate(hwnd, list, 0);
    assert!(pump_until(WAIT, || rows(list) == ["a.txt"]));
    command(hwnd, ui::CMD_PASTE);
    assert!(pump_until(WAIT, || rows(list) == ["a.txt", "b.txt"]), "{:?}", rows(list));
    assert!(!t.0.join("b.txt").exists(), "cut moves the original");
    close(hwnd);
    if let Some(text) = saved {
        restore_text(&text);
    }
}

#[test]
fn copy_as_path_puts_quoted_paths_on_the_clipboard() {
    let _lock = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
    let saved = clipboard_text();
    let t = TempDir::new("path");
    std::fs::write(t.0.join("one.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["one.txt"]));
    select_row(list, 0);
    command(hwnd, ui::CMD_COPY_PATH);
    assert_eq!(clipboard_text(), Some(format!("\"{}\"", t.0.join("one.txt").display())));
    close(hwnd);
    if let Some(text) = saved {
        restore_text(&text);
    }
}

fn restore_text(text: &str) {
    let owner = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("STATIC"),
            None,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
    }
    .unwrap();
    let _ = kestrel::clip::set_text(owner, text);
    unsafe { DestroyWindow(owner) }.unwrap();
}

// ---- settings and startup ----

#[test]
fn settings_shape_the_window_and_are_captured_on_close() {
    use kestrel::settings::Settings;
    use kestrel::sort::{Sort, SortKey};
    let t = TempDir::new("settings");
    std::fs::write(t.0.join("big.txt"), "x".repeat(5000)).unwrap();
    std::fs::write(t.0.join("small.txt"), "x").unwrap();
    let hidden = t.0.join("hidden.txt");
    std::fs::write(&hidden, "").unwrap();
    unsafe { SetFileAttributesW(&HSTRING::from(hidden.as_os_str()), FILE_ATTRIBUTE_HIDDEN) }.unwrap();
    let s = Settings {
        columns: vec![210, 111, 112, 113],
        sort: Sort { key: SortKey::Size, descending: true },
        show_hidden: true,
        split: Some(300),
        ..Default::default()
    };
    ui::init_process().unwrap();
    let hwnd = ui::create_main_window(&t.0, &s).unwrap();
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["big.txt", "small.txt", "hidden.txt"]), "{:?}", rows(list));
    let width = |c: usize| unsafe { SendMessageW(list, LVM_GETCOLUMNWIDTH, Some(WPARAM(c)), None) }.0;
    assert_eq!(width(0), 210);
    let mut r = windows::Win32::Foundation::RECT::default();
    unsafe { GetWindowRect(tree_of(hwnd), &mut r) }.unwrap();
    assert_eq!(r.right - r.left, 300);

    close(hwnd);
    let saved = ui::take_final_settings().expect("captured on close");
    assert_eq!((saved.sort, saved.show_hidden, saved.split), (s.sort, true, Some(300)));
    assert_eq!(saved.columns, s.columns);
    assert_eq!(saved.last_folder.as_deref(), Some(t.0.as_path()));
    assert!(saved.window.is_some_and(|(_, _, w, h)| w > 0 && h > 0));
}

#[test]
fn a_start_folder_that_is_gone_falls_back_to_the_user_folder() {
    let t = TempDir::new("gone");
    let hwnd = open(&t.0.join("deleted since last time"));
    let edit = find(hwnd, w!("Edit"));
    let home = std::env::var("USERPROFILE").unwrap();
    let text = || {
        let mut buf = [0u16; 512];
        let n = unsafe { GetWindowTextW(edit, &mut buf) };
        String::from_utf16_lossy(&buf[..n as usize])
    };
    assert!(pump_until(WAIT, || text().eq_ignore_ascii_case(&home)), "{} / {}", text(), status_text(hwnd));
    close(hwnd);
}

// ---- drag and drop ----

#[test]
fn dropping_files_copies_onto_folders_and_moves_into_the_open_folder() {
    use kestrel::dnd;
    use windows::Win32::Foundation::{POINT, POINTL};
    use windows::Win32::System::Ole::DROPEFFECT;
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    const MK_LBUTTON: u32 = 1;

    let t = TempDir::new("drop");
    let elsewhere = TempDir::new("drop-src");
    std::fs::create_dir(t.0.join("dest")).unwrap();
    std::fs::write(elsewhere.0.join("a.txt"), "A").unwrap();
    std::fs::write(elsewhere.0.join("b.txt"), "B").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["dest"]));
    let target = ui::drop_target().expect("list accepts drops");
    let screen = |x: i32, y: i32| {
        let mut p = POINT { x, y };
        unsafe { windows::Win32::Graphics::Gdi::ClientToScreen(list, &mut p) }.unwrap();
        POINTL { x: p.x, y: p.y }
    };
    let mut r = windows::Win32::Foundation::RECT { left: LVIR_BOUNDS as i32, ..Default::default() };
    unsafe { SendMessageW(list, LVM_GETITEMRECT, Some(WPARAM(0)), Some(LPARAM(&mut r as *mut _ as isize))) };
    let on_dest = screen(r.left + 20, (r.top + r.bottom) / 2);
    let below_rows = screen(r.left + 20, r.bottom + 200);
    let highlighted = || {
        let s = unsafe {
            SendMessageW(list, LVM_GETITEMSTATE, Some(WPARAM(0)), Some(LPARAM(LVIS_DROPHILITED.0 as isize)))
        };
        s.0 as u32 & LVIS_DROPHILITED.0 != 0
    };

    // Ctrl-drag a.txt onto the "dest" row: copy into dest.
    let data = dnd::data_object(&[elsewhere.0.join("a.txt")]).unwrap();
    let keys = MODIFIERKEYS_FLAGS(MK_LBUTTON | dnd::MK_CONTROL);
    let mut effect = DROPEFFECT(dnd::COPY | dnd::MOVE);
    unsafe { target.DragEnter(&data, keys, on_dest, &mut effect) }.unwrap();
    assert_eq!(effect.0, dnd::COPY);
    assert!(highlighted(), "destination folder row is highlighted");
    let mut effect = DROPEFFECT(dnd::COPY | dnd::MOVE);
    unsafe { target.Drop(&data, keys, on_dest, &mut effect) }.unwrap();
    assert_eq!(effect.0, dnd::COPY);
    assert!(!highlighted());
    assert!(pump_until(WAIT, || t.0.join("dest").join("a.txt").exists()));
    assert!(elsewhere.0.join("a.txt").exists(), "copy keeps the source");

    // Shift-drag b.txt onto empty space: move into the open folder; the source is told "none".
    let data = dnd::data_object(&[elsewhere.0.join("b.txt")]).unwrap();
    let keys = MODIFIERKEYS_FLAGS(MK_LBUTTON | dnd::MK_SHIFT);
    let mut effect = DROPEFFECT(dnd::COPY | dnd::MOVE);
    unsafe { target.DragEnter(&data, keys, below_rows, &mut effect) }.unwrap();
    assert_eq!(effect.0, dnd::MOVE);
    let mut effect = DROPEFFECT(dnd::COPY | dnd::MOVE);
    unsafe { target.Drop(&data, keys, below_rows, &mut effect) }.unwrap();
    assert_eq!(effect.0, dnd::NONE, "we move it ourselves; the source must not delete anything");
    assert!(pump_until(WAIT, || rows(list) == ["dest", "b.txt"]), "{:?}", rows(list));
    assert!(!elsewhere.0.join("b.txt").exists());

    // Dropping a file back into the folder it's in does nothing.
    let data = dnd::data_object(&[t.0.join("b.txt")]).unwrap();
    let mut effect = DROPEFFECT(dnd::COPY | dnd::MOVE);
    unsafe { target.DragEnter(&data, MODIFIERKEYS_FLAGS(MK_LBUTTON), below_rows, &mut effect) }.unwrap();
    assert_eq!(effect.0, dnd::NONE);
    unsafe { target.DragLeave() }.unwrap();
    close(hwnd);
}

// ---- full shell menu ----

#[test]
fn the_shell_menu_helper_runs_in_its_own_process() {
    let t = TempDir::new("shellmenu");
    std::fs::write(t.0.join("x.txt"), "").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_kestrel"))
        .args([kestrel::shellmenu::LIST_FLAG.as_ref(), t.0.join("x.txt").as_os_str()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let items = String::from_utf8_lossy(&out.stdout);
    assert!(items.lines().count() >= 3, "{items}");
}

// ---- filter and search ----

#[test]
fn typing_in_the_filter_narrows_and_enter_searches_subfolders() {
    let t = TempDir::new("filter");
    std::fs::create_dir_all(t.0.join("deep").join("er")).unwrap();
    std::fs::write(t.0.join("apple.txt"), "").unwrap();
    std::fs::write(t.0.join("banana.txt"), "").unwrap();
    std::fs::write(t.0.join("deep").join("er").join("pineapple.md"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["deep", "apple.txt", "banana.txt"]));
    // The second Edit control is the filter box (the first is the address bar).
    let address = find(hwnd, w!("Edit"));
    let filter = unsafe { FindWindowExW(Some(hwnd), Some(address), w!("Edit"), None) }.unwrap();

    type_into(filter, "APP");
    assert!(pump_until(WAIT, || rows(list) == ["apple.txt"]), "{:?}", rows(list));
    assert_eq!(status_text(hwnd), "1 of 3 items");

    key(filter, VK_RETURN);
    assert!(pump_until(WAIT, || rows(list).len() == 2), "{:?} / {}", rows(list), status_text(hwnd));
    let mut found = rows(list);
    found.sort();
    assert_eq!(found, ["apple.txt", r"deep\er\pineapple.md"]);
    assert!(status_text(hwnd).contains("results for \"APP\""), "{}", status_text(hwnd));

    alt_key(list, VK_LEFT);
    assert!(pump_until(WAIT, || rows(list) == ["deep", "apple.txt", "banana.txt"]), "{:?}", rows(list));
    close(hwnd);
}

// ---- tabs ----

/// Sends `vk` with Ctrl held, the way GetKeyState will see it.
fn ctrl_key(to: HWND, vk: VIRTUAL_KEY, shift: bool) {
    unsafe {
        let mut keys = [0u8; 256];
        GetKeyboardState(&mut keys).unwrap();
        keys[VK_CONTROL.0 as usize] = 0x80;
        if shift {
            keys[VK_SHIFT.0 as usize] = 0x80;
        }
        SetKeyboardState(&keys).unwrap();
        key(to, vk);
        pump_until(Duration::from_millis(50), || false);
        keys[VK_CONTROL.0 as usize] = 0;
        keys[VK_SHIFT.0 as usize] = 0;
        SetKeyboardState(&keys).unwrap();
    }
}

fn tab_labels(main: HWND) -> Vec<String> {
    let strip = find(main, w!("SysTabControl32"));
    let n = unsafe { SendMessageW(strip, TCM_GETITEMCOUNT, None, None) }.0 as usize;
    (0..n)
        .map(|i| {
            let mut buf = [0u16; 260];
            let mut item = TCITEMW {
                mask: TCIF_TEXT,
                pszText: PWSTR(buf.as_mut_ptr()),
                cchTextMax: buf.len() as i32,
                ..Default::default()
            };
            unsafe {
                SendMessageW(strip, TCM_GETITEMW, Some(WPARAM(i)), Some(LPARAM(&mut item as *mut _ as isize)))
            };
            let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf16_lossy(&buf[..len])
        })
        .collect()
}

#[test]
fn tabs_keep_their_own_folders() {
    let t = TempDir::new("tabs");
    std::fs::create_dir(t.0.join("sub")).unwrap();
    std::fs::write(t.0.join("sub").join("inner.txt"), "").unwrap();
    std::fs::write(t.0.join("a.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    let root_rows = ["sub", "a.txt"];
    assert!(pump_until(WAIT, || rows(ui::active_list().unwrap()) == root_rows));
    let root_name = t.0.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(tab_labels(hwnd), std::slice::from_ref(&root_name));

    ctrl_key(list, VK_T, false);
    assert!(
        pump_until(WAIT, || tab_labels(hwnd).len() == 2 && rows(ui::active_list().unwrap()) == root_rows),
        "{:?}",
        tab_labels(hwnd)
    );
    let list = ui::active_list().unwrap();
    activate(hwnd, list, 0);
    assert!(
        pump_until(WAIT, || rows(ui::active_list().unwrap()) == ["inner.txt"]),
        "{:?}",
        rows(ui::active_list().unwrap())
    );
    assert_eq!(tab_labels(hwnd), [root_name.clone(), "sub".to_string()]);

    ctrl_key(ui::active_list().unwrap(), VK_TAB, false);
    assert!(
        pump_until(WAIT, || rows(ui::active_list().unwrap()) == root_rows),
        "{:?}",
        rows(ui::active_list().unwrap())
    );
    ctrl_key(ui::active_list().unwrap(), VK_TAB, true);
    assert!(
        pump_until(WAIT, || rows(ui::active_list().unwrap()) == ["inner.txt"]),
        "{:?}",
        rows(ui::active_list().unwrap())
    );

    ctrl_key(ui::active_list().unwrap(), VK_W, false);
    assert!(
        pump_until(WAIT, || tab_labels(hwnd).len() == 1 && rows(ui::active_list().unwrap()) == root_rows),
        "{:?}",
        rows(ui::active_list().unwrap())
    );
    ctrl_key(ui::active_list().unwrap(), VK_W, false); // the last tab stays
    assert_eq!(tab_labels(hwnd).len(), 1);
    close(hwnd);
}

// ---- dark mode ----

/// Brightness (0-255) of the window's pixel at client point (x, y), rendered off-screen.
fn pixel_luma(hwnd: HWND, x: i32, y: i32) -> u32 {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    unsafe {
        let mut r = windows::Win32::Foundation::RECT::default();
        GetClientRect(hwnd, &mut r).unwrap();
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, r.right, r.bottom);
        let old = SelectObject(dc, HGDIOBJ(bmp.0));
        // PW_CLIENTONLY | PW_RENDERFULLCONTENT
        let _ = PrintWindow(hwnd, dc, PRINT_WINDOW_FLAGS(1 | 2));
        let c = GetPixel(dc, x, y).0;
        SelectObject(dc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        ((c & 0xFF) * 299 + ((c >> 8) & 0xFF) * 587 + ((c >> 16) & 0xFF) * 114) / 1000
    }
}

#[test]
fn dark_theme_colours_the_window_and_is_remembered() {
    let _desktop = desktop();
    use kestrel::settings::Settings;
    use kestrel::theme::ThemeChoice;
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmGetWindowAttribute};
    let t = TempDir::new("dark");
    std::fs::write(t.0.join("x.txt"), "").unwrap();
    ui::init_process().unwrap();
    let hwnd =
        ui::create_main_window(&t.0, &Settings { theme: ThemeChoice::Dark, ..Default::default() }).unwrap();
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["x.txt"]));
    assert_eq!(unsafe { SendMessageW(list, LVM_GETBKCOLOR, None, None) }.0, 0x00202020);
    let mut dark = windows::core::BOOL(0);
    unsafe { DwmGetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &mut dark as *mut _ as *mut _, 4) }
        .unwrap();
    assert!(dark.as_bool(), "dark title bar");

    // The status bar and tab strip are painted by us in dark mode.
    let mut sr = windows::Win32::Foundation::RECT::default();
    let status = find(hwnd, w!("msctls_statusbar32"));
    unsafe { GetWindowRect(status, &mut sr) }.unwrap();
    let mut cr = windows::Win32::Foundation::RECT::default();
    unsafe { GetClientRect(hwnd, &mut cr) }.unwrap();
    let status_y = cr.bottom - (sr.bottom - sr.top) / 2;
    assert!(pixel_luma(hwnd, cr.right - 80, status_y) < 80, "status bar not dark");

    close(hwnd);
    assert_eq!(ui::take_final_settings().unwrap().theme, ThemeChoice::Dark);

    let hwnd =
        ui::create_main_window(&t.0, &Settings { theme: ThemeChoice::Light, ..Default::default() }).unwrap();
    let list = list_of(hwnd);
    assert_eq!(unsafe { SendMessageW(list, LVM_GETBKCOLOR, None, None) }.0, 0x00FFFFFF);
    close(hwnd);
}

// ---- preview pane ----

fn visible_text_in(pane: HWND) -> String {
    let mut child = unsafe { FindWindowExW(Some(pane), None, None, None) }.unwrap_or_default();
    while !child.is_invalid() {
        if unsafe { IsWindowVisible(child) }.as_bool() {
            let mut buf = vec![0u16; 4096];
            let n = unsafe { GetWindowTextW(child, &mut buf) };
            let text = String::from_utf16_lossy(&buf[..n as usize]);
            if !text.is_empty() {
                return text;
            }
        }
        child = unsafe { FindWindowExW(Some(pane), Some(child), None, None) }.unwrap_or_default();
    }
    String::new()
}

#[test]
fn preview_pane_shows_text_and_says_when_it_cannot() {
    let _desktop = desktop();
    let t = TempDir::new("preview");
    std::fs::write(t.0.join("notes.txt"), "hello preview\nline two").unwrap();
    std::fs::write(t.0.join("data.fepbin"), [0x4D, 0x5A, 0, 0, 1, 2]).unwrap();
    let hwnd = open(&t.0);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["data.fepbin", "notes.txt"]), "{:?}", rows(list));
    let pane = find(hwnd, w!("KestrelPreview"));
    assert!(!unsafe { IsWindowVisible(pane) }.as_bool());

    alt_key(list, VK_P);
    assert!(pump_until(WAIT, || unsafe { IsWindowVisible(pane) }.as_bool()), "Alt+P shows the pane");

    select_row(list, 1);
    assert!(
        pump_until(WAIT, || visible_text_in(pane) == "hello preview\r\nline two"),
        "{:?}",
        visible_text_in(pane)
    );

    let none = LVITEMW {
        stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
        ..Default::default()
    };
    unsafe {
        SendMessageW(list, LVM_SETITEMSTATE, Some(WPARAM(1)), Some(LPARAM(&none as *const _ as isize)))
    };
    select_row(list, 0);
    assert!(
        pump_until(WAIT, || visible_text_in(pane) == "No preview available."),
        "{:?}",
        visible_text_in(pane)
    );

    alt_key(list, VK_P);
    assert!(pump_until(WAIT, || !unsafe { IsWindowVisible(pane) }.as_bool()));
    close(hwnd);
}

// ---- side-by-side windows ----

/// Top-level Kestrel windows whose title starts with `prefix`, other than `except`.
fn windows_titled(prefix: &str, except: HWND) -> Vec<HWND> {
    struct S {
        prefix: String,
        except: HWND,
        found: Vec<HWND>,
    }
    extern "system" fn each(h: HWND, lp: LPARAM) -> windows::core::BOOL {
        let s = unsafe { &mut *(lp.0 as *mut S) };
        let mut buf = [0u16; 512];
        let n = unsafe { GetWindowTextW(h, &mut buf) };
        let title = String::from_utf16_lossy(&buf[..n as usize]);
        if h != s.except && title.starts_with(&s.prefix) && title.ends_with("Kestrel") {
            s.found.push(h);
        }
        true.into()
    }
    let mut s = S { prefix: prefix.to_string(), except, found: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut s as *mut S as isize));
    }
    s.found
}

#[test]
fn ctrl_n_opens_a_second_window_side_by_side() {
    let _desktop = desktop();
    let t = TempDir::new("twin");
    std::fs::write(t.0.join("x.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["x.txt"]));
    ctrl_key(list, VK_N, false);
    let prefix = t.0.display().to_string();
    assert!(pump_until(WAIT, || windows_titled(&prefix, hwnd).len() == 1), "no second window");
    let twin = windows_titled(&prefix, hwnd)[0];
    let (mut a, mut b) =
        (windows::Win32::Foundation::RECT::default(), windows::Win32::Foundation::RECT::default());
    unsafe {
        GetWindowRect(hwnd, &mut a).unwrap();
        GetWindowRect(twin, &mut b).unwrap();
    }
    assert!(a.right <= b.left + 20, "side by side: {a:?} {b:?}");
    unsafe { PostMessageW(Some(twin), WM_CLOSE, WPARAM(0), LPARAM(0)) }.unwrap();
    ui::wait_for_other_windows(); // returns once the second window's thread has ended
    assert!(windows_titled(&prefix, hwnd).is_empty());
    close(hwnd);
}

// ---- network ----

#[test]
#[ignore = "requires localhost SMB access and a readable C$ administrative share"]
fn a_computer_lists_its_shares_and_up_returns_to_them() {
    let t = TempDir::new("net");
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    let edit = find(hwnd, w!("Edit"));
    unsafe { SetWindowTextW(edit, w!(r"\\localhost")) }.unwrap();
    key(edit, VK_RETURN);
    assert!(
        pump_until(WAIT, || status_text(hwnd).ends_with("items") && window_text_of(edit) == r"\\localhost"),
        "{} / {}",
        status_text(hwnd),
        window_text_of(edit)
    );
    // Only admin shares (C$, ...) exist here: they show with hidden items.
    ctrl_key(list, VK_H, false);
    assert!(pump_until(WAIT, || rows(list).iter().any(|r| r.eq_ignore_ascii_case("C$"))), "{:?}", rows(list));
    let c = rows(list).iter().position(|r| r.eq_ignore_ascii_case("C$")).unwrap();
    activate(hwnd, list, c as i32);
    assert!(
        pump_until(WAIT, || rows(list).iter().any(|r| r.eq_ignore_ascii_case("Windows"))),
        "{:?}",
        rows(list)
    );
    alt_key(list, VK_UP);
    assert!(pump_until(WAIT, || window_text_of(edit) == r"\\localhost"), "{}", window_text_of(edit));
    close(hwnd);
}

fn window_text_of(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n as usize])
}

#[test]
fn toolbar_commands_follow_selection_and_help_opens_with_f1() {
    let _desktop = desktop();
    let t = TempDir::new("toolbar");
    std::fs::write(t.0.join("one.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let list = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(list) == ["one.txt"]));
    let copy = unsafe { GetDlgItem(Some(hwnd), ui::CMD_COPY as i32) }.unwrap();
    assert!(!unsafe { IsWindowEnabled(copy) }.as_bool());
    command(hwnd, ui::CMD_SELECT_ALL);
    assert!(pump_until(WAIT, || unsafe { IsWindowEnabled(copy) }.as_bool()));
    key(list, VK_F1);
    let mut guide = HWND::default();
    assert!(pump_until(WAIT, || {
        guide = unsafe { FindWindowW(w!("KestrelShortcuts"), None) }.unwrap_or_default();
        !guide.is_invalid()
    }));
    let edit = unsafe { GetDlgItem(Some(guide), 30) }.unwrap();
    let mut buf = vec![0u16; 8192];
    let n = unsafe { GetWindowTextW(edit, &mut buf) };
    let text = String::from_utf16_lossy(&buf[..n as usize]);
    assert!(text.contains("Shift+Delete") && text.contains("Ctrl+Shift+Tab") && text.contains("Text boxes"));
    key(edit, VK_ESCAPE);
    assert!(pump_until(WAIT, || !unsafe { IsWindow(Some(guide)) }.as_bool()));
    close(hwnd);
}

#[test]
fn settings_validate_apply_and_cancel_without_losing_preferences() {
    let _desktop = desktop();
    use kestrel::settings::Settings;
    use kestrel::theme::ThemeChoice;
    let t = TempDir::new("preferences");
    ui::init_process().unwrap();
    let hwnd =
        ui::create_main_window(&t.0, &Settings { theme: ThemeChoice::Light, ..Default::default() }).unwrap();
    command(hwnd, ui::toolbar::SETTINGS);
    let dialog = unsafe { FindWindowW(w!("KestrelSettings"), None) }.unwrap();
    let field = |id| unsafe { GetDlgItem(Some(dialog), id) }.unwrap();
    unsafe {
        SendMessageW(field(10), CB_SETCURSEL, Some(WPARAM(2)), None);
        SetWindowTextW(field(11), w!("invalid")).unwrap();
    }
    command(dialog, 3);
    assert!(window_text_of(field(26)).contains("whole number"));
    assert_eq!(unsafe { SendMessageW(list_of(hwnd), LVM_GETBKCOLOR, None, None) }.0, 0x00ffffff);
    unsafe {
        SetWindowTextW(field(11), w!("300")).unwrap();
        SendMessageW(field(13), BM_SETCHECK, Some(WPARAM(BST_CHECKED.0 as usize)), None);
    }
    command(dialog, 3);
    assert_eq!(unsafe { SendMessageW(list_of(hwnd), LVM_GETBKCOLOR, None, None) }.0, 0x00202020);
    assert_eq!(window_text_of(field(26)), "Settings applied.");
    command(dialog, 4); // Restore defaults edits the form, not the running app.
    command(dialog, IDCANCEL.0 as usize);
    assert!(!unsafe { IsWindow(Some(dialog)) }.as_bool());
    close(hwnd);
    let saved = ui::take_final_settings().unwrap();
    assert_eq!(saved.theme, ThemeChoice::Dark);
    assert_eq!(saved.split, Some(300));
    assert!(saved.show_hidden);
    while ui::running_operations() > 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn lists_in(hwnd: HWND) -> Vec<HWND> {
    let mut lists = Vec::new();
    let mut after = None;
    while let Ok(list) = unsafe { FindWindowExW(Some(hwnd), after, w!("SysListView32"), None) } {
        lists.push(list);
        after = Some(list);
    }
    lists
}
fn focus_pane(hwnd: HWND, list: HWND) {
    notify(hwnd, hdr(list, NM_SETFOCUS));
}
fn navigate_to(hwnd: HWND, path: &Path) {
    let address = find(hwnd, w!("Edit"));
    type_into(address, &path.display().to_string());
    key(address, VK_RETURN);
}

#[test]
fn split_panes_navigate_independently_and_survive_tab_switches() {
    let _desktop = desktop();
    let t = TempDir::new("split");
    for name in ["left", "right"] {
        std::fs::create_dir(t.0.join(name)).unwrap();
        std::fs::write(t.0.join(name).join(format!("{name}.txt")), "").unwrap();
    }
    let hwnd = open(&t.0);
    let left = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(left) == ["left", "right"]));
    command(hwnd, ui::toolbar::SPLIT);
    let right = lists_in(hwnd).into_iter().find(|&h| h != left).unwrap();
    assert!(pump_until(WAIT, || rows(right) == ["left", "right"]));
    focus_pane(hwnd, right);
    navigate_to(hwnd, &t.0.join("right"));
    assert!(pump_until(WAIT, || rows(right) == ["right.txt"]));
    assert_eq!(rows(left), ["left", "right"]);
    focus_pane(hwnd, left);
    navigate_to(hwnd, &t.0.join("left"));
    assert!(pump_until(WAIT, || rows(left) == ["left.txt"]));
    assert_eq!(rows(right), ["right.txt"]);
    ctrl_key(left, VK_T, false);
    assert!(pump_until(WAIT, || tab_labels(hwnd).len() == 2));
    let new_list = ui::active_list().unwrap();
    ctrl_key(new_list, VK_TAB, false);
    assert!(pump_until(WAIT, || ui::active_list() == Some(left)));
    assert_eq!(rows(left), ["left.txt"]);
    assert_eq!(rows(right), ["right.txt"]);
    key(left, VK_F6);
    assert!(pump_until(WAIT, || ui::active_list() == Some(right)));
    key(right, VK_BACK);
    assert!(pump_until(WAIT, || rows(right) == ["left", "right"]));
    assert_eq!(rows(left), ["left.txt"]);
    command(hwnd, ui::toolbar::SPLIT);
    command(hwnd, ui::toolbar::SPLIT);
    assert_eq!(rows(left), ["left.txt"]);
    assert_eq!(rows(right), ["left", "right"]);
    close(hwnd);
}

#[test]
fn creating_a_folder_keeps_its_destination_when_focus_changes() {
    let _desktop = desktop();
    let t = TempDir::new("split-op");
    std::fs::create_dir(t.0.join("target")).unwrap();
    std::fs::write(t.0.join("target").join("marker.txt"), "").unwrap();
    let hwnd = open(&t.0);
    let left = list_of(hwnd);
    assert!(pump_until(WAIT, || rows(left) == ["target"]));
    command(hwnd, ui::toolbar::SPLIT);
    let right = lists_in(hwnd).into_iter().find(|&h| h != left).unwrap();
    assert!(pump_until(WAIT, || rows(right) == ["target"]));
    focus_pane(hwnd, right);
    navigate_to(hwnd, &t.0.join("target"));
    assert!(pump_until(WAIT, || rows(right) == ["marker.txt"]));
    command(hwnd, ui::CMD_NEW_FOLDER);
    focus_pane(hwnd, left);
    assert!(
        pump_until(WAIT, || t.0.join("target").join("New folder").exists() && ui::running_operations() == 0)
    );
    assert!(!t.0.join("New folder").exists());
    assert_eq!(ui::active_list(), Some(left));
    assert!(edit_control(left).is_invalid(), "completion must not start renaming in the other pane");
    close(hwnd);
}

#[test]
fn split_and_preview_dividers_are_dragged_and_remembered() {
    let _desktop = desktop();
    let t = TempDir::new("split-size");
    let hwnd = open(&t.0);
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    let left = list_of(hwnd);
    command(hwnd, ui::toolbar::SPLIT);
    let right = lists_in(hwnd).into_iter().find(|&h| h != left).unwrap();
    let rect = |h| {
        let mut r = windows::Win32::Foundation::RECT::default();
        unsafe {
            GetWindowRect(h, &mut r).unwrap();
        }
        r
    };
    let a = rect(left);
    let mut point = windows::Win32::Foundation::POINT { x: a.right + 2, y: a.top + 40 };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
    }
    let lp = |x: i32, y: i32| LPARAM(((y as isize & 0xffff) << 16) | (x as isize & 0xffff));
    unsafe {
        SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(point.x, point.y)));
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(lp(point.x + 70, point.y)));
        SendMessageW(hwnd, WM_LBUTTONUP, None, Some(lp(point.x + 70, point.y)));
    }
    assert!(rect(left).right > a.right + 30);
    assert!(rect(right).left > rect(left).right);
    command(hwnd, ui::toolbar::PREVIEW);
    let preview = find(hwnd, w!("KestrelPreview"));
    let p = rect(preview);
    let mut point = windows::Win32::Foundation::POINT { x: p.left - 2, y: p.top + 40 };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut point);
    }
    unsafe {
        SendMessageW(hwnd, WM_LBUTTONDOWN, Some(WPARAM(1)), Some(lp(point.x, point.y)));
        SendMessageW(hwnd, WM_MOUSEMOVE, Some(WPARAM(1)), Some(lp(point.x - 40, point.y)));
        SendMessageW(hwnd, WM_LBUTTONUP, None, Some(lp(point.x - 40, point.y)));
    }
    assert!(rect(preview).left < p.left);
    close(hwnd);
    let settings = ui::take_final_settings().unwrap();
    assert!(settings.dual_pane);
    assert!(settings.pane_ratio > 500);
    assert!(settings.preview_width.is_some());
}

#[test]
fn session_restores_tabs_and_both_folders() {
    let _desktop = desktop();
    let t = TempDir::new("restore");
    for name in ["one", "two", "three"] {
        std::fs::create_dir(t.0.join(name)).unwrap();
        std::fs::write(t.0.join(name).join(format!("{name}.txt")), "").unwrap();
    }
    use kestrel::settings::{SavedTab, Settings};
    let settings = Settings {
        tabs: vec![
            SavedTab {
                folder: t.0.join("one"),
                other: Some(t.0.join("two")),
                split: true,
                right_active: true,
                ratio: 620,
            },
            SavedTab {
                folder: t.0.join("three"),
                other: None,
                split: false,
                right_active: false,
                ratio: 500,
            },
        ],
        active_tab: 0,
        ..Default::default()
    };
    ui::init_process().unwrap();
    let hwnd = ui::create_main_window(&t.0, &settings).unwrap();
    assert!(pump_until(WAIT, || rows(ui::active_list().unwrap()) == ["one.txt"]));
    assert_eq!(tab_labels(hwnd).len(), 2);
    let first = ui::active_list().unwrap();
    key(first, VK_F6);
    assert!(pump_until(WAIT, || rows(ui::active_list().unwrap()) == ["two.txt"]));
    ctrl_key(ui::active_list().unwrap(), VK_TAB, false);
    assert!(pump_until(WAIT, || rows(ui::active_list().unwrap()) == ["three.txt"]));
    close(hwnd);
    let saved = ui::take_final_settings().unwrap();
    assert_eq!(saved.active_tab, 1);
    assert_eq!(saved.tabs.len(), 2);
    assert_eq!(saved.tabs[0].folder, t.0.join("two"));
    assert_eq!(saved.tabs[0].other, Some(t.0.join("one")));
    assert_eq!(saved.tabs[0].ratio, 620);
    assert_eq!(Settings::parse(&saved.to_text()).tabs, saved.tabs);
}
