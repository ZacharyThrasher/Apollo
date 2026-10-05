//! Headless tests for the arrange tools of a playlist of your own: Alt+Up / Alt+Down, Move to top
//! and Move to bottom (for one row and for the picked ones), and Remove duplicates with its
//! preview. The real shell and views on GPUI's test platform, the simulated library as the
//! "service" (it can refuse a write or lose its reply).

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::edits::{failure_text, Edit};
use apollo_app::library::model::Tab;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::duplicates_dialog::DuplicatesDialog;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Command, TrackMenu};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{Collection, WriteFailure};
use apollo_domain::listing::LoadFailure;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext,
};

const ROAD_TRIP: &str = "Road Trip";
const ROAD_TRIP_ID: &str = "PLsimroad";

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
    view: Entity<LibraryView>,
    menu: Entity<TrackMenu>,
    dups: Entity<DuplicatesDialog>,
    sim: SimLibrary,
}

fn prepared(sim: &SimLibrary) -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: apollo_domain::model::Track::new("simTrack001", "Midnight Drive"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    }
}

/// The shell on the Library page with the library loaded.
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
    let ready = prepared(&sim);
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    let (view, menu, dups) = shell.read_with(vcx, |s, _| {
        (
            s.library().clone(),
            s.menu().clone(),
            s.duplicates_dialog().clone(),
        )
    });
    (
        Page {
            shell,
            view,
            menu,
            dups,
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

/// Enter pressed and released: what activates a focused button (the harness's keystrokes only
/// press).
fn press_enter(vcx: &mut VisualTestContext) {
    let keystroke = gpui::Keystroke::parse("enter").unwrap();
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui::KeyUpEvent { keystroke });
    vcx.run_until_parked();
}

fn right_click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
}

fn collection(page: &Page, vcx: &mut VisualTestContext, tab: Tab, title: &str) -> Collection {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .list(tab)
            .iter()
            .find(|c| c.title == title)
            .cloned()
            .unwrap_or_else(|| panic!("{title} is in the library"))
    })
}

fn menu_open(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.menu.read_with(vcx, |m, _| m.is_open())
}

fn commands(page: &Page, vcx: &mut VisualTestContext) -> Vec<Command> {
    page.menu.read_with(vcx, |m, cx| m.commands(cx))
}

fn choose(page: &Page, vcx: &mut VisualTestContext, command: Command) {
    let ix = commands(page, vcx)
        .iter()
        .position(|c| *c == command)
        .unwrap_or_else(|| panic!("{command:?} is in the menu"));
    click(vcx, &format!("track-menu-item-{ix}"));
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.writes().read(cx).notice().map(|n| n.text.to_string())
    })
}

fn log(page: &Page) -> Vec<(String, String)> {
    page.sim.write_log()
}

fn open_playlist(page: &Page, vcx: &mut VisualTestContext, title: &str) {
    let c = collection(page, vcx, Tab::Playlists, title);
    page.view.update_in(vcx, |v, window, cx| v.open(c, window, cx));
    vcx.run_until_parked();
}

/// The titles of the rows the open page shows.
fn rows(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .map(|d| {
                d.listing
                    .items()
                    .iter()
                    .map(|r| r.track.title.clone())
                    .collect()
            })
            .unwrap_or_default()
    })
}



/// The writes of one kind the service received.
fn writes(page: &Page, kind: &str) -> usize {
    log(page)
        .iter()
        .filter(|(w, _)| w == &format!("{kind}:{ROAD_TRIP_ID}"))
        .count()
}

fn ctrl_click_row(vcx: &mut VisualTestContext, ix: usize) {
    let at = bounds(vcx, &format!("library-row-{ix}")).center();
    vcx.simulate_click(at, Modifiers::control());
    vcx.run_until_parked();
}

fn picked(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.view.read_with(vcx, |v, _| v.picked())
}

fn cursor(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.view.read_with(vcx, |v, _| v.selected())
}

