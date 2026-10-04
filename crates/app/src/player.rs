//! The UI's view of playback: owns the runtime driver (coordinator + service ports), turns
//! adapter completions into coordinator events, and exposes a plain snapshot for rendering.
//! Volume is a user setting kept here and sent straight to the engine port.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_domain::audio::{self, AudioDevice, Equalizer};
use apollo_domain::model::{QueueEntryId, Track};
use apollo_domain::playback::{EngineEvent, Event, Failure, Intent, Phase};
use apollo_domain::queue::{Heading, Queue, QueueEntry, QueueSnapshot, QueueSource, Repeat};
use apollo_domain::undo::{put_back, UndoHistory};
use apollo_runtime::Driver;
use apollo_theme::Accent;
use gpui::{Context, RenderImage, SharedString};

use crate::accent_anim::AccentAnim;
use crate::art::{ArtSource, DecodedArt};
use crate::backend::{LibraryTrack, Ports, Prepared};
use crate::latency::Latency;
use crate::library::LibrarySource;
use crate::os_media::{MediaState, MediaTrack, OsCommand};
use crate::persist::QueueSaver;
use crate::reporting::{Activity, Feed, PlayReporter, Snapshot};
use crate::settings::{AutoplayStore, GaplessStore, NormalizeStore, QueueStore, VolumeStore};
use crate::sleep_timer::{Playhead, SleepChoice, SleepStep, SleepTimer};
use crate::thumbs::Thumbs;

type AppDriver =
    Driver<Box<dyn apollo_runtime::Resolver + Send>, Box<dyn apollo_runtime::MediaEngine + Send>>;

/// Volume the app starts at when none was saved (coordinator decision).
pub const DEFAULT_VOLUME: u8 = 70;

/// How long the pointer (or keyboard selection) must rest on a track before it is resolved ahead
/// of a click: long enough that sweeping across a list asks YouTube for nothing.
const INTENT_DELAY: Duration = Duration::from_millis(250);

/// Tracks after the one that starts that are resolved right away, so the first Next is instant
/// (the coordinator itself only preloads the next track once the current one has loaded).
const LOOKAHEAD: usize = 2;

/// Autoplay asks for more once this few tracks are left up next, so the next song is already
/// resolved when the last one of the list ends.
const RADIO_LOW_WATER: usize = 2;

/// Most tracks one radio answer adds to the queue.
const RADIO_APPEND: usize = 25;

/// A radio that failed (or added nothing) is asked for again after this long, not on every tick.
const RADIO_RETRY: Duration = Duration::from_secs(30);

/// How often the playing position is saved between queue changes.
const CHECKPOINT_EVERY: Duration = Duration::from_secs(5);

/// How often the list of output devices is looked at again, so the Output device row follows a
/// headset being plugged in or pulled out. A read of what the engine already knows; the screen
/// is only redrawn when the list changed.
const DEVICE_REFRESH: Duration = Duration::from_secs(2);
/// How often the sleep timer looks while a fade is possible (a countdown a long way off, or the
/// end of a long track, is looked at once a second instead).
const SLEEP_TICK_FAST: Duration = Duration::from_millis(50);
const SLEEP_TICK_SLOW: Duration = Duration::from_secs(1);

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
    /// PNG for the OS media overlay, encoded off the UI thread.
    cover: Option<Arc<[u8]>>,
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
    /// The whole track, for what a row's menu needs (its artist and album, its link).
    pub track: Track,
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
    /// What the queue is playing from; none while nothing is queued.
    pub heading: Option<QueueHeading>,
}

