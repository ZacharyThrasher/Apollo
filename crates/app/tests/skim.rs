//! Skim: audition a list, each song from a third in for a few seconds. The real shell on GPUI's
//! test platform with the deterministic mock adapters; the engine's answers are sent by hand.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::{PlayerModel, SkimView};
use apollo_app::reporting::{PlayReporter, Snapshot};
use apollo_app::search::SearchSource;
use apollo_app::settings::AUTO_ADVANCE;
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, Phase};
use apollo_domain::queue::Repeat;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

struct NoArt;
impl ArtSource for NoArt {
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
}

impl PlayReporter for Recording {
    fn observe(&self, snapshot: Option<Snapshot>) {
        self.seen.lock().unwrap().push(snapshot);
    }
    fn cancel(&self) {}
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    reporter: Arc<Recording>,
}

/// Skim's window in these tests, in milliseconds (the setting's default).
const WINDOW: u64 = 20_000;
/// Every skimmed song is three minutes long, so each starts a minute in.
const SONG_MS: u64 = 180_000;
const START: u64 = 60_000;

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let reporter = Arc::new(Recording::default());
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: [("before0001", "Before"), ("before0002", "Before two")]
            .into_iter()
            .map(|(id, title)| LibraryTrack {
                track: Track::new(id, title),
            })
            .collect(),
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.set_play_reporter(Some(reporter.clone()));
        m.set_report_plays(true);
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (
        Page {
            player,
            shell,
            reporter,
        },
        vcx,
    )
}

fn song(id: &str, title: &str) -> Track {
    let mut track = Track::new(id, title);
    track.duration = Some(Duration::from_millis(SONG_MS));
    track
}

fn list() -> Vec<Track> {
    vec![
        song("skim0000001", "One"),
        song("skim0000002", "Two"),
        song("skim0000003", "Three"),
    ]
}

fn start(page: &Page, vcx: &mut VisualTestContext) {
    page.player
        .update(vcx, |m, cx| m.start_skim(list(), None, cx));
}

fn title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player
        .read_with(vcx, |m, _| m.view().title.to_string())
}

fn skim(page: &Page, vcx: &mut VisualTestContext) -> Option<SkimView> {
    page.player.read_with(vcx, |m, _| m.view().skim)
}

fn state(page: &Page, vcx: &mut VisualTestContext) -> (Intent, Phase, u64) {
    page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        (c.intent(), c.phase(), c.position_ms())
    })
}

/// The engine opens the current load (a song of `SONG_MS`) and lands on the requested start.
fn open_at(page: &Page, vcx: &mut VisualTestContext, position_ms: u64) {
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    page.player.update(vcx, |m, cx| {
        m.handle(
            Event::Resolved {
                load,
                result: Ok(LeaseId(1)),
            },
            cx,
        );
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Opened {
                    duration_ms: Some(SONG_MS),
                },
            },
            cx,
        );
        // The open asked to start at `position_ms`; the engine confirms whichever seek it was.
        for gen in 1..=40 {
            m.handle(
                Event::Engine {
                    load,
                    event: EngineEvent::Seeked { gen, position_ms },
                },
                cx,
            );
        }
    });
}

fn at(page: &Page, vcx: &mut VisualTestContext, position_ms: u64) {
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    page.player.update(vcx, |m, cx| {
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Position(position_ms),
            },
            cx,
        );
        m.skim_tick(cx);
    });
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

#[gpui::test]
fn each_song_starts_a_third_in_and_the_next_follows_the_window(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    assert_eq!(
        skim(&page, vcx),
        Some(SkimView {
            index: 1,
            total: 3,
            start_ms: START,
            window_ms: WINDOW
        })
    );
    assert_eq!(title(&page, vcx), "One");
    assert_eq!(state(&page, vcx).2, START, "asked to open a third in");
    open_at(&page, vcx, START);
    assert_eq!(state(&page, vcx).1, Phase::Playing);

    at(&page, vcx, START + 10_000);
    assert_eq!(title(&page, vcx), "One", "still inside the window");
    at(&page, vcx, START + WINDOW);
    assert_eq!(title(&page, vcx), "Two");
    assert_eq!(skim(&page, vcx).map(|s| s.index), Some(2));
    assert_eq!(state(&page, vcx).2, START, "the next one is opened a third in too");
}

