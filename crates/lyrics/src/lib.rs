//! Lyrics for the track on screen.
//!
//! * [`LyricsSource`] is one place lyrics can come from: [`Lrclib`] (synced, matched on title,
//!   artist and length) and, in the app, YouTube Music's own plain lyrics behind it. Another
//!   source slots in by implementing the trait and joining the list given to
//!   [`LyricsService::new`] (first synced answer wins, plain text is the fallback).
//! * [`LyricsService`] wraps the sources with a cache keyed by video id. A hit is kept for good;
//!   "not found" is kept for a week; a failure to reach the source is never cached.
//!
//! Everything here blocks: call it off the UI thread.

mod cache;
mod lrc;
mod lrclib;
mod normalize;

use std::fmt;
use std::sync::Arc;

pub use cache::{LyricsCache, SqliteCache, NOT_FOUND_TTL};
pub use lrc::{parse_lrc, to_lrc};
pub use lrclib::{Http, HttpReply, Lrclib, UreqHttp, USER_AGENT};

/// One line of synced lyrics: it starts at `start_ms` and lasts until the next line starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub start_ms: u64,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lyrics {
    /// Lines in time order, never empty.
    Synced(Vec<Line>),
    /// Unsynced text, never blank.
    Plain(String),
}

impl Lyrics {
    pub fn is_synced(&self) -> bool {
        matches!(self, Lyrics::Synced(_))
    }
}

/// What to look up: the track's identity as the service lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LyricsQuery {
    pub video_id: String,
    pub title: String,
    /// All artists, comma separated.
    pub artist: String,
    /// The first artist alone, for sources that match one name.
    pub first_artist: String,
    pub album: Option<String>,
    /// Whole seconds, when the track has a length.
    pub duration_secs: Option<u32>,
}

/// The source could not be asked (offline, timed out, server error). Not a "no lyrics" answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FetchError(pub String);

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lyrics source unavailable: {}", self.0)
    }
}

impl std::error::Error for FetchError {}

pub trait LyricsSource: Send + Sync {
    /// `Ok(None)` is a definite "this source has nothing for the track".
    fn fetch(&self, query: &LyricsQuery) -> Result<Option<Lyrics>, FetchError>;

    /// A source that never has synced lyrics. It is not asked once plain text is in hand.
    fn plain_only(&self) -> bool {
        false
    }
}

pub struct LyricsService {
    sources: Vec<Arc<dyn LyricsSource>>,
    cache: Option<Arc<dyn LyricsCache>>,
}

impl LyricsService {
    pub fn new(sources: Vec<Arc<dyn LyricsSource>>, cache: Option<Arc<dyn LyricsCache>>) -> Self {
        Self { sources, cache }
    }

    /// The cached answer, if there is one: `Some(None)` is a remembered "no lyrics".
    pub fn cached(&self, video_id: &str) -> Option<Option<Lyrics>> {
        self.cache.as_ref()?.get(video_id)
    }

