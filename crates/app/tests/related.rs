//! The Related tab of Now Playing: the Up next · Lyrics · Related control, the pane that shows
//! the playing track's related songs and shelves, and its lazy, track-following loading. The
//! real shell and views on GPUI's test platform, the simulated library (no latency) and the
//! mock playback adapters.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::{RelatedBehaviour, SimLibrary};
use apollo_app::player::PlayerModel;
use apollo_app::related::{Phase, RelatedModel};
use apollo_app::settings::RelatedOpenStore;
use apollo_app::style::Fonts;
use apollo_app::ui::pane_tabs::Pane;
use apollo_app::ui::related_pane::RelatedPane;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Bounds, Entity, Modifiers, Pixels, TestAppContext, VisualTestContext};

const IDS: [&str; 3] = ["track000000", "track000001", "track000002"];

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

#[derive(Default)]
struct MemFlag(Mutex<Option<bool>>);
impl RelatedOpenStore for MemFlag {
    fn load(&self) -> Option<bool> {
        *self.0.lock().unwrap()
    }
    fn save(&self, on: bool) {
        *self.0.lock().unwrap() = Some(on);
    }
}

fn prepared(library: SimLibrary) -> Prepared {
    let queue = IDS
        .iter()
        .map(|id| LibraryTrack {
            track: Track::new(*id, format!("Title {id}")),
        })
        .collect();
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue,
        art: Arc::new(TestArt),
        search: Arc::new(apollo_app::search::Unavailable),
        thumbs: Arc::new(TestThumbs),
        library: Arc::new(library),
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    model: Entity<RelatedModel>,
    pane: Entity<RelatedPane>,
    library: SimLibrary,
    flag: Arc<MemFlag>,
}

fn setup(cx: &mut TestAppContext, saved: Option<bool>) -> (Page, &mut VisualTestContext) {
    setup_with(cx, saved, SimLibrary::instant())
}

fn setup_with(
    cx: &mut TestAppContext,
    saved: Option<bool>,
    library: SimLibrary,
) -> (Page, &mut VisualTestContext) {
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
    let ready = prepared(library.clone());
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| m.install(ready, cx));
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    let flag = Arc::new(MemFlag(Mutex::new(saved)));
    shell.update(vcx, |s, cx| {
        s.install_related(Some(flag.clone() as Arc<dyn RelatedOpenStore>), cx)
    });
    vcx.run_until_parked();
    let model = shell.read_with(vcx, |s, _| s.related().clone());
    let pane = shell.read_with(vcx, |s, _| s.related_pane().clone());
    (
        Page {
            player,
            shell,
            model,
            pane,
            library,
            flag,
        },
        vcx,
    )
}

/// Several parks: the pane's rows appear a frame after its model answers.
fn settle(vcx: &mut VisualTestContext) {
    for _ in 0..4 {
        vcx.run_until_parked();
        vcx.update(|window, _| window.refresh());
    }
    vcx.run_until_parked();
}

fn bounds(vcx: &mut VisualTestContext, selector: &'static str) -> Bounds<Pixels> {
    vcx.debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"))
}

