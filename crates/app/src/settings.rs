//! What lives between runs, in the app's SQLite store (non-secret state only): the volume, the
//! autoplay, volume-normalization and lyrics switches (all in the `settings` table) and the queue
//! with its position.
//!
//! The Settings screen's switches go through [`SettingsModel`]: a typed accessor the screen and
//! the rest of the app share. A setting is declared once as a [`BoolSetting`] (or, for a value
//! that is text, a [`TextSetting`]) constant (key plus default); the model caches it, notifies
//! observers when it changes, and writes it through on a background thread.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;

use apollo_domain::audio::Equalizer;
use apollo_domain::queue::{Queue, QueueSnapshot};
use apollo_store::Store;
use gpui::Context;

const KEY_VOLUME: &str = "volume_percent";
const KEY_AUTOPLAY: &str = "autoplay";
const KEY_NORMALIZE: &str = "normalize_volume";
const KEY_GAPLESS: &str = "gapless_playback";
const KEY_LYRICS_OPEN: &str = "lyrics_open";

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

/// A text setting: the store key and what it is until the owner changes it.
#[derive(Clone, Copy, Debug)]
pub struct TextSetting {
    pub key: &'static str,
    pub default: &'static str,
}

impl TextSetting {
    pub const fn new(key: &'static str, default: &'static str) -> Self {
        Self { key, default }
    }
}

/// Window > Close to tray: the close button hides the window and playback continues.
pub const CLOSE_TO_TRAY: BoolSetting = BoolSetting::new("close_to_tray", true);
/// Window > Start minimized to tray: the window stays hidden on launch.
pub const START_MINIMIZED_TO_TRAY: BoolSetting = BoolSetting::new("start_minimized_to_tray", false);

/// Audio > Equalizer: the switch. Off until the owner turns it on.
pub const EQUALIZER_ENABLED: BoolSetting = BoolSetting::new("equalizer_enabled", false);
/// Audio > Equalizer: the ten band gains in dB, comma separated (see `Equalizer::gains_to_text`).
/// Empty is flat. Kept while the switch is off.
pub const EQUALIZER_GAINS: TextSetting = TextSetting::new("equalizer_gains", "");
/// Audio > Output device: the id of the device chosen (mpv's device name). Empty is "System
/// default". Kept while the device is unplugged, so it is used again when it returns.
pub const AUDIO_DEVICE: TextSetting = TextSetting::new("audio_device", "");

/// Where settings are kept. Blocking: the model only calls it off the UI thread for writes.
pub trait SettingsBackend: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str);
}

impl SettingsBackend for SqliteSettings {
    fn get(&self, key: &str) -> Option<String> {
        self.open()?.get_setting(key).ok()?
    }

