//! Headless tests for playlist editing: creating, renaming and deleting playlists through the
//! dialog and the menus, removing and reordering songs, and saving or unsaving albums and other
//! people's playlists. The real shell and views on GPUI's test platform, the simulated library
//! as the "service" (it can lose a reply, refuse a write or change account).

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::edits::{failure_text, Edit, LibraryEdits};
use apollo_app::library::model::Tab;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::playlist_dialog::{Mode, PlaylistDialog};
use apollo_app::ui::playlist_picker::PlaylistPicker;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{self, Anchor, Command, Subject, TrackMenu};
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
    picker: Entity<PlaylistPicker>,
    dialog: Entity<PlaylistDialog>,
    edits: Entity<LibraryEdits>,
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
    let (view, menu, picker, dialog, edits) = shell.read_with(vcx, |s, _| {
        (
            s.library().clone(),
            s.menu().clone(),
            s.picker().clone(),
            s.dialog().clone(),
            s.edits().clone(),
        )
    });
    (
        Page {
            shell,
            view,
            menu,
            picker,
            dialog,
            edits,
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

fn titles(page: &Page, vcx: &mut VisualTestContext, tab: Tab) -> Vec<String> {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .list(tab)
            .iter()
            .map(|c| c.title.clone())
            .collect()
    })
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

fn row_of(page: &Page, vcx: &mut VisualTestContext, tab: Tab, title: &str) -> usize {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .list(tab)
            .iter()
            .position(|c| c.title == title)
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

fn dialog_open(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.dialog.read_with(vcx, |d, _| d.is_open())
}

fn dialog_mode(page: &Page, vcx: &mut VisualTestContext) -> Option<Mode> {
    page.dialog.read_with(vcx, |d, _| d.mode().cloned())
}

fn type_name(page: &Page, vcx: &mut VisualTestContext, text: &str) {
    let field = page.dialog.read_with(vcx, |d, _| d.field().clone());
    field.update(vcx, |f, cx| f.set_text(text, cx));
    vcx.run_until_parked();
}

fn submit(page: &Page, vcx: &mut VisualTestContext) {
    page.dialog.update(vcx, |d, cx| d.submit(cx));
    vcx.run_until_parked();
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

fn depth(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.view.read_with(vcx, |v, cx| v.model().read(cx).depth())
}

// ---- create --------------------------------------------------------------------------------------

#[gpui::test]
fn the_new_playlist_button_asks_for_a_name_and_makes_a_private_playlist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(on_screen(vcx, "library-new-playlist"));
    assert!(!dialog_open(&page, vcx));
    click(vcx, "library-new-playlist");
    assert!(dialog_open(&page, vcx));
    assert_eq!(dialog_mode(&page, vcx), Some(Mode::Create { add_video: None }));
    // Nothing to send while the name is empty or blank.
    assert!(!page.dialog.read_with(vcx, |d, cx| d.can_submit(cx)));
    type_name(&page, vcx, "   ");
    assert!(!page.dialog.read_with(vcx, |d, cx| d.can_submit(cx)));
    submit(&page, vcx);
    assert!(log(&page).is_empty(), "a blank name sends nothing");

    type_name(&page, vcx, "  Apollo   test ");
    assert!(page.dialog.read_with(vcx, |d, cx| d.can_submit(cx)));
    submit(&page, vcx);
    assert_eq!(log(&page), vec![("create".to_owned(), "Apollo test".to_owned())]);
    assert!(!dialog_open(&page, vcx), "the dialog closes when it is made");
    // The library shows it at the top of the playlists, after Liked Music.
    assert_eq!(titles(&page, vcx, Tab::Playlists)[1], "Apollo test");
    assert_eq!(notice(&page, vcx), None, "success is silent");
    let made = page.sim.owned_ids()[0].clone();
    // What it made can be edited at once: it is one of the account's own playlists.
    let new = collection(&page, vcx, Tab::Playlists, "Apollo test");
    assert!(page.edits.read_with(vcx, |e, _| e.is_editable(&new)));
    assert!(made.starts_with("PLsimnew"));
}

#[gpui::test]
fn enter_confirms_the_name_and_escape_cancels(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    type_name(&page, vcx, "Apollo test");
    vcx.simulate_keystrokes("escape");
    assert!(!dialog_open(&page, vcx));
    assert!(log(&page).is_empty());

    click(vcx, "library-new-playlist");
    // The field starts empty again, with the caret in it.
    assert_eq!(page.dialog.read_with(vcx, |d, cx| d.field().read(cx).text().to_owned()), "");
    type_name(&page, vcx, "Apollo test");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(log(&page), vec![("create".to_owned(), "Apollo test".to_owned())]);
    assert!(!dialog_open(&page, vcx));
}

#[gpui::test]
fn a_refused_create_keeps_the_dialog_and_the_name_and_leaves_a_toast(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    type_name(&page, vcx, "Apollo test");
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    submit(&page, vcx);
    assert!(dialog_open(&page, vcx), "the name is not lost");
    assert!(!page.dialog.read_with(vcx, |d, _| d.is_busy()));
    assert_eq!(
        page.dialog.read_with(vcx, |d, cx| d.field().read(cx).text().to_owned()),
        "Apollo test"
    );
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::Create, WriteFailure::Rejected))
    );
    assert_eq!(titles(&page, vcx, Tab::Playlists).iter().filter(|t| *t == "Apollo test").count(), 0);

    // Trying again once the service answers makes it.
    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    submit(&page, vcx);
    assert!(!dialog_open(&page, vcx));
    assert_eq!(titles(&page, vcx, Tab::Playlists)[1], "Apollo test");
}

