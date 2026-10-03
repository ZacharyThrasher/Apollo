//! The UI's view of playback: owns the runtime driver (coordinator + service ports), turns
//! adapter completions into coordinator events, and exposes a plain snapshot for rendering.
//! Volume is a user setting kept here and sent straight to the engine port.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use apollo_domain::model::{QueueEntryId, Track};
use apollo_domain::playback::{Event, Failure, Intent, Phase};
use apollo_domain::queue::{QueueEntry, Repeat};
use apollo_runtime::Driver;
use apollo_theme::Accent;
use gpui::{Context, RenderImage, SharedString};

use crate::accent_anim::AccentAnim;
use crate::art::{ArtSource, DecodedArt};
use crate::backend::{LibraryTrack, Ports, Prepared};
use crate::library::LibrarySource;
use crate::settings::VolumeStore;
use crate::thumbs::Thumbs;

type AppDriver =
    Driver<Box<dyn apollo_runtime::Resolver + Send>, Box<dyn apollo_runtime::MediaEngine + Send>>;

/// Volume the app starts at when none was saved (coordinator decision).
pub const DEFAULT_VOLUME: u8 = 70;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Preparing the backend; controls are inert.
    Starting,
    Ready,
    /// The backend could not start. The text is already short enough to show.
    Unavailable(SharedString),
}

/// The images and accent made from one track's artwork.
struct ArtImages {
    image: Arc<RenderImage>,
    ambient: Arc<RenderImage>,
    accent: Accent,
}

enum ArtSlot {
    Loading,
    /// `None` = the track has no usable artwork.
    Ready(Option<ArtImages>),
}

/// One queue row, copied out of the model. Artwork is looked up per row with
/// [`PlayerModel::thumb`], so only the rows on screen ever load any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueRow {
    pub id: QueueEntryId,
    pub video_id: String,
    pub title: SharedString,
    pub artist: SharedString,
    pub duration_ms: Option<u64>,
}

/// The queue as the panel draws it: played, now playing, then what is up next in play order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueView {
    pub history: Vec<QueueRow>,
    pub current: Option<QueueRow>,
    pub upcoming: Vec<QueueRow>,
}

/// Everything the views draw, copied out of the model.
#[derive(Clone)]
pub struct PlayerView {
    pub status: Status,
    pub has_track: bool,
    pub title: SharedString,
    pub artist: SharedString,
    /// Artists and album on one line, for Now Playing.
    pub subtitle: SharedString,
    pub art: Option<Arc<RenderImage>>,
    /// Blurred artwork for the Now Playing background.
    pub ambient: Option<Arc<RenderImage>>,
    /// The user wants audio (play button shows pause).
    pub playing: bool,
    /// Waiting on the resolver/engine while the user wants audio.
    pub busy: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub volume: u8,
    pub muted: bool,
    pub error: Option<Failure>,
    pub accent: Accent,
    pub accent_moving: bool,
}

pub struct PlayerModel {
    status: Status,
    driver: Option<AppDriver>,
    art_source: Option<Arc<dyn ArtSource>>,
    library: Option<Arc<dyn LibrarySource>>,
    art: HashMap<String, ArtSlot>,
    thumbs: Thumbs,
    accent: AccentAnim,
    volume: u8,
    muted: bool,
    volume_store: Option<Arc<dyn VolumeStore>>,
    shuffle_seed: u64,
}

impl PlayerModel {
    pub fn new() -> Self {
        Self {
            status: Status::Starting,
            driver: None,
            art_source: None,
            library: None,
            art: HashMap::new(),
            thumbs: Thumbs::default(),
            accent: AccentAnim::new(Accent::fallback()),
            volume: DEFAULT_VOLUME,
            muted: false,
            volume_store: None,
            shuffle_seed: seed_from_clock(),
        }
    }

    /// Wire a prepared backend into the model. The queue loads paused: nothing plays until the
    /// user asks (never autoplay).
    pub fn install(&mut self, prepared: Prepared, cx: &mut Context<Self>) {
        let Prepared {
            ports: Ports { resolver, engine },
            queue,
            art,
            library,
            ..
        } = prepared;
        let mut driver = Driver::new(resolver, engine);
        driver.engine.set_volume(self.effective_volume());
        let tracks = queue
            .into_iter()
            .map(|LibraryTrack { track }| track)
            .collect();
        driver.edit_queue(|q| {
            q.play_context(tracks, 0);
        });
        self.driver = Some(driver);
        self.art_source = Some(art);
        self.library = Some(library);
        self.status = Status::Ready;
        self.sync(cx);
    }

