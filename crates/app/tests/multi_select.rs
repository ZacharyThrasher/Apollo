//! Headless tests for multi-select: Ctrl+click, Shift+click, Shift+Arrows, Ctrl+A and Esc pick
//! songs on the Search results and on a playlist page, and with two or more picked the row menu
//! (and a right-click on a picked row) acts on all of them. The real shell and views on GPUI's
//! test platform, the simulated library as the "service".

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::Tab;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::search_page::SearchPage;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Command, Subject, TrackMenu};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::WriteFailure;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext,
};

const FIRST: &str = "simTrack001";
/// Four search results, in this order.
const FOUND: [&str; 4] = ["foundTrk001", "foundTrk002", "foundTrk003", "foundTrk004"];
const ROAD_TRIP: &str = "Road Trip";
const ROAD_TRIP_ID: &str = "PLsimroad";

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

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    menu: Entity<TrackMenu>,
    search: Entity<SearchPage>,
    view: Entity<LibraryView>,
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
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let (menu, search, view) = shell.read_with(vcx, |s, _| {
        (s.menu().clone(), s.search_page().clone(), s.library().clone())
    });
    (
        Page {
            player,
            shell,
            menu,
            search,
            view,
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

fn right_click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
}

/// Let the clock run in 50 ms steps (a paced bulk write waits on timers).
fn advance(vcx: &mut VisualTestContext, ms: u64) {
    for _ in 0..(ms / 50) {
        vcx.executor().advance_clock(Duration::from_millis(50));
        vcx.run_until_parked();
    }
}

// ---- Search ---------------------------------------------------------------------------------

fn search(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Search, window, cx);
    });
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

fn pick(vcx: &mut VisualTestContext, at: Point<Pixels>, modifiers: Modifiers) {
    vcx.simulate_click(at, modifiers);
    vcx.run_until_parked();
}

fn ctrl_click_song(vcx: &mut VisualTestContext, ix: usize) {
    let at = song_at(vcx, ix);
    pick(vcx, at, Modifiers::control());
}

fn picked_songs(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.search.read_with(vcx, |p, _| p.picked())
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

fn upcoming(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.player.read_with(vcx, |m, _| {
        m.queue_view()
            .upcoming
            .iter()
            .map(|r| r.video_id.clone())
            .collect()
    })
}

fn menu_subject(page: &Page, vcx: &mut VisualTestContext) -> Option<Subject> {
    page.menu.read_with(vcx, |m, _| m.subject().cloned())
}

#[gpui::test]
fn ctrl_and_shift_click_pick_songs_and_the_count_shows_from_two(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    assert_eq!(picked_songs(&page, vcx), 0);
    ctrl_click_song(vcx, 0);
    assert_eq!(picked_songs(&page, vcx), 1);
    assert!(
        !on_screen(vcx, "search-selected-count"),
        "one picked song is a plain row"
    );
    ctrl_click_song(vcx, 2);
    assert_eq!(picked_songs(&page, vcx), 2);
    assert!(on_screen(vcx, "search-selected-count"));
    // Ctrl+click again takes a song out.
    ctrl_click_song(vcx, 2);
    assert_eq!(picked_songs(&page, vcx), 1);
    // Shift+click stretches from the last Ctrl+clicked song, replacing what Shift did before.
    let at = song_at(vcx, 3);
    pick(vcx, at, Modifiers::shift());
    assert_eq!(picked_songs(&page, vcx), 2, "rows 2 and 3");
    let at = song_at(vcx, 0);
    pick(vcx, at, Modifiers::shift());
    assert_eq!(picked_songs(&page, vcx), 3, "rows 0 to 2");
}

#[gpui::test]
fn a_plain_click_plays_and_clears_the_picks(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 1);
    assert_eq!(picked_songs(&page, vcx), 2);
    let at = song_at(vcx, 3);
    pick(vcx, at, Modifiers::none());
    assert_eq!(picked_songs(&page, vcx), 0);
    let playing = page.player.read_with(vcx, |m, _| m.view().title.to_string());
    assert_eq!(playing, format!("Song {}", FOUND[3]), "the song's own click still plays it");
}

