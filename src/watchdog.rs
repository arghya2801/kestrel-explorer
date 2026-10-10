//! Notices when the UI thread stops answering. That should never happen;
//! when it does, the log and a minidump of every thread's stack say why.

use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{IsWindow, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_NULL};

/// Pings `hwnd` every `period` from a background thread. Calls `on_hang` once
/// each time the window goes from answering to not answering within `timeout`.
/// Ends when the window is destroyed.
pub fn spawn(
    hwnd: isize,
    period: Duration,
    timeout: Duration,
    on_hang: impl Fn() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name("watchdog".into()).spawn(move || {
        let hwnd = HWND(hwnd as _);
        let mut hung = false;
        while unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            let answered = unsafe {
                SendMessageTimeoutW(
                    hwnd,
                    WM_NULL,
                    WPARAM(0),
                    LPARAM(0),
                    SMTO_ABORTIFHUNG,
                    timeout.as_millis() as u32,
                    None,
                )
            }
            .0 != 0;
            if !answered && !hung && unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                on_hang();
            }
            hung = !answered;
            std::thread::sleep(period);
        }
    })
}

/// What the app does on a hang: log it, and keep one minidump per run.
pub fn report_hang() {
    static DUMPED: AtomicBool = AtomicBool::new(false);
    let dump = if DUMPED.swap(true, Ordering::SeqCst) {
        None
    } else {
        crate::crash::write_minidump(std::ptr::null())
    };
    crate::crash::log(&format!("UI thread not responding; dump: {dump:?}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;
    use windows::Win32::UI::WindowsAndMessaging::*;
    use windows::core::w;

    fn pump_for(d: Duration) {
        let end = Instant::now() + d;
        let mut msg = MSG::default();
        while Instant::now() < end {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn reports_a_frozen_window_once_and_stops_when_it_closes() {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                None,
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap();
        let hangs = Arc::new(AtomicUsize::new(0));
        let h2 = hangs.clone();
        let period = Duration::from_millis(20);
        let dog = spawn(hwnd.0 as isize, period, Duration::from_millis(100), move || {
            h2.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();

        pump_for(Duration::from_millis(300));
        assert_eq!(hangs.load(Ordering::SeqCst), 0, "a responsive window is not a hang");

        std::thread::sleep(Duration::from_millis(600)); // frozen: no messages handled
        pump_for(Duration::from_millis(300));
        assert_eq!(hangs.load(Ordering::SeqCst), 1, "one freeze, one report");

        unsafe { DestroyWindow(hwnd) }.unwrap();
        dog.join().unwrap();
    }
}
