//! Headless tests for the page cache wiring ("instant pages"): the real shell, Home and Library
//! models on GPUI's test platform, the simulated library (no latency), and an in-memory page
//! cache. Nothing is fetched until the tests let the executor run, so "what is on screen right
//! after the open call" is exactly "what was drawn before the network answered".

use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::home::{Access, HomeModel, Phase};
use apollo_app::library::model::{ArtistTarget, LibraryModel, Load};
use apollo_app::library::sim::{self, SimLibrary, WriteBehaviour};
use apollo_app::library::{ArtistContent, LibrarySource, TrackRow};
use apollo_app::page_cache::{keys, PageCache};
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::library::{Collection, PlaylistTarget, WriteFailure};
use apollo_domain::listing::{LoadFailure, LoadState, Page};
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{HomePage, HomeShelf};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

/// The key the simulated account is filed under (its identity has no page id).
const ACCOUNT: &str = "Demo Listener";

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

/// The simulated library, with a switch that makes every read fail.
#[derive(Clone)]
struct Switch {
    inner: SimLibrary,
    failure: Arc<Mutex<Option<LoadFailure>>>,
}

impl Switch {
    fn new() -> Self {
        Self {
            inner: SimLibrary::instant(),
            failure: Arc::new(Mutex::new(None)),
        }
    }

    fn fail_with(&self, failure: Option<LoadFailure>) {
        *self.failure.lock().unwrap() = failure;
    }

    fn check(&self) -> Result<(), LoadFailure> {
        match *self.failure.lock().unwrap() {
            Some(f) => Err(f),
            None => Ok(()),
        }
    }
}

impl LibrarySource for Switch {
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure> {
        self.check()?;
        self.inner.collections()
    }
    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure> {
        self.check()?;
        self.inner.tracks(collection)
    }
    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure> {
        self.check()?;
        self.inner.more_tracks(token)
    }
    fn artist(&self, id: &BrowseId) -> Result<ArtistContent, LoadFailure> {
        self.check()?;
        self.inner.artist(id)
    }
    fn home(&self) -> Result<HomePage, LoadFailure> {
        self.check()?;
        self.inner.home()
    }
    fn home_more(&self, token: &str) -> Result<HomePage, LoadFailure> {
        self.check()?;
        self.inner.home_more(token)
    }
    fn artwork(&self, url: &str) -> Option<DecodedArt> {
        self.inner.artwork(url)
    }
    fn set_liked(&self, video_id: &str, liked: bool) -> Result<(), WriteFailure> {
        self.inner.set_liked(video_id, liked)
    }
    fn liked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        self.inner.liked(video_id)
    }
    fn playlists_for(&self, video_id: &str) -> Result<Vec<PlaylistTarget>, LoadFailure> {
        self.inner.playlists_for(video_id)
    }
    fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<(), WriteFailure> {
        self.inner.add_to_playlist(playlist_id, video_id)
    }
}

fn prepared(library: Switch) -> Prepared {
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
        search: Arc::new(apollo_app::search::Unavailable),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(library),
    }
}

struct Rig {
    cache: PageCache,
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    home: Entity<HomeModel>,
    model: Entity<LibraryModel>,
    library: Switch,
}

impl Rig {
    /// Installs the simulated backend: the library and Home start loading, cache first.
    fn install(&self, vcx: &mut VisualTestContext) {
        let ready = prepared(self.library.clone());
        let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
        self.player.update(vcx, |m, cx| {
            m.restore_autoplay(Some(false));
            m.install(ready, cx)
        });
        self.shell
            .update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    }
}

