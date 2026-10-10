//! What the window shows, independent of any window. Every navigation gets a
//! generation number; results for an older generation are dropped. History
//! only changes when a navigation succeeds.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

use crate::listing::Snapshot;
use crate::sort::{Sort, SortKey};

/// Work for a listing thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ListJob {
    pub generation: u64,
    pub dir: PathBuf,
    pub sort: Sort,
    pub show_hidden: bool,
}

/// Work for a sort thread: re-order `snapshot` (shown as `generation`).
pub struct SortJob {
    pub generation: u64,
    pub snapshot: Arc<Snapshot>,
    pub sort: Sort,
    pub show_hidden: bool,
}

/// Result of feeding a worker result to the browser.
#[derive(Debug, PartialEq)]
pub enum Applied {
    /// Old generation; nothing changed.
    Stale,
    /// Listing failed; the current folder stays, the status shows the error.
    Failed,
    /// New contents are in `order`. Select these rows; `new_folder` means reset scroll.
    Shown { select: Vec<usize>, new_folder: bool },
    /// The open folder no longer exists; list this parent instead.
    Gone(ListJob),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Nav {
    Go,
    Back,
    Forward,
    Refresh,
    /// Refresh because the folder changed on disk: no "Loading" status.
    Changed,
    /// Climbing out of a folder that was deleted while open.
    Escape,
}

pub struct Browser {
    generation: u64,
    shown_generation: u64,
    current: Option<Arc<Snapshot>>,
    /// Display order: indices into `current`, matching `order_spec`.
    pub order: Vec<u32>,
    order_spec: (Sort, bool),
    pub sort: Sort,
    pub show_hidden: bool,
    loading: Option<(PathBuf, Nav, Sort, bool)>,
    error: Option<String>,
    back: Vec<PathBuf>,
    forward: Vec<PathBuf>,
    /// The folder changed again while a load was running.
    dirty: bool,
    /// Name to select once it appears in this folder (a new or renamed item).
    select_next: Option<Vec<u16>>,
}

impl Default for Browser {
    fn default() -> Self {
        let sort = Sort { key: SortKey::Name, descending: false };
        Browser {
            generation: 0,
            shown_generation: 0,
            current: None,
            order: Vec::new(),
            order_spec: (sort, false),
            sort,
            show_hidden: false,
            loading: None,
            error: None,
            back: Vec::new(),
            forward: Vec::new(),
            dirty: false,
            select_next: None,
        }
    }
}

impl Browser {
    fn start(&mut self, dir: PathBuf, nav: Nav) -> ListJob {
        self.generation += 1;
        self.loading = Some((dir.clone(), nav, self.sort, self.show_hidden));
        self.error = None;
        self.dirty = false;
        if self.dir() != Some(dir.as_path()) {
            self.select_next = None;
        }
        ListJob { generation: self.generation, dir, sort: self.sort, show_hidden: self.show_hidden }
    }

    pub fn go(&mut self, dir: PathBuf) -> ListJob {
        let nav = if self.dir() == Some(dir.as_path()) { Nav::Refresh } else { Nav::Go };
        self.start(dir, nav)
    }

    // ponytail: pressing Back twice while the first is still loading goes back one step, not two.
    pub fn back(&mut self) -> Option<ListJob> {
        let dir = self.back.last()?.clone();
        Some(self.start(dir, Nav::Back))
    }

    pub fn forward(&mut self) -> Option<ListJob> {
        let dir = self.forward.last()?.clone();
        Some(self.start(dir, Nav::Forward))
    }

    pub fn up(&mut self) -> Option<ListJob> {
        let parent = self.dir()?.parent()?.to_path_buf();
        Some(self.go(parent))
    }

    pub fn refresh(&mut self) -> Option<ListJob> {
        let dir = self.dir()?.to_path_buf();
        Some(self.start(dir, Nav::Refresh))
    }

    /// The open folder changed on disk. Refreshes now, or once the running load
    /// finishes, so constant changes can't keep restarting a slow listing.
    pub fn on_dir_changed(&mut self) -> Option<ListJob> {
        if self.loading.is_some() {
            self.dirty = true;
            return None;
        }
        let dir = self.dir()?.to_path_buf();
        Some(self.start(dir, Nav::Changed))
    }

    /// Selects `name` after the refresh that first shows it.
    pub fn select_after_refresh(&mut self, name: &str) {
        self.select_next = Some(name.encode_utf16().collect());
    }

