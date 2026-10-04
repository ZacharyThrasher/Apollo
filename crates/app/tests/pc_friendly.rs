//! Playing nicely with the PC, in the player model: the output going away pauses in place and
//! the next Play reopens the track where it stopped; the sleep timer fades the engine's volume
//! (never the setting) and pauses; another app's sound pauses without resuming by itself.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::sleep_timer::SleepChoice;
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, LoadId, Phase};
use apollo_provider::SearchResults;
use apollo_runtime::{MediaEngine, ResolveRequest, Resolver};
use gpui::{AppContext, Entity, TestAppContext};

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}
struct NoThumbs;
impl ThumbSource for NoThumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}
struct NoSearch;
impl SearchSource for NoSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    Open { load: LoadId, start_ms: u64 },
    Pause(LoadId),
    Resume(LoadId),
    Close(LoadId),
    Volume(u8),
}

#[derive(Default)]
struct Log {
    calls: Vec<Call>,
    resolves: Vec<ResolveRequest>,
}

struct Recorder(Arc<Mutex<Log>>);

impl Resolver for Recorder {
    fn start(&mut self, request: ResolveRequest) {
        self.0.lock().unwrap().resolves.push(request);
    }
    fn cancel(&mut self, _: LoadId) {}
}

impl MediaEngine for Recorder {
    fn open(&mut self, load: LoadId, _: LeaseId, _: bool, start_ms: u64) {
        self.0.lock().unwrap().calls.push(Call::Open { load, start_ms });
    }
    fn pause(&mut self, load: LoadId) {
        self.0.lock().unwrap().calls.push(Call::Pause(load));
    }
    fn resume(&mut self, load: LoadId) {
        self.0.lock().unwrap().calls.push(Call::Resume(load));
    }
    fn seek(&mut self, _: LoadId, _: u64, _: u64) {}
    fn close(&mut self, load: LoadId) {
        self.0.lock().unwrap().calls.push(Call::Close(load));
    }
    fn stop(&mut self) {}
    fn set_volume(&mut self, percent: u8) {
        self.0.lock().unwrap().calls.push(Call::Volume(percent));
    }
}

struct Rig {
    player: Entity<PlayerModel>,
    log: Arc<Mutex<Log>>,
}

impl Rig {
    fn new(cx: &mut TestAppContext) -> Self {
        cx.update(|cx| cx.set_reduce_motion(true));
        let log = Arc::new(Mutex::new(Log::default()));
        let player = cx.new(|_| PlayerModel::new());
        let tracks = ["track000001", "track000002", "track000003"]
            .iter()
            .map(|id| LibraryTrack {
                track: Track::new(*id, "Song"),
            })
            .collect();
        let prepared = Prepared {
            label: "test",
            ports: Ports {
                resolver: Box::new(Recorder(log.clone())),
                engine: Box::new(Recorder(log.clone())),
            },
            queue: tracks,
            art: Arc::new(NoArt),
            search: Arc::new(NoSearch),
            thumbs: Arc::new(NoThumbs),
            library: Arc::new(SimLibrary::instant()),
        };
        player.update(cx, |m, cx| m.install(prepared, cx));
        Rig { player, log }
    }

    fn send(&self, cx: &mut TestAppContext, event: Event) {
        self.player.update(cx, |m, cx| m.handle(event, cx));
    }

    /// The load of the most recent resolve request.
    fn last_resolve(&self) -> ResolveRequest {
        self.log.lock().unwrap().resolves.last().cloned().expect("a resolve")
    }

    /// Plays the first track to `position_ms` of 200 s and returns its load.
    fn play(&self, cx: &mut TestAppContext, position_ms: u64) -> LoadId {
        self.send(cx, Event::Play);
        let load = self.last_resolve().load;
        self.send(
            cx,
            Event::Resolved {
                load,
                result: Ok(LeaseId(1)),
            },
        );
        self.send(
            cx,
            Event::Engine {
                load,
                event: EngineEvent::Opened {
                    duration_ms: Some(200_000),
                },
            },
        );
        self.send(
            cx,
            Event::Engine {
                load,
                event: EngineEvent::Position(position_ms),
            },
        );
        load
    }

    fn calls(&self) -> Vec<Call> {
        self.log.lock().unwrap().calls.clone()
    }

    fn volumes(&self) -> Vec<u8> {
        self.calls()
            .into_iter()
            .filter_map(|c| match c {
                Call::Volume(v) => Some(v),
                _ => None,
            })
            .collect()
    }

    fn intent(&self, cx: &mut TestAppContext) -> Intent {
        self.player
            .read_with(cx, |m, _| m.coordinator().unwrap().intent())
    }

    fn phase(&self, cx: &mut TestAppContext) -> Phase {
        self.player
            .read_with(cx, |m, _| m.coordinator().unwrap().phase())
    }

