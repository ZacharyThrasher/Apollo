//! Lyrics cache in the app's SQLite store, keyed by video id.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use apollo_store::Store;

use crate::{parse_lrc, to_lrc, Lyrics};

/// How long a remembered "no lyrics" is believed. Sources add lyrics over time.
pub const NOT_FOUND_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);

pub trait LyricsCache: Send + Sync {
    /// `Some(None)` is a remembered "no lyrics"; `None` means nothing is known.
    fn get(&self, video_id: &str) -> Option<Option<Lyrics>>;
    fn put(&self, video_id: &str, lyrics: Option<&Lyrics>);
}

pub struct SqliteCache {
    path: PathBuf,
}

impl SqliteCache {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    fn open(&self) -> Option<Store> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        match Store::open(&self.path) {
            Ok(store) => Some(store),
            Err(err) => {
                eprintln!("lyrics cache unavailable: {err}");
                None
            }
        }
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl LyricsCache for SqliteCache {
    fn get(&self, video_id: &str) -> Option<Option<Lyrics>> {
        let row = self.open()?.get_lyrics(video_id).ok()??;
        match row.kind.as_str() {
            "synced" => {
                let lines = parse_lrc(&row.body);
                (!lines.is_empty()).then_some(Some(Lyrics::Synced(lines)))
            }
            "plain" => (!row.body.trim().is_empty()).then(|| Some(Lyrics::Plain(row.body))),
            "none" => {
                let age = now_secs().saturating_sub(row.fetched_at);
                (age < NOT_FOUND_TTL.as_secs() as i64).then_some(None)
            }
            _ => None,
        }
    }

    fn put(&self, video_id: &str, lyrics: Option<&Lyrics>) {
        let Some(store) = self.open() else { return };
        let (kind, body) = match lyrics {
            Some(Lyrics::Synced(lines)) => ("synced", to_lrc(lines)),
            Some(Lyrics::Plain(text)) => ("plain", text.clone()),
            None => ("none", String::new()),
        };
        if let Err(err) = store.put_lyrics(video_id, kind, &body, now_secs()) {
            eprintln!("could not cache lyrics: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Line;

    #[test]
    fn found_plain_and_not_found_round_trip_and_an_old_not_found_expires() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("apollo.db");
        let cache = SqliteCache::at(path.clone());
        assert_eq!(cache.get("a"), None);

        let synced = Lyrics::Synced(vec![
            Line {
                start_ms: 13130,
                text: "Yeah".into(),
            },
            Line {
                start_ms: 16560,
                text: "♪".into(),
            },
        ]);
        cache.put("a", Some(&synced));
        cache.put("b", Some(&Lyrics::Plain("a\nb".into())));
        cache.put("c", None);
        assert_eq!(cache.get("a"), Some(Some(synced)));
        assert_eq!(cache.get("b"), Some(Some(Lyrics::Plain("a\nb".into()))));
        assert_eq!(cache.get("c"), Some(None));

        // A "no lyrics" answer from long ago is asked again; a found one never ages out.
        let store = Store::open(&path).unwrap();
        let old = now_secs() - NOT_FOUND_TTL.as_secs() as i64 - 1;
        store.put_lyrics("c", "none", "", old).unwrap();
        store.put_lyrics("a", "synced", "[00:01.00]x", old).unwrap();
        assert_eq!(cache.get("c"), None);
        assert!(cache.get("a").unwrap().is_some());
    }
}
