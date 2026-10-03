//! Headless UI tests: the real views on GPUI's test platform with the deterministic mock
//! adapters. They drive keyboard shortcuts and the model the way the window does.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::{PlayerModel, Status};
use apollo_app::search::{SearchSource, SearchState, DEBOUNCE};
use apollo_app::settings::VolumeStore;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
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

#[gpui::test]
fn the_player_bar_columns_fit_the_window_and_the_transport_stays_centred(cx: &mut TestAppContext) {
    let (_player, _shell, vcx) = setup(cx);
    for width in [864.0_f32, 1100.0, 1600.0] {
        vcx.simulate_resize(gpui::size(gpui::px(width), gpui::px(720.0)));
        vcx.run_until_parked();
        let (left, centre, right) = (
            bounds(vcx, "now-playing-toggle"),
            bounds(vcx, "player-bar-center"),
            bounds(vcx, "player-bar-right"),
        );
        let (left_edge, right_edge) = (f32::from(left.left()), f32::from(right.right()));
        assert!(left_edge >= 15.5, "{width}: left block starts inside the bar padding");
        assert!(
            right_edge <= width - 15.5,
            "{width}: the right block ends at {right_edge}, past the bar padding"
        );
        assert!(centre.left() >= left.right() && centre.right() <= right.left());
        let mid = f32::from(centre.left() + centre.size.width / 2.0);
        assert!(
            (mid - width / 2.0).abs() < 1.0,
            "{width}: transport centre {mid} is off the window centre"
        );
        assert_eq!(left.size.width, right.size.width, "{width}: side blocks are equal");
    }
}

#[gpui::test]
fn a_listings_thumbnail_reaches_the_art_source_for_tracks_it_was_not_told_about(
    cx: &mut TestAppContext,
) {
    use std::sync::Mutex;
    struct Hints(Mutex<Vec<(String, String)>>);
    impl ArtSource for Hints {
        fn fetch(&self, _: &str) -> Option<DecodedArt> {
            None
        }
        fn hint(&self, video_id: &str, url: &str) {
            self.0.lock().unwrap().push((video_id.into(), url.into()));
        }
    }
    let hints = Arc::new(Hints(Mutex::default()));
    let player = cx.new(|_| PlayerModel::new());
    let mut ready = prepared();
    ready.art = hints.clone();
    cx.update(|cx| player.update(cx, |m, cx| m.install(ready, cx)));
    player.read_with(cx, |m, _| {
        m.hint_art("abcdefghijk", Some("https://t/a"));
        m.hint_art("nothumbnail", None);
    });
    assert_eq!(
        *hints.0.lock().unwrap(),
        [("abcdefghijk".to_owned(), "https://t/a".to_owned())]
    );
}

// ---- queue panel ---------------------------------------------------------------------------

use apollo_app::ui::now_playing::art_side;
use apollo_domain::model::QueueEntryId;
use gpui::{Bounds, Modifiers, Pixels, Point};

fn titles(player: &Entity<PlayerModel>, cx: &mut VisualTestContext) -> Vec<String> {
    player.read_with(cx, |m, _| {
        m.queue_view()
            .upcoming
            .iter()
            .map(|r| r.title.to_string())
            .collect()
    })
}

fn current_title(player: &Entity<PlayerModel>, cx: &mut VisualTestContext) -> String {
    player.read_with(cx, |m, _| m.view().title.to_string())
}

fn bounds(cx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    cx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"))
}

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    b.center()
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let at = center(bounds(cx, selector));
    cx.simulate_click(at, Modifiers::none());
}

/// Shell with the queue panel open. Animations off, so the panel is fully open at once.
fn setup_with_queue(
    cx: &mut TestAppContext,
) -> (Entity<PlayerModel>, Entity<Shell>, &mut VisualTestContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (player, shell, vcx) = setup(cx);
    shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    (player, shell, vcx)
}

