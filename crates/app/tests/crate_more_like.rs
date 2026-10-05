//! "More like the Crate", headless: the rail under the Crate's list on the page (ten songs) and in
//! the docked panel (five), what it filters out, the + that sets a song aside, and above all how
//! little it asks YouTube: nothing before the Crate is opened, one radio per fetch, a new set
//! only after three songs changed or a click, and silence while the shared throttle is cooling.
//! The real shell and views on GPUI's test platform, the simulated library as the "service".

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::crate_more::{self, CrateMoreModel, Phase, DOCK_ROWS, PAGE_ROWS, REFRESH_AFTER};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::CRATE_MORE_LIKE;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use apollo_services_common::Throttle;
use gpui::{
    AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
};

const KEPT: [&str; 3] = ["crateAAAA01", "crateBBBB02", "crateCCCC03"];

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct NoSearch;
impl SearchSource for NoSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults::default())
    }
}

struct Page {
    shell: Entity<Shell>,
    crate_model: Entity<CrateModel>,
    more: Entity<CrateMoreModel>,
    sim: SimLibrary,
}

/// The shell on Home with a Crate of three songs, the rail not opened yet.
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
            track: Track::new("simTrack001", "Midnight Drive"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    player.update(vcx, |m, cx| m.install(ready, cx));
    vcx.run_until_parked();
    let crate_model = shell.read_with(vcx, |s, _| s.crate_model().clone());
    crate_model.update(vcx, |m, cx| {
        for id in KEPT {
            m.add(Track::new(id, format!("Song {id}")), None, cx);
        }
    });
    vcx.run_until_parked();
    let more = vcx.update(|_, cx| crate_more::handle(cx)).expect("the shell makes the rail's model");
    (
        Page {
            shell,
            crate_model,
            more,
            sim,
        },
        vcx,
    )
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

fn show_crate(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
}

fn phase(page: &Page, vcx: &mut VisualTestContext) -> Phase {
    page.more.read_with(vcx, |m, cx| m.phase(cx))
}

fn offered(page: &Page, vcx: &mut VisualTestContext, limit: usize) -> Vec<String> {
    page.more.read_with(vcx, |m, cx| {
        m.visible(limit, cx)
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn add_songs(page: &Page, vcx: &mut VisualTestContext, ids: &[&str]) {
    page.crate_model.update(vcx, |m, cx| {
        for id in ids {
            m.add(Track::new(*id, format!("Song {id}")), None, cx);
        }
    });
    vcx.run_until_parked();
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

// ---- when it asks -----------------------------------------------------------------------------

#[gpui::test]
fn nothing_is_asked_until_the_crate_is_opened(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.run_until_parked();
    assert!(page.sim.radio_seeds().is_empty(), "a Crate that is not on screen asks for nothing");
    // Changing it unseen asks for nothing either.
    add_songs(&page, vcx, &["moreAAAA01", "moreAAAA02", "moreAAAA03", "moreAAAA04"]);
    assert!(page.sim.radio_seeds().is_empty());
    assert_eq!(phase(&page, vcx), Phase::Hidden);
}

#[gpui::test]
fn opening_the_page_fetches_one_radio_seeded_from_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    show_crate(&page, vcx);
    assert_eq!(page.sim.radio_seeds(), vec![KEPT[0].to_owned()], "one request, the first song");
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert_eq!(offered(&page, vcx, PAGE_ROWS).len(), PAGE_ROWS);
    // Under the Crate's rows: the title, Refresh, and the first songs with their +.
    assert!(on_screen(vcx, "crate-rail-title"));
    assert!(on_screen(vcx, "crate-rail-refresh"));
    assert!(on_screen(vcx, "crate-rail-add-0"));
    let title = bounds(vcx, "crate-rail-title");
    let last = bounds(vcx, "crate-row-2");
    assert!(title.origin.y > last.origin.y, "after the last row of the Crate");
    // Coming back to the page asks for nothing more.
    page.shell.update(vcx, |s, cx| s.show_route(Route::Home, cx));
    show_crate(&page, vcx);
    assert_eq!(page.sim.radio_seeds().len(), 1);
}

#[gpui::test]
fn the_docked_panel_shows_five_and_the_page_is_not_needed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds().len(), 1, "opening the dock opens the rail");
    assert!(on_screen(vcx, "dock-rail-add-0"));
    assert!(on_screen(vcx, "dock-rail-add-4"));
    assert!(!on_screen(vcx, "dock-rail-add-5"), "five rows in the panel");
    assert_eq!(offered(&page, vcx, DOCK_ROWS).len(), DOCK_ROWS);
}

#[gpui::test]
fn the_switch_hides_the_rail_and_fetches_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&CRATE_MORE_LIKE, false, cx));
    vcx.run_until_parked();
    show_crate(&page, vcx);
    assert!(page.sim.radio_seeds().is_empty());
    assert_eq!(phase(&page, vcx), Phase::Hidden);
    assert!(!on_screen(vcx, "crate-rail-title"));
    // Switching it on asks, once.
    settings.update(vcx, |s, cx| s.set(&CRATE_MORE_LIKE, true, cx));
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds().len(), 1);
    assert!(on_screen(vcx, "crate-rail-title"));
}

