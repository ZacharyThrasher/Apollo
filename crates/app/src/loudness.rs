//! Volume normalization, app side: learn each track's loudness alongside the resolver, and keep
//! the answers in SQLite.
//!
//! [`LoudnessResolver`] wraps the real resolver. Everything that resolves a track (a click, the
//! coordinator's preload, the hover and lookahead prefetch) also asks the fetcher for that
//! track's loudness, so the number is normally known before the track plays. The fetcher never
//! blocks the resolver or the UI: it only queues the id.

use std::path::PathBuf;

use apollo_domain::model::Track;
use apollo_domain::playback::LoadId;
use apollo_runtime::{ResolveRequest, Resolver};
use apollo_services_common::{CachedLoudness, LoudnessCache, LoudnessFetcher};
use apollo_store::{LoudnessRow, Store};

/// A resolver that also gets the loudness of every track it is asked about.
pub struct LoudnessResolver<R> {
    inner: R,
    fetcher: LoudnessFetcher,
}

impl<R> LoudnessResolver<R> {
    pub fn new(inner: R, fetcher: LoudnessFetcher) -> Self {
        Self { inner, fetcher }
    }
}

impl<R: Resolver> Resolver for LoudnessResolver<R> {
    fn start(&mut self, request: ResolveRequest) {
        // The track playing now (or about to) goes ahead of the lookahead.
        self.fetcher
            .want(request.track.video_id.as_str(), !request.preload);
        self.inner.start(request);
    }

    fn cancel(&mut self, load: LoadId) {
        self.inner.cancel(load);
    }

    fn prefetch(&mut self, track: &Track) {
        self.fetcher.want(track.video_id.as_str(), false);
        self.inner.prefetch(track);
    }
}

/// The loudness cache in the app's SQLite file. Opened per call, like the other settings.
pub struct SqliteLoudness {
    path: PathBuf,
}

impl SqliteLoudness {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    fn open(&self) -> Option<Store> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        Store::open(&self.path).ok()
    }
}

impl LoudnessCache for SqliteLoudness {
    fn get(&self, video_id: &str) -> Option<CachedLoudness> {
        let row = self.open()?.get_loudness(video_id).ok()??;
        Some(CachedLoudness {
            loudness_db: row.loudness_db,
            fetched_unix: row.fetched_unix,
        })
    }

    fn put(&self, video_id: &str, entry: CachedLoudness) {
        if let Some(store) = self.open() {
            let _ = store.put_loudness(
                video_id,
                LoudnessRow {
                    loudness_db: entry.loudness_db,
                    fetched_unix: entry.fetched_unix,
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use apollo_domain::model::Track;
    use apollo_services_common::{LoudnessError, LoudnessSource, LoudnessTable, NoCache};

    use super::*;

    #[derive(Default)]
    struct Calls(Arc<Mutex<Vec<String>>>);

    struct Recording(Arc<Mutex<Vec<String>>>);

    impl Resolver for Recording {
        fn start(&mut self, request: ResolveRequest) {
            self.0
                .lock()
                .unwrap()
                .push(format!("start {}", request.track.video_id.as_str()));
        }
        fn cancel(&mut self, _: LoadId) {}
        fn prefetch(&mut self, track: &Track) {
            self.0
                .lock()
                .unwrap()
                .push(format!("prefetch {}", track.video_id.as_str()));
        }
    }

    struct Fixed;

    impl LoudnessSource for Fixed {
        fn fetch(&self, id: &str) -> Result<Option<f32>, LoudnessError> {
            Ok(match id {
                "loud" => Some(6.0),
                "quiet" => Some(-4.0),
                _ => None,
            })
        }
    }

    fn wait_for(table: &LoudnessTable, id: &str) -> Option<f32> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while table.get(id).is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        table.get(id)
    }

    #[test]
    fn a_prefetch_resolves_the_track_and_learns_its_loudness() {
        let calls = Calls::default();
        let table = LoudnessTable::new();
        let fetcher = LoudnessFetcher::new(table.clone(), Fixed, NoCache);
        let mut resolver = LoudnessResolver::new(Recording(calls.0.clone()), fetcher);
        resolver.prefetch(&Track::new("loud", "Loud"));
        resolver.prefetch(&Track::new("quiet", "Quiet"));
        assert_eq!(wait_for(&table, "loud"), Some(6.0));
        assert_eq!(wait_for(&table, "quiet"), Some(-4.0));
        assert_eq!(
            *calls.0.lock().unwrap(),
            ["prefetch loud", "prefetch quiet"],
            "the resolver still gets every call, in order"
        );
    }

    #[test]
    fn the_cache_remembers_across_runs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let cache = SqliteLoudness::at(path.clone());
        assert_eq!(cache.get("a"), None);
        let entry = CachedLoudness {
            loudness_db: Some(3.5),
            fetched_unix: 1_800_000_000,
        };
        cache.put("a", entry);
        assert_eq!(SqliteLoudness::at(path).get("a"), Some(entry));
    }
}