    pub fn fail(&mut self, detail: &str, cx: &mut Context<Self>) {
        eprintln!("backend unavailable: {detail}");
        self.status = Status::Unavailable("Couldn't start playback".into());
        cx.notify();
    }

    /// Read-only access to the coordinator (diagnostics and tests).
    pub fn coordinator(&self) -> Option<&apollo_domain::playback::Coordinator> {
        self.driver.as_ref().map(|d| d.coordinator())
    }

    pub fn is_ready(&self) -> bool {
        self.driver.is_some()
    }

    /// The accent right now (mid-crossfade included) and whether it is still moving, for views
    /// that draw it without the full [`PlayerView`] snapshot.
    pub fn accent_now(&self) -> (Accent, bool) {
        self.accent.at(Instant::now())
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// What the library screen reads through, once the backend is installed.
    pub fn library_source(&self) -> Option<Arc<dyn LibrarySource>> {
        self.library.clone()
    }

    /// Silence the engine and drop the adapters (which reap their helper processes).
    pub fn shutdown(&mut self) {
        if let Some(mut d) = self.driver.take() {
            d.handle(Event::Stop);
        }
    }

    // ---- commands ----------------------------------------------------------

    pub fn handle(&mut self, event: Event, cx: &mut Context<Self>) {
        if let Some(d) = self.driver.as_mut() {
            d.handle(event);
        }
        self.sync(cx);
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.driver.as_ref() else {
            return;
        };
        let event = if d.coordinator().intent() == Intent::Playing {
            Event::Pause
        } else {
            Event::Play
        };
        self.handle(event, cx);
    }

    pub fn next(&mut self, cx: &mut Context<Self>) {
        self.handle(Event::Next, cx);
    }

    pub fn previous(&mut self, cx: &mut Context<Self>) {
        self.handle(Event::Previous, cx);
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.handle(Event::Play, cx);
    }

    /// Play `track` now and carry on with the existing queue after it: the track goes in front of
    /// the up-next list and playback moves to it. Nothing already queued is lost.
    pub fn play_now(&mut self, track: Track, cx: &mut Context<Self>) {
        if let Some(d) = self.driver.as_mut() {
            d.edit_queue(|q| {
                q.play_next(vec![track]);
            });
        }
        self.handle(Event::Next, cx);
    }

    /// Add `track` to the end of the up-next list, ahead of the rest of the context.
    pub fn add_to_queue(&mut self, track: Track, cx: &mut Context<Self>) {
        if let Some(d) = self.driver.as_mut() {
            d.edit_queue(|q| {
                q.add_to_queue(vec![track]);
            });
        }
        self.sync(cx);
    }

    pub fn seek(&mut self, position_ms: u64, cx: &mut Context<Self>) {
        self.handle(Event::Seek { position_ms }, cx);
    }

    fn next_seed(&mut self) -> u64 {
        let seed = self.shuffle_seed;
        self.shuffle_seed = self
            .shuffle_seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        seed
    }

    /// Tells the artwork source where a listing keeps a track's thumbnail, ahead of the track
    /// being queued (a row that has none is simply not hinted).
    pub fn hint_art(&self, video_id: &str, thumbnail: Option<&str>) {
        if let (Some(source), Some(url)) = (self.art_source.as_ref(), thumbnail) {
            source.hint(video_id, url);
        }
    }

    /// Replace the queue with `tracks` and start playing. `start` is the first track's index
    /// (default: the first, or a random one when shuffling); `shuffle` sets the queue's shuffle
    /// on or off, `None` keeping the current setting.
    pub fn play_tracks(
        &mut self,
        tracks: Vec<Track>,
        start: Option<usize>,
        shuffle: Option<bool>,
        cx: &mut Context<Self>,
    ) {
        if tracks.is_empty() || self.driver.is_none() {
            return;
        }
        let seed = self.next_seed();
        let shuffle_on = match (shuffle, self.driver.as_ref()) {
            (Some(on), _) => on,
            (None, Some(d)) => d.coordinator().queue().shuffle(),
            (None, None) => false,
        };
        let start = start.unwrap_or(if shuffle_on {
            (seed >> 11) as usize % tracks.len()
        } else {
            0
        });
        if let Some(d) = self.driver.as_mut() {
            // Before the context is built, so the new queue is shuffled around its first track.
            // Off then on, so a repeated Shuffle draws a new order instead of reusing the seed.
            d.edit_queue(|q| {
                q.set_shuffle(false, seed);
                q.set_shuffle(shuffle_on, seed);
            });
        }
        self.handle(Event::PlayContext { tracks, start }, cx);
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        let seed = self.next_seed();
        if let Some(d) = self.driver.as_mut() {
            let on = !d.coordinator().queue().shuffle();
            d.edit_queue(|q| q.set_shuffle(on, seed));
        }
        self.sync(cx);
    }

    pub fn cycle_repeat(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = self.driver.as_mut() {
            let next = next_repeat(d.coordinator().queue().repeat());
            d.edit_queue(|q| q.set_repeat(next));
        }
        self.sync(cx);
    }

    /// Play an entry picked in the queue panel.
    pub fn play_entry(&mut self, id: QueueEntryId, cx: &mut Context<Self>) {
        self.handle(Event::PlayEntry { id }, cx);
    }

    pub fn remove_entry(&mut self, id: QueueEntryId, cx: &mut Context<Self>) {
        self.edit_queue(cx, |q| {
            q.remove(id);
        });
    }

    /// Move an upcoming entry to `index` in the up-next order (it takes that row's place).
    pub fn move_upcoming(&mut self, id: QueueEntryId, index: usize, cx: &mut Context<Self>) {
        self.edit_queue(cx, |q| {
            q.move_upcoming(id, index);
        });
    }

    /// Drop everything up next; what is playing keeps playing.
    pub fn clear_upcoming(&mut self, cx: &mut Context<Self>) {
        self.edit_queue(cx, |q| q.clear_upcoming());
    }

    fn edit_queue(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut apollo_domain::queue::Queue),
    ) {
        if let Some(d) = self.driver.as_mut() {
            d.edit_queue(f);
        }
        self.sync(cx);
    }

