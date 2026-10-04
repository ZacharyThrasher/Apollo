//! Headless tests for the artist page's own features: Subscribe (a write to the account, with a
//! read-back instead of a retry when the outcome is unknown), Radio, Shuffle, "Show all",
//! similar artists and the bio. The real shell and views on GPUI's test platform, with the
//! simulated library as the "service" (it keeps who the account subscribed to).

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::{ArtistTarget, LibraryModel, Target};
use apollo_app::library::sim::{self, SimLibrary, WriteBehaviour};
use apollo_app::library::writes::LibraryWrites;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::WriteFailure;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, px, AppContext, Entity, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext,
    TouchPhase, VisualTestContext,
};

const ARTIST: &str = "Neon Arcade";

fn artist_id(name: &str) -> String {
    format!("{}{name}", sim::SEARCH_ARTIST_PREFIX)
}

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
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    view: Entity<LibraryView>,
    model: Entity<LibraryModel>,
    shell: Entity<Shell>,
    sim: SimLibrary,
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
        cx.set_reduce_motion(true);
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
        queue: vec![LibraryTrack {
            track: Track::new("startTrack01", "Start"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    // These tests are about what the artist buttons queue, so no autoplay radio joins in.
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let (writes, view) = shell.read_with(vcx, |s, _| (s.writes().clone(), s.library().clone()));
    let model = view.read_with(vcx, |v, _| v.model().clone());
    (
        Page {
            player,
            writes,
            view,
            model,
            shell,
            sim,
        },
        vcx,
    )
}

fn open_artist(page: &Page, vcx: &mut VisualTestContext, name: &str) {
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let target = Target::Artist(ArtistTarget {
        id: BrowseId::new(artist_id(name)),
        name: name.into(),
        thumbnail: None,
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open_target(target, window, cx));
    vcx.run_until_parked();
}

fn click(vcx: &mut VisualTestContext, selector: &'static str) {
    let at = vcx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

/// Wheels the page until `selector` sits clear of the title bar and the player bar.
fn reveal(vcx: &mut VisualTestContext, selector: &'static str) {
    for _ in 0..60 {
        let size = vcx.update(|window, _| window.viewport_size());
        let y = f32::from(
            vcx.debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} is drawn"))
                .center()
                .y,
        );
        if y > 120.0 && y < f32::from(size.height) - 120.0 {
            return;
        }
        vcx.simulate_event(ScrollWheelEvent {
            position: point(size.width / 2.0, size.height / 2.0),
            delta: ScrollDelta::Pixels(point(px(0.0), px(if y > 120.0 { -160.0 } else { 160.0 }))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        vcx.run_until_parked();
    }
    panic!("{selector} never came into view");
}

fn on_screen(vcx: &mut VisualTestContext, selector: &'static str) -> bool {
    vcx.debug_bounds(selector).is_some()
}

fn shown_subscribed(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.model
        .read_with(vcx, |m, _| m.artist().expect("an artist page").subscribed())
}

fn held_subscribed(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.model.read_with(vcx, |m, _| {
        m.artist()
            .and_then(|a| a.content())
            .expect("a loaded artist page")
            .subscribed
    })
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.writes
        .read_with(vcx, |w, _| w.notice().map(|n| n.text.to_string()))
}

fn current_title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player
        .read_with(vcx, |m, _| m.view().title.to_string())
}

fn shuffle_on(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.player
        .read_with(vcx, |m, _| m.coordinator().unwrap().queue().shuffle())
}

fn queue_len(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        q.history().len() + usize::from(q.current().is_some()) + q.upcoming().count()
    })
}

fn channel() -> String {
    artist_id(ARTIST)
}

fn writes_sent(page: &Page, what: &str) -> usize {
    page.sim
        .write_log()
        .iter()
        .filter(|(w, c)| w == what && *c == channel())
        .count()
}

// ---- the page ---------------------------------------------------------------------------------

#[gpui::test]
fn the_page_has_the_banner_the_four_buttons_the_shelves_and_the_bio(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    for selector in [
        "artist-banner",
        "artist-play",
        "artist-shuffle",
        "artist-radio",
        "artist-subscribe",
        "artist-show-all",
        "artist-card-0-0",
        "artist-card-1-0",
    ] {
        assert!(on_screen(vcx, selector), "{selector} is drawn");
    }
    page.model.read_with(vcx, |m, _| {
        let content = m.artist().unwrap().content().unwrap();
        assert!(content.description.is_some());
        assert_eq!(content.subscribers.as_deref(), Some("1.2M subscribers"));
        assert!(!content.related.is_empty());
        assert!(!m.artist().unwrap().bio_open, "the bio starts clamped");
    });
}

#[gpui::test]
fn similar_artists_are_cards_that_open_their_own_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    let first = page.model.read_with(vcx, |m, _| {
        m.artist().unwrap().content().unwrap().related[0].name.clone()
    });
    assert_ne!(first, ARTIST);
    reveal(vcx, "artist-fan-0");
    click(vcx, "artist-fan-0");
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.depth(), 2, "opened over the first artist");
        assert_eq!(m.artist().unwrap().name(), first);
    });
}