    /// Cache first, then the sources in order. The first synced answer wins; otherwise the
    /// first plain one. `Err` only when no source answered at all.
    pub fn lookup(&self, query: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        if let Some(hit) = self.cached(&query.video_id) {
            return Ok(hit);
        }
        let mut best = None;
        let mut failure = None;
        let mut answered = false;
        for source in &self.sources {
            if best.is_some() && source.plain_only() {
                continue;
            }
            match source.fetch(query) {
                Ok(Some(found @ Lyrics::Synced(_))) => {
                    best = Some(found);
                    answered = true;
                    break;
                }
                Ok(Some(found)) => {
                    answered = true;
                    best.get_or_insert(found);
                }
                Ok(None) => answered = true,
                Err(e) => failure = Some(e),
            }
        }
        if !answered {
            return Err(failure.unwrap_or_else(|| FetchError("no source".into())));
        }
        // A source that failed may have had something the others lack, so a partial answer of
        // "nothing" is not remembered; a found result always is.
        if best.is_some() || failure.is_none() {
            if let Some(cache) = &self.cache {
                cache.put(&query.video_id, best.as_ref());
            }
        }
        Ok(best)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    fn query(id: &str) -> LyricsQuery {
        LyricsQuery {
            video_id: id.into(),
            title: "T".into(),
            artist: "A".into(),
            first_artist: "A".into(),
            album: None,
            duration_secs: Some(200),
        }
    }

    struct Fixed(Result<Option<Lyrics>, FetchError>, AtomicUsize);
    impl Fixed {
        fn new(r: Result<Option<Lyrics>, FetchError>) -> Arc<Self> {
            Arc::new(Self(r, AtomicUsize::new(0)))
        }
        fn calls(&self) -> usize {
            self.1.load(Ordering::SeqCst)
        }
    }
    impl LyricsSource for Fixed {
        fn fetch(&self, _: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
            self.1.fetch_add(1, Ordering::SeqCst);
            self.0.clone()
        }
    }

    #[derive(Default)]
    struct MemCache(Mutex<std::collections::HashMap<String, Option<Lyrics>>>);
    impl LyricsCache for MemCache {
        fn get(&self, id: &str) -> Option<Option<Lyrics>> {
            self.0.lock().unwrap().get(id).cloned()
        }
        fn put(&self, id: &str, lyrics: Option<&Lyrics>) {
            self.0.lock().unwrap().insert(id.into(), lyrics.cloned());
        }
    }

    fn synced() -> Lyrics {
        Lyrics::Synced(vec![Line {
            start_ms: 1000,
            text: "hi".into(),
        }])
    }

    #[test]
    fn a_found_answer_and_a_not_found_answer_are_both_asked_for_once() {
        let source = Fixed::new(Ok(Some(synced())));
        let cache = Arc::new(MemCache::default());
        let service = LyricsService::new(vec![source.clone()], Some(cache.clone()));
        assert_eq!(service.lookup(&query("a")).unwrap(), Some(synced()));
        assert_eq!(service.lookup(&query("a")).unwrap(), Some(synced()));
        assert_eq!(source.calls(), 1);

        let nothing = Fixed::new(Ok(None));
        let service = LyricsService::new(vec![nothing.clone()], Some(cache));
        assert_eq!(service.lookup(&query("b")).unwrap(), None);
        assert_eq!(service.lookup(&query("b")).unwrap(), None);
        assert_eq!(nothing.calls(), 1);
    }

    #[test]
    fn a_failure_is_reported_and_not_remembered() {
        let broken = Fixed::new(Err(FetchError("offline".into())));
        let cache = Arc::new(MemCache::default());
        let service = LyricsService::new(vec![broken.clone()], Some(cache.clone()));
        assert!(service.lookup(&query("a")).is_err());
        assert!(service.lookup(&query("a")).is_err());
        assert_eq!(broken.calls(), 2);
        assert_eq!(cache.get("a"), None);
    }

    #[test]
    fn the_first_synced_source_wins_and_plain_is_the_fallback() {
        let plain = Lyrics::Plain("words".into());
        let a = Fixed::new(Ok(Some(plain.clone())));
        let b = Fixed::new(Ok(Some(synced())));
        let c = Fixed::new(Ok(Some(Lyrics::Plain("other".into()))));
        let service = LyricsService::new(vec![a.clone(), b.clone(), c.clone()], None);
        assert_eq!(service.lookup(&query("x")).unwrap(), Some(synced()));
        assert_eq!(c.calls(), 0, "nothing is asked after a synced answer");

        let service = LyricsService::new(vec![a, c], None);
        assert_eq!(service.lookup(&query("y")).unwrap(), Some(plain));
    }

    #[test]
    fn a_plain_only_source_is_not_asked_once_there_is_text() {
        struct PlainOnly(Arc<Fixed>);
        impl LyricsSource for PlainOnly {
            fn fetch(&self, q: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
                self.0.fetch(q)
            }
            fn plain_only(&self) -> bool {
                true
            }
        }
        let first = Fixed::new(Ok(Some(Lyrics::Plain("from the first".into()))));
        let behind = Fixed::new(Ok(Some(Lyrics::Plain("from behind".into()))));
        let service = LyricsService::new(vec![first, Arc::new(PlainOnly(behind.clone()))], None);
        assert_eq!(
            service.lookup(&query("a")).unwrap(),
            Some(Lyrics::Plain("from the first".into()))
        );
        assert_eq!(behind.calls(), 0);

        // With nothing found before it, it is asked and its text is used.
        let nothing = Fixed::new(Ok(None));
        let service = LyricsService::new(vec![nothing, Arc::new(PlainOnly(behind.clone()))], None);
        assert_eq!(
            service.lookup(&query("b")).unwrap(),
            Some(Lyrics::Plain("from behind".into()))
        );
        assert_eq!(behind.calls(), 1);
    }

    #[test]
    fn one_source_failing_still_lets_another_answer() {
        let broken = Fixed::new(Err(FetchError("offline".into())));
        let nothing = Fixed::new(Ok(None));
        let cache = Arc::new(MemCache::default());
        let service = LyricsService::new(vec![broken, nothing], Some(cache.clone()));
        assert_eq!(service.lookup(&query("a")).unwrap(), None);
        // The failed source might have had it, so "none" is not written down.
        assert_eq!(cache.get("a"), None);
    }
}
