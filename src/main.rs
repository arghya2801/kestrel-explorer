#![windows_subsystem = "windows"]

use file_explorer_pro::{crash, ui};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, ShowWindow};

fn main() {
    if let Err(e) = ui::init_process() {
        crash::log(&format!("startup failed: {e}"));
        return;
    }
    match ui::create_main_window() {
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