    /// The refresh owed for changes seen during the last load, if any.
    pub fn follow_up(&mut self) -> Option<ListJob> {
        if !std::mem::take(&mut self.dirty) || self.loading.is_some() {
            return None;
        }
        let dir = self.dir()?.to_path_buf();
        Some(self.start(dir, Nav::Changed))
    }

    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }
    pub fn can_up(&self) -> bool {
        self.dir().and_then(Path::parent).is_some()
    }
    /// Generation of the folder on screen.
    pub fn shown_generation(&self) -> u64 {
        self.shown_generation
    }
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Abandons the pending navigation. Returns false if nothing was loading.
    pub fn cancel(&mut self) -> bool {
        let Some((dir, ..)) = self.loading.take() else { return false };
        self.generation += 1;
        self.error = Some(format!("Stopped loading {}", dir.display()));
        true
    }

    /// `selected` = rows selected in the list right now, used to keep the selection.
    pub fn on_listed(
        &mut self,
        generation: u64,
        result: io::Result<(Snapshot, Vec<u32>)>,
        selected: &[usize],
    ) -> Applied {
        if generation != self.generation {
            return Applied::Stale;
        }
        let Some((dir, nav, sort, hidden)) = self.loading.take() else { return Applied::Stale };
        let (snap, order) = match result {
            Ok(ok) => ok,
            Err(_) if matches!(nav, Nav::Changed | Nav::Escape) && dir.parent().is_some() => {
                let parent = dir.parent().unwrap().to_path_buf();
                return Applied::Gone(self.start(parent, Nav::Escape));
            }
            Err(e) => {
                self.error = Some(format!("Can't open {}: {e}", dir.display()));
                return Applied::Failed;
            }
        };
        let old = self.current.take();
        let old_dir = old.as_ref().map(|s| s.dir.clone());
        let same_dir = old_dir.as_deref() == Some(snap.dir.as_path());
        let keep: Vec<Vec<u16>> = match &old {
            Some(o) if same_dir => self.entries_at(selected).map(|i| o.name(i as usize).to_vec()).collect(),
            // Coming out of a folder (Up, or Back to its parent): select the folder we left.
            Some(o) if o.dir.parent() == Some(snap.dir.as_path()) => {
                o.dir.file_name().map(|n| n.to_string_lossy().encode_utf16().collect()).into_iter().collect()
            }
            _ => Vec::new(),
        };

        if let Some(prev) = old_dir.filter(|_| !same_dir) {
            match nav {
                Nav::Go | Nav::Escape => {
                    self.back.push(prev);
                    self.forward.clear();
                }
                Nav::Back => {
                    self.back.pop();
                    self.forward.push(prev);
                }
                Nav::Forward => {
                    self.forward.pop();
                    self.back.push(prev);
                }
                Nav::Refresh | Nav::Changed => {}
            }
        }
        self.current = Some(Arc::new(snap));
        self.order = order;
        self.order_spec = (sort, hidden);
        self.shown_generation = generation;
        let mut select: Vec<usize> = keep.iter().filter_map(|n| self.row_named(n)).collect();
        if let Some(row) = self.select_next.as_deref().and_then(|n| self.row_named(n)) {
            select = vec![row];
            self.select_next = None;
        }
        Applied::Shown { select, new_folder: !same_dir }
    }

    pub fn set_sort(&mut self, sort: Sort) -> Option<SortJob> {
        self.sort = sort;
        self.sort_job()
    }

    pub fn toggle_hidden(&mut self) -> Option<SortJob> {
        self.show_hidden = !self.show_hidden;
        self.sort_job()
    }

    /// A job to run if the shown order doesn't match the wanted sort and filter.
    pub fn sort_job(&self) -> Option<SortJob> {
        let snapshot = self.current.clone()?;
        (self.order_spec != (self.sort, self.show_hidden)).then_some(SortJob {
            generation: self.shown_generation,
            snapshot,
            sort: self.sort,
            show_hidden: self.show_hidden,
        })
    }

    pub fn on_sorted(
        &mut self,
        generation: u64,
        sort: Sort,
        show_hidden: bool,
        order: Vec<u32>,
        selected: &[usize],
    ) -> Applied {
        if generation != self.shown_generation || (sort, show_hidden) != (self.sort, self.show_hidden) {
            return Applied::Stale;
        }
        let keep: Vec<u32> = self.entries_at(selected).collect();
        self.order = order;
        self.order_spec = (sort, show_hidden);
        let mut row_of = vec![usize::MAX; self.snapshot().map_or(0, Snapshot::len)];
        for (row, &e) in self.order.iter().enumerate() {
            row_of[e as usize] = row;
        }
        let select = keep.iter().map(|&e| row_of[e as usize]).filter(|&r| r != usize::MAX).collect();
        Applied::Shown { select, new_folder: false }
    }