/// The queue header's "Playing from" line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueHeading {
    /// A playlist, album, artist, radio, search or other place.
    Source(QueueSource),
    /// Playback has gone past the source into autoplay; the track it was seeded from.
    Autoplay(String),
    /// The queue was put together by hand.
    Yours,
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
    /// Autoplay: a radio continues the queue when it would end.
    pub autoplay: bool,
    pub volume: u8,
    pub muted: bool,
    pub error: Option<Failure>,
    /// The sleep timer's setting, and while a countdown runs the minutes left (rounded up).
    pub sleep: SleepChoice,
    pub sleep_minutes: Option<u32>,
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
    /// The sleep timer's fade: the share of the user's volume that is let through (1.0 = all).
    /// Separate from `volume`, which stays what the user set.
    fade: f32,
    /// The fade ended in a pause and the volume is held down until the music is wanted again,
    /// so it is not brought back while the engine is still winding down.
    fade_held: bool,
    /// What was last sent to the engine, to skip sending the same percentage again.
    pushed_volume: Option<u8>,
    sleep: SleepTimer,
    sleep_ticking: bool,
    sleep_label: Option<u32>,
    volume_store: Option<Arc<dyn VolumeStore>>,
    shuffle_seed: u64,
    latency: Latency,
    /// The track the user last showed interest in (hover/selection), for the prefetch debounce.
    intent: Option<String>,
    autoplay: bool,
    autoplay_store: Option<Arc<dyn AutoplayStore>>,
    /// Volume normalization (Settings > Playback): on by default.
    normalize: bool,
    normalize_store: Option<Arc<dyn NormalizeStore>>,
    /// Gapless playback (Settings > Playback): on by default.
    gapless: bool,
    gapless_store: Option<Arc<dyn GaplessStore>>,
    /// Audio toolbox: the output device chosen (`None` = system default), the devices the engine
    /// last saw, the equalizer and the playback speed (the speed is not saved: 1.0 each launch).
    audio_device: Option<String>,
    audio_devices: Vec<AudioDevice>,
    equalizer: Equalizer,
    speed: f32,
    /// Writes the queue and position down; present while someone is signed in.
    saver: Option<QueueSaver>,
    /// What the saved queue was made from, to tell a changed queue from a moved position.
    saved_fp: Option<u64>,
    /// Whether a queue is saved at all (a fresh default queue is only saved once it is used).
    saved_row: bool,
    last_checkpoint: Instant,
    /// The position is worth saving right now (a pause, a seek).
    checkpoint_now: bool,
    /// Where plays are reported to YouTube Music; none in the simulation, tests and the headless
    /// harness, which never report.
    reporter: Option<Arc<dyn PlayReporter>>,
    /// Settings > Playback > Add plays to history. Off until the app says otherwise.
    report_plays: bool,
    report_feed: Feed,
    /// Queue-changing actions, for Ctrl+Z / Ctrl+Shift+Z.
    undo: UndoHistory,
    /// The undo history's generation right after Start radio recorded its step: the radio that
    /// arrives next joins that step instead of making its own, so one undo takes both back.
    radio_step: Option<u64>,
    radio_inflight: bool,
    /// The last track a radio was asked for. Not asked again until the queue's tail changes or
    /// [`RADIO_RETRY`] has passed, so a failure is not retried on every tick.
    radio_tried: Option<String>,
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
            fade: 1.0,
            fade_held: false,
            pushed_volume: None,
            sleep: SleepTimer::default(),
            sleep_ticking: false,
            sleep_label: None,
            volume_store: None,
            shuffle_seed: seed_from_clock(),
            latency: Latency::default(),
            intent: None,
            autoplay: true,
            autoplay_store: None,
            normalize: true,
            normalize_store: None,
            gapless: true,
            gapless_store: None,
            audio_device: None,
            audio_devices: Vec::new(),
            equalizer: Equalizer::flat(),
            speed: audio::NORMAL_SPEED,
            saver: None,
            saved_fp: None,
            saved_row: false,
            last_checkpoint: Instant::now(),
            checkpoint_now: false,
            reporter: None,
            report_plays: false,
            report_feed: Feed::default(),
            undo: UndoHistory::new(),
            radio_step: None,
            radio_inflight: false,
            radio_tried: None,
        }
    }

    /// Wire a prepared backend into the model. The queue loads paused: nothing plays until the
    /// user asks (never autoplay).
    pub fn install(&mut self, prepared: Prepared, cx: &mut Context<Self>) {
        self.install_with(prepared, None, cx);
    }

    /// [`install`](Self::install) with the queue a previous run saved: it comes back as it was
    /// (current track, what is up next, shuffle and repeat), paused at the saved position. Without
    /// a usable saved queue the backend's own starting queue is used.
    pub fn install_with(
        &mut self,
        prepared: Prepared,
        saved: Option<(Queue, u64)>,
        cx: &mut Context<Self>,
    ) {
        let Prepared {
            ports: Ports { resolver, engine },
            queue,
            art,
            library,
            ..
        } = prepared;
        let mut driver = match saved.filter(|(q, _)| q.current().is_some()) {
            Some((queue, position_ms)) => {
                self.saved_row = true;
                Driver::restored(resolver, engine, queue, position_ms)
            }
            None => {
                let mut driver = Driver::new(resolver, engine);
                let tracks = queue
                    .into_iter()
                    .map(|LibraryTrack { track }| track)
                    .collect();
                driver.edit_queue(|q| {
                    q.play_context(tracks, 0);
                });
                driver
            }
        };
        driver.engine.set_volume(self.effective_volume());
        driver.engine.set_normalization(self.normalize);
        driver.set_gapless(self.gapless);
        driver.engine.set_audio_device(self.audio_device.clone());
        driver.engine.set_equalizer(self.equalizer);
        driver.engine.set_speed(self.speed);
        // The queue starts paused on its current track: have it ready so the first Play is instant.
        let first = driver.coordinator().queue().current().map(|e| e.track.clone());
        self.saved_fp = Some(queue_fingerprint(driver.coordinator().queue()));
        self.driver = Some(driver);
        self.art_source = Some(art);
        self.library = Some(library);
        self.status = Status::Ready;
        if let Some(track) = first {
            self.prefetch(&track);
        }
        self.refresh_audio_devices(cx);
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(DEVICE_REFRESH).await;
            if this
                .update(cx, |m, cx| m.refresh_audio_devices(cx))
                .is_err()
            {
                break;
            }
        })
        .detach();
        self.sync(cx);
    }

    /// Where plays are reported (none leaves reporting off, whatever the setting says).
    pub fn set_play_reporter(&mut self, reporter: Option<Arc<dyn PlayReporter>>) {
        self.reporter = reporter;
    }

    /// Settings > Playback > Add plays to history. Turning it off sends nothing more at all, not
    /// even the closing ping of the track playing now; turning it on starts with whatever is
    /// playing then.
    pub fn set_report_plays(&mut self, on: bool) {
        if self.report_plays == on {
            return;
        }
        self.report_plays = on;
        if !on {
            self.report_feed.reset();
            if let Some(reporter) = self.reporter.as_ref() {
                reporter.cancel();
            }
        }
    }

    /// Hand the reporter what the player is doing, if reporting is on. Never blocks.
    fn report(&mut self) {
        let Some(reporter) = self.reporter.clone() else {
            return;
        };
        if !self.report_plays {
            return;
        }
        let snapshot = self.report_snapshot();
        if self.report_feed.due(snapshot.as_ref(), Instant::now()) {
            reporter.observe(snapshot);
        }
    }

    fn report_snapshot(&self) -> Option<Snapshot> {
        let c = self.driver.as_ref()?.coordinator();
        let entry = c.queue().current()?;
        let activity = match (c.intent(), c.phase()) {
            (Intent::Playing, Phase::Playing) => Activity::Playing,
            (Intent::Playing, _) => Activity::Stalled,
            _ => Activity::Paused,
        };
        Some(Snapshot {
            entry: entry.id.get(),
            video_id: entry.track.video_id.as_str().to_owned(),
            activity,
            position_ms: c.position_ms(),
            duration_ms: c
                .duration_ms()
                .or_else(|| entry.track.duration.map(|d| d.as_millis() as u64)),
            volume: self.volume,
            muted: self.muted,
        })
    }

    /// Where the autoplay switch is saved between runs (none in tests and the headless harness).
    pub fn set_autoplay_store(&mut self, store: Option<Arc<dyn AutoplayStore>>) {
        self.autoplay_store = store;
    }

    /// Apply the autoplay setting saved by a previous run (on when none was saved).
    pub fn restore_autoplay(&mut self, saved: Option<bool>) {
        if let Some(on) = saved {
            self.autoplay = on;
        }
    }

    pub fn autoplay(&self) -> bool {
        self.autoplay
    }

    /// Turn autoplay on or off and remember the choice. What is already queued stays.
    pub fn set_autoplay(&mut self, on: bool, cx: &mut Context<Self>) {
        self.autoplay = on;
        self.radio_tried = None;
        if let Some(store) = self.autoplay_store.clone() {
            std::thread::spawn(move || store.save(on));
        }
        self.sync(cx);
    }

    /// Where the volume-normalization switch is saved between runs (none in tests and the
    /// headless harness).
    pub fn set_normalize_store(&mut self, store: Option<Arc<dyn NormalizeStore>>) {
        self.normalize_store = store;
    }

    /// Apply the normalization setting saved by a previous run (on when none was saved), before
    /// the backend installs.
    pub fn restore_normalize(&mut self, saved: Option<bool>) {
        if let Some(on) = saved {
            self.normalize = on;
        }
    }

    pub fn normalize(&self) -> bool {
        self.normalize
    }

    /// Turn volume normalization on or off and remember the choice. It applies to the track that
    /// is playing, not just the next one.
    pub fn set_normalize(&mut self, on: bool, cx: &mut Context<Self>) {
        self.normalize = on;
        if let Some(d) = self.driver.as_mut() {
            d.engine.set_normalization(on);
        }
        if let Some(store) = self.normalize_store.clone() {
            std::thread::spawn(move || store.save(on));
        }
        cx.notify();
    }

    /// Where the gapless switch is saved between runs (none in tests and the headless harness).
    pub fn set_gapless_store(&mut self, store: Option<Arc<dyn GaplessStore>>) {
        self.gapless_store = store;
    }

    /// Apply the gapless setting saved by a previous run (on when none was saved), before the
    /// backend installs.
    pub fn restore_gapless(&mut self, saved: Option<bool>) {
        if let Some(on) = saved {
            self.gapless = on;
        }
    }

    pub fn gapless(&self) -> bool {
        self.gapless
    }

    /// Turn gapless playback on or off and remember the choice. Off means the next track opens
    /// when the current one ends, as before; on takes effect for the track that follows the one
    /// playing.
    pub fn set_gapless(&mut self, on: bool, cx: &mut Context<Self>) {
        self.gapless = on;
        if let Some(d) = self.driver.as_mut() {
            d.set_gapless(on);
        }
        if let Some(store) = self.gapless_store.clone() {
            std::thread::spawn(move || store.save(on));
        }
        cx.notify();
    }

    /// The output device chosen: `None` is the system default.
    pub fn audio_device(&self) -> Option<&str> {
        self.audio_device.as_deref()
    }

    /// The output devices the engine last saw, without the system default.
    pub fn audio_devices(&self) -> &[AudioDevice] {
        &self.audio_devices
    }

    /// What the Output device row shows: the device that is actually playing.
    pub fn audio_device_label(&self) -> String {
        audio::device_label(self.audio_device.as_deref(), &self.audio_devices)
    }

    /// Play on `device` (`None` = system default). The choice is kept while the device is
    /// unplugged: the engine plays on the default meanwhile and moves back when it returns.
    /// Saving it is the caller's (the Settings page's) business.
    pub fn set_audio_device(&mut self, device: Option<String>, cx: &mut Context<Self>) {
        if self.audio_device == device {
            return;
        }
        self.audio_device = device.clone();
        if let Some(d) = self.driver.as_mut() {
            d.engine.set_audio_device(device);
        }
        cx.notify();
    }

    /// Looks at the engine's list of devices again and redraws only if it changed.
    pub fn refresh_audio_devices(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.driver.as_ref() else { return };
        let devices = d.engine.audio_devices();
        if devices != self.audio_devices {
            self.audio_devices = devices;
            cx.notify();
        }
    }

    pub fn equalizer(&self) -> Equalizer {
        self.equalizer
    }

    /// Apply an equalizer (the switch and the curve) to the engine, live and without a click.
    /// Saving it is the caller's business: dragging a slider calls this on every move.
    pub fn set_equalizer(&mut self, eq: Equalizer, cx: &mut Context<Self>) {
        let eq = eq.sanitized();
        if self.equalizer == eq {
            return;
        }
        self.equalizer = eq;
        if let Some(d) = self.driver.as_mut() {
            d.engine.set_equalizer(eq);
        }
        cx.notify();
    }

    /// Playback speed, one of [`audio::SPEEDS`]. Not saved: normal speed on every launch.
    pub fn speed(&self) -> f32 {
        self.speed
    }

    pub fn set_speed(&mut self, speed: f32, cx: &mut Context<Self>) {
        let speed = audio::sanitize_speed(speed);
        if self.speed == speed {
            return;
        }
        self.speed = speed;
        if let Some(d) = self.driver.as_mut() {
            d.engine.set_speed(speed);
        }
        cx.notify();
    }

    /// Start saving the queue under `account_key` (an opaque id of the signed-in account). With
    /// no key, or no store, nothing is saved.
    pub fn set_queue_store(&mut self, store: Option<Arc<dyn QueueStore>>, account_key: Option<String>) {
        self.saver = store.zip(account_key).map(|(s, key)| QueueSaver::new(s, key));
    }

    /// Sign-out: delete the saved queue and stop saving.
    pub fn forget_saved_queue(&mut self) {
        if let Some(saver) = self.saver.take() {
            saver.forget();
            // Waits for the delete: nothing of the account may outlive its sign-out.
            saver.flush();
        }
        self.saved_row = false;
        self.clear_queue_history();
    }

    /// Where the "time to first audio" lines go (none in tests).
    pub fn set_latency_log(&mut self, path: Option<std::path::PathBuf>) {
        self.latency.set_log(path);
    }

    /// Resolve `track` ahead of need so that playing it starts from a ready stream. Silent and
    /// cheap to repeat: a track already ready or being resolved is left alone.
    pub fn prefetch(&mut self, track: &Track) {
        if let Some(d) = self.driver.as_mut() {
            d.prefetch(track);
        }
    }

    /// The user is lingering on `track` (hover, keyboard selection): prefetch it once the interest
    /// has lasted [`INTENT_DELAY`] and moved nowhere else.
    pub fn hint_play(&mut self, track: &Track, cx: &mut Context<Self>) {
        let id = track.video_id.as_str().to_owned();
        if self.intent.as_deref() == Some(id.as_str()) {
            return;
        }
        self.intent = Some(id.clone());
        let track = track.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(INTENT_DELAY).await;
            let _ = this.update(cx, |m, _| {
                if m.intent.as_deref() == Some(id.as_str()) {
                    m.prefetch(&track);
                }
            });
        })
        .detach();
    }

    /// Prefetch what plays after the current track, in play order.
    fn prefetch_upcoming(&mut self) {
        let Some(d) = self.driver.as_mut() else { return };
        let next: Vec<Track> = d
            .coordinator()
            .queue()
            .upcoming()
            .take(LOOKAHEAD)
            .map(|e| e.track.clone())
            .collect();
        for track in &next {
            d.prefetch(track);
        }
    }

    /// Time-to-first-audio bookkeeping: what the user just asked for, and the milestones that
    /// follow. Observes only; it never changes what is played.
    fn trace(&mut self, kind: Option<&'static str>, event: &Event) {
        let now = Instant::now();
        let Some(d) = self.driver.as_ref() else { return };
        let c = d.coordinator();
        let active = c.active_load();
        let begin = |l: &mut Latency, default: &'static str| l.begin(kind.unwrap_or(default), now);
        match event {
            Event::PlayContext { .. } => begin(&mut self.latency, "play_tracks"),
            Event::PlayEntry { .. } => begin(&mut self.latency, "queue_entry"),
            Event::Next => begin(&mut self.latency, "next"),
            Event::Previous => begin(&mut self.latency, "previous"),
            Event::Play if c.phase() == Phase::Idle => begin(&mut self.latency, "play"),
            Event::Pause | Event::Stop | Event::Logout => self.latency.cancel(),
            Event::Resolved { load, .. } if active == Some(*load) => self.latency.resolved(now),
            Event::Engine { load, event } if active == Some(*load) => match event {
                EngineEvent::Opened { .. } => self.latency.opened(now),
                EngineEvent::Position(p) if *p > 0 => {
                    self.latency.audio(now);
                }
                _ => {}
            },
            _ => {}
        }
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

    /// Silence the engine and drop the adapters (which reap their helper processes). The queue
    /// and position are written down first (stopping resets the position).
    pub fn shutdown(&mut self) {
        self.checkpoint_now = true;
        self.persist();
        if let Some(saver) = self.saver.as_ref() {
            saver.flush();
        }
        if let Some(mut d) = self.driver.take() {
            d.handle(Event::Stop);
        }
    }

    // ---- commands ----------------------------------------------------------

    pub fn handle(&mut self, event: Event, cx: &mut Context<Self>) {
        self.handle_as(None, event, cx);
    }

    /// `handle`, naming what the user did for the latency log (`None` = derived from the event).
    fn handle_as(&mut self, kind: Option<&'static str>, event: Event, cx: &mut Context<Self>) {
        self.trace(kind, &event);
        self.checkpoint_now |= matches!(
            event,
            Event::Pause | Event::OutputLost | Event::Seek { .. }
        );
        // Music wanted again after a sleep-timer fade: the volume is back before it starts.
        if self.fade_held
            && matches!(
                event,
                Event::Play
                    | Event::PlayContext { .. }
                    | Event::PlayEntry { .. }
                    | Event::Next
                    | Event::Previous
            )
        {
            self.release_fade();
        }
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
        let before = self.undo_capture();
        if let Some(d) = self.driver.as_mut() {
            d.edit_queue(|q| {
                q.play_next(vec![track]);
            });
        }
        self.handle_as(Some("play_now"), Event::Next, cx);
        self.undo_commit(before);
        self.prefetch_upcoming();
    }

    /// Add `track` to the end of the up-next list, ahead of the rest of the context.
    pub fn add_to_queue(&mut self, track: Track, cx: &mut Context<Self>) {
        self.edit_queue(cx, |q| {
            q.add_to_queue(vec![track]);
        });
    }

    /// Put `track` right after the one playing.
    pub fn play_next(&mut self, track: Track, cx: &mut Context<Self>) {
        self.edit_queue(cx, |q| {
            q.play_next(vec![track]);
        });
        self.prefetch_upcoming();
    }

    /// Add `tracks` to the end of the up-next list, ahead of the rest of the context.
    pub fn add_tracks_to_queue(&mut self, tracks: Vec<Track>, cx: &mut Context<Self>) {
        self.edit_queue(cx, |q| {
            q.add_to_queue(tracks);
        });
    }

    /// Start a radio: `seed` plays now and the queue carries on as it, once `radio` arrives
    /// ([`PlayerModel::finish_radio`]). The queue is left as it is until then, so a radio that
    /// cannot be fetched loses nothing.
    pub fn start_radio(&mut self, seed: Track, cx: &mut Context<Self>) {
        let before = self.undo_capture();
        self.edit_queue_untracked(cx, |q| {
            q.play_next(vec![seed]);
        });
        self.handle_as(Some("start_radio"), Event::Next, cx);
        self.undo_commit(before);
        self.radio_step = Some(self.undo.generation());
    }

    /// The radio's tracks replace everything that was up next, and the queue now plays from
    /// `source` (the radio).
    pub fn finish_radio(&mut self, tracks: Vec<Track>, source: QueueSource, cx: &mut Context<Self>) {
        let edit = |q: &mut Queue| {
            q.clear_upcoming();
            q.append_radio(tracks);
            q.set_source(Some(source));
        };
        // The radio belongs to the Start radio that asked for it, unless something else was
        // recorded since: then it is a step of its own.
        if self.radio_step.take() == Some(self.undo.generation()) {
            self.edit_queue_untracked(cx, edit);
        } else {
            self.edit_queue(cx, edit);
        }
        self.prefetch_upcoming();
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

    /// The https URL of a track's artwork, for surfaces that cannot take pixels.
    pub fn art_url(&self, video_id: &str) -> Option<String> {
        self.art_source.as_ref()?.url(video_id)
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
        self.play_tracks_from(tracks, start, shuffle, None, cx);
    }

    /// [`PlayerModel::play_tracks`] saying where the tracks come from (the queue header names
    /// it); `None` is a queue with no place of its own.
    pub fn play_tracks_from(
        &mut self,
        tracks: Vec<Track>,
        start: Option<usize>,
        shuffle: Option<bool>,
        source: Option<QueueSource>,
        cx: &mut Context<Self>,
    ) {
        if tracks.is_empty() || self.driver.is_none() {
            return;
        }
        let before = self.undo_capture();
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
        self.edit_queue_untracked(cx, |q| q.set_source(source));
        self.undo_commit(before);
        self.prefetch_upcoming();
    }

    pub fn toggle_shuffle(&mut self, cx: &mut Context<Self>) {
        let seed = self.next_seed();
        let on = match self.driver.as_ref() {
            Some(d) => !d.coordinator().queue().shuffle(),
            None => return,
        };
        self.edit_queue(cx, |q| q.set_shuffle(on, seed));
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

    /// A queue edit the user can take back with Ctrl+Z.
    fn edit_queue(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Queue)) {
        let before = self.undo_capture();
        self.edit_queue_untracked(cx, f);
        self.undo_commit(before);
    }

    fn edit_queue_untracked(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Queue)) {
        if let Some(d) = self.driver.as_mut() {
            d.edit_queue(f);
        }
        self.sync(cx);
    }

    // ---- undo ----------------------------------------------------------------

    /// The queue and position as they are before an action, for [`PlayerModel::undo_commit`].
    fn undo_capture(&self) -> Option<(QueueSnapshot, u64)> {
        let c = self.driver.as_ref()?.coordinator();
        Some((c.queue().snapshot(), c.position_ms()))
    }

    /// The action is done: it becomes a step if it changed the queue.
    fn undo_commit(&mut self, before: Option<(QueueSnapshot, u64)>) {
        let (Some((snapshot, position_ms)), Some(d)) = (before, self.driver.as_ref()) else {
            return;
        };
        self.undo
            .record(snapshot, position_ms, &d.coordinator().queue().snapshot());
    }

    pub fn can_undo_queue(&self) -> bool {
        self.undo.can_undo()
    }

    pub fn can_redo_queue(&self) -> bool {
        self.undo.can_redo()
    }

    /// Forget the undo history (sign-out, a different account).
    pub fn clear_queue_history(&mut self) {
        self.undo.clear();
        self.radio_step = None;
    }

    /// Ctrl+Z: put the queue back as it was before the last queue-changing action. What is
    /// playing keeps playing unless that action changed it; then the old track comes back
    /// paused where it was. False when there was nothing to undo.
    pub fn undo_queue(&mut self, cx: &mut Context<Self>) -> bool {
        self.step_queue(true, cx)
    }

    /// Ctrl+Shift+Z: do again what Ctrl+Z took back.
    pub fn redo_queue(&mut self, cx: &mut Context<Self>) -> bool {
        self.step_queue(false, cx)
    }

    fn step_queue(&mut self, undo: bool, cx: &mut Context<Self>) -> bool {
        let Some(d) = self.driver.as_mut() else {
            return false;
        };
        let live = d.coordinator().queue().snapshot();
        let position = d.coordinator().position_ms();
        let step = if undo {
            self.undo.undo(live, position)
        } else {
            self.undo.redo(live, position)
        };
        let Some(step) = step else {
            return false;
        };
        self.radio_step = None;
        let Some(queue) = put_back(&step, d.coordinator().queue()) else {
            return false;
        };
        d.replace_queue(queue, step.position_ms);
        self.sync(cx);
        self.prefetch_upcoming();
        true
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

    /// A volume key: `delta` percent louder or quieter, saved at once. Louder while muted
    /// unmutes at the stepped level; quieter while muted changes nothing.
    pub fn nudge_volume(&mut self, delta: i8, cx: &mut Context<Self>) {
        if self.muted && delta < 0 {
            return;
        }
        self.set_volume(self.volume.saturating_add_signed(delta), cx);
        self.save_volume();
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
            (f32::from(self.volume) * self.fade).round().clamp(0.0, 100.0) as u8
        }
    }

    fn push_volume(&mut self) {
        let v = self.effective_volume();
        self.pushed_volume = Some(v);
        if let Some(d) = self.driver.as_mut() {
            d.engine.set_volume(v);
        }
    }

    // ---- the PC around the player ---------------------------------------------

    /// The sound output went away or changed (headphones unplugged, Bluetooth dropped) or the PC
    /// slept: pause where the track is. The queue and position stay, nothing skips, and the next
    /// Play opens the track again so the sound comes out of whatever output exists by then.
    pub fn output_lost(&mut self, cx: &mut Context<Self>) {
        self.handle(Event::OutputLost, cx);
    }

    /// Another app has been playing sound for a while (a movie, a call): pause, if music is
    /// playing. Nothing resumes by itself afterwards.
    pub fn other_audio(&mut self, cx: &mut Context<Self>) {
        let playing = self
            .driver
            .as_ref()
            .is_some_and(|d| d.coordinator().intent() == Intent::Playing);
        if playing {
            self.handle(Event::Pause, cx);
        }
    }

    // ---- sleep timer -----------------------------------------------------------

    pub fn sleep_choice(&self) -> SleepChoice {
        self.sleep.choice()
    }

    /// Start the sleep timer (or turn it off). Picking a length again restarts the countdown.
    pub fn set_sleep(&mut self, choice: SleepChoice, cx: &mut Context<Self>) {
        self.set_sleep_at(choice, Instant::now(), cx);
    }

    /// [`set_sleep`](Self::set_sleep) with the clock passed in, so tests need no sleeping.
    #[doc(hidden)]
    pub fn set_sleep_at(&mut self, choice: SleepChoice, now: Instant, cx: &mut Context<Self>) {
        let was_fading = self.sleep.is_fading();
        self.sleep.set(choice, now);
        if was_fading {
            self.fade = 1.0;
            self.push_volume();
        }
        self.sleep_label = self.sleep.minutes_left(now);
        if choice.is_on() {
            self.ensure_sleep_ticker(cx);
        }
        cx.notify();
    }

    fn playhead(&self) -> Option<Playhead> {
        let d = self.driver.as_ref()?;
        let c = d.coordinator();
        let current = c.queue().current();
        Some(Playhead {
            playing: c.intent() == Intent::Playing,
            position_ms: c.position_ms(),
            duration_ms: c
                .duration_ms()
                .or_else(|| current.and_then(|e| e.track.duration.map(|d| d.as_millis() as u64))),
        })
    }

    fn ensure_sleep_ticker(&mut self, cx: &mut Context<Self>) {
        if self.sleep_ticking {
            return;
        }
        self.sleep_ticking = true;
        cx.spawn(async move |this, cx| loop {
            let wait = this
                .update(cx, |m, cx| m.sleep_tick(cx))
                .ok()
                .flatten();
            let Some(wait) = wait else { break };
            cx.background_executor().timer(wait).await;
        })
        .detach();
    }

    /// One look at the timer. Returns how long to wait before the next one, or `None` when the
    /// timer is over and the ticker can stop.
    fn sleep_tick(&mut self, cx: &mut Context<Self>) -> Option<Duration> {
        self.sleep_tick_at(Instant::now(), cx)
    }

    /// [`sleep_tick`](Self::sleep_tick) with the clock passed in, so tests need no sleeping.
    #[doc(hidden)]
    pub fn sleep_tick_at(&mut self, now: Instant, cx: &mut Context<Self>) -> Option<Duration> {
        let head = self.playhead().unwrap_or(Playhead {
            playing: false,
            position_ms: 0,
            duration_ms: None,
        });
        match self.sleep.tick(now, head) {
            SleepStep::Idle => {}
            SleepStep::Fade(factor) => {
                self.fade = factor.clamp(0.0, 1.0);
                if Some(self.effective_volume()) != self.pushed_volume {
                    self.push_volume();
                }
            }
            SleepStep::Done => {
                // Silent: pause, and keep the volume down until music is wanted again.
                self.fade = 0.0;
                self.push_volume();
                self.fade_held = true;
                self.handle(Event::Pause, cx);
            }
            SleepStep::Cancelled => {
                self.fade = 1.0;
                self.push_volume();
            }
        }
        let label = self.sleep.minutes_left(now);
        if label != self.sleep_label {
            self.sleep_label = label;
            cx.notify();
        }
        if !self.sleep.choice().is_on() {
            self.sleep_ticking = false;
            cx.notify();
            return None;
        }
        Some(if self.sleep.is_fading() || self.fade_possible(now, head) {
            SLEEP_TICK_FAST
        } else {
            SLEEP_TICK_SLOW
        })
    }

    /// Whether a fade could begin before the next slow look: a countdown in its last seconds, or
    /// the last seconds of the track.
    fn fade_possible(&self, now: Instant, head: Playhead) -> bool {
        use crate::sleep_timer::FADE;
        let soon = FADE * 2;
        match self.sleep.choice() {
            SleepChoice::Off => false,
            SleepChoice::Minutes(_) => self.sleep.time_left(now).is_some_and(|left| left <= soon),
            SleepChoice::EndOfTrack => head.playing
                && head
                    .duration_ms
                    .is_some_and(|d| d.saturating_sub(head.position_ms) <= soon.as_millis() as u64),
        }
    }

    /// Bring the volume back after a fade that ended in a pause.
    fn release_fade(&mut self) {
        self.fade_held = false;
        self.fade = 1.0;
        self.push_volume();
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
        // However music got wanted again, the sleep timer's fade does not outlive the pause.
        if self.fade_held
            && self
                .driver
                .as_ref()
                .is_some_and(|d| d.coordinator().intent() == Intent::Playing)
        {
            self.release_fade();
        }
        if let Some(id) = self.current_video() {
            if !self.art.contains_key(&id) {
                self.art.insert(id.clone(), ArtSlot::Loading);
                if let Some(source) = self.art_source.clone() {
                    let (key, owned) = (id.clone(), id.clone());
                    cx.spawn(async move |this, cx| {
                        let decoded = cx
                            .background_executor()
                            .spawn(async move {
                                source.fetch(&key).map(|d| {
                                    let cover = d.cover_png().map(Arc::from);
                                    (d, cover)
                                })
                            })
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
        self.persist();
        self.report();
        self.top_up_radio(cx);
        cx.notify();
    }

    // ---- saving the queue --------------------------------------------------

    /// Hand the saver what changed: the whole queue when it is different from what was saved
    /// (or when nothing is saved yet and something is playing), otherwise only the position, at
    /// most every [`CHECKPOINT_EVERY`] while playing and at once after a pause or a seek.
    fn persist(&mut self) {
        let (Some(saver), Some(d)) = (self.saver.as_ref(), self.driver.as_ref()) else {
            return;
        };
        let c = d.coordinator();
        let queue = c.queue();
        let fp = queue_fingerprint(queue);
        let now = Instant::now();
        let playing = c.intent() == Intent::Playing;
        let position = c.position_ms();
        if self.saved_fp != Some(fp) || (!self.saved_row && playing) {
            saver.save(queue.snapshot(), position);
            self.saved_fp = Some(fp);
            self.saved_row = true;
            self.last_checkpoint = now;
        } else if self.saved_row
            && (self.checkpoint_now
                || (playing && now.duration_since(self.last_checkpoint) >= CHECKPOINT_EVERY))
        {
            saver.checkpoint(position);
            self.last_checkpoint = now;
        }
        self.checkpoint_now = false;
    }

    // ---- autoplay ----------------------------------------------------------

    /// When the queue is about to run out (and the user is listening), ask for a radio seeded by
    /// the last track and queue it behind everything else. Repeat-all wins: it loops the queue,
    /// so there is nothing to run out of.
    fn top_up_radio(&mut self, cx: &mut Context<Self>) {
        if !self.autoplay || self.radio_inflight {
            return;
        }
        let (Some(d), Some(library)) = (self.driver.as_ref(), self.library.clone()) else {
            return;
        };
        let c = d.coordinator();
        let queue = c.queue();
        if c.intent() != Intent::Playing
            || queue.repeat() == Repeat::All
            || queue.upcoming().take(RADIO_LOW_WATER).count() >= RADIO_LOW_WATER
        {
            return;
        }
        let Some(seed) = queue_tail(queue) else {
            return;
        };
        if self.radio_tried.as_deref() == Some(seed.as_str()) {
            return;
        }
        self.radio_inflight = true;
        self.radio_tried = Some(seed.clone());
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn({
                    let seed = seed.clone();
                    async move { library.radio(&seed) }
                })
                .await;
            let _ = this.update(cx, |m, cx| m.radio_loaded(seed, found, cx));
        })
        .detach();
    }

    fn radio_loaded(
        &mut self,
        seed: String,
        found: Result<Vec<crate::library::TrackRow>, apollo_domain::listing::LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        self.radio_inflight = false;
        let Ok(rows) = found else {
            self.retry_radio_later(cx);
            return;
        };
        let Some(d) = self.driver.as_ref() else { return };
        let queue = d.coordinator().queue();
        // The queue moved on while the radio was coming (a new list, a repeat-all, autoplay
        // turned off): this answer is for a tail that is no longer the tail.
        if !self.autoplay || queue.repeat() == Repeat::All || queue_tail(queue) != Some(seed) {
            return;
        }
        let seed_title = queue
            .upcoming()
            .last()
            .or_else(|| queue.current())
            .map(|e| e.track.title.clone())
            .unwrap_or_default();
        let mut known: HashSet<String> = queue
            .history()
            .iter()
            .chain(queue.current())
            .chain(queue.upcoming())
            .map(|e| e.track.video_id.as_str().to_owned())
            .collect();
        let mut tracks = Vec::new();
        for row in rows {
            if tracks.len() == RADIO_APPEND {
                break;
            }
            if known.insert(row.track.video_id.as_str().to_owned()) {
                self.hint_art(row.track.video_id.as_str(), row.thumbnail.as_deref());
                tracks.push(row.track);
            }
        }
        if tracks.is_empty() {
            self.retry_radio_later(cx);
            return;
        }
        // Autoplay is not something the user did: it is not an undo step.
        self.edit_queue_untracked(cx, |q| {
            q.append_autoplay(tracks, &seed_title);
        });
        self.prefetch_upcoming();
    }

    /// Forget that the tail was tried after [`RADIO_RETRY`], and look again then.
    fn retry_radio_later(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(RADIO_RETRY).await;
            let _ = this.update(cx, |m, cx| {
                m.radio_tried = None;
                m.sync(cx);
            });
        })
        .detach();
    }

    fn art_loaded(
        &mut self,
        id: String,
        decoded: Option<(DecodedArt, Option<Arc<[u8]>>)>,
        cx: &mut Context<Self>,
    ) {
        let slot = decoded.map(|(d, cover)| ArtImages {
            image: d.render_image(),
            ambient: d.ambient_image(),
            accent: d.accent,
            cover,
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

    // ---- OS media keys and overlay -----------------------------------------

    /// What the OS media overlay shows. Cheaper than [`view`](Self::view): no artwork images,
    /// no accent, called on every update.
    pub fn media_state(&self) -> MediaState {
        let Some(d) = self.driver.as_ref() else {
            return MediaState::default();
        };
        let c = d.coordinator();
        let Some(entry) = c.queue().current() else {
            return MediaState::default();
        };
        let track = &entry.track;
        let id = track.video_id.as_str();
        MediaState {
            track: Some(MediaTrack {
                video_id: id.to_owned(),
                title: track.title.clone(),
                artist: artist_line(track).to_string(),
                album: track
                    .album
                    .as_ref()
                    .map(|a| a.title.clone())
                    .unwrap_or_default(),
                duration_ms: c
                    .duration_ms()
                    .or_else(|| track.duration.map(|d| d.as_millis() as u64)),
            }),
            playing: c.intent() == Intent::Playing,
            position_ms: c.position_ms(),
            cover: match self.art.get(id) {
                Some(ArtSlot::Ready(Some(images))) => images.cover.clone(),
                _ => None,
            },
        }
    }

    /// A media key or overlay button. Same coordinator events as the on-screen controls.
    pub fn os_command(&mut self, command: OsCommand, cx: &mut Context<Self>) {
        let Some(d) = self.driver.as_ref() else {
            return;
        };
        let c = d.coordinator();
        let playing = c.intent() == Intent::Playing;
        let seek_to = |ms: u64| Event::Seek {
            position_ms: c.duration_ms().map_or(ms, |end| ms.min(end)),
        };
        let event = match command {
            // Redundant requests (Play while playing) are dropped rather than replayed.
            OsCommand::Play => (!playing).then_some(Event::Play),
            OsCommand::Pause | OsCommand::Stop => playing.then_some(Event::Pause),
            OsCommand::Toggle => Some(if playing { Event::Pause } else { Event::Play }),
            OsCommand::Next => Some(Event::Next),
            OsCommand::Previous => Some(Event::Previous),
            OsCommand::SeekTo(ms) => Some(seek_to(ms)),
            OsCommand::SeekBy(delta) => {
                Some(seek_to(c.position_ms().saturating_add_signed(delta)))
            }
        };
        if let Some(event) = event {
            self.handle(event, cx);
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
            autoplay: self.autoplay,
            volume: self.volume,
            muted: self.muted,
            error: None,
            sleep: self.sleep.choice(),
            sleep_minutes: self.sleep.minutes_left(Instant::now()),
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
            heading: q.heading().map(|h| match h {
                Heading::Source(s) => QueueHeading::Source(s.clone()),
                Heading::Autoplay(seed) => QueueHeading::Autoplay(seed.to_owned()),
                Heading::Yours => QueueHeading::Yours,
            }),
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
        track: entry.track.clone(),
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

/// The video that plays last as the queue stands: the last one up next, else the current one.
fn queue_tail(queue: &Queue) -> Option<String> {
    queue
        .upcoming()
        .last()
        .or_else(|| queue.current())
        .map(|e| e.track.video_id.as_str().to_owned())
}

/// A fingerprint of everything the saved queue holds, so a changed queue is told from a moved
/// position without copying it.
fn queue_fingerprint(queue: &Queue) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in queue.history() {
        e.id.get().hash(&mut h);
    }
    queue.current().map(|e| e.id.get()).hash(&mut h);
    for e in queue.upcoming() {
        e.id.get().hash(&mut h);
    }
    (queue.shuffle(), queue.repeat() as u8).hash(&mut h);
    // The header's place is saved with the queue, so a new place alone is a change.
    match queue.heading() {
        None => 0u8.hash(&mut h),
        Some(Heading::Yours) => 1u8.hash(&mut h),
        Some(Heading::Autoplay(seed)) => (2u8, seed).hash(&mut h),
        Some(Heading::Source(s)) => (3u8, &s.name, &s.target, s.kind as u8).hash(&mut h),
    }
    h.finish()
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
