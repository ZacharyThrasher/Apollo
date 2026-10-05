//! Crate speed bits: auto-advance after C and A (Search, Library, queue, Crate), a running
//! length in the Crate's header and the dock's, Ctrl+Shift+C jumping into the dock and Esc
//! coming back, and the Crate button's count. The real shell and views on GPUI's test platform.
//! (Skim's K moving on is in `skim.rs`.)

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::{SettingsModel, AUTO_ADVANCE};
use apollo_app::style::Fonts;
use apollo_app::target_playlist::TargetModel;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

const PLAYING: &str = "simTrack001";
const QUEUED: [&str; 3] = ["queuedTrk01", "queuedTrk02", "queuedTrk03"];
const FOUND: [&str; 4] = ["foundTrk001", "foundTrk002", "foundTrk003", "foundTrk004"];
const KEPT: [&str; 3] = ["crateAAAA01", "crateBBBB02", "crateCCCC03"];
const TARGET: &str = "PLsimroad";

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
    target: Entity<TargetModel>,
    settings: Entity<SettingsModel>,
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
        .map(|id| LibraryTrack {
            track: Track::new(id, format!("Song {id}")),
        })
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
    let (target, settings) = shell.read_with(vcx, |s, cx| (s.target_model().clone(), s.settings(cx)));
    (
        Page {
            shell,
            target,
            settings,
            sim,
        },
        vcx,
    )
}

fn set_target(page: &Page, vcx: &mut VisualTestContext) {
    page.target.update(vcx, |m, cx| m.set(TARGET, "Road Trip", cx));
    vcx.run_until_parked();
}

fn advance_off(page: &Page, vcx: &mut VisualTestContext) {
    page.settings.update(vcx, |m, cx| m.set(&AUTO_ADVANCE, false, cx));
}

fn added(page: &Page) -> Vec<(String, String)> {
    page.sim.added()
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

fn fill(page: &Page, vcx: &mut VisualTestContext, ids: &[&str]) {
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    model.update(vcx, |m, cx| {
        for id in ids {
            m.add(Track::new(*id, format!("Song {id}")), None, cx);
        }
    });
    vcx.run_until_parked();
}

/// A song of `minutes` minutes (none: no length known) into the Crate.
fn fill_timed(page: &Page, vcx: &mut VisualTestContext, id: &str, minutes: Option<u64>) {
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    model.update(vcx, |m, cx| {
        let mut track = Track::new(id, format!("Song {id}"));
        track.duration = minutes.map(|m| Duration::from_secs(m * 60));
        m.add(track, None, cx);
    });
    vcx.run_until_parked();
}

fn route(page: &Page, vcx: &mut VisualTestContext, route: Route) {
    page.shell
        .update_in(vcx, |s, window, cx| s.choose_route(route, window, cx));
    vcx.run_until_parked();
}

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

fn selected_in_search(page: &Page, vcx: &mut VisualTestContext) -> Option<usize> {
    page.shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).selected())
}

fn focus_panel<V: gpui::Focusable + 'static>(vcx: &mut VisualTestContext, panel: &Entity<V>) {
    vcx.update(|window, cx| {
        let focus = panel.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
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

// ---- 2: auto-advance ---------------------------------------------------------------------------

#[gpui::test]
fn c_on_search_results_steps_down_the_list_one_song_at_a_time(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down c");
    assert_eq!(crate_ids(&page, vcx), [FOUND[0]]);
    assert_eq!(selected_in_search(&page, vcx), Some(1), "the cursor stepped on");
    vcx.simulate_keystrokes("c c");
    assert_eq!(crate_ids(&page, vcx), [FOUND[0], FOUND[1], FOUND[2]]);
    assert_eq!(selected_in_search(&page, vcx), Some(3));
    // The last row: the cursor stays where it is.
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx).len(), 4);
    assert_eq!(selected_in_search(&page, vcx), Some(3), "the last row stays put");
}

#[gpui::test]
fn picked_songs_win_the_cursor_does_not_walk_away(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down shift-down c");
    assert_eq!(crate_ids(&page, vcx), [FOUND[0], FOUND[1]]);
    assert_eq!(selected_in_search(&page, vcx), Some(1), "a bulk C leaves the cursor");
}

