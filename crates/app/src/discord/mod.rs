//! Discord Rich Presence: what is playing, shown in the Discord client as "Listening to Apollo"
//! with a progress bar. Off by default (Settings > Discord > Show what I'm playing).
//!
//! Everything goes through the Discord client's local IPC pipe ([`ipc`]); the app sends nothing
//! over the network for this. Discord itself fetches the album art from the URL given in the
//! activity.
//!
//! [`PresenceBridge`] turns the player's state into as few calls as will keep the presence right
//! (a new activity on track change, play, or seek; a clear on pause, stop, or when the setting is
//! off). The pipe worker in [`ipc`] adds Discord's rate limit and the quiet reconnects.
//!
//! The bridge is pure, so its decisions are tested anywhere; [`install`] wires it to the player.

pub mod ipc;
pub mod transport;

use std::time::{Duration, Instant};

use gpui::{App, AppContext, Entity};
use serde_json::{json, Map, Value};

use crate::os_media::{MediaState, MediaTrack};
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, DISCORD_PRESENCE};

/// The Discord application this presence belongs to. It names the activity ("Listening to
/// Apollo"), so it must be an application called "Apollo" created at
/// <https://discord.com/developers/applications>; the Application ID on its General Information
/// page is pasted here. Empty until then, which leaves the feature inert.
///
/// For trying one out without rebuilding, `APOLLO_DISCORD_APP_ID` in the environment wins.
pub const APPLICATION_ID: &str = "";

fn application_id() -> Option<String> {
    let from_env = std::env::var("APOLLO_DISCORD_APP_ID").ok();
    let id = from_env.as_deref().unwrap_or(APPLICATION_ID).trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// The one button on the presence.
pub const BUTTON_LABEL: &str = "Play on YouTube Music";
/// Discord activity type 2: "Listening to".
const TYPE_LISTENING: u8 = 2;
/// Discord rejects text shorter than two characters, and anything past 128.
const MIN_TEXT: usize = 2;
const MAX_TEXT: usize = 120;
/// Discord rejects image URLs past 256 characters.
const MAX_IMAGE_URL: usize = 256;
/// How far the position may stray from the extrapolation before the progress bar is restated: a
/// seek, a stall, a buffering gap.
const DRIFT_MS: u64 = 1500;
/// How long quitting waits for the presence to be cleared.
const QUIT_WAIT: Duration = Duration::from_millis(500);

/// What the Discord client is asked to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Activity {
    pub details: String,
    pub state: Option<String>,
    pub large_image: Option<String>,
    pub large_text: Option<String>,
    /// Unix seconds. With `end`, Discord draws the progress bar.
    pub start: u64,
    pub end: Option<u64>,
    pub button_url: Option<String>,
}

impl Activity {
    pub fn to_json(&self) -> Value {
        let mut timestamps = Map::new();
        timestamps.insert("start".into(), json!(self.start));
        if let Some(end) = self.end {
            timestamps.insert("end".into(), json!(end));
        }
        let mut activity = Map::new();
        activity.insert("type".into(), json!(TYPE_LISTENING));
        activity.insert("details".into(), json!(self.details));
        if let Some(state) = &self.state {
            activity.insert("state".into(), json!(state));
        }
        activity.insert("timestamps".into(), Value::Object(timestamps));
        let mut assets = Map::new();
        if let Some(image) = &self.large_image {
            assets.insert("large_image".into(), json!(image));
        }
        if let Some(text) = &self.large_text {
            assets.insert("large_text".into(), json!(text));
        }
        if !assets.is_empty() {
            activity.insert("assets".into(), Value::Object(assets));
        }
        if let Some(url) = &self.button_url {
            activity.insert(
                "buttons".into(),
                json!([{ "label": BUTTON_LABEL, "url": url }]),
            );
        }
        Value::Object(activity)
    }

    fn of(track: &MediaTrack, art_url: Option<&str>, position_ms: u64, unix_ms: u64) -> Self {
        let start = unix_ms.saturating_sub(position_ms) / 1000;
        Self {
            details: text_field(&track.title).unwrap_or_else(|| "Unknown".to_owned()),
            state: text_field(&track.artist),
            large_image: art_url
                .filter(|u| u.starts_with("https://") && u.len() <= MAX_IMAGE_URL)
                .map(str::to_owned),
            large_text: text_field(&track.album),
            start,
            end: track
                .duration_ms
                .filter(|d| *d > 0)
                .map(|d| start + d.div_ceil(1000)),
            button_url: watch_url(&track.video_id),
        }
    }
}

/// A line of presence text as Discord wants it: 2 to 128 characters, nothing for blank.
fn text_field(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut text: String = trimmed.chars().take(MAX_TEXT).collect();
    if trimmed.chars().count() > MAX_TEXT {
        text.push('…');
    }
    if text.chars().count() < MIN_TEXT {
        // A zero-width space makes a one-character title long enough without showing.
        text.push('\u{200b}');
    }
    Some(text)
}

