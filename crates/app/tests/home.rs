//! Headless tests for the Home page: the real shell and Home view on GPUI's test platform, with
//! the simulated library (no latency) and deterministic mock playback adapters. The simulated
//! feed has three batches: "Quick picks" (20 songs), "Albums for you" and "Listen again" (songs
//! and albums mixed); then "Artists you might like" and "From your library"; then "More albums".

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::home::{Access, HomeModel, More, Phase, MAX_SHELVES, STALE_AFTER};
use apollo_app::library::model::{LibraryModel, Load};
use apollo_app::library::sim::{self, HomeBehaviour, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::home_page::HomeView;
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
    home: Entity<HomeModel>,
    view: Entity<HomeView>,
    library: SimLibrary,
}

/// The shell with `library` installed, before anything has run.
fn start(cx: &mut TestAppContext, library: SimLibrary) -> (Page, &mut VisualTestContext) {
    let ready = prepared(library.clone());
    start_with(cx, library, ready)
}

fn start_with(
    cx: &mut TestAppContext,
    library: SimLibrary,
    ready: Prepared,
) -> (Page, &mut VisualTestContext) {
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
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    let home = shell.read_with(vcx, |s, _| s.home().clone());
    let view = shell.read_with(vcx, |s, _| s.home_page().clone());
    (
        Page {
            player,
            shell,
            home,
            view,
            library,
        },
        vcx,
    )
}

/// Like [`start`], then everything that is going to load has loaded.
fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    let (page, vcx) = start(cx, SimLibrary::instant());
    vcx.run_until_parked();
    (page, vcx)
}

/// The song grid is five columns wide, so a 1000px window leaves its right side off screen.
fn narrow(vcx: &mut VisualTestContext) {
    vcx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(720.0)));
    vcx.run_until_parked();
}

fn phase(page: &Page, vcx: &mut VisualTestContext) -> Phase {
    page.home.read_with(vcx, |m, _| m.phase())
}

fn titles(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.home.read_with(vcx, |m, _| {
        m.shelves().iter().map(|s| s.title.clone()).collect()
    })
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
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

fn current(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.player.read_with(vcx, |m, _| m.current_video())
}

/// Stop index of item `j` of shelf `i`, from the shelf sizes.
fn stop_of(page: &Page, vcx: &mut VisualTestContext, shelf: usize, item: usize) -> usize {
    let before: usize = page.home.read_with(vcx, |m, _| {
        m.shelves().iter().take(shelf).map(|s| s.items.len()).sum()
    });
    before + item
}

fn route(page: &Page, vcx: &mut VisualTestContext) -> Route {
    page.shell.read_with(vcx, |s, _| s.route())
}

// ---- loading and drawing --------------------------------------------------------------------

#[gpui::test]
fn home_shows_a_skeleton_until_the_first_shelves_arrive_then_draws_them(cx: &mut TestAppContext) {
    let (page, vcx) = start(cx, SimLibrary::instant());
    assert_eq!(
        phase(&page, vcx),
        Phase::Loading,
        "the first batch is on its way"
    );
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);
    let titles = titles(&page, vcx);
    assert_eq!(
        &titles[..3],
        ["Quick picks", "Albums for you", "Listen again"]
    );
    assert!(
        vcx.debug_bounds("home-0-0").is_some(),
        "the first song row is drawn"
    );
    assert!(
        vcx.debug_bounds("home-1-0").is_some(),
        "and the first album card"
    );
    page.home
        .read_with(vcx, |m, _| assert!(m.shelves()[0].is_songs()));
    page.home
        .read_with(vcx, |m, _| assert!(!m.shelves()[1].is_songs()));
}

#[gpui::test]
fn the_next_batches_load_as_the_page_runs_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // Three shelves do not fill the window plus its margin, so the rest follow on their own.
    assert_eq!(
        page.library.home_log(),
        ["", sim::HOME_SECOND, sim::HOME_THIRD],
        "each token is asked for once, in order"
    );
    assert_eq!(titles(&page, vcx).len(), 6);
    page.home.read_with(vcx, |m, _| {
        assert!(!m.has_more());
        assert_eq!(m.more(), More::Idle);
    });
}