/// A shell over the simulated account (signed in as [`ACCOUNT`]) with a page cache installed
/// and `seed` run on it before anything is shown. The backend is not installed yet.
fn rig(
    cx: &mut TestAppContext,
    seed: impl FnOnce(&PageCache),
) -> (Rig, &mut VisualTestContext) {
    let cache = PageCache::in_memory();
    cache.set_account(Some(ACCOUNT.into()));
    seed(&cache);
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
        cx.set_reduce_motion(true);
        cache.clone().install(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let player = player.clone();
        move |_, cx| Shell::new(player, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    vcx.run_until_parked();
    let home = shell.read_with(vcx, |s, _| s.home().clone());
    let model = shell.read_with(vcx, |s, cx| s.library().read(cx).model().clone());
    (
        Rig {
            cache,
            player,
            shell,
            home,
            model,
            library: Switch::new(),
        },
        vcx,
    )
}

fn put<T: serde::Serialize + Send + 'static>(cache: &PageCache, key: &str, value: T) {
    cache.put(&cache.ticket(), key.to_owned(), value);
}

fn road_trip() -> Collection {
    sim::all_collections()
        .into_iter()
        .find(|c| c.id.as_str() == "VLPLsimroad")
        .expect("the simulated Road Trip playlist")
}

fn artist_id() -> BrowseId {
    BrowseId::new(format!("{}Neon Arcade", sim::SEARCH_ARTIST_PREFIX))
}

fn artist_target() -> ArtistTarget {
    ArtistTarget {
        id: artist_id(),
        name: "Neon Arcade".into(),
        thumbnail: None,
    }
}

fn fresh_rows(collection: &Collection) -> Vec<TrackRow> {
    let page = SimLibrary::instant().tracks(collection).unwrap();
    assert!(page.continuation.is_none(), "a short playlist is one page");
    page.items
}

/// How many playlists and albums the model shows, `None` while there is no list.
fn counts(model: &Entity<LibraryModel>, vcx: &mut VisualTestContext) -> Option<(usize, usize)> {
    model.read_with(vcx, |m, _| match m.collections() {
        Load::Ready(c) => Some((c.playlists.len(), c.albums.len())),
        _ => None,
    })
}

/// Records what a model showed at every redraw: it must never blink back to a skeleton.
fn record<T: 'static>(
    vcx: &mut VisualTestContext,
    entity: &Entity<T>,
    read: impl Fn(&T) -> String + 'static,
) -> Arc<Mutex<Vec<String>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let sink = log.clone();
    vcx.update(|_, cx| {
        cx.observe(entity, move |entity, cx| {
            sink.lock().unwrap().push(read(entity.read(cx)));
        })
        .detach();
    });
    log
}

// ---- the library lists -----------------------------------------------------------------------------

#[gpui::test]
fn the_library_lists_are_drawn_from_the_cache_then_the_fresh_ones_replace_them(
    cx: &mut TestAppContext,
) {
    let all = sim::all_collections();
    let (rig, vcx) = rig(cx, |cache| {
        put(cache, keys::COLLECTIONS, all[..2].to_vec());
    });
    let shown = record(vcx, &rig.model, |m| match m.collections() {
        Load::Ready(c) => format!("ready {}+{}", c.playlists.len(), c.albums.len()),
        Load::Loading => "loading".into(),
        other => format!("{other:?}"),
    });
    rig.install(vcx);

    let stale = counts(&rig.model, vcx).expect("drawn before the service answered");
    assert!(stale.0 + stale.1 <= 3, "the cached two, plus Liked Music: {stale:?}");
    vcx.run_until_parked();
    let fresh = counts(&rig.model, vcx).unwrap();
    assert!(fresh.0 + fresh.1 > stale.0 + stale.1, "the fresh lists replaced them");

    let log = shown.lock().unwrap().clone();
    assert!(
        log.iter().all(|s| s.starts_with("ready")),
        "never a skeleton or an error in between: {log:?}"
    );
    // The answer was cached for the next launch.
    let again = rig.cache.get::<Vec<Collection>>(keys::COLLECTIONS).unwrap();
    assert_eq!(again.value.len(), all.len());
}

