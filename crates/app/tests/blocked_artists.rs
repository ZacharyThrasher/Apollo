//! Blocked artists, headless: radio and autoplay leave a blocked artist out, Settings counts the
//! list and opens the Manage dialog, the dialog unblocks, and the artist page's More menu blocks.
//! The real shell and views on GPUI's test platform, the simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::blocked_artists::{BlockHandle, BlockModel};
use apollo_app::library::model::{ArtistTarget, Target};
use apollo_app::library::sim::{self, SimLibrary};
use apollo_app::library::source::LibrarySource;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::blocked_dialog::BlockedDialog;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::blocklist::BlockedArtist;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

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

fn prepared(library: &SimLibrary) -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack {
            track: Track::new("startTrack01", "Start"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(library.clone()),
    }
}

fn by_name(name: &str) -> BlockedArtist {
    BlockedArtist {
        id: None,
        name: name.into(),
    }
}

// ---- radio and autoplay ---------------------------------------------------------------------------

/// A player over the simulated library with a blocklist installed as the shell's would be.
fn player(cx: &mut TestAppContext, blocked: &[&str]) -> (Entity<PlayerModel>, SimLibrary) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let model = cx.new(|_| BlockModel::new());
    for name in blocked {
        model.update(cx, |m, cx| m.block(by_name(name), cx));
    }
    cx.update(|cx| cx.set_global(BlockHandle(model)));
    let library = SimLibrary::instant();
    let player = cx.new(|_| PlayerModel::new());
    let ready = prepared(&library);
    player.update(cx, |m, cx| m.install(ready, cx));
    (player, library)
}

/// The artists of everything up next.
fn upcoming_artists(player: &Entity<PlayerModel>, cx: &mut TestAppContext) -> Vec<String> {
    player.read_with(cx, |m, _| {
        m.coordinator()
            .unwrap()
            .queue()
            .upcoming()
            .flat_map(|e| e.track.artists.iter().map(|a| a.name.clone()))
            .collect()
    })
}

/// An artist the simulated radio's first batch contains, and how many of its ten songs are theirs.
fn artist_in_first_radio() -> (String, usize) {
    let rows = SimLibrary::instant().radio("peek").unwrap();
    let name = rows[3].track.artists[0].name.clone();
    let theirs = rows
        .iter()
        .filter(|r| r.track.artists.iter().any(|a| a.name == name))
        .count();
    (name, theirs)
}

#[gpui::test]
fn autoplay_leaves_a_blocked_artist_out(cx: &mut TestAppContext) {
    let (name, theirs) = artist_in_first_radio();
    let (player, _library) = player(cx, &[&name.to_lowercase()]);
    let tracks = vec![Track::new("seedTrack01", "Seed")];
    player.update(cx, |m, cx| m.play_tracks(tracks, Some(0), Some(false), cx));
    cx.run_until_parked();
    let up = upcoming_artists(&player, cx);
    assert_eq!(up.len(), 10 - theirs, "{up:?}");
    assert!(!up.contains(&name), "{name} is blocked: {up:?}");
}

#[gpui::test]
fn without_a_block_the_same_radio_has_their_songs(cx: &mut TestAppContext) {
    let (name, _) = artist_in_first_radio();
    let (player, _library) = player(cx, &[]);
    let tracks = vec![Track::new("seedTrack01", "Seed")];
    player.update(cx, |m, cx| m.play_tracks(tracks, Some(0), Some(false), cx));
    cx.run_until_parked();
    assert!(upcoming_artists(&player, cx).contains(&name));
}

#[gpui::test]
fn start_radio_leaves_a_blocked_artist_out(cx: &mut TestAppContext) {
    let (name, theirs) = artist_in_first_radio();
    let (player, _library) = player(cx, &[&name]);
    player.update(cx, |m, cx| m.start_radio(Track::new("seedTrack01", "Seed"), cx));
    cx.run_until_parked();
    let up = upcoming_artists(&player, cx);
    assert!(!up.contains(&name), "{up:?}");
    assert!(up.len() >= 10 - theirs);
}

// ---- Settings, the dialog and the artist page -----------------------------------------------------

