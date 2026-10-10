//! What the window shows, independent of any window. Every navigation gets a
//! generation number; results for an older generation are dropped.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

use crate::listing::Snapshot;
use crate::sort::{Sort, SortKey};

/// Work for a sort thread: re-order `snapshot` (shown as generation `generation`).
pub struct SortJob {
    pub generation: u64,
    pub snapshot: Arc<Snapshot>,
    pub sort: Sort,
}

pub struct Browser {
    generation: u64,
    shown_generation: u64,
    current: Option<Arc<Snapshot>>,
    /// Display order: indices into `current`, sorted by `order_sort`.
    pub order: Vec<u32>,
    order_sort: Sort,
    pub sort: Sort,
    pub show_hidden: bool,
    loading: Option<(PathBuf, Sort)>,
    error: Option<String>,
}

impl Default for Browser {
    fn default() -> Self {
        let sort = Sort { key: SortKey::Name, descending: false };
        Browser {
            generation: 0,
            shown_generation: 0,
            current: None,
            order: Vec::new(),
            order_sort: sort,
            sort,
            show_hidden: false,
            loading: None,
            error: None,
        }
    }
}

impl Browser {
    /// Starts a navigation; the caller lists `dir` sorted by `self.sort` and
    /// reports back with the returned generation.
    pub fn navigate(&mut self, dir: PathBuf) -> u64 {
        self.generation += 1;
        self.loading = Some((dir, self.sort));
        self.error = None;
        self.generation
    }

    /// Returns false (and changes nothing) if `generation` is stale.
    pub fn on_listed(&mut self, generation: u64, result: io::Result<(Snapshot, Vec<u32>)>) -> bool {
        if generation != self.generation {
            return false;
        }
        let Some((dir, sort)) = self.loading.take() else { return false };
        match result {
            Ok((snap, order)) => {
                self.current = Some(Arc::new(snap));
                self.order = order;
                self.order_sort = sort;
                self.shown_generation = generation;
            }
            Err(e) => self.error = Some(format!("Can't open {}: {e}", dir.display())),
        }
        true
    }

    pub fn set_sort(&mut self, sort: Sort) -> Option<SortJob> {
        self.sort = sort;
        self.sort_job()
    }

    /// A job to run if the shown order doesn't match the wanted sort.
    pub fn sort_job(&self) -> Option<SortJob> {
        let snapshot = self.current.clone()?;
        (self.order_sort != self.sort).then_some(SortJob {
            generation: self.shown_generation,
            snapshot,
            sort: self.sort,
        })
    }

    pub fn on_sorted(&mut self, generation: u64, sort: Sort, order: Vec<u32>) -> bool {
        if generation != self.shown_generation || sort != self.sort {
            return false;
        }
        self.order = order;
        self.order_sort = sort;
        true
    }

    /// Sort to apply when a column header is clicked: same column flips direction.
    pub fn sort_for_column(&self, key: SortKey) -> Sort {
        Sort { key, descending: key == self.sort.key && !self.sort.descending }
    }