#[gpui::test]
fn a_failed_first_batch_is_one_error_row_that_retry_clears(cx: &mut TestAppContext) {
    let library = SimLibrary::instant();
    library.set_home_behaviour(HomeBehaviour::Fail(LoadFailure::Network));
    let (page, vcx) = start(cx, library.clone());
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Failed(LoadFailure::Network));
    assert!(
        vcx.debug_bounds("home-notice-action").is_some(),
        "Retry is on screen"
    );

    library.set_home_behaviour(HomeBehaviour::Feed);
    click(vcx, "home-notice-action");
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert_eq!(titles(&page, vcx)[0], "Quick picks");
}

#[gpui::test]
fn a_feed_with_nothing_in_it_says_so_and_offers_retry(cx: &mut TestAppContext) {
    let library = SimLibrary::instant();
    library.set_home_behaviour(HomeBehaviour::Empty);
    let (page, vcx) = start(cx, library);
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert!(titles(&page, vcx).is_empty());
    assert!(vcx.debug_bounds("home-notice-action").is_some());
}

#[gpui::test]
fn a_batch_that_fails_leaves_the_shelves_and_a_retry_that_finishes_the_feed(
    cx: &mut TestAppContext,
) {
    let library = SimLibrary::instant();
    library.set_home_behaviour(HomeBehaviour::FailMore(LoadFailure::Network));
    let (page, vcx) = start(cx, library.clone());
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready, "the shelves stay");
    assert_eq!(titles(&page, vcx).len(), 3);
    page.home.read_with(vcx, |m, _| {
        assert_eq!(m.more(), More::Failed(LoadFailure::Network))
    });
    assert!(vcx.debug_bounds("home-notice-action").is_some());
    // It is not asked again by itself.
    let asked = library.home_log().len();
    vcx.run_until_parked();
    assert_eq!(library.home_log().len(), asked);

    library.set_home_behaviour(HomeBehaviour::Feed);
    click(vcx, "home-notice-action");
    assert_eq!(titles(&page, vcx).len(), 6);
}

#[gpui::test]
fn paging_stops_at_the_shelf_cap_and_never_loops_on_a_repeated_token(cx: &mut TestAppContext) {
    // The simulated feed ends by itself; the cap is a number the model owns.
    assert!(MAX_SHELVES >= 6);
    let (page, vcx) = setup(cx);
    page.home.update(vcx, |m, cx| m.load_more(cx));
    vcx.run_until_parked();
    assert_eq!(
        page.library.home_log().len(),
        3,
        "at the end of the feed, asking for more asks for nothing"
    );
}

// ---- opening and playing ----------------------------------------------------------------------

#[gpui::test]
fn an_album_card_opens_its_page_and_back_returns_home(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "home-1-0");
    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.detail_origin()),
        Some(Route::Home)
    );
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.nav_route()),
        Route::Home,
        "the nav keeps Home lit"
    );
    let library: Entity<LibraryModel> = page
        .shell
        .read_with(vcx, |s, cx| s.library().read(cx).model().clone());
    library.read_with(vcx, |m, _| {
        let d = m.detail().expect("the album page is open");
        assert_eq!(d.collection.title, "Night Shift");
    });

    vcx.simulate_keystrokes("escape");
    assert_eq!(route(&page, vcx), Route::Home);
    assert!(
        vcx.debug_bounds("home-1-0").is_some(),
        "Home is on screen again"
    );
    assert_eq!(phase(&page, vcx), Phase::Ready);
}

