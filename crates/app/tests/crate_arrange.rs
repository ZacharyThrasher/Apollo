//! The Crate's filter box and Arrange menu: sort by title, artist, album, date added or length,
//! shuffle, reverse, spread artists, and Alt+Home / Alt+End (or the row menu) to send songs to
//! an end. All local; each rewrite is one step for Ctrl+Z. The real shell and views on GPUI's
//! test platform.

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_arrange::{Arrange, SortKey};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::crate_page::CratePage;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Command, TrackMenu};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{AlbumRef, ArtistRef, Track};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, TestAppContext, VisualTestContext,
};

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
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
    player: Entity<PlayerModel>,
    crate_model: Entity<CrateModel>,
    crate_page: Entity<CratePage>,
    menu: Entity<TrackMenu>,
}

/// id, title, artist, album, seconds, added at.
type Spec = (&'static str, &'static str, &'static str, &'static str, u64, i64);

const SONGS: [Spec; 5] = [
    ("sngAAAAAA01", "Mango", "Zed", "Fruit", 300, 50),
    ("sngBBBBBB02", "apple", "Amy", "Zoo", 120, 10),
    ("sngCCCCCC03", "Cherry", "Zed", "Fruit", 200, 40),
    ("sngDDDDDD04", "Banana", "Amy", "Barn", 240, 30),
    ("sngEEEEEE05", "Date", "Bo", "", 60, 20),
];

fn track(spec: &Spec) -> Track {
    let (id, title, artist, album, seconds, _) = *spec;
    let mut track = Track::new(id, title);
    track.artists = vec![ArtistRef { name: artist.into(), id: None }];
    if !album.is_empty() {
        track.album = Some(AlbumRef { title: album.into(), id: None });
    }
    track.duration = Some(Duration::from_secs(seconds));
    track
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    let sim = SimLibrary::instant();
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
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
        queue: ["queueTrack1", "queueTrack2"]
            .iter()
            .map(|id| LibraryTrack { track: Track::new(*id, format!("Queued {id}")) })
            .collect(),
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let (crate_model, crate_page, menu) = shell.read_with(vcx, |s, _| {
        (s.crate_model().clone(), s.crate_page().clone(), s.menu().clone())
    });
    (Page { shell, player, crate_model, crate_page, menu }, vcx)
}

/// The five songs in the Crate, in the order of `SONGS`, and the Crate page open.
fn fill(page: &Page, vcx: &mut VisualTestContext) {
    page.crate_model.update(vcx, |m, cx| {
        for spec in &SONGS {
            m.add(track(spec), None, cx);
        }
        // Songs are stamped with the clock as they go in; give them the times the table says.
        let mut rows = m.items().to_vec();
        for (row, spec) in rows.iter_mut().zip(&SONGS) {
            row.added_ms = spec.5;
        }
        m.replace_all(rows, "dated the songs", cx);
    });
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
}

fn titles(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items().iter().map(|r| r.track.title.clone()).collect()
    })
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

fn undo(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.undo(cx));
    vcx.run_until_parked();
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

fn set_filter(page: &Page, vcx: &mut VisualTestContext, text: &str) {
    let field = page
        .crate_page
        .read_with(vcx, |p, _| p.filter_field().cloned())
        .expect("the Crate has a filter box once it has songs");
    field.update(vcx, |f, cx| f.set_text(text, cx));
    vcx.run_until_parked();
}

fn rows_on_screen(vcx: &mut VisualTestContext) -> usize {
    (0..SONGS.len())
        .filter(|ix| on_screen(vcx, &format!("crate-row-{ix}")))
        .count()
}

fn up_next(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.player.read_with(vcx, |m, _| {
        m.queue_view().upcoming.iter().map(|r| r.video_id.clone()).collect()
    })
}

// ---- the Arrange menu ----------------------------------------------------------------------