#[gpui::test]
fn a_cached_list_that_is_still_right_is_not_redrawn(cx: &mut TestAppContext) {
    let all = sim::all_collections();
    let (rig, vcx) = rig(cx, |cache| put(cache, keys::COLLECTIONS, all.clone()));
    let redraws = Arc::new(Mutex::new(0usize));
    let sink = redraws.clone();
    vcx.update(|_, cx| {
        cx.observe(&rig.model, move |_, _| *sink.lock().unwrap() += 1)
            .detach()
    });
    rig.install(vcx);
    let after_open = *redraws.lock().unwrap();
    assert!(after_open >= 1);
    vcx.run_until_parked();
    assert_eq!(
        *redraws.lock().unwrap(),
        after_open,
        "the same answer swaps in silently: no flicker, no scroll jump"
    );
}

#[gpui::test]
fn without_a_cache_the_first_load_still_shows_the_skeleton_and_fills_the_cache(
    cx: &mut TestAppContext,
) {
    let (rig, vcx) = rig(cx, |_| {});
    rig.install(vcx);
    assert_eq!(
        rig.model.read_with(vcx, |m, _| m.collections().clone()),
        Load::Loading
    );
    vcx.run_until_parked();
    assert!(counts(&rig.model, vcx).is_some());
    assert!(rig.cache.get::<Vec<Collection>>(keys::COLLECTIONS).is_some());
}

#[gpui::test]
fn a_failed_refresh_leaves_cached_lists_alone_except_a_dead_session(cx: &mut TestAppContext) {
    let all = sim::all_collections();
    let (rig, vcx) = rig(cx, |cache| put(cache, keys::COLLECTIONS, all.clone()));
    rig.library.fail_with(Some(LoadFailure::Network));
    rig.install(vcx);
    vcx.run_until_parked();
    assert!(
        counts(&rig.model, vcx).is_some(),
        "a connection problem does not take the lists away"
    );

    rig.library.fail_with(Some(LoadFailure::Session));
    rig.model.update(vcx, |m, cx| m.reload_collections(cx));
    vcx.run_until_parked();
    assert_eq!(
        rig.model.read_with(vcx, |m, _| m.collections().clone()),
        Load::Failed(LoadFailure::Session),
        "an expired sign-in is always said"
    );
}

// ---- album and playlist pages ------------------------------------------------------------------------

#[gpui::test]
fn a_revisited_playlist_shows_its_rows_at_once_and_swaps_in_the_fresh_ones(
    cx: &mut TestAppContext,
) {
    let road = road_trip();
    let fresh = fresh_rows(&road);
    assert!(fresh.len() > 1);
    let (rig, vcx) = rig(cx, |cache| {
        put(cache, &keys::collection(&road.id), fresh[..1].to_vec())
    });
    rig.install(vcx);
    vcx.run_until_parked();
    let shown = record(vcx, &rig.model, |m| {
        let d = m.detail().expect("the page is open");
        format!("{:?} {}", d.listing.state(), d.listing.items().len())
    });

    rig.model.update(vcx, |m, cx| m.open(road.clone(), cx));
    rig.model.read_with(vcx, |m, _| {
        let d = m.detail().unwrap();
        assert_eq!(d.listing.state(), LoadState::Complete, "no skeleton");
        assert_eq!(d.listing.items().len(), 1, "the cached rows, before any fetch");
        assert!(!d.is_loading());
    });
    vcx.run_until_parked();
    rig.model.read_with(vcx, |m, _| {
        let d = m.detail().unwrap();
        assert_eq!(d.listing.items().len(), fresh.len());
        assert_eq!(d.listing.items(), fresh.as_slice());
    });
    let log = shown.lock().unwrap().clone();
    assert!(
        log.iter().all(|s| s.starts_with("Complete")),
        "the rows never blinked back to loading: {log:?}"
    );
    let cached = rig
        .cache
        .get::<Vec<TrackRow>>(&keys::collection(&road.id))
        .unwrap();
    assert_eq!(cached.value, fresh, "the fresh rows replaced the stale ones in the cache");
}