struct Page {
    shell: Entity<Shell>,
    dialog: Entity<BlockedDialog>,
    blocked: Entity<BlockModel>,
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
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let (dialog, blocked) = shell.read_with(vcx, |s, _| (s.blocked_dialog().clone(), s.block_model().clone()));
    (Page { shell, dialog, blocked }, vcx)
}

fn on_screen(vcx: &mut VisualTestContext, id: &'static str) -> bool {
    vcx.debug_bounds(id).is_some()
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn block(page: &Page, vcx: &mut VisualTestContext, name: &str) {
    page.blocked.update(vcx, |m, cx| m.block(by_name(name), cx));
    vcx.run_until_parked();
}

fn open_settings(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn the_settings_row_counts_the_blocked_artists_and_manage_opens_the_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_settings(&page, vcx);
    assert!(on_screen(vcx, "setting-blocked-artists"));
    assert!(on_screen(vcx, "setting-blocked-artists-manage"));
    assert!(!page.dialog.read_with(vcx, |d, _| d.is_open()));

    block(&page, vcx, "Neon Arcade");
    block(&page, vcx, "Night Owls");
    click(vcx, "setting-blocked-artists-manage");
    assert!(page.dialog.read_with(vcx, |d, _| d.is_open()));
    assert!(on_screen(vcx, "blocked-dialog"));
    assert!(on_screen(vcx, "blocked-row-0") && on_screen(vcx, "blocked-row-1"));
    let rows = page.dialog.read_with(vcx, |d, cx| d.rows(cx));
    assert_eq!(rows[0].name, "Night Owls", "newest first");
}

#[gpui::test]
fn unblock_takes_the_row_away_and_done_or_escape_closes(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_settings(&page, vcx);
    block(&page, vcx, "Neon Arcade");
    click(vcx, "setting-blocked-artists-manage");

    click(vcx, "blocked-unblock-0");
    assert!(page.blocked.read_with(vcx, |m, _| m.is_empty()));
    assert!(on_screen(vcx, "blocked-dialog-empty"), "an empty list says so");
    assert!(page.dialog.read_with(vcx, |d, _| d.is_open()), "the dialog stays open");

    click(vcx, "blocked-done");
    assert!(!page.dialog.read_with(vcx, |d, _| d.is_open()));

    click(vcx, "setting-blocked-artists-manage");
    assert!(page.dialog.read_with(vcx, |d, _| d.is_open()));
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!page.dialog.read_with(vcx, |d, _| d.is_open()));
}

#[gpui::test]
fn forgetting_the_account_empties_the_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    block(&page, vcx, "Neon Arcade");
    page.blocked.update(vcx, |m, cx| m.forget(cx));
    assert!(page.blocked.read_with(vcx, |m, _| m.is_empty()));
}

#[gpui::test]
fn the_artist_pages_more_menu_blocks_and_unblocks_the_artist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let view = page.shell.read_with(vcx, |s, _| s.library().clone());
    let target = Target::Artist(ArtistTarget {
        id: BrowseId::new(format!("{}Neon Arcade", sim::SEARCH_ARTIST_PREFIX)),
        name: "Neon Arcade".into(),
        thumbnail: None,
    });
    view.update_in(vcx, |v, window, cx| v.open_target(target, window, cx));
    vcx.run_until_parked();

    click(vcx, "artist-more");
    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    let label = |vcx: &mut VisualTestContext| {
        menu.read_with(vcx, |m, cx| m.labels(cx).into_iter().map(|(_, l)| l).collect::<Vec<_>>())
    };
    assert_eq!(label(vcx), ["Don't play this artist"]);
    click(vcx, "track-menu-item-0");
    assert_eq!(
        page.blocked.read_with(vcx, |m, _| m.list().entries().iter().map(|a| a.name.clone()).collect::<Vec<_>>()),
        ["Neon Arcade"]
    );

    click(vcx, "artist-more");
    assert_eq!(label(vcx), ["Play this artist again"]);
    click(vcx, "track-menu-item-0");
    assert!(page.blocked.read_with(vcx, |m, _| m.is_empty()));
}