#[gpui::test]
fn a_steps_on_too_once_the_song_is_in_the_target(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx);
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down a a");
    vcx.run_until_parked();
    assert_eq!(
        added(&page),
        [
            (TARGET.to_owned(), FOUND[0].to_owned()),
            (TARGET.to_owned(), FOUND[1].to_owned())
        ]
    );
    assert_eq!(selected_in_search(&page, vcx), Some(2));
}

#[gpui::test]
fn a_with_no_target_opens_the_picker_and_the_cursor_stays(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down a");
    vcx.run_until_parked();
    assert!(page.shell.read_with(vcx, |s, cx| s.picker().read(cx).is_open()));
    assert_eq!(selected_in_search(&page, vcx), Some(0), "the picker is for this row");
}

#[gpui::test]
fn the_setting_switches_it_off(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    advance_off(&page, vcx);
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down c");
    assert_eq!(crate_ids(&page, vcx), [FOUND[0]]);
    assert_eq!(selected_in_search(&page, vcx), Some(0), "the cursor stays on the row");
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [FOUND[0]], "C on the same row again adds nothing new");
}

#[gpui::test]
fn the_setting_is_a_row_in_settings_playlists_and_on_by_default(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(page.settings.read_with(vcx, |m, _| m.get(&AUTO_ADVANCE)), "on by default");
    route(&page, vcx, Route::Settings);
    assert!(on_screen(vcx, "setting-auto-advance"));
}

#[gpui::test]
fn c_and_a_step_down_a_library_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let view = page.shell.read_with(vcx, |s, _| s.library().clone());
    let model = view.read_with(vcx, |v, _| v.model().clone());
    let liked = model.read_with(vcx, |m, _| {
        m.list(apollo_app::library::Tab::Playlists)
            .iter()
            .find(|c| c.is_liked_music())
            .cloned()
            .expect("Liked Music is listed")
    });
    view.update_in(vcx, |v, window, cx| v.open(liked, window, cx));
    vcx.run_until_parked();
    let rows: Vec<String> = model.read_with(vcx, |m, _| {
        m.list_rows()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    });
    assert!(rows.len() >= 3, "the list has songs to walk");
    focus_panel(vcx, &view);
    vcx.simulate_keystrokes("c");
    assert_eq!(view.read_with(vcx, |v, _| v.selected()), 1);
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(view.read_with(vcx, |v, _| v.selected()), 2);
    assert_eq!(crate_ids(&page, vcx), [rows[0].clone()]);
    assert_eq!(added(&page), [(TARGET.to_owned(), rows[1].clone())]);
}

#[gpui::test]
fn c_steps_down_the_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    focus_panel(vcx, &panel);
    // The first press shows the cursor on the playing row; one more reaches the first up next.
    vcx.simulate_keystrokes("down down c c");
    assert_eq!(crate_ids(&page, vcx), [QUEUED[0], QUEUED[1]]);
}

#[gpui::test]
fn a_steps_down_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx);
    fill(&page, vcx, &KEPT);
    route(&page, vcx, Route::Crate);
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    focus_panel(vcx, &crate_page);
    vcx.simulate_keystrokes("down a a");
    vcx.run_until_parked();
    assert_eq!(
        added(&page),
        [
            (TARGET.to_owned(), KEPT[0].to_owned()),
            (TARGET.to_owned(), KEPT[1].to_owned())
        ]
    );
    assert_eq!(crate_page.read_with(vcx, |p, _| p.cursor().map(str::to_owned)), Some(KEPT[2].to_owned()));
}

// ---- 11: running length ------------------------------------------------------------------------

fn summary(page: &Page, vcx: &mut VisualTestContext, dock: bool) -> String {
    let panel = page.shell.read_with(vcx, |s, _| {
        if dock {
            s.crate_dock_panel().clone()
        } else {
            s.crate_page().clone()
        }
    });
    panel.read_with(vcx, |p, cx| p.summary(cx).to_string())
}