    /// First row at or after `start` (wrapping) whose name starts with `prefix`, ignoring case.
    pub fn find_prefix(&self, start: usize, prefix: &[u16]) -> Option<usize> {
        let s = self.snapshot()?;
        let n = self.order.len();
        (0..n).map(|k| (start + k) % n).find(|&row| {
            let name = s.name(self.order[row] as usize);
            name.len() >= prefix.len()
                && unsafe { CompareStringOrdinal(&name[..prefix.len()], prefix, true) } == CSTR_EQUAL
        })
    }

    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.current.as_deref()
    }

    pub fn dir(&self) -> Option<&Path> {
        self.snapshot().map(|s| s.dir.as_path())
    }

    pub fn status(&self) -> String {
        if let Some((dir, _)) = &self.loading {
            return format!("Loading {}…", dir.display());
        }
        if let Some(e) = &self.error {
            return e.clone();
        }
        match self.order.len() {
            1 => "1 item".into(),
            n => format!("{n} items"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn snap(dir: &str, names: &[&str]) -> Snapshot {
        let mut s = Snapshot::new(dir.into());
        for n in names {
            s.add(&n.encode_utf16().collect::<Vec<_>>(), 0, 0, 0);
        }
        s
    }

    fn listed(dir: &str, names: &[&str]) -> io::Result<(Snapshot, Vec<u32>)> {
        let s = snap(dir, names);
        let order = (0..s.len() as u32).collect();
        Ok((s, order))
    }

    #[test]
    fn navigate_starts_loading_and_keeps_old_view() {
        let mut b = Browser::default();
        let g1 = b.navigate(r"C:\a".into());
        assert!(b.on_listed(g1, listed(r"C:\a", &["x"])));
        let g2 = b.navigate(r"C:\b".into());
        assert!(g2 > g1);
        assert_eq!(b.dir(), Some(Path::new(r"C:\a")));
        assert_eq!(b.order.len(), 1);
        assert_eq!(b.status(), r"Loading C:\b…");
    }

    #[test]
    fn stale_results_are_dropped() {
        let mut b = Browser::default();
        let g1 = b.navigate(r"C:\slow".into());
        let g2 = b.navigate(r"C:\fast".into());
        assert!(b.on_listed(g2, listed(r"C:\fast", &["f"])));
        assert!(!b.on_listed(g1, listed(r"C:\slow", &["s1", "s2"])));
        assert_eq!(b.dir(), Some(Path::new(r"C:\fast")));
        assert_eq!(b.status(), "1 item");
    }

    #[test]
    fn error_keeps_current_folder_and_reports() {
        let mut b = Browser::default();
        let g1 = b.navigate(r"C:\a".into());
        b.on_listed(g1, listed(r"C:\a", &["x", "y"]));
        let g2 = b.navigate(r"C:\locked".into());
        assert!(b.on_listed(g2, Err(io::Error::from(io::ErrorKind::PermissionDenied))));
        assert_eq!(b.dir(), Some(Path::new(r"C:\a")));
        assert_eq!(b.order.len(), 2);
        assert!(b.status().starts_with(r"Can't open C:\locked: "), "{}", b.status());
    }

    #[test]
    fn sort_result_applies_only_to_current_folder_and_sort() {
        let mut b = Browser::default();
        let g = b.navigate(r"C:\a".into());
        b.on_listed(g, listed(r"C:\a", &["x", "y"]));
        let by_size = Sort { key: SortKey::Size, descending: false };
        let req = b.set_sort(by_size).unwrap();
        assert_eq!(req.generation, g);
        assert!(!b.on_sorted(g, Sort { key: SortKey::Modified, descending: false }, vec![1, 0]));
        assert!(!b.on_sorted(g + 1, by_size, vec![1, 0]));
        assert!(b.on_sorted(g, by_size, vec![1, 0]));
        assert_eq!(b.order, [1, 0]);
    }

    #[test]
    fn sort_changed_while_loading_requests_a_resort() {
        let mut b = Browser::default();
        let g = b.navigate(r"C:".into());
        let by_size = Sort { key: SortKey::Size, descending: false };
        assert!(b.set_sort(by_size).is_none()); // nothing shown yet
        b.on_listed(g, listed(r"C:", &["x"])); // listed with the old (name) sort
        let job = b.sort_job().expect("resort needed");
        assert_eq!((job.generation, job.sort), (g, by_size));
        assert!(b.on_sorted(g, by_size, vec![0]));
        assert!(b.sort_job().is_none());
    }

    #[test]
    fn column_click_toggles_direction_on_same_key() {
        let b = Browser::default();
        assert_eq!(b.sort, Sort { key: SortKey::Name, descending: false });
        assert_eq!(b.sort_for_column(SortKey::Name), Sort { key: SortKey::Name, descending: true });
        assert_eq!(b.sort_for_column(SortKey::Size), Sort { key: SortKey::Size, descending: false });
    }

    #[test]
    fn type_to_find_matches_prefix_ignoring_case_and_wraps() {
        let mut b = Browser::default();
        let g = b.navigate(r"C:".into());
        b.on_listed(g, listed(r"C:", &["apple", "Banana", "blue", "cherry"]));
        let w = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert_eq!(b.find_prefix(0, &w("b")), Some(1));
        assert_eq!(b.find_prefix(2, &w("b")), Some(2));
        assert_eq!(b.find_prefix(3, &w("b")), Some(1));
        assert_eq!(b.find_prefix(0, &w("BL")), Some(2));
        assert_eq!(b.find_prefix(0, &w("z")), None);
    }

    #[test]
    fn status_counts_items() {
        let mut b = Browser::default();
        let g = b.navigate(r"C:\a".into());
        b.on_listed(g, listed(r"C:\a", &["x", "y", "z"]));
        assert_eq!(b.status(), "3 items");
    }
}
