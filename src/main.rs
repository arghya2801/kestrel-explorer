#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::time::Duration;

use kestrel::settings::Settings;
use kestrel::{crash, shellmenu, ui, watchdog};
use windows::Win32::Graphics::Gdi::{RDW_ALLCHILDREN, RDW_UPDATENOW, RedrawWindow};
use windows::Win32::UI::WindowsAndMessaging::{SW_SHOW, SW_SHOWMAXIMIZED, ShowWindow};

fn main() {
    // Started as the helper that shows Windows' full right-click menu?
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|a| {
        [shellmenu::MENU_FLAG, shellmenu::LIST_FLAG, shellmenu::PROPERTIES_FLAG]
            .contains(&a.to_str().unwrap_or(""))
    }) {
        crash::install();
        std::process::exit(shellmenu::run_helper(&args));
    }
    ui::startup_mark("main");
    if let Some(old) = Settings::old_file() {
        Settings::migrate(&old, &Settings::file());
    }
    let mut settings = Settings::load_from(&Settings::file());
    let explicit = std::env::args_os().nth(1).is_some();
    let mut windows =
        if settings.restore_session && !explicit { kestrel::session::restored() } else { Vec::new() };
    if !windows.is_empty() {
        let theme = settings.theme;
        let pins = settings.pins.clone();
        let recent = settings.recent.clone();
        settings = windows.remove(0);
        settings.theme = theme;
        settings.pins = pins.clone();
        settings.recent = recent.clone();
        for window in &mut windows {
            window.theme = theme;
            window.pins = pins.clone();
            window.recent = recent.clone();
        }
    } else {
        settings.tabs.clear();
    }
    kestrel::session::begin();
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
    for window in windows {
        ui::open_restored_window(window);
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

    // Other windows (Ctrl+N) keep the app open until they're closed too.
    ui::wait_for_other_windows();
    // The windows are gone, but copies and moves keep going until done.
    while ui::running_operations() > 0 {
        std::thread::sleep(Duration::from_millis(100));
    }
    if let Err(e) = kestrel::session::finish() {
        crash::log(&format!("Session not saved: {e}"));
    }
}