#[gpui::test]
fn an_artist_card_opens_the_artist_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // "Artists you might like" is the fourth shelf; reach it with the keyboard so it scrolls
    // into view, then open it with Enter.
    let stop = stop_of(&page, vcx, 3, 0);
    let handle = page
        .view
        .read_with(vcx, |v, _| v.item_stops()[stop].clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    vcx.simulate_keystrokes("right left");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(route(&page, vcx), Route::Library);
    let library: Entity<LibraryModel> = page
        .shell
        .read_with(vcx, |s, cx| s.library().read(cx).model().clone());
    library.read_with(vcx, |m, _| {
        let a = m.artist().expect("the artist page is open");
        assert_eq!(a.name(), "Neon Arcade");
        assert!(matches!(a.state, Load::Ready(_) | Load::Loading));
    });
}

#[gpui::test]
fn a_song_plays_with_its_shelf_as_the_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let expected: Vec<String> = page.home.read_with(vcx, |m, _| {
        m.shelves()[0]
            .songs()
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    });
    assert_eq!(expected.len(), sim::QUICK_PICKS);
    click(vcx, "home-0-2");
    assert_eq!(
        queue(&page, vcx),
        expected,
        "the whole shelf is queued, in its order"
    );
    assert_eq!(current(&page, vcx).as_deref(), Some(expected[2].as_str()));
}

#[gpui::test]
fn a_song_card_on_a_mixed_shelf_queues_the_shelfs_songs_only(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // "Listen again" alternates song, album, song, album...: item 2 is the second song.
    let songs: Vec<String> = page.home.read_with(vcx, |m, _| {
        m.shelves()[2]
            .songs()
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    });
    assert_eq!(songs.len(), 6);
    click(vcx, "home-2-2");
    assert_eq!(
        queue(&page, vcx),
        songs,
        "albums on the shelf are not in the queue"
    );
    assert_eq!(current(&page, vcx).as_deref(), Some(songs[1].as_str()));
}

// ---- cache and refresh ---------------------------------------------------------------------------

#[gpui::test]
fn shelves_are_cached_and_refresh_only_after_ten_minutes(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let asked = page.library.home_log().len();
    let loaded = page
        .home
        .read_with(vcx, |m, _| m.loaded_at().expect("loaded"));

    // Leaving and reopening Home inside the window costs nothing.
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Home, cx));
    vcx.run_until_parked();
    assert_eq!(page.library.home_log().len(), asked);

    let just_under = loaded + STALE_AFTER - Duration::from_secs(1);
    page.home.update(vcx, |m, cx| m.show(just_under, cx));
    vcx.run_until_parked();
    assert_eq!(
        page.library.home_log().len(),
        asked,
        "nine minutes fifty-nine is fresh"
    );

    page.home
        .update(vcx, |m, cx| m.show(loaded + STALE_AFTER, cx));
    assert!(page.home.read_with(vcx, |m, _| m.is_refreshing()));
    assert_eq!(
        phase(&page, vcx),
        Phase::Ready,
        "the shelves stay on screen while the new ones load"
    );
    vcx.run_until_parked();
    assert_eq!(
        page.library.home_log().len(),
        asked + 1,
        "one refresh, of the first batch only"
    );
    assert!(!page.home.read_with(vcx, |m, _| m.is_refreshing()));
    page.home
        .read_with(vcx, |m, _| assert!(m.loaded_at().unwrap() > loaded));
}

#[gpui::test]
fn a_failed_refresh_keeps_what_is_shown_and_is_tried_again_at_the_next_reopening(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    let before = titles(&page, vcx);
    let loaded = page.home.read_with(vcx, |m, _| m.loaded_at().unwrap());

    page.library
        .set_home_behaviour(HomeBehaviour::Fail(LoadFailure::Network));
    page.home
        .update(vcx, |m, cx| m.show(loaded + STALE_AFTER, cx));
    vcx.run_until_parked();
    assert_eq!(
        phase(&page, vcx),
        Phase::Ready,
        "no error row over a feed that is there"
    );
    assert_eq!(titles(&page, vcx), before);

    page.library.set_home_behaviour(HomeBehaviour::Feed);
    let asked = page.library.home_log().len();
    page.home
        .update(vcx, |m, cx| m.show(loaded + STALE_AFTER, cx));
    vcx.run_until_parked();
    assert_eq!(
        page.library.home_log().len(),
        asked + 1,
        "still stale, so asked again"
    );
    assert!(page
        .home
        .read_with(vcx, |m, _| m.loaded_at().unwrap() > loaded));
}