#[gpui::test]
fn a_ctrl_click_does_not_play(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    let before = page.player.read_with(vcx, |m, _| m.view().title.to_string());
    ctrl_click_song(vcx, 1);
    let after = page.player.read_with(vcx, |m, _| m.view().title.to_string());
    assert_eq!(before, after);
}

#[gpui::test]
fn right_click_on_a_picked_row_opens_the_menu_of_the_selection(cx: &mut TestAppContext) {
    use Command::*;
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 2);
    let at = song_at(vcx, 2);
    right_click(vcx, at);
    assert_eq!(
        commands(&page, vcx),
        [PlayNext, AddToQueue, AddToPlaylist, AddToCrate, ToggleLike],
        "no Remove from playlist on Search"
    );
    match menu_subject(&page, vcx) {
        Some(Subject::Tracks(songs)) => {
            let ids: Vec<_> = songs.iter().map(|s| s.track.video_id.as_str().to_owned()).collect();
            assert_eq!(ids, [FOUND[0], FOUND[2]], "in the list's order");
        }
        other => panic!("expected the selection's menu, got {other:?}"),
    }
    vcx.simulate_keystrokes("escape");
    // A row that is not picked keeps its own menu, whatever is picked.
    let at = song_at(vcx, 1);
    right_click(vcx, at);
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Track(t)) if t.track.video_id.as_str() == FOUND[1]));
    assert!(commands(&page, vcx).contains(&StartRadio));
}

#[gpui::test]
fn one_picked_song_has_its_ordinary_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 1);
    let at = song_at(vcx, 1);
    right_click(vcx, at);
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Track(_))));
}

#[gpui::test]
fn the_more_button_of_a_picked_row_opens_the_selection_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 1);
    let slot = bounds(vcx, "song-more-1");
    vcx.simulate_mouse_move(slot.center(), None, Modifiers::none());
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
    click(vcx, "song-more-1");
    assert!(matches!(menu_subject(&page, vcx), Some(Subject::Tracks(s)) if s.len() == 2));
}

#[gpui::test]
fn add_to_queue_and_play_next_take_the_picked_songs_in_list_order(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    // Picked bottom to top: the order of the list wins, not the order of picking.
    ctrl_click_song(vcx, 3);
    ctrl_click_song(vcx, 1);
    let at = song_at(vcx, 1);
    right_click(vcx, at);
    choose(&page, vcx, Command::AddToQueue);
    assert_eq!(upcoming(&page, vcx), [FOUND[1], FOUND[3]]);
    assert_eq!(picked_songs(&page, vcx), 2, "the picks stay for the next verb");

    let at = song_at(vcx, 3);
    right_click(vcx, at);
    choose(&page, vcx, Command::PlayNext);
    assert_eq!(upcoming(&page, vcx), [FOUND[1], FOUND[3], FOUND[1], FOUND[3]]);
}

#[gpui::test]
fn add_to_crate_puts_every_picked_song_in_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 3);
    let at = song_at(vcx, 0);
    right_click(vcx, at);
    choose(&page, vcx, Command::AddToCrate);
    let held: Vec<bool> = {
        let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
        FOUND
            .iter()
            .map(|id| model.read_with(vcx, |m, _| m.contains(id)))
            .collect()
    };
    assert_eq!(held, [true, false, false, true]);
}

