//! Explorer-style ordering: folders first, natural ("file2" < "file10"),
//! case-insensitive names. Produces indices into a Snapshot; never moves entries.

use std::collections::HashMap;

use windows::Win32::Foundation::LPARAM;
use windows::Win32::Globalization::{LCMAP_SORTKEY, LCMapStringEx, NORM_IGNORECASE, SORT_DIGITSASNUMBERS};
use windows::core::PCWSTR;

use crate::listing::Snapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    /// By extension, which stands in for Explorer's type names without asking the shell.
    Type,
    Created,
    Attributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

/// Appends the locale sort key of `name` to `out`. Byte order of keys equals
/// CompareStringEx order, so sorting costs one NLS call per name instead of
/// one per comparison.
fn push_sort_key(name: &[u16], buf: &mut Vec<u16>, out: &mut Vec<u8>) {
    let flags = LCMAP_SORTKEY | NORM_IGNORECASE.0 | SORT_DIGITSASNUMBERS.0;
    let map = |dest: Option<&mut [u16]>| unsafe {
        LCMapStringEx(PCWSTR::null(), flags, name, dest, None, None, LPARAM(0))
    };
    // With LCMAP_SORTKEY the destination length is in bytes; a u16 buffer of
    // N elements holds 2N bytes, so passing N is always safe.
    let mut written = map(Some(buf)).max(0) as usize;
    if written == 0 {
        let needed = map(None).max(0) as usize;
        buf.resize(needed.max(buf.len()), 0);
        written = map(Some(buf)).max(0) as usize;
    }
    let raw = unsafe { std::slice::from_raw_parts(buf.as_ptr() as *const u8, written.min(buf.len() * 2)) };
    out.extend_from_slice(raw);
}

/// Extension after the last dot, ASCII-lowercased; empty if none.
fn lower_ext(name: &[u16]) -> impl Iterator<Item = u16> + '_ {
    let start = name.iter().rposition(|&c| c == b'.' as u16).map_or(name.len(), |i| i + 1);
    name[start..].iter().map(|&c| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c })
}

/// Lowercase form used for filtering ("Ä" matches "ä").
pub fn fold(text: &[u16]) -> Vec<u16> {
    if text.iter().all(|&c| c < 128) {
        return text
            .iter()
            .map(|&c| if (b'A' as u16..=b'Z' as u16).contains(&c) { c + 32 } else { c })
            .collect();
    }
    String::from_utf16_lossy(text).to_lowercase().encode_utf16().collect()
}

/// The entries of `order` that match the filter text (see `Query`); order kept.
pub fn filter_order(s: &Snapshot, order: &[u32], needle: &[u16]) -> Vec<u32> {
    if needle.is_empty() {
        return order.to_vec();
    }
    let q = Query::parse(&String::from_utf16_lossy(needle), now());
    order.iter().copied().filter(|&i| q.matches(s, i as usize)).collect()
}

/// Filter or search text: words to find in names (ignoring case), plus conditions:
/// `size:>10MB`, `size:<1KB`, `modified:<7d` (within 7 days), `modified:>12h` (older),
/// and `content:text`, which only the subfolder search reads files for.
#[derive(Debug, Default, PartialEq)]
pub struct Query {
    pub name: Vec<u16>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    /// FILETIME bounds on the modified time.
    after: Option<u64>,
    before: Option<u64>,
    /// Lowercase UTF-8.
    pub content: Option<Vec<u8>>,
}

/// The current time as a FILETIME number.
pub fn now() -> u64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
}

