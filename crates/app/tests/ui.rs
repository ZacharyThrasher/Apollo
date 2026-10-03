//! Headless UI tests: the real views on GPUI's test platform with the deterministic mock
//! adapters. They drive keyboard shortcuts and the model the way the window does.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::{PlayerModel, Status};
use apollo_app::search::{SearchSource, SearchState, DEBOUNCE};
use apollo_app::settings::VolumeStore;
use apollo_app::style::Fonts;
use apollo_app::thumbs::ThumbSource;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, Phase};
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Focusable, TestAppContext, VisualTestContext};

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

fn type_query(vcx: &mut VisualTestContext, keys: &str) {
    vcx.simulate_keystrokes(keys);
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
}

fn field_text(shell: &Entity<Shell>, vcx: &mut VisualTestContext) -> String {
    let field = shell.read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.read_with(vcx, |f, _| f.text().to_owned())
}

fn queue_titles(
    player: &Entity<PlayerModel>,
    vcx: &mut VisualTestContext,
) -> (String, Vec<String>) {
    player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        (
            q.current()
                .map(|e| e.track.title.clone())
                .unwrap_or_default(),
            q.manual().iter().map(|e| e.track.title.clone()).collect(),
        )
    })
}

#[gpui::test]
fn ctrl_k_and_ctrl_f_open_search_with_the_field_focused(cx: &mut TestAppContext) {
    for key in ["ctrl-k", "ctrl-f"] {
        let (_player, shell, vcx) = setup(cx);
        shell.read_with(vcx, |s, _| assert_eq!(s.route(), Route::Home));
        vcx.simulate_keystrokes(key);
        shell.read_with(vcx, |s, _| assert_eq!(s.route(), Route::Search));
        let focused = shell.update_in(vcx, |s, window, cx| {
            let field = s.search_page().read(cx).field().clone();
            field.read(cx).focus_handle(cx).is_focused(window)
        });
        assert!(focused, "{key} focuses the field");
    }
}

#[gpui::test]
fn typing_in_the_field_searches_and_a_space_is_a_space_not_play(cx: &mut TestAppContext) {
    let (player, shell, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    type_query(vcx, "d a f t space p u n k");
    assert_eq!(field_text(&shell, vcx), "daft punk");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Paused,
        "Space in the field must not toggle playback"
    );
    let model = shell.read_with(vcx, |s, _| s.search_model().clone());
    assert_eq!(model.read_with(vcx, |m, _| m.state()), SearchState::Ready);
    assert_eq!(
        model.read_with(vcx, |m, _| m.query().to_owned()),
        "daft punk"
    );
}

#[gpui::test]
fn down_enters_the_songs_enter_plays_and_ctrl_enter_queues(cx: &mut TestAppContext) {
    let (player, shell, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    type_query(vcx, "d a f t");
    vcx.simulate_keystrokes("down");
    let selected = shell.read_with(vcx, |s, cx| s.search_page().read(cx).selected());
    assert_eq!(selected, Some(0));
    vcx.simulate_keystrokes("down");
    vcx.simulate_keystrokes("ctrl-enter");
    let (current, manual) = queue_titles(&player, vcx);
    assert_eq!(current, "Track 0", "queueing does not interrupt what plays");
    assert_eq!(manual, ["Found 1"]);

    vcx.simulate_keystrokes("enter");
    let (current, manual) = queue_titles(&player, vcx);
    assert_eq!(current, "Found 1", "Enter plays the selected song now");
    assert_eq!(manual, ["Found 1"], "what was already queued stays queued");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing
    );
}

#[gpui::test]
fn up_from_the_first_song_returns_to_the_field_and_escape_clears_then_leaves(
    cx: &mut TestAppContext,
) {
    let (player, shell, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    type_query(vcx, "d a f t");
    vcx.simulate_keystrokes("down up");
    assert_eq!(
        shell.read_with(vcx, |s, cx| s.search_page().read(cx).selected()),
        None
    );
    vcx.simulate_keystrokes("x");
    assert_eq!(
        field_text(&shell, vcx),
        "daftx",
        "typing resumes in the field"
    );

    vcx.simulate_keystrokes("escape");
    assert_eq!(field_text(&shell, vcx), "", "first Escape clears");
    vcx.simulate_keystrokes("escape space");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing,
        "second Escape leaves the field, so Space plays again"
    );
}

#[gpui::test]
fn no_results_and_failures_are_distinct_states(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup(cx);
    let model = shell.read_with(vcx, |s, _| s.search_model().clone());
    vcx.simulate_keystrokes("ctrl-k");
    type_query(vcx, "n o t h i n g");
    assert!(model.read_with(vcx, |m, _| m.is_confirmed_empty()));
    vcx.simulate_keystrokes("escape");
    type_query(vcx, "f a i l");
    assert_eq!(
        model.read_with(vcx, |m, _| m.state()),
        SearchState::Failed(LoadFailure::Network)
    );
}

#[gpui::test]
fn the_search_page_renders_every_state_without_panicking(cx: &mut TestAppContext) {
    let (_player, _shell, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    // Idle, loading (skeletons), results, empty, failed: each draws a frame.
    vcx.run_until_parked();
    vcx.simulate_keystrokes("d a f t");
    vcx.run_until_parked();
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape n o t h i n g");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape f a i l");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
}

#[test]
fn the_minimum_window_fits_both_side_blocks_and_the_centre() {
    // 240 + 320 + 240 plus the bar padding and the gaps between the columns.
    assert_eq!(apollo_app::ui::player_bar::MIN_BAR_WIDTH, 864.0);
}
