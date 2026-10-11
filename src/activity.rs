//! In-memory operation history. Windows owns progress and cancellation dialogs.
use std::{
    collections::VecDeque,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(1);
static JOBS: Mutex<VecDeque<(u64, String, Option<String>)>> = Mutex::new(VecDeque::new());
pub struct Job {
    id: u64,
    done: bool,
}
impl Job {
    pub fn start(label: String) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let mut jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
        while jobs.len() >= 100 {
            if let Some(old) = jobs.iter().rposition(|j| j.2.is_some()) {
                jobs.remove(old);
            } else {
                break;
            }
        }
        jobs.push_front((id, label, None));
        Self { id, done: false }
    }
    pub fn finish(mut self, result: &Result<Option<String>, String>) {
        self.status(match result {
            Ok(_) => "Completed".into(),
            Err(e) => format!("Stopped: {e}"),
        });
        self.done = true;
    }
    fn status(&self, status: String) {
        if let Some(job) = JOBS.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().find(|j| j.0 == self.id)
        {
            job.2 = Some(status);
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if !self.done {
            self.status("Interrupted".into());
        }
    }
}
pub fn text() -> String {
    let jobs = JOBS.lock().unwrap_or_else(|e| e.into_inner());
    if jobs.is_empty() {
        return "No file operations in this session.".into();
    }
    jobs.iter()
        .map(|(_, label, state)| format!("{}\r\n{}\r\n", state.as_deref().unwrap_or("Running"), label))
        .collect::<Vec<_>>()
        .join("\r\n")
}
