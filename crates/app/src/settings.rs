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
const KEY_RELATED_OPEN: &str = "related_open";

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

/// Playback > Add plays to history: Apollo tells YouTube Music what was played, as its web client
/// does, so plays show in History and keep shaping Home and mixes. Off sends nothing at all.
pub const ADD_PLAYS_TO_HISTORY: BoolSetting = BoolSetting::new("add_plays_to_history", true);
/// Playback > Preload songs on hover: resolve a song ahead of the click when the pointer or the
/// keyboard rests on it, and the first song of a page when it opens. Off (the default), only the
/// queue's own lookahead and Skim's one-ahead resolve anything ahead of a click: every ahead-of-
/// time resolve is about six requests to YouTube, and hover is the one trigger that has no limit
/// of its own (see ADR 0005).
pub const PRELOAD_ON_HOVER: BoolSetting = BoolSetting::new("preload_on_hover", false);
/// Home > Personal shelves: "On repeat" and "Forgotten favourites" from Apollo's own play log sit
/// among Home's shelves. Off hides them; the log itself is kept either way.
pub const PERSONAL_SHELVES: BoolSetting = BoolSetting::new("personal_shelves", true);
/// Home > Mood chips: the row of mood chips ("Relax", "Workout") under Home's title, which filter
/// the feed the way YouTube Music's do. Off hides the row and shows the ordinary feed.
pub const MOOD_CHIPS: BoolSetting = BoolSetting::new("mood_chips", true);
/// Playback > Keep picker open after adding: the add-to-playlist picker stays open once songs are
/// added, so one song can go into several playlists in one visit. Off closes it after the add.
pub const PICKER_STAY_OPEN: BoolSetting = BoolSetting::new("picker_stay_open", false);
/// Playback > P opens the playlist picker: the P key opens the picker for the song under the
/// cursor (or the playing one). Off leaves P unbound.
pub const PICKER_KEY: BoolSetting = BoolSetting::new("picker_key", true);
/// The playlists most recently added to through the picker, newest first, as bare ids separated
/// by commas. Shown on top of the picker. Kept per app, not per account (an id of another
/// account's playlist is simply never found in the list).
pub const RECENT_PLAYLISTS: TextSetting = TextSetting::new("picker_recent_playlists", "");
/// Playlists > A adds to the target playlist: the A key adds the playing song (or the song under
/// the cursor, or every picked song) to the target playlist; with no target it opens the picker
/// to choose one. Off leaves A unbound.
pub const TARGET_KEY: BoolSetting = BoolSetting::new("target_key", true);
/// Playlists > Show target playlist in player bar: the small pill with the target's name. Off
/// hides it (the target and the A key stay).
pub const TARGET_PILL: BoolSetting = BoolSetting::new("target_pill", true);
/// The target playlist, as `account <TAB> playlist id <TAB> name` (see `target_playlist`). Empty
/// is none. Not in the diagnostics report: it carries a playlist's name.
pub const TARGET_PLAYLIST: TextSetting = TextSetting::new("target_playlist", "");
/// Playlists > Show More like this: a playlist of your own ends with a short list of songs like
/// its own, each with a + that adds it in place. Off hides the section and fetches nothing.
pub const MORE_LIKE_THIS: BoolSetting = BoolSetting::new("more_like_this", true);
/// Playlists > Confirm when adding: say so when something is added to a list, with the pill and
/// a short pulse where the list is on screen (see `confirm`).
pub const CONFIRM_ADDS: BoolSetting = BoolSetting::new("confirm_adds", true);
/// Playlists > Create playlists instantly: "New playlist" and "Save as playlist…" skip the name
/// dialog, make a private "Oct 5 · Untitled" at once and open it with its title ready to rename.
/// Off until the owner asks for it.
pub const INSTANT_PLAYLISTS: BoolSetting = BoolSetting::new("instant_playlists", false);
/// Playlists > Move to the next song after C, A or K: after C, A or K acts on a list's cursor
/// row (Search, Library, queue, playlist, album, Crate) the cursor steps to the next row; Skim's
/// K goes on to the next song. Picked songs keep the cursor where it is. Off leaves it be.
pub const AUTO_ADVANCE: BoolSetting = BoolSetting::new("auto_advance", true);
/// Playlists > Take songs out of the Crate once filed: a song the service confirms is in a playlist
/// (A, the P picker, "Add all to…") leaves the Crate. Ctrl+Z puts it back. Off: a song can belong
/// in two playlists, so nothing leaves unasked.
pub const CRATE_FILE_AND_FORGET: BoolSetting = BoolSetting::new("crate_file_and_forget", false);
/// Playlists > Prefer explicit versions: a clean song added to the Crate is swapped for its
/// explicit version at once, when the service has one. Off until the owner asks for it.
pub const PREFER_EXPLICIT: BoolSetting = BoolSetting::new("prefer_explicit", false);
/// Window > Close to tray: the close button hides the window and playback continues.
pub const CLOSE_TO_TRAY: BoolSetting = BoolSetting::new("close_to_tray", true);
/// Window > Start minimized to tray: the window stays hidden on launch.
pub const START_MINIMIZED_TO_TRAY: BoolSetting = BoolSetting::new("start_minimized_to_tray", false);
/// Playback > Pause when other audio plays: another app that keeps playing sound for a couple of
/// seconds (a movie, a call) pauses the music. Off until the owner asks for it.
pub const PAUSE_ON_OTHER_AUDIO: BoolSetting = BoolSetting::new("pause_on_other_audio", false);