impl Query {
    pub fn parse(text: &str, now: u64) -> Query {
        let mut q = Query::default();
        let mut words = Vec::new();
        for word in text.split_whitespace() {
            let lower = word.to_lowercase();
            let condition = |key: &str| {
                let rest = lower.strip_prefix(key)?;
                let op = rest.chars().next().filter(|c| matches!(c, '<' | '>'))?;
                Some((op, &rest[1..]))
            };
            if let Some((op, n)) = condition("size:")
                && let Some(bytes) = parse_size(n)
            {
                if op == '>' {
                    q.min_size = Some(bytes + 1)
                } else {
                    q.max_size = Some(bytes.saturating_sub(1))
                }
            } else if let Some((op, n)) = condition("modified:")
                && let Some(age) = parse_age(n)
            {
                // modified:<7d is "less than 7 days old": modified after now - 7 days.
                let at = now.saturating_sub(age);
                if op == '<' { q.after = Some(at) } else { q.before = Some(at) }
            } else if let Some(text) = lower.strip_prefix("content:").filter(|t| !t.is_empty()) {
                q.content = Some(text.as_bytes().to_vec());
            } else {
                words.push(word);
            }
        }
        q.name = fold(&words.join(" ").encode_utf16().collect::<Vec<_>>());
        q
    }

    /// Whether entry `i` passes the name and size/date conditions. Folders have no size,
    /// so a size condition leaves them out, as in Explorer.
    pub fn matches(&self, s: &Snapshot, i: usize) -> bool {
        let e = &s.entries[i];
        let sized = self.min_size.is_some() || self.max_size.is_some();
        (self.name.is_empty() || fold(s.name(i)).windows(self.name.len()).any(|w| w == self.name.as_slice()))
            && !(sized && s.is_dir(i))
            && self.min_size.is_none_or(|m| e.size >= m)
            && self.max_size.is_none_or(|m| e.size <= m)
            && self.after.is_none_or(|t| e.modified >= t)
            && self.before.is_none_or(|t| e.modified < t)
    }
}

/// "10mb", "1.5gb", "300" (bytes); 1024-based like Explorer.
fn parse_size(text: &str) -> Option<u64> {
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(text.len());
    let (n, unit) = text.split_at(split);
    let scale = match unit {
        "" | "b" => 1u64,
        "kb" | "k" => 1 << 10,
        "mb" | "m" => 1 << 20,
        "gb" | "g" => 1 << 30,
        _ => return None,
    };
    Some((n.parse::<f64>().ok()? * scale as f64) as u64)
}

/// "7d", "12h" as FILETIME ticks (100 ns).
fn parse_age(text: &str) -> Option<u64> {
    let (n, unit) = text.split_at(text.len().checked_sub(1)?);
    let hours = match unit {
        "h" => 1,
        "d" => 24,
        "w" => 24 * 7,
        _ => return None,
    };
    Some(n.parse::<u64>().ok()? * hours * 3600 * 10_000_000)
}

