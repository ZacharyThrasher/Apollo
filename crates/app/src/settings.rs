//! Settings that live between runs, kept in the `settings` table of the app's SQLite store
//! (non-secret state only).
//!
//! The volume has its own small store ([`VolumeStore`]). Everything else goes through
//! [`SettingsModel`]: a typed accessor the Settings screen and the rest of the app share. A
//! setting is declared once as a [`BoolSetting`] constant (key plus default); the model caches
//! it, notifies observers when it changes, and writes it through on a background thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use apollo_store::Store;
use gpui::Context;

const KEY_VOLUME: &str = "volume_percent";

/// A yes/no setting: the store key and what it is until the owner changes it.
#[derive(Clone, Copy, Debug)]
pub struct BoolSetting {
    pub key: &'static str,
    pub default: bool,
}

impl BoolSetting {
    pub const fn new(key: &'static str, default: bool) -> Self {
        Self { key, default }
    }
}

/// Window > Close to tray: the close button hides the window and playback continues.
pub const CLOSE_TO_TRAY: BoolSetting = BoolSetting::new("close_to_tray", true);
/// Window > Start minimized to tray: the window stays hidden on launch.
pub const START_MINIMIZED_TO_TRAY: BoolSetting = BoolSetting::new("start_minimized_to_tray", false);

/// Where settings are kept. Blocking: the model only calls it off the UI thread for writes.
pub trait SettingsBackend: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
}

/// The `settings` table of the app database.
pub struct SqliteSettings {
    path: PathBuf,
}

impl SqliteSettings {
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// The same database as [`SqliteVolume::default_location`]; `None` leaves persistence off.
    pub fn default_location() -> Option<Self> {
        SqliteVolume::default_location().map(|v| Self::at(v.path))
    }
}

impl SettingsBackend for SqliteSettings {
    fn get(&self, key: &str) -> Option<String> {
        open_store(&self.path)?.get_setting(key).ok()?
    }

    fn set(&self, key: &str, value: &str) {
        if let Some(store) = open_store(&self.path) {
            if let Err(err) = store.set_setting(key, value) {
                eprintln!("could not save setting {key}: {err}");
            }
        }
    }
}

/// `true`/`false` as stored; anything else reads as unset.
fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

/// The app's settings as the UI sees them. Reads come from a cache filled on first use; a change
/// updates the cache at once, tells observers, and is written by a background thread in the
/// order it was made.
pub struct SettingsModel {
    backend: Option<Arc<dyn SettingsBackend>>,
    cache: RefCell<HashMap<&'static str, bool>>,
    writer: Option<Sender<(&'static str, bool)>>,
}

impl SettingsModel {
    /// Without a backend nothing is persisted: every setting stays at its default until changed,
    /// and then only for this run (tests, non-Windows).
    pub fn new(backend: Option<Arc<dyn SettingsBackend>>) -> Self {
        let writer = backend.clone().map(|backend| {
            let (tx, rx) = channel::<(&'static str, bool)>();
            std::thread::spawn(move || {
                for (key, value) in rx {
                    backend.set(key, if value { "true" } else { "false" });
                }
            });
            tx
        });
        Self {
            backend,
            cache: RefCell::new(HashMap::new()),
            writer,
        }
    }

    pub fn get(&self, setting: &BoolSetting) -> bool {
        if let Some(value) = self.cache.borrow().get(setting.key) {
            return *value;
        }
        let stored = self
            .backend
            .as_ref()
            .and_then(|b| b.get(setting.key))
            .and_then(|v| parse_bool(&v));
        let value = stored.unwrap_or(setting.default);
        self.cache.borrow_mut().insert(setting.key, value);
        value
    }

    pub fn set(&mut self, setting: &BoolSetting, value: bool, cx: &mut Context<Self>) {
        if self.get(setting) == value {
            return;
        }
        self.cache.borrow_mut().insert(setting.key, value);
        if let Some(writer) = &self.writer {
            let _ = writer.send((setting.key, value));
        }
        cx.notify();
    }
}

fn open_store(path: &Path) -> Option<Store> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    match Store::open(path) {
        Ok(store) => Some(store),
        Err(err) => {
            eprintln!("settings store unavailable: {err}");
            None
        }
    }
}

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
        open_store(&self.path)
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

    #[test]
    fn bool_settings_start_at_their_default_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let backend = Arc::new(SqliteSettings::at(path));
        assert_eq!(backend.get(CLOSE_TO_TRAY.key), None);
        backend.set(CLOSE_TO_TRAY.key, "false");
        assert_eq!(backend.get(CLOSE_TO_TRAY.key).as_deref(), Some("false"));
        let model = SettingsModel::new(Some(backend));
        assert!(!model.get(&CLOSE_TO_TRAY));
        assert!(!model.get(&START_MINIMIZED_TO_TRAY));
    }

    #[test]
    fn unreadable_values_fall_back_to_the_default() {
        assert_eq!(parse_bool("true"), Some(true));
        assert_eq!(parse_bool("0"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteSettings::at(dir.path().join("apollo.db")));
        backend.set(CLOSE_TO_TRAY.key, "maybe");
        assert!(SettingsModel::new(Some(backend)).get(&CLOSE_TO_TRAY));
    }
}