#[gpui::test]
fn skim_plays_the_list_in_order_with_shuffle_and_repeat_off(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player.update(vcx, |m, cx| {
        m.toggle_shuffle(cx);
        m.cycle_repeat(cx);
    });
    start(&page, vcx);
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        assert!(!q.shuffle());
        assert_eq!(q.repeat(), Repeat::Off);
        assert_eq!(q.upcoming().count(), 2);
    });
}

#[gpui::test]
fn the_progress_bar_is_the_window_and_a_seek_lands_inside_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    open_at(&page, vcx, START);
    at(&page, vcx, START + 5_000);
    let view = skim(&page, vcx).unwrap();
    assert_eq!(view.in_window(START + 5_000), (5_000, WINDOW));
    assert_eq!(view.to_song(WINDOW / 2), START + WINDOW / 2);
}

#[gpui::test]
fn k_sets_the_song_aside_and_the_skim_goes_on(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // With auto-advance off K only sets the song aside; the skim goes on by its own clock.
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |m, cx| m.set(&AUTO_ADVANCE, false, cx));
    start(&page, vcx);
    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("k");
    assert_eq!(crate_ids(&page, vcx), vec!["skim0000001".to_owned()]);
    assert!(skim(&page, vcx).is_some(), "it carries on");
    assert_eq!(title(&page, vcx), "One");
    // Skimming on, then K again on the next song.
    at(&page, vcx, START + WINDOW);
    vcx.simulate_keystrokes("k");
    assert_eq!(
        crate_ids(&page, vcx),
        vec!["skim0000001".to_owned(), "skim0000002".to_owned()]
    );
}

#[gpui::test]
fn with_auto_advance_k_also_moves_on_to_the_next_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("k");
    assert_eq!(crate_ids(&page, vcx), vec!["skim0000001".to_owned()]);
    assert_eq!(title(&page, vcx), "Two", "K kept One and went to the next song");
    assert!(skim(&page, vcx).is_some(), "the skim carries on");

    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("k");
    assert_eq!(title(&page, vcx), "Three");
    assert_eq!(
        crate_ids(&page, vcx),
        vec!["skim0000001".to_owned(), "skim0000002".to_owned()]
    );

    // The last song: K sets it aside and stays; a key that only collects does not end the skim.
    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("k");
    assert_eq!(crate_ids(&page, vcx).len(), 3);
    assert_eq!(title(&page, vcx), "Three");
    assert!(skim(&page, vcx).is_some(), "still skimming");
}

#[gpui::test]
fn enter_keeps_the_song_and_plays_it_from_the_start_as_a_normal_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    open_at(&page, vcx, START);
    at(&page, vcx, START + WINDOW);
    assert_eq!(title(&page, vcx), "Two");
    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("enter");
    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Two");
    let (intent, _, position) = state(&page, vcx);
    assert_eq!(intent, Intent::Playing);
    assert_eq!(position, 0, "from the start");
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        assert_eq!(q.context().len(), 1, "the rest of the list plays on after it");
        assert!(m.can_undo_queue(), "Ctrl+Z goes back to what was queued before");
    });
}

#[gpui::test]
fn esc_puts_back_what_was_playing_before(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    page.player.update(vcx, |m, cx| {
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
                event: EngineEvent::Position(5_000),
            },
        ] {
            m.handle(event, cx);
        }
    });
    assert_eq!(title(&page, vcx), "Before");

    start(&page, vcx);
    open_at(&page, vcx, START);
    assert_eq!(title(&page, vcx), "One");
    vcx.simulate_keystrokes("escape");

    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Before");
    page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        assert_eq!(c.queue().context().len(), 1, "the old queue is back");
        assert_eq!(c.intent(), Intent::Playing, "it was playing, so it plays");
        assert_eq!(c.position_ms(), 5_000, "where it was");
        assert!(!m.can_undo_queue(), "skimming left no undo step");
    });
}

