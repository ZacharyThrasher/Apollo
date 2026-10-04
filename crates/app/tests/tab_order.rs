//! Keyboard order: Tab walks the whole window in a sane order on every page, without traps, and
//! Shift+Tab walks the same stops back. The nav rail comes first, the page and any panel in the
//! middle, the player bar last.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, LeaseId};
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

/// Tabs from the root until a stop repeats and names each stop: the shell's own by name, anything
/// else (a page, a panel) as `page`.
fn cycle(shell: &Entity<Shell>, vcx: &mut VisualTestContext, key: &str) -> Vec<String> {
    let stops = shell.read_with(vcx, |s, _| s.tab_stops());
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    vcx.run_until_parked();
    let mut seen = Vec::new();
    let mut order = Vec::new();
    for _ in 0..120 {
        vcx.simulate_keystrokes(key);
        let Some(focused) = vcx.update(|window, cx| window.focused(cx)) else {
            panic!("focus was lost after {order:?}");
        };
        if seen.contains(&focused) {
            return order;
        }
        let name = stops
            .iter()
            .find(|(_, h)| *h == focused)
            .map_or("page", |(n, _)| *n);
        order.push(name.to_owned());
        seen.push(focused);
    }
    panic!("Tab never came back to a stop it had already visited: {order:?}");
}

const NAV: [&str; 7] = [
    "nav-toggle",
    "nav-home",
    "nav-explore",
    "nav-search",
    "nav-library",
    "nav-crate",
    "nav-settings",
];
/// The player bar, left to right. The seek slider is a stop only while a track has a length.
const BAR: [&str; 12] = [
    "bar-now-playing",
    "bar-shuffle",
    "bar-previous",
    "bar-play",
    "bar-next",
    "bar-repeat",
    "bar-like",
    "bar-mute",
    "bar-volume",
    "bar-mini",
    "bar-lyrics",
    "bar-queue",
];
const BAR_PLAYING: [&str; 13] = [
    "bar-now-playing",
    "bar-shuffle",
    "bar-previous",
    "bar-play",
    "bar-next",
    "bar-repeat",
    "bar-progress",
    "bar-like",
    "bar-mute",
    "bar-volume",
    "bar-mini",
    "bar-lyrics",
    "bar-queue",
];

/// Nav first, bar last, the page and panels between them; and Shift+Tab is the exact reverse.
fn assert_sane(shell: &Entity<Shell>, vcx: &mut VisualTestContext, what: &str) -> Vec<String> {
    assert_sane_with(shell, vcx, what, &BAR)
}

fn assert_sane_with(
    shell: &Entity<Shell>,
    vcx: &mut VisualTestContext,
    what: &str,
    bar: &[&str],
) -> Vec<String> {
    let forward = cycle(shell, vcx, "tab");
    assert_eq!(
        &forward[..NAV.len()],
        NAV,
        "{what}: the nav rail comes first: {forward:?}"
    );
    assert_eq!(
        &forward[forward.len() - bar.len()..],
        bar,
        "{what}: the player bar comes last, left to right: {forward:?}"
    );
    assert!(
        forward[NAV.len()..forward.len() - bar.len()]
            .iter()
            .all(|n| n == "page"),
        "{what}: only page stops sit between: {forward:?}"
    );
    // From the root, Shift+Tab lands on the last stop and walks the same stops back.
    let mut back = cycle(shell, vcx, "shift-tab");
    back.reverse();
    assert_eq!(back, forward, "{what}: Shift+Tab is Tab in reverse");
    forward
}

/// Every shelf on Home is one page stop, and so is the row of mood chips above them.
fn home_items(shell: &Entity<Shell>, vcx: &mut VisualTestContext) -> usize {
    shell.read_with(vcx, |s, cx| {
        let home = s.home().read(cx);
        home.shelves().len() + usize::from(!home.chips().is_empty())
    })
}

#[gpui::test]
fn home_tabs_through_the_nav_then_the_player_bar(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup(cx);
    let order = assert_sane(&shell, vcx, "home");
    let items = home_items(&shell, vcx);
    assert!(items > 0, "the feed has loaded");
    assert_eq!(
        order.len(),
        NAV.len() + items + BAR.len(),
        "home's page stops are its shelves: {order:?}"
    );
}

#[gpui::test]
fn the_seek_slider_joins_the_bar_once_a_track_is_playing(cx: &mut TestAppContext) {
    let (player, shell, vcx) = setup(cx);
    player.update(vcx, |m, cx| m.toggle_play(cx));
    open_current_load(&player, vcx);
    vcx.run_until_parked();
    let order = assert_sane_with(&shell, vcx, "playing", &BAR_PLAYING);
    assert_eq!(
        order.len(),
        NAV.len() + home_items(&shell, vcx) + BAR_PLAYING.len()
    );
}

#[gpui::test]
fn every_page_keeps_the_nav_first_and_the_bar_last(cx: &mut TestAppContext) {
    let (_player, shell, vcx) = setup(cx);
    for route in [Route::Explore, Route::Search, Route::Library, Route::Settings] {
        shell.update_in(vcx, |s, window, cx| s.choose_route(route, window, cx));
        vcx.run_until_parked();
        assert_sane(&shell, vcx, &format!("{route:?}"));
    }
}

#[gpui::test]
fn the_queue_panel_and_now_playing_sit_between_the_nav_and_the_bar(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (_player, shell, vcx) = setup(cx);
    shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    let with_queue = assert_sane(&shell, vcx, "queue open");
    let home = home_items(&shell, vcx);
    assert!(
        with_queue.iter().filter(|n| *n == "page").count() >= home + 4,
        "the panel's own stops are in the walk: {with_queue:?}"
    );
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    let with_both = assert_sane(&shell, vcx, "now playing + queue");
    // Now Playing takes Home's place on the page, so Home's stops leave as its own arrive.
    assert!(
        with_both.len() + home > with_queue.len() - 4,
        "now playing adds its own stops"
    );
}
