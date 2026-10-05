//! The Crate docked on the right of the main window: the player bar's Crate button and
//! Ctrl+Shift+C open a 320px panel that stays while pages change, shares its slot with the queue
//! (Queue · Crate), overlays the page in narrow windows, is remembered across restarts, and takes
//! songs dragged in from the lists. The real shell and views on GPUI's test platform.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::SettingsBackend;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext,
};

const FIRST: &str = "simTrack001";
const FOUND: [&str; 4] = ["foundTrk001", "foundTrk002", "foundTrk003", "foundTrk004"];
const KEPT: [&str; 3] = ["crateAAAA01", "crateBBBB02", "crateCCCC03"];

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        let song = |id: &str| ListedTrack {
            track: Track::new(id, format!("Song {id}")),
            occurrence: None,
            thumbnail: None,
            explicit: false,
            liked: None,
        };
        Ok(SearchResults {
            songs: FOUND.iter().map(|id| song(id)).collect(),
            ..SearchResults::default()
        })
    }
}

/// A settings store in memory.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, String>>);

impl SettingsBackend for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
}

struct Page {
    shell: Entity<Shell>,
    crate_model: Entity<CrateModel>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let sim = SimLibrary::instant();
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
        queue: vec![LibraryTrack {
            track: Track::new(FIRST, "Midnight Drive"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(TestSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let crate_model = shell.read_with(vcx, |s, _| s.crate_model().clone());
    (Page { shell, crate_model }, vcx)
}

fn bounds(vcx: &mut VisualTestContext, selector: &str) -> Bounds<Pixels> {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"))
}

fn on_screen(vcx: &mut VisualTestContext, selector: &str) -> bool {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    vcx.debug_bounds(selector).is_some()
}

fn click(vcx: &mut VisualTestContext, selector: &str) {
    let at = bounds(vcx, selector).center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn fill(page: &Page, vcx: &mut VisualTestContext, ids: &[&str]) {
    page.crate_model.update(vcx, |m, cx| {
        for id in ids {
            m.add(Track::new(*id, format!("Song {id}")), None, cx);
        }
    });
    vcx.run_until_parked();
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn open_dock(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    vcx.run_until_parked();
}

fn route(page: &Page, vcx: &mut VisualTestContext, route: Route) {
    page.shell
        .update_in(vcx, |s, window, cx| s.choose_route(route, window, cx));
    vcx.run_until_parked();
}

fn search(page: &Page, vcx: &mut VisualTestContext) {
    route(page, vcx, Route::Search);
    let model = page.shell.read_with(vcx, |s, _| s.search_model().clone());
    model.update(vcx, |m, cx| {
        m.set_query("anything", cx);
        m.submit(cx);
    });
    vcx.run_until_parked();
}

/// A point in search row `ix`, on its title side, clear of every action.
fn song_at(vcx: &mut VisualTestContext, ix: usize) -> Point<Pixels> {
    let slot = bounds(vcx, &format!("song-like-{ix}")).center();
    point(slot.x - gpui::px(400.0), slot.y)
}

/// Drag from `start` and let go at `to`.
fn drag(vcx: &mut VisualTestContext, start: Point<Pixels>, to: Point<Pixels>) {
    vcx.simulate_mouse_move(start, None, Modifiers::none());
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let at = point(
            start.x + (to.x - start.x) * t,
            start.y + (to.y - start.y) * t,
        );
        vcx.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::none());
    }
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
}

fn upper_part_of(vcx: &mut VisualTestContext, id: &str) -> Point<Pixels> {
    let b = bounds(vcx, id);
    point(b.center().x, b.origin.y + gpui::px(8.0))
}

#[gpui::test]
fn the_crate_button_docks_a_320px_panel_and_toggles_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    route(&page, vcx, Route::Library);
    assert!(!on_screen(vcx, "crate-dock"), "closed to begin with");
    let page_before = bounds(vcx, "page").size.width;

    click(vcx, "bar-crate");
    assert!(page.shell.read_with(vcx, |s, _| s.crate_dock_open()));
    assert_eq!(bounds(vcx, "crate-dock").size.width, gpui::px(320.0));
    assert_eq!(
        bounds(vcx, "page").size.width,
        page_before - gpui::px(320.0),
        "the page gives up exactly the panel's width"
    );

    click(vcx, "bar-crate");
    assert!(!on_screen(vcx, "crate-dock"), "the button closes it again");
    assert_eq!(bounds(vcx, "page").size.width, page_before);
}

#[gpui::test]
fn ctrl_shift_c_toggles_it_and_it_stays_while_pages_change(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-shift-c");
    assert!(on_screen(vcx, "crate-dock"));
    for to in [Route::Library, Route::Search, Route::Explore, Route::Settings] {
        route(&page, vcx, to);
        assert!(on_screen(vcx, "crate-dock"), "still open on {to:?}");
    }
    // (A page that moved focus into a text field keeps its own keys, as for every plain key.)
    page.shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    vcx.simulate_keystrokes("ctrl-shift-c");
    assert!(!on_screen(vcx, "crate-dock"));
}

#[gpui::test]
fn the_crate_page_and_now_playing_have_the_window_to_themselves(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_dock(&page, vcx);
    route(&page, vcx, Route::Crate);
    assert!(!on_screen(vcx, "crate-dock"), "the page is the same list");
    assert!(
        page.shell.read_with(vcx, |s, _| s.crate_dock_open()),
        "but it is still open"
    );
    route(&page, vcx, Route::Home);
    assert!(on_screen(vcx, "crate-dock"));

    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "crate-dock"));
    page.shell.update(vcx, |s, cx| s.set_now_playing(false, cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "crate-dock"), "back when Now Playing closes");
}

#[gpui::test]
fn it_shares_the_slot_with_the_queue_as_tabs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "queue-panel"));
    assert!(on_screen(vcx, "dock-tab-Queue"), "the queue panel carries the control");

    // Opening the Crate takes the slot: never both at once.
    click(vcx, "bar-crate");
    assert!(on_screen(vcx, "crate-dock") && !on_screen(vcx, "queue-panel"));
    assert!(page.shell.read_with(vcx, |s, _| !s.queue_open()));

    // The tabs swap them either way.
    click(vcx, "dock-tab-Queue");
    assert!(on_screen(vcx, "queue-panel") && !on_screen(vcx, "crate-dock"));
    click(vcx, "dock-tab-Crate");
    assert!(on_screen(vcx, "crate-dock") && !on_screen(vcx, "queue-panel"));

    // And the queue button takes it back.
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "queue-panel") && !on_screen(vcx, "crate-dock"));
    assert!(page.shell.read_with(vcx, |s, _| !s.crate_dock_open()));
}