#[gpui::test]
fn a_create_whose_reply_is_lost_is_read_back_and_never_sent_twice(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    type_name(&page, vcx, "Apollo test");
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    submit(&page, vcx);
    let creates = log(&page)
        .iter()
        .filter(|(what, _)| what == "create")
        .count();
    assert_eq!(creates, 1, "one send, then a read-back");
    assert!(!dialog_open(&page, vcx), "the read-back found it: it was made");
    assert_eq!(
        titles(&page, vcx, Tab::Playlists)
            .iter()
            .filter(|t| *t == "Apollo test")
            .count(),
        1
    );
}

#[gpui::test]
fn a_create_that_cannot_be_read_back_is_reported_as_unknown_not_retried(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    type_name(&page, vcx, "Apollo test");
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    page.sim.set_collections_failure(Some(LoadFailure::Network));
    submit(&page, vcx);
    let creates = log(&page)
        .iter()
        .filter(|(what, _)| what == "create")
        .count();
    assert_eq!(creates, 1, "never retried blind");
    assert!(dialog_open(&page, vcx));
    assert!(notice(&page, vcx).is_some());
}

// ---- rename and delete ---------------------------------------------------------------------------

#[gpui::test]
fn rename_from_a_playlist_card_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let ix = row_of(&page, vcx, Tab::Playlists, ROAD_TRIP);
    let at = bounds(vcx, &format!("library-row-{ix}")).center();
    right_click(vcx, at);
    let offered = commands(&page, vcx);
    assert!(offered.contains(&Command::Rename));
    assert!(offered.contains(&Command::DeletePlaylist));
    assert!(!offered.contains(&Command::SaveToLibrary));
    assert!(!offered.contains(&Command::RemoveFromLibrary), "your own playlists are not 'saved'");
    choose(&page, vcx, Command::Rename);
    assert!(!menu_open(&page, vcx));
    assert!(matches!(dialog_mode(&page, vcx), Some(Mode::Rename(c)) if c.title == ROAD_TRIP));
    // It starts from the current name.
    assert_eq!(
        page.dialog.read_with(vcx, |d, cx| d.field().read(cx).text().to_owned()),
        ROAD_TRIP
    );
    // Unchanged: closes without a write.
    submit(&page, vcx);
    assert!(!dialog_open(&page, vcx));
    assert!(log(&page).is_empty());

    right_click(vcx, at);
    choose(&page, vcx, Command::Rename);
    type_name(&page, vcx, "  Summer drive ");
    submit(&page, vcx);
    assert!(!dialog_open(&page, vcx));
    assert_eq!(
        log(&page),
        vec![("rename:Summer drive".to_owned(), ROAD_TRIP_ID.to_owned())]
    );
    assert_eq!(titles(&page, vcx, Tab::Playlists)[ix], "Summer drive");
}

#[gpui::test]
fn a_failed_rename_keeps_the_old_title_and_the_dialog(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let ix = row_of(&page, vcx, Tab::Playlists, ROAD_TRIP);
    let at = bounds(vcx, &format!("library-row-{ix}")).center();
    right_click(vcx, at);
    choose(&page, vcx, Command::Rename);
    type_name(&page, vcx, "Summer drive");
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    submit(&page, vcx);
    assert!(dialog_open(&page, vcx));
    assert_eq!(titles(&page, vcx, Tab::Playlists)[ix], ROAD_TRIP);
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::Rename, WriteFailure::Rejected))
    );
}

