//! cargo run --release --example stress -- [seconds]
//!
//! Real window on a temp folder tree. One thread keeps creating, renaming and
//! deleting files; the UI thread does a random action every 50 ms (navigate,
//! back/forward, refresh, re-sort, expand sidebar folders). Prints handle and
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
    let (list, edit, tree) = (find(w!("SysListView32")), find(w!("Edit")), find(w!("SysTreeView32")));

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
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let t = (Instant::now(), thread_cpu());
                if !ui::translate(&msg) {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                slow(t, &format!("message {:#06x} to {}", msg.message, class_of(msg.hwnd)));
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
            let action = rng.next(6);
            match action {
                0 | 1 => {
                    let target =
                        if rng.next(4) == 0 { &root } else { &dirs[rng.next(dirs.len() as u64) as usize] };
                    unsafe {
                        let _ = SetWindowTextW(edit, &HSTRING::from(target.as_os_str()));
                    }
                    post(edit, WM_KEYDOWN, VK_RETURN.0 as usize, 0);
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
        }
        if now >= next_report {
            next_report = now + Duration::from_secs(10);
            let c = counts();
            if start.elapsed() > Duration::from_secs(5) && baseline.is_none() {
                baseline = Some(c);
            }
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
    let _ = std::fs::remove_dir_all(&root);
}