    fn entries_at<'a>(&'a self, rows: &'a [usize]) -> impl Iterator<Item = u32> + 'a {
        rows.iter().filter_map(|&r| self.order.get(r).copied())
    }

    /// Sort to apply when a column header is clicked: same column flips direction.
    pub fn sort_for_column(&self, key: SortKey) -> Sort {
        Sort { key, descending: key == self.sort.key && !self.sort.descending }
    }

    /// Row whose name equals `name`, ignoring case.
    pub fn row_named(&self, name: &[u16]) -> Option<usize> {
        let s = self.snapshot()?;
        let same = |a: &[u16]| unsafe { CompareStringOrdinal(a, name, true) } == CSTR_EQUAL;
        self.order.iter().position(|&i| same(s.name(i as usize)))
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

    /// Absolute path of the entry shown at `row`.
    pub fn path_at(&self, row: usize) -> Option<(PathBuf, bool)> {
        let s = self.snapshot()?;
        let i = *self.order.get(row)? as usize;
        Some((s.dir.join(s.name_string(i)), s.is_dir(i)))
    }

    pub fn status(&self) -> String {
        if let Some((dir, nav, ..)) = &self.loading
            && *nav != Nav::Changed
        {
            return format!("Loading {}…  (Esc to stop)", dir.display());
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

/// Turns address-bar text into an absolute path without touching the disk:
/// trims quotes, expands `%VARS%`, resolves relative input against `current`.
pub fn parse_location(input: &str, current: Option<&Path>) -> Option<PathBuf> {
    let s = input.trim().trim_matches('"').trim();
    if s.is_empty() {
        return None;
    }
    let s = expand_env(s);
    let b = s.as_bytes();
    // "C:" alone means the drive root, not the drive's current directory.
    if b.len() == 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return Some(PathBuf::from(format!("{s}\\")));
    }
    let p = Path::new(&s);
    let p = if p.is_absolute() { p.to_path_buf() } else { current?.join(p) };
    std::path::absolute(p).ok()
}

/// Replaces `%NAME%` with the environment variable's value; unknown names stay as written.
fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        out.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(v) if !name.is_empty() => out.push_str(&v),
            _ => out.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn w(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// Navigates to `dir` and completes it with `names`.
    fn visit(b: &mut Browser, dir: &str, names: &[&str]) -> Applied {
        let job = b.go(dir.into());
        b.on_listed(job.generation, listed(dir, names), &[])
    }

    fn shown(select: &[usize], new_folder: bool) -> Applied {
        Applied::Shown { select: select.to_vec(), new_folder }
    }

    #[test]
    fn navigate_starts_loading_and_keeps_old_view() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        let job = b.go(r"C:\b".into());
        assert_eq!(job.dir, Path::new(r"C:\b"));
        assert_eq!(b.dir(), Some(Path::new(r"C:\a")));
        assert_eq!(b.order.len(), 1);
        assert!(b.status().starts_with(r"Loading C:\b…"), "{}", b.status());
    }

    #[test]
    fn stale_results_are_dropped() {
        let mut b = Browser::default();
        let slow = b.go(r"C:\slow".into());
        let fast = b.go(r"C:\fast".into());
        assert_eq!(b.on_listed(fast.generation, listed(r"C:\fast", &["f"]), &[]), shown(&[], true));
        assert_eq!(b.on_listed(slow.generation, listed(r"C:\slow", &["s1", "s2"]), &[]), Applied::Stale);
        assert_eq!(b.dir(), Some(Path::new(r"C:\fast")));
        assert_eq!(b.status(), "1 item");
    }

    #[test]
    fn error_keeps_current_folder_and_history() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x", "y"]);
        let job = b.go(r"C:\locked".into());
        let err = Err(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(b.on_listed(job.generation, err, &[]), Applied::Failed);
        assert_eq!(b.dir(), Some(Path::new(r"C:\a")));
        assert_eq!(b.order.len(), 2);
        assert!(!b.can_back());
        assert!(b.status().starts_with(r"Can't open C:\locked: "), "{}", b.status());
    }

    #[test]
    fn back_and_forward_walk_history() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &[]);
        visit(&mut b, r"C:\b", &[]);
        visit(&mut b, r"C:\c", &[]);
        assert!(b.can_back() && !b.can_forward());

        let job = b.back().unwrap();
        assert_eq!(job.dir, Path::new(r"C:\b"));
        b.on_listed(job.generation, listed(r"C:\b", &[]), &[]);
        assert!(b.can_forward());

        let job = b.forward().unwrap();
        assert_eq!(job.dir, Path::new(r"C:\c"));
        b.on_listed(job.generation, listed(r"C:\c", &[]), &[]);
        assert!(!b.can_forward());

        b.back().map(|j| b.on_listed(j.generation, listed(r"C:\b", &[]), &[]));
        visit(&mut b, r"C:\d", &[]); // a new place drops the forward list
        assert!(!b.can_forward());
        assert_eq!(b.back().unwrap().dir, Path::new(r"C:\b"));
    }

