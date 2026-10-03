//! Headless tests for the track menu: right-click and the More button open it on every kind of
//! row and card, the keyboard works it, and each command does what it says. The real shell and
//! views on GPUI's test platform, the simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::Tab;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::playlist_picker::PlaylistPicker;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{Command, Subject, TrackMenu};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::LIKED_MUSIC_BROWSE_ID;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_provider::{Hit, ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext,
};

/// The track playing at the start (Liked Music in the simulation).
const FIRST: &str = "simTrack001";
/// A search result that knows its artist and its album.
const FOUND: &str = "foundTrk001";
const OTHER: &str = "foundTrk002";

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

/// Two songs and a playlist for any query.
struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        let song = |id: &str, title: &str| {
            let mut track = Track::new(id, title);
            track.artists = vec![ArtistRef {
                name: "Neon Arcade".into(),
                id: Some(BrowseId::new("UCsim_Neon Arcade")),
            }];
            track.album = Some(AlbumRef {
                title: "Night Drive".into(),
                id: Some(BrowseId::new("MPREbsimnight")),
            });
            ListedTrack {
                track,
                occurrence: None,
                thumbnail: None,
                explicit: false,
                liked: None,
            }
        };
        Ok(SearchResults {
            songs: vec![song(FOUND, "Found One"), song(OTHER, "Found Two")],
            playlists: vec![Hit {
                id: BrowseId::new("VLPLsimroad"),
                title: "Road Trip".into(),
                subtitle: None,
                thumbnail: None,
                explicit: false,
            }],
            artists: vec![Hit {
                id: BrowseId::new("UCsim_Neon Arcade"),
                title: "Neon Arcade".into(),
                subtitle: None,
                thumbnail: None,
                explicit: false,
            }],
            ..SearchResults::default()
        })
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    menu: Entity<TrackMenu>,
    picker: Entity<PlaylistPicker>,
    sim: SimLibrary,
}

fn prepared(sim: &SimLibrary) -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![
            LibraryTrack {
                track: Track::new(FIRST, "Midnight Drive"),
            },
            LibraryTrack {
                track: Track::new("queued00001", "Queued One"),
            },
            LibraryTrack {
                track: Track::new("queued00002", "Queued Two"),
            },
        ],
        art: Arc::new(TestArt),
        search: Arc::new(TestSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    }
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
    let ready = prepared(&sim);
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let (menu, picker) = shell.read_with(vcx, |s, _| (s.menu().clone(), s.picker().clone()));
    (
        Page {
            player,
            shell,
            menu,
            picker,
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
}

fn right_click(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
    vcx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
    vcx.run_until_parked();
}

fn hover(vcx: &mut VisualTestContext, at: Point<Pixels>) {
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.executor()
        .advance_clock(std::time::Duration::from_millis(400));
    vcx.run_until_parked();
}

fn is_open(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.menu.read_with(vcx, |m, _| m.is_open())
}

fn commands(page: &Page, vcx: &mut VisualTestContext) -> Vec<Command> {
    page.menu.read_with(vcx, |m, cx| m.commands(cx))
}

fn upcoming(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.player.read_with(vcx, |m, _| {
        m.queue_view()
            .upcoming
            .iter()
            .map(|r| r.title.to_string())
            .collect()
    })
}

fn current(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player.read_with(vcx, |m, _| m.view().title.to_string())
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.writes().read(cx).notice().map(|n| n.text.to_string())
    })
}

/// Search for anything: two songs, a playlist and an artist.
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

/// A point in the row whose like slot is `selector`, on its title side, clear of every action.
fn inside_row(vcx: &mut VisualTestContext, selector: &str) -> Point<Pixels> {
    let slot = bounds(vcx, selector).center();
    point(slot.x - gpui::px(400.0), slot.y)
}

/// Right-click the first search song.
fn open_on_first_song(page: &Page, vcx: &mut VisualTestContext) {
    search(page, vcx);
    let at = inside_row(vcx, "song-like-0");
    right_click(vcx, at);
}

fn choose(page: &Page, vcx: &mut VisualTestContext, command: Command) {
    let ix = commands(page, vcx)
        .iter()
        .position(|c| *c == command)
        .unwrap_or_else(|| panic!("{command:?} is in the menu"));
    click(vcx, &format!("track-menu-item-{ix}"));
    vcx.run_until_parked();
}