#[gpui::test]
fn the_queue_button_slides_the_panel_in_at_320px(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_player, shell, vcx) = setup(cx);
    assert!(vcx.debug_bounds("queue-panel").is_none());
    shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    assert!(shell.read_with(vcx, |s, _| s.queue_open()));
    assert_eq!(bounds(vcx, "queue-panel").size.width, gpui::px(320.0));
    shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("queue-panel").is_none(),
        "unmounted when closed"
    );
}

#[gpui::test]
fn the_panel_lists_the_playing_row_and_what_is_up_next(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    // Items: Current, "Up next" header, two upcoming rows.
    for ix in [0, 2, 3] {
        let selector: &'static str = ["queue-row-0", "", "queue-row-2", "queue-row-3"][ix];
        assert!(vcx.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(
        vcx.debug_bounds("queue-row-1").is_none(),
        "headers are not rows"
    );
    assert_eq!(titles(&player, vcx), ["Track 1", "Track 2"]);
}

#[gpui::test]
fn clicking_an_up_next_row_plays_it(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    click(vcx, "queue-row-3");
    assert_eq!(current_title(&player, vcx), "Track 2");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing
    );
    // Track 0 and 1 were skipped into history, in order.
    let history: Vec<String> = player.read_with(vcx, |m, _| {
        m.queue_view()
            .history
            .iter()
            .map(|r| r.title.to_string())
            .collect()
    });
    assert_eq!(history, ["Track 0", "Track 1"]);
}

#[gpui::test]
fn the_remove_button_removes_the_row_and_does_not_play_it(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    let over = center(bounds(vcx, "queue-row-2"));
    vcx.simulate_mouse_move(over, None, Modifiers::none());
    click(vcx, "queue-remove-2");
    assert_eq!(titles(&player, vcx), ["Track 2"]);
    assert_eq!(current_title(&player, vcx), "Track 0");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Paused,
        "removing is not a play"
    );
}

#[gpui::test]
fn the_clear_button_empties_up_next_and_keeps_the_current_track(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    click(vcx, "queue-clear");
    vcx.run_until_parked();
    assert!(titles(&player, vcx).is_empty());
    assert_eq!(current_title(&player, vcx), "Track 0");
    // The empty line replaces the rows.
    assert!(vcx.debug_bounds("queue-row-2").is_none());
}

#[gpui::test]
fn the_list_is_keyboard_driven(cx: &mut TestAppContext) {
    let (player, shell, vcx) = setup_with_queue(cx);
    let panel = shell.read_with(vcx, |s, _| s.queue_panel().clone());
    vcx.update(|window, cx| {
        let focus = gpui::Focusable::focus_handle(panel.read(cx), cx);
        window.focus(&focus, cx);
    });
    let id_of = |player: &Entity<PlayerModel>, cx: &mut VisualTestContext, i: usize| {
        player.read_with(cx, |m, _| m.queue_view().upcoming[i].id)
    };
    let cursor = |panel: &Entity<apollo_app::ui::queue_panel::QueuePanel>,
                  cx: &mut VisualTestContext| {
        panel.read_with(cx, |p, _| p.cursor())
    };

    // First Down shows the cursor on the playing row; the next one moves down.
    vcx.simulate_keystrokes("down");
    let current = player.read_with(vcx, |m, _| m.queue_view().current.unwrap().id);
    assert_eq!(cursor(&panel, vcx), Some(current));
    vcx.simulate_keystrokes("down");
    let first = id_of(&player, vcx, 0);
    assert_eq!(cursor(&panel, vcx), Some(first));

    // Alt+Down moves that row below the next one, and the cursor follows it.
    vcx.simulate_keystrokes("alt-down");
    assert_eq!(titles(&player, vcx), ["Track 2", "Track 1"]);
    assert_eq!(cursor(&panel, vcx), Some(first));
    // Alt+Up at the top of up next does nothing more than move it back.
    vcx.simulate_keystrokes("alt-up alt-up");
    assert_eq!(titles(&player, vcx), ["Track 1", "Track 2"]);

    // Delete removes the cursor row and the cursor takes the next one.
    vcx.simulate_keystrokes("delete");
    assert_eq!(titles(&player, vcx), ["Track 2"]);
    assert_eq!(cursor(&panel, vcx), Some(id_of(&player, vcx, 0)));

    // Enter plays the cursor row.
    vcx.simulate_keystrokes("enter");
    assert_eq!(current_title(&player, vcx), "Track 2");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing
    );
}

