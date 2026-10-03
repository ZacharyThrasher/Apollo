//! Headless UI tests: the real views on GPUI's test platform with the deterministic mock
//! adapters. They drive keyboard shortcuts and the model the way the window does.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::PlayerModel;
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