#[gpui::test]
fn the_header_says_how_long_the_crate_runs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill_timed(&page, vcx, "timedAAAA01", Some(10));
    fill_timed(&page, vcx, "timedBBBB02", Some(20));
    fill_timed(&page, vcx, "timedCCCC03", None);
    assert_eq!(summary(&page, vcx, false), "3 songs \u{b7} 30:00 + 1 unknown");
    assert_eq!(summary(&page, vcx, true), "3 songs \u{b7} 30:00 + 1 unknown", "the dock's too");
    for i in 0..40 {
        fill_timed(&page, vcx, &format!("timedFill{i:03}"), Some(4));
    }
    // 30 + 40 * 4 = 190 minutes.
    assert_eq!(summary(&page, vcx, false), "43 songs \u{b7} 3 h 10 min + 1 unknown");
}

#[gpui::test]
fn picked_songs_have_their_own_count_and_length(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill_timed(&page, vcx, "timedAAAA01", Some(10));
    fill_timed(&page, vcx, "timedBBBB02", Some(20));
    fill_timed(&page, vcx, "timedCCCC03", Some(5));
    route(&page, vcx, Route::Crate);
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    focus_panel(vcx, &crate_page);
    vcx.simulate_keystrokes("down shift-down");
    vcx.run_until_parked();
    assert_eq!(summary(&page, vcx, false), "2 picked \u{b7} 30:00");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_eq!(summary(&page, vcx, false), "3 songs \u{b7} 35:00");
}

#[gpui::test]
fn the_page_and_the_dock_show_the_line(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill_timed(&page, vcx, "timedAAAA01", Some(10));
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "dock-count"));
    route(&page, vcx, Route::Crate);
    assert!(on_screen(vcx, "crate-count"));
}

// ---- 15: jump into the dock --------------------------------------------------------------------

fn focused_dock(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.shell
        .update_in(vcx, |s, window, cx| s.crate_dock_focused(window, cx))
}

fn dock_cursor(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    let dock = page.shell.read_with(vcx, |s, _| s.crate_dock_panel().clone());
    dock.read_with(vcx, |p, _| p.cursor().map(str::to_owned))
}

fn focused(vcx: &mut VisualTestContext) -> Option<gpui::FocusHandle> {
    vcx.update(|window, cx| window.focused(cx))
}

fn dock_open(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.shell.read_with(vcx, |s, _| s.crate_dock_open())
}

/// The Library page's list has the keyboard.
fn on_a_list(page: &Page, vcx: &mut VisualTestContext) -> gpui::FocusHandle {
    route(page, vcx, Route::Library);
    let view = page.shell.read_with(vcx, |s, _| s.library().clone());
    focus_panel(vcx, &view);
    focused(vcx).expect("the list has focus")
}

#[gpui::test]
fn ctrl_shift_c_opens_the_dock_on_its_first_row_and_esc_goes_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    let before = on_a_list(&page, vcx);
    assert!(!dock_open(&page, vcx));

    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(dock_open(&page, vcx));
    assert!(focused_dock(&page, vcx), "the keyboard is in the dock");
    assert_eq!(dock_cursor(&page, vcx).as_deref(), Some(KEPT[0]), "on the first row");

    // The arrows now move the dock's cursor.
    vcx.simulate_keystrokes("down");
    assert_eq!(dock_cursor(&page, vcx).as_deref(), Some(KEPT[1]));

    // Esc: back to where it was; the dock stays open.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!focused_dock(&page, vcx));
    assert!(dock_open(&page, vcx));
    assert!(focused(vcx) == Some(before.clone()), "focus is back on the list it left");

    // Again: it lands on the row it was last on.
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(focused_dock(&page, vcx));
    assert_eq!(dock_cursor(&page, vcx).as_deref(), Some(KEPT[1]), "the last-focused row");

    // Open and focused: the key closes it, and focus goes back too.
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(!dock_open(&page, vcx));
    assert!(focused(vcx) == Some(before), "focus is back on the list it left");
}

#[gpui::test]
fn with_the_dock_open_but_not_focused_the_key_focuses_it_instead_of_closing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    vcx.run_until_parked();
    let _ = on_a_list(&page, vcx);
    assert!(dock_open(&page, vcx) && !focused_dock(&page, vcx));
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(dock_open(&page, vcx), "still open");
    assert!(focused_dock(&page, vcx));
}

