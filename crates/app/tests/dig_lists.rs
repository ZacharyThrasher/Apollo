//! Dig from here on the lists that have a cursor: the Library, Search, the queue and the Crate.
//! D digs from the row's song (the pointer's row first in the Library), hovering never asks for
//! anything, and Esc ends the dig from wherever it started. The real shell on GPUI's test platform.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::{LibraryModel, Tab};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{Collection, LIKED_MUSIC_BROWSE_ID};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Focusable, Modifiers, TestAppContext, VisualTestContext};

const PLAYING: &str = "simTrack001";
const QUEUED: [&str; 2] = ["queuedTrk01", "queuedTrk02"];
const FOUND: [&str; 3] = ["foundTrk001", "foundTrk002", "foundTrk003"];
const KEPT: [&str; 2] = ["crateAAAA01", "crateBBBB02"];

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
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

struct Page {
    shell: Entity<Shell>,
    player: Entity<PlayerModel>,
    sim: SimLibrary,
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
    let queue = std::iter::once(PLAYING)
        .chain(QUEUED)
        .map(|id| LibraryTrack { track: Track::new(id, format!("Song {id}")) })
        .collect();
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue,
        art: Arc::new(NoArt),
        search: Arc::new(TestSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (Page { shell, player, sim }, vcx)
}

fn route(page: &Page, vcx: &mut VisualTestContext, route: Route) {
    page.shell
        .update_in(vcx, |s, window, cx| s.choose_route(route, window, cx));
    vcx.run_until_parked();
}

fn digging(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.player.read_with(vcx, |m, _| m.view().dig.is_some())
}

fn title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player.read_with(vcx, |m, _| m.view().title.to_string())
}

fn focus_panel<V: Focusable + 'static>(vcx: &mut VisualTestContext, panel: &Entity<V>) {
    vcx.update(|window, cx| {
        let focus = panel.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
}

// ---- the Library ---------------------------------------------------------------------------------

fn open_liked(page: &Page, vcx: &mut VisualTestContext) -> (Entity<LibraryView>, Entity<LibraryModel>) {
    route(page, vcx, Route::Library);
    let view = page.shell.read_with(vcx, |s, _| s.library().clone());
    let model = view.read_with(vcx, |v, _| v.model().clone());
    let liked: Collection = model.read_with(vcx, |m, _| {
        [Tab::Playlists, Tab::Albums]
            .into_iter()
            .flat_map(|t| m.list(t).iter())
            .find(|c| c.id.as_str() == LIKED_MUSIC_BROWSE_ID)
            .cloned()
            .expect("liked music is listed")
    });
    view.update_in(vcx, |v, window, cx| v.open(liked, window, cx));
    vcx.run_until_parked();
    (view, model)
}

fn row_id(model: &Entity<LibraryModel>, vcx: &mut VisualTestContext, ix: usize) -> String {
    model.read_with(vcx, |m, _| m.list_rows()[ix].track.video_id.as_str().to_owned())
}

fn row_center(vcx: &mut VisualTestContext, ix: usize) -> gpui::Point<gpui::Pixels> {
    let name: &'static str = Box::leak(format!("library-row-{ix}").into_boxed_str());
    vcx.debug_bounds(name)
        .unwrap_or_else(|| panic!("row {ix} is on screen"))
        .center()
}

#[gpui::test]
fn d_in_a_library_list_digs_from_the_pointers_row_and_hovering_alone_asks_for_nothing(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    let (_view, model) = open_liked(&page, vcx);
    let hovered = row_id(&model, vcx, 3);
    let at = row_center(vcx, 3);
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.run_until_parked();
    assert!(page.sim.radio_seeds().is_empty(), "hovering sends nothing");
    vcx.simulate_keystrokes("d");
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds(), [hovered]);
    assert!(digging(&page, vcx));
}

#[gpui::test]
fn d_with_the_pointer_off_the_rows_digs_from_the_keyboards_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let (_view, model) = open_liked(&page, vcx);
    let second = row_id(&model, vcx, 1);
    vcx.simulate_keystrokes("down d");
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds(), [second]);
}

#[gpui::test]
fn esc_in_a_library_list_ends_a_dig_before_it_goes_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let (_view, _model) = open_liked(&page, vcx);
    vcx.simulate_keystrokes("down d");
    vcx.run_until_parked();
    assert!(digging(&page, vcx));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!digging(&page, vcx));
    assert_eq!(title(&page, vcx), format!("Song {PLAYING}"));
}

// ---- Search ---------------------------------------------------------------------------------------

fn search_for(page: &Page, vcx: &mut VisualTestContext) {
    route(page, vcx, Route::Search);
    let search = page.shell.read_with(vcx, |s, _| s.search_model().clone());
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text("anything", cx));
    search.update(vcx, |m, cx| m.submit(cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn d_on_a_search_result_digs_from_it_and_esc_ends_the_dig(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down down d");
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds(), [FOUND[1].to_owned()]);
    assert!(digging(&page, vcx));
    // Esc is the dig's, not "back to the search field".
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!digging(&page, vcx));
    assert_eq!(title(&page, vcx), format!("Song {PLAYING}"));
}

#[gpui::test]
fn typing_d_in_the_search_field_types_a_d(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    route(&page, vcx, Route::Search);
    vcx.simulate_keystrokes("d");
    vcx.run_until_parked();
    assert!(page.sim.radio_seeds().is_empty());
    assert!(!digging(&page, vcx));
}

// ---- the queue and the Crate ----------------------------------------------------------------------

#[gpui::test]
fn d_in_the_queue_digs_from_the_cursor_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    focus_panel(vcx, &panel);
    // The first press shows the cursor on the playing row; one more reaches the first up next.
    vcx.simulate_keystrokes("down down d");
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds(), [QUEUED[0].to_owned()]);
}

#[gpui::test]
fn d_on_a_crate_row_digs_from_it_and_k_keeps_back_into_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let crate_model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    crate_model.update(vcx, |m, cx| {
        for id in KEPT {
            m.add(Track::new(id, format!("Song {id}")), None, cx);
        }
    });
    route(&page, vcx, Route::Crate);
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    focus_panel(vcx, &crate_page);
    vcx.simulate_keystrokes("down d");
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds(), [KEPT[0].to_owned()]);
    assert!(digging(&page, vcx));
    // The radio leaves out what the Crate holds, and K sets the song being skimmed aside.
    vcx.simulate_keystrokes("k");
    vcx.run_until_parked();
    assert_eq!(crate_model.read_with(vcx, |m, _| m.len()), 3);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!digging(&page, vcx), "Esc on the Crate page ends the dig first");
}
