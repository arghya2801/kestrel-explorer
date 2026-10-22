//! One owner for preferences and the windows restored on the next launch.
use crate::settings::Settings;
use std::sync::Mutex;

#[derive(Default)]
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
    let s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(p) = &s.preferences {
        p.save_to(&Settings::file())?;
    }
    Ok(())
}
pub fn finish() -> std::io::Result<()> {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
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
    write_windows(&mut s)
}
/// Writes the open windows to session.txt, so a crash or kill restores them on the next launch.
pub(crate) fn checkpoint() -> std::io::Result<()> {
    let mut s = SESSION.lock().unwrap_or_else(|e| e.into_inner());
    if !s.enabled { Ok(()) } else { write_windows(&mut s) }
}
fn write_windows(s: &mut Session) -> std::io::Result<()> {
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
    s.written = text;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_keep_distinct_tab_records() {
        let a = Settings::parse("last_folder=C:\\a\n");
        let b = Settings::parse("last_folder=D:\\b\n");
        let text = format!("[window]\n{}[window]\n{}", a.to_text(), b.to_text());
        let parsed: Vec<_> = text.split("[window]\n").skip(1).map(Settings::parse).collect();
        assert_eq!(parsed, [a, b]);
    }
}