// ---- opening and closing ---------------------------------------------------------------------

#[gpui::test]
fn right_clicking_a_song_opens_its_menu(cx: &mut TestAppContext) {
    use Command::*;
    let (page, vcx) = setup(cx);
    assert!(!is_open(&page, vcx));
    open_on_first_song(&page, vcx);
    assert!(is_open(&page, vcx));
    assert!(on_screen(vcx, "track-menu"));
    assert_eq!(
        commands(&page, vcx),
        [
            PlayNext,
            AddToQueue,
            StartRadio,
            AddToPlaylist,
            ToggleLike,
            GoToArtist,
            GoToAlbum,
            CopyLink
        ]
    );
    let subject = page.menu.read_with(vcx, |m, _| m.subject().cloned());
    assert!(matches!(subject, Some(Subject::Track(t)) if t.track.video_id.as_str() == FOUND));
    // It opened at the pointer.
    let menu = bounds(vcx, "track-menu");
    assert_eq!(menu.origin, inside_row(vcx, "song-like-0"));
}

#[gpui::test]
fn escape_and_a_click_outside_close_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    vcx.simulate_keystrokes("escape");
    assert!(!is_open(&page, vcx));
    assert!(!on_screen(vcx, "track-menu"));

    let at = inside_row(vcx, "song-like-0");
    right_click(vcx, at);
    assert!(is_open(&page, vcx));
    // Anywhere outside the menu, with no scrim: the page behind stays as it is.
    let size = vcx.update(|window, _| window.viewport_size());
    vcx.simulate_click(
        point(size.width - gpui::px(20.0), size.height / 2.0),
        Modifiers::none(),
    );
    assert!(!is_open(&page, vcx));
    // A right-click outside closes it too.
    right_click(vcx, at);
    assert!(is_open(&page, vcx));
    right_click(vcx, point(size.width - gpui::px(20.0), size.height / 2.0));
    assert!(!is_open(&page, vcx));
}

#[gpui::test]
fn the_more_button_opens_the_menu_under_itself_with_its_right_edge_aligned(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    let slot = bounds(vcx, "song-more-0");
    hover(vcx, slot.center());
    click(vcx, "song-more-0");
    assert!(is_open(&page, vcx));
    let menu = bounds(vcx, "track-menu");
    assert_eq!(menu.origin.x + menu.size.width, slot.origin.x + slot.size.width);
    assert!(menu.origin.y >= slot.origin.y + slot.size.height);
}

#[gpui::test]
fn the_menu_flips_to_stay_inside_the_window(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    let size = vcx.update(|window, _| window.viewport_size());
    // On a row, close to the window's right edge (the like slot): the menu opens to the left
    // of the pointer instead of running off the window.
    let at = bounds(vcx, "song-like-0").center();
    assert!(at.x + gpui::px(220.0) > size.width, "no room to the right");
    right_click(vcx, at);
    let menu = bounds(vcx, "track-menu");
    assert!(is_open(&page, vcx));
    assert_eq!(menu.origin.x + menu.size.width, at.x);
    assert!(menu.origin.x >= gpui::px(0.0));
    assert!(menu.origin.y + menu.size.height <= size.height);
}

// ---- commands ----------------------------------------------------------------------------------

#[gpui::test]
fn add_to_queue_and_play_next_put_the_song_at_the_end_and_at_the_front(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(upcoming(&page, vcx), ["Queued One", "Queued Two"]);
    open_on_first_song(&page, vcx);
    choose(&page, vcx, Command::AddToQueue);
    assert!(!is_open(&page, vcx), "choosing closes the menu");
    // Queued songs go ahead of the context the queue was built from.
    assert_eq!(upcoming(&page, vcx), ["Found One", "Queued One", "Queued Two"]);
    assert_eq!(current(&page, vcx), "Midnight Drive", "queueing does not interrupt");

    let at = inside_row(vcx, "song-like-1");
    right_click(vcx, at);
    choose(&page, vcx, Command::PlayNext);
    assert_eq!(
        upcoming(&page, vcx),
        ["Found Two", "Found One", "Queued One", "Queued Two"]
    );
}

