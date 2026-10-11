//! cargo run --release --example stress -- [seconds]
//!
//! Real window on a temp folder tree. One thread keeps creating, renaming and
//! deleting files; the UI thread does a random action every 50 ms (navigate,
//! back/forward, refresh, sorting, sidebar, search, tabs, panes, preview, grid, settings and windows). Prints handle and
//! GDI/USER object counts over time and the longest the UI thread was busy.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use kestrel::settings::Settings;
use kestrel::ui;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Threading::{
    GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
};
use windows::Win32::UI::Controls::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, w};

/// Tiny xorshift: no dependency needed for "random enough".
struct Rng(u64);
impl Rng {
    fn next(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// CPU time this thread has used (kernel + user).
fn thread_cpu() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut a, mut b, mut k, mut u) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let t = |f: FILETIME| (f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64;
    unsafe {
        let _ = GetThreadTimes(GetCurrentThread(), &mut a, &mut b, &mut k, &mut u);
    }
    Duration::from_nanos((t(k) + t(u)) * 100)
}

/// Prints anything that kept the UI thread busy for more than 100 ms, with how
/// much of that was the thread actually running (vs. waiting or preempted).
fn slow(since: (Instant, Duration), what: &str) {
    let took = since.0.elapsed();
    if took > Duration::from_millis(100) {
        println!("SLOW {took:?} (cpu {:?}): {what}", thread_cpu() - since.1);
    }
}

fn class_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn counts() -> (u32, u32, u32) {
    unsafe {
        let p = GetCurrentProcess();
        let mut handles = 0;
        let _ = GetProcessHandleCount(p, &mut handles);
        (handles, GetGuiResources(p, GR_GDIOBJECTS), GetGuiResources(p, GR_USEROBJECTS))
    }
}

fn command(hwnd: HWND, id: usize) {
    unsafe {
        SendMessageW(hwnd, WM_COMMAND, Some(WPARAM(id)), Some(LPARAM(0)));
    }
}
fn press(hwnd: HWND, key: VIRTUAL_KEY, ctrl: bool, shift: bool) {
    unsafe {
        let mut original = [0u8; 256];
        GetKeyboardState(&mut original).unwrap();
        let mut keys = original;
        keys[VK_CONTROL.0 as usize] = if ctrl { 0x80 } else { 0 };
        keys[VK_SHIFT.0 as usize] = if shift { 0x80 } else { 0 };
        SetKeyboardState(&keys).unwrap();
        let msg = MSG { hwnd, message: WM_KEYDOWN, wParam: WPARAM(key.0 as usize), ..Default::default() };
        if !ui::translate(&msg) {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        SetKeyboardState(&original).unwrap();
    }
}
fn own_windows() -> Vec<HWND> {
    unsafe extern "system" fn each(hwnd: HWND, param: LPARAM) -> windows::core::BOOL {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() && class_of(hwnd).starts_with("Kestrel") {
                (*(param.0 as *mut Vec<HWND>)).push(hwnd);
            }
        }
        true.into()
    }
    let mut windows = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut windows as *mut _ as isize));
    }
    windows
}
fn pump_for(duration: Duration) {
    let until = Instant::now() + duration;
    let mut message = MSG::default();
    while Instant::now() < until {
        let batch = Instant::now();
        unsafe {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                if !ui::translate(&message) {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                if batch.elapsed() > Duration::from_millis(16) {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn normalize(hwnd: HWND, edit: HWND, strip: HWND, root: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for window in own_windows().into_iter().filter(|w| *w != hwnd) {
            unsafe {
                let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        pump_for(Duration::from_millis(20));
        if ui::other_window_count() == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "secondary window did not close");
    }
    ui::wait_for_other_windows();
    while unsafe { SendMessageW(strip, TCM_GETITEMCOUNT, None, None).0 } > 1 {
        press(ui::active_list().unwrap(), VK_W, true, false);
    }
    let mut visible = 0;
    let mut after = None;
    while let Ok(list) = unsafe { FindWindowExW(Some(hwnd), after, w!("SysListView32"), None) } {
        if unsafe { IsWindowVisible(list) }.as_bool() {
            visible += 1;
        }
        after = Some(list);
    }
    if visible < 2 {
        command(hwnd, ui::toolbar::SPLIT);
    }
    for _ in 0..2 {
        command(hwnd, ui::toolbar::GRID);
        command(hwnd, ui::toolbar::DETAILS);
        unsafe {
            let _ = SetWindowTextW(edit, &HSTRING::from(root.as_os_str()));
        }
        press(edit, VK_RETURN, false, false);
        press(ui::active_list().unwrap(), VK_F6, false, false);
    }
    if let Ok(pane) = unsafe { FindWindowExW(Some(hwnd), None, w!("KestrelPreview"), None) }
        && unsafe { IsWindowVisible(pane) }.as_bool()
    {
        command(hwnd, ui::toolbar::PREVIEW);
    }
    pump_for(Duration::from_secs(2));
}

fn main() {
    let seconds: u64 = std::env::args().nth(1).map_or(120, |s| s.parse().unwrap());
    let root = std::env::temp_dir().join(format!("fep-stress-{}", std::process::id()));
    let dirs: Vec<PathBuf> = (0..5).map(|i| root.join(format!("dir{i}"))).collect();
    for d in &dirs {
        std::fs::create_dir_all(d).unwrap();
        for f in 0..200 {
            std::fs::write(d.join(format!("file{f}.txt")), "x").unwrap();
        }
    }

    ui::init_process().unwrap();
    let hwnd = ui::create_main_window(&root, &Settings::default()).unwrap();
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    let find = |class| unsafe { FindWindowExW(Some(hwnd), None, class, None) }.unwrap();
    let (edit, tree, strip) = (find(w!("Edit")), find(w!("SysTreeView32")), find(w!("SysTabControl32")));
    let filter = unsafe { FindWindowExW(Some(hwnd), Some(edit), w!("Edit"), None) }.unwrap();

    let stop = Arc::new(AtomicBool::new(false));
    let churn = {
        let (stop, dirs) = (stop.clone(), dirs.clone());
        std::thread::spawn(move || {
            let mut rng = Rng(0x9E3779B97F4A7C15);
            let mut ops = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let d = &dirs[rng.next(dirs.len() as u64) as usize];
                let a = d.join(format!("churn{}.txt", rng.next(50)));
                let b = d.join(format!("churn{}.txt", rng.next(50)));
                let _ = match rng.next(3) {
                    0 => std::fs::write(&a, "y"),
                    1 => std::fs::rename(&a, &b),
                    _ => std::fs::remove_file(&a),
                };
                ops += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
            ops
        })
    };

    let post = |to: HWND, msg: u32, wp: usize, lp: isize| unsafe {
        let _ = PostMessageW(Some(to), msg, WPARAM(wp), LPARAM(lp));
    };
    let mut rng = Rng(12345);
    let start = Instant::now();
    let mut next_action = start;
    let mut next_report = start;
    let mut baseline = None;
    let mut worst_gap = Duration::ZERO;
    let mut last = Instant::now();
    let mut actions = 0u64;
    let mut msg = MSG::default();
    while start.elapsed() < Duration::from_secs(seconds) {
        let batch = Instant::now();
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let t = (Instant::now(), thread_cpu());
                if !ui::translate(&msg) {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                slow(t, &format!("message {:#06x} to {}", msg.message, class_of(msg.hwnd)));
                // Inputs must keep arriving even when workers continuously post results.
                if batch.elapsed() > Duration::from_millis(16) {
                    break;
                }
            }
        }
        // Time between loop turns = longest stretch the UI thread spent on one batch of messages.
        let now = Instant::now();
        if start.elapsed() > Duration::from_secs(2) {
            worst_gap = worst_gap.max(now - last);
        }
        last = now;

        if now >= next_action {
            next_action = now + Duration::from_millis(50);
            actions += 1;
            let t = (Instant::now(), thread_cpu());
            let list = ui::active_list().unwrap();
            let action = rng.next(16);
            match action {
                0 | 1 => {
                    let target =
                        if rng.next(4) == 0 { &root } else { &dirs[rng.next(dirs.len() as u64) as usize] };
                    unsafe {
                        let _ = SetWindowTextW(edit, &HSTRING::from(target.as_os_str()));
                    }
                    press(edit, VK_RETURN, false, false);
                }
                2 => post(list, WM_SYSKEYDOWN, [VK_LEFT, VK_RIGHT][rng.next(2) as usize].0 as usize, 1 << 29),
                3 => post(list, WM_KEYDOWN, VK_F5.0 as usize, 0),
                4 => {
                    let mut nm = NMLISTVIEW {
                        hdr: NMHDR { hwndFrom: list, idFrom: 0, code: LVN_COLUMNCLICK },
                        iSubItem: rng.next(4) as i32,
                        ..Default::default()
                    };
                    unsafe {
                        SendMessageW(
                            hwnd,
                            WM_NOTIFY,
                            Some(WPARAM(0)),
                            Some(LPARAM(&mut nm as *mut _ as isize)),
                        )
                    };
                }

                6 => {
                    unsafe {
                        let _ = SetWindowTextW(
                            filter,
                            &HSTRING::from(["file1", "churn", ""][rng.next(3) as usize]),
                        );
                    }
                    if rng.next(4) == 0 {
                        press(filter, VK_RETURN, false, false);
                    }
                }
                7 => {
                    let count = unsafe { SendMessageW(strip, TCM_GETITEMCOUNT, None, None).0 };
                    press(list, if count < 4 && rng.next(2) == 0 { VK_T } else { VK_W }, true, false);
                }
                8 => press(list, VK_TAB, true, rng.next(2) == 0),
                9 => command(hwnd, ui::toolbar::SPLIT),
                10 => press(list, VK_F6, false, false),
                11 => {
                    command(hwnd, ui::toolbar::PREVIEW);
                    let count = unsafe { SendMessageW(list, LVM_GETITEMCOUNT, None, None).0 };
                    if count > 0 {
                        let item = LVITEMW {
                            state: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
                            stateMask: LIST_VIEW_ITEM_STATE_FLAGS(LVIS_SELECTED.0 | LVIS_FOCUSED.0),
                            ..Default::default()
                        };
                        unsafe {
                            SendMessageW(
                                list,
                                LVM_SETITEMSTATE,
                                Some(WPARAM(rng.next(count as u64) as usize)),
                                Some(LPARAM(&item as *const _ as isize)),
                            );
                        }
                    }
                }
                12 => command(hwnd, if rng.next(2) == 0 { ui::toolbar::GRID } else { ui::toolbar::DETAILS }),
                13 => {
                    command(hwnd, ui::toolbar::SETTINGS);
                    if let Some(dialog) =
                        own_windows().into_iter().find(|h| class_of(*h) == "KestrelSettings")
                    {
                        unsafe {
                            if let Ok(theme) = GetDlgItem(Some(dialog), 10) {
                                SendMessageW(theme, CB_SETCURSEL, Some(WPARAM(rng.next(3) as usize)), None);
                            }
                        }
                        command(dialog, 3);
                        command(dialog, IDCANCEL.0 as usize);
                    }
                }
                14 => {
                    let secondary =
                        own_windows().into_iter().find(|h| *h != hwnd && class_of(*h) == "Kestrel");
                    if let Some(window) = secondary {
                        post(window, WM_CLOSE, 0, 0);
                    } else {
                        press(list, VK_N, true, false);
                    }
                }
                15 => command(hwnd, ui::toolbar::HIDDEN),
                _ => {
                    // Expand a random visible sidebar node.
                    let item = unsafe {
                        SendMessageW(
                            tree,
                            TVM_GETNEXTITEM,
                            Some(WPARAM(TVGN_FIRSTVISIBLE as usize)),
                            Some(LPARAM(0)),
                        )
                    }
                    .0;
                    let mut it = item;
                    for _ in 0..rng.next(12) {
                        let n = unsafe {
                            SendMessageW(
                                tree,
                                TVM_GETNEXTITEM,
                                Some(WPARAM(TVGN_NEXTVISIBLE as usize)),
                                Some(LPARAM(it)),
                            )
                        }
                        .0;
                        if n == 0 {
                            break;
                        }
                        it = n;
                    }
                    unsafe {
                        SendMessageW(tree, TVM_EXPAND, Some(WPARAM(TVE_EXPAND.0 as usize)), Some(LPARAM(it)))
                    };
                }
            }
            slow(t, &format!("action {action}"));
            worst_gap = worst_gap.max(t.0.elapsed());
        }
        if now >= next_report {
            next_report = now + Duration::from_secs(10);
            if start.elapsed() > Duration::from_secs((seconds / 4).clamp(5, 60)) && baseline.is_none() {
                normalize(hwnd, edit, strip, &root);
                baseline = Some(counts());
                last = Instant::now();
                println!("Normalized baseline: {:?}", baseline.unwrap());
            }
            let c = counts();
            println!(
                "{:>4}s  handles {:>5}  gdi {:>4}  user {:>4}  actions {actions}",
                start.elapsed().as_secs(),
                c.0,
                c.1,
                c.2
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    stop.store(true, Ordering::Relaxed);
    let churned = churn.join().unwrap();
    normalize(hwnd, edit, strip, &root);
    let end = counts();
    unsafe { DestroyWindow(hwnd) }.unwrap();
    let b = baseline.unwrap_or(end);
    println!("\n{actions} UI actions, {churned} file changes in {seconds}s");
    println!("longest UI stall: {worst_gap:?}");
    println!(
        "growth since warm-up: handles {:+}, gdi {:+}, user {:+}",
        end.0 as i64 - b.0 as i64,
        end.1 as i64 - b.1 as i64,
        end.2 as i64 - b.2 as i64
    );
    while ui::running_operations() > 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = std::fs::remove_dir_all(&root);
}
