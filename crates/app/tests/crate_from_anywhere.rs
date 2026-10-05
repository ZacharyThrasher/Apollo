//! C from anywhere: C acts on the song card the keyboard ring or the pointer is on (Home,
//! Explore and Related shelves), and "Add all to Crate" / "Open in Crate" take what a page has
//! loaded in one step. The real shell and views on GPUI's test platform, the simulated library
//! as the "service".

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::model::{LibraryModel, Target, Tab};
use apollo_app::library::sim::{self, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Command, TrackMenu};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::HomeItem;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{point, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};
use apollo_app::ui::home_page::HomeView;
use apollo_app::ui::library::LibraryView;

const PLAYING: &str = "simTrack001";
const QUEUED: [&str; 2] = ["queued00001", "queued00002"];

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

struct Page {
    shell: Entity<Shell>,
    player: Entity<PlayerModel>,
    crate_model: Entity<CrateModel>,
    menu: Entity<TrackMenu>,
    home: Entity<HomeView>,
    library: Entity<LibraryView>,
    model: Entity<LibraryModel>,
    sim: SimLibrary,
}

const UNDO: &str = " \u{b7} Ctrl+Z to undo";

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
        search: Arc::new(apollo_app::search::Unavailable),
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
    let (crate_model, menu, home, library) = shell.read_with(vcx, |s, _| {
        (
            s.crate_model().clone(),
            s.menu().clone(),
            s.home_page().clone(),
            s.library().clone(),
        )
    });
    let model = library.read_with(vcx, |v, _| v.model().clone());
    (
        Page {
            shell,
            player,
            crate_model,
            menu,
            home,
            library,
            model,
            sim,
        },
        vcx,
    )
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

fn settle(vcx: &mut VisualTestContext) {
    vcx.executor().advance_clock(Duration::from_secs(4));
    vcx.run_until_parked();
}

fn leak(id: String) -> &'static str {
    Box::leak(id.into_boxed_str())
}

fn bounds_of(vcx: &mut VisualTestContext, id: &str) -> gpui::Bounds<gpui::Pixels> {
    vcx.debug_bounds(leak(id.to_owned()))
        .unwrap_or_else(|| panic!("{id} is on screen"))
}

fn click(vcx: &mut VisualTestContext, id: &str) {
    let at = bounds_of(vcx, id).center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn hover(vcx: &mut VisualTestContext, id: &str) {
    let at = bounds_of(vcx, id).center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.executor().advance_clock(Duration::from_millis(400));
    vcx.run_until_parked();
}

/// Choose `command` from the menu that is open.
fn choose(page: &Page, vcx: &mut VisualTestContext, command: Command) {
    let commands = page.menu.read_with(vcx, |m, cx| m.commands(cx));
    let ix = commands
        .iter()
        .position(|c| *c == command)
        .unwrap_or_else(|| panic!("{command:?} is in the menu {commands:?}"));
    click(vcx, &format!("track-menu-item-{ix}"));
}

fn menu_commands(page: &Page, vcx: &mut VisualTestContext) -> Vec<Command> {
    page.menu.read_with(vcx, |m, cx| m.commands(cx))
}

fn menu_labels(page: &Page, vcx: &mut VisualTestContext) -> Vec<&'static str> {
    page.menu
        .read_with(vcx, |m, cx| m.labels(cx).into_iter().map(|(_, l)| l).collect())
}

/// The id of song `j` of Home shelf `shelf`.
fn home_song(page: &Page, vcx: &mut VisualTestContext, shelf: usize, j: usize) -> String {
    let home = page.shell.read_with(vcx, |s, _| s.home().clone());
    home.read_with(vcx, |m, _| match &m.shelves()[shelf].items[j] {
        HomeItem::Song(song) => song.track.video_id.as_str().to_owned(),
        other => panic!("not a song: {other:?}"),
    })
}

fn focus_home_stop(page: &Page, vcx: &mut VisualTestContext, stop: usize) {
    let handle = page.home.read_with(vcx, |v, _| v.item_stops()[stop].clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    vcx.run_until_parked();
}

fn playlist_card(id: &str, title: &str) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: None,
        thumbnail: None,
    }
}

fn open_page(page: &Page, vcx: &mut VisualTestContext, collection: Collection) {
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    page.library.update_in(vcx, |v, window, cx| {
        v.open_target(Target::Collection(collection), window, cx)
    });
    vcx.run_until_parked();
}

