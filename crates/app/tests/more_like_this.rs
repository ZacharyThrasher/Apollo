//! "More like this" on a playlist page, headless: the section under a playlist of your own, its +
//! adding a song in place (and handing it back when refused), Refresh, the blocked artists and the
//! Settings switch. The real shell and views on GPUI's test platform, the simulated library as the
//! "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::Tab;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::library::LibrarySource;
use apollo_app::more_like::{MoreLikeModel, Phase, SUGGESTIONS};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::MORE_LIKE_THIS;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::blocklist::BlockedArtist;
use apollo_domain::library::{Collection, WriteFailure};
use apollo_domain::listing::LoadFailure;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext,
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
    more: Entity<MoreLikeModel>,
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
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    let more = view.read_with(vcx, |v, _| v.more_like().clone());
    (
        Page {
            shell,
            view,
            more,
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

fn open(page: &Page, vcx: &mut VisualTestContext, collection: Collection) {
    page.view
        .update_in(vcx, |v, window, cx| v.open(collection, window, cx));
    vcx.run_until_parked();
}

fn open_road_trip(page: &Page, vcx: &mut VisualTestContext) {
    let c = collection(page, vcx, Tab::Playlists, ROAD_TRIP);
    open(page, vcx, c);
}

/// The video ids of the rows the open page shows, in order.
fn page_videos(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .map(|d| {
                d.listing
                    .items()
                    .iter()
                    .map(|r| r.track.video_id.as_str().to_owned())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn page_has_tokens(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .detail()
            .is_some_and(|d| d.listing.items().iter().all(|r| r.occurrence.is_some()))
    })
}

/// The video ids of the songs on offer, in order.
fn offered(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.more.read_with(vcx, |m, _| {
        m.songs()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn phase(page: &Page, vcx: &mut VisualTestContext) -> Phase {
    page.more.read_with(vcx, |m, _| m.phase())
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.writes().read(cx).notice().map(|n| n.text.to_string())
    })
}

// ---- where it shows ---------------------------------------------------------------------------

#[gpui::test]
fn a_playlist_of_your_own_ends_with_ten_songs_like_its_own(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_road_trip(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Ready);
    let songs = offered(&page, vcx);
    assert_eq!(songs.len(), SUGGESTIONS);
    // Seeded from the playlist's first song, and none of its own songs is offered back.
    let own = page_videos(&page, vcx);
    assert_eq!(page.sim.radio_seeds(), vec![own[0].clone()]);
    assert!(songs.iter().all(|s| !own.contains(s)));
    // Under the playlist's rows: the title row, its Refresh, the first songs and their +.
    assert!(on_screen(vcx, "more-like-title"));
    assert!(on_screen(vcx, "more-like-refresh"));
    assert!(on_screen(vcx, "more-like-add-0"));
    assert!(on_screen(vcx, "more-like-add-1"));
    let title = bounds(vcx, "more-like-title");
    let last_own = bounds(vcx, &format!("library-row-{}", own.len() - 1));
    assert!(title.origin.y > last_own.origin.y, "after the last row of the playlist");
}

#[gpui::test]
fn liked_music_and_albums_have_no_suggestions(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let liked = collection(&page, vcx, Tab::Playlists, "Liked Music");
    open(&page, vcx, liked);
    assert_eq!(phase(&page, vcx), Phase::Hidden);
    assert!(!on_screen(vcx, "more-like-title"));
    page.view.update_in(vcx, |v, _, cx| v.model().update(cx, |m, cx| m.close_all(cx)));
    vcx.run_until_parked();

    page.view.update(vcx, |v, cx| v.model().update(cx, |m, cx| m.set_tab(Tab::Albums, cx)));
    vcx.run_until_parked();
    let album = page
        .view
        .read_with(vcx, |v, cx| v.model().read(cx).list(Tab::Albums)[0].clone());
    open(&page, vcx, album);
    assert_eq!(phase(&page, vcx), Phase::Hidden);
    assert!(!on_screen(vcx, "more-like-title"));
    assert!(page.sim.radio_seeds().is_empty(), "nothing was fetched for them");
}

#[gpui::test]
fn the_settings_switch_hides_the_section_and_fetches_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&MORE_LIKE_THIS, false, cx));
    vcx.run_until_parked();
    open_road_trip(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Hidden);
    assert!(!on_screen(vcx, "more-like-title"));
    assert!(page.sim.radio_seeds().is_empty());

    // Switching it on asks for the open playlist's songs.
    settings.update(vcx, |s, cx| s.set(&MORE_LIKE_THIS, true, cx));
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert!(on_screen(vcx, "more-like-title"));
}

#[gpui::test]
fn leaving_the_page_forgets_the_suggestions(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_road_trip(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Ready);
    page.view.update(vcx, |v, cx| v.model().update(cx, |m, cx| m.close_all(cx)));
    vcx.run_until_parked();
    assert_eq!(phase(&page, vcx), Phase::Hidden);
    assert!(offered(&page, vcx).is_empty());
}

#[gpui::test]
fn a_failed_radio_says_so_and_retry_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_radio_failure(Some(LoadFailure::Network));
    open_road_trip(&page, vcx);
    assert_eq!(phase(&page, vcx), Phase::Failed(LoadFailure::Network));
    assert!(on_screen(vcx, "more-like-title"));
    page.sim.set_radio_failure(None);
    click(vcx, "more-like-refresh");
    assert_eq!(phase(&page, vcx), Phase::Ready);
    assert_eq!(offered(&page, vcx).len(), SUGGESTIONS);
}

