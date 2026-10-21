//! What the window shows, independent of any window. Every navigation gets a
//! generation number; results for an older generation are dropped. History
//! only changes when a navigation succeeds.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use windows::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

use crate::listing::Snapshot;
use crate::sort::{Sort, SortKey};

/// The sort each folder shows, shared by every view of a window: a folder keeps the sort
/// last chosen in it, and every other folder uses `default` (Settings > File list).
#[derive(Debug, Clone, PartialEq)]
pub struct FolderSorts {
    pub default: Sort,
    pub folders: Vec<(PathBuf, Sort)>,
}

impl Default for FolderSorts {
    fn default() -> Self {
        FolderSorts { default: Sort { key: SortKey::Name, descending: false }, folders: Vec::new() }
    }
}

impl FolderSorts {
    pub fn get(&self, dir: &Path) -> Sort {
        self.folders.iter().find(|(d, _)| same_dir(d, dir)).map_or(self.default, |&(_, s)| s)
    }

    /// Remembers `sort` for `dir`; a folder back on the default sort is forgotten.
    pub fn remember(&mut self, dir: &Path, sort: Sort) {
        self.folders.retain(|(d, _)| !same_dir(d, dir));
        if sort != self.default {
            self.folders.push((dir.to_path_buf(), sort));
        }
    }
}

/// Folder paths compare without case, as Windows does.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

/// A place in history: a folder, or search results below it.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    pub dir: PathBuf,
    pub query: Option<String>,
}

/// Work for a listing thread.
#[derive(Debug, Clone, PartialEq)]
pub struct ListJob {
    pub generation: u64,
    pub dir: PathBuf,
    /// Search below `dir` for this instead of listing it.
    pub query: Option<String>,
    pub sort: Sort,
    pub show_hidden: bool,
    /// Keep only names containing this (folded); empty keeps all.
    pub filter: Vec<u16>,
}

/// A navigation waiting for its listing.
struct Pending {
    at: Location,
    nav: Nav,
    sort: Sort,
    show_hidden: bool,
    filter: Vec<u16>,
}

/// Work for a sort thread: re-order `snapshot` (shown as `generation`).
pub struct SortJob {
    pub generation: u64,
    pub snapshot: Arc<Snapshot>,
    pub sort: Sort,
    pub show_hidden: bool,
    pub filter: Vec<u16>,
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
    Back(usize),
    Forward(usize),
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
    order_spec: (Sort, bool, Vec<u16>),
    pub sort: Sort,
    /// Per-folder sorts, shared with the window's other views.
    pub sorts: Rc<RefCell<FolderSorts>>,
    pub show_hidden: bool,
    loading: Option<Pending>,
    /// Folded filter text; empty shows everything.
    filter: Vec<u16>,
    /// Set while the list shows search results rather than a folder.
    shown_query: Option<String>,
    error: Option<String>,
    back: Vec<Location>,
    forward: Vec<Location>,
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
            order_spec: (sort, false, Vec::new()),
            filter: Vec::new(),
            shown_query: None,
            sort,
            sorts: Default::default(),
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
    /// A browser whose generation numbers start at `base` (see `tabs::generation_base`).
    pub fn with_generation_base(base: u64) -> Browser {
        Browser { generation: base, shown_generation: base, ..Browser::default() }
    }

    fn start(&mut self, at: Location, nav: Nav) -> ListJob {
        self.generation += 1;
        self.error = None;
        self.dirty = false;
        if self.location().as_ref() != Some(&at) {
            // Somewhere else: a filter typed for the old place would only confuse.
            self.select_next = None;
            self.filter.clear();
        }
        // Entering another folder shows it in its own sort; searches keep the sort they started with.
        if at.query.is_none() && !self.dir().is_some_and(|d| same_dir(d, &at.dir)) {
            self.sort = self.sorts.borrow().get(&at.dir);
        }
        let filter = self.filter.clone();
        let job = ListJob {
            generation: self.generation,
            dir: at.dir.clone(),
            query: at.query.clone(),
            sort: self.sort,
            show_hidden: self.show_hidden,
            filter: filter.clone(),
        };
        self.loading = Some(Pending { at, nav, sort: self.sort, show_hidden: self.show_hidden, filter });
        job
    }

    pub fn requested_location(&self) -> Option<Location> {
        self.loading.as_ref().map(|p| p.at.clone()).or_else(|| self.location())
    }

    /// Where the list is now.
    pub fn location(&self) -> Option<Location> {
        Some(Location { dir: self.dir()?.to_path_buf(), query: self.shown_query.clone() })
    }