fn page_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.model.read_with(vcx, |m, _| {
        m.detail()
            .expect("a page is open")
            .listing
            .items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn undo(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.undo(cx));
    vcx.run_until_parked();
}

// ---- shelves ----------------------------------------------------------------------------------

#[gpui::test]
fn c_on_a_focused_home_song_sets_that_song_aside_not_the_playing_one(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let wanted = home_song(&page, vcx, 0, 2);
    focus_home_stop(&page, vcx, 2);
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [wanted.clone()]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Crate \u{b7} Ctrl+Z to undo"));
    // The same song again says so, and a different card takes its own song.
    settle(vcx);
    vcx.simulate_keystrokes("c");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Already in Crate"));
    let other = home_song(&page, vcx, 0, 4);
    focus_home_stop(&page, vcx, 4);
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [wanted, other]);
    assert!(!crate_ids(&page, vcx).contains(&PLAYING.to_owned()));
}

#[gpui::test]
fn c_on_the_song_under_the_pointer_when_no_card_holds_the_keyboard(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let wanted = home_song(&page, vcx, 0, 1);
    hover(vcx, "home-0-1");
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [wanted]);
}

#[gpui::test]
fn c_with_nothing_under_the_pointer_still_sets_the_playing_song_aside(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [PLAYING.to_owned()]);
}

#[gpui::test]
fn once_the_pointer_leaves_a_card_c_goes_back_to_the_playing_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    hover(vcx, "home-0-1");
    // Off every card (the title bar's strip).
    vcx.simulate_mouse_move(point(gpui::px(2.0), gpui::px(2.0)), None, Modifiers::none());
    vcx.run_until_parked();
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [PLAYING.to_owned()]);
}

#[gpui::test]
fn the_pointer_on_a_home_card_is_forgotten_when_the_page_changes(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    hover(vcx, "home-0-1");
    page.shell.update(vcx, |s, cx| s.show_route(Route::Explore, cx));
    vcx.run_until_parked();
    // Explore shows its own cards, none under the pointer's old place: C is the playing song's.
    vcx.simulate_mouse_move(point(gpui::px(2.0), gpui::px(2.0)), None, Modifiers::none());
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [PLAYING.to_owned()]);
}

#[gpui::test]
fn adding_from_a_shelf_asks_the_service_for_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let (home_before, related_before) = (page.sim.home_log().len(), page.sim.related_log().len());
    let writes = page.sim.write_log().len();
    hover(vcx, "home-0-1");
    vcx.simulate_keystrokes("c");
    focus_home_stop(&page, vcx, 3);
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx).len(), 2);
    assert_eq!(page.sim.home_log().len(), home_before);
    assert_eq!(page.sim.related_log().len(), related_before);
    assert_eq!(page.sim.write_log().len(), writes);
}

#[gpui::test]
fn c_on_a_focused_explore_chart_song_sets_that_song_aside(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(2800.0)));
    page.shell.update(vcx, |s, cx| s.show_route(Route::Explore, cx));
    vcx.run_until_parked();
    let explore = page.shell.read_with(vcx, |s, _| s.explore().clone());
    let wanted = explore.read_with(vcx, |m, _| {
        m.page().unwrap().top_songs[2].track.video_id.as_str().to_owned()
    });
    let view = page.shell.read_with(vcx, |s, _| s.explore_page().clone());
    let first_chart_stop = apollo_app::backend::sim::FIXTURES.len();
    let handle = view.read_with(vcx, |v, _| v.item_stops()[first_chart_stop + 2].clone());
    vcx.update(|window, cx| window.focus(&handle, cx));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [wanted]);
}

#[gpui::test]
fn c_on_a_hovered_related_song_sets_that_song_aside(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell
        .update(vcx, |s, cx| s.show_pane(apollo_app::ui::pane_tabs::Pane::Related, cx));
    for _ in 0..4 {
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
    }
    let related = page.shell.read_with(vcx, |s, _| s.related().clone());
    let wanted = related.read_with(vcx, |m, _| {
        m.shelves()[0].songs()[0].track.video_id.as_str().to_owned()
    });
    hover(vcx, "related-0-0");
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [wanted]);
}

// ---- Add all to Crate -------------------------------------------------------------------------

