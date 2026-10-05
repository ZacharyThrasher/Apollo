//! Headless tests for the Explore page: the real shell and Explore view on GPUI's test
//! platform, with the simulated library (no latency) and deterministic mock playback adapters.
//! The simulated page has 24 new releases' worth of album cards (one per fixture), a 24-song
//! chart that shows 20 rows, one "Trending" card shelf and 18 mood tiles ("Empty" and "Broken"
//! are the failure states).

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::explore::{Access, ExploreModel, MoodPhase, Phase, STALE_AFTER};
use apollo_app::library::model::LibraryModel;
use apollo_app::library::sim::{self, ExploreBehaviour, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::explore_page::{ExploreView, TOP_SHOWN};
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

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

fn prepared(library: SimLibrary) -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new("startTrack01", "Start"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(apollo_app::search::Unavailable),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(library),
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    explore: Entity<ExploreModel>,
    view: Entity<ExploreView>,
    library: SimLibrary,
}

/// The shell with `library` installed, before anything has run.
fn start(cx: &mut TestAppContext, library: SimLibrary) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = prepared(library.clone());
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    let explore = shell.read_with(vcx, |s, _| s.explore().clone());
    let view = shell.read_with(vcx, |s, _| s.explore_page().clone());
    (
        Page {
            player,
            shell,
            explore,
            view,
            library,
        },
        vcx,
    )
}

/// Explore opened and loaded.
fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    let (page, vcx) = start(cx, SimLibrary::instant());
    open(&page, vcx);
    (page, vcx)
}

/// A window tall enough for the whole page: the chart alone is twenty 48px rows.
fn tall(vcx: &mut VisualTestContext) {
    vcx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(2800.0)));
    vcx.run_until_parked();
}

fn open(page: &Page, vcx: &mut VisualTestContext) {
    tall(vcx);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Explore, cx));
    vcx.run_until_parked();
}

fn phase(page: &Page, vcx: &mut VisualTestContext) -> Phase {
    page.explore.read_with(vcx, |m, _| m.phase())
}

fn route(page: &Page, vcx: &mut VisualTestContext) -> Route {
    page.shell.read_with(vcx, |s, _| s.route())
}

/// Element ids are `'static`; a test can afford to leak a few.
fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

/// The page scrolls: bring a section into view the way the keyboard does.
fn focus_stop(page: &Page, vcx: &mut VisualTestContext, stop: usize) {
    let handle = page
        .view
        .read_with(vcx, |v, _| v.item_stops()[stop].clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    vcx.run_until_parked();
}

/// Every video in the queue, played, current, then up next.
fn queue(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.player.read_with(vcx, |m, _| {
        let q = m.queue_view();
        q.history
            .iter()
            .chain(q.current.iter())
            .chain(q.upcoming.iter())
            .map(|r| r.video_id.clone())
            .collect()
    })
}

fn mood_phase(page: &Page, vcx: &mut VisualTestContext) -> Option<MoodPhase> {
    page.explore
        .read_with(vcx, |m, _| m.mood().map(|o| o.phase.clone()))
}

// ---- loading and drawing --------------------------------------------------------------------

#[gpui::test]
fn explore_loads_when_first_opened_not_at_startup(cx: &mut TestAppContext) {
    let (page, vcx) = start(cx, SimLibrary::instant());
    vcx.run_until_parked();
    assert_eq!(page.library.explore_requests(), 0, "nobody has opened it");
    assert_eq!(phase(&page, vcx), Phase::Loading);

    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Explore, cx));
    assert_eq!(phase(&page, vcx), Phase::Loading, "the skeleton shows first");
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert_eq!(page.library.explore_requests(), 1);
}

