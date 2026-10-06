//! Nothing resolves on its own: with the real shell on screen, a page left alone for minutes, a
//! pointer parked on a row, a cursor moved with the keys, a docked Crate sitting open, all ask the
//! resolver for nothing beyond what the user did. The thread that wrote this was chasing a
//! prefetch queue that stayed full for twenty minutes with the pointer hardly moving (the second
//! YouTube 429); a loop that re-enqueues speculative work on a frame or a tick would show here as
//! a count that grows with the idle time.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::LoadId;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::MockEngine;
use apollo_runtime::{ResolveRequest, Resolver};
use gpui::{
    point, AppContext, Entity, Modifiers, Pixels, Point, TestAppContext, VisualTestContext,
};

const FIRST: &str = "simTrack001";

/// What the resolver was asked, in order.
#[derive(Default)]
struct Asked {
    prefetched: Vec<String>,
    started: Vec<String>,
}

struct Recorder(Arc<Mutex<Asked>>);

impl Resolver for Recorder {
    fn start(&mut self, request: ResolveRequest) {
        self.0.lock().unwrap().started.push(request.track.video_id.as_str().to_owned());
    }
    fn cancel(&mut self, _: LoadId) {}
    fn prefetch(&mut self, track: &Track) {
        self.0.lock().unwrap().prefetched.push(track.video_id.as_str().to_owned());
    }
}

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        let song = |n: usize| ListedTrack {
            track: Track::new(format!("found{n:06}"), format!("Song {n}")),
            occurrence: None,
            thumbnail: None,
            explicit: false,
            liked: None,
        };
        Ok(SearchResults {
            songs: (0..8).map(song).collect(),
            ..SearchResults::default()
        })
    }
}

struct Page {
    shell: Entity<Shell>,
    player: Entity<PlayerModel>,
    asked: Arc<Mutex<Asked>>,
}

fn setup(cx: &mut TestAppContext, hover_preload: bool) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let asked = Arc::new(Mutex::new(Asked::default()));
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, _| m.set_preload_hover(hover_preload));
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(Recorder(asked.clone())),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new(FIRST, "Midnight Drive"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(TestSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (Page { shell, player, asked }, vcx)
}

fn route(page: &Page, vcx: &mut VisualTestContext, route: Route) {
    page.shell
        .update_in(vcx, |s, window, cx| s.choose_route(route, window, cx));
    vcx.run_until_parked();
}

fn search(page: &Page, vcx: &mut VisualTestContext) {
    route(page, vcx, Route::Search);
    let model = page.shell.read_with(vcx, |s, _| s.search_model().clone());
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text("anything", cx));
    model.update(vcx, |m, cx| m.submit(cx));
    vcx.run_until_parked();
}

/// Let `seconds` of idle time pass in quarter-second steps, drawing a frame in each.
fn idle(vcx: &mut VisualTestContext, seconds: u64) {
    for _ in 0..seconds * 4 {
        vcx.executor().advance_clock(Duration::from_millis(250));
        vcx.update(|window, _| window.refresh());
        vcx.run_until_parked();
    }
}

fn prefetched(page: &Page) -> Vec<String> {
    page.asked.lock().unwrap().prefetched.clone()
}

fn song_at(vcx: &mut VisualTestContext, ix: usize) -> Point<Pixels> {
    let selector: &'static str = Box::leak(format!("song-like-{ix}").into_boxed_str());
    let slot = vcx.debug_bounds(selector).expect("row on screen").center();
    point(slot.x - gpui::px(400.0), slot.y)
}

#[gpui::test]
fn a_pointer_parked_on_a_row_asks_once_with_hover_preload_on_and_never_off(cx: &mut TestAppContext) {
    for (on, want) in [(true, 1usize), (false, 0)] {
        let (page, vcx) = setup(cx, on);
        search(&page, vcx);
        let before = prefetched(&page).len();
        let at = song_at(vcx, 2);
        vcx.simulate_mouse_move(at, None, Modifiers::none());
        idle(vcx, 20);
        let got = prefetched(&page);
        assert_eq!(got.len() - before, want, "hover preload {on}: {got:?}");
    }
}

#[gpui::test]
fn an_idle_search_page_resolves_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    search(&page, vcx);
    let before = prefetched(&page).len();
    idle(vcx, 20);
    assert_eq!(prefetched(&page).len(), before, "{:?}", prefetched(&page));
    assert!(page.asked.lock().unwrap().started.is_empty());
}

#[gpui::test]
fn a_docked_crate_and_a_playing_queue_left_alone_resolve_nothing_more(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, true);
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    let crate_model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    crate_model.update(vcx, |m, cx| {
        for n in 0..30 {
            m.add(Track::new(format!("crate{n:06}"), format!("Crate {n}")), None, cx);
        }
    });
    vcx.run_until_parked();
    page.player.update(vcx, |m, cx| {
        m.play_tracks(
            (0..30).map(|n| Track::new(format!("queue{n:06}"), format!("Queue {n}"))).collect(),
            Some(0),
            Some(false),
            cx,
        )
    });
    vcx.run_until_parked();
    let after_start = prefetched(&page);
    idle(vcx, 30);
    assert_eq!(prefetched(&page), after_start, "nothing new in thirty idle seconds");
}

#[gpui::test]
fn collecting_with_c_and_auto_advance_resolves_nothing_for_the_rows_passed(cx: &mut TestAppContext) {
    // Hover preload ON, the old behaviour's worst case: every C steps the cursor to the next
    // row and, once the cursor rested there a moment, that row used to be pre-resolved.
    let (page, vcx) = setup(cx, true);
    search(&page, vcx);
    vcx.simulate_keystrokes("down");
    idle(vcx, 1);
    let after_first_row = prefetched(&page);
    for _ in 0..6 {
        vcx.simulate_keystrokes("c");
        idle(vcx, 2);
    }
    let crate_model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    let added: Vec<String> = crate_model.read_with(vcx, |m, _| {
        m.items().iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    });
    assert!(added.len() >= 4, "the cursor walked down the results setting songs aside: {added:?}");
    assert_eq!(
        prefetched(&page),
        after_first_row,
        "the rows the cursor stepped past were not resolved"
    );
}

#[gpui::test]
fn moving_the_cursor_with_the_arrows_asks_only_when_hover_preload_is_on(cx: &mut TestAppContext) {
    for (on, want) in [(true, 1usize), (false, 0)] {
        let (page, vcx) = setup(cx, on);
        search(&page, vcx);
        let before = prefetched(&page).len();
        vcx.simulate_keystrokes("down down down");
        idle(vcx, 2);
        assert_eq!(prefetched(&page).len() - before, want, "hover preload {on}");
    }
}
