//! Headless tests for Back and Forward: the real shell on GPUI's test platform, with the simulated
//! library (no latency) and deterministic mock playback adapters. Search answers with one album,
//! one artist and one playlist card.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::{ArtistTarget, LibraryModel, Load, PlayRequest, Target};
use apollo_app::library::sim::{self, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::search::{SearchSource, DEBOUNCE};
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{Hit, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use apollo_app::library::model::Tab;
use apollo_app::ui::nav_history::Place;
use apollo_domain::library::Collection;
use gpui::{point, px, MouseButton, MouseDownEvent, NavigationDirection};


struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct TestThumbs;
impl ThumbSource for TestThumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        // Wide, like an artist banner: the page must crop it, not squeeze it.
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(100.0).render(96)))
    }
}

fn hit(id: &str, title: &str, subtitle: Option<&str>) -> Hit {
    Hit {
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: subtitle.map(str::to_owned),
        thumbnail: Some("https://t/x".into()),
        explicit: false,
    }
}

/// One card of each kind, for any query.
struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults {
            albums: vec![hit(
                &format!("{}Night Shift", sim::SEARCH_ALBUM_PREFIX),
                "Night Shift",
                Some("Neon Arcade • 2024"),
            )],
            artists: vec![hit(
                &format!("{}Neon Arcade", sim::SEARCH_ARTIST_PREFIX),
                "Neon Arcade",
                None,
            )],
            playlists: vec![hit("VLPLsim1", "Night Drive Mix", Some("Apollo"))],
            ..SearchResults::default()
        })
    }
}

fn prepared() -> Prepared {
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
        search: Arc::new(TestSearch),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(SimLibrary::instant()),
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    view: Entity<LibraryView>,
    model: Entity<LibraryModel>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
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
    let ready = prepared();
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    // These pages are about what gets queued from a list, so no radio is queued behind it.
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    let model = view.read_with(vcx, |v, _| v.model().clone());
    (
        Page {
            player,
            shell,
            view,
            model,
        },
        vcx,
    )
}

/// Ctrl+K, a query, and the cards it answers with on screen.
fn search(page: &Page, vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("ctrl-k");
    // Text nothing in the simulated library holds, so only the service's answer is on the page.
    vcx.simulate_keystrokes("z q x j");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    page.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Search));
}

fn click_card(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn route(page: &Page, vcx: &mut VisualTestContext) -> Route {
    page.shell.read_with(vcx, |s, _| s.route())
}

fn go(page: &Page, vcx: &mut VisualTestContext, to: Route) {
    page.shell
        .update_in(vcx, |s, window, cx| s.choose_route(to, window, cx));
    vcx.run_until_parked();
}

fn places(page: &Page, vcx: &mut VisualTestContext) -> (Vec<Place>, usize) {
    page.shell.read_with(vcx, |s, _| s.history_places())
}

fn query(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.shell.read_with(vcx, |s, cx| {
        s.search_page().read(cx).field().read(cx).text().to_owned()
    })
}

fn collection(page: &Page, vcx: &mut VisualTestContext, id: &str) -> Collection {
    page.model.read_with(vcx, |m, _| {
        [Tab::Playlists, Tab::Albums]
            .into_iter()
            .flat_map(|t| m.list(t).iter())
            .find(|c| c.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("no collection {id}"))
    })
}

fn open_collection(page: &Page, vcx: &mut VisualTestContext, id: &str) {
    let c = collection(page, vcx, id);
    page.shell
        .update_in(vcx, |s, window, cx| s.open_target(Target::Collection(c), window, cx));
    vcx.run_until_parked();
}

fn can(page: &Page, vcx: &mut VisualTestContext) -> (bool, bool) {
    page.shell
        .read_with(vcx, |s, cx| (s.can_go_back(cx), s.can_go_forward()))
}

#[gpui::test]
fn the_nav_rail_builds_a_history_that_alt_left_and_alt_right_walk(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(can(&page, vcx), (false, false), "nowhere to go at the start");
    go(&page, vcx, Route::Explore);
    go(&page, vcx, Route::Library);
    go(&page, vcx, Route::Settings);
    assert_eq!(
        places(&page, vcx),
        (
            vec![
                Place::Home,
                Place::Explore,
                Place::Library(Tab::Playlists),
                Place::Settings
            ],
            3
        )
    );
    assert_eq!(can(&page, vcx), (true, false));

    vcx.simulate_keystrokes("alt-left");
    assert_eq!(route(&page, vcx), Route::Library);
    vcx.simulate_keystrokes("alt-left");
    assert_eq!(route(&page, vcx), Route::Explore);
    assert_eq!(can(&page, vcx), (true, true));
    vcx.simulate_keystrokes("alt-right");
    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(places(&page, vcx).1, 2, "walking does not add entries");
}

#[gpui::test]
fn choosing_the_page_you_are_on_adds_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    go(&page, vcx, Route::Explore);
    go(&page, vcx, Route::Home);
    go(&page, vcx, Route::Home);
    assert_eq!(
        places(&page, vcx),
        (vec![Place::Home, Place::Explore, Place::Home], 2)
    );
}