#[gpui::test]
fn the_bio_opens_when_clicked_and_closes_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    reveal(vcx, "artist-bio");
    let open = |page: &Page, vcx: &mut VisualTestContext| {
        page.model
            .read_with(vcx, |m, _| m.artist().unwrap().bio_open)
    };
    assert!(!open(&page, vcx));
    click(vcx, "artist-bio");
    assert!(open(&page, vcx));
    click(vcx, "artist-bio");
    assert!(!open(&page, vcx));
}

#[gpui::test]
fn show_all_opens_the_playlist_of_every_top_song_over_the_artist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    click(vcx, "artist-show-all");
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.depth(), 2);
        let d = m.detail().expect("a playlist page is open");
        assert_eq!(d.collection.title, "Top songs");
        assert_eq!(d.collection.subtitle.as_deref(), Some(ARTIST));
        assert!(!d.listing.items().is_empty());
    });
    vcx.simulate_keystrokes("escape");
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.depth(), 1);
        assert!(m.artist().is_some(), "Back returns to the artist");
    });
}

#[gpui::test]
fn play_starts_the_top_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    click(vcx, "artist-play");
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
}

// ---- radio and shuffle ------------------------------------------------------------------------

#[gpui::test]
fn radio_replaces_the_queue_with_the_artists_radio(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    click(vcx, "artist-radio");
    assert_eq!(page.sim.mix_log(), [format!("RDEMsim_{ARTIST}")]);
    assert!(current_title(&page, vcx).starts_with("Generated Track"));
    assert_eq!(queue_len(&page, vcx), 10, "the radio is the queue now");
    page.model
        .read_with(vcx, |m, _| assert!(m.artist().unwrap().mix.is_none()));
}

#[gpui::test]
fn a_radio_that_cannot_be_fetched_leaves_the_queue_alone_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim.set_radio_failure(Some(LoadFailure::Network));
    click(vcx, "artist-radio");
    assert_eq!(current_title(&page, vcx), "Start");
    assert_eq!(queue_len(&page, vcx), 1);
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't start radio"));
}

#[gpui::test]
fn shuffle_plays_the_artists_shuffle_mix_not_just_the_top_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    click(vcx, "artist-shuffle");
    assert_eq!(page.sim.mix_log(), [format!("RDAOsim_{ARTIST}")]);
    assert!(current_title(&page, vcx).starts_with("Generated Track"));
    assert!(shuffle_on(&page, vcx));
}

#[gpui::test]
fn a_shuffle_mix_that_cannot_be_had_shuffles_the_top_songs_instead(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim.set_radio_failure(Some(LoadFailure::Network));
    click(vcx, "artist-shuffle");
    assert_eq!(current_title(&page, vcx), "Midnight Drive");
    assert!(shuffle_on(&page, vcx));
    assert_eq!(notice(&page, vcx), None, "music played, so nothing to report");
}

#[gpui::test]
fn a_shuffle_asked_before_the_page_arrives_waits_and_then_uses_the_mix(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let target = Target::Artist(ArtistTarget {
        id: BrowseId::new(sim::ARTIST_SLOW_ID),
        name: ARTIST.into(),
        thumbnail: None,
    });
    page.view
        .update_in(vcx, |v, window, cx| v.open_target(target, window, cx));
    page.model.update(vcx, |m, cx| {
        m.request_play(apollo_app::library::model::PlayRequest::SHUFFLE, cx)
    });
    assert_eq!(current_title(&page, vcx), "Start");
    vcx.run_until_parked();
    assert!(page.sim.mix_log()[0].starts_with("RDAO"));
    assert!(shuffle_on(&page, vcx));
}

// ---- subscribe --------------------------------------------------------------------------------

#[gpui::test]
fn subscribing_shows_at_once_and_the_account_holds_it_afterwards(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    assert!(!shown_subscribed(&page, vcx));

    // Before the write is answered the button already shows the new state.
    page.model.update(vcx, |m, cx| m.toggle_subscribe(cx));
    assert!(shown_subscribed(&page, vcx), "shown at once");
    assert!(!held_subscribed(&page, vcx), "not yet confirmed");
    vcx.run_until_parked();
    assert!(shown_subscribed(&page, vcx));
    assert!(held_subscribed(&page, vcx), "the service answered");
    assert_eq!(page.sim.subscribed_channels(), [channel()]);
    assert_eq!(notice(&page, vcx), None, "success is silent");

    // Unsubscribing through the button undoes it.
    click(vcx, "artist-subscribe");
    assert!(!shown_subscribed(&page, vcx));
    assert!(page.sim.subscribed_channels().is_empty());
    assert_eq!(writes_sent(&page, "subscribe"), 1);
    assert_eq!(writes_sent(&page, "unsubscribe"), 1);
}

#[gpui::test]
fn a_page_loaded_for_a_subscribed_account_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.preset_subscription(&channel(), true);
    open_artist(&page, vcx, ARTIST);
    assert!(shown_subscribed(&page, vcx));
}