    fn position(&self, cx: &mut TestAppContext) -> u64 {
        self.player
            .read_with(cx, |m, _| m.coordinator().unwrap().position_ms())
    }

    fn tick(&self, cx: &mut TestAppContext, at: Instant) {
        self.player.update(cx, |m, cx| {
            m.sleep_tick_at(at, cx);
        });
    }
}

#[gpui::test]
fn the_output_going_away_pauses_in_place_and_play_reopens_at_the_same_position(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    let load = rig.play(cx, 73_000);
    assert_eq!(rig.intent(cx), Intent::Playing);

    rig.player.update(cx, |m, cx| m.output_lost(cx));
    assert_eq!(rig.intent(cx), Intent::Paused);
    assert_eq!(rig.phase(cx), Phase::Paused);
    assert_eq!(rig.position(cx), 73_000);
    assert!(rig.calls().contains(&Call::Pause(load)));
    // Nothing skipped, nothing lost, nothing to shout about.
    let (queue_len, view) = rig.player.read_with(cx, |m, _| {
        (m.coordinator().unwrap().queue().upcoming_ids().len(), m.view())
    });
    assert_eq!(queue_len, 2);
    assert!(view.error.is_none());
    assert!(!view.playing);
    assert_eq!(view.position_ms, 73_000);

    // Play opens the track again from the same spot.
    rig.player.update(cx, |m, cx| m.toggle_play(cx));
    let again = rig.last_resolve();
    assert_ne!(again.load, load);
    assert_eq!(again.resume_ms, 73_000);
    assert!(rig.calls().contains(&Call::Close(load)));
    rig.send(
        cx,
        Event::Resolved {
            load: again.load,
            result: Ok(LeaseId(2)),
        },
    );
    assert!(rig.calls().contains(&Call::Open {
        load: again.load,
        start_ms: 73_000
    }));
}

#[gpui::test]
fn the_output_going_away_with_nothing_playing_does_nothing(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.player.update(cx, |m, cx| m.output_lost(cx));
    assert_eq!(rig.phase(cx), Phase::Idle);
    assert!(rig.calls().iter().all(|c| matches!(c, Call::Volume(_))));
}

#[gpui::test]
fn other_audio_pauses_music_and_never_resumes_it(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    // Paused: nothing to do, and no Pause sent.
    rig.player.update(cx, |m, cx| m.other_audio(cx));
    assert!(!rig.calls().iter().any(|c| matches!(c, Call::Pause(_))));

    let load = rig.play(cx, 10_000);
    rig.player.update(cx, |m, cx| m.other_audio(cx));
    assert_eq!(rig.intent(cx), Intent::Paused);
    assert_eq!(rig.calls().iter().filter(|c| **c == Call::Pause(load)).count(), 1);
    assert_eq!(rig.position(cx), 10_000);
    // The same stretch of sound again does not do anything more, and nothing resumes.
    rig.player.update(cx, |m, cx| m.other_audio(cx));
    assert_eq!(rig.calls().iter().filter(|c| **c == Call::Pause(load)).count(), 1);
    assert!(!rig.calls().contains(&Call::Resume(load)));
    // Pausing for another app is a plain pause: Play just resumes the stream.
    rig.player.update(cx, |m, cx| m.toggle_play(cx));
    assert!(rig.calls().contains(&Call::Resume(load)));
}

fn secs(s: f32) -> Duration {
    Duration::from_secs_f32(s)
}

#[gpui::test]
fn the_sleep_timer_fades_the_engine_not_the_setting_then_pauses(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let load = rig.play(cx, 1_000);
    let t0 = Instant::now();
    rig.player
        .update(cx, |m, cx| m.set_sleep_at(SleepChoice::Minutes(15), t0, cx));
    assert_eq!(
        rig.player.read_with(cx, |m, _| m.view().sleep_minutes),
        Some(15)
    );
    let end = t0 + Duration::from_secs(15 * 60);

    // Before the end: nothing touches the volume.
    rig.tick(cx, end - secs(1.0));
    let before = rig.volumes();

    rig.tick(cx, end + secs(2.5));
    let mid = *rig.volumes().last().unwrap();
    assert!((33..=37).contains(&mid), "{mid} of 70");
    // The user's slider still says what the user set.
    assert_eq!(rig.player.read_with(cx, |m, _| m.view().volume), 70);
    assert_eq!(rig.intent(cx), Intent::Playing);

    rig.tick(cx, end + secs(5.0));
    // Silent first, then paused; the timer is over.
    let calls = rig.calls();
    let silent = calls
        .iter()
        .rposition(|c| *c == Call::Volume(0))
        .expect("faded to silence");
    let paused = calls
        .iter()
        .rposition(|c| *c == Call::Pause(load))
        .expect("paused");
    assert!(silent < paused, "{calls:?}");
    assert_eq!(rig.intent(cx), Intent::Paused);
    assert_eq!(
        rig.player.read_with(cx, |m, _| m.sleep_choice()),
        SleepChoice::Off
    );
    assert_eq!(rig.player.read_with(cx, |m, _| m.view().volume), 70);
    assert!(before.len() < rig.volumes().len());

    // Wanting music again brings the volume back before the stream resumes.
    rig.player.update(cx, |m, cx| m.toggle_play(cx));
    let calls = rig.calls();
    let back = calls
        .iter()
        .rposition(|c| *c == Call::Volume(70))
        .expect("volume restored");
    let resumed = calls
        .iter()
        .rposition(|c| *c == Call::Resume(load))
        .expect("resumed");
    assert!(back < resumed, "{calls:?}");
}

