#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::time::Duration;

use file_explorer_pro::settings::Settings;
use file_explorer_pro::{crash, ui, watchdog};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, SW_SHOWMAXIMIZED, ShowWindow};

fn main() {
    let settings = Settings::load_from(&Settings::file());
    // A folder given on the command line wins; then where the user left off; then their home.
    let start = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| settings.last_folder.clone())
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(r"C:\"));
    if let Err(e) = ui::init_process() {
        crash::log(&format!("startup failed: {e}"));
        return;
    }
    let hwnd = match ui::create_main_window(&start, &settings) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            crash::log(&format!("window creation failed: {e}"));
            return;
        }
    };
    unsafe {
        let _ = ShowWindow(hwnd, if settings.maximized { SW_SHOWMAXIMIZED } else { SW_SHOW });
    }
    if let Err(e) = watchdog::spawn(
        hwnd.0 as isize,
        Duration::from_secs(1),
        Duration::from_secs(2),
        watchdog::report_hang,
    ) {
        crash::log(&format!("no hang watchdog: {e}"));
    }
    ui::run_message_loop();

    if let Some(s) = ui::take_final_settings()
        && let Err(e) = s.save_to(&Settings::file())
    {
        crash::log(&format!("settings not saved: {e}"));
    }
    // The window is gone, but copies and moves keep going until done.
    while ui::running_operations() > 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
}