#[gpui::test]
fn a_playlist_opened_for_the_first_time_loads_with_the_skeleton_and_is_cached(
    cx: &mut TestAppContext,
) {
    let road = road_trip();
    let (rig, vcx) = rig(cx, |_| {});
    rig.install(vcx);
    vcx.run_until_parked();
    rig.model.update(vcx, |m, cx| m.open(road.clone(), cx));
    rig.model
        .read_with(vcx, |m, _| assert!(m.detail().unwrap().is_loading()));
    vcx.run_until_parked();
    assert_eq!(
        rig.cache
            .get::<Vec<TrackRow>>(&keys::collection(&road.id))
            .unwrap()
            .value,
        fresh_rows(&road)
    );
}

#[gpui::test]
fn a_cached_page_stays_when_the_refresh_fails_and_plays_from_the_cached_rows(
    cx: &mut TestAppContext,
) {
    let road = road_trip();
    let fresh = fresh_rows(&road);
    let (rig, vcx) = rig(cx, |cache| {
        put(cache, &keys::collection(&road.id), fresh.clone())
    });
    rig.install(vcx);
    vcx.run_until_parked();
    rig.library.fail_with(Some(LoadFailure::Network));
    rig.model.update(vcx, |m, cx| m.open(road.clone(), cx));
    vcx.run_until_parked();
    rig.model.read_with(vcx, |m, _| {
        let d = m.detail().unwrap();
        assert_eq!(d.failure(), None, "no error row over rows that are there");
        assert_eq!(d.listing.items().len(), fresh.len());
    });
}

// ---- artist pages ------------------------------------------------------------------------------------

#[gpui::test]
fn a_revisited_artist_page_draws_at_once_and_follows_the_fresh_answer(cx: &mut TestAppContext) {
    let fresh = SimLibrary::instant().artist(&artist_id()).unwrap();
    let mut stale = fresh.clone();
    stale.top_songs.truncate(0);
    stale.sections.clear();
    let (rig, vcx) = rig(cx, |cache| {
        put(cache, &keys::artist(&artist_id()), stale.clone())
    });
    rig.install(vcx);
    vcx.run_until_parked();
    let shown = record(vcx, &rig.model, |m| match m.artist() {
        Some(a) => format!("{:?}", a.content().map(|c| c.top_songs.len())),
        None => "none".into(),
    });

    rig.model
        .update(vcx, |m, cx| m.open_artist(artist_target(), cx));
    rig.model.read_with(vcx, |m, _| {
        let content = m.artist().unwrap().content().expect("drawn before the fetch");
        assert!(content.top_songs.is_empty(), "the cached answer");
    });
    vcx.run_until_parked();
    rig.model.read_with(vcx, |m, _| {
        let content = m.artist().unwrap().content().unwrap();
        assert_eq!(**content, fresh);
    });
    let log = shown.lock().unwrap().clone();
    assert!(
        log.iter().all(|s| s.starts_with("Some")),
        "the page never went back to a skeleton: {log:?}"
    );
    assert_eq!(
        rig.cache.get::<ArtistContent>(&keys::artist(&artist_id())).unwrap().value,
        fresh
    );
}

#[gpui::test]
fn a_cached_artist_survives_a_failed_refresh_but_not_a_dead_session(cx: &mut TestAppContext) {
    let fresh = SimLibrary::instant().artist(&artist_id()).unwrap();
    let (rig, vcx) = rig(cx, |cache| {
        put(cache, &keys::artist(&artist_id()), fresh.clone())
    });
    rig.install(vcx);
    vcx.run_until_parked();
    rig.library.fail_with(Some(LoadFailure::Network));
    rig.model
        .update(vcx, |m, cx| m.open_artist(artist_target(), cx));
    vcx.run_until_parked();
    assert!(rig.model.read_with(vcx, |m, _| m.artist().unwrap().content().is_some()));

    rig.model.update(vcx, |m, cx| {
        m.close(cx);
    });
    rig.library.fail_with(Some(LoadFailure::Session));
    rig.model
        .update(vcx, |m, cx| m.open_artist(artist_target(), cx));
    vcx.run_until_parked();
    assert!(
        rig.model.read_with(vcx, |m, _| m.artist().unwrap().content().is_none()),
        "an expired sign-in replaces the cached page with the error row"
    );
}

// ---- Home --------------------------------------------------------------------------------------------