/// Playback > Skim: the Skim button on playlist, album and Explore lists, which plays each song
/// for a few seconds from a third of the way in. Off hides the button (and ends a skim).
pub const SKIM: BoolSetting = BoolSetting::new("skim", true);
/// Playback > Seconds per song: how long Skim stays on each song (10, 20 or 30).
pub const SKIM_SECONDS: TextSetting = TextSetting::new("skim_seconds", "20");

/// Mini player > Keep on top: the mini player floats above other windows.
pub const MINI_PLAYER_ON_TOP: BoolSetting = BoolSetting::new("mini_player_on_top", true);

/// Discord > Show what I'm playing: the track shows as Rich Presence in the Discord client.
pub const DISCORD_PRESENCE: BoolSetting = BoolSetting::new("discord_presence", false);

/// Keyboard > Global hotkeys: Ctrl+Alt shortcuts (play/pause, next, previous, volume) that work
/// while Apollo is not focused.
pub const GLOBAL_HOTKEYS: BoolSetting = BoolSetting::new("global_hotkeys", false);

/// Keyboard > Show shortcut hints: tooltips and menu rows name their key, and the third time a
/// command is done with the mouse a one-line tip names it (once ever per command).
pub const SHOW_SHORTCUT_HINTS: BoolSetting = BoolSetting::new("show_shortcut_hints", true);
/// Keyboard > Hold Ctrl for key badges: holding Ctrl for a moment shows each button's key on it.
pub const KEY_BADGES: BoolSetting = BoolSetting::new("key_badges", true);
/// How often each command was done with the mouse and whether its tip was shown
/// (`shortcuts::HintLedger`). Not a switch and not worth a report line.
pub const SHORTCUT_HINT_STATE: TextSetting = TextSetting::new("shortcut_hint_state", "");

