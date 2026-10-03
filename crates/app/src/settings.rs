//! What lives between runs, in the app's SQLite store (non-secret state only): the volume, the
//! autoplay switch (both in the `settings` table) and the queue with its position.

use std::path::PathBuf;

use apollo_domain::queue::{Queue, QueueSnapshot};
use apollo_store::Store;

const KEY_VOLUME: &str = "volume_percent";
const KEY_AUTOPLAY: &str = "autoplay";

/// Where the volume is kept. Blocking: call off the UI thread.
pub trait VolumeStore: Send + Sync {
    fn load(&self) -> Option<u8>;
    fn save(&self, percent: u8);
}

/// Whether autoplay is on. Blocking: call off the UI thread.
pub trait AutoplayStore: Send + Sync {
    fn load(&self) -> Option<bool>;
    fn save(&self, on: bool);
}

/// The saved queue, bound to the account that made it. Blocking: call off the UI thread.
pub trait QueueStore: Send + Sync {
    /// The queue saved for `account_key` and the position it was left at. A queue saved for
    /// another account is forgotten, not returned.
    fn load(&self, account_key: &str) -> Option<(Queue, u64)>;
    fn save(&self, account_key: &str, snapshot: &QueueSnapshot, position_ms: u64);
    /// Move the saved position only (the queue itself is unchanged).
    fn checkpoint(&self, position_ms: u64);
    /// Drop the saved queue (sign-out).
    fn forget(&self);
}

pub struct SqliteSettings {
    path: PathBuf,
}

impl SqliteSettings {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// `%LOCALAPPDATA%\Apollo\app\apollo.db`. `None` when the variable is unset (non-Windows),
    /// which leaves persistence off.
    pub fn default_location() -> Option<Self> {
        let base = std::env::var_os("LOCALAPPDATA")?;
        Some(Self::at(
            PathBuf::from(base)
                .join("Apollo")
                .join("app")
                .join("apollo.db"),
        ))
    }

    fn open(&self) -> Option<Store> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        match Store::open(&self.path) {
            Ok(store) => Some(store),
            Err(err) => {
                eprintln!("settings store unavailable: {err}");
                None
            }
        }
    }
}

impl VolumeStore for SqliteSettings {
    fn load(&self) -> Option<u8> {
        let value = self.open()?.get_setting(KEY_VOLUME).ok()??;
        value.trim().parse::<u8>().ok().filter(|v| *v <= 100)
    }

    fn save(&self, percent: u8) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(KEY_VOLUME, &percent.min(100).to_string()) {
                eprintln!("could not save volume: {err}");
            }
        }
    }
}

impl AutoplayStore for SqliteSettings {
    fn load(&self) -> Option<bool> {
        match self.open()?.get_setting(KEY_AUTOPLAY).ok()??.trim() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    fn save(&self, on: bool) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(KEY_AUTOPLAY, if on { "on" } else { "off" }) {
                eprintln!("could not save autoplay: {err}");
            }
        }
    }
}

impl QueueStore for SqliteSettings {
    fn load(&self, account_key: &str) -> Option<(Queue, u64)> {
        let mut store = self.open()?;
        match store.load_queue(account_key) {
            Ok(saved) => saved,
            Err(err) => {
                // Unreadable is not worth keeping: start from a fresh queue.
                eprintln!("saved queue discarded: {err}");
                let _ = store.purge_private();
                None
            }
        }
    }

    fn save(&self, account_key: &str, snapshot: &QueueSnapshot, position_ms: u64) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.save_queue(account_key, snapshot, position_ms) {
                eprintln!("could not save the queue: {err}");
            }
        }
    }

    fn checkpoint(&self, position_ms: u64) {
        if let Some(store) = self.open() {
            if let Err(err) = store.checkpoint_position(position_ms) {
                eprintln!("could not save the position: {err}");
            }
        }
    }

    fn forget(&self) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.purge_private() {
                eprintln!("could not forget the saved queue: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_round_trips_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteSettings::at(dir.path().join("nested").join("apollo.db"));
        assert_eq!(VolumeStore::load(&store), None);
        VolumeStore::save(&store, 35);
        assert_eq!(VolumeStore::load(&store), Some(35));
        VolumeStore::save(&store, 0);
        assert_eq!(VolumeStore::load(&store), Some(0));
    }

    #[test]
    fn out_of_range_or_garbage_values_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let raw = Store::open(&path).unwrap();
        raw.set_setting(KEY_VOLUME, "250").unwrap();
        assert_eq!(VolumeStore::load(&SqliteSettings::at(path.clone())), None);
        raw.set_setting(KEY_VOLUME, "loud").unwrap();
        assert_eq!(VolumeStore::load(&SqliteSettings::at(path)), None);
    }

    #[test]
    fn autoplay_round_trips_and_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let store = SqliteSettings::at(path.clone());
        assert_eq!(AutoplayStore::load(&store), None);
        AutoplayStore::save(&store, false);
        assert_eq!(AutoplayStore::load(&store), Some(false));
        AutoplayStore::save(&store, true);
        assert_eq!(AutoplayStore::load(&store), Some(true));
        Store::open(&path)
            .unwrap()
            .set_setting(KEY_AUTOPLAY, "maybe")
            .unwrap();
        assert_eq!(AutoplayStore::load(&store), None);
    }

    #[test]
    fn the_queue_round_trips_and_belongs_to_one_account() {
        use apollo_domain::model::Track;
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteSettings::at(dir.path().join("apollo.db"));
        let mut q = Queue::new();
        q.play_context(vec![Track::new("a", "A"), Track::new("b", "B")], 0);
        q.advance(apollo_domain::queue::AdvanceKind::Explicit);
        QueueStore::save(&store, "acct", &q.snapshot(), 12_000);
        QueueStore::checkpoint(&store, 30_000);
        let (restored, pos) = QueueStore::load(&store, "acct").expect("saved");
        assert_eq!(pos, 30_000);
        assert_eq!(restored.snapshot(), q.snapshot());
        // Someone else's queue is never replayed, and is gone afterwards.
        assert!(QueueStore::load(&store, "other").is_none());
        assert!(QueueStore::load(&store, "acct").is_none());
        QueueStore::save(&store, "acct", &q.snapshot(), 1);
        QueueStore::forget(&store);
        assert!(QueueStore::load(&store, "acct").is_none());
    }
}