fn on_screen(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

fn show(page: &Page, vcx: &mut VisualTestContext, pane: Pane) {
    page.shell.update(vcx, |s, cx| s.show_pane(pane, cx));
    settle(vcx);
}

fn pane_of(page: &Page, vcx: &mut VisualTestContext) -> Pane {
    page.shell.read_with(vcx, |s, cx| s.pane(cx))
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = bounds(vcx, id).center();
    vcx.simulate_click(at, Modifiers::none());
    settle(vcx);
}

fn titles(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.model.read_with(vcx, |m, _| {
        m.shelves().iter().map(|s| s.title.clone()).collect()
    })
}

#[gpui::test]
fn nothing_is_fetched_until_the_related_tab_is_opened(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    assert!(page.library.related_log().is_empty());
    // Skipping around, and opening Now Playing on its queue, ask for nothing either.
    page.player.update(vcx, |m, cx| m.next(cx));
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    settle(vcx);
    assert!(page.library.related_log().is_empty());
    assert_eq!(page.model.read_with(vcx, |m, _| m.phase()), Phase::Idle);

    show(&page, vcx, Pane::Related);
    let current = page.player.read_with(vcx, |m, _| m.current_video()).unwrap();
    assert_eq!(page.library.related_log(), [current.clone()]);
    assert_eq!(pane_of(&page, vcx), Pane::Related);
    assert_eq!(page.model.read_with(vcx, |m, _| m.phase()), Phase::Ready);
    assert_eq!(
        page.model
            .read_with(vcx, |m, _| m.shown_for().map(str::to_owned)),
        Some(current)
    );
}

#[gpui::test]
fn related_shows_songs_first_then_album_and_artist_shelves(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    assert_eq!(
        titles(&page, vcx),
        ["You might also like", "Albums", "Similar artists"]
    );
    assert!(on_screen(vcx, "related-pane"));
    // The songs are rows, the others cards.
    assert!(on_screen(vcx, "related-0-0"));
    assert!(on_screen(vcx, "related-1-0"));
    let song = bounds(vcx, "related-0-0");
    let album = bounds(vcx, "related-1-0");
    assert!(song.bottom() <= album.top(), "the song list comes first");
}

#[gpui::test]
fn the_pane_control_swaps_the_right_pane_and_the_bar_buttons_stay_in_sync(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    for tab in ["pane-tab-Up next", "pane-tab-Lyrics", "pane-tab-Related"] {
        assert!(on_screen(vcx, tab), "{tab}");
    }
    assert!(!page.shell.read_with(vcx, |s, cx| s.lyrics_showing(cx)));
    assert!(!on_screen(vcx, "queue-panel"));

    // Lyrics takes the side from Related (and the Lyrics button reads as on).
    click(vcx, "pane-tab-Lyrics");
    assert_eq!(pane_of(&page, vcx), Pane::Lyrics);
    assert!(!page.pane.read_with(vcx, |p, _| p.is_open()));
    assert!(on_screen(vcx, "pane-tab-Related"));

    // Back to Related, then the player bar's Queue button puts the queue back.
    click(vcx, "pane-tab-Related");
    assert_eq!(pane_of(&page, vcx), Pane::Related);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    settle(vcx);
    assert_eq!(pane_of(&page, vcx), Pane::UpNext);
    assert!(page.shell.read_with(vcx, |s, _| s.queue_open()));
    assert!(on_screen(vcx, "queue-panel"));
    // The queue carries the same control, with Up next picked.
    assert!(on_screen(vcx, "pane-tab-Up next"));

    // The Lyrics button over Related replaces it.
    show(&page, vcx, Pane::Related);
    page.shell.update(vcx, |s, cx| s.toggle_lyrics(cx));
    settle(vcx);
    assert_eq!(pane_of(&page, vcx), Pane::Lyrics);
    // ... and the queue's own control gets to Related too.
    click(vcx, "pane-tab-Up next");
    assert_eq!(pane_of(&page, vcx), Pane::UpNext);
    click(vcx, "pane-tab-Related");
    assert_eq!(pane_of(&page, vcx), Pane::Related);
}

#[gpui::test]
fn the_queue_has_no_pane_control_while_now_playing_is_closed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    settle(vcx);
    assert!(on_screen(vcx, "queue-panel"));
    assert!(!on_screen(vcx, "pane-tab-Related"));
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    settle(vcx);
    assert!(on_screen(vcx, "pane-tab-Related"));
}

#[gpui::test]
fn a_new_track_asks_again_and_the_old_shelves_stay_until_the_answer(cx: &mut TestAppContext) {
    let library = SimLibrary::new();
    let (page, vcx) = setup_with(cx, None, library);
    show(&page, vcx, Pane::Related);
    let first = page.player.read_with(vcx, |m, _| m.current_video()).unwrap();
    let shown = titles(&page, vcx);
    assert_eq!(shown.len(), 3);

    page.player.update(vcx, |m, cx| m.next(cx));
    // The simulated answer takes a moment: the request is out, the old shelves are still there.
    page.model.read_with(vcx, |m, _| {
        assert!(m.is_refreshing());
        assert_eq!(m.shown_for(), Some(first.as_str()));
        assert_eq!(m.phase(), Phase::Ready);
    });
    assert_eq!(titles(&page, vcx), shown);

    vcx.executor().advance_clock(Duration::from_secs(1));
    settle(vcx);
    let second = page.player.read_with(vcx, |m, _| m.current_video()).unwrap();
    assert_ne!(first, second);
    assert_eq!(page.library.related_log(), [first, second.clone()]);
    page.model.read_with(vcx, |m, _| {
        assert!(!m.is_refreshing());
        assert_eq!(m.shown_for(), Some(second.as_str()));
    });
}

#[gpui::test]
fn an_answer_for_a_track_that_is_gone_is_dropped(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    let requests = page.model.read_with(vcx, |m, _| m.requests());
    // Two skips in a row: only the track playing at the end is shown.
    page.player.update(vcx, |m, cx| m.next(cx));
    page.player.update(vcx, |m, cx| m.next(cx));
    settle(vcx);
    let current = page.player.read_with(vcx, |m, _| m.current_video()).unwrap();
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.requests(), requests + 2);
        assert_eq!(m.shown_for(), Some(current.as_str()));
    });
}