/// Mini player > HUD mode: the mini player becomes an always-on-top, click-through
/// overlay that never takes the keyboard (for watching and controlling music over a game).
pub const MINI_PLAYER_HUD: BoolSetting = BoolSetting::new("mini_player_hud", false);
/// Mini player > Auto-hide HUD: the HUD dims after a few seconds without a track change.
pub const HUD_AUTO_HIDE: BoolSetting = BoolSetting::new("hud_auto_hide", true);
/// Mini player > Fade controls when idle: after a few seconds without the pointer, the transport
/// buttons and the volume slider fade out.
pub const MINI_FADE_CONTROLS: BoolSetting = BoolSetting::new("mini_fade_controls", true);
/// Mini player > Dim when idle: after the same wait, the whole window dims.
pub const MINI_DIM: BoolSetting = BoolSetting::new("mini_dim", true);
/// Mini player > Show volume: the volume slider, and the mouse wheel changing the volume.
pub const MINI_SHOW_VOLUME: BoolSetting = BoolSetting::new("mini_show_volume", true);
/// Mini player > Show lyrics: the current synced lyric line under the artist (also in the HUD).
pub const MINI_SHOW_LYRICS: BoolSetting = BoolSetting::new("mini_show_lyrics", false);
/// Mini player > Collapse when Apollo loses focus: a main window expanded from the mini player
/// goes back to it after a short grace period once Apollo is no longer the active app.
pub const MINI_COLLAPSE_ON_BLUR: BoolSetting = BoolSetting::new("mini_collapse_on_blur", true);

/// Audio > Equalizer: the switch. Off until the owner turns it on.
pub const EQUALIZER_ENABLED: BoolSetting = BoolSetting::new("equalizer_enabled", false);
/// Audio > Equalizer: the ten band gains in dB, comma separated (see `Equalizer::gains_to_text`).
/// Empty is flat. Kept while the switch is off.
pub const EQUALIZER_GAINS: TextSetting = TextSetting::new("equalizer_gains", "");
/// Audio > Output device: the id of the device chosen (mpv's device name). Empty is "System
/// default". Kept while the device is unplugged, so it is used again when it returns.
pub const AUDIO_DEVICE: TextSetting = TextSetting::new("audio_device", "");

/// Data > Export library: the format the save dialog proposes (`json` or `csv`, see
/// `export::ExportFormat`).
pub const EXPORT_FORMAT: TextSetting = TextSetting::new("export_format", "json");