    pub fn set_volume(&mut self, percent: u8, cx: &mut Context<Self>) {
        self.volume = percent.min(100);
        // Dragging the slider up from mute means "unmute".
        self.muted = self.muted && self.volume == 0;
        self.push_volume();
        cx.notify();
    }

    /// Where the volume is saved between runs (none in tests and the headless harness).
    pub fn set_volume_store(&mut self, store: Option<Arc<dyn VolumeStore>>) {
        self.volume_store = store;
    }

    /// Apply the volume saved by a previous run, before the backend installs.
    pub fn restore_volume(&mut self, saved: Option<u8>) {
        if let Some(v) = saved {
            self.volume = v.min(100);
        }
    }

    /// Save the current volume off the UI thread. Call when a change is final (slider released,
    /// key step), not for every drag position.
    pub fn save_volume(&self) {
        if let Some(store) = self.volume_store.clone() {
            let volume = self.volume;
            std::thread::spawn(move || store.save(volume));
        }
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        self.push_volume();
        cx.notify();
    }

    fn effective_volume(&self) -> u8 {
        if self.muted {
            0
        } else {
            self.volume
        }
    }

    fn push_volume(&mut self) {
        let v = self.effective_volume();
        if let Some(d) = self.driver.as_mut() {
            d.engine.set_volume(v);
        }
    }

    // ---- artwork -----------------------------------------------------------

    /// The video id of the queue's current track.
    pub fn current_video(&self) -> Option<String> {
        let d = self.driver.as_ref()?;
        Some(
            d.coordinator()
                .queue()
                .current()?
                .track
                .video_id
                .as_str()
                .to_owned(),
        )
    }