#[gpui::test]
fn dragging_an_up_next_row_onto_another_reorders_it(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    let from = center(bounds(vcx, "queue-row-3"));
    let to = center(bounds(vcx, "queue-row-2"));
    vcx.simulate_mouse_down(from, gpui::MouseButton::Left, Modifiers::none());
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        vcx.simulate_mouse_move(
            gpui::point(from.x + (to.x - from.x) * t, from.y + (to.y - from.y) * t),
            gpui::MouseButton::Left,
            Modifiers::none(),
        );
    }
    vcx.simulate_mouse_up(to, gpui::MouseButton::Left, Modifiers::none());
    assert_eq!(titles(&player, vcx), ["Track 2", "Track 1"]);
}

#[gpui::test]
fn queue_edits_through_the_model_keep_playback_and_report_the_view(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup(cx);
    let ids: Vec<QueueEntryId> = player.read_with(vcx, |m, _| {
        m.queue_view().upcoming.iter().map(|r| r.id).collect()
    });
    player.update(vcx, |m, cx| m.move_upcoming(ids[1], 0, cx));
    assert_eq!(titles(&player, vcx), ["Track 2", "Track 1"]);
    player.update(vcx, |m, cx| m.remove_entry(ids[0], cx));
    assert_eq!(titles(&player, vcx), ["Track 2"]);
    player.update(vcx, |m, cx| m.clear_upcoming(cx));
    assert!(titles(&player, vcx).is_empty());
    assert_eq!(current_title(&player, vcx), "Track 0");
    // Shuffle and repeat state come from the queue the panel shows.
    player.update(vcx, |m, cx| m.toggle_shuffle(cx));
    assert!(player.read_with(vcx, |m, _| m.view().shuffle));
}

#[gpui::test]
fn row_artwork_loads_lazily_for_rows_on_screen(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    vcx.run_until_parked();
    let (a, b) = player.read_with(vcx, |m, _| {
        let v = m.queue_view();
        (
            m.thumb(&v.upcoming[0].video_id).is_some(),
            m.thumb(&v.upcoming[1].video_id).is_some(),
        )
    });
    assert!(a && b, "visible rows get thumbnails");
}

// ---- Now Playing ---------------------------------------------------------------------------

#[gpui::test]
fn the_player_bar_opens_now_playing_and_escape_closes_it(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup(cx);
    assert!(!shell.read_with(vcx, |s, _| s.now_playing_open()));
    click(vcx, "now-playing-toggle");
    vcx.run_until_parked();
    assert!(shell.read_with(vcx, |s, _| s.now_playing_open()));
    let art = bounds(vcx, "now-playing-art");
    let viewport_h = vcx.update(|window, _| f32::from(window.viewport_size().height));
    assert_eq!(f32::from(art.size.width), art_side(viewport_h));
    assert_eq!(art.size.width, art.size.height, "square");

    // The shell root keeps the keyboard, so Escape closes it.
    vcx.simulate_keystrokes("escape");
    assert!(!shell.read_with(vcx, |s, _| s.now_playing_open()));
    assert!(vcx.debug_bounds("now-playing-art").is_none());
}

#[gpui::test]
fn now_playing_has_the_ambient_art_and_its_own_working_transport(cx: &mut TestAppContext) {
    let (player, shell, vcx) = setup(cx);
    assert!(
        player.read_with(vcx, |m, _| m.view().ambient.is_some()),
        "the blurred artwork arrives with the art"
    );
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    // Its play button is the same control as the bar's: clicking toggles playback.
    vcx.simulate_keystrokes("space");
    assert_eq!(
        player.read_with(vcx, |m, _| m.coordinator().unwrap().intent()),
        Intent::Playing
    );
}