/// Visible entries in display order. Ties on size/date fall back to name, ascending.
/// Folders sort by their size in `dir_sizes` (from Calculate folder sizes), else as empty.
pub fn sorted_order(
    s: &Snapshot,
    sort: Sort,
    show_hidden: bool,
    dir_sizes: &HashMap<Vec<u16>, u64>,
) -> Vec<u32> {
    let mut v: Vec<u32> = (0..s.len() as u32).filter(|&i| show_hidden || !s.is_hidden(i as usize)).collect();

    let mut keys = Vec::new();
    let mut buf = vec![0u16; 1024];
    let mut spans = vec![(0u32, 0u32); s.len()];
    for &i in &v {
        let start = keys.len();
        push_sort_key(s.name(i as usize), &mut buf, &mut keys);
        spans[i as usize] = (start as u32, (keys.len() - start) as u32);
    }
    let key = |i: usize| &keys[spans[i].0 as usize..(spans[i].0 + spans[i].1) as usize];
    let size = |i: usize| match s.is_dir(i) {
        true => dir_sizes.get(s.name(i)).copied().unwrap_or(0),
        false => s.entries[i].size,
    };

    v.sort_by(|&a, &b| {
        let (a, b) = (a as usize, b as usize);
        let primary = match sort.key {
            SortKey::Name => key(a).cmp(key(b)),
            SortKey::Size => size(a).cmp(&size(b)),
            SortKey::Modified => s.entries[a].modified.cmp(&s.entries[b].modified),
            SortKey::Type => lower_ext(s.name(a)).cmp(lower_ext(s.name(b))),
            SortKey::Created => s.entries[a].created.cmp(&s.entries[b].created),
            SortKey::Attributes => crate::format::attributes(s.entries[a].attrs)
                .cmp(&crate::format::attributes(s.entries[b].attrs)),
        };
        let primary = if sort.descending { primary.reverse() } else { primary };
        s.is_dir(b).cmp(&s.is_dir(a)).then(primary).then_with(|| key(a).cmp(key(b)))
    });
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::listing::Snapshot;
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_HIDDEN};

    const DIR: u32 = FILE_ATTRIBUTE_DIRECTORY.0;
    const HIDDEN: u32 = FILE_ATTRIBUTE_HIDDEN.0;

    fn snap(items: &[(&str, u32, u64, u64)]) -> Snapshot {
        let mut s = Snapshot::new("C:\\".into());
        for &(name, attrs, size, modified) in items {
            s.add(&name.encode_utf16().collect::<Vec<_>>(), attrs, size, modified);
        }
        s
    }

    fn sorted(s: &Snapshot, key: SortKey, descending: bool, show_hidden: bool) -> Vec<String> {
        sorted_order(s, Sort { key, descending }, show_hidden, &HashMap::new())
            .into_iter()
            .map(|i| s.name_string(i as usize))
            .collect()
    }

    #[test]
    fn names_sort_naturally_and_ignore_case() {
        let s = snap(&[("file10", 0, 0, 0), ("file2", 0, 0, 0), ("File1", 0, 0, 0), ("b", 0, 0, 0)]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), ["b", "File1", "file2", "file10"]);
    }

    #[test]
    fn numbers_inside_names_compare_numerically() {
        let s = snap(&[("v1.10", 0, 0, 0), ("v1.9", 0, 0, 0), ("x 100 y", 0, 0, 0), ("x 20 y", 0, 0, 0)]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), ["v1.9", "v1.10", "x 20 y", "x 100 y"]);
    }

    #[test]
    fn sort_key_grows_a_too_small_buffer() {
        let name: Vec<u16> = "file10".encode_utf16().collect();
        let (mut big, mut small) = (Vec::new(), Vec::new());
        push_sort_key(&name, &mut vec![0; 1024], &mut big);
        push_sort_key(&name, &mut vec![0; 1], &mut small);
        assert!(!big.is_empty());
        assert_eq!(big, small);
    }

    #[test]
    fn max_length_names_compare_on_their_last_char() {
        let base: String = "Ünïcødé 名前 ".repeat(20).chars().take(200).collect();
        let (a, b) = (format!("{base}{}b", "x".repeat(54)), format!("{base}{}a", "x".repeat(54)));
        let s = snap(&[(&a, 0, 0, 0), (&b, 0, 0, 0)]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), [b, a]);
    }

    #[test]
    fn type_sorts_by_extension_then_name() {
        let s = snap(&[
            ("b.TXT", 0, 0, 0),
            ("a.zip", 0, 0, 0),
            ("c.txt", 0, 0, 0),
            ("noext", 0, 0, 0),
            ("d", DIR, 0, 0),
        ]);
        assert_eq!(sorted(&s, SortKey::Type, false, true), ["d", "noext", "b.TXT", "c.txt", "a.zip"]);
        assert_eq!(sorted(&s, SortKey::Type, true, true), ["d", "a.zip", "b.TXT", "c.txt", "noext"]);
    }

    #[test]
    fn filter_keeps_matching_names_in_order_ignoring_case() {
        let s =
            snap(&[("Report.txt", 0, 0, 0), ("notes", DIR, 0, 0), ("ÜBER report", 0, 0, 0), ("x", 0, 0, 0)]);
        let w = |t: &str| t.encode_utf16().collect::<Vec<_>>();
        assert_eq!(filter_order(&s, &[0, 1, 2, 3], &w("REPORT")), [0, 2]);
        assert_eq!(filter_order(&s, &[3, 2, 1, 0], &w("über")), [2]);
        assert_eq!(filter_order(&s, &[0, 1, 2, 3], &w("  ")), [0, 1, 2, 3], "blank text keeps everything");
        assert_eq!(filter_order(&s, &[3, 2, 1, 0], &w("")), [3, 2, 1, 0]);
        assert!(filter_order(&s, &[0, 1, 2, 3], &w("zzz")).is_empty());
    }

    #[test]
    fn query_conditions_filter_by_size_and_age() {
        const DAY: u64 = 24 * 3600 * 10_000_000;
        let now = 1000 * DAY;
        let s = snap(&[
            ("small.txt", 0, 100, now - DAY),
            ("big.iso", 0, 20 << 20, now - 30 * DAY),
            ("folder", DIR, 0, now - DAY),
        ]);
        let pick = |text: &str| {
            let q = Query::parse(text, now);
            (0..3).filter(|&i| q.matches(&s, i)).collect::<Vec<_>>()
        };
        assert_eq!(pick("size:>10MB"), [1]);
        assert_eq!(pick("size:<1kb"), [0], "folders have no size");
        assert_eq!(pick("modified:<7d"), [0, 2]);
        assert_eq!(pick("modified:>1w"), [1]);
        assert_eq!(pick("BIG size:>1mb"), [1]);
        assert_eq!(pick("size:huge"), Vec::<usize>::new(), "unparseable conditions are name words");
        let q = Query::parse("Report content:Total Due", now);
        assert_eq!(
            (q.name, q.content),
            (fold(&"report due".encode_utf16().collect::<Vec<_>>()), Some(b"total".to_vec()))
        );
    }

    #[test]
    fn folders_come_first_even_descending() {
        let s = snap(&[("aaa", 0, 0, 0), ("zzz", DIR, 0, 0), ("mmm", DIR, 0, 0), ("bbb", 0, 0, 0)]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), ["mmm", "zzz", "aaa", "bbb"]);
        assert_eq!(sorted(&s, SortKey::Name, true, true), ["zzz", "mmm", "bbb", "aaa"]);
    }

    #[test]
    fn size_and_date_ties_break_by_name() {
        let s = snap(&[("c", 0, 5, 3), ("a", 0, 10, 1), ("b", 0, 5, 2)]);
        assert_eq!(sorted(&s, SortKey::Size, false, true), ["b", "c", "a"]);
        assert_eq!(sorted(&s, SortKey::Size, true, true), ["a", "b", "c"]);
        assert_eq!(sorted(&s, SortKey::Modified, false, true), ["a", "b", "c"]);
    }

    #[test]
    fn folders_sort_by_calculated_size() {
        let s = snap(&[("small", DIR, 0, 0), ("big", DIR, 0, 0), ("unmeasured", DIR, 0, 0), ("f", 0, 9, 0)]);
        let sizes =
            HashMap::from([("small".encode_utf16().collect(), 5), ("big".encode_utf16().collect(), 50)]);
        let names = |descending| -> Vec<String> {
            sorted_order(&s, Sort { key: SortKey::Size, descending }, true, &sizes)
                .into_iter()
                .map(|i| s.name_string(i as usize))
                .collect()
        };
        assert_eq!(names(false), ["unmeasured", "small", "big", "f"]);
        assert_eq!(names(true), ["big", "small", "unmeasured", "f"]);
    }

    #[test]
    fn hidden_items_are_filtered_unless_shown() {
        let s = snap(&[("seen", 0, 0, 0), ("secret", HIDDEN, 0, 0), ("hdir", DIR | HIDDEN, 0, 0)]);
        assert_eq!(sorted(&s, SortKey::Name, false, false), ["seen"]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), ["hdir", "secret", "seen"]);
    }
}