#[gpui::test]
fn start_radio_plays_the_song_and_the_radio_replaces_what_was_up_next(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    choose(&page, vcx, Command::StartRadio);
    assert_eq!(current(&page, vcx), "Found One");
    assert_eq!(page.sim.radio_seeds(), [FOUND]);
    let up = upcoming(&page, vcx);
    assert!(!up.is_empty(), "the radio is up next");
    assert!(!up.contains(&"Queued One".to_owned()), "the old up-next is gone");
    assert!(!up.contains(&"Found One".to_owned()), "the seed is not repeated");
    assert_eq!(notice(&page, vcx), None, "success is silence");
}

#[gpui::test]
fn a_radio_that_cannot_be_fetched_leaves_the_queue_and_an_error(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_radio_failure(Some(LoadFailure::Network));
    open_on_first_song(&page, vcx);
    choose(&page, vcx, Command::StartRadio);
    assert_eq!(current(&page, vcx), "Found One", "the song still plays");
    assert_eq!(upcoming(&page, vcx), ["Queued One", "Queued Two"]);
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't start radio"));
}

#[gpui::test]
fn like_and_remove_from_liked_toggle_the_heart(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    page.menu.read_with(vcx, |m, cx| {
        assert_eq!(m.commands(cx)[4], Command::ToggleLike);
    });
    choose(&page, vcx, Command::ToggleLike);
    assert_eq!(page.sim.write_log(), [("like".to_owned(), FOUND.to_owned())]);
    // Now liked: the same row's menu offers to take it back.
    let at = inside_row(vcx, "song-like-0");
    right_click(vcx, at);
    choose(&page, vcx, Command::ToggleLike);
    assert_eq!(
        page.sim.write_log(),
        [
            ("like".to_owned(), FOUND.to_owned()),
            ("unlike".to_owned(), FOUND.to_owned())
        ]
    );
}

#[gpui::test]
fn copy_link_puts_the_watch_url_on_the_clipboard(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    choose(&page, vcx, Command::CopyLink);
    let copied = vcx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("something was copied");
    assert_eq!(copied, "https://music.youtube.com/watch?v=foundTrk001");
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn add_to_playlist_opens_the_picker_where_the_menu_was(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    let menu = bounds(vcx, "track-menu");
    choose(&page, vcx, Command::AddToPlaylist);
    assert!(!is_open(&page, vcx));
    assert_eq!(
        page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)),
        Some(FOUND.to_owned())
    );
    let panel = bounds(vcx, "playlist-picker");
    assert_eq!(panel.origin.x, menu.origin.x);
}

#[gpui::test]
fn go_to_artist_and_go_to_album_open_their_pages(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    choose(&page, vcx, Command::GoToArtist);
    assert_eq!(page.shell.read_with(vcx, |s, _| s.route()), Route::Library);
    let view = page.shell.read_with(vcx, |s, _| s.library().clone());
    let artist = view.read_with(vcx, |v, cx| {
        v.model().read(cx).artist().map(|a| a.name().to_owned())
    });
    assert_eq!(artist.as_deref(), Some("Neon Arcade"));

    // Back to the search results, then the album.
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Search, window, cx);
    });
    vcx.run_until_parked();
    let at = inside_row(vcx, "song-like-0");
    right_click(vcx, at);
    choose(&page, vcx, Command::GoToAlbum);
    let detail = view.read_with(vcx, |v, cx| {
        v.model().read(cx).detail().map(|d| d.collection.title.clone())
    });
    assert_eq!(detail.as_deref(), Some("Night Drive"));
}

// ---- keyboard ----------------------------------------------------------------------------------

#[gpui::test]
fn arrows_move_and_enter_chooses(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    assert_eq!(page.menu.read_with(vcx, |m, _| m.cursor()), Some(0));
    vcx.simulate_keystrokes("down");
    assert_eq!(page.menu.read_with(vcx, |m, _| m.cursor()), Some(1));
    vcx.simulate_keystrokes("enter");
    assert!(!is_open(&page, vcx));
    // Item 1 is Add to queue.
    assert_eq!(upcoming(&page, vcx), ["Found One", "Queued One", "Queued Two"]);
}

