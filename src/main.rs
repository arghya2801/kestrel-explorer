#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::time::Duration;

use file_explorer_pro::settings::Settings;
use file_explorer_pro::{crash, shellmenu, ui, watchdog};
use windows::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, SW_SHOWMAXIMIZED, ShowWindow};

fn main() {
    // Started as the helper that shows Windows' full right-click menu?
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| a == shellmenu::MENU_FLAG || a == shellmenu::LIST_FLAG) {
        crash::install();
        std::process::exit(shellmenu::run_helper(&args));
    }
    ui::startup_mark("main");
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
    ui::startup_mark("init");
    let hwnd = match ui::create_main_window(&start, &settings) {
        Ok(hwnd) => hwnd,
        Err(e) => {
            crash::log(&format!("window creation failed: {e}"));
            return;
        }
    };
    unsafe {
        let _ = ShowWindow(hwnd, if settings.maximized { SW_SHOWMAXIMIZED } else { SW_SHOW });
        // Paint now: Windows otherwise paints only once every queued message is handled.
        let _ = RedrawWindow(Some(hwnd), None, None, RDW_UPDATENOW | RDW_ALLCHILDREN);
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
    // Other windows (Ctrl+N) keep the app open until they're closed too.
    ui::wait_for_other_windows();
    // The windows are gone, but copies and moves keep going until done.
    while ui::running_operations() > 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
}
