//! Where a track's loudness is learned and kept, shared by whoever fetches it (the loudness
//! fetcher, a producer) and whoever applies it (the media engine, a consumer).
//!
//! The number is YouTube's own `loudnessDb` for a video (see `apollo_domain::loudness`). It is
//! not account data and not a credential: it is a few bytes per track, kept in a small
//! in-memory table for the engine and in the app's SQLite store for the next run.
//!
//! Fetching is never on the path of playback. [`LoudnessFetcher::want`] only queues the id; a
//! single worker thread does the (blocking) lookup, one track at a time, and publishes the
//! answer to the [`LoudnessTable`]. A track that starts before its answer arrives plays at unity
//! and is turned down the moment the answer lands.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::unix_now;

/// Entries the in-memory table keeps. A queue is a few hundred tracks at most; the SQLite cache
/// holds the long tail.
const TABLE_CAPACITY: usize = 512;
/// How long "YouTube has no measurement for this track" is believed before asking again.
const UNKNOWN_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// After a failed lookup (offline, refused) the same track is left alone this long.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(60);

/// Loudness by video id, as the engine reads it. Cheap to clone; clones share the table.
#[derive(Clone, Default)]
pub struct LoudnessTable {
    inner: Arc<Mutex<Table>>,
}

#[derive(Default)]
struct Table {
    values: HashMap<String, f32>,
    /// Insertion order, oldest first, for eviction.
    order: VecDeque<String>,
}

impl std::fmt::Debug for LoudnessTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LoudnessTable({} tracks)", self.len())
    }
}