    /// Reconcile derived state after anything changed: request artwork for a new current
    /// track, steer the accent crossfade, and repaint.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.current_video() {
            if !self.art.contains_key(&id) {
                self.art.insert(id.clone(), ArtSlot::Loading);
                if let Some(source) = self.art_source.clone() {
                    let (key, owned) = (id.clone(), id.clone());
                    cx.spawn(async move |this, cx| {
                        let decoded = cx
                            .background_executor()
                            .spawn(async move { source.fetch(&key) })
                            .await;
                        let _ = this.update(cx, |m, cx| m.art_loaded(owned, decoded, cx));
                    })
                    .detach();
                }
            }
            self.steer_accent(&id, cx);
        } else {
            self.accent
                .retarget(Instant::now(), Accent::fallback(), !cx.reduce_motion());
        }
        cx.notify();
    }

    fn art_loaded(&mut self, id: String, decoded: Option<DecodedArt>, cx: &mut Context<Self>) {
        let slot = decoded.map(|d| ArtImages {
            image: d.render_image(),
            ambient: d.ambient_image(),
            accent: d.accent,
        });
        self.art.insert(id.clone(), ArtSlot::Ready(slot));
        if self.current_video().as_deref() == Some(id.as_str()) {
            self.steer_accent(&id, cx);
        }
        cx.notify();
    }

    /// While art is still loading the accent stays where it is; once known (or known to be
    /// absent) it crossfades to the track's accent or the fallback.
    fn steer_accent(&mut self, id: &str, cx: &Context<Self>) {
        let target = match self.art.get(id) {
            Some(ArtSlot::Ready(Some(images))) => Some(images.accent),
            Some(ArtSlot::Ready(None)) => Some(Accent::fallback()),
            _ => None,
        };
        if let Some(t) = target {
            self.accent.retarget(Instant::now(), t, !cx.reduce_motion());
        }
    }

    // ---- snapshot ----------------------------------------------------------

    pub fn view(&self) -> PlayerView {
        let (accent, accent_moving) = self.accent.at(Instant::now());
        let base = PlayerView {
            status: self.status.clone(),
            has_track: false,
            title: SharedString::default(),
            artist: SharedString::default(),
            subtitle: SharedString::default(),
            art: None,
            ambient: None,
            playing: false,
            busy: false,
            position_ms: 0,
            duration_ms: None,
            shuffle: false,
            repeat: Repeat::Off,
            volume: self.volume,
            muted: self.muted,
            error: None,
            accent,
            accent_moving,
        };
        let Some(d) = self.driver.as_ref() else {
            return base;
        };
        let c = d.coordinator();
        let current = c.queue().current();
        let images = current.and_then(|e| match self.art.get(e.track.video_id.as_str()) {
            Some(ArtSlot::Ready(Some(images))) => Some(images),
            _ => None,
        });
        let art = images.map(|i| i.image.clone());
        let ambient = images.map(|i| i.ambient.clone());
        let playing = c.intent() == Intent::Playing;
        PlayerView {
            has_track: current.is_some(),
            title: current
                .map(|e| e.track.title.clone().into())
                .unwrap_or_default(),
            artist: current.map(|e| artist_line(&e.track)).unwrap_or_default(),
            subtitle: current.map(|e| subtitle_line(&e.track)).unwrap_or_default(),
            art,
            ambient,
            playing,
            busy: playing
                && matches!(
                    c.phase(),
                    Phase::Resolving | Phase::Opening | Phase::Buffering
                ),
            position_ms: c.position_ms(),
            duration_ms: c
                .duration_ms()
                .or_else(|| current.and_then(|e| e.track.duration.map(|d| d.as_millis() as u64))),
            shuffle: c.queue().shuffle(),
            repeat: c.queue().repeat(),
            error: c.error(),
            ..base
        }
    }
}

impl PlayerModel {
    // ---- queue panel -------------------------------------------------------

    /// The queue as the panel draws it. Empty until the backend is installed.
    pub fn queue_view(&self) -> QueueView {
        let Some(d) = self.driver.as_ref() else {
            return QueueView::default();
        };
        let q = d.coordinator().queue();
        QueueView {
            history: q.history().iter().map(queue_row).collect(),
            current: q.current().map(queue_row),
            upcoming: q.upcoming().map(queue_row).collect(),
        }
    }

    /// A row's small artwork, once loaded.
    pub fn thumb(&self, video_id: &str) -> Option<Arc<RenderImage>> {
        self.thumbs.get(video_id)
    }

