//! The operating system's media surface (Windows media keys and the System Media Transport
//! Controls overlay), kept apart from the window system so the decisions are testable anywhere.
//!
//! Two directions:
//!
//! * **State out.** [`MediaBridge`] turns the player's [`MediaState`] into as few calls on an
//!   [`OsMediaSink`] as will keep the overlay right: metadata only when the track (or its
//!   artwork, or its length) changes, playback only when the status changes or the position
//!   jumps. The overlay extrapolates the position while playing, so there is no per-tick traffic.
//! * **Commands in.** [`OsCommand`] is what the media keys and the overlay ask for. They are
//!   applied by `PlayerModel::os_command`, so they drive the same coordinator as the buttons.
//!
//! The Windows sink is `smtc.rs` (Souvlaki on GPUI's own window handle).

use std::sync::Arc;
use std::time::{Duration, Instant};

/// How far the reported position may drift from the overlay's own extrapolation before it is
/// corrected: a seek, a stall, a buffering gap.
const DRIFT_MS: u64 = 1500;
/// While playing, the position is restated at least this often so a slow drift cannot build up.
const RESYNC: Duration = Duration::from_secs(10);

/// A request from the media keys or the overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsCommand {
    /// Play and Pause are distinct on purpose: the OS decides which one a play/pause key means
    /// from the status the app last reported, so a toggle here would make a stale key do the
    /// opposite of what was asked.
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// The overlay's scrubber.
    SeekTo(u64),
    /// Fast-forward / rewind buttons, in milliseconds (negative is back).
    SeekBy(i64),
}

/// The track the overlay describes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaTrack {
    pub video_id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: Option<u64>,
}

/// Everything the overlay shows, copied out of the player.
#[derive(Clone, Debug, Default)]
pub struct MediaState {
    pub track: Option<MediaTrack>,
    /// The user wants audio (what the play/pause button shows), buffering included.
    pub playing: bool,
    pub position_ms: u64,
    /// PNG of the current track's artwork, once it has loaded.
    pub cover: Option<Arc<[u8]>>,
}

/// What a sink is told about the track.
#[derive(Clone, Debug)]
pub struct OsMetadata<'a> {
    pub video_id: &'a str,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: Option<Duration>,
    /// `None` until the artwork loads, or for a track that has none; the sink must then show no
    /// picture rather than the previous track's.
    pub cover: Option<&'a Arc<[u8]>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsPlayback {
    Stopped,
    Paused { position: Duration },
    Playing { position: Duration },
}

/// The platform end: Souvlaki on Windows, a recorder in tests.
pub trait OsMediaSink {
    fn set_metadata(&mut self, metadata: &OsMetadata<'_>);
    fn set_playback(&mut self, playback: OsPlayback);
}

/// What the last metadata call said; a new value means a new call.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MetaKey {
    track: MediaTrack,
    cover: bool,
}

struct Pushed {
    playback: OsPlayback,
    at: Instant,
}

/// Decides what to tell the sink, and when.
pub struct MediaBridge<S: OsMediaSink> {
    sink: S,
    meta: Option<MetaKey>,
    pushed: Option<Pushed>,
}

impl<S: OsMediaSink> MediaBridge<S> {
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            meta: None,
            pushed: None,
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// Reconcile the overlay with `state`. Cheap when nothing changed; call on every player
    /// update.
    pub fn apply(&mut self, state: &MediaState, now: Instant) {
        let key = state.track.as_ref().map(|track| MetaKey {
            track: track.clone(),
            cover: state.cover.is_some(),
        });
        let meta_changed = key != self.meta;
        if meta_changed {
            if let Some(MetaKey { track, .. }) = &key {
                self.sink.set_metadata(&OsMetadata {
                    video_id: &track.video_id,
                    title: &track.title,
                    artist: &track.artist,
                    album: &track.album,
                    duration: track.duration_ms.map(Duration::from_millis),
                    cover: state.cover.as_ref(),
                });
            }
            self.meta = key;
        }
        let position = Duration::from_millis(state.position_ms);
        let wanted = match (&state.track, state.playing) {
            (None, _) => OsPlayback::Stopped,
            (Some(_), true) => OsPlayback::Playing { position },
            (Some(_), false) => OsPlayback::Paused { position },
        };
        if meta_changed || self.playback_needs_push(wanted, now) {
            self.sink.set_playback(wanted);
            self.pushed = Some(Pushed {
                playback: wanted,
                at: now,
            });
        }
    }

    /// Tell the OS the app is going away so the overlay does not outlive the window.
    pub fn stop(&mut self) {
        self.sink.set_playback(OsPlayback::Stopped);
        self.pushed = Some(Pushed {
            playback: OsPlayback::Stopped,
            at: Instant::now(),
        });
        self.meta = None;
    }

