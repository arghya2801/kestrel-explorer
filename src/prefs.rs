//! Preferences that shape how Kestrel looks and behaves, beyond the view state in `settings`:
//! the Settings pages for appearance, files, tabs, the sidebar and the terminal. They are one
//! set per process, read by the formatting, sorting and file-operation code as well as the
//! windows, and saved in settings.txt with the rest.
use std::sync::RwLock;

/// How dates read in the file list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dates {
    /// The system's short date and time.
    #[default]
    System,
    /// "Today 14:02", "Yesterday 09:13", then the system's short date.
    Relative,
    /// 2026-11-04 22:33.
    Iso,
}

/// Which program Open terminal here starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Terminal {
    #[default]
    WindowsPowerShell,
    PowerShell7,
    WindowsTerminal,
    CommandPrompt,
}

/// Where a new tab opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NewTab {
    #[default]
    CurrentFolder,
    ThisPc,
    UserFolder,
    Downloads,
}

/// Which tabs show a close button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TabClose {
    /// The active tab and the one under the mouse.
    #[default]
    ActiveAndHovered,
    Every,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    // Files and folders
    pub show_extensions: bool,
    /// Protected operating system files: hidden and system both. Shown only with this on.
    pub show_system: bool,
    pub folders_first: bool,
    pub single_click: bool,
    /// Double-clicking empty space in the file list goes up a folder.
    pub up_on_double_click: bool,
    pub confirm_recycle: bool,
    /// 1 kB = 1,000 bytes, rather than Explorer's 1 KB = 1,024.
    pub decimal_sizes: bool,
    pub dates: Dates,
    // Appearance
    /// An index into `ACCENTS`; 0 is Windows' accent color.
    pub accent: usize,
    /// An index into `TEXT_SIZES`.
    pub text_size: usize,
    /// An index into `ROW_SPACING`.
    pub row_spacing: usize,
    pub toolbar_labels: bool,
    pub status_bar: bool,
    pub full_path_title: bool,
    // Tabs
    pub new_tab: NewTab,
    pub tab_close: TabClose,
    pub last_tab_closes_window: bool,
    // Sidebar
    pub sidebar_quick_access: bool,
    pub sidebar_this_pc: bool,
    pub sidebar_network: bool,
    /// The sidebar expands to each folder opened.
    pub sidebar_follows: bool,
    // Terminal
    pub terminal: Terminal,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            show_extensions: true,
            show_system: false,
            folders_first: true,
            single_click: false,
            up_on_double_click: false,
            confirm_recycle: false,
            decimal_sizes: false,
            dates: Dates::System,
            accent: 0,
            text_size: 1,
            row_spacing: 0,
            toolbar_labels: true,
            status_bar: true,
            full_path_title: true,
            new_tab: NewTab::CurrentFolder,
            tab_close: TabClose::ActiveAndHovered,
            last_tab_closes_window: false,
            sidebar_quick_access: true,
            sidebar_this_pc: true,
            sidebar_network: true,
            sidebar_follows: false,
            terminal: Terminal::WindowsPowerShell,
        }
    }
}

/// Accent colors to pick from, as COLORREF (0x00BBGGRR). The first is Windows' own.
pub const ACCENTS: [(&str, u32); 9] = [
    ("Windows accent color", 0),
    ("Blue", 0x00D4_7800),
    ("Teal", 0x0088_8800),
    ("Green", 0x0042_8A10),
    ("Purple", 0x00B4_4F88),
    ("Pink", 0x008C_33E3),
    ("Orange", 0x0000_63F7),
    ("Red", 0x0023_11E8),
    ("Graphite", 0x0069_6969),
];

/// Text sizes, as a percentage of Windows' menu font.
pub const TEXT_SIZES: [(&str, i32); 4] = [("Small", 90), ("Default", 100), ("Large", 115), ("Larger", 130)];

/// File list row heights in 96-DPI pixels; 0 is the list's own.
pub const ROW_SPACING: [(&str, i32); 3] = [("Default", 0), ("Roomy", 28), ("Roomier", 34)];

const DATES: [(Dates, &str); 3] =
    [(Dates::System, "system"), (Dates::Relative, "relative"), (Dates::Iso, "iso")];
const TERMINALS: [(Terminal, &str); 4] = [
    (Terminal::WindowsPowerShell, "powershell"),
    (Terminal::PowerShell7, "pwsh"),
    (Terminal::WindowsTerminal, "wt"),
    (Terminal::CommandPrompt, "cmd"),
];
const NEW_TABS: [(NewTab, &str); 4] = [
    (NewTab::CurrentFolder, "current"),
    (NewTab::ThisPc, "this_pc"),
    (NewTab::UserFolder, "user"),
    (NewTab::Downloads, "downloads"),
];
const TAB_CLOSES: [(TabClose, &str); 3] =
    [(TabClose::ActiveAndHovered, "active"), (TabClose::Every, "every"), (TabClose::Never, "never")];