    /// Start loading artwork for rows that are on screen. Ids already loading or loaded are
    /// skipped, so calling this on every paint is cheap.
    pub fn request_thumbs(&mut self, video_ids: Vec<String>, cx: &mut Context<Self>) {
        let Some(source) = self.art_source.clone() else {
            return;
        };
        for id in self.thumbs.take_missing(video_ids) {
            let source = source.clone();
            let key = id.clone();
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move { source.fetch(&key).map(|d| d.thumb_image()) })
                    .await;
                let _ = this.update(cx, |m, cx| {
                    m.thumbs.finish(id, image);
                    cx.notify();
                });
            })
            .detach();
        }
    }
}

fn queue_row(entry: &QueueEntry) -> QueueRow {
    QueueRow {
        id: entry.id,
        video_id: entry.track.video_id.as_str().to_owned(),
        title: entry.track.title.clone().into(),
        artist: artist_line(&entry.track),
        duration_ms: entry.track.duration.map(|d| d.as_millis() as u64),
    }
}

impl Default for PlayerModel {
    fn default() -> Self {
        Self::new()
    }
}

fn seed_from_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15)
}

fn artist_line(track: &apollo_domain::model::Track) -> SharedString {
    track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}

/// Artist name(s) and album joined by " · "; whichever is missing is left out.
pub fn subtitle_line(track: &apollo_domain::model::Track) -> SharedString {
    let artists = artist_line(track);
    let album = track.album.as_ref().map(|a| a.title.as_str()).unwrap_or("");
    [artists.as_ref(), album]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
        .into()
}

pub fn next_repeat(r: Repeat) -> Repeat {
    match r {
        Repeat::Off => Repeat::All,
        Repeat::All => Repeat::One,
        Repeat::One => Repeat::Off,
    }
}

/// `m:ss`, or `h:mm:ss` from one hour up.
pub fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// What went wrong in six words or fewer (DIRECTION.md, States and copy).
pub fn failure_text(f: Failure) -> &'static str {
    match f {
        Failure::SessionExpired => "Sign-in expired",
        Failure::Restricted => "Not available to play",
        Failure::Throttled => "Too many requests",
        Failure::Network => "Connection problem",
        Failure::Unsupported => "Can't play this track",
        Failure::LeaseExpired => "Stream expired",
        Failure::Media => "Playback error",
        Failure::Device => "Audio device unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_format_as_m_ss_and_h_mm_ss() {
        assert_eq!(format_time(0), "0:00");
        assert_eq!(format_time(59_999), "0:59");
        assert_eq!(format_time(61_000), "1:01");
        assert_eq!(format_time(3_599_000), "59:59");
        assert_eq!(format_time(3_600_000), "1:00:00");
        assert_eq!(format_time(3_725_000), "1:02:05");
    }

    #[test]
    fn the_subtitle_joins_artists_and_album_and_skips_what_is_missing() {
        use apollo_domain::model::{AlbumRef, ArtistRef, Track};
        let mut t = Track::new("v", "Title");
        assert_eq!(subtitle_line(&t), "");
        t.artists = vec![
            ArtistRef {
                name: "A".into(),
                id: None,
            },
            ArtistRef {
                name: "B".into(),
                id: None,
            },
        ];
        assert_eq!(subtitle_line(&t), "A, B");
        t.album = Some(AlbumRef {
            title: "Album".into(),
            id: None,
        });
        assert_eq!(subtitle_line(&t), "A, B · Album");
        t.artists.clear();
        assert_eq!(subtitle_line(&t), "Album");
    }

    #[test]
    fn repeat_cycles_off_all_one() {
        assert_eq!(next_repeat(Repeat::Off), Repeat::All);
        assert_eq!(next_repeat(Repeat::All), Repeat::One);
        assert_eq!(next_repeat(Repeat::One), Repeat::Off);
    }

    #[test]
    fn failure_text_is_six_words_or_fewer() {
        for f in [
            Failure::SessionExpired,
            Failure::Restricted,
            Failure::Throttled,
            Failure::Network,
            Failure::Unsupported,
            Failure::LeaseExpired,
            Failure::Media,
            Failure::Device,
        ] {
            assert!(failure_text(f).split_whitespace().count() <= 6, "{f:?}");
        }
    }
}