    fn playback_needs_push(&self, wanted: OsPlayback, now: Instant) -> bool {
        let Some(last) = &self.pushed else {
            return true;
        };
        match (last.playback, wanted) {
            (OsPlayback::Stopped, OsPlayback::Stopped) => false,
            (OsPlayback::Playing { position: from }, OsPlayback::Playing { position: to }) => {
                let elapsed = now.saturating_duration_since(last.at);
                let expected = from + elapsed;
                let drift = expected.abs_diff(to);
                drift > Duration::from_millis(DRIFT_MS) || elapsed >= RESYNC
            }
            (OsPlayback::Paused { position: from }, OsPlayback::Paused { position: to }) => {
                from.abs_diff(to) > Duration::from_millis(DRIFT_MS)
            }
            _ => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        metadata: Vec<(String, String, String, bool)>,
        playback: Vec<OsPlayback>,
    }

    impl OsMediaSink for Recorder {
        fn set_metadata(&mut self, m: &OsMetadata<'_>) {
            self.metadata.push((
                m.title.to_owned(),
                m.artist.to_owned(),
                m.album.to_owned(),
                m.cover.is_some(),
            ));
        }
        fn set_playback(&mut self, p: OsPlayback) {
            self.playback.push(p);
        }
    }

    fn track(id: &str) -> MediaTrack {
        MediaTrack {
            video_id: id.into(),
            title: format!("Title {id}"),
            artist: "Artist".into(),
            album: String::new(),
            duration_ms: Some(200_000),
        }
    }

    fn state(id: &str, playing: bool, position_ms: u64, cover: bool) -> MediaState {
        MediaState {
            track: Some(track(id)),
            playing,
            position_ms,
            cover: cover.then(|| Arc::from(vec![1u8, 2, 3])),
        }
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    #[test]
    fn a_new_track_sends_metadata_then_playback_once() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", false, 0, false), t0);
        b.apply(&state("a", false, 0, false), t0 + secs(1));
        assert_eq!(b.sink().metadata.len(), 1);
        assert_eq!(b.sink().metadata[0].0, "Title a");
        assert_eq!(b.sink().playback, vec![OsPlayback::Paused { position: secs(0) }]);
    }

    #[test]
    fn artwork_arriving_later_resends_the_metadata_with_the_cover() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", true, 0, false), t0);
        b.apply(&state("a", true, 400, true), t0 + Duration::from_millis(400));
        let meta = &b.sink().metadata;
        assert_eq!(meta.len(), 2);
        assert!(!meta[0].3 && meta[1].3);
    }

    #[test]
    fn play_pause_changes_the_status_and_nothing_else() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", true, 0, true), t0);
        b.apply(&state("a", false, 5_000, true), t0 + secs(5));
        b.apply(&state("a", true, 5_000, true), t0 + secs(6));
        assert_eq!(b.sink().metadata.len(), 1);
        assert_eq!(
            b.sink().playback,
            vec![
                OsPlayback::Playing { position: secs(0) },
                OsPlayback::Paused { position: secs(5) },
                OsPlayback::Playing { position: secs(5) },
            ]
        );
    }

    #[test]
    fn normal_progress_is_not_restated_but_a_seek_is() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", true, 0, true), t0);
        // Position keeps pace with the clock: the overlay extrapolates, so nothing is sent.
        b.apply(&state("a", true, 1_000, true), t0 + secs(1));
        b.apply(&state("a", true, 2_100, true), t0 + secs(2));
        assert_eq!(b.sink().playback.len(), 1);
        // A jump to 100 s is a seek.
        b.apply(&state("a", true, 100_000, true), t0 + secs(3));
        assert_eq!(b.sink().playback.len(), 2);
        assert_eq!(b.sink().playback[1], OsPlayback::Playing { position: secs(100) });
    }

    #[test]
    fn a_long_play_is_restated_so_drift_cannot_build_up() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", true, 0, true), t0);
        b.apply(&state("a", true, 10_000, true), t0 + secs(10));
        assert_eq!(b.sink().playback.len(), 2);
    }

    #[test]
    fn a_new_track_resets_the_playback_position() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", true, 90_000, true), t0);
        b.apply(&state("b", true, 0, false), t0 + secs(1));
        assert_eq!(b.sink().metadata.len(), 2);
        assert_eq!(b.sink().playback.last(), Some(&OsPlayback::Playing { position: secs(0) }));
    }

    #[test]
    fn no_track_reports_stopped_once() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&MediaState::default(), t0);
        b.apply(&MediaState::default(), t0 + secs(1));
        assert!(b.sink().metadata.is_empty());
        assert_eq!(b.sink().playback, vec![OsPlayback::Stopped]);
        b.apply(&state("a", true, 0, false), t0 + secs(2));
        b.apply(&MediaState::default(), t0 + secs(3));
        assert_eq!(b.sink().playback.last(), Some(&OsPlayback::Stopped));
    }

    #[test]
    fn stop_reports_stopped_even_after_playing() {
        let t0 = Instant::now();
        let mut b = MediaBridge::new(Recorder::default());
        b.apply(&state("a", true, 0, true), t0);
        b.stop();
        assert_eq!(b.sink().playback.last(), Some(&OsPlayback::Stopped));
    }
}