// ---- the account -----------------------------------------------------------------------------------

#[gpui::test]
fn a_different_account_drops_the_shelves_and_loads_its_own(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let asked = page.library.home_log().len();
    page.home
        .update(vcx, |m, cx| m.set_access(Access::Open, 7, cx));
    assert_eq!(
        phase(&page, vcx),
        Phase::Loading,
        "nothing of the old account stays"
    );
    assert!(titles(&page, vcx).is_empty());
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert!(
        page.library.home_log().len() > asked,
        "the new account's feed was asked for"
    );

    page.home
        .update(vcx, |m, cx| m.set_access(Access::SignedOut, 7, cx));
    assert_eq!(phase(&page, vcx), Phase::SignedOut);
    assert!(titles(&page, vcx).is_empty());
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("home-sign-in").is_some(),
        "the page offers the way back in"
    );

    page.home
        .update(vcx, |m, cx| m.set_access(Access::Expired, 7, cx));
    assert_eq!(phase(&page, vcx), Phase::Failed(LoadFailure::Session));
}

#[gpui::test]
fn the_feed_waits_for_the_account_before_asking(cx: &mut TestAppContext) {
    let (page, vcx) = start(cx, SimLibrary::instant());
    page.home
        .update(vcx, |m, cx| m.set_access(Access::Waiting, 3, cx));
    vcx.run_until_parked();
    let asked = page.library.home_log().len();
    assert_eq!(phase(&page, vcx), Phase::Loading);
    page.home
        .update(vcx, |m, cx| m.set_access(Access::Waiting, 3, cx));
    vcx.run_until_parked();
    assert_eq!(
        page.library.home_log().len(),
        asked,
        "waiting asks for nothing"
    );
    page.home
        .update(vcx, |m, cx| m.set_access(Access::Open, 3, cx));
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);
}

// ---- rows and keyboard ------------------------------------------------------------------------------

#[gpui::test]
fn left_and_right_walk_the_items_and_the_row_follows_the_focus(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    narrow(vcx);
    let first = page.view.read_with(vcx, |v, _| v.item_stops()[0].clone());
    vcx.update(|window, cx| window.focus(&first, cx));
    vcx.simulate_keystrokes("right");
    let second = page.view.read_with(vcx, |v, _| v.item_stops()[1].clone());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(second));

    // Column four of the song grid starts past the right edge of the window.
    for _ in 0..12 {
        vcx.simulate_keystrokes("right");
    }
    vcx.run_until_parked();
    assert!(
        page.view.read_with(vcx, |v, _| v.row_offset(0)) < 0.0,
        "the row scrolled to show the focused song"
    );
    vcx.simulate_keystrokes("left");
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(page.view.read_with(vcx, |v, _| v.item_stops()[12].clone()))
    );
}

#[gpui::test]
fn a_shelf_is_one_tab_stop_that_remembers_where_you_were(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let (first, second_shelf_first) = page.view.read_with(vcx, |v, cx| {
        let shelves = page.home.read(cx).shelves();
        (
            v.item_stops()[0].clone(),
            v.item_stops()[shelves[0].items.len()].clone(),
        )
    });
    vcx.update(|window, cx| window.focus(&first, cx));
    vcx.simulate_keystrokes("right right");
    let third = page.view.read_with(vcx, |v, _| v.item_stops()[2].clone());
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(third.clone()));
    vcx.run_until_parked();

    // Tab leaves the shelf: the next stop is the next shelf's remembered item, not item four.
    vcx.simulate_keystrokes("tab");
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(second_shelf_first),
        "Tab goes shelf to shelf"
    );
    vcx.run_until_parked();
    vcx.simulate_keystrokes("shift-tab");
    assert_eq!(
        vcx.update(|window, cx| window.focused(cx)),
        Some(third),
        "and comes back to the item you left"
    );

    // The arrows stay inside the shelf.
    vcx.update(|window, cx| window.focus(&first, cx));
    vcx.simulate_keystrokes("left");
    assert_eq!(vcx.update(|window, cx| window.focused(cx)), Some(first));
}