/// The settings keys the diagnostics report lists, with their values. A key that is not here is
/// not in the report: the table also holds the recent searches, the saved queue and window
/// positions, which a report has no business carrying. A new setting goes here when it helps
/// to know its value.
pub const REPORTED_KEYS: &[&str] = &[
    KEY_VOLUME,
    KEY_AUTOPLAY,
    KEY_NORMALIZE,
    KEY_GAPLESS,
    KEY_LYRICS_OPEN,
    KEY_RELATED_OPEN,
    ADD_PLAYS_TO_HISTORY.key,
    PRELOAD_ON_HOVER.key,
    PERSONAL_SHELVES.key,
    PICKER_STAY_OPEN.key,
    PICKER_KEY.key,
    TARGET_KEY.key,
    TARGET_PILL.key,
    MORE_LIKE_THIS.key,
    CONFIRM_ADDS.key,
    INSTANT_PLAYLISTS.key,
    AUTO_ADVANCE.key,
    CRATE_FILE_AND_FORGET.key,
    PREFER_EXPLICIT.key,
    CLOSE_TO_TRAY.key,
    START_MINIMIZED_TO_TRAY.key,
    PAUSE_ON_OTHER_AUDIO.key,
    SKIM.key,
    SKIM_SECONDS.key,
    MINI_PLAYER_ON_TOP.key,
    DISCORD_PRESENCE.key,
    GLOBAL_HOTKEYS.key,
    SHOW_SHORTCUT_HINTS.key,
    KEY_BADGES.key,
    MINI_PLAYER_HUD.key,
    HUD_AUTO_HIDE.key,
    MINI_FADE_CONTROLS.key,
    MINI_DIM.key,
    MINI_SHOW_VOLUME.key,
    MINI_SHOW_LYRICS.key,
    MINI_COLLAPSE_ON_BLUR.key,
    EQUALIZER_ENABLED.key,
    EQUALIZER_GAINS.key,
    AUDIO_DEVICE.key,
    EXPORT_FORMAT.key,
    crate::search::SAVE_RECENT_SEARCHES.key,
    crate::search::HIDE_MUSIC_VIDEOS.key,
];

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

    /// Where the settings are kept, for state that is not a switch (the recent searches).
    pub fn backend(&self) -> Option<Arc<dyn SettingsBackend>> {
        self.backend.clone()
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

    /// Seconds per song for Skim: one of the lengths offered, the default when the stored value
    /// is anything else.
    pub fn skim_seconds(&self) -> u32 {
        self.get_text(&SKIM_SECONDS)
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|s| crate::player::SKIM_SECONDS_CHOICES.contains(s))
            .unwrap_or(crate::player::SKIM_DEFAULT_SECONDS)
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

/// Whether Now Playing shows the Related tab where the queue would be. Blocking: call off the UI
/// thread.
pub trait RelatedOpenStore: Send + Sync {
    fn load(&self) -> Option<bool>;
    fn save(&self, on: bool);
}

/// How each Library tab is sorted, by tab name (`playlists`, `albums`, `artists`) and sort name
/// (`recent`, `title`, `artist`). Blocking: call off the UI thread.
pub trait LibrarySortStore: Send + Sync {
    fn load(&self, tab: &str) -> Option<String>;
    fn save(&self, tab: &str, sort: &str);
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

    pub(crate) fn open(&self) -> Option<Store> {
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

impl RelatedOpenStore for SqliteSettings {
    fn load(&self) -> Option<bool> {
        match self.open()?.get_setting(KEY_RELATED_OPEN).ok()??.trim() {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    fn save(&self, on: bool) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(KEY_RELATED_OPEN, if on { "on" } else { "off" }) {
                eprintln!("could not save the related choice: {err}");
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

fn library_sort_key(tab: &str) -> String {
    format!("library_sort_{tab}")
}

impl LibrarySortStore for SqliteSettings {
    fn load(&self, tab: &str) -> Option<String> {
        self.open()?.get_setting(&library_sort_key(tab)).ok()?
    }

    fn save(&self, tab: &str, sort: &str) {
        if let Some(store) = self.open() {
            if let Err(err) = store.set_setting(&library_sort_key(tab), sort) {
                eprintln!("could not save the library sort: {err}");
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
    fn the_related_choice_round_trips_and_garbage_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let store = SqliteSettings::at(path.clone());
        assert_eq!(RelatedOpenStore::load(&store), None);
        RelatedOpenStore::save(&store, true);
        assert_eq!(RelatedOpenStore::load(&store), Some(true));
        RelatedOpenStore::save(&store, false);
        assert_eq!(RelatedOpenStore::load(&store), Some(false));
        Store::open(&path)
            .unwrap()
            .set_setting(KEY_RELATED_OPEN, "maybe")
            .unwrap();
        assert_eq!(RelatedOpenStore::load(&store), None);
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
        assert!(!model.get(&PAUSE_ON_OTHER_AUDIO), "off until asked for");
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

    #[test]
    fn skim_seconds_is_one_of_the_offered_lengths_and_twenty_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(SqliteSettings::at(dir.path().join("apollo.db")));
        let model = SettingsModel::new(Some(backend.clone()));
        assert_eq!(model.skim_seconds(), 20);
        assert!(model.get(&SKIM));
        backend.set(SKIM_SECONDS.key, "30");
        assert_eq!(SettingsModel::new(Some(backend.clone())).skim_seconds(), 30);
        backend.set(SKIM_SECONDS.key, "7");
        assert_eq!(SettingsModel::new(Some(backend.clone())).skim_seconds(), 20);
        backend.set(SKIM_SECONDS.key, "soon");
        assert_eq!(SettingsModel::new(Some(backend)).skim_seconds(), 20);
    }
}