#[gpui::test]
fn the_arrange_menu_sorts_the_crate_in_one_undoable_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    assert!(on_screen(vcx, "crate-filter") && on_screen(vcx, "crate-arrange"));
    assert!(!on_screen(vcx, "crate-arrange-menu"), "closed to begin with");

    click(vcx, "crate-arrange");
    assert!(on_screen(vcx, "crate-arrange-menu"));
    // Five sorts, then Shuffle, Reverse, Spread artists.
    for ix in 0..8 {
        assert!(on_screen(vcx, &format!("crate-arrange-item-{ix}")), "choice {ix}");
    }

    click(vcx, "crate-arrange-item-0"); // Title
    assert!(!on_screen(vcx, "crate-arrange-menu"), "choosing closes it");
    assert_eq!(titles(&page, vcx), ["apple", "Banana", "Cherry", "Date", "Mango"]);
    assert!(pill(&page, vcx).is_some_and(|p| p.starts_with("Sorted by title")));

    undo(&page, vcx);
    assert_eq!(titles(&page, vcx), ["Mango", "apple", "Cherry", "Banana", "Date"]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: sorted the Crate by title"));
}

#[gpui::test]
fn every_choice_rewrites_the_crate_and_each_is_one_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    let original = titles(&page, vcx);
    let arrange = |page: &Page, vcx: &mut VisualTestContext, how: Arrange| {
        page.crate_model.update(vcx, |m, cx| m.arrange_seeded(how, 42, cx))
    };

    assert!(arrange(&page, vcx, Arrange::Sort(SortKey::Artist)));
    assert_eq!(titles(&page, vcx), ["apple", "Banana", "Date", "Mango", "Cherry"], "Amy, Amy, Bo, Zed, Zed; ties keep order");
    assert!(arrange(&page, vcx, Arrange::Sort(SortKey::Album)));
    assert_eq!(titles(&page, vcx), ["Banana", "Mango", "Cherry", "apple", "Date"], "Barn, Fruit, Fruit, Zoo, none last");
    assert!(arrange(&page, vcx, Arrange::Sort(SortKey::DateAdded)));
    assert_eq!(titles(&page, vcx), ["apple", "Date", "Banana", "Cherry", "Mango"]);
    assert!(arrange(&page, vcx, Arrange::Sort(SortKey::Length)));
    assert_eq!(titles(&page, vcx), ["Date", "apple", "Cherry", "Banana", "Mango"]);
    assert!(arrange(&page, vcx, Arrange::Reverse));
    assert_eq!(titles(&page, vcx), ["Mango", "Banana", "Cherry", "apple", "Date"]);
    assert!(arrange(&page, vcx, Arrange::Shuffle));
    let mut shuffled = titles(&page, vcx);
    shuffled.sort();
    let mut all = original.clone();
    all.sort();
    assert_eq!(shuffled, all, "a shuffle loses nothing");
    // Sorting what is sorted already changes nothing and says so.
    assert!(arrange(&page, vcx, Arrange::Sort(SortKey::Title)));
    assert!(!arrange(&page, vcx, Arrange::Sort(SortKey::Title)));
    assert_eq!(pill(&page, vcx).as_deref().map(|p| p.starts_with("Already sorted by title")), Some(true));

    // Seven steps went in; taking them all back (the "already" one made no step) lands where we began.
    for _ in 0..7 {
        undo(&page, vcx);
    }
    assert_eq!(titles(&page, vcx), original);
}

#[gpui::test]
fn spread_artists_keeps_the_same_artist_apart(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    // Zed, Amy, Zed, Amy, Bo is already spread: nothing to do.
    assert!(!page.crate_model.update(vcx, |m, cx| m.arrange(Arrange::SpreadArtists, cx)));
    // Put the pairs together, then spread them again.
    page.crate_model.update(vcx, |m, cx| m.arrange(Arrange::Sort(SortKey::Artist), cx));
    assert_eq!(titles(&page, vcx), ["apple", "Banana", "Date", "Mango", "Cherry"]);
    assert!(page.crate_model.update(vcx, |m, cx| m.arrange(Arrange::SpreadArtists, cx)));
    assert_eq!(
        titles(&page, vcx),
        ["apple", "Date", "Mango", "Banana", "Cherry"],
        "Amy, Bo, Zed, Amy, Zed: no two alike in a row"
    );
}

