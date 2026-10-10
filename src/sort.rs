//! Explorer-style ordering: folders first, natural ("file2" < "file10"),
//! case-insensitive names. Produces indices into a Snapshot; never moves entries.

use std::cmp::Ordering;

use windows::Win32::Globalization::{CompareStringEx, NORM_IGNORECASE, SORT_DIGITSASNUMBERS};
use windows::core::PCWSTR;

use crate::listing::Snapshot;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub descending: bool,
}

pub fn compare_names(a: &[u16], b: &[u16]) -> Ordering {
    let r = unsafe {
        CompareStringEx(
            PCWSTR::null(),
            NORM_IGNORECASE | SORT_DIGITSASNUMBERS,
            a,
            b,
            None,
            None,
            None,
        )
    };
    // CSTR_LESS_THAN = 1, CSTR_EQUAL = 2, CSTR_GREATER_THAN = 3, 0 = error.
    match r.0 {
        1 => Ordering::Less,
        3 => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// Visible entries in display order. Ties on size/date fall back to name, ascending.
pub fn sorted_order(s: &Snapshot, sort: Sort, show_hidden: bool) -> Vec<u32> {
    let mut v: Vec<u32> = (0..s.len() as u32)
        .filter(|&i| show_hidden || !s.is_hidden(i as usize))
        .collect();
    let by_name = |a: usize, b: usize| compare_names(s.name(a), s.name(b));
    v.sort_by(|&a, &b| {
        let (a, b) = (a as usize, b as usize);
        let primary = match sort.key {
            SortKey::Name => by_name(a, b),
            SortKey::Size => s.entries[a].size.cmp(&s.entries[b].size),
            SortKey::Modified => s.entries[a].modified.cmp(&s.entries[b].modified),
        };
        let primary = if sort.descending {
            primary.reverse()
        } else {
            primary
        };
        s.is_dir(b)
            .cmp(&s.is_dir(a))
            .then(primary)
            .then_with(|| by_name(a, b))
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
        let s = snap(&[
            ("file10", 0, 0, 0),
            ("file2", 0, 0, 0),
            ("File1", 0, 0, 0),
            ("b", 0, 0, 0),
        ]);
        assert_eq!(
            sorted(&s, SortKey::Name, false, true),
            ["b", "File1", "file2", "file10"]
        );
    }

    #[test]
    fn folders_come_first_even_descending() {
        let s = snap(&[
            ("aaa", 0, 0, 0),
            ("zzz", DIR, 0, 0),
            ("mmm", DIR, 0, 0),
            ("bbb", 0, 0, 0),
        ]);
        assert_eq!(
            sorted(&s, SortKey::Name, false, true),
            ["mmm", "zzz", "aaa", "bbb"]
        );
        assert_eq!(
            sorted(&s, SortKey::Name, true, true),
            ["zzz", "mmm", "bbb", "aaa"]
        );
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
        let s = snap(&[
            ("seen", 0, 0, 0),
            ("secret", HIDDEN, 0, 0),
            ("hdir", DIR | HIDDEN, 0, 0),
        ]);
        assert_eq!(sorted(&s, SortKey::Name, false, false), ["seen"]);
        assert_eq!(sorted(&s, SortKey::Name, false, true), ["hdir", "secret", "seen"]);
    }
}