fn named<T: Copy>(table: &[(T, &'static str)], value: &str) -> Option<T> {
    table.iter().find(|(_, name)| *name == value).map(|&(v, _)| v)
}
fn name_of<T: Copy + PartialEq>(table: &[(T, &'static str)], value: T) -> &'static str {
    table.iter().find(|(v, _)| *v == value).map_or("", |&(_, name)| name)
}

impl Prefs {
    /// Reads one `key=value` line of settings.txt. Returns false for keys that aren't ours.
    /// A bad value keeps the default.
    pub fn parse(&mut self, key: &str, value: &str) -> bool {
        let flag = |v: &str, default: bool| match v {
            "true" => true,
            "false" => false,
            _ => default,
        };
        let index =
            |v: &str, len: usize, default: usize| v.parse().ok().filter(|&i| i < len).unwrap_or(default);
        match key {
            "show_extensions" => self.show_extensions = flag(value, self.show_extensions),
            "show_system" => self.show_system = flag(value, self.show_system),
            "folders_first" => self.folders_first = flag(value, self.folders_first),
            "single_click" => self.single_click = flag(value, self.single_click),
            "up_on_double_click" => self.up_on_double_click = flag(value, self.up_on_double_click),
            "confirm_recycle" => self.confirm_recycle = flag(value, self.confirm_recycle),
            "decimal_sizes" => self.decimal_sizes = flag(value, self.decimal_sizes),
            "dates" => self.dates = named(&DATES, value).unwrap_or(self.dates),
            "accent" => self.accent = index(value, ACCENTS.len(), self.accent),
            "text_size" => self.text_size = index(value, TEXT_SIZES.len(), self.text_size),
            "row_spacing" => self.row_spacing = index(value, ROW_SPACING.len(), self.row_spacing),
            "toolbar_labels" => self.toolbar_labels = flag(value, self.toolbar_labels),
            "status_bar" => self.status_bar = flag(value, self.status_bar),
            "full_path_title" => self.full_path_title = flag(value, self.full_path_title),
            "new_tab" => self.new_tab = named(&NEW_TABS, value).unwrap_or(self.new_tab),
            "tab_close" => self.tab_close = named(&TAB_CLOSES, value).unwrap_or(self.tab_close),
            "last_tab_closes_window" => {
                self.last_tab_closes_window = flag(value, self.last_tab_closes_window)
            }
            "sidebar_quick_access" => self.sidebar_quick_access = flag(value, self.sidebar_quick_access),
            "sidebar_this_pc" => self.sidebar_this_pc = flag(value, self.sidebar_this_pc),
            "sidebar_network" => self.sidebar_network = flag(value, self.sidebar_network),
            "sidebar_follows" => self.sidebar_follows = flag(value, self.sidebar_follows),
            "terminal" => self.terminal = named(&TERMINALS, value).unwrap_or(self.terminal),
            _ => return false,
        }
        true
    }

    /// These preferences as settings.txt lines.
    pub fn to_text(&self) -> String {
        let lines = [
            format!("show_extensions={}", self.show_extensions),
            format!("show_system={}", self.show_system),
            format!("folders_first={}", self.folders_first),
            format!("single_click={}", self.single_click),
            format!("up_on_double_click={}", self.up_on_double_click),
            format!("confirm_recycle={}", self.confirm_recycle),
            format!("decimal_sizes={}", self.decimal_sizes),
            format!("dates={}", name_of(&DATES, self.dates)),
            format!("accent={}", self.accent),
            format!("text_size={}", self.text_size),
            format!("row_spacing={}", self.row_spacing),
            format!("toolbar_labels={}", self.toolbar_labels),
            format!("status_bar={}", self.status_bar),
            format!("full_path_title={}", self.full_path_title),
            format!("new_tab={}", name_of(&NEW_TABS, self.new_tab)),
            format!("tab_close={}", name_of(&TAB_CLOSES, self.tab_close)),
            format!("last_tab_closes_window={}", self.last_tab_closes_window),
            format!("sidebar_quick_access={}", self.sidebar_quick_access),
            format!("sidebar_this_pc={}", self.sidebar_this_pc),
            format!("sidebar_network={}", self.sidebar_network),
            format!("sidebar_follows={}", self.sidebar_follows),
            format!("terminal={}", name_of(&TERMINALS, self.terminal)),
        ];
        lines.iter().map(|l| format!("{l}\n")).collect()
    }
}

static CURRENT: RwLock<Option<Prefs>> = RwLock::new(None);

/// This process's preferences.
pub fn get() -> Prefs {
    CURRENT.read().unwrap_or_else(|e| e.into_inner()).unwrap_or_default()
}

/// The preferences a window starts with, unless an earlier window already set them.
pub fn init(prefs: Prefs) {
    let mut current = CURRENT.write().unwrap_or_else(|e| e.into_inner());
    if current.is_none() {
        *current = Some(prefs);
    }
}

/// Replaces this process's preferences, as Settings or startup do.
pub fn set(prefs: Prefs) {
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = Some(prefs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferences_round_trip_and_bad_values_keep_defaults() {
        let p = Prefs {
            show_extensions: false,
            show_system: true,
            folders_first: false,
            dates: Dates::Iso,
            accent: 3,
            text_size: 3,
            row_spacing: 2,
            new_tab: NewTab::Downloads,
            tab_close: TabClose::Never,
            terminal: Terminal::WindowsTerminal,
            sidebar_network: false,
            ..Prefs::default()
        };
        let mut back = Prefs::default();
        for line in p.to_text().lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert!(back.parse(k, v), "{k}");
        }
        assert_eq!(back, p);
        let mut bad = Prefs::default();
        for (k, v) in [("accent", "99"), ("dates", "soon"), ("show_system", "maybe"), ("text_size", "-1")] {
            bad.parse(k, v);
        }
        assert_eq!(bad, Prefs::default());
        assert!(!bad.parse("window", "1,2,3,4"));
    }
}