/// The track's page on YouTube Music. Video ids are URL-safe; anything else gets no button.
fn watch_url(video_id: &str) -> Option<String> {
    let safe = !video_id.is_empty()
        && video_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    safe.then(|| format!("https://music.youtube.com/watch?v={video_id}"))
}

/// Where the bridge sends its decisions: the worker, or a recorder in tests.
pub trait PresenceSink {
    /// Show this activity, or nothing for `None`.
    fn set(&mut self, activity: Option<Activity>);
    /// The feature is off: clear the presence and let go of the Discord client.
    fn release(&mut self);
}

impl PresenceSink for ipc::Handle {
    fn set(&mut self, activity: Option<Activity>) {
        ipc::Handle::set(self, activity);
    }

    fn release(&mut self) {
        ipc::Handle::release(self);
    }
}

/// What the last call said; a different value means a new call.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    video_id: String,
    title: String,
    artist: String,
    album: String,
    duration_ms: Option<u64>,
    art_url: Option<String>,
}

enum Last {
    /// Nothing sent since the feature was last off: no connection is wanted.
    Untouched,
    Cleared,
    Showing {
        key: Key,
        position_ms: u64,
        at: Instant,
    },
}

pub struct PresenceBridge<S: PresenceSink> {
    sink: S,
    last: Last,
}

impl<S: PresenceSink> PresenceBridge<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            last: Last::Untouched,
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// Reconcile the presence with `state`. Cheap when nothing changed; call on every player
    /// or settings update. `unix_ms` is the wall clock, for the progress bar's timestamps.
    pub fn apply(
        &mut self,
        enabled: bool,
        state: &MediaState,
        art_url: Option<&str>,
        now: Instant,
        unix_ms: u64,
    ) {
        if !enabled {
            if !matches!(self.last, Last::Untouched) {
                self.sink.release();
                self.last = Last::Untouched;
            }
            return;
        }
        let track = state.track.as_ref().filter(|_| state.playing);
        let Some(track) = track else {
            if matches!(self.last, Last::Showing { .. }) {
                self.sink.set(None);
                self.last = Last::Cleared;
            }
            return;
        };
        let key = Key {
            video_id: track.video_id.clone(),
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration_ms: track.duration_ms,
            art_url: art_url.map(str::to_owned),
        };
        let unchanged = match &self.last {
            Last::Showing {
                key: shown,
                position_ms,
                at,
            } => {
                let expected = position_ms + now.saturating_duration_since(*at).as_millis() as u64;
                *shown == key && expected.abs_diff(state.position_ms) <= DRIFT_MS
            }
            _ => false,
        };
        if unchanged {
            return;
        }
        self.sink.set(Some(Activity::of(
            track,
            art_url,
            state.position_ms,
            unix_ms,
        )));
        self.last = Last::Showing {
            key,
            position_ms: state.position_ms,
            at: now,
        };
    }
}

struct Host {
    bridge: PresenceBridge<ipc::Handle>,
}

impl Host {
    fn sync(&mut self, player: &Entity<PlayerModel>, settings: &Entity<SettingsModel>, cx: &App) {
        let enabled = settings.read(cx).get(&DISCORD_PRESENCE);
        let player = player.read(cx);
        let state = player.media_state();
        let art_url = state
            .track
            .as_ref()
            .and_then(|t| player.art_url(&t.video_id));
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        self.bridge
            .apply(enabled, &state, art_url.as_deref(), Instant::now(), unix_ms);
    }
}

/// Hooks the presence to `player` and the Settings switch for the life of the app. Does nothing
/// until [`APPLICATION_ID`] is set.
pub fn install(cx: &mut App, player: Entity<PlayerModel>, settings: Entity<SettingsModel>) {
    let Some(id) = application_id() else {
        return;
    };
    let handle = ipc::Handle::spawn(
        id,
        Box::new(transport::connect),
        ipc::Timing::default(),
    );
    let host = cx.new(|cx| {
        let (p, s) = (player.clone(), settings.clone());
        cx.observe(&player, move |host: &mut Host, _, cx| host.sync(&p, &s, cx))
            .detach();
        let (p, s) = (player.clone(), settings.clone());
        cx.observe(&settings, move |host: &mut Host, _, cx| host.sync(&p, &s, cx))
            .detach();
        Host {
            bridge: PresenceBridge::new(handle),
        }
    });
    host.update(cx, |host, cx| host.sync(&player, &settings, cx));
    // Quitting must not leave the presence behind.
    cx.on_app_quit(move |cx| {
        host.update(cx, |host, _| host.bridge.sink().shutdown(QUIT_WAIT));
        async {}
    })
    .detach();
}

#[cfg(test)]
mod tests;
