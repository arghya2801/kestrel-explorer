//! Remembered window and view state, as `key=value` lines in
//! `%LOCALAPPDATA%\Kestrel\settings.txt`. Saved by writing a temp file
//! and renaming it over the old one, so a crash mid-save leaves the old file.
//! Anything unreadable falls back to its default.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::sort::{Sort, SortKey};
use crate::theme::ThemeChoice;

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Restored (not maximized) bounds: x, y, width, height.
    pub window: Option<(i32, i32, i32, i32)>,
    pub maximized: bool,
    pub columns: Vec<i32>,
    pub sort: Sort,
    pub show_hidden: bool,
    pub split: Option<i32>,
    pub preview: bool,
    pub grid: bool,
    /// Thumbnail edge in pixels at 100% scaling; one of `THUMB_SIZES`.
    pub thumb_size: i32,
    pub dual_pane: bool,
    pub pane_ratio: i32,
    pub preview_width: Option<i32>,
    pub last_folder: Option<PathBuf>,
    pub theme: ThemeChoice,
    pub restore_session: bool,
    pub tabs: Vec<SavedTab>,
    pub active_tab: usize,
    pub pins: Vec<PathBuf>,
    /// Known folders (Desktop, Downloads...) removed from Quick access.
    pub unpinned: Vec<PathBuf>,
    /// Tabs listed above the sidebar instead of along the top.
    pub vertical_tabs: bool,
    pub recent: Vec<PathBuf>,
    /// Folders shown grouped by date.
    pub grouped: Vec<PathBuf>,
    /// Folders sorted other than `sort`, the default.
    pub folder_sorts: Vec<(PathBuf, Sort)>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            window: None,
            maximized: false,
            columns: Vec::new(),
            sort: Sort { key: SortKey::Name, descending: false },
            show_hidden: false,
            split: None,
            preview: false,
            grid: false,
            thumb_size: 96,
            dual_pane: false,
            pane_ratio: 500,
            preview_width: None,
            last_folder: None,
            theme: ThemeChoice::System,
            restore_session: true,
            tabs: Vec::new(),
            active_tab: 0,
            pins: Vec::new(),
            unpinned: Vec::new(),
            vertical_tabs: false,
            recent: Vec::new(),
            grouped: Vec::new(),
            folder_sorts: Vec::new(),
        }
    }
}

/// Thumbnail sizes offered in View and stepped through with Ctrl+mouse wheel.
pub const THUMB_SIZES: [(i32, &str); 4] = [
    (64, "Small thumbnails"),
    (96, "Medium thumbnails"),
    (160, "Large thumbnails"),
    (256, "Extra large thumbnails"),
];

const KEYS: [(SortKey, &str); 6] = [
    (SortKey::Name, "Name"),
    (SortKey::Size, "Size"),
    (SortKey::Modified, "Modified"),
    (SortKey::Type, "Type"),
    (SortKey::Created, "Created"),
    (SortKey::Attributes, "Attributes"),
];

fn ints(v: &str) -> Option<Vec<i32>> {
    v.split(',').map(|n| n.trim().parse().ok()).collect()
}