#[gpui::test]
fn a_playlist_pages_more_menu_adds_every_loaded_song_in_one_undoable_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_page(&page, vcx, playlist_card("VLPLsimroad", "Road Trip"));
    let ids = page_ids(&page, vcx);
    assert!(ids.len() >= 3, "the Road Trip page has songs");

    click(vcx, "detail-more");
    let commands = menu_commands(&page, vcx);
    let add = commands.iter().position(|c| *c == Command::AddAllToCrate).unwrap();
    assert_eq!(commands[add + 1], Command::OpenInCrate, "Open in Crate follows, on a playlist");
    assert_eq!(menu_labels(&page, vcx)[add], "Add all to Crate");
    choose(&page, vcx, Command::AddAllToCrate);

    assert_eq!(crate_ids(&page, vcx), ids, "in page order");
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some(format!("Added {} songs to Crate{UNDO}", ids.len()).as_str())
    );
    assert_eq!(page.shell.read_with(vcx, |s, _| s.route()), Route::Library, "no page change");

    // One Ctrl+Z takes the whole add back.
    undo(&page, vcx);
    assert!(crate_ids(&page, vcx).is_empty());
    assert!(pill(&page, vcx).unwrap().starts_with("Undid: added"));
}

#[gpui::test]
fn songs_the_crate_holds_are_skipped_and_the_pill_says_how_many(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_page(&page, vcx, playlist_card("VLPLsimroad", "Road Trip"));
    let ids = page_ids(&page, vcx);
    page.crate_model.update(vcx, |m, cx| {
        m.add(Track::new(ids[1].as_str(), "held"), None, cx);
    });
    click(vcx, "detail-more");
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some(format!("Added {} \u{b7} 1 already in Crate{UNDO}", ids.len() - 1).as_str())
    );
    assert_eq!(crate_ids(&page, vcx).len(), ids.len());
    assert_eq!(crate_ids(&page, vcx)[0], ids[1], "what was held stays where it was");

    settle(vcx);
    click(vcx, "detail-more");
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Already in Crate"));
    // Nothing went in, so there is no step of this add to undo: Ctrl+Z takes back the last real one.
    undo(&page, vcx);
    assert_eq!(crate_ids(&page, vcx), [ids[1].clone()]);
}

#[gpui::test]
fn a_playlist_still_paging_in_adds_what_is_loaded_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let long = playlist_card(sim::LONG_ID, "Long");
    page.library
        .update_in(vcx, |v, window, cx| v.open_target(Target::Collection(long.clone()), window, cx));
    // Let the first page arrive, and stop before the rest do.
    let loaded = |page: &Page, vcx: &mut VisualTestContext| {
        page.model.read_with(vcx, |m, _| {
            m.detail().map_or((0, false), |d| (d.listing.items().len(), d.is_loading()))
        })
    };
    for _ in 0..500 {
        vcx.executor().tick();
        if loaded(&page, vcx).0 > 0 {
            break;
        }
    }
    let (loaded, loading) = loaded(&page, vcx);
    assert!(loaded > 0 && loaded < sim::LONG_LEN && loading, "{loaded} rows, loading: {loading}");

    // The menu is read now, with those rows.
    vcx.update(|window, cx| {
        apollo_app::ui::track_menu::open(
            apollo_app::ui::track_menu::Subject::Collection(long.clone()),
            apollo_app::ui::track_menu::Anchor::Point(point(gpui::px(300.0), gpui::px(300.0))),
            window,
            cx,
        )
    });
    let ix = menu_commands(&page, vcx)
        .iter()
        .position(|c| *c == Command::AddAllToCrate)
        .expect("Add all to Crate is offered while rows are loaded");
    page.menu.update_in(vcx, |m, window, cx| m.choose(ix, window, cx));
    assert_eq!(crate_ids(&page, vcx).len(), loaded);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some(format!("Added {loaded} songs to Crate \u{b7} still loading{UNDO}").as_str())
    );

    // Adding again later, once every page is in, takes the rest and skips what is held.
    vcx.run_until_parked();
    settle(vcx);
    click(vcx, "detail-more");
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(crate_ids(&page, vcx).len(), sim::LONG_LEN);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some(format!("Added {} \u{b7} {loaded} already in Crate{UNDO}", sim::LONG_LEN - loaded).as_str())
    );
}

#[gpui::test]
fn an_album_page_has_add_all_but_no_open_in_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let album = Collection {
        kind: CollectionKind::Album,
        id: BrowseId::new("MPREbsimnight"),
        title: "Night Drive".into(),
        subtitle: None,
        thumbnail: None,
    };
    open_page(&page, vcx, album);
    let ids = page_ids(&page, vcx);
    assert!(!ids.is_empty());
    click(vcx, "detail-more");
    let commands = menu_commands(&page, vcx);
    assert!(commands.contains(&Command::AddAllToCrate));
    assert!(!commands.contains(&Command::OpenInCrate));
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(crate_ids(&page, vcx), ids);
}