// ---- blocked artists ----------------------------------------------------------------------------

#[gpui::test]
fn a_blocked_artists_songs_are_not_offered(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // The artist of the first radio's fourth song.
    let first = SimLibrary::instant().radio("peek").unwrap();
    let name = first[3].track.artists[0].name.clone();
    let blocked = page.shell.read_with(vcx, |s, _| s.block_model().clone());
    blocked.update(vcx, |m, cx| {
        m.block(BlockedArtist { id: None, name: name.clone() }, cx)
    });
    open_road_trip(&page, vcx);
    let songs = page.more.read_with(vcx, |m, _| m.songs().to_vec());
    assert!(!songs.is_empty());
    assert!(songs
        .iter()
        .all(|r| r.track.artists.iter().all(|a| a.name != name)));
    assert!(songs.len() < SUGGESTIONS);
}

// ---- adding -------------------------------------------------------------------------------------

#[gpui::test]
fn the_plus_adds_the_song_in_place_and_it_leaves_the_offer(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_road_trip(&page, vcx);
    let before = page_videos(&page, vcx);
    let songs = offered(&page, vcx);
    let picked = songs[1].clone();

    click(vcx, "more-like-add-1");

    // One write, to this playlist, for that song.
    assert_eq!(
        page.sim.write_log(),
        vec![(format!("add:{ROAD_TRIP_ID}"), picked.clone())]
    );
    // The song is the page's last row, with the service's own token once it was read back.
    let after = page_videos(&page, vcx);
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after.last(), Some(&picked));
    assert!(page_has_tokens(&page, vcx));
    // The offer is still ten songs, without the one added.
    let now = offered(&page, vcx);
    assert!(!now.contains(&picked));
    assert!(now.iter().all(|s| !after.contains(s)));
    assert_eq!(notice(&page, vcx), None, "success is silent");
}

#[gpui::test]
fn a_refused_add_takes_the_row_off_the_page_and_back_into_the_offer(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_road_trip(&page, vcx);
    let before = page_videos(&page, vcx);
    let songs = offered(&page, vcx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));

    click(vcx, "more-like-add-0");

    assert_eq!(page_videos(&page, vcx), before, "the row is gone again");
    let now = offered(&page, vcx);
    assert_eq!(now.first(), Some(&songs[0]), "the song is back where it was");
    assert_eq!(now.len(), SUGGESTIONS);
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't add to playlist"));
}

#[gpui::test]
fn an_add_whose_reply_is_lost_is_settled_by_reading_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_road_trip(&page, vcx);
    let picked = offered(&page, vcx)[0].clone();
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);

    click(vcx, "more-like-add-0");

    let adds = page
        .sim
        .write_log()
        .iter()
        .filter(|(w, _)| w.starts_with("add:"))
        .count();
    assert_eq!(adds, 1, "never sent twice");
    assert_eq!(page_videos(&page, vcx).last(), Some(&picked));
    assert!(!offered(&page, vcx).contains(&picked));
    assert_eq!(notice(&page, vcx), None);
}

// ---- refresh ------------------------------------------------------------------------------------

#[gpui::test]
fn refresh_shows_ten_other_songs_from_another_seed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_road_trip(&page, vcx);
    let first = offered(&page, vcx);

    click(vcx, "more-like-refresh");

    let second = offered(&page, vcx);
    assert_eq!(second.len(), SUGGESTIONS);
    assert!(second.iter().all(|s| !first.contains(s)), "a new set, nothing repeated");
    let seeds = page.sim.radio_seeds();
    assert_eq!(seeds.len(), 2);
    assert_ne!(seeds[0], seeds[1], "another song of the playlist seeds it");
}
