//! OS media keys and the SMTC overlay, headless: the commands a key press becomes, applied to
//! the real player model with the mock adapters, and the state the overlay would be shown.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, Phase};
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct TestThumbs;
impl ThumbSource for TestThumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(100.0).render(32)))
    }
}

/// Three songs for any query; "nothing" finds nothing and "fail" fails.
struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        match query {
            "nothing" => Ok(SearchResults::default()),
            "fail" => Err(LoadFailure::Network),
            _ => Ok(SearchResults {
                songs: (0..3)
                    .map(|i| ListedTrack {
                        track: Track::new(format!("found{i:06}"), format!("Found {i}")),
                        occurrence: None,
                        thumbnail: Some(format!("https://t/{i}")),
                        explicit: false,
                        liked: None,
                    })
                    .collect(),
                ..SearchResults::default()
            }),
        }
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
        search: Arc::new(TestSearch),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(apollo_app::library::sim::SimLibrary::instant()),
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
    let ready = prepared();
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
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

use apollo_app::os_media::OsCommand;

fn intent(player: &Entity<PlayerModel>, cx: &mut VisualTestContext) -> Intent {
    player.read_with(cx, |m, _| m.coordinator().unwrap().intent())
}

fn title(player: &Entity<PlayerModel>, cx: &mut VisualTestContext) -> String {
    player.read_with(cx, |m, _| m.view().title.to_string())
}

fn position(player: &Entity<PlayerModel>, cx: &mut VisualTestContext) -> u64 {
    player.read_with(cx, |m, _| m.coordinator().unwrap().position_ms())
}

fn command(player: &Entity<PlayerModel>, cx: &mut VisualTestContext, c: OsCommand) {
    player.update(cx, |m, cx| m.os_command(c, cx));
}

#[gpui::test]
fn play_and_pause_commands_set_the_intent_and_are_idempotent(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    assert_eq!(intent(&player, vcx), Intent::Paused);
    command(&player, vcx, OsCommand::Pause);
    assert_eq!(intent(&player, vcx), Intent::Paused, "Pause while paused is a no-op");
    command(&player, vcx, OsCommand::Play);
    assert_eq!(intent(&player, vcx), Intent::Playing);
    open_current_load(&player, vcx);
    command(&player, vcx, OsCommand::Play);
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().phase()),
        Phase::Playing,
        "Play while playing does not restart the load"
    );
    command(&player, vcx, OsCommand::Pause);
    assert_eq!(intent(&player, vcx), Intent::Paused);
    command(&player, vcx, OsCommand::Toggle);
    assert_eq!(intent(&player, vcx), Intent::Playing);
    command(&player, vcx, OsCommand::Stop);
    assert_eq!(intent(&player, vcx), Intent::Paused);
}

#[gpui::test]
fn next_and_previous_commands_move_through_the_queue(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    assert_eq!(title(&player, vcx), "Track 0");
    command(&player, vcx, OsCommand::Next);
    assert_eq!(title(&player, vcx), "Track 1");
    command(&player, vcx, OsCommand::Next);
    assert_eq!(title(&player, vcx), "Track 2");
    command(&player, vcx, OsCommand::Previous);
    assert_eq!(title(&player, vcx), "Track 1");
}

#[gpui::test]
fn seek_commands_clamp_to_the_track(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    command(&player, vcx, OsCommand::Play);
    open_current_load(&player, vcx);
    command(&player, vcx, OsCommand::SeekTo(60_000));
    assert_eq!(position(&player, vcx), 60_000);
    command(&player, vcx, OsCommand::SeekBy(-10_000));
    assert_eq!(position(&player, vcx), 50_000);
    command(&player, vcx, OsCommand::SeekBy(-500_000));
    assert_eq!(position(&player, vcx), 0);
    command(&player, vcx, OsCommand::SeekTo(9_000_000));
    assert_eq!(position(&player, vcx), 200_000, "past the end lands on the end");
}

#[gpui::test]
fn the_overlay_state_follows_the_player(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    vcx.run_until_parked();
    let state = player.read_with(vcx, |m, _| m.media_state());
    let track = state.track.expect("a track is loaded");
    assert_eq!((track.title.as_str(), track.video_id.as_str()), ("Track 0", "track000000"));
    assert!(!state.playing);
    let cover = state.cover.expect("the artwork has loaded and been encoded for the overlay");
    assert!(cover.starts_with(b"\x89PNG"), "the cover is a PNG");

    command(&player, vcx, OsCommand::Play);
    assert!(player.read_with(vcx, |m, _| m.media_state().playing));
    command(&player, vcx, OsCommand::Next);
    vcx.run_until_parked();
    let state = player.read_with(vcx, |m, _| m.media_state());
    assert_eq!(state.track.unwrap().title, "Track 1");
    assert!(state.playing, "next keeps playing");
}

#[gpui::test]
fn before_the_backend_installs_there_is_nothing_to_show(cx: &mut TestAppContext) {
    let player = cx.new(|_| PlayerModel::new());
    let state = player.read_with(cx, |m, _| m.media_state());
    assert!(state.track.is_none() && !state.playing && state.cover.is_none());
    player.update(cx, |m, cx| m.os_command(OsCommand::Next, cx));
}