#[gpui::test]
fn like_goes_out_one_song_at_a_time_and_every_picked_song_ends_up_liked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    for ix in 0..3 {
        ctrl_click_song(vcx, ix);
    }
    let at = song_at(vcx, 0);
    right_click(vcx, at);
    choose(&page, vcx, Command::ToggleLike);
    // The first like is out at once; the next waits for it and for the pause.
    let likes = |page: &Page| -> Vec<String> {
        page.sim
            .write_log()
            .into_iter()
            .filter(|(w, _)| w == "like")
            .map(|(_, id)| id)
            .collect()
    };
    assert_eq!(likes(&page), [FOUND[0]]);
    advance(vcx, 100);
    assert_eq!(likes(&page), [FOUND[0]], "still inside the pause");
    advance(vcx, 1500);
    assert_eq!(likes(&page), [FOUND[0], FOUND[1], FOUND[2]], "in the list's order, none twice");
    let liked = page.sim.liked_ids();
    for id in &FOUND[..3] {
        assert!(liked.contains(&(*id).to_owned()));
    }
    assert!(!liked.contains(&FOUND[3].to_owned()));

    // All liked now: the same item takes them all out again.
    let at = song_at(vcx, 0);
    right_click(vcx, at);
    assert!(commands(&page, vcx).contains(&Command::ToggleLike));
    choose(&page, vcx, Command::ToggleLike);
    advance(vcx, 1500);
    let liked = page.sim.liked_ids();
    for id in &FOUND[..3] {
        assert!(!liked.contains(&(*id).to_owned()));
    }
}

#[gpui::test]
fn a_refused_like_in_a_bulk_run_rolls_that_heart_back_and_the_run_goes_on(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 1);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let at = song_at(vcx, 0);
    right_click(vcx, at);
    choose(&page, vcx, Command::ToggleLike);
    advance(vcx, 1000);
    assert!(page.sim.liked_ids().iter().all(|id| !FOUND.contains(&id.as_str())));
    let writes = page.shell.read_with(vcx, |s, _| s.writes().clone());
    for id in &FOUND[..2] {
        assert!(!writes.read_with(vcx, |w, _| w.is_liked(id)), "{id} rolled back");
    }
    let notice = writes.read_with(vcx, |w, _| w.notice().map(|n| n.text.to_string()));
    assert!(notice.is_some(), "a refusal leaves the usual toast");
    let attempts = page.sim.write_log().iter().filter(|(w, _)| w == "like").count();
    assert_eq!(attempts, 2, "the second song was still tried");
}

#[gpui::test]
fn the_keyboard_picks_with_shift_arrows_ctrl_a_and_esc(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    // A Ctrl+click moves the keyboard into the list, on that row.
    ctrl_click_song(vcx, 1);
    assert_eq!(page.search.read_with(vcx, |p, _| p.selected()), Some(1));
    vcx.simulate_keystrokes("shift-down");
    assert_eq!(picked_songs(&page, vcx), 2);
    vcx.simulate_keystrokes("shift-down");
    assert_eq!(picked_songs(&page, vcx), 3);
    vcx.simulate_keystrokes("shift-up");
    assert_eq!(picked_songs(&page, vcx), 2, "going back shrinks it");
    vcx.simulate_keystrokes("ctrl-a");
    assert_eq!(picked_songs(&page, vcx), 4);
    // Esc clears the picks first; the list keeps the keyboard.
    vcx.simulate_keystrokes("escape");
    assert_eq!(picked_songs(&page, vcx), 0);
    assert!(page.search.read_with(vcx, |p, _| p.selected()).is_some());
    // A second Esc leaves the list as it did before.
    vcx.simulate_keystrokes("escape");
    assert_eq!(page.search.read_with(vcx, |p, _| p.selected()), None);
}

#[gpui::test]
fn plain_arrows_drop_the_picks_and_a_new_search_starts_clean(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 1);
    vcx.simulate_keystrokes("down");
    assert_eq!(picked_songs(&page, vcx), 0);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 1);
    assert_eq!(picked_songs(&page, vcx), 2);
    let model = page.shell.read_with(vcx, |s, _| s.search_model().clone());
    model.update(vcx, |m, cx| {
        m.set_query("something else", cx);
        m.submit(cx);
    });
    vcx.run_until_parked();
    assert_eq!(picked_songs(&page, vcx), 0);
}