// ---- adding and filtering ---------------------------------------------------------------------

#[gpui::test]
fn the_plus_sets_the_song_aside_and_it_leaves_the_rail_without_another_request(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    show_crate(&page, vcx);
    let before = offered(&page, vcx, PAGE_ROWS);
    click(vcx, "crate-rail-add-0");
    assert_eq!(crate_ids(&page, vcx).last(), Some(&before[0]), "at the bottom of the Crate");
    assert!(
        pill(&page, vcx).is_some_and(|p| p.starts_with("Added to Crate")),
        "the usual pill (it may add the undo hint)"
    );
    let after = offered(&page, vcx, PAGE_ROWS);
    assert!(!after.contains(&before[0]), "it is in the Crate, so it is not offered");
    assert_eq!(&after[..], &before[1..], "the others keep their order");
    assert_eq!(page.sim.radio_seeds().len(), 1, "no request for one song");
}

#[gpui::test]
fn liked_songs_and_songs_in_the_crate_are_not_offered(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    show_crate(&page, vcx);
    let before = offered(&page, vcx, PAGE_ROWS);
    // One liked here, one set aside elsewhere: both leave at once, and the rail refills from what
    // was fetched.
    let writes = page.shell.read_with(vcx, |s, _| s.writes().clone());
    writes.update(vcx, |w, cx| w.set_liked(&before[1], true, cx));
    page.crate_model.update(vcx, |m, cx| {
        m.add(Track::new(before[2].clone(), "Elsewhere"), None, cx);
    });
    vcx.run_until_parked();
    let after = offered(&page, vcx, PAGE_ROWS);
    assert!(!after.contains(&before[1]));
    assert!(!after.contains(&before[2]));
    assert_eq!(after.len(), PAGE_ROWS.min(before.len() - 2));
    assert_eq!(page.sim.radio_seeds().len(), 1);
}

// ---- refreshing -------------------------------------------------------------------------------

#[gpui::test]
fn two_songs_changing_asks_nothing_and_three_ask_once_from_another_seed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    show_crate(&page, vcx);
    assert_eq!(page.sim.radio_seeds().len(), 1);
    add_songs(&page, vcx, &["moreAAAA01", "moreAAAA02"]);
    assert_eq!(page.sim.radio_seeds().len(), 1, "{} changes are not enough", REFRESH_AFTER - 1);
    add_songs(&page, vcx, &["moreAAAA03"]);
    let seeds = page.sim.radio_seeds();
    assert_eq!(seeds.len(), 2, "the third change asks once");
    assert_ne!(seeds[0], seeds[1], "the seed rotates through the Crate");
    assert!(crate_ids(&page, vcx).contains(&seeds[1]));
    // Removals count too.
    page.crate_model.update(vcx, |m, cx| {
        m.remove("moreAAAA01", cx);
        m.remove("moreAAAA02", cx);
    });
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds().len(), 2);
    page.crate_model.update(vcx, |m, cx| m.remove("moreAAAA03", cx));
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds().len(), 3);
}