#[gpui::test]
fn delete_asks_first_and_cancel_keeps_the_playlist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let ix = row_of(&page, vcx, Tab::Playlists, ROAD_TRIP);
    let at = bounds(vcx, &format!("library-row-{ix}")).center();
    right_click(vcx, at);
    choose(&page, vcx, Command::DeletePlaylist);
    assert!(matches!(dialog_mode(&page, vcx), Some(Mode::Delete(c)) if c.title == ROAD_TRIP));
    assert!(on_screen(vcx, "playlist-dialog-name"));
    assert!(!on_screen(vcx, "playlist-dialog-field"), "no name field to fill in");
    // Enter on the focused Cancel keeps the playlist; so does Escape.
    press_enter(vcx);
    assert!(!dialog_open(&page, vcx));
    assert!(log(&page).is_empty());
    assert!(titles(&page, vcx, Tab::Playlists).contains(&ROAD_TRIP.to_owned()));

    right_click(vcx, at);
    choose(&page, vcx, Command::DeletePlaylist);
    vcx.simulate_keystrokes("escape");
    assert!(!dialog_open(&page, vcx));
    assert!(log(&page).is_empty());

    right_click(vcx, at);
    choose(&page, vcx, Command::DeletePlaylist);
    submit(&page, vcx);
    assert!(!dialog_open(&page, vcx));
    assert_eq!(log(&page), vec![("delete".to_owned(), ROAD_TRIP_ID.to_owned())]);
    assert!(!titles(&page, vcx, Tab::Playlists).contains(&ROAD_TRIP.to_owned()));
}

#[gpui::test]
fn deleting_from_the_page_menu_closes_the_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    assert_eq!(depth(&page, vcx), 1);
    click(vcx, "detail-more");
    assert!(menu_open(&page, vcx));
    choose(&page, vcx, Command::DeletePlaylist);
    submit(&page, vcx);
    // The page closes the frame after the delete lands.
    vcx.run_until_parked();
    assert_eq!(depth(&page, vcx), 0);
    assert!(!titles(&page, vcx, Tab::Playlists).contains(&ROAD_TRIP.to_owned()));
}

#[gpui::test]
fn renaming_from_the_page_menu_retitles_the_open_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    // Right-click on the header, beside the title.
    let more = bounds(vcx, "detail-more").center();
    right_click(vcx, point(more.x - gpui::px(300.0), more.y));
    assert!(commands(&page, vcx).contains(&Command::Rename), "right-click on the header too");
    choose(&page, vcx, Command::Rename);
    type_name(&page, vcx, "Summer drive");
    submit(&page, vcx);
    let title = page.view.read_with(vcx, |v, cx| {
        v.model().read(cx).detail().map(|d| d.collection.title.clone())
    });
    assert_eq!(title.as_deref(), Some("Summer drive"));
}

#[gpui::test]
fn only_one_write_per_playlist_is_out_at_a_time(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let c = collection(&page, vcx, Tab::Playlists, ROAD_TRIP);
    page.edits.update(vcx, |e, cx| {
        e.rename(c.clone(), "One".into(), cx);
        e.rename(c.clone(), "Two".into(), cx);
    });
    vcx.run_until_parked();
    let renames: Vec<_> = log(&page)
        .into_iter()
        .filter(|(what, _)| what.starts_with("rename:"))
        .collect();
    assert_eq!(renames.len(), 1, "the second ask while one is out is ignored");
}

// ---- songs ---------------------------------------------------------------------------------------

#[gpui::test]
fn remove_from_playlist_is_in_the_menu_of_your_own_playlist_only(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    assert_eq!(before.len(), 3);
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    assert!(commands(&page, vcx).contains(&Command::RemoveFromPlaylist));
    choose(&page, vcx, Command::RemoveFromPlaylist);
    assert!(!menu_open(&page, vcx));
    assert_eq!(rows(&page, vcx), before[1..].to_vec(), "the row is gone at once");
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID).len(), 2);
    assert_eq!(notice(&page, vcx), None);
    assert!(log(&page).iter().any(|(w, _)| w == &format!("remove:{ROAD_TRIP_ID}")));

    // Liked Music is not a playlist you edit: the song menu has no such item.
    page.view.update_in(vcx, |v, window, cx| v.back(window, cx));
    let liked = collection(&page, vcx, Tab::Playlists, "Liked Music");
    page.view.update_in(vcx, |v, window, cx| v.open(liked, window, cx));
    vcx.run_until_parked();
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    assert!(!commands(&page, vcx).contains(&Command::RemoveFromPlaylist));
}