fn first_shelf_only() -> HomePage {
    let mut page = SimLibrary::instant().home().unwrap();
    page.shelves.truncate(1);
    page.shelves[0].title = "Last time".into();
    page.continuation = Some("stale-token".into());
    page
}

fn shelf_titles(home: &Entity<HomeModel>, vcx: &mut VisualTestContext) -> Vec<String> {
    home.read_with(vcx, |m, _| {
        m.shelves().iter().map(|s: &Arc<HomeShelf>| s.title.clone()).collect()
    })
}

#[gpui::test]
fn home_is_drawn_from_the_cache_before_the_session_is_confirmed_and_refreshes_in_place(
    cx: &mut TestAppContext,
) {
    let (rig, vcx) = rig(cx, |cache| put(cache, keys::HOME, first_shelf_only()));
    // Nothing has been installed: no backend, no fetch. The last feed is already up.
    assert_eq!(shelf_titles(&rig.home, vcx), ["Last time"]);
    assert_eq!(rig.home.read_with(vcx, |m, _| m.phase()), Phase::Ready);
    assert!(rig.home.read_with(vcx, |m, _| m.shows_cache()));

    let phases = record(vcx, &rig.home, |m| format!("{:?}", m.phase()));
    rig.install(vcx);
    assert_eq!(
        shelf_titles(&rig.home, vcx),
        ["Last time"],
        "the skeleton never replaces it while the feed loads"
    );
    vcx.run_until_parked();
    let titles = shelf_titles(&rig.home, vcx);
    assert_ne!(titles, ["Last time"], "the fresh feed replaced it");
    assert!(titles.len() >= 2);
    assert!(!rig.home.read_with(vcx, |m, _| m.shows_cache()));
    assert_eq!(rig.home.read_with(vcx, |m, _| m.requests()), 1, "one fetch, behind the cache");
    assert!(
        phases.lock().unwrap().iter().all(|p| p == "Ready"),
        "never a skeleton or an error: {:?}",
        phases.lock().unwrap()
    );
    assert_eq!(
        rig.cache.get::<HomePage>(keys::HOME).unwrap().value.shelves[0].title,
        titles[0],
        "the fresh feed is what the next launch draws"
    );
    // The stale continuation token was never used: the batches after it still page in.
    assert!(titles.len() > 3, "the batches after the first still page in: {titles:?}");
}

#[gpui::test]
fn cached_home_stays_through_a_failed_fetch_but_a_dead_session_replaces_it(
    cx: &mut TestAppContext,
) {
    let (rig, vcx) = rig(cx, |cache| put(cache, keys::HOME, first_shelf_only()));
    rig.library.fail_with(Some(LoadFailure::Network));
    rig.install(vcx);
    vcx.run_until_parked();
    assert_eq!(shelf_titles(&rig.home, vcx), ["Last time"]);
    assert_eq!(rig.home.read_with(vcx, |m, _| m.phase()), Phase::Ready);

    // A new account check that ends in a refusal.
    rig.home
        .update(vcx, |m, cx| m.set_access(Access::Expired, 1, cx));
    assert_eq!(
        rig.home.read_with(vcx, |m, _| m.phase()),
        Phase::Failed(LoadFailure::Session)
    );
    assert!(shelf_titles(&rig.home, vcx).is_empty());
}

#[gpui::test]
fn first_ever_home_load_keeps_the_skeleton_and_fills_the_cache(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx, |_| {});
    rig.install(vcx);
    assert_eq!(rig.home.read_with(vcx, |m, _| m.phase()), Phase::Loading);
    vcx.run_until_parked();
    assert_eq!(rig.home.read_with(vcx, |m, _| m.phase()), Phase::Ready);
    assert!(rig.cache.get::<HomePage>(keys::HOME).is_some());
}

// ---- local writes and the account -----------------------------------------------------------------------

