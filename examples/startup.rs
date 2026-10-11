//! cargo run --release --example startup -- [samples]
//! Opens and closes only its own test windows; records first-paint logs, working set and executable size.
use std::os::windows::io::AsRawHandle;
use std::{
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HANDLE, HWND, LPARAM, WPARAM},
    System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
    UI::WindowsAndMessaging::*,
};
fn close(pid: u32) {
    unsafe extern "system" fn each(hwnd: HWND, param: LPARAM) -> windows::core::BOOL {
        let mut owner = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == param.0 as u32 && GetWindow(hwnd, GW_OWNER).is_err() {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        true.into()
    }
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(pid as isize));
    }
}
fn main() {
    let count = std::env::args().nth(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(7);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let executable = root.join("target/release/kestrel.exe");
    let data = root.join("target").join(format!("startup-probe-{}", std::process::id()));
    std::fs::create_dir_all(&data).unwrap();
    let log = data.join("Kestrel/log.txt");
    println!("Executable: {} bytes", std::fs::metadata(&executable).unwrap().len());
    for sample in 0..count {
        let before = std::fs::read_to_string(&log).unwrap_or_default().len();
        let mut child =
            Command::new(&executable).arg(r"C:\Windows\System32").env("LOCALAPPDATA", &data).spawn().unwrap();
        let started = Instant::now();
        let line = loop {
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            if let Some(line) = text.get(before..).and_then(|s| s.lines().find(|l| l.contains("startup (ms")))
            {
                break line.to_owned();
            }
            if started.elapsed() > Duration::from_secs(10) || child.try_wait().unwrap().is_some() {
                let _ = child.kill();
                let _ = child.wait();
                panic!("No first-paint sample");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let mut memory =
            PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
        unsafe {
            let _ = GetProcessMemoryInfo(
                HANDLE(child.as_raw_handle()),
                &mut memory,
                size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
            );
        }
        println!("Sample {sample}: {line}; working set {} KiB", memory.WorkingSetSize / 1024);
        close(child.id());
        let ending = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if ending.elapsed() > Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Sample did not close");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    println!("Logs: {}", log.display());
}