#[gpui::test]
fn refresh_shows_what_was_fetched_before_it_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    vcx.run_until_parked();
    let first = offered(&page, vcx, DOCK_ROWS);
    // The radio held ten songs and the panel shows five: Refresh moves on to the other five.
    click(vcx, "crate-rail-refresh");
    let second = offered(&page, vcx, DOCK_ROWS);
    assert_eq!(page.sim.radio_seeds().len(), 1, "no request: it was fetched already");
    assert_eq!(second.len(), DOCK_ROWS);
    assert!(second.iter().all(|s| !first.contains(s)), "the songs seen are not shown again");
    // Nothing left to show: the next click asks for another radio, one request.
    click(vcx, "crate-rail-refresh");
    assert_eq!(page.sim.radio_seeds().len(), 2);
    let third = offered(&page, vcx, DOCK_ROWS);
    assert_eq!(third.len(), DOCK_ROWS);
    assert!(third.iter().all(|s| !first.contains(s) && !second.contains(s)));
}

#[gpui::test]
fn emptying_the_crate_forgets_the_rail(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    show_crate(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Ready);
    page.crate_model.update(vcx, |m, cx| m.clear(cx));
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Hidden);
    assert!(offered(&page, vcx, PAGE_ROWS).is_empty());
    // A new Crate starts from nothing: one request for its first song.
    add_songs(&page, vcx, &["freshAAAA01"]);
    assert_eq!(page.sim.radio_seeds().len(), 2);
    assert_eq!(page.sim.radio_seeds()[1], "freshAAAA01");
}

// ---- rate limits ------------------------------------------------------------------------------

#[gpui::test]
fn while_youtube_is_busy_nothing_is_asked_and_nothing_is_said(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let throttle = Throttle::new();
    throttle.report_throttled(Some(Duration::from_secs(120)));
    page.more.update(vcx, |m, _| m.set_throttle(throttle.clone()));
    show_crate(&page, vcx);
    assert!(page.sim.radio_seeds().is_empty(), "the shared cooldown holds the rail back");
    assert_eq!(phase(&page, vcx), Phase::Idle, "a title and Refresh, no error line");
    assert!(on_screen(vcx, "crate-rail-title"));
    // Changes of the Crate and clicks are held back too.
    add_songs(&page, vcx, &["moreAAAA01", "moreAAAA02", "moreAAAA03"]);
    click(vcx, "crate-rail-refresh");
    assert!(page.sim.radio_seeds().is_empty());
}

#[gpui::test]
fn a_too_many_requests_answer_is_quiet_and_not_repeated_on_its_own(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_radio_failure(Some(LoadFailure::Throttled));
    show_crate(&page, vcx);
    assert_eq!(page.sim.radio_seeds().len(), 1);
    assert_eq!(phase(&page, vcx), Phase::Idle, "no \"Couldn't load\" line for being asked to slow down");
    // Two more songs: not enough to ask again.
    add_songs(&page, vcx, &["moreAAAA01", "moreAAAA02"]);
    assert_eq!(page.sim.radio_seeds().len(), 1);
    // A click is the user's own: it asks, and the rail recovers.
    page.sim.set_radio_failure(None);
    click(vcx, "crate-rail-refresh");
    assert_eq!(page.sim.radio_seeds().len(), 2);
    assert_eq!(phase(&page, vcx), Phase::Ready);
}

#[gpui::test]
fn another_failure_says_so_and_retry_asks_once(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_radio_failure(Some(LoadFailure::Network));
    show_crate(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Failed(LoadFailure::Network));
    page.sim.set_radio_failure(None);
    click(vcx, "crate-rail-refresh");
    assert_eq!(page.sim.radio_seeds().len(), 2);
    assert_eq!(phase(&page, vcx), Phase::Ready);
}
