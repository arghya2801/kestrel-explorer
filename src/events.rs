//! Worker -> UI event queue. Workers never touch windows: they `send` here and
//! the UI thread is woken (one posted message while anything is pending, so a
//! burst of events can't flood the 10,000-message thread queue).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

pub struct Events<T> {
    queue: Mutex<Vec<T>>,
    pending: AtomicBool,
    closed: AtomicBool,
    /// Wakes the UI thread; returns false if the wake could not be delivered.
    wake: Box<dyn Fn() -> bool + Send + Sync>,
}

impl<T> Events<T> {
    pub fn new(wake: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Events {
            queue: Mutex::new(Vec::new()),
            pending: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            wake: Box::new(wake),
        }
    }

    pub fn send(&self, ev: T) {
        {
            let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if self.closed.load(Ordering::SeqCst) {
                return;
            }
            queue.push(ev);
        }
        if !self.pending.swap(true, Ordering::SeqCst) && !(self.wake)() {
            self.pending.store(false, Ordering::SeqCst);
        }
    }

    pub fn close(&self) {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
        self.closed.store(true, Ordering::SeqCst);
        queue.clear();
    }

    /// Takes everything queued. Clears `pending` first so a concurrent send wakes again.
    pub fn drain(&self) -> Vec<T> {
        self.pending.store(false, Ordering::SeqCst);
        std::mem::take(&mut *self.queue.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;

    fn counting() -> (Arc<AtomicUsize>, Events<u32>) {
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let ev = Events::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
            true
        });
        (n, ev)
    }

    #[test]
    fn many_sends_wake_the_ui_once() {
        let (wakes, ev) = counting();
        ev.send(1);
        ev.send(2);
        ev.send(3);
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        assert_eq!(ev.drain(), [1, 2, 3]);
    }

    #[test]
    fn send_after_drain_wakes_again() {
        let (wakes, ev) = counting();
        ev.send(1);
        ev.drain();
        ev.send(2);
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert_eq!(ev.drain(), [2]);
    }

    #[test]
    fn failed_wake_does_not_block_later_wakes() {
        let tries = Arc::new(AtomicUsize::new(0));
        let t2 = tries.clone();
        let ev = Events::new(move || t2.fetch_add(1, Ordering::SeqCst) > 0); // first wake fails
        ev.send(1);
        ev.send(2);
        assert_eq!(tries.load(Ordering::SeqCst), 2);
        assert_eq!(ev.drain(), [1, 2]);
    }

    #[test]
    fn works_across_threads() {
        let (_, ev) = counting();
        let ev = Arc::new(ev);
        let hs: Vec<_> = (0..4)
            .map(|t| {
                let ev = ev.clone();
                std::thread::spawn(move || (0..100).for_each(|i| ev.send(t * 100 + i)))
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        let mut got = ev.drain();
        got.sort();
        assert_eq!(got, (0..400).collect::<Vec<_>>());
    }
    #[test]
    fn closing_discards_queued_and_late_resources() {
        let resource = Arc::new(());
        let events = Events::new(|| true);
        events.send(resource.clone());
        assert_eq!(Arc::strong_count(&resource), 2);
        events.close();
        events.send(resource.clone());
        assert!(events.drain().is_empty());
        assert_eq!(Arc::strong_count(&resource), 1);
    }
}
