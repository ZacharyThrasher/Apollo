//! Settings that live between runs. Only the volume so far, kept in the `settings` table of
//! the app's SQLite store (non-secret state only).

use std::path::PathBuf;

use apollo_store::Store;

const KEY_VOLUME: &str = "volume_percent";

/// Where the volume is kept. Blocking: call off the UI thread.
pub trait VolumeStore: Send + Sync {
    fn load(&self) -> Option<u8>;
    fn save(&self, percent: u8);
}

pub struct SqliteVolume {
    path: PathBuf,
}

impl SqliteVolume {
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

impl VolumeStore for SqliteVolume {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_round_trips_through_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteVolume::at(dir.path().join("nested").join("apollo.db"));
        assert_eq!(store.load(), None);
        store.save(35);
        assert_eq!(store.load(), Some(35));
        store.save(0);
        assert_eq!(store.load(), Some(0));
    }

    #[test]
    fn out_of_range_or_garbage_values_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let raw = Store::open(&path).unwrap();
        raw.set_setting(KEY_VOLUME, "250").unwrap();
        assert_eq!(SqliteVolume::at(path.clone()).load(), None);
        raw.set_setting(KEY_VOLUME, "loud").unwrap();
        assert_eq!(SqliteVolume::at(path).load(), None);
    }
}