#[gpui::test]
fn the_page_draws_new_releases_the_chart_a_shelf_and_the_tiles(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(
        vcx.debug_bounds("explore-0-0").is_some(),
        "the first new release is a card"
    );
    assert!(vcx.debug_bounds("explore-chart-0").is_some(), "the chart starts");
    assert!(
        vcx.debug_bounds("explore-chart-1").is_some(),
        "and the second row follows it"
    );
    assert!(
        vcx.debug_bounds("explore-chart-0").unwrap().origin.y
            < vcx.debug_bounds("explore-chart-1").unwrap().origin.y,
        "the rows run down the page"
    );
    page.explore.read_with(vcx, |m, _| {
        let p = m.page().expect("loaded");
        assert_eq!(p.new_releases.len(), apollo_app::backend::sim::FIXTURES.len());
        assert_eq!(p.top_songs.len(), sim::CHART_SONGS);
        assert_eq!(p.shelves[0].title, "Trending");
        assert_eq!(p.moods.len(), 18);
    });
}

#[gpui::test]
fn the_chart_shows_twenty_rows_and_show_all_opens_the_chart_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // Rows past the twentieth are not drawn; the chart page has the rest.
    focus_stop(&page, vcx, apollo_app::backend::sim::FIXTURES.len() + TOP_SHOWN - 1);
    assert!(vcx.debug_bounds(leak(format!("explore-chart-{}", TOP_SHOWN - 1))).is_some());
    assert!(vcx.debug_bounds(leak(format!("explore-chart-{TOP_SHOWN}"))).is_none());
    assert!(vcx.debug_bounds("explore-show-all").is_some());

    click(vcx, "explore-show-all");
    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.detail_origin()),
        Some(Route::Explore)
    );
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.nav_route()),
        Route::Explore,
        "the nav keeps Explore lit"
    );
    let library: Entity<LibraryModel> = page
        .shell
        .read_with(vcx, |s, cx| s.library().read(cx).model().clone());
    library.read_with(vcx, |m, _| {
        let d = m.detail().expect("the chart page is open");
        assert_eq!(d.collection.id.as_str(), sim::CHART_ID);
    });

    vcx.simulate_keystrokes("escape");
    assert_eq!(route(&page, vcx), Route::Explore);
    assert!(vcx.debug_bounds("explore-chart-0").is_some(), "back on Explore");
}

#[gpui::test]
fn the_charts_skim_button_auditions_the_whole_chart(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let expected: Vec<String> = page.explore.read_with(vcx, |m, _| {
        m.page()
            .unwrap()
            .top_songs
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    });
    focus_stop(&page, vcx, apollo_app::backend::sim::FIXTURES.len());
    click(vcx, "explore-skim");
    assert_eq!(queue(&page, vcx), expected, "every chart song, in rank order");
    page.player.read_with(vcx, |m, _| {
        let skim = m.view().skim.expect("skimming");
        assert_eq!((skim.index, skim.total), (1, expected.len()));
    });
    // Off in Settings, the button is gone.
    page.player.update(vcx, |m, cx| m.set_skim_enabled(false, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("explore-skim").is_none());
}

#[gpui::test]
fn a_chart_song_plays_with_the_whole_chart_as_the_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let expected: Vec<String> = page.explore.read_with(vcx, |m, _| {
        m.page()
            .unwrap()
            .top_songs
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    });
    // The chart sits below the new releases: bring it into view with the keyboard.
    let first_chart_stop = apollo_app::backend::sim::FIXTURES.len();
    focus_stop(&page, vcx, first_chart_stop + 2);
    click(vcx, "explore-chart-2");
    assert_eq!(queue(&page, vcx), expected, "every chart song, in rank order");
    assert_eq!(
        page.player.read_with(vcx, |m, _| m.current_video()).as_deref(),
        Some(expected[2].as_str())
    );
}

#[gpui::test]
fn a_new_release_opens_its_album_and_back_returns_to_explore(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "explore-0-0");
    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.detail_origin()),
        Some(Route::Explore)
    );
    vcx.simulate_keystrokes("escape");
    assert_eq!(route(&page, vcx), Route::Explore);
    assert_eq!(phase(&page, vcx), Phase::Ready);
}

// ---- moods ----------------------------------------------------------------------------------