#[gpui::test]
fn another_persons_playlist_has_no_song_editing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // "Mix 020" is in the library but not one of the account's own playlists.
    open_playlist(&page, vcx, "Mix 020");
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    assert!(!commands(&page, vcx).contains(&Command::RemoveFromPlaylist));
    vcx.simulate_keystrokes("escape");
    // Its page menu offers the library toggle instead of Rename and Delete.
    click(vcx, "detail-more");
    let offered = commands(&page, vcx);
    assert!(offered.contains(&Command::RemoveFromLibrary));
    assert!(!offered.contains(&Command::Rename));
    assert!(!offered.contains(&Command::DeletePlaylist));
}

#[gpui::test]
fn a_refused_remove_puts_the_row_back_and_leaves_a_toast(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::RemoveFromPlaylist);
    vcx.run_until_parked();
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::RemoveSong, WriteFailure::Rejected))
    );
    assert_eq!(rows(&page, vcx), before, "the service's own list is back");
}

#[gpui::test]
fn a_remove_whose_reply_is_lost_is_settled_by_reading_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    choose(&page, vcx, Command::RemoveFromPlaylist);
    vcx.run_until_parked();
    // It had landed: the row stays gone and nothing is reported.
    assert_eq!(rows(&page, vcx), before[1..].to_vec());
    assert_eq!(notice(&page, vcx), None);
    let removes = log(&page)
        .iter()
        .filter(|(w, _)| w.starts_with("remove:"))
        .count();
    assert_eq!(removes, 1, "never sent twice");
}

#[gpui::test]
fn dragging_a_row_reorders_the_playlist_and_the_service(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    let tokens_before = page.sim.occurrences(ROAD_TRIP_ID);
    let from = bounds(vcx, "library-row-0").center();
    let target = bounds(vcx, "library-row-2");
    // Down on the first row, move across the rows, let go in the upper half of the third: the
    // row lands before it.
    vcx.simulate_mouse_move(from, None, Modifiers::none());
    vcx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    let to = point(target.center().x, target.origin.y + gpui::px(8.0));
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let at = point(
            from.x + (to.x - from.x) * t,
            from.y + (to.y - from.y) * t,
        );
        vcx.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::none());
    }
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    let after = rows(&page, vcx);
    assert_eq!(
        after,
        vec![before[1].clone(), before[0].clone(), before[2].clone()],
        "the first row moved in front of the third"
    );
    let tokens_after = page.sim.occurrences(ROAD_TRIP_ID);
    assert_eq!(
        tokens_after,
        vec![
            tokens_before[1].clone(),
            tokens_before[0].clone(),
            tokens_before[2].clone()
        ]
    );
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn moving_a_row_to_the_end_and_a_row_onto_itself(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    // Dropped where it already is: nothing moves and nothing is sent.
    assert!(!model.update(vcx, |m, cx| m.move_row(1, 1, cx)));
    assert!(!model.update(vcx, |m, cx| m.move_row(1, 2, cx)));
    vcx.run_until_parked();
    assert!(log(&page).is_empty());
    assert_eq!(rows(&page, vcx), before);
    // The end slot is one past the last row.
    assert!(model.update(vcx, |m, cx| m.move_row(0, 3, cx)));
    vcx.run_until_parked();
    assert_eq!(
        rows(&page, vcx),
        vec![before[1].clone(), before[2].clone(), before[0].clone()]
    );
    assert_eq!(
        page.sim.occurrences(ROAD_TRIP_ID).len(),
        3,
        "the service holds the same three rows"
    );
}

#[gpui::test]
fn reloading_the_library_while_an_edit_is_out_does_not_stall_the_next_edit(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    let tokens = page.sim.occurrences(ROAD_TRIP_ID);
    // Nothing runs between these calls: the first edit is out when the library reloads.
    model.update(vcx, |m, cx| {
        m.remove_row(ROAD_TRIP_ID, &tokens[0], cx);
        m.reload_collections(cx);
        m.remove_row(ROAD_TRIP_ID, &tokens[1], cx);
    });
    vcx.run_until_parked();
    assert_eq!(page.sim.occurrences(ROAD_TRIP_ID).len(), 1, "both removes went out, one after the other");
    assert!(!model.read_with(vcx, |m, _| m.edits_pending(ROAD_TRIP_ID)));
}