fn flag(v: &str) -> Option<bool> {
    match v {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

impl Settings {
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let Some((key, value)) = line.trim_start_matches('\u{feff}').split_once('=') else { continue };
            let value = value.trim_end_matches('\r');
            match key {
                "window" => {
                    s.window = ints(value)
                        .filter(|v| v.len() == 4 && v[2] > 0 && v[3] > 0)
                        .map(|v| (v[0], v[1], v[2], v[3]))
                }
                "maximized" => s.maximized = flag(value).unwrap_or(false),
                // Width 0 is a column hidden from the header menu.
                "columns" => {
                    s.columns = ints(value).filter(|v| v.iter().all(|&w| w >= 0)).unwrap_or_default()
                }
                "sort" => {
                    if let Some((k, d)) = value.split_once(',')
                        && let (Some(&(key, _)), Some(descending)) =
                            (KEYS.iter().find(|(_, n)| *n == k), flag(d))
                    {
                        s.sort = Sort { key, descending };
                    }
                }
                "show_hidden" => s.show_hidden = flag(value).unwrap_or(false),
                "grid" => s.grid = flag(value).unwrap_or(false),
                "thumb_size" => {
                    s.thumb_size =
                        value.parse().ok().filter(|v| THUMB_SIZES.iter().any(|t| t.0 == *v)).unwrap_or(96)
                }
                "preview" => s.preview = flag(value).unwrap_or(false),
                "dual_pane" => s.dual_pane = flag(value).unwrap_or(false),
                "pane_ratio" => {
                    s.pane_ratio =
                        value.parse::<i32>().ok().filter(|v| (100..=900).contains(v)).unwrap_or(500)
                }
                "preview_width" => {
                    s.preview_width = value.parse::<i32>().ok().filter(|v| *v >= 100 && *v <= 10000)
                }
                "split" => s.split = value.parse().ok().filter(|&w: &i32| w > 0),
                "theme" => s.theme = ThemeChoice::parse(value).unwrap_or(ThemeChoice::System),
                "restore_session" => s.restore_session = flag(value).unwrap_or(true),
                "pin" => {
                    if let Some(p) = decode_path(value).filter(|p| rememberable(p)) {
                        s.pins.push(p);
                    }
                }
                "unpin" => {
                    if let Some(p) = decode_path(value).filter(|p| rememberable(p)) {
                        s.unpinned.push(p);
                    }
                }
                "vertical_tabs" => s.vertical_tabs = flag(value).unwrap_or(false),
                "folder_sort" => {
                    let mut parts = value.splitn(3, ',');
                    if let (Some(p), Some(k), Some(d)) = (parts.next(), parts.next(), parts.next())
                        && let Some(p) = decode_path(p).filter(|p| rememberable(p))
                        && let (Some(&(key, _)), Some(descending)) =
                            (KEYS.iter().find(|(_, n)| *n == k), flag(d))
                    {
                        s.folder_sorts.push((p, Sort { key, descending }));
                    }
                }
                "group" => {
                    if let Some(p) = decode_path(value).filter(|p| rememberable(p)) {
                        s.grouped.push(p);
                    }
                }
                "recent" => {
                    if let Some(p) = decode_path(value).filter(|p| rememberable(p))
                        && s.recent.len() < 20
                    {
                        s.recent.push(p);
                    }
                }
                "tab" => {
                    if let Some(tab) = SavedTab::parse(value) {
                        s.tabs.push(tab);
                    }
                }
                "active_tab" => s.active_tab = value.parse().unwrap_or(0),
                "last_folder" => s.last_folder = (!value.is_empty()).then(|| PathBuf::from(value)),
                _ => {}
            }
        }
        s.active_tab = s.active_tab.min(s.tabs.len().saturating_sub(1));
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some((x, y, w, h)) = self.window {
            out += &format!("window={x},{y},{w},{h}\n");
        }
        out += &format!("maximized={}\n", self.maximized);
        if !self.columns.is_empty() {
            let cols: Vec<String> = self.columns.iter().map(i32::to_string).collect();
            out += &format!("columns={}\n", cols.join(","));
        }
        let key = KEYS.iter().find(|(k, _)| *k == self.sort.key).map_or("Name", |k| k.1);
        out += &format!("sort={key},{}\n", self.sort.descending);
        out += &format!("show_hidden={}\n", self.show_hidden);
        out += &format!("preview={}\ngrid={}\nthumb_size={}\n", self.preview, self.grid, self.thumb_size);
        out += &format!("dual_pane={}\npane_ratio={}\n", self.dual_pane, self.pane_ratio);
        if let Some(width) = self.preview_width {
            out += &format!("preview_width={width}\n");
        }
        if let Some(w) = self.split {
            out += &format!("split={w}\n");
        }
        out += &format!("theme={}\n", self.theme.name());
        if let Some(p) = &self.last_folder {
            out += &format!("last_folder={}\n", p.display());
        }
        out += &format!("restore_session={}\n", self.restore_session);
        for p in self.pins.iter().filter(|p| rememberable(p)) {
            out += &format!("pin={}\n", encode_path(p));
        }
        for p in self.unpinned.iter().filter(|p| rememberable(p)) {
            out += &format!("unpin={}\n", encode_path(p));
        }
        out += &format!("vertical_tabs={}\n", self.vertical_tabs);
        for (p, sort) in self.folder_sorts.iter().filter(|(p, _)| rememberable(p)) {
            let key = KEYS.iter().find(|(k, _)| *k == sort.key).map_or("Name", |k| k.1);
            out += &format!("folder_sort={},{key},{}\n", encode_path(p), sort.descending);
        }
        for p in self.grouped.iter().filter(|p| rememberable(p)) {
            out += &format!(
                "group={}
",
                encode_path(p)
            );
        }
        for p in self.recent.iter().filter(|p| rememberable(p)).take(20) {
            out += &format!("recent={}\n", encode_path(p));
        }
        out += &format!("active_tab={}\n", self.active_tab);
        for tab in &self.tabs {
            out += &format!("tab={}\n", tab.text());
        }
        out
    }

    pub fn file() -> PathBuf {
        crate::crash::data_dir().join("settings.txt")
    }

    /// Settings saved before the app was renamed (from "File Explorer Pro").
    pub fn old_file() -> Option<PathBuf> {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(PathBuf::from(base).join("FileExplorerPro").join("settings.txt"))
    }

    /// Copies `old` to `new` once: only if `new` doesn't exist yet. Returns true if it copied.
    pub fn migrate(old: &Path, new: &Path) -> bool {
        if new.exists() || !old.exists() {
            return false;
        }
        new.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::copy(old, new).is_ok()
    }

    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path).map(|t| Settings::parse(&t)).unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(self.to_text().as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path) // replaces the old file in one step
    }
}

