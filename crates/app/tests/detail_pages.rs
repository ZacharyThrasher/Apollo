//! Headless tests for the album, artist and playlist pages: the real shell, Search and Library
//! views on GPUI's test platform, with the simulated library (no latency) and deterministic mock
//! playback adapters. Search answers with one album, one artist and one playlist card.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::{ArtistTarget, LibraryModel, Load, PlayRequest, Target};
use apollo_app::library::sim::{self, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::search::{SearchSource, DEBOUNCE};
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{Hit, SearchResults};
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
        // Wide, like an artist banner: the page must crop it, not squeeze it.
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(100.0).render(96)))
    }
}

fn hit(id: &str, title: &str, subtitle: Option<&str>) -> Hit {
    Hit {
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: subtitle.map(str::to_owned),
        thumbnail: Some("https://t/x".into()),
        explicit: false,
    }
}

/// One card of each kind, for any query.
struct TestSearch;
impl SearchSource for TestSearch {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Ok(SearchResults {
            albums: vec![hit(
                &format!("{}Night Shift", sim::SEARCH_ALBUM_PREFIX),
                "Night Shift",
                Some("Neon Arcade • 2024"),
            )],
            artists: vec![hit(
                &format!("{}Neon Arcade", sim::SEARCH_ARTIST_PREFIX),
                "Neon Arcade",
                None,
            )],
            playlists: vec![hit("VLPLsim1", "Night Drive Mix", Some("Apollo"))],
            ..SearchResults::default()
        })
    }
}

fn prepared() -> Prepared {
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
        search: Arc::new(TestSearch),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(SimLibrary::instant()),
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    view: Entity<LibraryView>,
    model: Entity<LibraryModel>,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
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
    let ready = prepared();
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    // These pages are about what gets queued from a list, so no radio is queued behind it.
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    let model = view.read_with(vcx, |v, _| v.model().clone());
    (
        Page {
            player,
            shell,
            view,
            model,
        },
        vcx,
    )
}

/// Ctrl+K, a query, and the cards it answers with on screen.
fn search(page: &Page, vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("a b");
    vcx.executor().advance_clock(DEBOUNCE);
    vcx.run_until_parked();
    page.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Search));
}

fn click_card(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

fn route(page: &Page, vcx: &mut VisualTestContext) -> Route {
    page.shell.read_with(vcx, |s, _| s.route())
}

fn depth(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.model.read_with(vcx, |m, _| m.depth())
}

fn current_title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player
        .read_with(vcx, |m, _| m.view().title.to_string())
}

fn queue_len(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        q.history().len() + usize::from(q.current().is_some()) + q.upcoming().count()
    })
}

fn open_artist(page: &Page, vcx: &mut VisualTestContext, id: &str) {
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let target = Target::Artist(ArtistTarget {
        id: BrowseId::new(id),
        name: "Artist".into(),
        thumbnail: None,
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open_target(target, window, cx));
    vcx.run_until_parked();
}

// ---- album and playlist cards ----------------------------------------------------------------

#[gpui::test]
fn tapping_an_album_card_opens_its_page_and_back_returns_to_the_results(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-album-0");

    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.detail_origin()),
        Some(Route::Search)
    );
    assert_eq!(
        page.shell.read_with(vcx, |s, _| s.nav_route()),
        Route::Search,
        "the nav keeps the route the page was opened from lit"
    );
    page.model.read_with(vcx, |m, _| {
        let d = m.detail().expect("the album page is open");
        assert_eq!(d.collection.title, "Night Shift");
        assert_eq!(d.listing.items().len(), 1);
        assert_eq!(d.listing.items()[0].track.title, "Midnight Drive");
    });

    // Escape is Back: the Search page is where it was, results and all.
    vcx.simulate_keystrokes("escape");
    assert_eq!(route(&page, vcx), Route::Search);
    assert_eq!(depth(&page, vcx), 0);
    assert_eq!(page.shell.read_with(vcx, |s, _| s.detail_origin()), None);
    let results = page
        .shell
        .read_with(vcx, |s, cx| s.search_model().read(cx).results().cloned());
    assert_eq!(results.expect("results survive").albums.len(), 1);
    assert!(
        vcx.debug_bounds("search-album-0").is_some(),
        "the cards are on screen again"
    );
}