impl LoudnessTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// The loudness in dB for `video_id`, if it is known. `None` is "not known (yet)".
    pub fn get(&self, video_id: &str) -> Option<f32> {
        self.inner.lock().unwrap().values.get(video_id).copied()
    }

    /// Records a measurement. A non-finite value is ignored.
    pub fn set(&self, video_id: &str, loudness_db: f32) {
        if !loudness_db.is_finite() {
            return;
        }
        let mut t = self.inner.lock().unwrap();
        if t.values.insert(video_id.to_owned(), loudness_db).is_none() {
            t.order.push_back(video_id.to_owned());
            while t.order.len() > TABLE_CAPACITY {
                if let Some(old) = t.order.pop_front() {
                    t.values.remove(&old);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.inner.lock().unwrap().values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Where measurements come from. Blocking; called on the fetcher's worker thread only.
pub trait LoudnessSource: Send + Sync + 'static {
    /// `Ok(Some(db))` a measurement, `Ok(None)` the service has none for this track, `Err` the
    /// lookup itself failed (offline, refused): worth asking again later.
    fn fetch(&self, video_id: &str) -> Result<Option<f32>, LoudnessError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoudnessError;

/// One remembered answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CachedLoudness {
    /// `None` = the service had no measurement for this track.
    pub loudness_db: Option<f32>,
    pub fetched_unix: u64,
}

/// Durable memory of answers (SQLite in the app). Blocking; worker thread only.
pub trait LoudnessCache: Send + Sync + 'static {
    fn get(&self, video_id: &str) -> Option<CachedLoudness>;
    fn put(&self, video_id: &str, entry: CachedLoudness);
}

/// A cache that remembers nothing (simulation, tests).
pub struct NoCache;

impl LoudnessCache for NoCache {
    fn get(&self, _: &str) -> Option<CachedLoudness> {
        None
    }
    fn put(&self, _: &str, _: CachedLoudness) {}
}

struct Queue {
    items: VecDeque<String>,
    queued: HashSet<String>,
    closed: bool,
}

struct Inner {
    table: LoudnessTable,
    source: Box<dyn LoudnessSource>,
    cache: Box<dyn LoudnessCache>,
    queue: Mutex<Queue>,
    wake: Condvar,
    failed: Mutex<HashMap<String, Instant>>,
}

/// Learns loudness in the background. Dropping it stops the worker.
pub struct LoudnessFetcher {
    inner: Arc<Inner>,
}

impl LoudnessFetcher {
    pub fn new(
        table: LoudnessTable,
        source: impl LoudnessSource,
        cache: impl LoudnessCache,
    ) -> Self {
        let inner = Arc::new(Inner {
            table,
            source: Box::new(source),
            cache: Box::new(cache),
            queue: Mutex::new(Queue {
                items: VecDeque::new(),
                queued: HashSet::new(),
                closed: false,
            }),
            wake: Condvar::new(),
            failed: Mutex::new(HashMap::new()),
        });
        let worker = inner.clone();
        // A thread that cannot start leaves the table empty: every track plays at unity.
        let _ = thread::Builder::new()
            .name("apollo-loudness".into())
            .spawn(move || worker.run());
        Self { inner }
    }

    /// Wants the loudness of `video_id` known. Never blocks and never fails; a track already
    /// known or already queued is left alone. `urgent` (the track is about to play, or playing)
    /// goes ahead of the lookahead.
    pub fn want(&self, video_id: &str, urgent: bool) {
        if self.inner.table.get(video_id).is_some() {
            return;
        }
        let mut q = self.inner.queue.lock().unwrap();
        if q.closed {
            return;
        }
        if q.queued.contains(video_id) {
            if urgent {
                // Move it to the front.
                if let Some(at) = q.items.iter().position(|i| i == video_id) {
                    let id = q.items.remove(at).expect("position is in range");
                    q.items.push_front(id);
                }
            }
            return;
        }
        q.queued.insert(video_id.to_owned());
        if urgent {
            q.items.push_front(video_id.to_owned());
        } else {
            q.items.push_back(video_id.to_owned());
        }
        self.inner.wake.notify_one();
    }

    /// The table the answers land in.
    pub fn table(&self) -> &LoudnessTable {
        &self.inner.table
    }
}

impl Drop for LoudnessFetcher {
    fn drop(&mut self) {
        self.inner.queue.lock().unwrap().closed = true;
        self.inner.wake.notify_all();
    }
}

impl Inner {
    fn run(&self) {
        loop {
            let id = {
                let mut q = self.queue.lock().unwrap();
                loop {
                    if q.closed {
                        return;
                    }
                    if let Some(id) = q.items.pop_front() {
                        break id;
                    }
                    q = self.wake.wait(q).unwrap();
                }
            };
            self.resolve(&id);
            self.queue.lock().unwrap().queued.remove(&id);
        }
    }

    /// Cache first, then the service. Publishes any measurement to the table.
    fn resolve(&self, video_id: &str) {
        let now = unix_now();
        if let Some(entry) = self.cache.get(video_id) {
            match entry.loudness_db {
                Some(db) => return self.table.set(video_id, db),
                None if now.saturating_sub(entry.fetched_unix) < UNKNOWN_TTL.as_secs() => return,
                None => {}
            }
        }
        {
            let mut failed = self.failed.lock().unwrap();
            failed.retain(|_, at| at.elapsed() < RETRY_AFTER_FAILURE);
            if failed.contains_key(video_id) {
                return;
            }
        }
        match self.source.fetch(video_id) {
            Ok(loudness_db) => {
                if let Some(db) = loudness_db {
                    self.table.set(video_id, db);
                }
                self.cache.put(
                    video_id,
                    CachedLoudness {
                        loudness_db: loudness_db.filter(|d| d.is_finite()),
                        fetched_unix: now,
                    },
                );
            }
            Err(LoudnessError) => {
                self.failed
                    .lock()
                    .unwrap()
                    .insert(video_id.to_owned(), Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    /// Answers from a script and counts the calls.
    struct Scripted {
        answers: Mutex<HashMap<String, Result<Option<f32>, LoudnessError>>>,
        calls: Arc<AtomicUsize>,
    }

    impl Scripted {
        fn new(answers: &[(&str, Result<Option<f32>, LoudnessError>)]) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    answers: Mutex::new(answers.iter().map(|(k, v)| (k.to_string(), *v)).collect()),
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    impl LoudnessSource for Scripted {
        fn fetch(&self, id: &str) -> Result<Option<f32>, LoudnessError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.answers
                .lock()
                .unwrap()
                .get(id)
                .copied()
                .unwrap_or(Err(LoudnessError))
        }
    }

    #[derive(Clone, Default)]
    struct MemCache(Arc<Mutex<HashMap<String, CachedLoudness>>>);

    impl LoudnessCache for MemCache {
        fn get(&self, id: &str) -> Option<CachedLoudness> {
            self.0.lock().unwrap().get(id).copied()
        }
        fn put(&self, id: &str, entry: CachedLoudness) {
            self.0.lock().unwrap().insert(id.to_owned(), entry);
        }
    }

    fn inner(source: Scripted, cache: MemCache) -> (Inner, LoudnessTable) {
        let table = LoudnessTable::new();
        (
            Inner {
                table: table.clone(),
                source: Box::new(source),
                cache: Box::new(cache),
                queue: Mutex::new(Queue {
                    items: VecDeque::new(),
                    queued: HashSet::new(),
                    closed: false,
                }),
                wake: Condvar::new(),
                failed: Mutex::new(HashMap::new()),
            },
            table,
        )
    }

    #[test]
    fn table_keeps_a_bounded_number_and_drops_the_oldest() {
        let t = LoudnessTable::new();
        for i in 0..(TABLE_CAPACITY + 10) {
            t.set(&format!("v{i}"), 1.0);
        }
        assert_eq!(t.len(), TABLE_CAPACITY);
        assert_eq!(t.get("v0"), None);
        assert_eq!(t.get(&format!("v{}", TABLE_CAPACITY + 9)), Some(1.0));
    }

    #[test]
    fn table_ignores_non_finite_values_and_overwrites_without_growing() {
        let t = LoudnessTable::new();
        t.set("a", f32::NAN);
        assert!(t.is_empty());
        t.set("a", 2.0);
        t.set("a", 3.0);
        assert_eq!(t.get("a"), Some(3.0));
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn a_measurement_is_published_and_remembered() {
        let (src, calls) = Scripted::new(&[("a", Ok(Some(4.5)))]);
        let cache = MemCache::default();
        let (inner, table) = inner(src, cache.clone());
        inner.resolve("a");
        assert_eq!(table.get("a"), Some(4.5));
        assert_eq!(cache.get("a").unwrap().loudness_db, Some(4.5));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_remembered_measurement_needs_no_request() {
        let (src, calls) = Scripted::new(&[]);
        let cache = MemCache::default();
        cache.put(
            "a",
            CachedLoudness {
                loudness_db: Some(-2.0),
                fetched_unix: 1,
            },
        );
        let (inner, table) = inner(src, cache);
        inner.resolve("a");
        assert_eq!(
            table.get("a"),
            Some(-2.0),
            "an old known value is still good"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn no_measurement_is_remembered_for_a_day_then_asked_again() {
        let (src, calls) = Scripted::new(&[("a", Ok(None))]);
        let cache = MemCache::default();
        let (inner, table) = inner(src, cache.clone());
        inner.resolve("a");
        assert_eq!(table.get("a"), None);
        assert_eq!(cache.get("a").unwrap().loudness_db, None);
        inner.resolve("a");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "fresh 'unknown' is believed"
        );

        cache.put(
            "a",
            CachedLoudness {
                loudness_db: None,
                fetched_unix: 1,
            },
        );
        inner.resolve("a");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "a stale 'unknown' is asked again"
        );
    }

    #[test]
    fn a_failed_lookup_is_not_remembered_and_not_hammered() {
        let (src, calls) = Scripted::new(&[("a", Err(LoudnessError))]);
        let cache = MemCache::default();
        let (inner, table) = inner(src, cache.clone());
        inner.resolve("a");
        inner.resolve("a");
        assert_eq!(calls.load(Ordering::SeqCst), 1, "backs off after a failure");
        assert_eq!(table.get("a"), None);
        assert!(
            cache.get("a").is_none(),
            "a failure says nothing about the track"
        );
    }

    #[test]
    fn the_worker_fills_the_table_without_the_caller_waiting() {
        let (src, _) = Scripted::new(&[("a", Ok(Some(3.0))), ("b", Ok(Some(5.0)))]);
        let fetcher = LoudnessFetcher::new(LoudnessTable::new(), src, NoCache);
        fetcher.want("a", false);
        fetcher.want("b", true);
        fetcher.want("a", false); // duplicate: harmless
        let deadline = Instant::now() + Duration::from_secs(5);
        while fetcher.table().len() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fetcher.table().get("a"), Some(3.0));
        assert_eq!(fetcher.table().get("b"), Some(5.0));
    }

    #[test]
    fn an_urgent_request_goes_ahead_of_the_lookahead() {
        let (src, _) = Scripted::new(&[]);
        let (inner, _) = inner(src, MemCache::default());
        let fetcher = LoudnessFetcher {
            inner: Arc::new(inner),
        }; // no worker: inspect the queue
        fetcher.want("later", false);
        fetcher.want("sooner", false);
        fetcher.want("now", true);
        fetcher.want("sooner", true);
        let order: Vec<_> = fetcher
            .inner
            .queue
            .lock()
            .unwrap()
            .items
            .iter()
            .cloned()
            .collect();
        assert_eq!(order, ["sooner", "now", "later"]);
    }
}
