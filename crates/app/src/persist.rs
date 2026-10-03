//! Keeps the saved queue in step with the live one, off the UI thread.
//!
//! The model hands over a snapshot whenever the queue changes and a bare position now and then.
//! One worker thread does all the writing, in order, and folds a burst of changes into the last
//! one, so a flurry of edits is a single write and an old snapshot can never land after a newer
//! one. Only the final write at exit waits (briefly) for the worker.

use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use apollo_domain::queue::QueueSnapshot;

use crate::settings::QueueStore;

/// How long quitting waits for the last write.
const FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

enum Job {
    Save(Box<QueueSnapshot>, u64),
    Position(u64),
    Forget,
    /// Everything before this is written; then the sender is told.
    Barrier(Sender<()>),
}

pub struct QueueSaver {
    tx: Sender<Job>,
}

impl QueueSaver {
    /// A saver writing under `account_key`.
    pub fn new(store: Arc<dyn QueueStore>, account_key: String) -> Self {
        let (tx, rx) = channel::<Job>();
        let _ = std::thread::Builder::new()
            .name("apollo-queue-save".into())
            .spawn(move || {
                let mut pending = rx.recv().ok();
                while let Some(first) = pending.take() {
                    // Fold what is already waiting: the last snapshot wins, the last position
                    // wins, and a barrier or a forget ends the fold so order is kept.
                    let mut snapshot = None;
                    let mut position = None;
                    let mut tail = None;
                    let mut job = Some(first);
                    while let Some(j) = job.take() {
                        match j {
                            Job::Save(s, p) => {
                                snapshot = Some(s);
                                position = Some(p);
                            }
                            Job::Position(p) => position = Some(p),
                            other => {
                                tail = Some(other);
                                break;
                            }
                        }
                        match rx.try_recv() {
                            Ok(next) => job = Some(next),
                            Err(TryRecvError::Empty | TryRecvError::Disconnected) => {}
                        }
                    }
                    match (snapshot, position) {
                        (Some(s), Some(p)) => store.save(&account_key, &s, p),
                        (None, Some(p)) => store.checkpoint(p),
                        _ => {}
                    }
                    match tail {
                        Some(Job::Forget) => store.forget(),
                        Some(Job::Barrier(done)) => {
                            let _ = done.send(());
                        }
                        _ => {}
                    }
                    pending = rx.recv().ok();
                }
            });
        Self { tx }
    }

    /// The queue changed: write it with the position it is at.
    pub fn save(&self, snapshot: QueueSnapshot, position_ms: u64) {
        let _ = self.tx.send(Job::Save(Box::new(snapshot), position_ms));
    }

    /// Only the position moved.
    pub fn checkpoint(&self, position_ms: u64) {
        let _ = self.tx.send(Job::Position(position_ms));
    }

    /// Delete what is saved (sign-out).
    pub fn forget(&self) {
        let _ = self.tx.send(Job::Forget);
    }

    /// Wait until everything handed over so far is written, up to a short timeout.
    pub fn flush(&self) {
        let (done, wait) = channel();
        if self.tx.send(Job::Barrier(done)).is_ok() {
            let _ = wait.recv_timeout(FLUSH_TIMEOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use apollo_domain::model::Track;
    use apollo_domain::queue::Queue;

    use super::*;

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);

    impl QueueStore for Log {
        fn load(&self, _: &str) -> Option<(Queue, u64)> {
            None
        }
        fn save(&self, key: &str, snap: &QueueSnapshot, position_ms: u64) {
            let current = snap
                .current
                .as_ref()
                .map(|(e, _)| e.track.video_id.as_str().to_owned())
                .unwrap_or_default();
            self.0
                .lock()
                .unwrap()
                .push(format!("save {key} {current} {position_ms}"));
        }
        fn checkpoint(&self, position_ms: u64) {
            self.0.lock().unwrap().push(format!("position {position_ms}"));
        }
        fn forget(&self) {
            self.0.lock().unwrap().push("forget".into());
        }
    }

    fn snapshot(first: &str) -> QueueSnapshot {
        let mut q = Queue::new();
        q.play_context(vec![Track::new(first, first), Track::new("zz", "zz")], 0);
        q.snapshot()
    }

    #[test]
    fn writes_arrive_in_order_and_flush_waits_for_them() {
        let log = Arc::new(Log::default());
        let saver = QueueSaver::new(log.clone(), "acct".into());
        saver.save(snapshot("a"), 0);
        saver.flush();
        saver.checkpoint(5_000);
        saver.save(snapshot("b"), 7);
        saver.forget();
        saver.flush();
        let lines = log.0.lock().unwrap().clone();
        // The first save is its own write; what followed it may have been folded into one.
        assert_eq!(lines[0], "save acct a 0");
        assert_eq!(lines.last().unwrap(), "forget");
        let middle = &lines[1..lines.len() - 1];
        assert!(
            middle == ["position 5000", "save acct b 7"] || middle == ["save acct b 7"],
            "{middle:?}"
        );
    }

    #[test]
    fn a_burst_of_changes_ends_on_the_last_one() {
        let log = Arc::new(Log::default());
        let saver = QueueSaver::new(log.clone(), "acct".into());
        for n in 0..50u64 {
            saver.save(snapshot("a"), n);
        }
        saver.save(snapshot("z"), 99);
        saver.flush();
        let lines = log.0.lock().unwrap().clone();
        assert_eq!(lines.last().unwrap(), "save acct z 99");
        assert!(lines.len() <= 51);
    }
}