#[gpui::test]
fn a_collection_card_that_is_not_open_has_no_add_all(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // Library > Playlists lists cards; their rows are not loaded.
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    page.model.update(vcx, |m, cx| m.set_tab(Tab::Playlists, cx));
    vcx.run_until_parked();
    let at = bounds_of(vcx, "library-row-0").center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, gpui::MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, gpui::MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
    let commands = menu_commands(&page, vcx);
    assert!(!commands.contains(&Command::AddAllToCrate), "{commands:?}");
    assert!(!commands.contains(&Command::OpenInCrate));
}

#[gpui::test]
fn open_in_crate_appends_to_what_is_there_and_opens_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_page(&page, vcx, playlist_card("VLPLsimroad", "Road Trip"));
    let ids = page_ids(&page, vcx);
    page.crate_model.update(vcx, |m, cx| {
        m.add(Track::new("alreadyHere", "Already here"), None, cx);
    });
    click(vcx, "detail-more");
    choose(&page, vcx, Command::OpenInCrate);
    assert_eq!(page.shell.read_with(vcx, |s, _| s.route()), Route::Crate);
    let mut expected = vec!["alreadyHere".to_owned()];
    expected.extend(ids);
    assert_eq!(crate_ids(&page, vcx), expected, "appended, no dialog");
}

#[gpui::test]
fn the_songs_tab_has_a_more_menu_with_add_all_that_keeps_to_the_filter(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    click(vcx, "library-tab-songs");
    let total = page.model.read_with(vcx, |m, _| m.songs_kept().len());
    assert!(total > 3);
    click(vcx, "songs-more");
    assert_eq!(menu_commands(&page, vcx), [Command::AddAllToCrate]);
    assert_eq!(menu_labels(&page, vcx), ["Add all to Crate"]);
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(crate_ids(&page, vcx).len(), total);
    undo(&page, vcx);
    assert!(crate_ids(&page, vcx).is_empty());

    // With the filter box in use, only the rows it keeps.
    let titles: Vec<String> = page.model.read_with(vcx, |m, _| {
        m.songs_kept().iter().map(|r| r.track.title.clone()).collect()
    });
    let field = page
        .library
        .read_with(vcx, |v, cx| v.filter_field(cx))
        .expect("a filter field on the Songs tab");
    field.update(vcx, |f, cx| f.set_text(&titles[2].to_lowercase(), cx));
    vcx.run_until_parked();
    let kept = page.model.read_with(vcx, |m, _| m.songs_kept().len());
    assert!(kept >= 1 && kept < total);
    click(vcx, "songs-more");
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(crate_ids(&page, vcx).len(), kept);
}

#[gpui::test]
fn explores_top_songs_has_a_more_menu_with_add_all(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(2800.0)));
    page.shell.update(vcx, |s, cx| s.show_route(Route::Explore, cx));
    vcx.run_until_parked();
    let explore = page.shell.read_with(vcx, |s, _| s.explore().clone());
    let chart: Vec<String> = explore.read_with(vcx, |m, _| {
        m.page()
            .unwrap()
            .top_songs
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    });
    click(vcx, "explore-more");
    assert_eq!(menu_commands(&page, vcx), [Command::AddAllToCrate]);
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(crate_ids(&page, vcx), chart);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some(format!("Added {} songs to Crate{UNDO}", chart.len()).as_str())
    );
}

#[gpui::test]
fn the_queues_more_menu_adds_the_whole_queue_as_add_queue_to_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    click(vcx, "queue-more");
    assert_eq!(menu_commands(&page, vcx), [Command::SaveQueue, Command::AddAllToCrate]);
    assert_eq!(menu_labels(&page, vcx)[1], "Add queue to Crate");
    choose(&page, vcx, Command::AddAllToCrate);
    let mut expected = vec![PLAYING.to_owned()];
    expected.extend(QUEUED.iter().map(|s| (*s).to_owned()));
    assert_eq!(crate_ids(&page, vcx), expected);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added 3 songs to Crate \u{b7} Ctrl+Z to undo"));
    undo(&page, vcx);
    assert!(crate_ids(&page, vcx).is_empty(), "one Ctrl+Z for the whole queue");
}

#[gpui::test]
fn adding_everything_never_starts_playing_or_loading_anything(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_page(&page, vcx, playlist_card("VLPLsimroad", "Road Trip"));
    let (current, writes) = (
        page.player.read_with(vcx, |m, _| m.current_video()),
        page.sim.write_log().len(),
    );
    click(vcx, "detail-more");
    choose(&page, vcx, Command::AddAllToCrate);
    assert_eq!(page.player.read_with(vcx, |m, _| m.current_video()), current);
    assert_eq!(page.sim.write_log().len(), writes, "nothing written to the account");
}