#[gpui::test]
fn below_1100px_the_panel_overlays_the_page_instead_of_squeezing_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    route(&page, vcx, Route::Library);
    vcx.simulate_resize(gpui::size(gpui::px(1000.0), gpui::px(700.0)));
    vcx.run_until_parked();
    let page_before = bounds(vcx, "page").size.width;
    open_dock(&page, vcx);
    let dock = bounds(vcx, "crate-dock");
    assert_eq!(dock.size.width, gpui::px(320.0));
    assert_eq!(bounds(vcx, "page").size.width, page_before, "the page keeps its width");
    assert_eq!(dock.right(), bounds(vcx, "page").right(), "over the page's right edge");

    // From 1100px up it squeezes the page instead.
    page.shell.update(vcx, |s, cx| s.set_crate_dock(false, cx));
    vcx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(700.0)));
    vcx.run_until_parked();
    let wide = bounds(vcx, "page").size.width;
    open_dock(&page, vcx);
    assert_eq!(bounds(vcx, "page").size.width, wide - gpui::px(320.0));
}

#[gpui::test]
fn the_open_state_is_remembered_across_restarts(cx: &mut TestAppContext) {
    let store = Arc::new(Memory::default());
    {
        let (page, vcx) = setup(cx);
        page.shell
            .update(vcx, |s, cx| s.install_crate_dock(Some(store.clone()), cx));
        vcx.run_until_parked();
        assert!(!page.shell.read_with(vcx, |s, _| s.crate_dock_open()));
        page.shell.update(vcx, |s, cx| s.toggle_crate_dock(cx));
        vcx.run_until_parked();
        // The write goes through the background executor.
        for _ in 0..200 {
            if store.get("crate_dock_open").as_deref() == Some("on") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(store.get("crate_dock_open").as_deref(), Some("on"));
    }
    let (page, vcx) = setup(cx);
    page.shell
        .update(vcx, |s, cx| s.install_crate_dock(Some(store.clone()), cx));
    vcx.run_until_parked();
    assert!(page.shell.read_with(vcx, |s, _| s.crate_dock_open()), "reopened as left");
    assert!(on_screen(vcx, "crate-dock"));
    page.shell.update(vcx, |s, cx| s.toggle_crate_dock(cx));
    vcx.run_until_parked();
    for _ in 0..200 {
        if store.get("crate_dock_open").as_deref() == Some("off") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(store.get("crate_dock_open").as_deref(), Some("off"));
}

#[gpui::test]
fn the_panel_lists_the_crate_in_order_and_its_rows_are_arranged_by_drag(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    open_dock(&page, vcx);
    for ix in 0..3 {
        assert!(on_screen(vcx, &format!("dock-row-{ix}")), "row {ix}");
    }

    // The last row, let go on the upper half of the first: it lands before it.
    let start = bounds(vcx, "dock-row-2").center();
    let to = upper_part_of(vcx, "dock-row-0");
    drag(vcx, start, to);
    assert_eq!(crate_ids(&page, vcx), ["crateCCCC03", "crateAAAA01", "crateBBBB02"]);
}

#[gpui::test]
fn a_song_dragged_from_search_lands_where_it_is_dropped(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    search(&page, vcx);
    open_dock(&page, vcx);

    // Onto the upper half of the second row: before it.
    let start = song_at(vcx, 1);
    let to = upper_part_of(vcx, "dock-row-1");
    drag(vcx, start, to);
    assert_eq!(
        crate_ids(&page, vcx),
        ["crateAAAA01", "foundTrk002", "crateBBBB02", "crateCCCC03"]
    );

    // Below the last row: at the bottom.
    let last = bounds(vcx, "dock-row-3");
    let end = point(last.center().x, last.bottom() + gpui::px(20.0));
    let start = song_at(vcx, 0);
    drag(vcx, start, end);
    assert_eq!(
        crate_ids(&page, vcx),
        ["crateAAAA01", "foundTrk002", "crateBBBB02", "crateCCCC03", "foundTrk001"]
    );

    // A song the crate already holds moves instead of doubling.
    let start = song_at(vcx, 1);
    let to = upper_part_of(vcx, "dock-row-0");
    drag(vcx, start, to);
    assert_eq!(
        crate_ids(&page, vcx),
        ["foundTrk002", "crateAAAA01", "crateBBBB02", "crateCCCC03", "foundTrk001"]
    );
}

#[gpui::test]
fn picked_songs_drag_together_and_an_empty_panel_takes_the_first_drop(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    open_dock(&page, vcx);
    assert!(on_screen(vcx, "crate-dock-empty"), "the hint shows while it is empty");

    // Ctrl+click two rows, then drag one of them: both go, in list order.
    for ix in [2, 0] {
        let at = song_at(vcx, ix);
        vcx.simulate_click(at, Modifiers::control());
        vcx.run_until_parked();
    }
    let start = song_at(vcx, 2);
    let to = bounds(vcx, "crate-dock-empty").center();
    drag(vcx, start, to);
    assert_eq!(crate_ids(&page, vcx), ["foundTrk001", "foundTrk003"]);
    assert!(!on_screen(vcx, "crate-dock-empty"));
}