#[gpui::test]
fn a_refused_subscribe_is_put_back_with_a_short_error(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    click(vcx, "artist-subscribe");
    assert!(!shown_subscribed(&page, vcx));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't subscribe"));
    assert!(page.sim.subscribed_channels().is_empty());
    assert_eq!(writes_sent(&page, "subscribe"), 1, "a refusal is not retried");
}

#[gpui::test]
fn an_expired_session_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Session));
    click(vcx, "artist-subscribe");
    assert_eq!(notice(&page, vcx).as_deref(), Some("Sign-in expired"));
    assert!(!shown_subscribed(&page, vcx));
}

#[gpui::test]
fn a_lost_reply_is_read_back_not_retried_and_a_write_that_landed_stands(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim
        .set_write_behaviour(WriteBehaviour::LandsReplyLost);
    click(vcx, "artist-subscribe");
    assert!(shown_subscribed(&page, vcx), "the read-back found it landed");
    assert!(held_subscribed(&page, vcx));
    assert_eq!(notice(&page, vcx), None);
    assert_eq!(
        writes_sent(&page, "subscribe"),
        1,
        "sent once; never sent again"
    );
}

#[gpui::test]
fn a_lost_reply_for_a_write_that_did_not_land_rolls_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    click(vcx, "artist-subscribe");
    assert!(!shown_subscribed(&page, vcx), "the read-back says it did not");
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't subscribe"));
    assert_eq!(writes_sent(&page, "subscribe"), 1, "never sent again");
}

#[gpui::test]
fn a_lost_reply_whose_read_back_fails_too_rolls_back_and_still_sends_once(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    page.sim.set_artist_failure(Some(LoadFailure::Network));
    click(vcx, "artist-subscribe");
    assert!(!shown_subscribed(&page, vcx));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't subscribe"));
    assert_eq!(writes_sent(&page, "subscribe"), 1);
}

#[gpui::test]
fn the_button_ignores_presses_while_a_write_is_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.model.update(vcx, |m, cx| m.toggle_subscribe(cx));
    // A second press before the answer must not queue an unsubscribe behind it.
    page.model.update(vcx, |m, cx| m.toggle_subscribe(cx));
    vcx.run_until_parked();
    assert!(shown_subscribed(&page, vcx));
    assert_eq!(writes_sent(&page, "subscribe"), 1);
    assert_eq!(writes_sent(&page, "unsubscribe"), 0);
}

#[gpui::test]
fn an_answer_for_the_old_account_is_dropped_after_a_change_of_account(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.model.update(vcx, |m, cx| m.toggle_subscribe(cx));
    // The account changes before the answer arrives: every page closes.
    page.view.update(vcx, |v, cx| v.reset(cx));
    vcx.run_until_parked();
    assert_eq!(page.model.read_with(vcx, |m, _| m.depth()), 0);
    assert_eq!(notice(&page, vcx), None, "nothing of the old page reaches the new account");
}

#[gpui::test]
fn a_page_with_no_channel_to_subscribe_to_has_a_dead_button(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    // The empty artist has nothing to show at all, so the page offers no button row.
    page.model.update(vcx, |m, cx| m.close(cx));
    page.view
        .update_in(vcx, |v, window, cx| {
            v.open_target(
                Target::Artist(ArtistTarget {
                    id: BrowseId::new(sim::ARTIST_EMPTY_ID),
                    name: "Quiet".into(),
                    thumbnail: None,
                }),
                window,
                cx,
            )
        });
    vcx.run_until_parked();
    page.model.update(vcx, |m, cx| m.toggle_subscribe(cx));
    vcx.run_until_parked();
    assert!(page.sim.write_log().is_empty(), "no write for a page that cannot subscribe");
}

// ---- layout -----------------------------------------------------------------------------------

#[gpui::test]
fn the_banner_is_280px_tall_the_buttons_sit_under_it_in_a_row_and_nothing_shifts(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_artist(&page, vcx, ARTIST);
    let bounds = |vcx: &mut VisualTestContext, s: &'static str| {
        vcx.debug_bounds(s)
            .unwrap_or_else(|| panic!("{s} is drawn"))
    };
    let banner = bounds(vcx, "artist-banner");
    assert_eq!(f32::from(banner.size.height), 280.0);
    assert!(f32::from(banner.size.width) > 400.0, "as wide as the page");

    let names = ["artist-play", "artist-shuffle", "artist-radio", "artist-subscribe"];
    let row: Vec<_> = names.iter().map(|n| bounds(vcx, n)).collect();
    for b in &row {
        assert!(b.top() >= banner.bottom(), "the buttons sit under the banner");
        assert_eq!(f32::from(b.size.height), 32.0);
    }
    for pair in row.windows(2) {
        assert!(pair[0].right() <= pair[1].left(), "left to right, no overlap");
    }
    assert!(row.iter().all(|b| b.top() == row[0].top()), "one row");

    // "Subscribe" and "Subscribed" are the same width, so nothing moves when it toggles.
    let before = bounds(vcx, "artist-subscribe").size.width;
    click(vcx, "artist-subscribe");
    assert!(shown_subscribed(&page, vcx));
    assert_eq!(bounds(vcx, "artist-subscribe").size.width, before);
    assert_eq!(bounds(vcx, "artist-radio").left(), row[2].left());
}