#[gpui::test]
fn closing_the_tab_stops_the_fetching(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    assert_eq!(page.library.related_log().len(), 1);
    show(&page, vcx, Pane::UpNext);
    page.player.update(vcx, |m, cx| m.next(cx));
    settle(vcx);
    assert_eq!(page.library.related_log().len(), 1, "hidden: not asked");
    // Closing Now Playing hides the tab too.
    show(&page, vcx, Pane::Related);
    assert_eq!(page.library.related_log().len(), 2, "shown again: asked for the new track");
    page.shell.update(vcx, |s, cx| s.set_now_playing(false, cx));
    page.player.update(vcx, |m, cx| m.next(cx));
    settle(vcx);
    assert_eq!(page.library.related_log().len(), 2);
    // Reopening picks up where the choice was left, for the track now playing.
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    settle(vcx);
    assert_eq!(page.library.related_log().len(), 3);
}

#[gpui::test]
fn the_chosen_segment_is_remembered(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    for _ in 0..200 {
        if page.flag.load() == Some(true) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(page.flag.load(), Some(true));
    show(&page, vcx, Pane::Lyrics);
    for _ in 0..200 {
        if page.flag.load() == Some(false) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(page.flag.load(), Some(false));
}

#[gpui::test]
fn a_saved_related_choice_is_applied_at_startup(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, Some(true));
    assert!(page.pane.read_with(vcx, |p, _| p.is_open()));
    // It waits for Now Playing, and asks for nothing on other pages.
    assert_eq!(pane_of(&page, vcx), Pane::UpNext);
    assert!(page.library.related_log().is_empty());
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    settle(vcx);
    assert_eq!(pane_of(&page, vcx), Pane::Related);
    assert_eq!(page.library.related_log().len(), 1);
}

#[gpui::test]
fn a_failure_says_so_with_retry_and_retry_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    page.library
        .set_related_behaviour(RelatedBehaviour::Fail(LoadFailure::Network));
    show(&page, vcx, Pane::Related);
    assert_eq!(
        page.model.read_with(vcx, |m, _| m.phase()),
        Phase::Failed(LoadFailure::Network)
    );
    assert!(on_screen(vcx, "related-error"));
    assert!(on_screen(vcx, "related-retry"));

    page.library.set_related_behaviour(RelatedBehaviour::Shelves);
    click(vcx, "related-retry");
    assert_eq!(page.model.read_with(vcx, |m, _| m.phase()), Phase::Ready);
    assert!(!on_screen(vcx, "related-error"));
    assert!(on_screen(vcx, "related-0-0"));
}

#[gpui::test]
fn a_track_with_nothing_related_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    page.library.set_related_behaviour(RelatedBehaviour::Empty);
    show(&page, vcx, Pane::Related);
    assert_eq!(page.model.read_with(vcx, |m, _| m.phase()), Phase::Ready);
    assert!(titles(&page, vcx).is_empty());
    assert!(on_screen(vcx, "related-pane"));
    assert!(!on_screen(vcx, "related-0-0"));
}

#[gpui::test]
fn clicking_a_related_song_plays_it_with_the_shelf_as_the_queue(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    let songs: Vec<String> = page.model.read_with(vcx, |m, _| {
        m.shelves()[0]
            .songs()
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    });
    assert!(songs.len() > 2);
    click(vcx, "related-0-1");
    let current = page.player.read_with(vcx, |m, _| m.current_video()).unwrap();
    assert_eq!(current, songs[1]);
    // The pane follows the song that is playing now.
    settle(vcx);
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.shown_for(), Some(songs[1].as_str()));
    });
}

#[gpui::test]
fn clicking_an_album_card_opens_its_page_and_leaves_now_playing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    show(&page, vcx, Pane::Related);
    click(vcx, "related-1-0");
    assert!(!page.shell.read_with(vcx, |s, _| s.now_playing_open()));
}

#[gpui::test]
fn the_pane_fits_beside_the_cover_at_the_minimum_window(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx, None);
    for (w, h) in [(864.0_f32, 480.0_f32), (1100.0, 720.0), (1400.0, 900.0)] {
        vcx.simulate_resize(gpui::size(gpui::px(w), gpui::px(h)));
        show(&page, vcx, Pane::Related);
        let (art, pane) = (bounds(vcx, "now-playing-art"), bounds(vcx, "np-lyrics"));
        assert!(art.right() <= pane.left(), "{w}x{h}: art {art:?} pane {pane:?}");
        assert!(pane.size.width >= gpui::px(320.0), "{w}x{h}");
        assert!(pane.right() <= gpui::px(w), "{w}x{h}");
        let tab = bounds(vcx, "pane-tab-Related");
        assert!(tab.left() >= pane.left() && tab.right() <= pane.right());
    }
}