#[gpui::test]
fn arrange_needs_two_songs_and_waits_for_the_filter_to_be_empty(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model.update(vcx, |m, cx| {
        m.add(track(&SONGS[0]), None, cx);
    });
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    click(vcx, "crate-arrange");
    assert!(!on_screen(vcx, "crate-arrange-menu"), "one song has nothing to arrange");

    page.crate_model.update(vcx, |m, cx| {
        m.add(track(&SONGS[1]), None, cx);
    });
    vcx.run_until_parked();
    set_filter(&page, vcx, "apple");
    click(vcx, "crate-arrange");
    assert!(!on_screen(vcx, "crate-arrange-menu"), "not while the filter has text");
    set_filter(&page, vcx, "");
    click(vcx, "crate-arrange");
    assert!(on_screen(vcx, "crate-arrange-menu"));
    // Esc closes it with nothing done.
    vcx.simulate_keystrokes("escape");
    assert!(!on_screen(vcx, "crate-arrange-menu"));
    assert_eq!(titles(&page, vcx), ["Mango", "apple"]);
}

#[gpui::test]
fn the_arrange_menu_works_from_the_keyboard(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    click(vcx, "crate-arrange");
    // Title is first; Down moves to Artist, Enter chooses it.
    vcx.simulate_keystrokes("down enter");
    assert_eq!(titles(&page, vcx), ["apple", "Banana", "Date", "Mango", "Cherry"]);
    assert!(!on_screen(vcx, "crate-arrange-menu"));
}

// ---- the filter ----------------------------------------------------------------------------

#[gpui::test]
fn the_filter_keeps_songs_with_every_word_and_the_actions_follow_what_is_shown(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    assert_eq!(rows_on_screen(vcx), 5);

    set_filter(&page, vcx, "amy");
    assert_eq!(rows_on_screen(vcx), 2, "apple and Banana are by Amy");
    set_filter(&page, vcx, "fruit zed");
    assert_eq!(rows_on_screen(vcx), 2, "album and artist words both count");
    set_filter(&page, vcx, "AMY barn");
    assert_eq!(rows_on_screen(vcx), 1);
    set_filter(&page, vcx, "nothing like it");
    assert_eq!(rows_on_screen(vcx), 0);
    assert!(on_screen(vcx, "crate-no-matches"));
    click(vcx, "crate-clear-filter");
    assert_eq!(rows_on_screen(vcx), 5, "Clear filter brings everything back");

    // Add all to queue and Play take what is shown, in order.
    set_filter(&page, vcx, "amy");
    click(vcx, "crate-add-all");
    let queued = up_next(&page, vcx);
    let at = |id: &str| queued.iter().position(|q| q == id).unwrap_or_else(|| panic!("{id} is queued"));
    assert!(at("sngBBBBBB02") < at("sngDDDDDD04"), "in the order shown: {queued:?}");
    assert!(!queued.iter().any(|q| q == "sngAAAAAA01"), "Mango is hidden by the filter");
    click(vcx, "crate-play");
    assert_eq!(
        page.player.read_with(vcx, |m, _| m.current_video()).as_deref(),
        Some("sngBBBBBB02"),
        "Play starts from the first song shown"
    );
}

#[gpui::test]
fn clear_with_a_filter_takes_out_only_what_is_shown(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    set_filter(&page, vcx, "amy");
    click(vcx, "crate-clear");
    assert_eq!(titles(&page, vcx), ["Mango", "Cherry", "Date"]);
    undo(&page, vcx);
    assert_eq!(titles(&page, vcx).len(), 5, "one step brings both back");
}