/// A playlist of `ids` as the service holds it, opened: the titles are "Song <id>".
fn open_with(page: &Page, vcx: &mut VisualTestContext, ids: &[&str]) -> Vec<String> {
    page.sim.preset_playlist(ROAD_TRIP_ID, ids);
    // The library page reads the playlist afresh.
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    open_playlist(page, vcx, ROAD_TRIP);
    ids.iter().map(|id| format!("Song {id}")).collect()
}

fn titles_of(all: &[String], order: &[usize]) -> Vec<String> {
    order.iter().map(|&i| all[i].clone()).collect()
}

const SIX: [&str; 6] = ["s1", "s2", "s3", "s4", "s5", "s6"];

// ---- Alt+Up / Alt+Down ----------------------------------------------------------------------

#[gpui::test]
fn alt_up_and_alt_down_move_the_cursor_row_one_place_and_the_cursor_follows(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    let tokens = page.sim.occurrences(ROAD_TRIP_ID);
    // Focus the list and put the cursor on the third row.
    ctrl_click_row(vcx, 2);
    vcx.simulate_keystrokes("escape");
    assert_eq!(cursor(&page, vcx), 2);

    vcx.simulate_keystrokes("alt-up");
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 2, 1, 3, 4, 5]), "at once");
    assert_eq!(cursor(&page, vcx), 1, "the cursor goes with the row");
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), [
        tokens[0].clone(), tokens[2].clone(), tokens[1].clone(),
        tokens[3].clone(), tokens[4].clone(), tokens[5].clone(),
    ]);
    assert_eq!(writes(&page, "move"), 1);

    vcx.simulate_keystrokes("alt-up");
    assert_eq!(cursor(&page, vcx), 0);
    // At the top the key does nothing, and nothing is sent.
    vcx.simulate_keystrokes("alt-up");
    assert_eq!(rows(&page, vcx), titles_of(&all, &[2, 0, 1, 3, 4, 5]));
    assert_eq!(writes(&page, "move"), 2);

    vcx.simulate_keystrokes("alt-down alt-down");
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 1, 2, 3, 4, 5]));
    assert_eq!(cursor(&page, vcx), 2);
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn alt_down_at_the_bottom_does_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    ctrl_click_row(vcx, 5);
    vcx.simulate_keystrokes("escape alt-down");
    assert_eq!(rows(&page, vcx), all);
    assert_eq!(writes(&page, "move"), 0);
}

#[gpui::test]
fn alt_up_and_down_move_the_picked_songs_together(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    ctrl_click_row(vcx, 1);
    ctrl_click_row(vcx, 3);
    assert_eq!(picked(&page, vcx), 2);

    vcx.simulate_keystrokes("alt-down");
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 2, 1, 4, 3, 5]));
    assert_eq!(picked(&page, vcx), 2, "the picks stay on their songs");
    assert_eq!(
        page.sim.occurrences(ROAD_TRIP_ID),
        tokens_of_orig(&page, &[0, 2, 1, 4, 3, 5]),
    );
    vcx.simulate_keystrokes("alt-up alt-up");
    // The second one finds a pick at the top and moves nothing.
    assert_eq!(rows(&page, vcx), titles_of(&all, &[1, 0, 3, 2, 4, 5]));
    assert_eq!(notice(&page, vcx), None);
}

/// The tokens the preset gave the rows, in the order they were preset (their own numbering).
fn tokens_of_orig(page: &Page, order: &[usize]) -> Vec<String> {
    let _ = page;
    order
        .iter()
        .map(|n| format!("occ-preset-{ROAD_TRIP_ID}-{n}"))
        .collect()
}

#[gpui::test]
fn a_refused_move_puts_the_services_order_back_with_the_usual_notice(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    ctrl_click_row(vcx, 2);
    vcx.simulate_keystrokes("escape");
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    vcx.simulate_keystrokes("alt-down");
    vcx.run_until_parked();
    assert_eq!(rows(&page, vcx), all, "the service's own order is back");
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::MoveSong, WriteFailure::Rejected))
    );
}