    #[test]
    fn failed_back_leaves_history_alone() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &[]);
        visit(&mut b, r"C:\b", &[]);
        let job = b.back().unwrap();
        b.on_listed(job.generation, Err(io::ErrorKind::NotFound.into()), &[]);
        assert_eq!(b.back().unwrap().dir, Path::new(r"C:\a"));
        assert!(!b.can_forward());
    }

    #[test]
    fn going_to_the_current_folder_is_a_refresh() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &[]);
        visit(&mut b, r"C:\a", &[]);
        assert!(!b.can_back());
    }

    #[test]
    fn up_selects_the_folder_we_came_from() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a\inner", &[]);
        let job = b.up().unwrap();
        assert_eq!(job.dir, Path::new(r"C:\a"));
        let r = b.on_listed(job.generation, listed(r"C:\a", &["first", "INNER", "z"]), &[]);
        assert_eq!(r, shown(&[1], true));
        assert!(b.can_back());
    }

    #[test]
    fn entering_a_folder_selects_nothing() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &[]);
        assert_eq!(visit(&mut b, r"C:\a\b", &["a"]), shown(&[], true));
    }

    #[test]
    fn refresh_keeps_selection_by_name() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x", "y", "z"]);
        let job = b.refresh().unwrap();
        let r = b.on_listed(job.generation, listed(r"C:\a", &["new", "z", "x"]), &[0, 2]);
        assert_eq!(r, shown(&[2, 1], false));
    }

    #[test]
    fn cancel_drops_the_pending_result() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        let job = b.go(r"\\dead\share".into());
        assert!(b.cancel());
        assert!(!b.is_loading());
        assert_eq!(b.on_listed(job.generation, listed(r"\\dead\share", &[]), &[]), Applied::Stale);
        assert_eq!(b.dir(), Some(Path::new(r"C:\a")));
        assert!(b.status().starts_with("Stopped loading"), "{}", b.status());
        assert!(!b.cancel());
    }

    #[test]
    fn sort_result_applies_only_to_current_folder_and_sort() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x", "y"]);
        let g = b.shown_generation;
        let by_size = Sort { key: SortKey::Size, descending: false };
        let req = b.set_sort(by_size).unwrap();
        assert_eq!(req.generation, g);
        let by_date = Sort { key: SortKey::Modified, descending: false };
        assert_eq!(b.on_sorted(g, by_date, false, vec![1, 0], &[]), Applied::Stale);
        assert_eq!(b.on_sorted(g + 1, by_size, false, vec![1, 0], &[]), Applied::Stale);
        assert_eq!(b.on_sorted(g, by_size, false, vec![1, 0], &[0]), shown(&[1], false));
        assert_eq!(b.order, [1, 0]);
    }

    #[test]
    fn sort_changed_while_loading_requests_a_resort() {
        let mut b = Browser::default();
        let job = b.go(r"C:\a".into());
        let by_size = Sort { key: SortKey::Size, descending: false };
        assert!(b.set_sort(by_size).is_none()); // nothing shown yet
        b.on_listed(job.generation, listed(r"C:\a", &["x"]), &[]); // listed with the old sort
        let resort = b.sort_job().expect("resort needed");
        assert_eq!((resort.generation, resort.sort), (job.generation, by_size));
        b.on_sorted(job.generation, by_size, false, vec![0], &[]);
        assert!(b.sort_job().is_none());
    }

    #[test]
    fn toggling_hidden_requests_a_refilter() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        let job = b.toggle_hidden().unwrap();
        assert!(job.show_hidden);
        assert_eq!(b.on_sorted(job.generation, job.sort, false, vec![], &[]), Applied::Stale);
        assert!(matches!(b.on_sorted(job.generation, job.sort, true, vec![0], &[]), Applied::Shown { .. }));
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
        visit(&mut b, r"C:\a", &["apple", "Banana", "blue", "cherry"]);
        assert_eq!(b.find_prefix(0, &w("b")), Some(1));
        assert_eq!(b.find_prefix(2, &w("b")), Some(2));
        assert_eq!(b.find_prefix(3, &w("b")), Some(1));
        assert_eq!(b.find_prefix(0, &w("BL")), Some(2));
        assert_eq!(b.find_prefix(0, &w("z")), None);
    }

    #[test]
    fn status_counts_items() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x", "y", "z"]);
        assert_eq!(b.status(), "3 items");
    }

    #[test]
    fn change_in_the_open_folder_refreshes_quietly() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x", "y"]);
        let job = b.on_dir_changed().unwrap();
        assert_eq!(job.dir, Path::new(r"C:\a"));
        assert_eq!(b.status(), "2 items", "no Loading flicker");
        assert_eq!(b.on_listed(job.generation, listed(r"C:\a", &["x", "y", "z"]), &[1]), shown(&[1], false));
        assert!(!b.can_back());
    }

    #[test]
    fn changes_during_a_load_queue_one_follow_up() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        let first = b.on_dir_changed().unwrap();
        assert!(b.on_dir_changed().is_none());
        assert!(b.on_dir_changed().is_none());
        b.on_listed(first.generation, listed(r"C:\a", &["x", "y"]), &[]);
        let again = b.follow_up().expect("one more refresh for the changes seen meanwhile");
        assert!(b.follow_up().is_none());
        b.on_listed(again.generation, listed(r"C:\a", &["x", "y", "z"]), &[]);
        assert!(b.follow_up().is_none());
    }

    #[test]
    fn navigating_away_drops_a_queued_refresh() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        b.on_dir_changed();
        b.on_dir_changed();
        let job = b.go(r"C:\b".into());
        b.on_listed(job.generation, listed(r"C:\b", &[]), &[]);
        assert!(b.follow_up().is_none());
    }

    #[test]
    fn vanished_folder_climbs_to_an_existing_parent() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a\b\c", &["x"]);
        let job = b.on_dir_changed().unwrap();
        let gone = || Err(io::Error::from(io::ErrorKind::NotFound));
        let Applied::Gone(up) = b.on_listed(job.generation, gone(), &[]) else { panic!("expected Gone") };
        assert_eq!(up.dir, Path::new(r"C:\a\b"));
        let Applied::Gone(up2) = b.on_listed(up.generation, gone(), &[]) else { panic!("expected Gone") };
        assert_eq!(up2.dir, Path::new(r"C:\a"));
        assert_eq!(b.on_listed(up2.generation, listed(r"C:\a", &["other"]), &[]), shown(&[], true));
        assert_eq!(b.dir(), Some(Path::new(r"C:\a")));
    }

    #[test]
    fn a_new_or_renamed_item_is_selected_once_it_shows_up() {
        let mut b = Browser::default();
        visit(&mut b, r"C:", &["x", "y"]);
        b.select_after_refresh("New folder");
        // A refresh that started before the folder existed doesn't use up the request.
        let early = b.on_dir_changed().unwrap();
        assert_eq!(b.on_listed(early.generation, listed(r"C:", &["x", "y"]), &[0]), shown(&[0], false));
        let job = b.on_dir_changed().unwrap();
        let r = b.on_listed(job.generation, listed(r"C:", &["New folder", "x", "y"]), &[1]);
        assert_eq!(r, shown(&[0], false), "only the new item is selected");
        let again = b.on_dir_changed().unwrap();
        assert_eq!(
            b.on_listed(again.generation, listed(r"C:", &["New folder", "x", "y"]), &[2]),
            shown(&[2], false)
        );
    }

    #[test]
    fn location_input_is_normalized() {
        let cur = Some(Path::new(r"C:\a\b"));
        let p = |s| parse_location(s, cur).map(|p| p.display().to_string());
        assert_eq!(p(r#"  "C:\Windows"  "#).as_deref(), Some(r"C:\Windows"));
        assert_eq!(p("C:/Windows/System32").as_deref(), Some(r"C:\Windows\System32"));
        assert_eq!(p("d:").as_deref(), Some(r"d:\"));
        assert_eq!(p("..").as_deref(), Some(r"C:\a"));
        assert_eq!(p(r"sub\..\x").as_deref(), Some(r"C:\a\b\x"));
        assert_eq!(p(r"\\srv\share\dir").as_deref(), Some(r"\\srv\share\dir"));
        assert_eq!(p("   "), None);
        assert_eq!(parse_location("rel", None), None);
    }

    #[test]
    fn location_expands_environment_variables() {
        let root = std::env::var("SystemRoot").unwrap();
        let p = parse_location(r"%SystemRoot%\System32", None).unwrap();
        assert_eq!(p, Path::new(&root).join("System32"));
        assert_eq!(expand_env("100% sure %NOPE_NOT_SET% x"), "100% sure %NOPE_NOT_SET% x");
    }
}