#[gpui::test]
fn add_to_playlist_adds_every_picked_song_in_one_write(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 3);
    ctrl_click_song(vcx, 1);
    let at = song_at(vcx, 1);
    right_click(vcx, at);
    choose(&page, vcx, Command::AddToPlaylist);
    vcx.run_until_parked();
    let picker = page.shell.read_with(vcx, |s, _| s.picker().clone());
    assert!(picker.read_with(vcx, |p, _| p.is_open()));
    vcx.update(|window, cx| picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();
    let added: Vec<String> = page.sim.added().into_iter().map(|(_, v)| v).collect();
    assert_eq!(added, [FOUND[1], FOUND[3]], "in the list's order");
    let writes: Vec<_> = page.sim.write_log().into_iter().filter(|(w, _)| w.starts_with("add")).collect();
    assert_eq!(writes.len(), 1, "one request for the lot");
}

#[gpui::test]
fn the_p_key_opens_the_picker_for_the_picked_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 2);
    vcx.simulate_keystrokes("p");
    vcx.run_until_parked();
    let picker = page.shell.read_with(vcx, |s, _| s.picker().clone());
    assert!(picker.read_with(vcx, |p, _| p.is_open()));
    assert_eq!(picker.read_with(vcx, |p, _| p.video().map(str::to_owned)), Some(FOUND[0].to_owned()));
}

#[gpui::test]
fn the_like_and_crate_keys_act_on_the_picked_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    ctrl_click_song(vcx, 0);
    ctrl_click_song(vcx, 2);
    vcx.simulate_keystrokes("c");
    let crate_model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    assert!(crate_model.read_with(vcx, |m, _| m.contains(FOUND[0]) && m.contains(FOUND[2])));
    assert!(!crate_model.read_with(vcx, |m, _| m.contains(FOUND[1])));
    vcx.simulate_keystrokes("l");
    advance(vcx, 1000);
    let liked = page.sim.liked_ids();
    assert!(liked.contains(&FOUND[0].to_owned()) && liked.contains(&FOUND[2].to_owned()));
    assert!(!liked.contains(&FOUND[1].to_owned()));
}

// ---- playlist pages -------------------------------------------------------------------------

fn open_playlist(page: &Page, vcx: &mut VisualTestContext, title: &str) {
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    let c = page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .list(Tab::Playlists)
            .iter()
            .find(|c| c.title == title)
            .cloned()
            .unwrap_or_else(|| panic!("{title} is in the library"))
    });
    page.view.update_in(vcx, |v, window, cx| v.open(c, window, cx));
    vcx.run_until_parked();
}

fn rows(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .map(|d| d.listing.items().iter().map(|r| r.track.title.clone()).collect())
            .unwrap_or_default()
    })
}

fn picked_rows(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.view.read_with(vcx, |v, _| v.picked())
}

fn ctrl_click_row(vcx: &mut VisualTestContext, ix: usize) {
    let at = bounds(vcx, &format!("library-row-{ix}")).center();
    pick(vcx, at, Modifiers::control());
}

fn depth(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.view.read_with(vcx, |v, cx| v.model().read(cx).depth())
}

#[gpui::test]
fn picking_on_a_playlist_page_and_removing_the_picked_songs(cx: &mut TestAppContext) {
    use Command::*;
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    assert_eq!(before.len(), 3);
    let tokens = page.sim.occurrences(ROAD_TRIP_ID);

    ctrl_click_row(vcx, 0);
    assert_eq!(picked_rows(&page, vcx), 1);
    ctrl_click_row(vcx, 2);
    assert_eq!(picked_rows(&page, vcx), 2);
    assert!(on_screen(vcx, "library-selected-count"));

    let at = bounds(vcx, "library-row-2").center();
    right_click(vcx, at);
    assert_eq!(
        commands(&page, vcx),
        [PlayNext, AddToQueue, AddToPlaylist, AddToCrate, ToggleLike, RemoveFromPlaylist]
    );
    choose(&page, vcx, RemoveFromPlaylist);
    vcx.run_until_parked();
    assert_eq!(rows(&page, vcx), [before[1].clone()], "both rows are gone");
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID), [tokens[1].clone()]);
    let removes: Vec<_> = page
        .sim
        .write_log()
        .into_iter()
        .filter(|(w, _)| w.starts_with("remove:"))
        .collect();
    assert_eq!(removes.len(), 2, "one write per row, never batched into a burst");
    // The page redraws without the removed rows; nothing is picked any more.
    assert_eq!(picked_rows(&page, vcx), 0);
}