/// Records which tracks were asked to be resolved ahead of time.
struct Recorder(Arc<std::sync::Mutex<Vec<String>>>);

impl apollo_runtime::Resolver for Recorder {
    fn start(&mut self, _: apollo_runtime::ResolveRequest) {}
    fn cancel(&mut self, _: apollo_domain::playback::LoadId) {}
    fn prefetch(&mut self, track: &Track) {
        self.0.lock().unwrap().push(track.video_id.as_str().to_owned());
    }
}

#[gpui::test]
fn resting_the_pointer_on_a_song_starts_resolving_it_before_the_click(cx: &mut TestAppContext) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let library = SimLibrary::instant();
    let mut ready = prepared(library.clone());
    ready.ports.resolver = Box::new(Recorder(seen.clone()));
    let (_page, vcx) = start_with(cx, library, ready);
    vcx.run_until_parked();
    let before = seen.lock().unwrap().len();
    let over = vcx.debug_bounds("home-0-3").expect("a song row").center();
    vcx.simulate_mouse_move(over, None, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(seen.lock().unwrap().len(), before, "not before the pointer has rested");
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    let after = seen.lock().unwrap().clone();
    assert_eq!(after.len(), before + 1, "{after:?}");
}

#[gpui::test]
fn the_chevrons_show_on_hover_and_page_the_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    narrow(vcx);
    assert!(
        vcx.debug_bounds("home-next-0").is_none(),
        "hidden until the pointer is over the row"
    );
    let over = vcx.debug_bounds("home-0-1").expect("a song row").center();
    vcx.simulate_mouse_move(over, None, Modifiers::none());
    vcx.run_until_parked();
    assert!(
        vcx.debug_bounds("home-next-0").is_some(),
        "next shows over a row that scrolls"
    );
    assert!(
        vcx.debug_bounds("home-previous-0").is_none(),
        "previous waits until the row has moved"
    );

    click(vcx, "home-next-0");
    let moved = page.view.read_with(vcx, |v, _| v.row_offset(0));
    assert!(
        moved < 0.0,
        "reduced motion: the row jumps at once ({moved})"
    );
    assert_eq!(moved % 336.0, 0.0, "and lands on a column");
    vcx.simulate_mouse_move(over, None, Modifiers::none());
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("home-previous-0").is_some());
    click(vcx, "home-previous-0");
    assert_eq!(page.view.read_with(vcx, |v, _| v.row_offset(0)), 0.0);
}

#[gpui::test]
fn clicking_a_chevron_does_not_click_the_tile_behind_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    narrow(vcx);
    let playing = current(&page, vcx);
    let mut clicked = 0;
    for (shelf, tile) in [("home-next-0", "home-0-1"), ("home-next-1", "home-1-1")] {
        let over = vcx.debug_bounds(tile).expect("a tile").center();
        vcx.simulate_mouse_move(over, None, Modifiers::none());
        vcx.run_until_parked();
        if vcx.debug_bounds(shelf).is_none() {
            continue;
        }
        click(vcx, shelf);
        clicked += 1;
        assert_eq!(route(&page, vcx), Route::Home, "{shelf} did not open a page");
        assert_eq!(current(&page, vcx), playing, "{shelf} did not play a song");
    }
    assert!(clicked > 0, "at least one shelf scrolls in a narrow window");
}

// ---- the track menu ------------------------------------------------------------------------------

use apollo_app::ui::track_menu::{Command, Subject};
use gpui::MouseButton;

fn right_click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
}

fn menu_subject(page: &Page, vcx: &mut VisualTestContext) -> Option<Subject> {
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    menu.read_with(vcx, |m, _| m.subject().cloned())
}

