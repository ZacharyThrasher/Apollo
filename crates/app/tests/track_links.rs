//! The artist and album names under the title of the player bar and Now Playing open their pages
//! and leave the bar's own click (open Now Playing) alone.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::LibraryModel;
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

struct TestThumbs;
impl ThumbSource for TestThumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(100.0).render(32)))
    }
}

fn artist(name: &str, id: Option<&str>) -> ArtistRef {
    ArtistRef {
        name: name.into(),
        id: id.map(|i| BrowseId::new(i.to_owned())),
    }
}

fn start(
    cx: &mut TestAppContext,
    track: Track,
) -> (Entity<Shell>, &mut VisualTestContext) {
    let library = SimLibrary::instant();
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack { track }],
        art: Arc::new(TestArt),
        search: Arc::new(apollo_app::search::Unavailable),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(library),
    };
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
        cx.set_reduce_motion(true);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (shell, vcx)
}

fn song() -> Track {
    let mut t = Track::new("linkTrack001", "Take Your Mask Off");
    t.artists = vec![
        artist("Tyler, The Creator", Some("UCtyler")),
        artist("Daniel Caesar", Some("UCcaesar")),
    ];
    t.album = Some(AlbumRef {
        title: "CHROMAKOPIA".into(),
        id: Some(BrowseId::new("MPREbchroma")),
    });
    t
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn model(shell: &Entity<Shell>, vcx: &mut VisualTestContext) -> Entity<LibraryModel> {
    shell.read_with(vcx, |s, cx| s.library().read(cx).model().clone())
}

#[gpui::test]
fn an_artist_in_the_player_bar_opens_that_artists_page(cx: &mut TestAppContext) {
    let (shell, vcx) = start(cx, song());
    // Two artists, two links (0 and 2 are the names, 1 the comma).
    click(vcx, "artist-2");
    assert_eq!(shell.read_with(vcx, |s, _| s.route()), Route::Library);
    assert!(
        !shell.read_with(vcx, |s, _| s.now_playing_open()),
        "the click was the link's, not the block's"
    );
    let model = model(&shell, vcx);
    model.read_with(vcx, |m, _| {
        let a = m.artist().expect("the artist page is open");
        assert_eq!(a.target.name, "Daniel Caesar");
        assert_eq!(a.target.id.as_str(), "UCcaesar");
    });
}

#[gpui::test]
fn the_album_in_now_playing_opens_the_album_page(cx: &mut TestAppContext) {
    let (shell, vcx) = start(cx, song());
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    click(vcx, "np-artist-4");
    assert!(!shell.read_with(vcx, |s, _| s.now_playing_open()));
    let model = model(&shell, vcx);
    model.read_with(vcx, |m, _| {
        let d = m.detail().expect("the album page is open");
        assert_eq!(d.collection.title, "CHROMAKOPIA");
    });
}

#[gpui::test]
fn the_first_artist_in_now_playing_opens_the_artist_page(cx: &mut TestAppContext) {
    let (shell, vcx) = start(cx, song());
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    click(vcx, "np-artist-0");
    let model = model(&shell, vcx);
    model.read_with(vcx, |m, _| {
        assert_eq!(m.artist().expect("artist page").target.name, "Tyler, The Creator");
    });
}

#[gpui::test]
fn names_without_ids_are_plain_text(cx: &mut TestAppContext) {
    let mut t = Track::new("linkTrack002", "Plain");
    t.artists = vec![artist("Nobody", None)];
    t.album = Some(AlbumRef {
        title: "Nowhere".into(),
        id: None,
    });
    let (shell, vcx) = start(cx, t);
    assert!(vcx.debug_bounds("artist-0").is_none(), "no link in the bar");
    shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("np-artist-0").is_none(), "no link in Now Playing");
    assert!(vcx.debug_bounds("np-text").is_some(), "Now Playing is open");
}