#[gpui::test]
fn esc_after_a_skim_started_while_paused_leaves_it_paused(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    vcx.simulate_keystrokes("escape");
    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Before");
    assert_eq!(state(&page, vcx).0, Intent::Paused);
}

#[gpui::test]
fn the_skim_keys_do_nothing_when_not_skimming(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let before = state(&page, vcx);
    vcx.simulate_keystrokes("k enter");
    assert!(crate_ids(&page, vcx).is_empty());
    assert_eq!(state(&page, vcx), before);
    assert_eq!(title(&page, vcx), "Before");
}

#[gpui::test]
fn skimmed_songs_are_never_reported(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let before = page.reporter.seen.lock().unwrap().len();
    start(&page, vcx);
    open_at(&page, vcx, START);
    at(&page, vcx, START + 3_000);
    at(&page, vcx, START + WINDOW);
    open_at(&page, vcx, START);
    at(&page, vcx, START + 3_000);
    let seen = page.reporter.seen.lock().unwrap();
    assert!(
        seen[before..].iter().all(Option::is_none),
        "only 'nothing loaded' reached the reporter: {seen:?}"
    );
}

#[gpui::test]
fn keeping_a_song_makes_it_a_normal_play_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("enter");
    open_at(&page, vcx, 0);
    at(&page, vcx, 4_000);
    let seen = page.reporter.seen.lock().unwrap();
    assert!(
        seen.iter().any(Option::is_some),
        "after Enter it is a play like any other"
    );
}

#[gpui::test]
fn playing_another_list_ends_the_skim_and_leaves_that_list_alone(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    page.player.update(vcx, |m, cx| {
        m.play_tracks(vec![song("other000001", "Other")], None, None, cx)
    });
    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Other");
    page.player.update(vcx, |m, cx| {
        m.skim_tick(cx);
    });
    assert_eq!(title(&page, vcx), "Other");
}

#[gpui::test]
fn picking_a_song_of_the_skimmed_list_moves_the_skim_there(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    page.player
        .update(vcx, |m, cx| m.play_tracks_from(list(), Some(2), None, None, cx));
    assert_eq!(title(&page, vcx), "Three");
    let view = skim(&page, vcx).expect("still skimming");
    assert_eq!(view.label(), "Skimming 3 of 3");
    assert_eq!(view.start_ms, START, "from its window start");
    open_at(&page, vcx, START);
    at(&page, vcx, START + WINDOW);
    assert!(skim(&page, vcx).is_none(), "the list ran out after the last song");
    assert_eq!(title(&page, vcx), "Before", "and Esc's before is still what comes back");
}

#[gpui::test]
fn a_pick_keeps_what_esc_restores_and_stays_out_of_history(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let before = page.reporter.seen.lock().unwrap().len();
    start(&page, vcx);
    page.player
        .update(vcx, |m, cx| m.play_tracks_from(list(), Some(1), None, None, cx));
    open_at(&page, vcx, START);
    at(&page, vcx, START + 3_000);
    {
        let seen = page.reporter.seen.lock().unwrap();
        assert!(
            seen[before..].iter().all(Option::is_none),
            "only 'nothing loaded' reached the reporter: {seen:?}"
        );
    }
    vcx.simulate_keystrokes("escape");
    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Before");
}

#[gpui::test]
fn a_pick_in_the_middle_can_go_back_to_earlier_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    page.player
        .update(vcx, |m, cx| m.play_tracks_from(list(), Some(2), None, None, cx));
    page.player.update(vcx, |m, cx| m.previous(cx));
    assert_eq!(title(&page, vcx), "Two");
    assert_eq!(skim(&page, vcx).unwrap().label(), "Skimming 2 of 3");
    page.player.update(vcx, |m, cx| {
        m.skim_tick(cx);
    });
    assert!(skim(&page, vcx).is_some(), "earlier songs are still the skim's own");
}