#[gpui::test]
fn a_like_drops_the_liked_music_page_and_a_page_fetched_before_it_cannot_put_it_back(
    cx: &mut TestAppContext,
) {
    let (rig, vcx) = rig(cx, |_| {});
    rig.install(vcx);
    vcx.run_until_parked();
    let liked = apollo_app::library::model::liked_music();
    // Open the page and let it land: it is cached.
    rig.model.update(vcx, |m, cx| m.open(liked.clone(), cx));
    vcx.run_until_parked();
    assert!(rig
        .cache
        .get::<Vec<TrackRow>>(&keys::collection(&liked.id))
        .is_some());
    rig.model.update(vcx, |m, cx| {
        m.close(cx);
    });

    // Opened again with the cache gone: a live fetch is in flight (it holds a ticket)...
    rig.cache.invalidate_liked_music();
    rig.model.update(vcx, |m, cx| m.open(liked.clone(), cx));
    // ...and a like is made before it lands. The service refuses it, so nothing refetches the
    // page afterwards: only the refusal of the older answer keeps the cache empty.
    rig.library
        .inner
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let writes = rig.shell.read_with(vcx, |s, _| s.writes().clone());
    writes.update(vcx, |w, cx| w.set_liked("someNewTrack", true, cx));
    assert!(
        rig.cache.get::<Vec<TrackRow>>(&keys::collection(&liked.id)).is_none(),
        "dropped the moment the like is made"
    );
    vcx.run_until_parked();
    assert!(
        rig.cache.get::<Vec<TrackRow>>(&keys::collection(&liked.id)).is_none(),
        "the older fetch was refused, so the next visit fetches fresh"
    );
}

#[gpui::test]
fn adding_to_a_playlist_drops_that_playlists_page_and_the_library_list(cx: &mut TestAppContext) {
    let road = road_trip();
    let (rig, vcx) = rig(cx, |cache| {
        put(cache, &keys::collection(&road.id), fresh_rows(&road));
        put(cache, keys::COLLECTIONS, sim::all_collections());
        put(cache, "coll:VLPLsimlong", Vec::<TrackRow>::new());
    });
    rig.install(vcx);
    vcx.run_until_parked();
    let target = PlaylistTarget {
        id: "PLsimroad".into(),
        title: "Road Trip".into(),
        thumbnail: None,
        holds_track: false,
    };
    let writes = rig.shell.read_with(vcx, |s, _| s.writes().clone());
    writes.update(vcx, |w, cx| w.add_to_playlist(&target, "someNewTrack", cx));
    assert!(rig.cache.get::<Vec<TrackRow>>(&keys::collection(&road.id)).is_none());
    assert!(rig.cache.get::<Vec<Collection>>(keys::COLLECTIONS).is_none());
    assert!(
        rig.cache.get::<Vec<TrackRow>>("coll:VLPLsimlong").is_some(),
        "other playlists are untouched"
    );
    vcx.run_until_parked();
    assert!(rig.cache.get::<Vec<TrackRow>>(&keys::collection(&road.id)).is_none());
}

#[gpui::test]
fn the_shell_files_pages_under_the_chosen_account_and_sign_out_forgets_them(
    cx: &mut TestAppContext,
) {
    use apollo_app::account::AccountModel;
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let cache = PageCache::in_memory();
    cx.update(|cx| cache.clone().install(cx));
    let ports = apollo_app::backend::account::ports();
    apollo_app::backend::account::start(&ports, true);
    let account = cx.new(|cx| AccountModel::new(ports, cx));
    account.update(cx, |m, cx| m.startup_finished(cx));
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let (player, account) = (player.clone(), account.clone());
        move |_, cx| Shell::with_account(player, account, cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        cache.account().as_deref(),
        Some(ACCOUNT),
        "the shell tells the cache whose pages these are"
    );
    put(&cache, "k", "page".to_owned());
    assert!(cache.get::<String>("k").is_some());

    account.update(vcx, |m, cx| m.sign_out(cx));
    vcx.run_until_parked();
    assert_eq!(cache.account(), None);
    assert!(cache.get::<String>("k").is_none(), "nothing stays after a sign-out");
    // And nothing is stored for nobody.
    put(&cache, "k", "again".to_owned());
    assert!(cache.get::<String>("k").is_none());
    drop(shell);
}
