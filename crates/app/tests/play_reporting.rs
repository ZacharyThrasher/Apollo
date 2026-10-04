//! The player model hands the play reporter what the player is doing, only while "Add plays to
//! history" is on, and never for the simulation (which installs no reporter).

use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::reporting::{Activity, PlayReporter, Snapshot};
use apollo_app::search::SearchSource;
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, LeaseId};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
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

#[derive(Default)]
struct Recording {
    seen: Mutex<Vec<Option<Snapshot>>>,
    cancels: Mutex<u32>,
}

impl PlayReporter for Recording {
    fn observe(&self, snapshot: Option<Snapshot>) {
        self.seen.lock().unwrap().push(snapshot);
    }
    fn cancel(&self) {
        *self.cancels.lock().unwrap() += 1;
    }
}

impl Recording {
    fn activities(&self) -> Vec<Option<Activity>> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.as_ref().map(|s| s.activity))
            .collect()
    }
}

fn install(cx: &mut TestAppContext, reporter: Option<Arc<Recording>>) -> Entity<PlayerModel> {
    cx.update(|cx| cx.set_reduce_motion(true));
    let player = cx.new(|_| PlayerModel::new());
    if let Some(r) = reporter {
        player.update(cx, |m, _| {
            m.set_play_reporter(Some(r));
            m.set_report_plays(true);
        });
    }
    let prepared = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new("track000001", "Song"),
        }],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    player.update(cx, |m, cx| m.install(prepared, cx));
    player
}

/// Press play and let the engine report audio: resolved, opened, then a position past zero.
fn start_audio(player: &Entity<PlayerModel>, cx: &mut TestAppContext, position_ms: u64) {
    player.update(cx, |m, cx| m.toggle_play(cx));
    let load = player.read_with(cx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    for event in [
        Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        },
        Event::Engine {
            load,
            event: EngineEvent::Opened {
                duration_ms: Some(200_000),
            },
        },
        Event::Engine {
            load,
            event: EngineEvent::Position(position_ms),
        },
    ] {
        player.update(cx, |m, cx| m.handle(event, cx));
    }
}

#[gpui::test]
fn nothing_is_handed_over_until_audio_is_playing(cx: &mut TestAppContext) {
    let reporter = Arc::new(Recording::default());
    let player = install(cx, Some(reporter.clone()));
    // Asking to play is not playing.
    player.update(cx, |m, cx| m.toggle_play(cx));
    let activities = reporter.activities();
    assert!(
        activities.iter().all(|a| *a != Some(Activity::Playing)),
        "{activities:?}"
    );
    let load = player.read_with(cx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    player.update(cx, |m, cx| {
        m.handle(
            Event::Resolved {
                load,
                result: Ok(LeaseId(1)),
            },
            cx,
        )
    });
    assert!(
        reporter.activities().iter().all(|a| *a != Some(Activity::Playing)),
        "resolved is not playing either"
    );
}

#[gpui::test]
fn audio_playing_is_handed_over_with_the_track_and_position(cx: &mut TestAppContext) {
    let reporter = Arc::new(Recording::default());
    let player = install(cx, Some(reporter.clone()));
    start_audio(&player, cx, 1_500);
    let seen = reporter.seen.lock().unwrap();
    let last = seen.last().unwrap().as_ref().unwrap();
    assert_eq!(last.activity, Activity::Playing);
    assert_eq!(last.video_id, "track000001");
    assert_eq!(last.position_ms, 1_500);
    assert_eq!(last.duration_ms, Some(200_000));
    assert_eq!(last.volume, 70);
    assert!(!last.muted);
}

#[gpui::test]
fn pausing_is_handed_over(cx: &mut TestAppContext) {
    let reporter = Arc::new(Recording::default());
    let player = install(cx, Some(reporter.clone()));
    start_audio(&player, cx, 1_500);
    player.update(cx, |m, cx| m.toggle_play(cx));
    assert_eq!(
        reporter.activities().last().copied().flatten(),
        Some(Activity::Paused)
    );
}

#[gpui::test]
fn the_setting_off_hands_nothing_over_and_cancels(cx: &mut TestAppContext) {
    let reporter = Arc::new(Recording::default());
    let player = install(cx, Some(reporter.clone()));
    start_audio(&player, cx, 1_500);
    let before = reporter.seen.lock().unwrap().len();
    assert!(before > 0);

    player.update(cx, |m, _| m.set_report_plays(false));
    assert_eq!(*reporter.cancels.lock().unwrap(), 1);
    player.update(cx, |m, cx| m.toggle_play(cx));
    player.update(cx, |m, cx| m.toggle_play(cx));
    assert_eq!(
        reporter.seen.lock().unwrap().len(),
        before,
        "off means no snapshots at all, so no pings"
    );

    // Turning it back on picks up whatever is going on then.
    player.update(cx, |m, _| m.set_report_plays(true));
    player.update(cx, |m, cx| m.toggle_play(cx));
    assert!(reporter.seen.lock().unwrap().len() > before);
    assert_eq!(*reporter.cancels.lock().unwrap(), 1, "turning on cancels nothing");
}

#[gpui::test]
fn off_is_the_state_until_the_app_says_otherwise(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let reporter = Arc::new(Recording::default());
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, _| m.set_play_reporter(Some(reporter.clone())));
    let prepared = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new("track000001", "Song"),
        }],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    player.update(cx, |m, cx| m.install(prepared, cx));
    start_audio(&player, cx, 1_500);
    assert!(reporter.seen.lock().unwrap().is_empty());
}