#[gpui::test]
fn another_persons_playlist_cannot_be_arranged(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, "Mix 020");
    let before = rows(&page, vcx);
    ctrl_click_row(vcx, 1);
    vcx.simulate_keystrokes("escape alt-up");
    assert_eq!(rows(&page, vcx), before);
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    let found = commands(&page, vcx);
    assert!(!found.contains(&Command::MoveToTop) && !found.contains(&Command::MoveToBottom));
    assert!(log(&page).is_empty());
}

// ---- Move to top / Move to bottom ------------------------------------------------------------

#[gpui::test]
fn move_to_top_and_bottom_are_in_the_menu_of_a_song_of_your_own_playlist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    let at = bounds(vcx, "library-row-3").center();
    right_click(vcx, at);
    let found = commands(&page, vcx);
    assert!(found.contains(&Command::MoveToTop) && found.contains(&Command::MoveToBottom));
    assert_eq!(found.last(), Some(&Command::RemoveFromPlaylist), "remove stays last");
    choose(&page, vcx, Command::MoveToTop);
    assert!(!menu_open(&page, vcx));
    assert_eq!(rows(&page, vcx), titles_of(&all, &[3, 0, 1, 2, 4, 5]));
    assert_eq!(writes(&page, "move"), 1, "one write");
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), tokens_of_orig(&page, &[3, 0, 1, 2, 4, 5]));

    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::MoveToBottom);
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 1, 2, 4, 5, 3]));
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), tokens_of_orig(&page, &[0, 1, 2, 4, 5, 3]));
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn the_menu_of_picked_songs_sends_them_all_to_an_end_in_their_own_order(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    ctrl_click_row(vcx, 4);
    ctrl_click_row(vcx, 1);
    let at = bounds(vcx, "library-row-4").center();
    right_click(vcx, at);
    let found = commands(&page, vcx);
    assert!(found.contains(&Command::MoveToTop) && found.contains(&Command::MoveToBottom));
    choose(&page, vcx, Command::MoveToTop);
    assert_eq!(rows(&page, vcx), titles_of(&all, &[1, 4, 0, 2, 3, 5]));
    assert_eq!(writes(&page, "move"), 2, "one write per moved song at most");
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), tokens_of_orig(&page, &[1, 4, 0, 2, 3, 5]));
    assert_eq!(picked(&page, vcx), 2);

    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::MoveToBottom);
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 2, 3, 5, 1, 4]));
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), tokens_of_orig(&page, &[0, 2, 3, 5, 1, 4]));
}

#[gpui::test]
fn a_move_that_changes_nothing_sends_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::MoveToTop);
    assert_eq!(rows(&page, vcx), all);
    assert_eq!(writes(&page, "move"), 0);
}

#[gpui::test]
fn a_refused_move_to_the_top_shows_the_services_order(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let at = bounds(vcx, "library-row-4").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::MoveToTop);
    vcx.run_until_parked();
    assert_eq!(rows(&page, vcx), all);
    assert!(notice(&page, vcx).is_some());
}

// ---- Remove duplicates ---------------------------------------------------------------------

/// The header's More menu, opened as the Edit item's tests open it.
fn open_header_menu(page: &Page, vcx: &mut VisualTestContext) {
    let more = bounds(vcx, "detail-more").center();
    right_click(vcx, point(more.x - gpui::px(300.0), more.y));
    assert!(menu_open(page, vcx));
}

fn dup_rows(page: &Page, vcx: &mut VisualTestContext) -> Vec<(String, usize, usize)> {
    page.dups.read_with(vcx, |d, _| {
        d.rows()
            .into_iter()
            .map(|r| (r.video_id, r.number, r.kept))
            .collect()
    })
}