#[gpui::test]
fn turning_the_timer_off_mid_fade_brings_the_volume_back(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    rig.play(cx, 1_000);
    let t0 = Instant::now();
    rig.player
        .update(cx, |m, cx| m.set_sleep_at(SleepChoice::Minutes(15), t0, cx));
    let end = t0 + Duration::from_secs(15 * 60);
    rig.tick(cx, end + secs(3.0));
    assert!(*rig.volumes().last().unwrap() < 70);
    rig.player
        .update(cx, |m, cx| m.set_sleep_at(SleepChoice::Off, end + secs(3.1), cx));
    assert_eq!(*rig.volumes().last().unwrap(), 70);
    assert_eq!(rig.intent(cx), Intent::Playing);
    // And the timer does nothing more.
    rig.tick(cx, end + secs(60.0));
    assert_eq!(*rig.volumes().last().unwrap(), 70);
    assert_eq!(rig.intent(cx), Intent::Playing);
}

#[gpui::test]
fn pausing_in_the_middle_of_a_fade_leaves_the_volume_where_the_user_set_it(
    cx: &mut TestAppContext,
) {
    let rig = Rig::new(cx);
    rig.play(cx, 1_000);
    let t0 = Instant::now();
    rig.player
        .update(cx, |m, cx| m.set_sleep_at(SleepChoice::Minutes(15), t0, cx));
    let end = t0 + Duration::from_secs(15 * 60);
    rig.tick(cx, end + secs(2.0));
    rig.player.update(cx, |m, cx| m.toggle_play(cx)); // the user pauses
    rig.tick(cx, end + secs(2.2));
    assert_eq!(*rig.volumes().last().unwrap(), 70);
    assert_eq!(
        rig.player.read_with(cx, |m, _| m.sleep_choice()),
        SleepChoice::Off
    );
}

#[gpui::test]
fn end_of_track_fades_over_the_last_seconds_and_pauses_before_the_end(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    let load = rig.play(cx, 100_000);
    let t0 = Instant::now();
    rig.player
        .update(cx, |m, cx| m.set_sleep_at(SleepChoice::EndOfTrack, t0, cx));
    assert_eq!(
        rig.player.read_with(cx, |m, _| m.view().sleep_minutes),
        None,
        "no countdown label for end of track"
    );
    rig.tick(cx, t0);
    assert!(rig.volumes().iter().all(|v| *v == 70), "{:?}", rig.volumes());

    fn position(rig: &Rig, cx: &mut TestAppContext, load: LoadId, ms: u64) {
        rig.send(
            cx,
            Event::Engine {
                load,
                event: EngineEvent::Position(ms),
            },
        )
    }
    position(&rig, cx, load, 197_500);
    rig.tick(cx, t0);
    let mid = *rig.volumes().last().unwrap();
    assert!((25..=45).contains(&mid), "{mid}");
    assert_eq!(rig.intent(cx), Intent::Playing);

    position(&rig, cx, load, 199_800);
    rig.tick(cx, t0);
    assert_eq!(rig.intent(cx), Intent::Paused);
    assert!(rig.calls().contains(&Call::Pause(load)));
    assert_eq!(rig.position(cx), 199_800);
}

#[gpui::test]
fn the_sleep_choice_and_minutes_reach_the_view(cx: &mut TestAppContext) {
    let rig = Rig::new(cx);
    assert_eq!(
        rig.player.read_with(cx, |m, _| m.view().sleep),
        SleepChoice::Off
    );
    let t0 = Instant::now();
    rig.player
        .update(cx, |m, cx| m.set_sleep_at(SleepChoice::Minutes(45), t0, cx));
    let v = rig.player.read_with(cx, |m, _| m.view());
    assert_eq!(v.sleep, SleepChoice::Minutes(45));
    assert!(matches!(v.sleep_minutes, Some(44..=45)));
}
