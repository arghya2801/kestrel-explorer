#![windows_subsystem = "windows"]

use std::path::PathBuf;

use file_explorer_pro::{crash, ui};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, ShowWindow};

fn main() {
    let start = std::env::args_os()
        .nth(1)
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from(r"C:\"), PathBuf::from);
    if let Err(e) = ui::init_process() {
        crash::log(&format!("startup failed: {e}"));
        return;
    }
    match ui::create_main_window(&start) {
        Ok(hwnd) => unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
        },
        Err(e) => {
            crash::log(&format!("window creation failed: {e}"));
            return;
        }
    }
    ui::run_message_loop();
}