/// Moves (and if needed shrinks) a saved window rect `(x, y, w, h)` so it lies
/// fully inside a monitor's work area `(left, top, right, bottom)`. Screens
/// change between runs; a window half off-screen is hard to grab.
pub fn fit_window(
    (x, y, w, h): (i32, i32, i32, i32),
    (l, t, r, b): (i32, i32, i32, i32),
) -> (i32, i32, i32, i32) {
    let (w, h) = (w.min(r - l), h.min(b - t));
    (x.clamp(l, r - w), y.clamp(t, b - h), w, h)
}

pub fn rememberable(path: &Path) -> bool {
    let text = path.to_string_lossy();
    if text.contains("://") || text.contains(['\r', '\n', '\0']) {
        return false;
    }
    if crate::places::is_this_pc(path) {
        return true;
    }
    if let Some(rest) = text.strip_prefix(r"\\") {
        return !rest.split('\\').next().unwrap_or_default().contains('@');
    }
    path.is_absolute()
}
pub fn remember(recent: &mut Vec<PathBuf>, path: PathBuf) {
    if !rememberable(&path) {
        return;
    }
    recent.retain(|p| !p.to_string_lossy().eq_ignore_ascii_case(&path.to_string_lossy()));
    recent.insert(0, path);
    recent.truncate(20);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_text() {
        let s = Settings {
            window: Some((10, -20, 1200, 800)),
            maximized: true,
            columns: vec![300, 150, 140, 90],
            sort: Sort { key: SortKey::Size, descending: true },
            show_hidden: true,
            split: Some(260),
            preview: true,
            grid: true,
            thumb_size: 160,
            dual_pane: true,
            pane_ratio: 650,
            preview_width: Some(350),
            last_folder: Some(PathBuf::from(r"C:\Users\me\Ünï =x")),
            theme: ThemeChoice::Light,
            restore_session: true,
            tabs: vec![SavedTab {
                folder: PathBuf::from(r"C:\a,b=Ü"),
                other: Some(PathBuf::from(r"D:\")),
                split: true,
                right_active: true,
                ratio: 600,
            }],
            active_tab: 0,
            pins: vec![PathBuf::from(r"C:\pinned,=Ü")],
            unpinned: vec![PathBuf::from(r"C:\Users\me\Music")],
            vertical_tabs: true,
            recent: vec![PathBuf::from(r"D:\recent")],
            grouped: vec![PathBuf::from(r"C:\Users\me\Downloads")],
            folder_sorts: vec![(
                PathBuf::from(r"C:\Users\me\Downloads"),
                Sort { key: SortKey::Modified, descending: true },
            )],
        };
        assert_eq!(Settings::parse(&s.to_text()), s);
    }

    #[test]
    fn recent_locations_are_bounded_deduplicated_and_credential_free() {
        let mut paths = Vec::new();
        for i in 0..25 {
            remember(&mut paths, PathBuf::from(format!(r"C:\folder-{i}")));
        }
        assert_eq!(paths.len(), 20);
        remember(&mut paths, PathBuf::from(r"c:\FOLDER-24"));
        assert_eq!(paths.len(), 20);
        for path in [r"\\user:password@server\share", "https://user:pass@example.com", "relative"] {
            remember(&mut paths, PathBuf::from(path));
        }
        assert_eq!(paths.len(), 20);
        assert_eq!(paths[0], PathBuf::from(r"c:\FOLDER-24"));
    }

    #[test]
    fn garbage_falls_back_per_key() {
        let text = "window=1,2,nope,4\nmaximized=yes\ncolumns=10,abc\nsort=Bogus,true\nshow_hidden=1\n\
                    split=-5\nlast_folder=\nunknown=whatever\nno equals sign\n\u{feff}";
        let s = Settings::parse(text);
        assert_eq!(s.window, None);
        assert!(!s.maximized);
        assert!(s.columns.is_empty());
        assert_eq!(s.sort, Settings::default().sort);
        assert!(!s.show_hidden);
        assert_eq!(s.split, None);
        assert_eq!(s.last_folder, None);
    }

    #[test]
    fn theme_choice_is_remembered() {
        let s = Settings { theme: ThemeChoice::Dark, ..Default::default() };
        assert_eq!(Settings::parse(&s.to_text()).theme, ThemeChoice::Dark);
        assert_eq!(Settings::parse("theme=neon").theme, ThemeChoice::System);
    }

    #[test]
    fn restored_windows_are_moved_fully_onto_the_screen() {
        let work = (0, 0, 1920, 1040);
        assert_eq!(fit_window((100, 50, 800, 600), work), (100, 50, 800, 600), "already fine");
        assert_eq!(fit_window((-856, 104, 1100, 700), work), (0, 104, 1100, 700), "off the left edge");
        assert_eq!(fit_window((1500, 900, 800, 600), work), (1120, 440, 800, 600), "off bottom-right");
        assert_eq!(fit_window((10, 10, 4000, 3000), work), (0, 0, 1920, 1040), "bigger than the screen");
        let second = (1920, 0, 3840, 1080); // a monitor to the right
        assert_eq!(fit_window((1800, 100, 800, 600), second), (1920, 100, 800, 600));
    }

    #[test]
    fn old_settings_are_copied_once() {
        let dir = std::env::temp_dir().join(format!("fep-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (old, new) = (dir.join("old").join("settings.txt"), dir.join("new").join("settings.txt"));
        assert!(!Settings::migrate(&old, &new), "nothing to copy");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "show_hidden=true\n").unwrap();
        assert!(Settings::migrate(&old, &new));
        assert!(Settings::load_from(&new).show_hidden);
        std::fs::write(&old, "show_hidden=false\n").unwrap();
        assert!(!Settings::migrate(&old, &new), "never overwrites");
        assert!(Settings::load_from(&new).show_hidden);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn save_replaces_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("fep-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("settings.txt");
        let mut s = Settings { show_hidden: true, ..Default::default() };
        s.save_to(&file).unwrap();
        assert_eq!(Settings::load_from(&file), s);
        s.split = Some(300);
        s.save_to(&file).unwrap();
        assert_eq!(Settings::load_from(&file), s);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp file left behind");
        assert_eq!(Settings::load_from(&dir.join("missing.txt")), Settings::default());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedTab {
    pub folder: PathBuf,
    pub other: Option<PathBuf>,
    pub split: bool,
    pub right_active: bool,
    pub ratio: i32,
}

// Encode UTF-16 units so commas, line breaks and unpaired surrogates round-trip.
fn encode_path(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().map(|c| format!("{c:04x}")).collect()
}
fn decode_path(text: &str) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    if text.is_empty() || !text.len().is_multiple_of(4) || !text.is_ascii() {
        return None;
    }
    let units: Option<Vec<u16>> = text
        .as_bytes()
        .chunks(4)
        .map(|c| u16::from_str_radix(std::str::from_utf8(c).ok()?, 16).ok())
        .collect();
    let units = units?;
    if units.contains(&0) {
        return None;
    }
    Some(std::ffi::OsString::from_wide(&units).into())
}
impl SavedTab {
    fn parse(text: &str) -> Option<Self> {
        let parts: Vec<_> = text.split(',').collect();
        if parts.len() != 5 {
            return None;
        }
        let ratio = parts[2].parse().ok().filter(|v| (100..=900).contains(v))?;
        let other = if parts[4].is_empty() { None } else { Some(decode_path(parts[4])?) };
        Some(Self {
            split: flag(parts[0])? && other.is_some(),
            right_active: flag(parts[1])?,
            ratio,
            folder: decode_path(parts[3])?,
            other,
        })
    }
    fn text(&self) -> String {
        format!(
            "{},{},{},{},{}",
            self.split,
            self.right_active,
            self.ratio,
            encode_path(&self.folder),
            self.other.as_deref().map(encode_path).unwrap_or_default()
        )
    }
}