#[gpui::test]
fn a_refused_move_shows_the_services_order_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = rows(&page, vcx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    assert!(model.update(vcx, |m, cx| m.move_row(0, 3, cx)));
    vcx.run_until_parked();
    assert_eq!(rows(&page, vcx), before);
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::MoveSong, WriteFailure::Rejected))
    );
}

#[gpui::test]
fn a_playlist_that_is_not_yours_cannot_be_reordered(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, "Mix 020");
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    assert!(!model.update(vcx, |m, cx| m.move_row(0, 2, cx)));
    vcx.run_until_parked();
    assert!(log(&page).is_empty());
}

// ---- saving to the library -------------------------------------------------------------------------

#[gpui::test]
fn an_album_can_be_taken_out_of_the_library_and_saved_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.view.update(vcx, |v, cx| {
        v.model().update(cx, |m, cx| m.set_tab(Tab::Albums, cx))
    });
    vcx.run_until_parked();
    let album = collection(&page, vcx, Tab::Albums, "Greenhouse");
    let ix = row_of(&page, vcx, Tab::Albums, "Greenhouse");
    let at = bounds(vcx, &format!("library-row-{ix}")).center();
    right_click(vcx, at);
    let offered = commands(&page, vcx);
    assert!(offered.contains(&Command::RemoveFromLibrary));
    assert!(!offered.contains(&Command::Rename));
    choose(&page, vcx, Command::RemoveFromLibrary);
    assert!(!titles(&page, vcx, Tab::Albums).contains(&"Greenhouse".to_owned()));
    assert!(log(&page).iter().any(|(w, id)| w == "unsave" && id.starts_with("OLAK5uy_")));
    assert_eq!(notice(&page, vcx), None);

    // From an album page that is no longer in the library the menu offers to save it.
    page.view
        .update_in(vcx, |v, window, cx| v.open(album.clone(), window, cx));
    vcx.run_until_parked();
    click(vcx, "detail-more");
    assert!(commands(&page, vcx).contains(&Command::SaveToLibrary));
    choose(&page, vcx, Command::SaveToLibrary);
    assert!(titles(&page, vcx, Tab::Albums).contains(&"Greenhouse".to_owned()));
    assert!(log(&page).iter().any(|(w, _)| w == "save"));
}

#[gpui::test]
fn another_persons_playlist_is_saved_and_unsaved_from_its_card(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let mix = collection(&page, vcx, Tab::Playlists, "Mix 020");
    let ix = row_of(&page, vcx, Tab::Playlists, "Mix 020");
    // Bring the card into view.
    page.view.update(vcx, |v, cx| {
        v.model().update(cx, |m, cx| m.set_tab(Tab::Playlists, cx))
    });
    let handle_row = ix.min(8);
    let at = bounds(vcx, &format!("library-row-{handle_row}")).center();
    right_click(vcx, at);
    let offered = commands(&page, vcx);
    assert!(offered.contains(&Command::RemoveFromLibrary) || offered.contains(&Command::Rename));
    vcx.simulate_keystrokes("escape");

    // Drive the same menu through the shared entry point.
    page.edits.update(vcx, |e, cx| e.set_saved(mix.clone(), false, cx));
    vcx.run_until_parked();
    assert!(!titles(&page, vcx, Tab::Playlists).contains(&"Mix 020".to_owned()));
    assert!(log(&page).iter().any(|(w, id)| w == "unsave" && id == "PLsimmix020"));

    vcx.update(|window, cx| {
        track_menu::open(
            Subject::Collection(mix.clone()),
            Anchor::Point(point(gpui::px(300.0), gpui::px(300.0))),
            window,
            cx,
        )
    });
    vcx.run_until_parked();
    assert!(commands(&page, vcx).contains(&Command::SaveToLibrary));
    choose(&page, vcx, Command::SaveToLibrary);
    assert!(titles(&page, vcx, Tab::Playlists).contains(&"Mix 020".to_owned()));
}

#[gpui::test]
fn your_own_playlists_and_liked_music_have_no_save_item(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let liked = collection(&page, vcx, Tab::Playlists, "Liked Music");
    let road = collection(&page, vcx, Tab::Playlists, ROAD_TRIP);
    for c in [liked, road] {
        assert_eq!(page.edits.read_with(vcx, |e, _| e.saved_state(&c)), None);
    }
}