#[gpui::test]
fn a_mood_tile_opens_a_page_of_playlists_and_back_returns_to_the_tiles(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let tiles_from = apollo_app::backend::sim::FIXTURES.len() + TOP_SHOWN + 1;
    focus_stop(&page, vcx, tiles_from);
    click(vcx, "explore-mood-0");
    assert_eq!(page.library.mood_requests(), ["sim-mood:Chill"]);
    page.explore.read_with(vcx, |m, _| {
        assert_eq!(m.mood().expect("a mood page is open").title, "Chill");
    });
    assert!(matches!(mood_phase(&page, vcx), Some(MoodPhase::Ready(_))));
    assert!(
        vcx.debug_bounds("explore-playlist-0").is_some(),
        "its playlists are cards"
    );
    assert!(vcx.debug_bounds("explore-mood-0").is_none(), "the tiles are gone");

    vcx.simulate_keystrokes("escape");
    assert!(mood_phase(&page, vcx).is_none());
    assert!(vcx.debug_bounds("explore-mood-0").is_some(), "the tiles are back");
}

#[gpui::test]
fn a_playlist_opened_from_a_mood_returns_to_that_mood_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_stop(&page, vcx, apollo_app::backend::sim::FIXTURES.len() + TOP_SHOWN + 1);
    click(vcx, "explore-mood-1");
    click(vcx, "explore-playlist-0");
    assert_eq!(route(&page, vcx), Route::Library);
    vcx.simulate_keystrokes("escape");
    assert_eq!(route(&page, vcx), Route::Explore);
    page.explore.read_with(vcx, |m, _| {
        assert_eq!(m.mood().expect("still on the mood page").title, "Workout");
    });
    assert!(vcx.debug_bounds("explore-playlist-0").is_some());
    assert_eq!(
        page.library.mood_requests().len(),
        1,
        "the page was kept, not asked for again"
    );
}

#[gpui::test]
fn a_mood_with_no_playlists_says_so_and_a_failed_one_offers_retry(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let tiles = page.explore.read_with(vcx, |m, _| m.page().unwrap().moods.clone());
    let index_of = |title: &str| tiles.iter().position(|t| t.title == title).unwrap();
    let first = apollo_app::backend::sim::FIXTURES.len() + TOP_SHOWN + 1;

    focus_stop(&page, vcx, first + index_of("Empty"));
    click(vcx, leak(format!("explore-mood-{}", index_of("Empty"))));
    assert!(matches!(
        mood_phase(&page, vcx),
        Some(MoodPhase::Ready(found)) if found.is_empty()
    ));
    assert!(vcx.debug_bounds("explore-notice-action").is_some(), "Back is offered");
    click(vcx, "explore-notice-action");
    assert!(mood_phase(&page, vcx).is_none(), "and it goes back");

    focus_stop(&page, vcx, first + index_of("Broken"));
    click(vcx, leak(format!("explore-mood-{}", index_of("Broken"))));
    assert_eq!(
        mood_phase(&page, vcx),
        Some(MoodPhase::Failed(LoadFailure::Network))
    );
    click(vcx, "explore-notice-action");
    assert_eq!(
        page.library.mood_requests().len(),
        3,
        "Retry asks again: Empty, Broken, Broken"
    );
}

// ---- states ---------------------------------------------------------------------------------

#[gpui::test]
fn a_failed_page_is_one_error_row_that_retry_clears(cx: &mut TestAppContext) {
    let library = SimLibrary::instant();
    library.set_explore_behaviour(ExploreBehaviour::Fail(LoadFailure::Network));
    let (page, vcx) = start(cx, library.clone());
    open(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Failed(LoadFailure::Network));
    assert!(vcx.debug_bounds("explore-notice-action").is_some());

    library.set_explore_behaviour(ExploreBehaviour::Feed);
    click(vcx, "explore-notice-action");
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert!(vcx.debug_bounds("explore-chart-0").is_some());
}