    /// The search shown, if the list holds search results.
    pub fn query(&self) -> Option<&str> {
        self.shown_query.as_deref()
    }

    /// Searches below the current folder for names containing `query`.
    pub fn search(&mut self, query: &str) -> Option<ListJob> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        let at = Location { dir: self.dir()?.to_path_buf(), query: Some(query.to_string()) };
        let nav = if self.location().as_ref() == Some(&at) { Nav::Refresh } else { Nav::Go };
        Some(self.start(at, nav))
    }

    /// Narrows the view to names containing `text`, ignoring case.
    pub fn set_filter(&mut self, text: &str) -> Option<SortJob> {
        self.filter = crate::sort::fold(&text.encode_utf16().collect::<Vec<_>>());
        self.sort_job()
    }

    pub fn filter(&self) -> String {
        String::from_utf16_lossy(&self.filter)
    }

    pub fn go(&mut self, dir: PathBuf) -> ListJob {
        let at = Location { dir, query: None };
        let nav = if self.location().as_ref() == Some(&at) { Nav::Refresh } else { Nav::Go };
        self.start(at, nav)
    }

    pub fn back(&mut self) -> Option<ListJob> {
        self.history(-1)
    }

    pub fn forward(&mut self) -> Option<ListJob> {
        self.history(1)
    }

    fn history_offset(&self) -> isize {
        match self.loading.as_ref().map(|p| p.nav) {
            Some(Nav::Back(n)) => -(n as isize),
            Some(Nav::Forward(n)) => n as isize,
            _ => 0,
        }
    }

    fn history(&mut self, step: isize) -> Option<ListJob> {
        let offset = self.history_offset() + step;
        let (at, nav) = if offset < 0 {
            let n = offset.unsigned_abs();
            (self.back.get(self.back.len().checked_sub(n)?)?.clone(), Nav::Back(n))
        } else if offset > 0 {
            let n = offset as usize;
            (self.forward.get(self.forward.len().checked_sub(n)?)?.clone(), Nav::Forward(n))
        } else {
            (self.location()?, Nav::Refresh)
        };
        Some(self.start(at, nav))
    }

    /// Up one folder; from search results, up to the folder that was searched.
    pub fn up(&mut self) -> Option<ListJob> {
        let dir = self.dir()?;
        let target =
            if self.shown_query.is_some() { dir.to_path_buf() } else { crate::network::up_from(dir)? };
        Some(self.go(target))
    }

    pub fn refresh(&mut self) -> Option<ListJob> {
        let at = self.location()?;
        Some(self.start(at, Nav::Refresh))
    }

    /// The open folder changed on disk. Refreshes now, or once the running load
    /// finishes, so constant changes can't keep restarting a slow listing.
    /// Search results aren't searched again for every change (F5 does that).
    pub fn on_dir_changed(&mut self) -> Option<ListJob> {
        if self.shown_query.is_some() {
            return None;
        }
        if self.loading.is_some() {
            self.dirty = true;
            return None;
        }
        let dir = self.dir()?.to_path_buf();
        Some(self.start(Location { dir, query: None }, Nav::Changed))
    }

    /// Selects `name` after the refresh that first shows it.
    pub fn select_after_refresh(&mut self, name: &str) {
        self.select_next = Some(name.encode_utf16().collect());
    }

    /// The refresh owed for changes seen during the last load, if any.
    pub fn follow_up(&mut self) -> Option<ListJob> {
        if !std::mem::take(&mut self.dirty) || self.loading.is_some() || self.shown_query.is_some() {
            return None;
        }
        let dir = self.dir()?.to_path_buf();
        Some(self.start(Location { dir, query: None }, Nav::Changed))
    }

    pub fn can_back(&self) -> bool {
        self.history_offset() > -(self.back.len() as isize)
    }
    pub fn can_forward(&self) -> bool {
        self.history_offset() < self.forward.len() as isize
    }
    pub fn can_up(&self) -> bool {
        self.dir().and_then(crate::network::up_from).is_some()
    }
    /// Generation of the folder on screen.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn shown_generation(&self) -> u64 {
        self.shown_generation
    }
    pub fn is_loading(&self) -> bool {
        self.loading.is_some()
    }

    /// Abandons the pending navigation. Returns false if nothing was loading.
    pub fn cancel(&mut self) -> bool {
        let Some(p) = self.loading.take() else { return false };
        self.generation += 1;
        let what = if p.at.query.is_some() { "searching" } else { "loading" };
        self.error = Some(format!("Stopped {what} {}", p.at.dir.display()));
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
        let Some(Pending { at, nav, sort, show_hidden: hidden, filter }) = self.loading.take() else {
            return Applied::Stale;
        };
        let (snap, order) = match result {
            Ok(ok) => ok,
            Err(_) if matches!(nav, Nav::Changed | Nav::Escape) && at.dir.parent().is_some() => {
                let parent = at.dir.parent().unwrap().to_path_buf();
                return Applied::Gone(self.start(Location { dir: parent, query: None }, Nav::Escape));
            }
            Err(e) => {
                self.error = Some(format!("Can't open {}: {e}", at.dir.display()));
                return Applied::Failed;
            }
        };
        let old = self.current.take();
        let old_at = old.as_ref().map(|s| Location { dir: s.dir.clone(), query: self.shown_query.clone() });
        let same_place = old_at.as_ref() == Some(&at);
        let keep: Vec<Vec<u16>> = match &old {
            Some(o) if same_place => self.entries_at(selected).map(|i| o.name(i as usize).to_vec()).collect(),
            // Coming out of a folder (Up, or Back to its parent): select the folder we left.
            Some(o) if o.dir.parent() == Some(snap.dir.as_path()) && self.shown_query.is_none() => {
                o.dir.file_name().map(|n| n.to_string_lossy().encode_utf16().collect()).into_iter().collect()
            }
            _ => Vec::new(),
        };
        self.shown_query = at.query;
        let same_dir = same_place;

        if let Some(prev) = old_at.filter(|_| !same_dir || matches!(nav, Nav::Back(_) | Nav::Forward(_))) {
            match nav {
                Nav::Go | Nav::Escape => {
                    self.back.push(prev);
                    self.forward.clear();
                }
                Nav::Back(n) => {
                    self.forward.push(prev);
                    for _ in 1..n {
                        if let Some(at) = self.back.pop() {
                            self.forward.push(at);
                        }
                    }
                    self.back.pop();
                }
                Nav::Forward(n) => {
                    self.back.push(prev);
                    for _ in 1..n {
                        if let Some(at) = self.forward.pop() {
                            self.back.push(at);
                        }
                    }
                    self.forward.pop();
                }
                Nav::Refresh | Nav::Changed => {}
            }
        }
        self.current = Some(Arc::new(snap));
        self.order = order;
        self.order_spec = (sort, hidden, filter);
        self.shown_generation = generation;
        let mut select = self.rows_named(&keep);
        if let Some(row) = self.select_next.as_deref().and_then(|n| self.row_named(n)) {
            select = vec![row];
            self.select_next = None;
        }
        Applied::Shown { select, new_folder: !same_dir }
    }

    /// Sorts the list and remembers the sort for this folder.
    pub fn set_sort(&mut self, sort: Sort) -> Option<SortJob> {
        self.sort = sort;
        if let Some(at) = self.requested_location().filter(|at| at.query.is_none()) {
            self.sorts.borrow_mut().remember(&at.dir, sort);
        }
        self.sort_job()
    }

    pub fn toggle_hidden(&mut self) -> Option<SortJob> {
        self.show_hidden = !self.show_hidden;
        self.sort_job()
    }

    /// A job to run if the shown order doesn't match the wanted sort and filter.
    pub fn sort_job(&self) -> Option<SortJob> {
        let wanted = (self.sort, self.show_hidden, self.filter.clone());
        if self.order_spec == wanted {
            return None;
        }
        self.resort_job()
    }

    /// A job to re-order the shown entries with the current sort, even if it hasn't changed.
    pub fn resort_job(&self) -> Option<SortJob> {
        Some(SortJob {
            generation: self.shown_generation,
            snapshot: self.current.clone()?,
            sort: self.sort,
            show_hidden: self.show_hidden,
            filter: self.filter.clone(),
        })
    }

    pub fn on_sorted(
        &mut self,
        generation: u64,
        sort: Sort,
        show_hidden: bool,
        filter: &[u16],
        order: Vec<u32>,
        selected: &[usize],
    ) -> Applied {
        let wanted = (self.sort, self.show_hidden, self.filter.as_slice());
        if generation != self.shown_generation || (sort, show_hidden, filter) != wanted {
            return Applied::Stale;
        }
        let keep: Vec<u32> = self.entries_at(selected).collect();
        self.order = order;
        self.order_spec = (sort, show_hidden, filter.to_vec());
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

    fn rows_named(&self, names: &[Vec<u16>]) -> Vec<usize> {
        if names.len() <= 8 {
            return names.iter().filter_map(|n| self.row_named(n)).collect();
        }
        let Some(snapshot) = self.snapshot() else { return Vec::new() };
        let name = |row: usize| snapshot.name(self.order[row] as usize);
        let compare = |a: &[u16], b: &[u16]| unsafe { CompareStringOrdinal(a, b, true) }.0.cmp(&CSTR_EQUAL.0);
        let mut index: Vec<usize> = (0..self.order.len()).collect();
        index.sort_unstable_by(|&a, &b| compare(name(a), name(b)).then(a.cmp(&b)));
        names
            .iter()
            .filter_map(|wanted| {
                let at = index.partition_point(|&row| compare(name(row), wanted).is_lt());
                index.get(at).copied().filter(|&row| compare(name(row), wanted).is_eq())
            })
            .collect()
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

    /// `status` plus the selection's count and file size, and the drive's free space.
    pub fn status_line(&self, selected: &[usize]) -> String {
        let mut text = self.status();
        let Some(s) = self.snapshot() else { return text };
        if self.error.is_some() || self.loading.as_ref().is_some_and(|p| p.nav != Nav::Changed) {
            return text;
        }
        if !selected.is_empty() {
            let (mut files, mut bytes) = (0, 0);
            for i in self.entries_at(selected).map(|i| i as usize).filter(|&i| !s.is_dir(i)) {
                files += 1;
                bytes += s.entries[i].size;
            }
            text += &format!("    {} selected", selected.len());
            if files > 0 {
                text += &format!(", {}", crate::format::bytes(bytes));
            }
        }
        if let Some(free) = s.free {
            text += &format!("    {} free", crate::format::bytes(free));
        }
        text
    }

    pub fn status(&self) -> String {
        if let Some(p) = &self.loading
            && p.nav != Nav::Changed
        {
            return match &p.at.query {
                Some(q) => format!("Searching for \"{q}\" in {}…  (Esc to stop)", p.at.dir.display()),
                None => format!("Loading {}…  (Esc to stop)", p.at.dir.display()),
            };
        }
        if let Some(e) = &self.error {
            return e.clone();
        }
        let total = self.snapshot().map_or(0, Snapshot::len);
        match (self.order.len(), &self.shown_query) {
            (n, _) if !self.filter.is_empty() => format!("{n} of {total} items"),
            (1, Some(q)) => format!("1 result for \"{q}\""),
            (n, Some(q)) => format!("{n} results for \"{q}\""),
            (1, None) => "1 item".into(),
            (n, None) => format!("{n} items"),
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
    // A computer on the network: `\\srv` lists its shares.
    if crate::network::server_of(Path::new(&s)).is_some() {
        return Some(PathBuf::from(s.trim_end_matches('\\')));
    }
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
    fn repeated_history_navigation_keeps_every_intermediate_entry() {
        let mut b = Browser::default();
        for dir in [r"C:\a", r"C:\b", r"C:\c", r"C:\d"] {
            visit(&mut b, dir, &[]);
        }
        let old = b.back().unwrap();
        let latest = b.back().unwrap();
        assert_eq!(latest.dir, Path::new(r"C:\b"));
        assert_eq!(b.on_listed(old.generation, listed(r"C:\c", &[]), &[]), Applied::Stale);
        b.on_listed(latest.generation, listed(r"C:\b", &[]), &[]);
        assert_eq!(b.forward().unwrap().dir, Path::new(r"C:\c"));
        let latest = b.forward().unwrap();
        assert_eq!(latest.dir, Path::new(r"C:\d"));
        b.on_listed(latest.generation, listed(r"C:\d", &[]), &[]);
        assert_eq!(b.back().unwrap().dir, Path::new(r"C:\c"));
        assert_eq!(b.back().unwrap().dir, Path::new(r"C:\b"));
        assert!(b.cancel());
        assert_eq!(b.back().unwrap().dir, Path::new(r"C:\c"));
        let reverse = b.forward().unwrap();
        assert_eq!(reverse.dir, Path::new(r"C:\d"));
        b.on_listed(reverse.generation, listed(r"C:\d", &[]), &[]);
        assert!(!b.can_forward());
    }

    #[test]
    fn history_can_skip_to_an_earlier_visit_of_the_same_folder() {
        let mut b = Browser::default();
        for dir in [r"C:\a", r"C:\b", r"C:\a"] {
            visit(&mut b, dir, &[]);
        }
        b.back().unwrap();
        let back = b.back().unwrap();
        assert!(!b.can_back());
        assert!(b.back().is_none());
        b.on_listed(back.generation, listed(r"C:\a", &[]), &[]);
        assert!(!b.can_back());
        assert_eq!(b.forward().unwrap().dir, Path::new(r"C:\b"));
    }
    #[test]
    fn failed_back_leaves_history_alone() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &[]);
        visit(&mut b, r"C:\b", &[]);
        let job = b.back().unwrap();
        b.on_listed(job.generation, Err(io::ErrorKind::NotFound.into()), &[]);
        assert!(!b.can_forward());
        assert_eq!(b.back().unwrap().dir, Path::new(r"C:\a"));
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
        assert_eq!(b.on_sorted(g, by_date, false, &[], vec![1, 0], &[]), Applied::Stale);
        assert_eq!(b.on_sorted(g + 1, by_size, false, &[], vec![1, 0], &[]), Applied::Stale);
        assert_eq!(b.on_sorted(g, by_size, false, &[], vec![1, 0], &[0]), shown(&[1], false));
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
        b.on_sorted(job.generation, by_size, false, &[], vec![0], &[]);
        assert!(b.sort_job().is_none());
    }

    #[test]
    fn each_folder_keeps_its_own_sort_and_others_use_the_default() {
        let mut b = Browser::default();
        let by_date = Sort { key: SortKey::Modified, descending: true };
        visit(&mut b, r"C:\Downloads", &["x"]);
        b.set_sort(by_date);
        visit(&mut b, r"C:\other", &["y"]);
        assert_eq!(b.sort, FolderSorts::default().default, "other folders stay on the default");
        let job = b.back().unwrap();
        assert_eq!((job.sort, b.sort), (by_date, by_date), "Downloads comes back sorted by date");
        b.on_listed(job.generation, listed(r"C:\Downloads", &["x"]), &[]);
        let job = b.go(r"c:\downloads".into());
        assert_eq!(job.sort, by_date, "folder paths ignore case");
        b.on_listed(job.generation, listed(r"C:\Downloads", &["x"]), &[]);
        b.set_sort(FolderSorts::default().default);
        assert!(b.sorts.borrow().folders.is_empty(), "back on the default, nothing to remember");
    }

    #[test]
    fn toggling_hidden_requests_a_refilter() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        let job = b.toggle_hidden().unwrap();
        assert!(job.show_hidden);
        assert_eq!(b.on_sorted(job.generation, job.sort, false, &[], vec![], &[]), Applied::Stale);
        assert!(matches!(
            b.on_sorted(job.generation, job.sort, true, &[], vec![0], &[]),
            Applied::Shown { .. }
        ));
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
    fn status_adds_selection_size_and_free_space() {
        let mut b = Browser::default();
        let job = b.go(r"C:".into());
        let mut s = Snapshot::new(r"C:".into());
        s.add(&w("dir"), windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY.0, 0, 0);
        s.add(&w("f"), 0, 2048, 0);
        s.free = Some(1024);
        b.on_listed(job.generation, Ok((s, vec![0, 1])), &[]);
        assert_eq!(b.status_line(&[]), format!("2 items    {} free", crate::format::bytes(1024)));
        assert_eq!(
            b.status_line(&[0]),
            format!("2 items    1 selected    {} free", crate::format::bytes(1024))
        );
        assert!(b.status_line(&[0, 1]).contains(&format!("2 selected, {}", crate::format::bytes(2048))));
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
        visit(&mut b, r"C:\a", &["x", "y"]);
        b.select_after_refresh("New folder");
        // A refresh that started before the folder existed doesn't use up the request.
        let early = b.on_dir_changed().unwrap();
        assert_eq!(b.on_listed(early.generation, listed(r"C:\a", &["x", "y"]), &[0]), shown(&[0], false));
        let job = b.on_dir_changed().unwrap();
        let r = b.on_listed(job.generation, listed(r"C:\a", &["New folder", "x", "y"]), &[1]);
        assert_eq!(r, shown(&[0], false), "only the new item is selected");
        let again = b.on_dir_changed().unwrap();
        assert_eq!(
            b.on_listed(again.generation, listed(r"C:\a", &["New folder", "x", "y"]), &[2]),
            shown(&[2], false)
        );
    }

    #[test]
    fn filter_narrows_the_view_and_resets_on_navigation() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["apple", "banana", "grape"]);
        let job = b.set_filter("AP").expect("refilter");
        assert_eq!(job.filter, w("ap"));
        assert_eq!(
            b.on_sorted(job.generation, job.sort, job.show_hidden, &[], vec![], &[]),
            Applied::Stale,
            "old filter"
        );
        let r = b.on_sorted(job.generation, job.sort, job.show_hidden, &job.filter, vec![0, 2], &[]);
        assert_eq!(r, shown(&[], false));
        assert_eq!(b.status(), "2 of 3 items");
        // A refresh keeps the filter; going elsewhere drops it.
        assert_eq!(b.refresh().unwrap().filter, w("ap"));
        let job = b.go(r"C:\b".into());
        assert!(job.filter.is_empty());
        assert_eq!(b.filter(), "");
    }

    #[test]
    fn search_results_are_a_place_you_can_go_back_from() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x", "sub"]);
        let job = b.search("rep").unwrap();
        assert_eq!((job.dir.as_path(), job.query.as_deref()), (Path::new(r"C:\a"), Some("rep")));
        assert!(b.status().starts_with(r#"Searching for "rep" in C:\a"#), "{}", b.status());
        let r = b.on_listed(job.generation, listed(r"C:\a", &[r"sub\report.txt"]), &[]);
        assert_eq!(r, shown(&[], true));
        assert_eq!(b.status(), r#"1 result for "rep""#);
        assert_eq!(b.query(), Some("rep"));
        assert!(b.on_dir_changed().is_none(), "results aren't re-searched on every change");
        assert_eq!(b.refresh().unwrap().query.as_deref(), Some("rep"), "F5 searches again");

        let back = b.back().unwrap();
        assert_eq!((back.dir.as_path(), back.query.as_deref()), (Path::new(r"C:\a"), None));
        b.on_listed(back.generation, listed(r"C:\a", &["x", "sub"]), &[]);
        assert_eq!(b.query(), None);
        assert_eq!(b.forward().unwrap().query.as_deref(), Some("rep"));
    }

    #[test]
    fn an_empty_search_does_nothing() {
        let mut b = Browser::default();
        visit(&mut b, r"C:\a", &["x"]);
        assert!(b.search("   ").is_none());
    }

    #[test]
    fn each_tab_numbers_its_navigations_from_its_own_base() {
        let mut b = Browser::with_generation_base(crate::tabs::generation_base(3));
        let job = b.go(PathBuf::from(r"C:\x"));
        assert_eq!(crate::tabs::tab_of_generation(job.generation), 3);
        assert_eq!(b.on_listed(job.generation, listed(r"C:\x", &[]), &[]), shown(&[], true));
    }

    #[test]
    fn up_from_a_network_share_goes_to_its_computer() {
        let mut b = Browser::default();
        visit(&mut b, r"\\srv\share", &["x"]);
        assert!(b.can_up());
        assert_eq!(b.up().unwrap().dir, PathBuf::from(r"\\srv"));
        let mut b = Browser::default();
        visit(&mut b, r"\\srv", &["share"]);
        assert!(!b.can_up(), "a computer is the top");
    }

    #[test]
    fn a_computer_name_is_a_location() {
        assert_eq!(parse_location(r"\\srv", Some(Path::new(r"C:\a"))), Some(PathBuf::from(r"\\srv")));
        assert_eq!(parse_location(r"  \\srv\  ", None), Some(PathBuf::from(r"\\srv")));
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

    #[test]
    fn large_refresh_preserves_selection_across_reordering_and_case_changes() {
        let mut b = Browser::default();
        let mut old = Snapshot::new(PathBuf::from(r"C:\large"));
        let mut new = Snapshot::new(PathBuf::from(r"C:\large"));
        for i in 0..100_001 {
            old.add(&w(&format!("item-{i:06}.txt")), 0, 0, 0);
            new.add(&w(&format!("ITEM-{i:06}.TXT")), 0, 0, 0);
        }
        let job = b.go(old.dir.clone());
        b.on_listed(job.generation, Ok((old, (0..100_001).collect())), &[]);
        let selected: Vec<_> = (0..100_001).step_by(2).collect();
        let job = b.refresh().unwrap();
        let began = std::time::Instant::now();
        let result = b.on_listed(job.generation, Ok((new, (0..100_001).rev().collect())), &selected);
        eprintln!("100k selection restoration: {:?}", began.elapsed());
        assert_eq!(result, shown(&selected.iter().map(|row| 100_000 - row).collect::<Vec<_>>(), false));
    }
}
