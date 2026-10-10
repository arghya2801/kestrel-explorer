//! Light or dark, following Windows' "app mode" setting unless the settings
//! file says otherwise. Colors are COLORREF values (0x00BBGGRR).

use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows::core::w;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeChoice {
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    pub fn is_dark(self, system_dark: bool) -> bool {
        match self {
            ThemeChoice::System => system_dark,
            ThemeChoice::Light => false,
            ThemeChoice::Dark => true,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    pub fn parse(s: &str) -> Option<ThemeChoice> {
        [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark]
            .into_iter()
            .find(|c| c.name().eq_ignore_ascii_case(s))
    }
}

/// Windows' "Choose your app mode" setting. Light if it can't be read.
pub fn system_prefers_dark() -> bool {
    let mut value = 1u32;
    let mut size = 4u32;
    let read = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut value as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    read.is_ok() && value == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    /// Window and list background.
    pub window: u32,
    pub text: u32,
    /// Text boxes and the selected tab.
    pub field: u32,
    /// Toolbar, tab strip and status bar.
    pub chrome: u32,
}

impl Palette {
    pub fn new(dark: bool) -> Palette {
        if dark {
            Palette { dark, window: 0x00202020, text: 0x00F0F0F0, field: 0x00383838, chrome: 0x002B2B2B }
        } else {
            Palette { dark, window: 0x00FFFFFF, text: 0x00000000, field: 0x00FFFFFF, chrome: 0x00F0F0F0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_resolves_against_the_system_setting() {
        assert!(ThemeChoice::Dark.is_dark(false));
        assert!(!ThemeChoice::Light.is_dark(true));
        assert!(ThemeChoice::System.is_dark(true));
        assert!(!ThemeChoice::System.is_dark(false));
    }

    #[test]
    fn choice_parses_and_prints() {
        for c in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            assert_eq!(ThemeChoice::parse(c.name()), Some(c));
        }
        assert_eq!(ThemeChoice::parse("Dark"), Some(ThemeChoice::Dark));
        assert_eq!(ThemeChoice::parse("purple"), None);
    }

    #[test]
    fn dark_palette_is_dark_with_readable_text() {
        let luma = |c: u32| ((c & 0xFF) * 299 + ((c >> 8) & 0xFF) * 587 + ((c >> 16) & 0xFF) * 114) / 1000;
        let d = Palette::new(true);
        assert!(luma(d.window) < 60 && luma(d.text) > 180);
        let l = Palette::new(false);
        assert!(luma(l.window) > 200 && luma(l.text) < 60);
    }

    #[test]
    fn system_setting_can_be_read() {
        // Whatever the machine is set to, reading it must not fail or panic.
        let _ = system_prefers_dark();
    }
}
