//! Instant playlists: with Settings > Playlists > Create playlists instantly on, every way of
//! making a playlist skips the name dialog and opens the new page with its title ready to rename;
//! and the title of a playlist of your own can be renamed in place with the switch off too. The
//! real shell and views on GPUI's test platform, the simulated library as the "service".

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
use apollo_app::instant_playlist;
use apollo_app::settings::INSTANT_PLAYLISTS;
use apollo_app::ui::playlist_dialog::{self, Mode, PlaylistDialog};
use apollo_app::ui::playlist_picker::{PickerSong, PlaylistPicker};
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{self, Anchor, Command, Subject, TrackMenu};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{Collection, Privacy, WriteFailure};
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

// ---- helpers for this file -----------------------------------------------------------------------

/// What an instant playlist is called today when nothing is taken.
fn todays_name() -> String {
    let (month, day) = instant_playlist::today();
    instant_playlist::default_name(month, day, |_| false)
}

fn instant(page: &Page, vcx: &mut VisualTestContext, on: bool) {
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&INSTANT_PLAYLISTS, on, cx));
    vcx.run_until_parked();
}

/// The title of the page on screen, per the model.
fn open_title(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.view.read_with(vcx, |v, cx| {
        v.model().read(cx).detail().map(|d| d.collection.title.clone())
    })
}

fn renaming(vcx: &mut VisualTestContext) -> bool {
    on_screen(vcx, "detail-title-field")
}

fn rename_text(page: &Page, vcx: &mut VisualTestContext, text: &str) {
    // The page's title field is the only field of its kind on screen: type through the focus.
    vcx.simulate_input(text);
    vcx.run_until_parked();
    let _ = page;
}

fn only_writes(page: &Page) -> Vec<String> {
    log(page).into_iter().map(|(what, _)| what).collect()
}

// ---- off by default -----------------------------------------------------------------------------

#[gpui::test]
fn it_is_off_by_default_and_the_dialog_still_asks(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(!page
        .shell
        .read_with(vcx, |s, cx| s.settings(cx).read(cx).get(&INSTANT_PLAYLISTS)));
    click(vcx, "library-new-playlist");
    assert!(dialog_open(&page, vcx));
    assert!(log(&page).is_empty());
}

// ---- New playlist ---------------------------------------------------------------------------------

#[gpui::test]
fn new_playlist_makes_a_dated_private_playlist_and_opens_it_ready_to_rename(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    click(vcx, "library-new-playlist");

    assert!(!dialog_open(&page, vcx), "no dialog");
    assert_eq!(log(&page), vec![("create".to_owned(), todays_name())]);
    // (An empty create is always private: the transport sends PRIVATE.)
    assert!(page.sim.owned_ids()[0].starts_with("PLsimnew"));
    // Its page is open, the title a field with the name selected.
    assert_eq!(depth(&page, vcx), 1);
    assert_eq!(open_title(&page, vcx), Some(todays_name()));
    assert!(renaming(vcx));
    assert_eq!(notice(&page, vcx), None, "success is silent");
}

#[gpui::test]
fn typing_over_the_selected_name_and_pressing_enter_renames_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    click(vcx, "library-new-playlist");
    rename_text(&page, vcx, "Gym mix");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(
        log(&page),
        vec![
            ("create".to_owned(), todays_name()),
            ("edit:title=Gym mix".to_owned(), made),
        ]
    );
    assert!(!renaming(vcx), "the field gives the title back");
    assert!(on_screen(vcx, "detail-title-rename"));
    assert_eq!(open_title(&page, vcx), Some("Gym mix".to_owned()));
    assert!(titles(&page, vcx, Tab::Playlists).contains(&"Gym mix".to_owned()));
    assert!(!titles(&page, vcx, Tab::Playlists).contains(&todays_name()));
}

#[gpui::test]
fn escape_keeps_the_default_name_and_sends_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    click(vcx, "library-new-playlist");
    rename_text(&page, vcx, "Never mind");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();

    assert!(!renaming(vcx));
    assert_eq!(only_writes(&page), vec!["create".to_owned()]);
    assert_eq!(open_title(&page, vcx), Some(todays_name()));
    // Esc ended the rename only: the page stays open.
    assert_eq!(depth(&page, vcx), 1);
}

#[gpui::test]
fn enter_with_the_name_unchanged_or_empty_sends_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    click(vcx, "library-new-playlist");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(!renaming(vcx));
    assert_eq!(only_writes(&page), vec!["create".to_owned()]);

    // Blank counts as nothing typed.
    click(vcx, "detail-title-rename");
    assert!(renaming(vcx));
    rename_text(&page, vcx, "   ");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(only_writes(&page), vec!["create".to_owned()]);
    assert_eq!(open_title(&page, vcx), Some(todays_name()));
}

