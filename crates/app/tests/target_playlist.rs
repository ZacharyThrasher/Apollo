//! The target playlist: A adds the playing song (or the cursor row, or every picked song) to one
//! chosen playlist, with no dialog. The real shell and views on GPUI's test platform, the
//! simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::Load;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::{SettingsModel, TARGET_KEY, TARGET_PILL};
use apollo_app::style::Fonts;
use apollo_app::target_playlist::{Target, TargetModel};
use apollo_app::ui::nav::Route;
use apollo_app::ui::playlist_picker::PlaylistPicker;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::WriteFailure;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

/// The track that is playing at the start.
const PLAYING: &str = "simTrack001";
/// Up next, and the first of the search results.
const NEXT: &str = "plainTrack1";
const OTHER: &str = "otherTrack1";

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        let song = |id: &str, title: &str| ListedTrack {
            track: Track::new(id, title),
            occurrence: None,
            thumbnail: None,
            explicit: false,
            liked: None,
        };
        Ok(SearchResults {
            songs: vec![song(NEXT, "Plain Song"), song(OTHER, "Other Song")],
            ..SearchResults::default()
        })
    }
}

struct Page {
    shell: Entity<Shell>,
    picker: Entity<PlaylistPicker>,
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
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![
            LibraryTrack {
                track: Track::new(PLAYING, "Midnight Drive"),
            },
            LibraryTrack {
                track: Track::new(NEXT, "Plain Song"),
            },
        ],
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
    let (picker, target, settings) = shell.read_with(vcx, |s, cx| {
        (s.picker().clone(), s.target_model().clone(), s.settings(cx))
    });
    (
        Page {
            shell,
            picker,
            target,
            settings,
            sim,
        },
        vcx,
    )
}

fn target_of(page: &Page, vcx: &mut VisualTestContext) -> Option<Target> {
    page.target.read_with(vcx, |m, cx| m.target(cx))
}

fn set_target(page: &Page, vcx: &mut VisualTestContext, id: &str, name: &str) {
    page.target.update(vcx, |m, cx| m.set(id, name, cx));
    vcx.run_until_parked();
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.writes().read(cx).notice().map(|n| n.text.to_string())
    })
}

fn added(page: &Page) -> Vec<(String, String)> {
    page.sim.added()
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
}

fn picker_open(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.picker.read_with(vcx, |p, _| p.is_open())
}

fn first_playlist(page: &Page, vcx: &mut VisualTestContext) -> (String, String) {
    page.picker.read_with(vcx, |p, _| match p.playlists() {
        Some(Load::Ready(list)) => (list[0].id.clone(), list[0].title.clone()),
        other => panic!("playlists are listed: {}", other.is_some()),
    })
}

// ---- A with a target ----------------------------------------------------------------------------

#[gpui::test]
fn a_adds_the_playing_song_to_the_target_with_no_dialog(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimroad".to_owned(), PLAYING.to_owned())]);
    assert!(!picker_open(&page, vcx), "nothing opens");
    assert_eq!(notice(&page, vcx), None, "success is silent");
}

#[gpui::test]
fn a_song_already_added_is_not_sent_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimempty", "Nothing Yet");
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(
        added(&page),
        [("PLsimempty".to_owned(), PLAYING.to_owned())],
        "one add, not two"
    );
    assert_eq!(page.sim.write_log().len(), 1, "and one request");
}

#[gpui::test]
fn a_refused_add_rolls_back_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimempty", "Nothing Yet");
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert!(added(&page).is_empty());
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't add to playlist"));
    // The failure took the song back, so a second press tries again.
    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimempty".to_owned(), PLAYING.to_owned())]);
}

#[gpui::test]
fn the_key_does_nothing_when_its_switch_is_off(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    page.settings.update(vcx, |s, cx| s.set(&TARGET_KEY, false, cx));
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert!(added(&page).is_empty());
    assert!(!picker_open(&page, vcx));
}

// ---- A with no target ---------------------------------------------------------------------------

#[gpui::test]
fn a_with_no_target_opens_the_picker_to_choose_one_then_adds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(target_of(&page, vcx), None);
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert!(picker_open(&page, vcx));
    assert!(page.picker.read_with(vcx, |p, _| p.is_choosing_target()));
    assert!(vcx.debug_bounds("picker-caption").is_some(), "the caption says what it is for");
    let (id, title) = first_playlist(&page, vcx);

    vcx.update(|window, cx| page.picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();
    assert!(!picker_open(&page, vcx));
    assert_eq!(
        target_of(&page, vcx),
        Some(Target {
            id: id.clone(),
            name: title
        })
    );
    assert_eq!(added(&page), [(id, PLAYING.to_owned())], "the song that asked goes in too");
}

#[gpui::test]
fn the_pill_shows_the_name_and_reopens_the_picker_to_change_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(vcx.debug_bounds("target-pill").is_none(), "no target, no pill");
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("target-pill").is_some());

    click(vcx, "target-pill");
    vcx.run_until_parked();
    assert!(picker_open(&page, vcx));
    assert!(page.picker.read_with(vcx, |p, _| p.is_choosing_target()));
    // The current target carries the check; choosing another changes the target and adds nothing.
    let (id, _) = first_playlist(&page, vcx);
    let other = page.picker.read_with(vcx, |p, _| match p.playlists() {
        Some(Load::Ready(list)) => list.iter().position(|t| t.id != "PLsimroad").expect("another"),
        _ => panic!("loaded"),
    });
    let _ = id;
    vcx.update(|window, cx| page.picker.update(cx, |p, cx| p.choose(other, window, cx)));
    vcx.run_until_parked();
    assert_ne!(target_of(&page, vcx).map(|t| t.id), Some("PLsimroad".to_owned()));
    assert!(added(&page).is_empty(), "changing the target adds nothing");
}