#[gpui::test]
fn places_in_the_list_are_not_changed_while_the_filter_hides_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    let before = titles(&page, vcx);
    // Put the cursor on the first row, then filter and press Alt+Down.
    let at = bounds(vcx, "crate-row-1").center();
    vcx.simulate_click(at, Modifiers::control());
    vcx.run_until_parked();
    set_filter(&page, vcx, "a");
    page.crate_page.update(vcx, |p, cx| p.drop_row(1, 0, cx));
    vcx.simulate_keystrokes("alt-down");
    assert_eq!(titles(&page, vcx), before, "neither a drop nor Alt+Down moves a song");
    // With the filter gone the same key moves the row under the cursor (apple) one place down.
    set_filter(&page, vcx, "");
    vcx.simulate_keystrokes("alt-down");
    assert_eq!(titles(&page, vcx), ["Mango", "Cherry", "apple", "Banana", "Date"]);
}

// ---- to the top or the bottom --------------------------------------------------------------

#[gpui::test]
fn alt_home_and_alt_end_send_the_row_or_the_picks_to_an_end(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    // Ctrl+click puts the cursor on a row without playing it.
    let at = bounds(vcx, "crate-row-3").center();
    vcx.simulate_click(at, Modifiers::control());
    vcx.run_until_parked();
    vcx.simulate_keystrokes("alt-home");
    assert_eq!(titles(&page, vcx), ["Banana", "Mango", "apple", "Cherry", "Date"]);
    assert!(pill(&page, vcx).is_some_and(|p| p.starts_with("Moved to top")));
    vcx.simulate_keystrokes("alt-end");
    assert_eq!(titles(&page, vcx), ["Mango", "apple", "Cherry", "Date", "Banana"]);
    // A song already at the end stays and makes no step.
    vcx.simulate_keystrokes("alt-end");
    undo(&page, vcx);
    undo(&page, vcx);
    assert_eq!(titles(&page, vcx), ["Mango", "apple", "Cherry", "Banana", "Date"], "two steps back");

    // Picked rows go together, keeping their order.
    let at = bounds(vcx, "crate-row-4").center();
    vcx.simulate_click(at, Modifiers::control());
    vcx.simulate_keystrokes("shift-up"); // rows 3 and 4 are picked now
    // The undo tips above hold the pill for a few seconds; let them go first.
    vcx.executor().advance_clock(Duration::from_secs(6));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("alt-home");
    assert_eq!(titles(&page, vcx), ["Banana", "Date", "Mango", "apple", "Cherry"]);
    assert!(pill(&page, vcx).is_some_and(|p| p.starts_with("Moved 2 songs to top")));
}

#[gpui::test]
fn the_row_menu_has_move_to_top_and_bottom(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx);
    let at = bounds(vcx, "crate-row-2").center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
    let commands = page.menu.read_with(vcx, |m, cx| m.commands(cx));
    let ix = commands
        .iter()
        .position(|c| *c == Command::CrateToTop)
        .expect("Move to top is in a Crate row's menu");
    assert!(commands.contains(&Command::CrateToBottom));
    assert!(!commands.contains(&Command::MoveToTop), "that one is for a playlist of your own");
    click(vcx, &format!("track-menu-item-{ix}"));
    assert_eq!(titles(&page, vcx), ["Cherry", "Mango", "apple", "Banana", "Date"]);
}

#[gpui::test]
fn the_docked_crate_has_the_same_filter_and_arrange(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model.update(vcx, |m, cx| {
        for spec in &SONGS {
            m.add(track(spec), None, cx);
        }
    });
    page.shell.update(vcx, |s, cx| s.set_crate_dock(true, cx));
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "dock-filter") && on_screen(vcx, "dock-arrange"));
    click(vcx, "dock-arrange");
    assert!(on_screen(vcx, "dock-arrange-menu"));
    click(vcx, "dock-arrange-item-6"); // Reverse
    assert_eq!(titles(&page, vcx), ["Date", "Banana", "Cherry", "apple", "Mango"]);

    let dock = page.shell.read_with(vcx, |s, _| s.crate_dock_panel().clone());
    let field = dock.read_with(vcx, |p, _| p.filter_field().cloned()).expect("the dock has a filter");
    field.update(vcx, |f, cx| f.set_text("mango", cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "dock-row-0") && !on_screen(vcx, "dock-row-1"));
}