#[gpui::test]
fn a_new_page_from_the_middle_drops_what_was_ahead(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    go(&page, vcx, Route::Crate);
    vcx.simulate_keystrokes("alt-left alt-left");
    assert_eq!(route(&page, vcx), Route::Home);
    assert_eq!(can(&page, vcx), (false, true));
    go(&page, vcx, Route::Settings);
    assert_eq!(
        places(&page, vcx),
        (vec![Place::Home, Place::Settings], 1)
    );
    assert_eq!(can(&page, vcx), (true, false));
}

#[gpui::test]
fn backspace_goes_back_except_in_a_text_field(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    vcx.simulate_keystrokes("backspace");
    assert_eq!(route(&page, vcx), Route::Home, "Backspace is Back");

    // In the Search field it deletes a letter and goes nowhere.
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("a b");
    assert_eq!(query(&page, vcx), "ab");
    vcx.simulate_keystrokes("backspace");
    assert_eq!(query(&page, vcx), "a");
    assert_eq!(route(&page, vcx), Route::Search);
    // Alt+Left leaves the field, as it does in a browser's address bar.
    vcx.simulate_keystrokes("alt-left");
    assert_eq!(route(&page, vcx), Route::Home);
}

#[gpui::test]
fn the_title_bar_buttons_go_back_and_forward(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    let at = vcx
        .debug_bounds("history-back-slot")
        .expect("Back is in the title bar")
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(route(&page, vcx), Route::Home);
    let at = vcx
        .debug_bounds("history-forward-slot")
        .expect("Forward is in the title bar")
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    assert_eq!(route(&page, vcx), Route::Explore);
}

#[gpui::test]
fn the_mouse_side_buttons_go_back_and_forward(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    let at = point(px(400.0), px(300.0));
    vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Navigate(NavigationDirection::Back),
        click_count: 1,
        first_mouse: false,
    });
    assert_eq!(route(&page, vcx), Route::Home);
    vcx.simulate_event(MouseDownEvent {
        position: at,
        modifiers: Modifiers::none(),
        button: MouseButton::Navigate(NavigationDirection::Forward),
        click_count: 1,
        first_mouse: false,
    });
    assert_eq!(route(&page, vcx), Route::Explore);
}

#[gpui::test]
fn back_from_a_page_returns_to_the_results_and_forward_reopens_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    let typed = query(&page, vcx);
    assert!(!typed.is_empty());
    click_card(vcx, "search-album-0");
    assert_eq!(route(&page, vcx), Route::Library);
    assert!(page.model.read_with(vcx, |m, _| m.detail().is_some()));

    vcx.simulate_keystrokes("alt-left");
    assert_eq!(route(&page, vcx), Route::Search);
    assert_eq!(query(&page, vcx), typed, "the search is as it was left");
    assert_eq!(page.model.read_with(vcx, |m, _| m.depth()), 0);
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.nav_route()),
        Route::Search
    );

    vcx.simulate_keystrokes("alt-right");
    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(
        page.model
            .read_with(vcx, |m, _| m.detail().map(|d| d.collection.title.clone())),
        Some("Night Shift".to_owned()),
        "Forward opens the same page"
    );
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.nav_route()),
        Route::Search,
        "the nav keeps the route it was opened from lit"
    );
}

#[gpui::test]
fn the_page_buttons_and_escape_go_through_the_same_history(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    open_collection(&page, vcx, sim::LONG_ID);
    assert!(matches!(places(&page, vcx).0.last(), Some(Place::Collection(_))));
    vcx.simulate_keystrokes("escape");
    assert_eq!(route(&page, vcx), Route::Explore, "back to where it was opened");
    assert_eq!(can(&page, vcx), (true, true), "and Forward has the page");
    vcx.simulate_keystrokes("alt-right");
    assert_eq!(route(&page, vcx), Route::Library);
}