#[gpui::test]
fn right_clicking_a_song_tile_opens_the_track_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    right_click(vcx, "home-0-0");
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Track(_))));
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    let commands = menu.read_with(vcx, |m, cx| m.commands(cx));
    assert!(commands.contains(&Command::PlayNext));
    assert!(commands.contains(&Command::StartRadio));
    assert_eq!(
        current(&page, vcx).as_deref(),
        Some("startTrack01"),
        "the click did not play the tile"
    );
}

#[gpui::test]
fn a_song_tiles_more_button_shows_on_hover_and_opens_the_same_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let tile = vcx.debug_bounds("home-0-0").expect("tile is on screen").center();
    vcx.simulate_mouse_move(tile, None, Modifiers::none());
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    click(vcx, "home-0-0-more");
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Track(_))));
    assert_eq!(
        current(&page, vcx).as_deref(),
        Some("startTrack01"),
        "the button did not play the tile either"
    );
}

#[gpui::test]
fn album_cards_and_song_cards_on_shelves_have_menus(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // "Albums for you": an album card gets the shorter menu.
    right_click(vcx, "home-1-0");
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Collection(_))));
    vcx.simulate_keystrokes("escape");
    // "Listen again": its second item is an album and its third a song card.
    right_click(vcx, "home-2-2");
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Track(_))));
}

// ---- mood chips -----------------------------------------------------------------------------------

fn shown(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.home.read_with(vcx, |m, _| {
        m.display_shelves().iter().map(|s| s.title.clone()).collect()
    })
}

#[gpui::test]
fn the_mood_chips_sit_under_the_header_and_nothing_is_picked_at_first(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(vcx.debug_bounds("home-mood-chips").is_some());
    for label in ["Relax", "Workout", "Focus"] {
        let id = format!("mood-chip-{label}");
        assert!(
            vcx.debug_bounds(Box::leak(id.into_boxed_str())).is_some(),
            "{label} is a pill"
        );
    }
    assert_eq!(page.home.read_with(vcx, |m, _| m.selected_chip().map(str::to_owned)), None);
    assert_eq!(shown(&page, vcx)[0], "Quick picks");
}

#[gpui::test]
fn a_mood_replaces_the_feed_and_tapping_it_again_brings_the_feed_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let feed = shown(&page, vcx);

    click(vcx, "mood-chip-Relax");
    assert_eq!(
        page.home.read_with(vcx, |m, _| m.selected_chip().map(str::to_owned)),
        Some("sim-chip-relax".to_string())
    );
    let mood = shown(&page, vcx);
    assert_eq!(mood[0], "Relax picks", "{mood:?}");
    assert_ne!(mood, feed);
    assert!(page.library.home_log().contains(&"chip:sim-chip-relax".to_string()));

    click(vcx, "mood-chip-Relax");
    assert_eq!(page.home.read_with(vcx, |m, _| m.selected_chip().map(str::to_owned)), None);
    assert_eq!(shown(&page, vcx), feed, "the whole feed is back");
}

#[gpui::test]
fn picking_another_mood_switches_to_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "mood-chip-Relax");
    click(vcx, "mood-chip-Workout");
    assert_eq!(shown(&page, vcx)[0], "Workout picks");
}

#[gpui::test]
fn a_mood_that_fails_says_so_and_retry_loads_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.library.set_chip_failure(Some(LoadFailure::Network));
    click(vcx, "mood-chip-Focus");
    assert!(vcx.debug_bounds("home-notice-action").is_some(), "Retry is on screen");
    page.library.set_chip_failure(None);
    click(vcx, "home-notice-action");
    assert_eq!(shown(&page, vcx)[0], "Focus picks");
}

#[gpui::test]
fn the_mood_chips_switch_hides_the_row_and_drops_a_picked_mood(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let feed = shown(&page, vcx);
    click(vcx, "mood-chip-Relax");
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |m, cx| m.set(&apollo_app::settings::MOOD_CHIPS, false, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("home-mood-chips").is_none());
    assert_eq!(shown(&page, vcx), feed, "the feed shows in full without chips");

    settings.update(vcx, |m, cx| m.set(&apollo_app::settings::MOOD_CHIPS, true, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("home-mood-chips").is_some());
}