#[gpui::test]
fn esc_clears_picks_first_and_only_then_leaves_the_dock(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    let before = on_a_list(&page, vcx);
    vcx.simulate_keystrokes("ctrl-shift-c shift-down");
    vcx.run_until_parked();
    let dock = page.shell.read_with(vcx, |s, _| s.crate_dock_panel().clone());
    assert_eq!(dock.read_with(vcx, |p, _| p.picked()), 2);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_eq!(dock.read_with(vcx, |p, _| p.picked()), 0, "the picks went first");
    assert!(focused_dock(&page, vcx), "still in the dock");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!focused_dock(&page, vcx));
    assert!(focused(vcx) == Some(before));
}

#[gpui::test]
fn an_empty_crate_opens_the_dock_without_taking_the_keyboard(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let before = on_a_list(&page, vcx);
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(dock_open(&page, vcx), "it opens");
    assert!(!focused_dock(&page, vcx), "there is no list to land on");
    assert!(focused(vcx) == Some(before));
    // With nothing to jump into the key is a plain toggle, so the dock is never stuck open.
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(!dock_open(&page, vcx));
}

#[gpui::test]
fn on_the_crate_page_the_key_only_opens_and_closes_the_dock(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    route(&page, vcx, Route::Crate);
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    focus_panel(vcx, &crate_page);
    vcx.simulate_keystrokes("ctrl-shift-c");
    assert!(dock_open(&page, vcx));
    vcx.simulate_keystrokes("ctrl-shift-c");
    assert!(!dock_open(&page, vcx));
}

#[gpui::test]
fn esc_after_the_queue_gave_up_its_slot_lands_on_the_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    focus_panel(vcx, &panel);
    vcx.simulate_keystrokes("ctrl-shift-c");
    vcx.run_until_parked();
    assert!(focused_dock(&page, vcx), "the Crate took the queue's slot and the keyboard");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!focused_dock(&page, vcx));
    assert!(focused(vcx).is_some(), "the keyboard is somewhere that exists");
}

// ---- 16: the count on the Crate button ---------------------------------------------------------

#[gpui::test]
fn the_crate_button_shows_how_many_songs_it_holds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(!on_screen(vcx, "button-count"), "nothing while the Crate is empty");
    let bare = bounds(vcx, "bar-crate").size.width;

    fill(&page, vcx, &KEPT[..1]);
    assert!(on_screen(vcx, "button-count"));
    let one = bounds(vcx, "bar-crate").size.width;
    assert!(one > bare, "the button grows to hold the number");
    let count = bounds(vcx, "button-count");
    let button = bounds(vcx, "bar-crate");
    assert!(
        count.left() >= button.left() + gpui::px(20.0) && count.right() <= button.right(),
        "the number sits right of the icon, inside the button"
    );

    // Taking the song out takes the number away.
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    model.update(vcx, |m, cx| m.remove(KEPT[0], cx));
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "button-count"));
    assert_eq!(bounds(vcx, "bar-crate").size.width, bare);
}

#[gpui::test]
fn even_99_plus_fits_the_bar_at_the_minimum_window(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_resize(gpui::size(gpui::px(944.0), gpui::px(720.0)));
    vcx.run_until_parked();
    let ids: Vec<String> = (0..120).map(|i| format!("manySongs{i:03}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    fill(&page, vcx, &refs);
    assert!(on_screen(vcx, "button-count"));
    let right = bounds(vcx, "player-bar-right");
    let like = bounds(vcx, "bar-like");
    assert!(
        like.left() >= right.left() - gpui::px(0.5),
        "the right block's first button ({}) stays inside the block ({})",
        f32::from(like.left()),
        f32::from(right.left())
    );
    assert!(bounds(vcx, "bar-crate").right() <= right.right() + gpui::px(0.5));
}

#[gpui::test]
fn the_count_does_not_change_what_a_click_does(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &KEPT);
    let at = bounds(vcx, "bar-crate").center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    assert!(dock_open(&page, vcx));
}
