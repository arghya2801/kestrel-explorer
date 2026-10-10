//! The window's tabs: an ordered list with one active, Explorer's rules for
//! where new tabs go and which tab takes over when one closes.

/// Each tab numbers its navigations from its own base, so a worker's result
/// says which tab it belongs to.
const TAB_SHIFT: u32 = 40;

pub fn generation_base(tab_id: u64) -> u64 {
    tab_id << TAB_SHIFT
}

pub fn tab_of_generation(generation: u64) -> u64 {
    generation >> TAB_SHIFT
}

pub struct Tabs<T> {
    items: Vec<T>,
    active: usize,
}

impl<T> Tabs<T> {
    pub fn new(first: T) -> Self {
        Tabs { items: vec![first], active: 0 }
    }

    pub fn items(&self) -> &[T] {
        &self.items
    }
    pub fn items_mut(&mut self) -> &mut [T] {
        &mut self.items
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn active(&self) -> usize {
        self.active
    }

    /// Inserts `item` right of the active tab and makes it active. Returns its index.
    pub fn open(&mut self, item: T) -> usize {
        self.active += 1;
        self.items.insert(self.active, item);
        self.active
    }

    /// Removes tab `i`; the last tab can't be closed. If it was active, the
    /// tab to its right takes over (or the new last one).
    pub fn close(&mut self, i: usize) -> Option<T> {
        if self.items.len() <= 1 || i >= self.items.len() {
            return None;
        }
        let item = self.items.remove(i);
        if i < self.active || self.active >= self.items.len() {
            self.active -= 1;
        }
        Some(item)
    }

    pub fn select(&mut self, i: usize) {
        if i < self.items.len() {
            self.active = i;
        }
    }

    /// The tab after the active one, wrapping around.
    pub fn next_index(&self) -> usize {
        (self.active + 1) % self.items.len()
    }

    /// The tab before the active one, wrapping around.
    pub fn prev_index(&self) -> usize {
        (self.active + self.items.len() - 1) % self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(names: &[&'static str], active: usize) -> Tabs<&'static str> {
        let mut t = Tabs::new(names[0]);
        for &n in &names[1..] {
            t.open(n);
        }
        t.select(active);
        t
    }

    #[test]
    fn new_tabs_open_right_of_the_active_one_and_become_active() {
        let mut t = tabs(&["a", "b", "c"], 0);
        assert_eq!(t.open("new"), 1);
        assert_eq!(t.items(), ["a", "new", "b", "c"]);
        assert_eq!(t.active(), 1);
    }

    #[test]
    fn closing_the_active_tab_activates_its_right_neighbour_or_the_last() {
        let mut t = tabs(&["a", "b", "c"], 1);
        assert_eq!(t.close(1), Some("b"));
        assert_eq!((t.items(), t.active()), (&["a", "c"][..], 1));
        assert_eq!(t.close(1), Some("c"));
        assert_eq!((t.items(), t.active()), (&["a"][..], 0));
    }

    #[test]
    fn closing_another_tab_keeps_the_same_tab_active() {
        let mut t = tabs(&["a", "b", "c"], 2);
        t.close(0);
        assert_eq!(t.items()[t.active()], "c");
    }

    #[test]
    fn the_last_tab_cannot_be_closed() {
        let mut t = Tabs::new("only");
        assert_eq!(t.close(0), None);
        assert_eq!(t.len(), 1);
        assert_eq!(t.close(5), None);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut t = tabs(&["a", "b", "c"], 2);
        assert_eq!((t.next_index(), t.prev_index()), (0, 1));
        t.select(0);
        assert_eq!((t.next_index(), t.prev_index()), (1, 2));
        t.select(99); // out of range: ignored
        assert_eq!(t.active(), 0);
    }

    #[test]
    fn generation_ranges_identify_the_tab() {
        let base = generation_base(3);
        assert_eq!(tab_of_generation(base + 1), 3);
        assert_eq!(tab_of_generation(generation_base(0) + 12345), 0);
        assert_ne!(generation_base(1), generation_base(2));
    }
}