#[gpui::test]
fn start_skim_at_begins_at_that_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player
        .update(vcx, |m, cx| m.start_skim_at(list(), 1, None, cx));
    assert_eq!(title(&page, vcx), "Two");
    assert_eq!(skim(&page, vcx).unwrap().label(), "Skimming 2 of 3");
}

#[gpui::test]
fn the_end_of_the_list_goes_back_to_what_was_playing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    for _ in 0..2 {
        open_at(&page, vcx, START);
        at(&page, vcx, START + WINDOW);
    }
    assert_eq!(title(&page, vcx), "Three");
    open_at(&page, vcx, START);
    at(&page, vcx, START + WINDOW);
    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Before");
}

#[gpui::test]
fn next_and_previous_move_between_windows(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    open_at(&page, vcx, START);
    vcx.simulate_keystrokes("shift-right");
    assert_eq!(title(&page, vcx), "Two");
    assert_eq!(state(&page, vcx).2, START);
    assert_eq!(skim(&page, vcx).map(|s| s.index), Some(2));
    vcx.simulate_keystrokes("shift-left");
    assert_eq!(title(&page, vcx), "One");
    assert_eq!(state(&page, vcx).2, START, "its window start, not the song's start");
}

#[gpui::test]
fn undo_does_not_touch_a_skim(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    vcx.simulate_keystrokes("ctrl-z");
    assert!(skim(&page, vcx).is_some());
    assert_eq!(title(&page, vcx), "One");
}

#[gpui::test]
fn switching_skim_off_ends_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    start(&page, vcx);
    page.player.update(vcx, |m, cx| m.set_skim_enabled(false, cx));
    assert!(skim(&page, vcx).is_none());
    assert_eq!(title(&page, vcx), "Before");
    start(&page, vcx);
    assert!(skim(&page, vcx).is_none(), "off means no skim");
}

#[gpui::test]
fn the_pill_says_where_the_skim_is(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(vcx.debug_bounds("skim-pill").is_none());
    start(&page, vcx);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("skim-pill").is_some());
    assert_eq!(skim(&page, vcx).unwrap().label(), "Skimming 1 of 3");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("skim-pill").is_none());
}

// ---- Settings > Playback -----------------------------------------------------------------------

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, gpui::Modifiers::none());
    vcx.run_until_parked();
}

#[gpui::test]
fn settings_playback_has_a_skim_switch_and_a_seconds_select(cx: &mut TestAppContext) {
    use apollo_app::settings::SKIM;
    use apollo_app::ui::nav::Route;
    let (page, vcx) = setup(cx);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));

    let switch = vcx.debug_bounds("setting-skim").expect("a Skim row");
    let seconds = vcx
        .debug_bounds("setting-skim-seconds")
        .expect("a Seconds per song row");
    assert!(seconds.top() >= switch.bottom(), "the select sits under the switch");
    assert!(settings.read_with(vcx, |s, _| s.get(&SKIM)), "on by default");
    assert_eq!(settings.read_with(vcx, |s, _| s.skim_seconds()), 20);

    click(vcx, "setting-skim");
    assert!(!settings.read_with(vcx, |s, _| s.get(&SKIM)));
    click(vcx, "setting-skim");
    assert!(settings.read_with(vcx, |s, _| s.get(&SKIM)));

    click(vcx, "setting-skim-seconds");
    let menu = page.shell.read_with(vcx, |s, _| s.choices().clone());
    let labels: Vec<String> = menu.read_with(vcx, |m, _| {
        m.labels().iter().map(|l| l.to_string()).collect()
    });
    assert_eq!(labels, ["10 seconds", "20 seconds", "30 seconds"]);
    assert_eq!(menu.read_with(vcx, |m, _| m.cursor()), Some(1));
    click(vcx, "choice-menu-item-2");
    assert_eq!(settings.read_with(vcx, |s, _| s.skim_seconds()), 30);
}