#[gpui::test]
fn a_playlist_card_opens_the_playlist_and_a_row_plays_from_there(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-playlist-0");
    page.model.read_with(vcx, |m, _| {
        let d = m.detail().expect("the playlist page is open");
        assert_eq!(d.collection.title, "Night Drive Mix");
        assert_eq!(d.listing.items().len(), 3);
    });
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::row(1), cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
    assert_eq!(queue_len(&page, vcx), 3, "the whole playlist is queued");
}

#[gpui::test]
fn album_play_and_shuffle_use_the_queue_like_the_library(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-album-0");
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx));
    vcx.run_until_parked();
    page.player.read_with(vcx, |m, _| {
        assert!(m.coordinator().unwrap().queue().shuffle());
    });
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
}

// ---- artist ----------------------------------------------------------------------------------

#[gpui::test]
fn tapping_an_artist_card_opens_the_artist_with_songs_and_album_rows(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-artist-0");
    assert_eq!(route(&page, vcx), Route::Library);
    page.model.read_with(vcx, |m, _| {
        let a = m.artist().expect("the artist page is open");
        assert_eq!(a.name(), "Neon Arcade");
        let content = a.content().expect("the page has loaded");
        assert_eq!(content.top_songs.len(), 1);
        let titles: Vec<_> = content.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Albums", "Singles"]);
    });
    assert!(
        vcx.debug_bounds("artist-card-0-0").is_some(),
        "an album card is drawn"
    );
    assert!(
        vcx.debug_bounds("artist-card-1-0").is_some(),
        "a single card is drawn"
    );
}

#[gpui::test]
fn an_album_opened_from_an_artist_stacks_and_back_unwinds_one_page_at_a_time(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-artist-0");
    click_card(vcx, "artist-card-0-0");
    assert_eq!(depth(&page, vcx), 2);
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.detail().unwrap().collection.title, "Night Shift");
    });

    vcx.simulate_keystrokes("escape");
    assert_eq!(depth(&page, vcx), 1);
    assert!(page.model.read_with(vcx, |m, _| m.artist().is_some()));
    assert_eq!(route(&page, vcx), Route::Library, "still on the artist");

    vcx.simulate_keystrokes("alt-left");
    assert_eq!(depth(&page, vcx), 0);
    assert_eq!(
        route(&page, vcx),
        Route::Search,
        "the last page returns to Search"
    );
}

#[gpui::test]
fn the_artists_play_and_shuffle_act_on_the_top_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(
        &page,
        vcx,
        &format!("{}Neon Arcade", sim::SEARCH_ARTIST_PREFIX),
    );
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx));
    vcx.run_until_parked();
    page.player.read_with(vcx, |m, _| {
        assert!(m.coordinator().unwrap().queue().shuffle());
    });
}

#[gpui::test]
fn a_play_asked_before_the_artist_page_arrives_waits_for_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let target = Target::Artist(ArtistTarget {
        id: BrowseId::new(sim::ARTIST_SLOW_ID),
        name: "Neon Arcade".into(),
        thumbnail: None,
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open_target(target, window, cx));
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx));
    assert_eq!(
        current_title(&page, vcx),
        "Start",
        "nothing plays from a page still loading"
    );
    page.model.read_with(vcx, |m, _| {
        let a = m.artist().unwrap();
        assert!(matches!(a.state, Load::Loading));
        assert!(a.pending.is_some());
    });
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
}

#[gpui::test]
fn a_failed_artist_page_says_so_and_retry_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, sim::ARTIST_BROKEN_ID);
    page.model.read_with(vcx, |m, _| {
        assert!(matches!(
            m.artist().unwrap().state,
            Load::Failed(LoadFailure::Network)
        ));
    });
    page.model.update(vcx, |m, cx| m.reload_artist(cx));
    page.model.read_with(vcx, |m, _| {
        assert!(matches!(m.artist().unwrap().state, Load::Loading));
    });
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        assert!(matches!(m.artist().unwrap().state, Load::Failed(_)));
    });
    // A failed page never starts playback.
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx));
    assert_eq!(current_title(&page, vcx), "Start");
}

#[gpui::test]
fn an_artist_with_nothing_on_the_page_is_empty_and_play_does_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, sim::ARTIST_EMPTY_ID);
    page.model.read_with(vcx, |m, _| {
        assert!(m.artist().unwrap().content().unwrap().is_empty());
    });
    page.model
        .update(vcx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx));
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Start");
}