#[gpui::test]
fn the_player_bar_block_is_inert_without_a_track(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    vcx.run_until_parked();
    click(vcx, "now-playing-toggle");
    assert!(!shell.read_with(vcx, |s, _| s.now_playing_open()));
}

// ---- layout at the smallest window ------------------------------------------------------------

#[gpui::test]
fn at_the_minimum_window_now_playing_and_the_queue_do_not_overlap(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup_with_queue(cx);
    vcx.simulate_resize(gpui::size(gpui::px(864.0), gpui::px(480.0)));
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    let art = bounds(vcx, "now-playing-art");
    let panel = bounds(vcx, "queue-panel");
    assert!(art.right() <= panel.left(), "art {art:?} panel {panel:?}");
    assert!(art.left() >= gpui::px(64.0), "clear of the nav rail");
    assert_eq!(panel.size.width, gpui::px(320.0));
    assert_eq!(art.size.width, art.size.height);
}

#[gpui::test]
fn now_playing_shows_its_progress_row_and_transport_without_scrolling(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup(cx);
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    for (w, h) in [(864.0_f32, 480.0_f32), (864.0, 560.0), (1100.0, 720.0), (1400.0, 900.0)] {
        vcx.simulate_resize(gpui::size(gpui::px(w), gpui::px(h)));
        vcx.run_until_parked();
        let (art, controls) = (bounds(vcx, "now-playing-art"), bounds(vcx, "np-controls"));
        let page_bottom = gpui::px(h - 80.0);
        assert!(
            controls.bottom() <= page_bottom,
            "{w}x{h}: transport ends at {:?}, past the page's end {page_bottom:?}",
            controls.bottom()
        );
        assert!(art.top() >= gpui::px(32.0), "{w}x{h}: art starts under the title bar");
        assert!(art.size.width >= gpui::px(160.0));
    }
}

#[gpui::test]
fn queue_rows_fit_inside_the_panel(cx: &mut TestAppContext) {
    let (_player, _shell, vcx) = setup_with_queue(cx);
    let panel = bounds(vcx, "queue-panel");
    let row = bounds(vcx, "queue-row-2");
    assert!(row.left() >= panel.left() && row.right() <= panel.right());
    assert_eq!(row.size.height, gpui::px(48.0));
}

#[gpui::test]
fn the_autoplay_switch_sits_right_of_the_list_controls_and_flips_the_setting(cx: &mut TestAppContext) {
    let (player, _shell, vcx) = setup_with_queue(cx);
    let panel = bounds(vcx, "queue-panel");
    let clear = bounds(vcx, "queue-clear");
    let switch = bounds(vcx, "queue-autoplay");
    assert!(switch.left() >= clear.right(), "after shuffle, repeat and clear");
    assert!(switch.right() <= panel.right(), "inside the panel");
    assert_eq!(switch.size.height, gpui::px(32.0));
    assert!(player.read_with(vcx, |m, _| m.view().autoplay), "on by default");

    click(vcx, "queue-autoplay");
    assert!(!player.read_with(vcx, |m, _| m.view().autoplay));
    click(vcx, "queue-autoplay");
    assert!(player.read_with(vcx, |m, _| m.view().autoplay));
}

#[gpui::test]
fn settings_has_a_playback_section_whose_switch_flips_normalization(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (player, shell, vcx) = setup(cx);
    shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();

    let row = bounds(vcx, "setting-normalize-volume");
    let switch = bounds(vcx, "normalize-volume");
    assert_eq!(row.size.height, gpui::px(48.0), "a settings row is 48px");
    assert!(switch.left() > row.center().x, "the switch is on the right of its label");
    assert!(switch.right() <= row.right());
    assert!(player.read_with(vcx, |m, _| m.normalize()), "on by default");

    click(vcx, "normalize-volume");
    assert!(!player.read_with(vcx, |m, _| m.normalize()));
    click(vcx, "normalize-volume");
    assert!(player.read_with(vcx, |m, _| m.normalize()));
}