#[gpui::test]
fn clear_takes_the_target_away_and_the_pill_with_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    click(vcx, "target-pill");
    vcx.run_until_parked();
    click(vcx, "picker-clear-target");
    vcx.run_until_parked();
    assert_eq!(target_of(&page, vcx), None);
    assert!(!picker_open(&page, vcx));
    assert!(vcx.debug_bounds("target-pill").is_none());
    // A is back to asking.
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert!(picker_open(&page, vcx));
}

#[gpui::test]
fn the_pill_can_be_switched_off_and_the_target_stays(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    page.settings.update(vcx, |s, cx| s.set(&TARGET_PILL, false, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("target-pill").is_none());
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimroad".to_owned(), PLAYING.to_owned())]);
}

#[gpui::test]
fn a_new_name_in_the_target_picker_makes_a_playlist_and_targets_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    let field = page.picker.read_with(vcx, |p, _| p.field().clone());
    field.update(vcx, |f, cx| f.set_text("Gym", cx));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(!picker_open(&page, vcx));
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), [PLAYING], "the song that asked is in it");
    assert_eq!(
        target_of(&page, vcx),
        Some(Target {
            id: made,
            name: "Gym".into()
        })
    );
}

// ---- where the target lives ---------------------------------------------------------------------

#[gpui::test]
fn the_target_belongs_to_its_account(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.target
        .update(vcx, |m, cx| m.set_account(Some("acct-1".into()), cx));
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    assert!(target_of(&page, vcx).is_some());
    page.target
        .update(vcx, |m, cx| m.set_account(Some("acct-2".into()), cx));
    assert_eq!(target_of(&page, vcx), None, "another account's playlist is not a target here");
    page.target
        .update(vcx, |m, cx| m.set_account(Some("acct-1".into()), cx));
    assert_eq!(target_of(&page, vcx).map(|t| t.name), Some("Road Trip".to_owned()));
}

#[gpui::test]
fn deleting_the_target_playlist_clears_it_and_renaming_follows(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    let edits = page.shell.read_with(vcx, |s, _| s.edits().clone());
    let collection = |title: &str| apollo_domain::library::Collection {
        kind: apollo_domain::library::CollectionKind::Playlist,
        id: apollo_domain::model::BrowseId::new("VLPLsimroad"),
        title: title.to_owned(),
        subtitle: None,
        thumbnail: None,
    };
    edits.update(vcx, |_, cx| {
        cx.emit(apollo_app::library::edits::EditEvent::Renamed(collection("Highway")))
    });
    vcx.run_until_parked();
    assert_eq!(target_of(&page, vcx).map(|t| t.name), Some("Highway".to_owned()));
    edits.update(vcx, |_, cx| {
        cx.emit(apollo_app::library::edits::EditEvent::Deleted(collection("Highway")))
    });
    vcx.run_until_parked();
    assert_eq!(target_of(&page, vcx), None);
}

// ---- lists --------------------------------------------------------------------------------------

fn search_for(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Search, window, cx);
    });
    let search = page.shell.read_with(vcx, |s, _| s.search_model().clone());
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text("anything", cx));
    search.update(vcx, |m, cx| m.submit(cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn on_search_results_a_adds_the_cursor_row_not_the_playing_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimroad".to_owned(), NEXT.to_owned())]);
}

#[gpui::test]
fn on_search_results_a_adds_every_picked_song_in_one_request(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    search_for(&page, vcx);
    vcx.simulate_keystrokes("down shift-down a");
    vcx.run_until_parked();
    assert_eq!(
        added(&page),
        [
            ("PLsimroad".to_owned(), NEXT.to_owned()),
            ("PLsimroad".to_owned(), OTHER.to_owned())
        ]
    );
    assert_eq!(
        page.sim.write_log(),
        vec![("add-many:PLsimroad".to_owned(), format!("{NEXT},{OTHER}"))]
    );
}

#[gpui::test]
fn on_a_library_list_a_adds_the_cursor_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimempty", "Nothing Yet");
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
    let first = model.read_with(vcx, |m, _| {
        m.list_rows()[0].track.video_id.as_str().to_owned()
    });
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimempty".to_owned(), first)]);
}

fn focus_panel<V: gpui::Focusable + 'static>(vcx: &mut VisualTestContext, panel: &Entity<V>) {
    vcx.update(|window, cx| {
        let focus = panel.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
}

#[gpui::test]
fn the_crate_adds_its_cursor_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    let crate_model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    crate_model.update(vcx, |m, cx| m.add(Track::new("crateAAAA01", "In the crate"), None, cx));
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    focus_panel(vcx, &crate_page);
    vcx.simulate_keystrokes("down a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimroad".to_owned(), "crateAAAA01".to_owned())]);
}

#[gpui::test]
fn the_queue_adds_its_cursor_row_not_the_playing_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    focus_panel(vcx, &panel);
    // The first press shows the cursor on the playing row; one more reaches the song up next.
    vcx.simulate_keystrokes("down down a");
    vcx.run_until_parked();
    assert_eq!(added(&page), [("PLsimroad".to_owned(), NEXT.to_owned())]);
}