#[gpui::test]
fn a_late_artist_answer_never_lands_on_a_page_that_was_closed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let slow = ArtistTarget {
        id: BrowseId::new(sim::ARTIST_SLOW_ID),
        name: "Slow".into(),
        thumbnail: None,
    };
    let other = ArtistTarget {
        id: BrowseId::new(format!("{}Verdant", sim::SEARCH_ARTIST_PREFIX)),
        name: "Verdant".into(),
        thumbnail: None,
    };
    page.model.update(vcx, |m, cx| m.open_artist(slow, cx));
    page.model.update(vcx, |m, cx| m.close(cx));
    page.model.update(vcx, |m, cx| m.open_artist(other, cx));
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.depth(), 1);
        assert_eq!(m.artist().unwrap().name(), "Verdant");
    });
}

#[gpui::test]
fn the_keyboard_walks_an_artists_top_songs_and_enter_plays(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // Verdant has one song; Neon Arcade has one too, so use the generic selection bounds.
    open_artist(&page, vcx, &format!("{}Verdant", sim::SEARCH_ARTIST_PREFIX));
    vcx.simulate_keystrokes("down");
    assert_eq!(
        page.view.read_with(vcx, |v, _| v.selected()),
        0,
        "one song: stays on it"
    );
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert_eq!(current_title(&page, vcx), "Fern");
}

// ---- navigation ------------------------------------------------------------------------------

#[gpui::test]
fn pages_opened_from_the_library_itself_return_to_its_lists_not_elsewhere(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    let album = page.model.read_with(vcx, |m, _| {
        m.list(apollo_app::library::Tab::Albums)[0].clone()
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open(album, window, cx));
    vcx.run_until_parked();
    assert_eq!(page.shell.read_with(vcx, |s, _| s.detail_origin()), None);
    vcx.simulate_keystrokes("escape");
    assert_eq!(depth(&page, vcx), 0);
    assert_eq!(route(&page, vcx), Route::Library);
}

#[gpui::test]
fn choosing_a_nav_item_closes_pages_opened_from_search(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-album-0");
    assert_eq!(depth(&page, vcx), 1);

    // Library again: the lists, with nothing left open and nothing to return to.
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Library, window, cx)
    });
    assert_eq!(depth(&page, vcx), 0);
    assert_eq!(route(&page, vcx), Route::Library);
    assert_eq!(page.shell.read_with(vcx, |s, _| s.detail_origin()), None);

    // Another page from Search, then Settings: the page closes, Search is not resurrected later.
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Search, window, cx)
    });
    click_card(vcx, "search-artist-0");
    assert_eq!(depth(&page, vcx), 1);
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Settings, window, cx)
    });
    assert_eq!(depth(&page, vcx), 0);
    assert_eq!(route(&page, vcx), Route::Settings);
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Library, window, cx)
    });
    assert_eq!(depth(&page, vcx), 0, "the lists, not the old artist");
}

#[gpui::test]
fn ctrl_k_from_an_opened_page_goes_to_search_and_closes_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-album-0");
    vcx.simulate_keystrokes("ctrl-k");
    assert_eq!(route(&page, vcx), Route::Search);
    assert_eq!(depth(&page, vcx), 0);
}

#[gpui::test]
fn tab_reaches_the_cards_in_reading_order_and_enter_opens_the_focused_one(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    // From the field: its clear button, the chip row (one stop), the results list, then the
    // groups in reading order: each group's "Show all", then its cards (albums, artists,
    // playlists).
    vcx.simulate_keystrokes("tab tab tab tab tab tab tab enter");
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        let artist = m.artist().expect("the second card is the artist");
        assert_eq!(artist.name(), "Neon Arcade");
    });
}

#[gpui::test]
fn an_account_change_closes_pages_and_forgets_where_they_came_from(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    search(&page, vcx);
    click_card(vcx, "search-album-0");
    page.view.update(vcx, |v, cx| v.reset(cx));
    assert_eq!(depth(&page, vcx), 0);
}

#[gpui::test]
fn enter_on_an_artist_pages_card_opens_it_instead_of_playing_the_selected_song(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_artist(
        &page,
        vcx,
        &format!("{}Neon Arcade", sim::SEARCH_ARTIST_PREFIX),
    );
    // The list holds focus and has a song selected; the next tab stop is the first album card.
    vcx.simulate_keystrokes("down tab enter");
    vcx.run_until_parked();
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.depth(), 2, "a card opened a page over the artist");
    });
    assert_eq!(
        current_title(&page, vcx),
        "Start",
        "nothing started playing"
    );
}
