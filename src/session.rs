//! One owner for preferences and the windows restored on the next launch.
use crate::settings::Settings;
use std::sync::Mutex;

#[derive(Clone, Default)]
struct Session {
    enabled: bool,
    windows: Vec<(isize, Settings)>,
    primary: isize,
    closing_primary: bool,
    preferences: Option<Settings>,
    written: String,
}
static SESSION: Mutex<Session> = Mutex::new(Session {
    enabled: false,
    windows: Vec::new(),
    primary: 0,
    closing_primary: false,
    preferences: None,
    written: String::new(),
});

// Serialize disk writes without holding the state lock that window threads need.
static WRITER: Mutex<()> = Mutex::new(());

pub fn begin() {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = Session { enabled: true, ..Default::default() };
}
pub fn restored() -> Vec<Settings> {
    std::fs::read_to_string(crate::crash::data_dir().join("session.txt"))
        .map(|text| text.split("[window]\n").skip(1).map(Settings::parse).collect())
        .unwrap_or_default()
}
pub(crate) fn register(hwnd: isize, settings: Settings) {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if !s.enabled {
        return;
    }
    if s.primary == 0 {
        s.primary = hwnd;
    }
    s.windows.push((hwnd, settings));
}
pub(crate) fn closing(hwnd: isize, settings: Settings) {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if !s.enabled {
        return;
    }
    if hwnd == s.primary {
        s.closing_primary = true;
    }
    if !s.closing_primary {
        s.windows.retain(|(h, _)| *h != hwnd);
    } else if let Some((_, saved)) = s.windows.iter_mut().find(|(h, _)| *h == hwnd) {
        *saved = settings;
    }
}
/// Refreshes an open window's record, so a checkpoint or forced shutdown keeps its current state.
pub(crate) fn update(hwnd: isize, settings: Settings) {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, saved)) = s.windows.iter_mut().find(|(h, _)| *h == hwnd) {
        *saved = settings;
    }
}
pub(crate) fn preferences(settings: Settings) -> Vec<isize> {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    s.preferences = Some(settings);
    s.windows.iter().map(|(h, _)| *h).collect()
}
/// Quick access as last saved: the pins and the known folders taken out.
pub(crate) fn pins() -> Option<(Vec<std::path::PathBuf>, Vec<std::path::PathBuf>)> {
    SESSION
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .preferences
        .as_ref()
        .map(|s| (s.pins.clone(), s.unpinned.clone()))
}
pub(crate) fn theme() -> Option<crate::theme::ThemeChoice> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).preferences.as_ref().map(|s| s.theme)
}
pub(crate) fn flush_preferences() -> std::io::Result<()> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let preferences = SESSION.lock().unwrap_or_else(|e| e.into_inner()).preferences.clone();
    if let Some(p) = preferences {
        p.save_to(&Settings::file())?;
    }
    Ok(())
}
pub fn finish() -> std::io::Result<()> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let s = SESSION.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !s.enabled {
        return Ok(());
    }
    if let Some(p) = s.preferences.as_ref().or_else(|| s.windows.first().map(|w| &w.1)) {
        let mut p = p.clone();
        for (_, window) in s.windows.iter().rev() {
            for path in window.recent.iter().rev() {
                crate::settings::remember(&mut p.recent, path.clone());
            }
        }
        p.save_to(&Settings::file())?;
    }
    write_windows(&s)
}
/// Writes the open windows to session.txt, so a crash or kill restores them on the next launch.
pub(crate) fn checkpoint() -> std::io::Result<()> {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let s = SESSION.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !s.enabled { Ok(()) } else { write_windows(&s) }
}
fn write_windows(s: &Session) -> std::io::Result<()> {
    let mut text = String::new();
    for (_, window) in &s.windows {
        text += "[window]
";
        text += &window.to_text();
    }
    if text == s.written {
        return Ok(());
    }
    let path = crate::crash::data_dir().join("session.txt");
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    use std::io::Write;
    let mut file = std::fs::File::create(&tmp)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)?;
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).written = text;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn periodic_saves_leave_the_ui_responsive_and_do_not_queue_more_writers() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{DestroyWindow, SendMessageW, WM_TIMER};
        crate::ui::init_process().unwrap();
        let hwnd = crate::ui::create_main_window(&std::env::temp_dir(), &Settings::default()).unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let slow_disk = std::thread::spawn(move || {
            let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        });
        ready_rx.recv().unwrap();
        let operations = crate::ui::running_operations();
        let started = Instant::now();
        unsafe {
            SendMessageW(hwnd, WM_TIMER, Some(WPARAM(8)), Some(LPARAM(0)));
            SendMessageW(hwnd, WM_TIMER, Some(WPARAM(8)), Some(LPARAM(0)));
        }
        let elapsed = started.elapsed();
        let writers = crate::ui::running_operations() - operations;
        let state_available = SESSION.try_lock().is_ok();
        let _ = release_tx.send(());
        slow_disk.join().unwrap();
        unsafe { DestroyWindow(hwnd).unwrap() };
        assert!(elapsed < Duration::from_secs(1), "UI waited for storage: {elapsed:?}");
        assert_eq!(writers, 1, "only one periodic writer per window");
        assert!(state_available, "the window state lock must remain available");
    }

    #[test]
    fn windows_keep_distinct_tab_records() {
        let a = Settings::parse("last_folder=C:\\a\n");
        let b = Settings::parse("last_folder=D:\\b\n");
        let text = format!("[window]\n{}[window]\n{}", a.to_text(), b.to_text());
        let parsed: Vec<_> = text.split("[window]\n").skip(1).map(Settings::parse).collect();
        assert_eq!(parsed, [a, b]);
    }
}