#[gpui::test]
fn without_a_reporter_playing_just_works(cx: &mut TestAppContext) {
    // The simulation, tests and the headless harness install none.
    let player = install(cx, None);
    player.update(cx, |m, _| m.set_report_plays(true));
    start_audio(&player, cx, 1_500);
    assert!(player.read_with(cx, |m, _| m.view().playing));
}

// ---- the Settings row ---------------------------------------------------------------------

#[derive(Default)]
struct Memory(Mutex<std::collections::HashMap<String, String>>);

impl apollo_app::settings::SettingsBackend for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
}

#[gpui::test]
fn settings_playback_has_an_add_plays_to_history_row_that_is_on_and_flips(cx: &mut TestAppContext) {
    use apollo_app::account::AccountModel;
    use apollo_app::settings::{SettingsModel, ADD_PLAYS_TO_HISTORY};
    use apollo_app::style::Fonts;
    use apollo_app::ui::nav::Route;
    use apollo_app::ui::shell::{self, Shell};

    cx.update(|cx| {
        cx.set_reduce_motion(true);
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let backend = Arc::new(Memory::default());
    let player = cx.new(|_| PlayerModel::new());
    let settings = cx.new(|_| SettingsModel::new(Some(backend.clone())));
    let (shell, vcx) = cx.add_window_view({
        let (player, settings) = (player.clone(), settings.clone());
        move |_, cx| {
            let ports = apollo_app::backend::account::ports();
            apollo_app::backend::account::start(&ports, true);
            let account = cx.new(|cx| AccountModel::new(ports, cx));
            account.update(cx, |m, cx| m.startup_finished(cx));
            Shell::with_settings(player, account, settings, cx)
        }
    });
    shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();

    let gapless = vcx.debug_bounds("setting-gapless-playback").expect("gapless row");
    let row = vcx.debug_bounds("setting-add-plays-to-history").expect("history row");
    assert_eq!(row.size.height, gpui::px(48.0), "a settings row is 48px");
    assert!(row.top() >= gapless.bottom(), "last row of Playback, in the same card");
    assert_eq!(row.left(), gapless.left());
    let switch = vcx
        .debug_bounds("setting-add-plays-to-history-switch")
        .expect("its switch");
    assert!(switch.left() > row.center().x && switch.right() <= row.right());
    assert!(
        settings.read_with(vcx, |m, _| m.get(&ADD_PLAYS_TO_HISTORY)),
        "on by default"
    );

    vcx.simulate_click(row.center(), gpui::Modifiers::none());
    vcx.run_until_parked();
    assert!(!settings.read_with(vcx, |m, _| m.get(&ADD_PLAYS_TO_HISTORY)));
    vcx.simulate_click(row.center(), gpui::Modifiers::none());
    vcx.run_until_parked();
    assert!(settings.read_with(vcx, |m, _| m.get(&ADD_PLAYS_TO_HISTORY)));
}
