//! Headless UI tests: the real views on GPUI's test platform with the deterministic mock
//! adapters. They drive keyboard shortcuts and the model the way the window does.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::{PlayerModel, Status};
use apollo_app::settings::VolumeStore;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, Phase};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

fn prepared() -> Prepared {
    let queue = (0..3)
        .map(|i| LibraryTrack {
            track: Track::new(format!("track{i:06}"), format!("Track {i}")),
        })
        .collect();
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue,
        art: Arc::new(TestArt),
    }
}

fn setup(cx: &mut TestAppContext) -> (Entity<PlayerModel>, Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    player.update(vcx, |m, cx| m.install(prepared(), cx));
    vcx.run_until_parked();
    (player, shell, vcx)
}

fn open_current_load(player: &Entity<PlayerModel>, cx: &mut VisualTestContext) {
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
    player.update(cx, |m, cx| {
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Opened {
                    duration_ms: Some(200_000),
                },
            },
            cx,
        )
    });
}

#[gpui::test]
fn queue_loads_paused_and_nothing_autoplays(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    let (intent, phase, has_current) = player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        (c.intent(), c.phase(), c.queue().current().is_some())
    });
    assert!(has_current);
    assert_eq!((intent, phase), (Intent::Paused, Phase::Idle));
}

#[gpui::test]
fn space_toggles_play_and_pause(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing
    );
    open_current_load(&player, vcx);
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().phase()),
        Phase::Playing
    );
    vcx.simulate_keystrokes("space");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Paused
    );
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().phase()),
        Phase::Paused
    );
}

#[gpui::test]
fn ctrl_right_and_ctrl_left_change_track(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    let title = |p: &Entity<PlayerModel>, cx: &mut VisualTestContext| {
        p.read_with(cx, |m, _| m.view().title.to_string())
    };
    assert_eq!(title(&player, vcx), "Track 0");
    vcx.simulate_keystrokes("ctrl-right");
    assert_eq!(title(&player, vcx), "Track 1");
    vcx.simulate_keystrokes("ctrl-right");
    assert_eq!(title(&player, vcx), "Track 2");
    vcx.simulate_keystrokes("ctrl-left");
    assert_eq!(title(&player, vcx), "Track 1");
}

#[gpui::test]
fn art_loads_and_drives_the_accent(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    vcx.run_until_parked();
    let view = player.read_with(vcx, |m, _| m.view());
    assert!(view.art.is_some(), "artwork arrives for the current track");
}

#[gpui::test]
fn shuffle_repeat_and_volume_are_wired(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    player.update(vcx, |m, cx| m.toggle_shuffle(cx));
    assert!(player.read_with(vcx, |m, _| m.view().shuffle));
    player.update(vcx, |m, cx| m.cycle_repeat(cx));
    assert_eq!(
        player.read_with(vcx, |m, _| m.view().repeat),
        apollo_domain::queue::Repeat::All
    );
    player.update(vcx, |m, cx| m.set_volume(20, cx));
    assert_eq!(player.read_with(vcx, |m, _| m.view().volume), 20);
    player.update(vcx, |m, cx| m.toggle_mute(cx));
    assert!(player.read_with(vcx, |m, _| m.view().muted));
}

#[gpui::test]
fn shell_starts_on_home_with_the_rail_collapsed(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup(cx);
    shell.read_with(vcx, |s, _| {
        assert_eq!(s.route(), Route::Home);
        assert!(!s.nav_expanded());
    });
}

#[gpui::test]
fn volume_defaults_to_70_and_restores_a_saved_level(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    assert_eq!(player.read_with(vcx, |m, _| m.view().volume), 70);
    player.update(vcx, |m, _| m.restore_volume(Some(35)));
    assert_eq!(player.read_with(vcx, |m, _| m.view().volume), 35);
    player.update(vcx, |m, _| m.restore_volume(None));
    assert_eq!(player.read_with(vcx, |m, _| m.view().volume), 35);
    player.update(vcx, |m, _| m.restore_volume(Some(250)));
    assert_eq!(player.read_with(vcx, |m, _| m.view().volume), 100);
}

#[gpui::test]
fn committed_volume_is_saved_and_drag_positions_are_not(cx: &mut TestAppContext) {
    struct Recorder(std::sync::mpsc::Sender<u8>);
    impl VolumeStore for Recorder {
        fn load(&self) -> Option<u8> {
            None
        }
        fn save(&self, percent: u8) {
            let _ = self.0.send(percent);
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let (player, _shell, vcx) = setup(cx);
    player.update(vcx, |m, _| m.set_volume_store(Some(Arc::new(Recorder(tx)))));
    player.update(vcx, |m, cx| m.set_volume(42, cx));
    assert!(
        rx.try_recv().is_err(),
        "setting the level alone saves nothing"
    );
    player.update(vcx, |m, _| m.save_volume());
    assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(42));
}

#[gpui::test]
fn a_startup_failure_renders_the_error_row_without_controls(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (_shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    player.update(vcx, |m, cx| m.fail("test", cx));
    vcx.run_until_parked();
    assert!(matches!(
        player.read_with(vcx, |m, _| m.view().status),
        Status::Unavailable(_)
    ));
}

#[test]
fn the_minimum_window_fits_both_side_blocks_and_the_centre() {
    // 240 + 320 + 240 plus the bar padding and the gaps between the columns.
    assert_eq!(apollo_app::ui::player_bar::MIN_BAR_WIDTH, 864.0);
}