// ---- the add-to-playlist popover -------------------------------------------------------------------

#[gpui::test]
fn the_popover_has_a_new_playlist_row_that_makes_a_playlist_holding_the_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.picker.update_in(vcx, |p, window, cx| {
        p.open("simTrack001", "Midnight Drive", window, cx)
    });
    vcx.run_until_parked();
    assert!(on_screen(vcx, "picker-new"), "the first row is always there");
    click(vcx, "picker-new");
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()), "the popover closes");
    assert_eq!(
        dialog_mode(&page, vcx),
        Some(Mode::Create {
            add_video: Some("simTrack001".to_owned())
        })
    );
    type_name(&page, vcx, "Apollo test");
    submit(&page, vcx);
    assert!(!dialog_open(&page, vcx));
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(
        page.sim.added(),
        vec![(made, "simTrack001".to_owned())],
        "the song went into the new playlist"
    );
}

#[gpui::test]
fn up_from_the_first_playlist_reaches_the_new_playlist_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.picker.update_in(vcx, |p, window, cx| {
        p.open("simTrack001", "Midnight Drive", window, cx)
    });
    vcx.run_until_parked();
    assert!(!page.picker.read_with(vcx, |p, _| p.on_new()));
    vcx.simulate_keystrokes("up");
    assert!(page.picker.read_with(vcx, |p, _| p.on_new()));
    // Down returns to the first playlist.
    vcx.simulate_keystrokes("down");
    assert!(!page.picker.read_with(vcx, |p, _| p.on_new()));
    assert_eq!(page.picker.read_with(vcx, |p, _| p.cursor()), Some(0));
    vcx.simulate_keystrokes("up enter");
    vcx.run_until_parked();
    assert!(dialog_open(&page, vcx));
    assert!(page.sim.added().is_empty());
}

#[gpui::test]
fn a_picker_that_cannot_list_playlists_still_offers_a_new_one(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_playlists_failure(Some(LoadFailure::Network));
    page.picker.update_in(vcx, |p, window, cx| {
        p.open("simTrack001", "Midnight Drive", window, cx)
    });
    vcx.run_until_parked();
    assert!(on_screen(vcx, "picker-new"));
    assert!(page.picker.read_with(vcx, |p, _| p.on_new()));
}

// ---- the dialog itself -----------------------------------------------------------------------------

#[gpui::test]
fn tab_moves_between_the_field_and_the_buttons_and_stays_inside(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    let field_focused = |page: &Page, vcx: &mut VisualTestContext| {
        let field = page.dialog.read_with(vcx, |d, _| d.field().clone());
        vcx.update(|window, cx| {
            gpui::Focusable::focus_handle(field.read(cx), cx).is_focused(window)
        })
    };
    assert!(field_focused(&page, vcx), "the caret starts in the name");
    vcx.simulate_keystrokes("tab");
    assert!(!field_focused(&page, vcx));
    vcx.simulate_keystrokes("tab tab");
    assert!(field_focused(&page, vcx), "three stops, and round again");
    vcx.simulate_keystrokes("shift-tab");
    assert!(!field_focused(&page, vcx));
    assert!(dialog_open(&page, vcx));
}

#[gpui::test]
fn a_click_outside_cancels_unless_a_write_is_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    let size = vcx.update(|window, _| window.viewport_size());
    let outside = point(size.width - gpui::px(10.0), size.height - gpui::px(10.0));
    vcx.simulate_click(outside, Modifiers::none());
    assert!(!dialog_open(&page, vcx));
    assert!(log(&page).is_empty());
}

#[gpui::test]
fn a_name_is_cut_to_what_the_service_accepts(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    click(vcx, "library-new-playlist");
    let long = "x".repeat(400);
    type_name(&page, vcx, &long);
    let cleaned = page.dialog.read_with(vcx, |d, cx| d.cleaned(cx)).unwrap();
    assert_eq!(cleaned.chars().count(), apollo_domain::library::MAX_PLAYLIST_TITLE);
}

#[gpui::test]
fn the_new_playlist_button_waits_for_the_library(cx: &mut TestAppContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
    let sim = SimLibrary::instant();
    sim.set_collections_failure(Some(LoadFailure::Network));
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    let ready = prepared(&sim);
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    assert!(
        !on_screen(vcx, "library-new-playlist"),
        "no button while the library failed to load"
    );
}