#[gpui::test]
fn remove_duplicates_previews_the_repeats_and_removes_exactly_those_rows(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &["x", "y", "x", "z", "y", "x"]);
    open_header_menu(&page, vcx);
    assert!(commands(&page, vcx).contains(&Command::RemoveDuplicates));
    choose(&page, vcx, Command::RemoveDuplicates);
    assert!(page.dups.read_with(vcx, |d, _| d.is_open()));
    assert_eq!(
        dup_rows(&page, vcx),
        [("x".to_owned(), 3, 1), ("y".to_owned(), 5, 2), ("x".to_owned(), 6, 1)],
        "the first copy stays; each repeat names its place and the copy that stays"
    );
    assert!(on_screen(vcx, "duplicate-row-2"));
    assert_eq!(writes(&page, "remove"), 0, "nothing goes before the confirm");
    assert_eq!(rows(&page, vcx), all);

    click(vcx, "duplicates-remove");
    assert!(!page.dups.read_with(vcx, |d, _| d.is_open()));
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 1, 3]));
    // The copies listed went, by their own tokens: the first of each stays.
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), tokens_of_orig(&page, &[0, 1, 3]));
    assert_eq!(writes(&page, "remove"), 3, "one write per row");
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn cancel_and_escape_keep_every_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &["x", "y", "x"]);
    open_header_menu(&page, vcx);
    choose(&page, vcx, Command::RemoveDuplicates);
    click(vcx, "duplicates-cancel");
    assert!(!page.dups.read_with(vcx, |d, _| d.is_open()));
    assert_eq!(rows(&page, vcx), all);

    open_header_menu(&page, vcx);
    choose(&page, vcx, Command::RemoveDuplicates);
    vcx.simulate_keystrokes("escape");
    assert!(!page.dups.read_with(vcx, |d, _| d.is_open()));
    assert_eq!(rows(&page, vcx), all);
    assert_eq!(writes(&page, "remove"), 0);
}

#[gpui::test]
fn a_stray_enter_keeps_the_songs_because_cancel_has_the_focus(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &["x", "y", "x"]);
    open_header_menu(&page, vcx);
    choose(&page, vcx, Command::RemoveDuplicates);
    press_enter(vcx);
    assert!(!page.dups.read_with(vcx, |d, _| d.is_open()));
    assert_eq!(rows(&page, vcx), all);
    assert_eq!(writes(&page, "remove"), 0);
}

#[gpui::test]
fn tab_reaches_the_remove_button_and_enter_presses_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &["x", "y", "x"]);
    open_header_menu(&page, vcx);
    choose(&page, vcx, Command::RemoveDuplicates);
    vcx.simulate_keystrokes("tab");
    press_enter(vcx);
    assert_eq!(rows(&page, vcx), titles_of(&all, &[0, 1]));
}

#[gpui::test]
fn a_playlist_without_repeats_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &SIX);
    open_header_menu(&page, vcx);
    choose(&page, vcx, Command::RemoveDuplicates);
    assert!(page.dups.read_with(vcx, |d, _| d.is_open()));
    assert!(dup_rows(&page, vcx).is_empty());
    assert!(!on_screen(vcx, "duplicates-remove"));
    click(vcx, "duplicates-done");
    assert!(!page.dups.read_with(vcx, |d, _| d.is_open()));
    assert_eq!(rows(&page, vcx), all);
}

#[gpui::test]
fn refused_removals_put_the_playlist_back_with_the_usual_notice(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let all = open_with(&page, vcx, &["x", "y", "x", "y"]);
    open_header_menu(&page, vcx);
    choose(&page, vcx, Command::RemoveDuplicates);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    click(vcx, "duplicates-remove");
    vcx.run_until_parked();
    assert_eq!(rows(&page, vcx), all, "the service's own list is back");
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::RemoveSong, WriteFailure::Rejected))
    );
}

#[gpui::test]
fn another_persons_playlist_has_no_remove_duplicates(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, "Mix 020");
    open_header_menu(&page, vcx);
    assert!(!commands(&page, vcx).contains(&Command::RemoveDuplicates));
}