#[gpui::test]
fn a_refused_bulk_remove_puts_the_rows_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    ctrl_click_row(vcx, 0);
    ctrl_click_row(vcx, 1);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::RemoveFromPlaylist);
    vcx.run_until_parked();
    assert_eq!(rows(&page, vcx), before, "the service's own list is back");
    let writes = page.shell.read_with(vcx, |s, _| s.writes().clone());
    assert!(writes.read_with(vcx, |w, _| w.notice().is_some()));
}

#[gpui::test]
fn another_persons_playlist_has_no_remove_in_the_selection_menu(cx: &mut TestAppContext) {
    use Command::*;
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, "Mix 020");
    ctrl_click_row(vcx, 0);
    ctrl_click_row(vcx, 1);
    let at = bounds(vcx, "library-row-1").center();
    right_click(vcx, at);
    assert_eq!(
        commands(&page, vcx),
        [PlayNext, AddToQueue, AddToPlaylist, AddToCrate, ToggleLike]
    );
}

#[gpui::test]
fn shift_click_and_the_keyboard_stretch_the_picks_on_a_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    ctrl_click_row(vcx, 0);
    let at = bounds(vcx, "library-row-2").center();
    pick(vcx, at, Modifiers::shift());
    assert_eq!(picked_rows(&page, vcx), 3);
    vcx.simulate_keystrokes("shift-up");
    assert_eq!(picked_rows(&page, vcx), 2, "the cursor was on row 2: back to rows 0 and 1");
    vcx.simulate_keystrokes("ctrl-a");
    assert_eq!(picked_rows(&page, vcx), 3);
    vcx.simulate_keystrokes("down");
    assert_eq!(picked_rows(&page, vcx), 0);
}

#[gpui::test]
fn escape_clears_the_picks_before_it_closes_the_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    ctrl_click_row(vcx, 0);
    ctrl_click_row(vcx, 1);
    vcx.simulate_keystrokes("escape");
    assert_eq!(picked_rows(&page, vcx), 0);
    assert_eq!(depth(&page, vcx), 1, "the page is still open");
    vcx.simulate_keystrokes("escape");
    assert_eq!(depth(&page, vcx), 0);
}

#[gpui::test]
fn the_crate_key_on_a_page_takes_every_picked_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let ids: Vec<String> = page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .map(|d| {
                d.listing
                    .items()
                    .iter()
                    .map(|r| r.track.video_id.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    });
    ctrl_click_row(vcx, 0);
    ctrl_click_row(vcx, 2);
    vcx.simulate_keystrokes("c");
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    let held: Vec<bool> = ids
        .iter()
        .map(|id| model.read_with(vcx, |m, _| m.contains(id)))
        .collect();
    assert_eq!(held, [true, false, true]);
}

#[gpui::test]
fn picks_do_not_follow_to_another_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    ctrl_click_row(vcx, 0);
    ctrl_click_row(vcx, 1);
    assert_eq!(picked_rows(&page, vcx), 2);
    page.view.update_in(vcx, |v, window, cx| v.back(window, cx));
    vcx.run_until_parked();
    open_playlist(&page, vcx, "Mix 020");
    assert_eq!(picked_rows(&page, vcx), 0);
}
