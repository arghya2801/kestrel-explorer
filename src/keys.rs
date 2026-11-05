//! Custom key bindings from `keys.txt` in the data folder, one per line:
//! `Ctrl+Shift+E = show-in-explorer`. Lines starting with `#` are comments.
//! Read on first use; Settings > Keyboard writes the file and reloads it.

use std::sync::{Arc, RwLock};

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

/// Keys known by name, as keys.txt writes them.
const NAMED: [(&str, &str, u16); 15] = [
    ("left", "Left", 0x25),
    ("up", "Up", 0x26),
    ("right", "Right", 0x27),
    ("down", "Down", 0x28),
    ("home", "Home", 0x24),
    ("end", "End", 0x23),
    ("pageup", "PageUp", 0x21),
    ("pagedown", "PageDown", 0x22),
    ("insert", "Insert", 0x2D),
    ("delete", "Delete", 0x2E),
    ("backspace", "Backspace", 0x08),
    ("enter", "Enter", 0x0D),
    ("space", "Space", 0x20),
    ("tab", "Tab", 0x09),
    ("esc", "Esc", 0x1B),
];

/// `chord` as keys.txt writes it: "Ctrl+Shift+K". `None` for keys keys.txt can't name.
pub fn chord_text(chord: Chord) -> Option<String> {
    let key = match chord.vk {
        vk @ (0x30..=0x39 | 0x41..=0x5A) => (vk as u8 as char).to_string(),
        vk @ 0x70..=0x87 => format!("F{}", vk - 0x70 + 1),
        vk => NAMED.iter().find(|n| n.2 == vk)?.1.to_string(),
    };
    let mods = [(chord.ctrl, "Ctrl+"), (chord.shift, "Shift+"), (chord.alt, "Alt+")];
    Some(mods.iter().filter(|m| m.0).map(|m| m.1).collect::<String>() + &key)
}

fn key_code(key: &str) -> Option<u16> {
    if let Some(&(_, _, vk)) = NAMED.iter().find(|(name, _, _)| *name == key) {
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

static LOADED: RwLock<Option<Arc<Vec<Binding>>>> = RwLock::new(None);

/// This process's bindings from keys.txt, read on first use and after `save`.
pub fn bindings(commands: &[(&str, usize)]) -> Arc<Vec<Binding>> {
    if let Some(loaded) = LOADED.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return loaded;
    }
    let text = std::fs::read_to_string(crate::crash::data_dir().join("keys.txt")).unwrap_or_default();
    let (bindings, problems) = parse(&text, commands);
    for p in problems {
        crate::crash::log(&p);
    }
    let bindings = Arc::new(bindings);
    *LOADED.write().unwrap_or_else(|e| e.into_inner()) = Some(bindings.clone());
    bindings
}

/// Writes keys.txt with `lines` (keys, command name), keeping its comments, and reloads it.
pub fn save(lines: &[(String, &str)]) -> std::io::Result<()> {
    let path = crate::crash::data_dir().join("keys.txt");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let mut text: String =
        old.lines().filter(|l| l.trim_start().starts_with('#')).map(|l| format!("{l}\n")).collect();
    if text.is_empty() {
        text = "# Kestrel key bindings: keys = command. Settings > Keyboard edits this file.\n".into();
    }
    for (keys, name) in lines {
        text += &format!("{keys} = {name}\n");
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, text)?;
    *LOADED.write().unwrap_or_else(|e| e.into_inner()) = None;
    Ok(())
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

    #[test]
    fn chords_name_themselves_as_keys_txt_reads_them() {
        for text in ["Ctrl+Shift+K", "Alt+Left", "F12", "Ctrl+7", "Shift+Delete", "Ctrl+Alt+PageDown"] {
            assert_eq!(chord_text(parse_chord(text).unwrap()).as_deref(), Some(text));
        }
        assert_eq!(chord_text(Chord { vk: 0xBA, ctrl: true, shift: false, alt: false }), None);
    }
}