#[gpui::test]
fn the_cursor_stops_at_both_ends(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_on_first_song(&page, vcx);
    vcx.simulate_keystrokes("up");
    assert_eq!(page.menu.read_with(vcx, |m, _| m.cursor()), Some(0));
    vcx.simulate_keystrokes("end");
    let last = commands(&page, vcx).len() - 1;
    assert_eq!(page.menu.read_with(vcx, |m, _| m.cursor()), Some(last));
    vcx.simulate_keystrokes("down");
    assert_eq!(page.menu.read_with(vcx, |m, _| m.cursor()), Some(last));
    vcx.simulate_keystrokes("home");
    assert_eq!(page.menu.read_with(vcx, |m, _| m.cursor()), Some(0));
}

// ---- other surfaces ---------------------------------------------------------------------------

#[gpui::test]
fn library_rows_have_the_menu_and_the_more_button(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let view = page.shell.read_with(vcx, |s, _| s.library().clone());
    let liked = view.read_with(vcx, |v, cx| {
        let model = v.model().read(cx);
        [Tab::Playlists, Tab::Albums]
            .into_iter()
            .flat_map(|t| model.list(t).iter())
            .find(|c| c.id.as_str() == LIKED_MUSIC_BROWSE_ID)
            .cloned()
            .expect("Liked Music is listed")
    });
    // The list of collections: a right-click on a row offers the shorter menu.
    let at = bounds(vcx, "library-row-0").center();
    right_click(vcx, at);
    assert!(is_open(&page, vcx));
    assert_eq!(
        commands(&page, vcx),
        [
            Command::Play,
            Command::Shuffle,
            Command::AddToQueue,
            Command::StartRadio,
            Command::CopyLink
        ]
    );
    vcx.simulate_keystrokes("escape");

    view.update_in(vcx, |v, window, cx| v.open(liked, window, cx));
    vcx.run_until_parked();
    let slot = bounds(vcx, "track-more-1");
    hover(vcx, slot.center());
    click(vcx, "track-more-1");
    assert!(is_open(&page, vcx));
    assert!(commands(&page, vcx).contains(&Command::StartRadio));
}

#[gpui::test]
fn a_playlist_card_gets_the_shorter_menu_and_plays(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    let card = bounds(vcx, "search-playlist-0");
    right_click(vcx, card.center());
    assert_eq!(
        commands(&page, vcx),
        [
            Command::Play,
            Command::Shuffle,
            Command::AddToQueue,
            Command::StartRadio,
            Command::CopyLink
        ]
    );
    choose(&page, vcx, Command::AddToQueue);
    // The playlist's three songs join the queue.
    assert_eq!(upcoming(&page, vcx).len(), 2 + 3);

    right_click(vcx, card.center());
    choose(&page, vcx, Command::Play);
    // The queue is the playlist now: its first song plays and the other two follow.
    assert_eq!(upcoming(&page, vcx).len(), 2);
    assert!(!upcoming(&page, vcx).contains(&"Queued One".to_owned()));

    right_click(vcx, card.center());
    choose(&page, vcx, Command::CopyLink);
    let copied = vcx.read_from_clipboard().and_then(|i| i.text()).unwrap();
    assert_eq!(copied, "https://music.youtube.com/playlist?list=PLsimroad");
}

#[gpui::test]
fn an_artist_card_has_no_menu(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    let card = bounds(vcx, "search-artist-0");
    right_click(vcx, card.center());
    assert!(!is_open(&page, vcx));
}

#[gpui::test]
fn queue_rows_offer_remove_from_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    // Row 0 is the playing one, 1 is the "Up next" header, 2 and 3 are up next.
    let at = bounds(vcx, "queue-row-2").center();
    right_click(vcx, at);
    assert!(is_open(&page, vcx));
    let list = commands(&page, vcx);
    assert_eq!(list.last(), Some(&Command::RemoveFromQueue));
    choose(&page, vcx, Command::RemoveFromQueue);
    assert_eq!(upcoming(&page, vcx), ["Queued Two"]);

    // The playing row has no remove.
    let at = bounds(vcx, "queue-row-0").center();
    right_click(vcx, at);
    assert!(is_open(&page, vcx));
    assert!(!commands(&page, vcx).contains(&Command::RemoveFromQueue));
}
