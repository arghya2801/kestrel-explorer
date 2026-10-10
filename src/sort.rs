//! Explorer-style ordering: folders first, natural ("file2" < "file10"),
//! case-insensitive names. Produces indices into a Snapshot; never moves entries.

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

/// The entries of `order` whose names contain `needle`, ignoring case; order kept.
pub fn filter_order(s: &Snapshot, order: &[u32], needle: &[u16]) -> Vec<u32> {
    if needle.is_empty() {
        return order.to_vec();
    }
    let needle = fold(needle);
    order
        .iter()
        .copied()
        .filter(|&i| fold(s.name(i as usize)).windows(needle.len()).any(|w| w == needle.as_slice()))
        .collect()
}

/// Visible entries in display order. Ties on size/date fall back to name, ascending.
pub fn sorted_order(s: &Snapshot, sort: Sort, show_hidden: bool) -> Vec<u32> {
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

    v.sort_by(|&a, &b| {
        let (a, b) = (a as usize, b as usize);
        let primary = match sort.key {
            SortKey::Name => key(a).cmp(key(b)),
            SortKey::Size => s.entries[a].size.cmp(&s.entries[b].size),
            SortKey::Modified => s.entries[a].modified.cmp(&s.entries[b].modified),
            SortKey::Type => lower_ext(s.name(a)).cmp(lower_ext(s.name(b))),
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
        sorted_order(s, Sort { key, descending }, show_hidden)
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
        assert_eq!(filter_order(&s, &[3, 2, 1, 0], &w("")), [3, 2, 1, 0]);
        assert!(filter_order(&s, &[0, 1, 2, 3], &w("zzz")).is_empty());
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
    fn hidden_items_are_filtered_unless_shown() {
        let s = snap(&[("seen", 0, 0, 0), ("secret", HIDDEN, 0, 0), ("hdir", DIR | HIDDEN, 0, 0)]);
        assert_eq!(sorted(&s, SortKey::Name, false, false), ["seen"]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), ["hdir", "secret", "seen"]);
    }
}