#[gpui::test]
fn a_second_untitled_playlist_the_same_day_is_numbered(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    click(vcx, "library-new-playlist");
    vcx.simulate_keystrokes("escape");
    page.view.update_in(vcx, |v, window, cx| v.back(window, cx));
    vcx.run_until_parked();
    click(vcx, "library-new-playlist");
    vcx.simulate_keystrokes("escape");
    page.view.update_in(vcx, |v, window, cx| v.back(window, cx));
    vcx.run_until_parked();
    click(vcx, "library-new-playlist");

    let first = todays_name();
    let created: Vec<String> = log(&page).into_iter().map(|(_, title)| title).collect();
    assert_eq!(created, vec![first.clone(), format!("{first} 2"), format!("{first} 3")]);
}

#[gpui::test]
fn a_refused_instant_create_opens_nothing_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    click(vcx, "library-new-playlist");
    assert!(!dialog_open(&page, vcx));
    assert_eq!(depth(&page, vcx), 0);
    assert!(!renaming(vcx));
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::Create, WriteFailure::Rejected))
    );

    // The next try works: the refusal left nothing waiting.
    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    click(vcx, "library-new-playlist");
    assert_eq!(depth(&page, vcx), 1);
    assert!(renaming(vcx));
}

// ---- Save as playlist -------------------------------------------------------------------------

#[gpui::test]
fn saving_the_queue_makes_it_instantly_with_its_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    let ids = vec!["simTrack001".to_owned(), "simTrack002".to_owned()];
    vcx.update(|window, cx| {
        playlist_dialog::open(
            Mode::FromQueue {
                name: "My queue".into(),
                video_ids: ids.clone(),
            },
            window,
            cx,
        )
    });
    vcx.run_until_parked();

    assert!(!dialog_open(&page, vcx));
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), ids);
    assert_eq!(
        log(&page),
        vec![("create-with:2".to_owned(), todays_name())],
        "one write holds the playlist and its songs"
    );
    assert_eq!(open_title(&page, vcx), Some(todays_name()));
    assert!(renaming(vcx));
}

#[gpui::test]
fn saving_the_crate_makes_it_instantly_and_leaves_the_crate_alone(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    instant(&page, vcx, true);
    let crate_model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    crate_model.update(vcx, |m, cx| {
        for id in ["crateAAAA01", "crateBBBB02"] {
            m.add(apollo_domain::model::Track::new(id, format!("Song {id}")), None, cx);
        }
    });
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    click(vcx, "crate-save-playlist");

    assert!(!dialog_open(&page, vcx), "no dialog, so no Clear Crate switch to ask");
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), ["crateAAAA01", "crateBBBB02"]);
    assert_eq!(crate_model.read_with(vcx, |m, _| m.len()), 2, "the Crate stays");
    assert_eq!(open_title(&page, vcx), Some(todays_name()));
    assert!(renaming(vcx));
}

// ---- the title, with the switch off ----------------------------------------------------------

#[gpui::test]
fn the_title_of_a_playlist_of_your_own_renames_in_place_whatever_the_switch_says(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    assert!(on_screen(vcx, "detail-title-rename"), "yours: the title is a click target");
    click(vcx, "detail-title-rename");
    assert!(renaming(vcx));
    rename_text(&page, vcx, "Summer drive");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        log(&page),
        vec![("edit:title=Summer drive".to_owned(), ROAD_TRIP_ID.to_owned())]
    );
    assert_eq!(open_title(&page, vcx), Some("Summer drive".to_owned()));
    assert!(titles(&page, vcx, Tab::Playlists).contains(&"Summer drive".to_owned()));
}

#[gpui::test]
fn a_refused_rename_keeps_the_old_title_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    click(vcx, "detail-title-rename");
    rename_text(&page, vcx, "Summer drive");
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(open_title(&page, vcx), Some(ROAD_TRIP.to_owned()));
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::Update, WriteFailure::Rejected))
    );
}

#[gpui::test]
fn only_a_playlist_of_your_own_has_a_title_to_click(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // An album, and Liked Music: not the account's to rename.
    let album = page
        .view
        .read_with(vcx, |v, cx| v.model().read(cx).list(Tab::Albums)[0].clone());
    page.view
        .update_in(vcx, |v, window, cx| v.open(album, window, cx));
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "detail-title-rename"));
    assert!(!renaming(vcx));
}

#[gpui::test]
fn leaving_the_page_ends_the_rename_without_sending_a_changed_name_elsewhere(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    click(vcx, "detail-title-rename");
    assert!(renaming(vcx));
    // Back from the page: the field does not follow to the lists.
    page.view.update_in(vcx, |v, window, cx| v.back(window, cx));
    vcx.run_until_parked();
    assert!(!renaming(vcx));
    assert!(log(&page).is_empty(), "nothing was changed, nothing is sent");
}