#[gpui::test]
fn a_page_with_nothing_on_it_says_so_and_offers_retry(cx: &mut TestAppContext) {
    let library = SimLibrary::instant();
    library.set_explore_behaviour(ExploreBehaviour::Empty);
    let (page, vcx) = start(cx, library);
    open(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert!(vcx.debug_bounds("explore-notice-action").is_some());
}

#[gpui::test]
fn the_page_is_cached_and_refreshes_only_after_ten_minutes(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let loaded = page
        .explore
        .read_with(vcx, |m, _| m.loaded_at().expect("loaded"));

    // Leaving and reopening inside the window costs nothing.
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Explore, cx));
    vcx.run_until_parked();
    assert_eq!(page.library.explore_requests(), 1);

    page.explore
        .update(vcx, |m, cx| m.show(loaded + STALE_AFTER - Duration::from_secs(1), cx));
    vcx.run_until_parked();
    assert_eq!(page.library.explore_requests(), 1, "nine minutes fifty-nine is fresh");

    page.explore
        .update(vcx, |m, cx| m.show(loaded + STALE_AFTER, cx));
    assert!(page.explore.read_with(vcx, |m, _| m.is_refreshing()));
    assert_eq!(phase(&page, vcx), Phase::Ready, "the page stays while the new one loads");
    vcx.run_until_parked();
    assert_eq!(page.library.explore_requests(), 2);
    assert!(page
        .explore
        .read_with(vcx, |m, _| m.loaded_at().unwrap() > loaded));
}

#[gpui::test]
fn a_failed_refresh_keeps_what_is_shown(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let loaded = page.explore.read_with(vcx, |m, _| m.loaded_at().unwrap());
    page.library
        .set_explore_behaviour(ExploreBehaviour::Fail(LoadFailure::Network));
    page.explore
        .update(vcx, |m, cx| m.show(loaded + STALE_AFTER, cx));
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready, "no error row over a page that is there");
    assert!(page.explore.read_with(vcx, |m, _| m.page().is_some()));
}

#[gpui::test]
fn a_different_account_drops_the_page_and_the_open_mood(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    focus_stop(&page, vcx, apollo_app::backend::sim::FIXTURES.len() + TOP_SHOWN + 1);
    click(vcx, "explore-mood-0");
    assert!(mood_phase(&page, vcx).is_some());

    page.explore
        .update(vcx, |m, cx| m.set_access(Access::Open, 7, cx));
    assert_eq!(phase(&page, vcx), Phase::Loading, "nothing of the old account stays");
    assert!(mood_phase(&page, vcx).is_none());
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);

    page.explore
        .update(vcx, |m, cx| m.set_access(Access::SignedOut, 7, cx));
    assert_eq!(phase(&page, vcx), Phase::SignedOut);
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("explore-sign-in").is_some());

    page.explore
        .update(vcx, |m, cx| m.set_access(Access::Expired, 7, cx));
    assert_eq!(phase(&page, vcx), Phase::Failed(LoadFailure::Session));
}

// ---- keyboard -------------------------------------------------------------------------------

#[gpui::test]
fn left_right_walk_a_row_and_up_down_walk_the_chart(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let releases = apollo_app::backend::sim::FIXTURES.len();
    focus_stop(&page, vcx, 0);
    vcx.simulate_keystrokes("right");
    let second = page.view.read_with(vcx, |v, _| v.item_stops()[1].clone());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(second));

    focus_stop(&page, vcx, releases);
    vcx.simulate_keystrokes("down down");
    let third = page.view.read_with(vcx, |v, _| v.item_stops()[releases + 2].clone());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(third));
    vcx.simulate_keystrokes("up");
    let second = page.view.read_with(vcx, |v, _| v.item_stops()[releases + 1].clone());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(second));

    // The arrows stay inside their group.
    focus_stop(&page, vcx, releases);
    vcx.simulate_keystrokes("up");
    let top = page.view.read_with(vcx, |v, _| v.item_stops()[releases].clone());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(top));
}

#[gpui::test]
fn the_nav_has_explore_after_home(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(Route::ALL[..2], [Route::Home, Route::Explore]);
    assert_eq!(Route::Explore.title(), "Explore");
    assert_eq!(route(&page, vcx), Route::Explore);
    assert_eq!(page.shell.read_with(vcx, |s, _| s.nav_route()), Route::Explore);
}
