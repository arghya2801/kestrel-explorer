//! Custom key bindings from `keys.txt` in the data folder, one per line:
//! `Ctrl+Shift+E = show-in-explorer`. Lines starting with `#` are comments.
//! Read once per process; restart Kestrel after editing.

use std::sync::OnceLock;

/// A key press: virtual-key code plus Ctrl, Shift and Alt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A binding: the chord, the command id it runs, and both as written (for the guide).
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    pub chord: Chord,
    pub command: usize,
    pub text: String,
}

/// "Ctrl+Shift+F7", "Alt+Left", "Ctrl+K". Key names: letters, digits, F1-F24, Left, Right, Up,
/// Down, Home, End, PageUp, PageDown, Insert, Delete, Backspace, Enter, Space, Tab, Esc.
pub fn parse_chord(text: &str) -> Option<Chord> {
    let mut chord = Chord { vk: 0, ctrl: false, shift: false, alt: false };
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" => chord.alt = true,
            key if chord.vk == 0 => chord.vk = key_code(key)?,
            _ => return None,
        }
    }
    (chord.vk != 0).then_some(chord)
}

fn key_code(key: &str) -> Option<u16> {
    let named = [
        ("left", 0x25),
        ("up", 0x26),
        ("right", 0x27),
        ("down", 0x28),
        ("home", 0x24),
        ("end", 0x23),
        ("pageup", 0x21),
        ("pagedown", 0x22),
        ("insert", 0x2D),
        ("delete", 0x2E),
        ("backspace", 0x08),
        ("enter", 0x0D),
        ("space", 0x20),
        ("tab", 0x09),
        ("esc", 0x1B),
    ];
    if let Some(&(_, vk)) = named.iter().find(|(name, _)| *name == key) {
        return Some(vk);
    }
    if let [c] = key.as_bytes()
        && c.is_ascii_alphanumeric()
    {
        return Some(c.to_ascii_uppercase() as u16);
    }
    let n: u16 = key.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&n).then_some(0x70 + n - 1)
}

/// Bindings in `text`, given the command names. Bad lines are skipped and reported.
pub fn parse(text: &str, commands: &[(&str, usize)]) -> (Vec<Binding>, Vec<String>) {
    let (mut bindings, mut problems) = (Vec::new(), Vec::new());
    for (n, line) in text.lines().enumerate().map(|(n, l)| (n + 1, l.trim())) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keys, name)) = line.split_once('=') else {
            problems.push(format!("keys.txt line {n}: expected keys = command"));
            continue;
        };
        let (keys, name) = (keys.trim(), name.trim());
        match (parse_chord(keys), commands.iter().find(|(c, _)| c.eq_ignore_ascii_case(name))) {
            (Some(chord), Some(&(_, command))) => {
                bindings.push(Binding { chord, command, text: format!("{keys}    {name}") })
            }
            (None, _) => problems.push(format!("keys.txt line {n}: unknown keys \"{keys}\"")),
            (_, None) => problems.push(format!("keys.txt line {n}: unknown command \"{name}\"")),
        }
    }
    (bindings, problems)
}

/// This process's bindings from keys.txt, read on first use.
pub fn bindings(commands: &[(&str, usize)]) -> &'static [Binding] {
    static LOADED: OnceLock<Vec<Binding>> = OnceLock::new();
    LOADED.get_or_init(|| {
        let text = std::fs::read_to_string(crate::crash::data_dir().join("keys.txt")).unwrap_or_default();
        let (bindings, problems) = parse(&text, commands);
        for p in problems {
            crate::crash::log(&p);
        }
        bindings
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_and_lines_parse() {
        let c = |vk, ctrl, shift, alt| Some(Chord { vk, ctrl, shift, alt });
        assert_eq!(parse_chord("Ctrl+Shift+E"), c(b'E' as u16, true, true, false));
        assert_eq!(parse_chord("alt + left"), c(0x25, false, false, true));
        assert_eq!(parse_chord("F12"), c(0x7B, false, false, false));
        assert_eq!(parse_chord("Ctrl+7"), c(b'7' as u16, true, false, false));
        for bad in ["", "Ctrl", "Ctrl+A+B", "Hyper+A", "F25"] {
            assert_eq!(parse_chord(bad), None, "{bad}");
        }
        let text = "# comment\n\nCtrl+E = Show-In-Explorer\nCtrl+Q = nope\nwhat\n";
        let (bindings, problems) = parse(text, &[("show-in-explorer", 109)]);
        assert_eq!(bindings.len(), 1);
        assert_eq!((bindings[0].command, bindings[0].chord.vk), (109, b'E' as u16));
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("line 4") && problems[1].contains("line 5"));
    }
}