#[gpui::test]
fn a_library_tab_is_a_place(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Library);
    page.view
        .update(vcx, |v, cx| v.set_tab(Tab::Albums, cx));
    vcx.run_until_parked();
    assert_eq!(
        places(&page, vcx).0.last(),
        Some(&Place::Library(Tab::Albums))
    );
    vcx.simulate_keystrokes("alt-left");
    assert_eq!(
        page.model.read_with(vcx, |m, _| m.tab()),
        Tab::Playlists,
        "Back returns to the Playlists tab"
    );
    vcx.simulate_keystrokes("alt-right");
    assert_eq!(page.model.read_with(vcx, |m, _| m.tab()), Tab::Albums);
}

#[gpui::test]
fn search_text_comes_back_with_its_place(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("a b c");
    vcx.run_until_parked();
    go(&page, vcx, Route::Home);
    page.shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("x y z");
    vcx.run_until_parked();
    assert_eq!(query(&page, vcx), "abcxyz");
    page.shell.update(vcx, |s, cx| {
        s.search_page().read(cx).field().clone().update(cx, |f, cx| f.set_text("xyz", cx));
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("alt-left");
    assert_eq!(route(&page, vcx), Route::Home);
    vcx.simulate_keystrokes("alt-left");
    assert_eq!(route(&page, vcx), Route::Search);
    assert_eq!(query(&page, vcx), "abc", "that visit's own text, not the later one");
}

#[gpui::test]
fn a_page_comes_back_scrolled_where_it_was(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Library);
    open_collection(&page, vcx, sim::LONG_ID);
    page.view.update(vcx, |v, cx| {
        v.set_scroll_offset(point(px(0.0), px(-600.0)), cx)
    });
    vcx.run_until_parked();
    let before = page.view.read_with(vcx, |v, cx| v.scroll_offset(cx).y);
    assert!(f32::from(before) < -100.0, "the list is scrolled: {before:?}");

    vcx.simulate_keystrokes("alt-left");
    assert_eq!(page.model.read_with(vcx, |m, _| m.depth()), 0);
    vcx.simulate_keystrokes("alt-right");
    vcx.run_until_parked();
    // The restore takes a few frames to hold.
    for _ in 0..6 {
        vcx.executor().advance_clock(std::time::Duration::from_millis(20));
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
    }
    let after = page.view.read_with(vcx, |v, cx| v.scroll_offset(cx).y);
    assert!(
        (f32::from(after) - f32::from(before)).abs() < 2.0,
        "scrolled to {after:?}, was {before:?}"
    );
}

#[gpui::test]
fn closing_now_playing_steps_back_instead_of_adding_a_place(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Explore);
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    assert_eq!(
        places(&page, vcx),
        (vec![Place::Home, Place::Explore, Place::NowPlaying], 2)
    );
    vcx.simulate_keystrokes("escape");
    assert_eq!(
        places(&page, vcx),
        (vec![Place::Home, Place::Explore, Place::NowPlaying], 1)
    );
    // Forward reopens Now Playing, Back closes it.
    vcx.simulate_keystrokes("alt-right");
    assert!(page.shell.read_with(vcx, |s, _| s.now_playing_open()));
    vcx.simulate_keystrokes("alt-left");
    assert!(!page.shell.read_with(vcx, |s, _| s.now_playing_open()));
    assert_eq!(route(&page, vcx), Route::Explore);
}

#[gpui::test]
fn a_playlists_filter_text_comes_back_with_its_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    go(&page, vcx, Route::Library);
    open_collection(&page, vcx, sim::LONG_ID);
    page.view
        .update_in(vcx, |v, window, cx| v.restore_filter("night", window, cx));
    vcx.run_until_parked();
    assert_eq!(
        page.view.read_with(vcx, |v, cx| v.filter_snapshot(cx)),
        Some("night".to_owned())
    );
    page.shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    vcx.simulate_keystrokes("alt-left");
    assert_eq!(page.model.read_with(vcx, |m, _| m.depth()), 0);
    vcx.simulate_keystrokes("alt-right");
    vcx.run_until_parked();
    assert_eq!(
        page.view.read_with(vcx, |v, cx| v.filter_snapshot(cx)),
        Some("night".to_owned()),
        "the filter box holds what it held"
    );
}
