//! Memory-only reuse of resolved stream URLs.
//!
//! The cache maps a video id to a lease that already sits in the [`LeaseStore`]; nothing is
//! copied and nothing is written anywhere. A hit must still be fresh by the same rule the media
//! engine applies (reported expiry, with `min_remaining` of headroom so the track can play
//! through; unknown expiry only for a few minutes), so a stale URL is never handed out.
//! Hits are only served while the account has a session (the caller checks), and `clear`
//! forgets everything on logout.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use apollo_domain::playback::LeaseId;
use apollo_services_common::{unix_now, LeaseStore};

use crate::config::CacheConfig;

struct Entry {
    lease: LeaseId,
    at: Instant,
}

pub struct LeaseCache {
    cfg: CacheConfig,
    entries: Mutex<HashMap<String, Entry>>,
}

impl LeaseCache {
    pub fn new(cfg: CacheConfig) -> Self {
        Self {
            cfg,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The cached lease for `video_id` if it is still usable for a whole track.
    pub fn get(&self, video_id: &str, leases: &LeaseStore) -> Option<LeaseId> {
        let mut m = self.entries.lock().unwrap();
        let id = m.get(video_id)?.lease;
        match leases.get_fresh(id, unix_now(), self.cfg.min_remaining, self.cfg.max_unknown_age) {
            Ok(_) => Some(id),
            Err(_) => {
                m.remove(video_id); // expired or gone: forget it
                None
            }
        }
    }

    pub fn insert(&self, video_id: &str, lease: LeaseId) {
        let mut m = self.entries.lock().unwrap();
        if m.len() >= self.cfg.max_entries && !m.contains_key(video_id) {
            if let Some(oldest) = m.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) {
                m.remove(&oldest);
            }
        }
        m.insert(
            video_id.to_owned(),
            Entry {
                lease,
                at: Instant::now(),
            },
        );
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::lease::{AudioCodec, StreamCandidate, Transport, UrlPolicy};
    use apollo_domain::model::VideoId;
    use std::time::Duration;

    fn lease(id: &str, expires_in: Option<u64>) -> apollo_domain::lease::StreamLease {
        let c = StreamCandidate {
            video_id: id.into(),
            url: "https://rr1---sn-a.googlevideo.com/videoplayback?sig=X".into(),
            transport: Transport::Https,
            has_audio: true,
            drm: false,
            codec: Some(AudioCodec::Opus),
            bitrate_kbps: Some(256),
            sample_rate_hz: Some(48_000),
            duration: Some(Duration::from_secs(200)),
            expires_at_unix: expires_in.map(|s| unix_now() + s),
            headers: vec![],
        };
        UrlPolicy::production()
            .validate(&VideoId::new(id), c)
            .unwrap()
    }

    fn cfg() -> CacheConfig {
        CacheConfig {
            max_entries: 2,
            min_remaining: Duration::from_secs(600),
            max_unknown_age: Duration::from_secs(300),
        }
    }

    #[test]
    fn serves_a_fresh_lease_and_forgets_a_short_lived_one() {
        let leases = LeaseStore::new();
        let cache = LeaseCache::new(cfg());
        let long = leases.insert(lease("aaaaaaaaaaa", Some(6 * 3600)));
        let short = leases.insert(lease("bbbbbbbbbbb", Some(60)));
        cache.insert("aaaaaaaaaaa", long);
        cache.insert("bbbbbbbbbbb", short);
        assert_eq!(cache.get("aaaaaaaaaaa", &leases), Some(long));
        assert_eq!(
            cache.get("bbbbbbbbbbb", &leases),
            None,
            "expires within min_remaining: not safe to start a track from"
        );
        assert_eq!(cache.len(), 1, "the unusable entry is dropped");
    }

    #[test]
    fn a_lease_missing_from_the_store_is_a_miss() {
        let leases = LeaseStore::new();
        let cache = LeaseCache::new(cfg());
        let id = leases.insert(lease("aaaaaaaaaaa", Some(6 * 3600)));
        cache.insert("aaaaaaaaaaa", id);
        leases.clear(); // logout
        assert_eq!(cache.get("aaaaaaaaaaa", &leases), None);
    }

    #[test]
    fn evicts_the_oldest_beyond_the_limit() {
        let leases = LeaseStore::new();
        let cache = LeaseCache::new(cfg());
        for id in ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc"] {
            let l = leases.insert(lease(id, Some(6 * 3600)));
            cache.insert(id, l);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(cache.len(), 2);
        assert!(cache.get("aaaaaaaaaaa", &leases).is_none());
        assert!(cache.get("ccccccccccc", &leases).is_some());
    }

    #[test]
    fn clear_forgets_everything() {
        let leases = LeaseStore::new();
        let cache = LeaseCache::new(cfg());
        let l = leases.insert(lease("aaaaaaaaaaa", Some(6 * 3600)));
        cache.insert("aaaaaaaaaaa", l);
        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.get("aaaaaaaaaaa", &leases), None);
    }
}