    fn set(&self, key: &str, value: &str) {
        if let Some(store) = self.open() {
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
    text: RefCell<HashMap<&'static str, String>>,
    writer: Option<Sender<(&'static str, String)>>,
}

impl SettingsModel {
    /// Without a backend nothing is persisted: every setting stays at its default until changed,
    /// and then only for this run (tests, non-Windows).
    pub fn new(backend: Option<Arc<dyn SettingsBackend>>) -> Self {
        let writer = backend.clone().map(|backend| {
            let (tx, rx) = channel::<(&'static str, String)>();
            std::thread::spawn(move || {
                for (key, value) in rx {
                    backend.set(key, &value);
                }
            });
            tx
        });
        Self {
            backend,
            cache: RefCell::new(HashMap::new()),
            text: RefCell::new(HashMap::new()),
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
            let _ = writer.send((setting.key, if value { "true" } else { "false" }.to_owned()));
        }
        cx.notify();
    }

    pub fn get_text(&self, setting: &TextSetting) -> String {
        if let Some(value) = self.text.borrow().get(setting.key) {
            return value.clone();
        }
        let value = self
            .backend
            .as_ref()
            .and_then(|b| b.get(setting.key))
            .unwrap_or_else(|| setting.default.to_owned());
        self.text
            .borrow_mut()
            .insert(setting.key, value.clone());
        value
    }

    pub fn set_text(&mut self, setting: &TextSetting, value: &str, cx: &mut Context<Self>) {
        if self.get_text(setting) == value {
            return;
        }
        self.text.borrow_mut().insert(setting.key, value.to_owned());
        if let Some(writer) = &self.writer {
            let _ = writer.send((setting.key, value.to_owned()));
        }
        cx.notify();
    }

    /// The equalizer as saved: the switch and the curve. A damaged curve reads as flat.
    pub fn equalizer(&self) -> Equalizer {
        Equalizer {
            enabled: self.get(&EQUALIZER_ENABLED),
            gains_db: Equalizer::gains_from_text(&self.get_text(&EQUALIZER_GAINS))
                .unwrap_or_default(),
        }
    }

    /// The output device chosen, `None` for the system default.
    pub fn audio_device(&self) -> Option<String> {
        Some(self.get_text(&AUDIO_DEVICE)).filter(|id| !id.is_empty())
    }
}

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

/// Whether volume normalization is on. Blocking: call off the UI thread.
pub trait NormalizeStore: Send + Sync {
    fn load(&self) -> Option<bool>;
    fn save(&self, on: bool);
}

/// Whether Now Playing shows lyrics where the queue would be. Blocking: call off the UI thread.
pub trait LyricsOpenStore: Send + Sync {
    fn load(&self) -> Option<bool>;
    fn save(&self, on: bool);
}

/// Whether gapless playback is on. Blocking: call off the UI thread.
pub trait GaplessStore: Send + Sync {
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

    /// The database file (the lyrics cache lives in the same store).
    pub fn path(&self) -> &std::path::Path {
        &self.path
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

impl NormalizeStore for SqliteSettings {
    fn load(&self) -> Option<bool> {
        match self.open()?.get_setting(KEY_NORMALIZE).ok()??.trim() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    fn save(&self, on: bool) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(KEY_NORMALIZE, if on { "on" } else { "off" }) {
                eprintln!("could not save volume normalization: {err}");
            }
        }
    }
}

impl LyricsOpenStore for SqliteSettings {
    fn load(&self) -> Option<bool> {
        match self.open()?.get_setting(KEY_LYRICS_OPEN).ok()??.trim() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    fn save(&self, on: bool) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(KEY_LYRICS_OPEN, if on { "on" } else { "off" }) {
                eprintln!("could not save the lyrics choice: {err}");
            }
        }
    }
}

impl GaplessStore for SqliteSettings {
    fn load(&self) -> Option<bool> {
        match self.open()?.get_setting(KEY_GAPLESS).ok()??.trim() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    fn save(&self, on: bool) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(KEY_GAPLESS, if on { "on" } else { "off" }) {
                eprintln!("could not save gapless playback: {err}");
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
    fn normalization_round_trips_and_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let store = SqliteSettings::at(path.clone());
        assert_eq!(NormalizeStore::load(&store), None);
        NormalizeStore::save(&store, false);
        assert_eq!(NormalizeStore::load(&store), Some(false));
        NormalizeStore::save(&store, true);
        assert_eq!(NormalizeStore::load(&store), Some(true));
        Store::open(&path)
            .unwrap()
            .set_setting(KEY_NORMALIZE, "maybe")
            .unwrap();
        assert_eq!(NormalizeStore::load(&store), None);
    }

    #[test]
    fn the_lyrics_choice_round_trips_and_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let store = SqliteSettings::at(path.clone());
        assert_eq!(LyricsOpenStore::load(&store), None);
        LyricsOpenStore::save(&store, true);
        assert_eq!(LyricsOpenStore::load(&store), Some(true));
        LyricsOpenStore::save(&store, false);
        assert_eq!(LyricsOpenStore::load(&store), Some(false));
        Store::open(&path)
            .unwrap()
            .set_setting(KEY_LYRICS_OPEN, "maybe")
            .unwrap();
        assert_eq!(LyricsOpenStore::load(&store), None);
    }

    #[test]
    fn gapless_round_trips_and_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let store = SqliteSettings::at(path.clone());
        assert_eq!(GaplessStore::load(&store), None);
        GaplessStore::save(&store, false);
        assert_eq!(GaplessStore::load(&store), Some(false));
        GaplessStore::save(&store, true);
        assert_eq!(GaplessStore::load(&store), Some(true));
        Store::open(&path)
            .unwrap()
            .set_setting(KEY_GAPLESS, "maybe")
            .unwrap();
        assert_eq!(GaplessStore::load(&store), None);
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

    #[test]
    fn text_settings_start_at_their_default_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteSettings::at(dir.path().join("apollo.db")));
        let model = SettingsModel::new(Some(backend.clone()));
        assert_eq!(model.get_text(&AUDIO_DEVICE), "");
        assert_eq!(model.audio_device(), None);
        backend.set(AUDIO_DEVICE.key, "wasapi/{abc}");
        let model = SettingsModel::new(Some(backend));
        assert_eq!(model.audio_device().as_deref(), Some("wasapi/{abc}"));
    }

    #[test]
    fn the_equalizer_reads_as_off_and_flat_until_saved_and_damage_reads_as_flat() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteSettings::at(dir.path().join("apollo.db")));
        let model = SettingsModel::new(Some(backend.clone()));
        assert_eq!(model.equalizer(), Equalizer::flat());
        backend.set(EQUALIZER_ENABLED.key, "true");
        backend.set(EQUALIZER_GAINS.key, "6,5,4,2,0,0,0,0,0,0");
        let model = SettingsModel::new(Some(backend.clone()));
        let eq = model.equalizer();
        assert!(eq.enabled);
        assert_eq!(eq.gains_db[0], 6.0);
        backend.set(EQUALIZER_GAINS.key, "6,5,oops");
        let model = SettingsModel::new(Some(backend));
        assert_eq!(model.equalizer().gains_db, [0.0; 10]);
    }
}
