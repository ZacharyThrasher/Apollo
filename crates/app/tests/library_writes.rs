//! Headless tests for library writes: liking and unliking tracks and adding a track to a
//! playlist. The real shell and views on GPUI's test platform, the simulated library as the
//! "service" (it keeps what was written, so the tests read the result back from it) and the
//! deterministic mock playback adapters.
//!
//! A click or a key press parks the executor, so a write has landed by the time it returns;
//! the optimistic state is read by calling the model directly, before parking.

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::model::{LibraryModel, Load, Tab};
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::library::writes::LibraryWrites;
use apollo_app::player::PlayerModel;
use apollo_app::settings::{SettingsModel, PICKER_KEY, PICKER_STAY_OPEN, RECENT_PLAYLISTS};
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::playlist_picker::{self, PlaylistPicker};
use apollo_app::ui::toast;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{Collection, WriteFailure, LIKED_MUSIC_BROWSE_ID};
use apollo_domain::listing::{LoadFailure, LoadState};
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchResults};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    AppContext, Bounds, Entity, Focusable, Modifiers, Pixels, TestAppContext, VisualTestContext,
};

/// The track that is playing at the start: in the simulated Liked Music.
const LIKED: &str = "simTrack001";
/// A track that is not liked and is up next.
const PLAIN: &str = "plainTrack1";

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

/// Two songs for any query: the liked fixture and a plain one.
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
            songs: vec![song(PLAIN, "Plain Song"), song("otherTrack1", "Other Song")],
            ..SearchResults::default()
        })
    }
}

struct Page {
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    writes: Entity<LibraryWrites>,
    picker: Entity<PlaylistPicker>,
    view: Entity<LibraryView>,
    model: Entity<LibraryModel>,
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
                track: Track::new(LIKED, "Midnight Drive"),
            },
            LibraryTrack {
                track: Track::new(PLAIN, "Plain Song"),
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
    let (writes, picker, view) = shell.read_with(vcx, |s, _| {
        (s.writes().clone(), s.picker().clone(), s.library().clone())
    });
    let model = view.read_with(vcx, |v, _| v.model().clone());
    (
        Page {
            player,
            shell,
            writes,
            picker,
            view,
            model,
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

fn liked(page: &Page, vcx: &mut VisualTestContext, id: &str) -> Option<bool> {
    page.writes.read_with(vcx, |w, _| w.liked(id))
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.writes
        .read_with(vcx, |w, _| w.notice().map(|n| n.text.to_string()))
}

fn log(page: &Page) -> Vec<(String, String)> {
    page.sim.write_log()
}

fn entry(what: &str, video: &str) -> (String, String) {
    (what.to_owned(), video.to_owned())
}

fn next_track(page: &Page, vcx: &mut VisualTestContext) {
    page.player.update(vcx, |m, cx| m.next(cx));
    vcx.run_until_parked();
}

fn collection(page: &Page, vcx: &mut VisualTestContext, id: &str) -> Collection {
    page.model.read_with(vcx, |m, _| {
        [Tab::Playlists, Tab::Albums]
            .into_iter()
            .flat_map(|t| m.list(t).iter())
            .find(|c| c.id.as_str() == id)
            .cloned()
            .unwrap_or_else(|| panic!("no collection {id}"))
    })
}

fn open_liked_music(page: &Page, vcx: &mut VisualTestContext) {
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    let c = collection(page, vcx, LIKED_MUSIC_BROWSE_ID);
    page.view
        .update_in(vcx, |v, window, cx| v.open(c, window, cx));
    vcx.run_until_parked();
}

fn liked_listing(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
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

// ---- like state ------------------------------------------------------------------------------

#[gpui::test]
fn the_playing_tracks_like_state_is_read_from_the_service(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(liked(&page, vcx, LIKED), Some(true));
    assert_eq!(liked(&page, vcx, PLAIN), None, "not asked about yet");
    next_track(&page, vcx);
    assert_eq!(
        liked(&page, vcx, PLAIN),
        Some(false),
        "asked once it is playing"
    );
}

#[gpui::test]
fn liking_shows_at_once_and_the_service_holds_it_afterwards(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    // Before anything has run: the heart is already filled, and nothing has been sent.
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert!(log(&page).is_empty());
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("like", PLAIN)]);
    assert_eq!(page.sim.liked_ids()[0], PLAIN, "most recent first");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert_eq!(notice(&page, vcx), None, "success is silence");

    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
    vcx.run_until_parked();
    assert!(!page.sim.liked_ids().contains(&PLAIN.to_owned()));
    assert_eq!(log(&page), [entry("like", PLAIN), entry("unlike", PLAIN)]);
}

#[gpui::test]
fn the_player_bar_heart_likes_the_playing_track(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    click(vcx, "bar-like");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert_eq!(log(&page), [entry("like", PLAIN)]);
    click(vcx, "bar-like");
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
    assert_eq!(log(&page), [entry("like", PLAIN), entry("unlike", PLAIN)]);
}

#[gpui::test]
fn now_playing_has_the_heart_and_the_playlist_picker(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.shell.update(vcx, |s, cx| s.set_now_playing(true, cx));
    vcx.run_until_parked();
    click(vcx, "np-like");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert_eq!(log(&page), [entry("like", PLAIN)]);

    click(vcx, "np-playlist");
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()));
    assert_eq!(page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)), Some(PLAIN.to_owned()));
}

#[gpui::test]
fn a_refused_like_is_put_back_with_a_short_error(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    assert_eq!(liked(&page, vcx, PLAIN), Some(true), "optimistic first");
    vcx.run_until_parked();
    assert_eq!(liked(&page, vcx, PLAIN), Some(false), "rolled back");
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't like this song"));
    assert!(on_screen(vcx, "toast"));
    assert!(!page.sim.liked_ids().contains(&PLAIN.to_owned()));
    // A definite no is not read back and not retried.
    assert_eq!(log(&page).len(), 1);
}

#[gpui::test]
fn a_failed_unlike_puts_the_heart_back_on(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Throttled));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(LIKED, cx));
    assert_eq!(liked(&page, vcx, LIKED), Some(false));
    vcx.run_until_parked();
    assert_eq!(liked(&page, vcx, LIKED), Some(true));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Too many requests"));
}

#[gpui::test]
fn an_error_goes_away_when_it_is_clicked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Session));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    vcx.run_until_parked();
    assert_eq!(notice(&page, vcx).as_deref(), Some("Sign-in expired"));
    click(vcx, "toast");
    assert_eq!(notice(&page, vcx), None);
    // It fades out for 150ms and is gone after.
    assert!(on_screen(vcx, "toast"), "still fading");
    vcx.executor().advance_clock(toast::FADE + std::time::Duration::from_millis(1));
    vcx.run_until_parked();
    assert!(!on_screen(vcx, "toast"));
}

#[gpui::test]
fn under_reduced_motion_the_error_appears_and_goes_at_once(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "toast"));
    click(vcx, "toast");
    assert!(!on_screen(vcx, "toast"), "no fade-out");
}

#[gpui::test]
fn a_new_error_replaces_one_that_is_fading_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    vcx.run_until_parked();
    click(vcx, "toast");
    page.writes
        .update(vcx, |w, cx| w.toggle_like(LIKED, cx));
    vcx.run_until_parked();
    vcx.executor().advance_clock(toast::FADE + std::time::Duration::from_millis(1));
    vcx.run_until_parked();
    assert!(notice(&page, vcx).is_some());
    assert!(on_screen(vcx, "toast"), "the new error is not cleared by the old fade");
}

#[gpui::test]
fn an_error_leaves_by_itself(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    vcx.run_until_parked();
    assert!(notice(&page, vcx).is_some());
    vcx.executor()
        .advance_clock(apollo_app::library::writes::NOTICE_FOR + std::time::Duration::from_millis(1));
    vcx.run_until_parked();
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn a_lost_reply_is_read_back_not_retried_and_a_write_that_landed_stands(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("like", PLAIN)], "sent once, never retried");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true), "the read-back found it liked");
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn a_lost_reply_for_a_write_that_did_not_land_rolls_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("like", PLAIN)], "sent once, never retried");
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't like this song"));
}

#[gpui::test]
fn toggling_while_a_write_is_out_never_reorders_requests(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    // Like, unlike: two writes, in that order.
    page.writes.update(vcx, |w, cx| {
        w.toggle_like(PLAIN, cx);
        w.toggle_like(PLAIN, cx);
    });
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("like", PLAIN), entry("unlike", PLAIN)]);
    assert!(!page.sim.liked_ids().contains(&PLAIN.to_owned()));
    // Like, unlike, like: the middle one never needed to go out.
    page.writes.update(vcx, |w, cx| {
        w.toggle_like(PLAIN, cx);
        w.toggle_like(PLAIN, cx);
        w.toggle_like(PLAIN, cx);
    });
    vcx.run_until_parked();
    assert_eq!(
        log(&page)[2..],
        [entry("like", PLAIN)],
        "the wish that ended where the write began sends nothing more"
    );
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert!(page.sim.liked_ids().contains(&PLAIN.to_owned()));
}

#[gpui::test]
fn rows_loaded_from_the_service_never_undo_a_change_made_here(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    vcx.run_until_parked();
    page.writes
        .update(vcx, |w, cx| w.seed([(PLAIN.to_owned(), false)], cx));
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    // A track nothing is known about takes the row's word.
    page.writes
        .update(vcx, |w, cx| w.seed([("rowTrack001".to_owned(), true)], cx));
    assert_eq!(liked(&page, vcx, "rowTrack001"), Some(true));
}

#[gpui::test]
fn an_answer_for_the_old_account_is_dropped_after_a_change_of_account(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    page.writes.update(vcx, |w, cx| w.reset(cx));
    vcx.run_until_parked();
    assert_eq!(liked(&page, vcx, PLAIN), None, "nothing of the old account stays");
    assert_eq!(notice(&page, vcx), None, "and its failure is not shown to the new one");
}

// ---- Liked Music ------------------------------------------------------------------------------

#[gpui::test]
fn opening_liked_music_marks_every_row_liked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    for id in liked_listing(&page, vcx) {
        assert_eq!(liked(&page, vcx, &id), Some(true), "{id}");
    }
}

#[gpui::test]
fn unliking_drops_the_row_from_an_open_liked_music_without_a_reload(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    assert_eq!(liked_listing(&page, vcx).len(), 6);
    // The keyboard is on the first row; L toggles its heart.
    vcx.simulate_keystrokes("l");
    let rows = liked_listing(&page, vcx);
    assert_eq!(rows.len(), 5);
    assert!(!rows.contains(&LIKED.to_owned()));
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.detail().unwrap().listing.state(), LoadState::Complete);
    });
    assert_eq!(page.sim.liked_ids().len(), 5, "the service agrees");
}

#[gpui::test]
fn liking_adds_the_track_to_an_open_liked_music_at_the_top(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    page.writes
        .update(vcx, |w, cx| w.set_liked(PLAIN, true, cx));
    vcx.run_until_parked();
    let rows = liked_listing(&page, vcx);
    assert_eq!(rows.len(), 7);
    assert_eq!(rows[0], PLAIN, "most recently liked first, as the service lists it");
    page.model.read_with(vcx, |m, _| {
        assert_eq!(m.detail().unwrap().listing.state(), LoadState::Complete);
    });
}

#[gpui::test]
fn a_like_that_fails_leaves_liked_music_as_it_was(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.set_liked(PLAIN, true, cx));
    vcx.run_until_parked();
    assert_eq!(liked_listing(&page, vcx).len(), 6);
}

#[gpui::test]
fn liked_music_opened_later_lists_what_was_liked_meanwhile(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.writes
        .update(vcx, |w, cx| w.set_liked(PLAIN, true, cx));
    vcx.run_until_parked();
    open_liked_music(&page, vcx);
    assert_eq!(liked_listing(&page, vcx)[0], PLAIN);
}

// ---- rows ---------------------------------------------------------------------------------------

#[gpui::test]
fn a_rows_heart_toggles_the_like_and_does_not_play_the_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    // Select the third row: its actions are showing.
    vcx.simulate_keystrokes("down down");
    let id = liked_listing(&page, vcx)[2].clone();
    click(vcx, "track-like-2");
    assert_eq!(liked(&page, vcx, &id), Some(false));
    assert_eq!(log(&page), [entry("unlike", &id)]);
    let title = page.player.read_with(vcx, |m, _| m.view().title.to_string());
    assert_eq!(title, "Midnight Drive", "the click did not start the row");
}

#[gpui::test]
fn a_rows_playlist_action_opens_the_picker_for_that_track(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    // Hover the row (the mouse takes over from the keyboard), let the hover fade in, then click.
    let row = bounds(vcx, "track-playlist-2").center();
    vcx.simulate_mouse_move(row, None, Modifiers::none());
    vcx.executor().advance_clock(std::time::Duration::from_millis(400));
    vcx.run_until_parked();
    let id = liked_listing(&page, vcx)[2].clone();
    click(vcx, "track-playlist-2");
    assert_eq!(page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)), Some(id));
    assert!(on_screen(vcx, "playlist-picker"));
}

#[gpui::test]
fn the_keyboard_adds_the_selected_row_to_a_playlist_with_p(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    vcx.simulate_keystrokes("p");
    let video = liked_listing(&page, vcx)[0].clone();
    assert_eq!(page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)), Some(video));
}

#[gpui::test]
fn search_songs_have_the_same_actions(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update_in(vcx, |s, window, cx| {
        s.choose_route(Route::Search, window, cx);
    });
    let search = page.shell.read_with(vcx, |s, _| s.search_model().clone());
    // Typed, as a person would: a field left empty offers its recent searches under it.
    let field = page
        .shell
        .read_with(vcx, |s, cx| s.search_page().read(cx).field().clone());
    field.update(vcx, |f, cx| f.set_text("anything", cx));
    search.update(vcx, |m, cx| m.submit(cx));
    vcx.run_until_parked();
    // Down from the field selects the first song.
    vcx.simulate_keystrokes("down");
    vcx.simulate_keystrokes("l");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert_eq!(log(&page), [entry("like", PLAIN)]);
    // The heart stays on a liked row; click it to unlike.
    click(vcx, "song-like-0");
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
    vcx.simulate_keystrokes("p");
    assert_eq!(
        page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)),
        Some(PLAIN.to_owned())
    );
}

// ---- the picker ---------------------------------------------------------------------------------

fn open_picker(page: &Page, vcx: &mut VisualTestContext, video: &str) {
    page.picker
        .update_in(vcx, |p, window, cx| p.open(video, "A Song", window, cx));
    vcx.run_until_parked();
}

fn picker_len(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.picker.read_with(vcx, |p, _| match p.playlists() {
        Some(Load::Ready(list)) => list.len(),
        other => panic!("not loaded: {other:?}"),
    })
}

#[gpui::test]
fn the_picker_lists_the_accounts_playlists_and_marks_the_ones_holding_the_track(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    page.picker
        .update_in(vcx, |p, window, cx| p.open(LIKED, "Midnight Drive", window, cx));
    assert!(
        matches!(page.picker.read_with(vcx, |p, _| p.playlists().cloned().map(|l| matches!(l, Load::Loading))), Some(true)),
        "a skeleton shows while the list loads"
    );
    vcx.run_until_parked();
    assert!(picker_len(&page, vcx) > 6, "long enough to scroll");
    page.picker.read_with(vcx, |p, _| {
        let Some(Load::Ready(list)) = p.playlists() else {
            unreachable!()
        };
        assert_eq!(list[0].title, "Road Trip");
        assert!(list[0].holds_track, "Road Trip already has this one");
        assert!(!list[1].holds_track);
        assert!(!list[0].id.starts_with("VL"), "bare ids, as the edit takes them");
    });
    assert!(on_screen(vcx, "playlist-picker"));
    assert!(on_screen(vcx, "picker-row-0"));
}

#[gpui::test]
fn choosing_a_playlist_adds_the_track_closes_the_picker_and_shows_it_held(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    vcx.simulate_keystrokes("down enter"); // second playlist: Long Haul
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
    assert_eq!(page.sim.added(), [("PLsimlong".to_owned(), PLAIN.to_owned())]);
    assert_eq!(notice(&page, vcx), None);
    // Asked again, the list says it holds it.
    open_picker(&page, vcx, PLAIN);
    page.picker.read_with(vcx, |p, _| {
        let Some(Load::Ready(list)) = p.playlists() else {
            unreachable!()
        };
        assert!(list[1].holds_track);
    });
}

#[gpui::test]
fn the_add_is_shown_before_the_service_answers_and_rolled_back_when_it_refuses(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let target = page.picker.read_with(vcx, |p, _| match p.playlists() {
        Some(Load::Ready(list)) => list[1].clone(),
        _ => unreachable!(),
    });
    page.picker
        .update_in(vcx, |p, window, cx| p.choose(1, window, cx));
    assert!(
        page.writes.read_with(vcx, |w, _| w.holds(&target, PLAIN)),
        "optimistic"
    );
    vcx.run_until_parked();
    assert!(!page.writes.read_with(vcx, |w, _| w.holds(&target, PLAIN)));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't add to playlist"));
    assert!(page.sim.added().is_empty());
}

#[gpui::test]
fn an_add_whose_reply_was_lost_is_read_back_and_kept_when_it_landed(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    let target = page.picker.read_with(vcx, |p, _| match p.playlists() {
        Some(Load::Ready(list)) => list[1].clone(),
        _ => unreachable!(),
    });
    page.picker
        .update_in(vcx, |p, window, cx| p.choose(1, window, cx));
    vcx.run_until_parked();
    assert!(page.writes.read_with(vcx, |w, _| w.holds(&target, PLAIN)));
    assert_eq!(notice(&page, vcx), None);
    assert_eq!(log(&page).len(), 1, "never retried");
}

#[gpui::test]
fn a_playlist_that_already_holds_the_track_does_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, LIKED);
    vcx.simulate_keystrokes("enter"); // Road Trip, which has it
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()), "stays open");
    assert!(log(&page).is_empty());
}

#[gpui::test]
fn escape_and_a_click_outside_close_the_picker_without_writing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    vcx.simulate_keystrokes("escape");
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));

    open_picker(&page, vcx, PLAIN);
    let layer = bounds(vcx, "playlist-picker-layer");
    vcx.simulate_click(layer.origin + gpui::point(gpui::px(4.0), gpui::px(4.0)), Modifiers::none());
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
    assert!(log(&page).is_empty());
}

#[gpui::test]
fn a_click_inside_the_panel_does_not_close_it_and_a_row_click_adds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    let panel = bounds(vcx, "playlist-picker");
    vcx.simulate_click(panel.origin + gpui::point(gpui::px(8.0), gpui::px(8.0)), Modifiers::none());
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()));
    click(vcx, "picker-row-2");
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
    assert_eq!(page.sim.added(), [("PLsimempty".to_owned(), PLAIN.to_owned())]);
}

#[gpui::test]
fn tab_stays_inside_the_picker_and_focus_returns_when_it_closes(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    let list_focus = page.view.read_with(vcx, |v, cx| v.focus_handle(cx));
    vcx.simulate_keystrokes("p");
    let picker_focus = page.picker.read_with(vcx, |p, cx| p.focus_handle(cx));
    vcx.run_until_parked();
    assert!(vcx.update(|window, _| picker_focus.is_focused(window)));
    vcx.simulate_keystrokes("tab tab shift-tab");
    assert!(vcx.update(|window, _| picker_focus.is_focused(window)));
    vcx.simulate_keystrokes("escape");
    assert!(
        vcx.update(|window, _| list_focus.is_focused(window)),
        "the list the picker was opened from has the keyboard again"
    );
}

#[gpui::test]
fn the_pickers_cursor_stops_at_both_ends(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    vcx.simulate_keystrokes("up");
    assert_eq!(page.picker.read_with(vcx, |p, _| p.cursor()), Some(0));
    let last = picker_len(&page, vcx) - 1;
    for _ in 0..picker_len(&page, vcx) + 3 {
        vcx.simulate_keystrokes("down");
    }
    assert_eq!(page.picker.read_with(vcx, |p, _| p.cursor()), Some(last));
}

fn type_filter(page: &Page, vcx: &mut VisualTestContext, text: &str) {
    let field = page.picker.read_with(vcx, |p, _| p.field().clone());
    field.update(vcx, |f, cx| f.set_text(text, cx));
    vcx.run_until_parked();
}

fn picker_settings(page: &Page, vcx: &mut VisualTestContext) -> Entity<SettingsModel> {
    page.shell.read_with(vcx, |s, cx| s.settings(cx))
}

fn sorted(mut pairs: Vec<(String, String)>) -> Vec<(String, String)> {
    pairs.sort();
    pairs
}

#[gpui::test]
fn the_picker_opens_with_the_caret_in_the_filter_box(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    let field = page.picker.read_with(vcx, |p, _| p.field().clone());
    let focused = vcx.update(|window, cx| field.read(cx).focus_handle(cx).is_focused(window));
    assert!(focused);
    assert!(on_screen(vcx, "picker-filter"));
    assert_eq!(page.picker.read_with(vcx, |p, _| p.songs().len()), 1);
}

#[gpui::test]
fn typing_narrows_the_list_and_enter_adds_to_the_first_match(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    type_filter(&page, vcx, "long");
    let rows = page.picker.read_with(vcx, |p, cx| p.layout(cx)).unwrap();
    assert_eq!(rows.rest, vec![1, 3], "Long Haul and the long title, in the service's order");
    assert_eq!(page.picker.read_with(vcx, |p, _| p.cursor()), Some(0), "the cursor starts on the first");
    vcx.simulate_keystrokes("enter");
    assert_eq!(page.sim.added(), [("PLsimlong".to_owned(), PLAIN.to_owned())]);
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
}

#[gpui::test]
fn a_name_that_matches_nothing_and_no_playlist_shows_only_the_create_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    type_filter(&page, vcx, "zzz nothing");
    assert!(on_screen(vcx, "picker-create"));
    assert!(!on_screen(vcx, "picker-row-0"));
    // A name that is a playlist already offers no second one.
    type_filter(&page, vcx, "long haul");
    assert!(!on_screen(vcx, "picker-create"));
}

#[gpui::test]
fn space_marks_and_enter_adds_to_every_marked_playlist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    vcx.simulate_keystrokes("space");
    assert_eq!(page.picker.read_with(vcx, |p, _| p.marked().to_vec()), ["PLsimroad"]);
    assert!(log(&page).is_empty(), "a mark writes nothing");
    vcx.simulate_keystrokes("down down space");
    assert_eq!(
        page.picker.read_with(vcx, |p, _| p.marked().to_vec()),
        ["PLsimroad", "PLsimempty"]
    );
    // A second Space takes a mark back.
    vcx.simulate_keystrokes("space");
    assert_eq!(page.picker.read_with(vcx, |p, _| p.marked().to_vec()), ["PLsimroad"]);
    vcx.simulate_keystrokes("space up enter");
    assert_eq!(
        sorted(page.sim.added()),
        [
            ("PLsimempty".to_owned(), PLAIN.to_owned()),
            ("PLsimroad".to_owned(), PLAIN.to_owned()),
        ],
        "both marked playlists, not the one the cursor ended on"
    );
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
}

#[gpui::test]
fn once_something_is_typed_space_is_a_space_and_ctrl_space_marks(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    type_filter(&page, vcx, "long");
    vcx.simulate_keystrokes("space");
    let field = page.picker.read_with(vcx, |p, _| p.field().clone());
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "long ");
    assert!(page.picker.read_with(vcx, |p, _| p.marked().is_empty()));
    vcx.simulate_keystrokes("ctrl-space");
    assert_eq!(page.picker.read_with(vcx, |p, _| p.marked().to_vec()), ["PLsimlong"]);
}

#[gpui::test]
fn a_playlist_that_holds_the_song_takes_no_mark(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, LIKED);
    vcx.simulate_keystrokes("space"); // Road Trip already has it
    assert!(page.picker.read_with(vcx, |p, _| p.marked().is_empty()));
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()));
}

#[gpui::test]
fn the_playlists_added_to_come_first_next_time(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    vcx.simulate_keystrokes("down down enter"); // Nothing Yet
    let settings = picker_settings(&page, vcx);
    assert_eq!(
        settings.read_with(vcx, |s, _| s.get_text(&RECENT_PLAYLISTS)),
        "PLsimempty"
    );
    open_picker(&page, vcx, LIKED);
    let rows = page.picker.read_with(vcx, |p, cx| p.layout(cx)).unwrap();
    assert_eq!(rows.recent, vec![2]);
    assert_eq!(rows.row(0), Some(playlist_picker::Row::Playlist(2)));
    assert_eq!(rows.rest.first(), Some(&0), "the rest keep the service's order");
    vcx.simulate_keystrokes("down down enter"); // the third row is "Long Haul"
    assert_eq!(
        settings.read_with(vcx, |s, _| s.get_text(&RECENT_PLAYLISTS)),
        "PLsimlong,PLsimempty"
    );
}

#[gpui::test]
fn keep_open_leaves_the_picker_up_with_the_marks_and_filter_cleared(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let settings = picker_settings(&page, vcx);
    settings.update(vcx, |s, cx| s.set(&PICKER_STAY_OPEN, true, cx));
    open_picker(&page, vcx, PLAIN);
    type_filter(&page, vcx, "long");
    vcx.simulate_keystrokes("enter");
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()), "stays open");
    let field = page.picker.read_with(vcx, |p, _| p.field().clone());
    assert_eq!(field.read_with(vcx, |f, _| f.text().to_owned()), "", "filter cleared");
    // The check shows at once, and the next playlist goes in on the same visit.
    let long = page.picker.read_with(vcx, |p, cx| p.holds_all(1, cx));
    assert!(long);
    vcx.simulate_keystrokes("space enter"); // first row: the recent one, Long Haul, is held
    assert!(page.picker.read_with(vcx, |p, _| p.marked().is_empty()));
    vcx.simulate_keystrokes("down space enter");
    assert_eq!(
        sorted(page.sim.added()),
        [
            ("PLsimlong".to_owned(), PLAIN.to_owned()),
            ("PLsimroad".to_owned(), PLAIN.to_owned()),
        ]
    );
    vcx.simulate_keystrokes("escape");
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
}

#[gpui::test]
fn escape_closes_even_with_text_in_the_filter(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    type_filter(&page, vcx, "road");
    vcx.simulate_keystrokes("escape");
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
    assert!(log(&page).is_empty());
}

#[gpui::test]
fn several_songs_go_into_a_playlist_one_after_another(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let songs = vec![
        playlist_picker::PickerSong::new(PLAIN, "Plain Song"),
        playlist_picker::PickerSong::new("otherTrk01", "Other"),
        playlist_picker::PickerSong::new("otherTrk02", "Another"),
    ];
    page.picker
        .update_in(vcx, |p, window, cx| p.open_songs(songs, None, window, cx));
    vcx.run_until_parked();
    assert!(on_screen(vcx, "picker-caption"));
    let target = page.picker.read_with(vcx, |p, _| match p.playlists() {
        Some(Load::Ready(list)) => list[0].clone(),
        _ => unreachable!(),
    });
    vcx.simulate_keystrokes("enter"); // Road Trip
    assert!(
        page.writes.read_with(vcx, |w, _| {
            w.holds(&target, PLAIN)
                && w.added_here(&target, "otherTrk01")
                && w.added_here(&target, "otherTrk02")
        }),
        "every song shows as held at once"
    );
    assert_eq!(page.sim.added().len(), 1, "but the writes are paced: one has gone out");
    for _ in 0..2 {
        vcx.executor()
            .advance_clock(apollo_app::library::writes::ADD_PACE);
        vcx.run_until_parked();
    }
    assert_eq!(
        page.sim.added(),
        [PLAIN, "otherTrk01", "otherTrk02"]
            .map(|v| ("PLsimroad".to_owned(), v.to_owned()))
            .to_vec()
    );
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn p_opens_the_picker_for_the_playing_song_and_the_setting_switches_it_off(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("p");
    vcx.run_until_parked();
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()));
    assert_eq!(
        page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)),
        Some(LIKED.to_owned()),
        "the song that is playing"
    );
    vcx.simulate_keystrokes("escape");
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));

    let settings = picker_settings(&page, vcx);
    settings.update(vcx, |s, cx| s.set(&PICKER_KEY, false, cx));
    vcx.simulate_keystrokes("p");
    vcx.run_until_parked();
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()), "P does nothing while it is off");
}

#[gpui::test]
fn p_in_a_list_still_opens_it_for_the_cursor_row_and_follows_the_switch(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    vcx.simulate_keystrokes("p");
    vcx.run_until_parked();
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()));
    vcx.simulate_keystrokes("escape");
    let settings = picker_settings(&page, vcx);
    settings.update(vcx, |s, cx| s.set(&PICKER_KEY, false, cx));
    vcx.simulate_keystrokes("p");
    vcx.run_until_parked();
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
}

#[gpui::test]
fn a_list_that_fails_to_load_says_so_and_retry_asks_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_playlists_failure(Some(LoadFailure::Network));
    open_picker(&page, vcx, PLAIN);
    assert!(matches!(
        page.picker.read_with(vcx, |p, _| p.playlists().cloned()),
        Some(Load::Failed(LoadFailure::Network))
    ));
    page.sim.set_playlists_failure(None);
    page.picker.update(vcx, |p, cx| p.retry(cx));
    vcx.run_until_parked();
    assert!(picker_len(&page, vcx) > 0);
}

#[gpui::test]
fn a_change_of_account_closes_the_picker(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    page.picker.update(vcx, |p, cx| p.reset(cx));
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
    assert!(!on_screen(vcx, "playlist-picker"));
}

// ---- the popover ---------------------------------------------------------------------------------

fn hover_row_actions(vcx: &mut VisualTestContext, selector: &str) {
    let at = bounds(vcx, selector).center();
    vcx.simulate_mouse_move(at, None, Modifiers::none());
    vcx.executor().advance_clock(std::time::Duration::from_millis(400));
    vcx.run_until_parked();
}

#[gpui::test]
fn the_picker_opens_as_a_popover_under_the_button_that_opened_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    hover_row_actions(vcx, "track-playlist-2");
    let button = bounds(vcx, "track-playlist-2");
    click(vcx, "track-playlist-2");
    let panel = bounds(vcx, "playlist-picker");
    assert_eq!(panel.size.width, gpui::px(playlist_picker::PANEL_WIDTH));
    assert_eq!(playlist_picker::PANEL_WIDTH, 280.0);
    assert!(panel.size.height <= gpui::px(playlist_picker::PANEL_MAX_HEIGHT));
    assert!(panel.origin.y >= button.bottom(), "below the button");
    assert!(
        (panel.right() - button.right()).abs() < gpui::px(1.0),
        "right edges line up"
    );
}

#[gpui::test]
fn the_p_key_anchors_the_popover_to_the_selected_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    vcx.simulate_keystrokes("down p");
    let slot = bounds(vcx, "track-playlist-1");
    let panel = bounds(vcx, "playlist-picker");
    assert!(panel.origin.y >= slot.bottom());
    assert!((panel.right() - slot.right()).abs() < gpui::px(1.0));
}

#[gpui::test]
fn without_an_anchor_the_popover_is_centred(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_picker(&page, vcx, PLAIN);
    let layer = bounds(vcx, "playlist-picker-layer");
    let panel = bounds(vcx, "playlist-picker");
    assert!((panel.center().x - layer.center().x).abs() < gpui::px(1.0));
    assert!((panel.center().y - layer.center().y).abs() < gpui::px(1.0));
}

#[gpui::test]
fn a_click_on_the_page_behind_the_popover_closes_it_and_a_scrim_is_not_drawn(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    open_liked_music(&page, vcx);
    vcx.simulate_keystrokes("p");
    assert!(page.picker.read_with(vcx, |p, _| p.is_open()));
    let layer = bounds(vcx, "playlist-picker-layer");
    vcx.simulate_click(layer.center(), Modifiers::none());
    assert!(!page.picker.read_with(vcx, |p, _| p.is_open()));
    assert!(log(&page).is_empty());
}

// ---- the queue -----------------------------------------------------------------------------------

fn open_queue(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn a_queue_rows_heart_likes_the_track_and_does_not_play_it(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (page, vcx) = setup(cx);
    open_queue(&page, vcx);
    // Row 0 is playing, row 1 the "Up next" header, row 2 the first up-next track.
    click(vcx, "queue-like-2");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert_eq!(log(&page), [entry("like", PLAIN)]);
    let title = page.player.read_with(vcx, |m, _| m.view().title.to_string());
    assert_eq!(title, "Midnight Drive", "the click did not start the row");
    click(vcx, "queue-like-2");
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
}

#[gpui::test]
fn the_l_key_likes_the_queue_row_under_the_cursor(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (page, vcx) = setup(cx);
    open_queue(&page, vcx);
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    vcx.update(|window, cx| {
        let focus = gpui::Focusable::focus_handle(panel.read(cx), cx);
        window.focus(&focus, cx);
    });
    vcx.simulate_keystrokes("down down l");
    assert_eq!(liked(&page, vcx, PLAIN), Some(true));
    assert_eq!(log(&page), [entry("like", PLAIN)]);
}

#[gpui::test]
fn the_p_key_opens_the_picker_for_the_queue_row_under_the_cursor(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (page, vcx) = setup(cx);
    open_queue(&page, vcx);
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    vcx.update(|window, cx| {
        let focus = gpui::Focusable::focus_handle(panel.read(cx), cx);
        window.focus(&focus, cx);
    });
    vcx.simulate_keystrokes("down down p");
    vcx.run_until_parked();
    assert_eq!(
        page.picker.read_with(vcx, |p, _| p.video().map(str::to_owned)),
        Some(PLAIN.to_owned())
    );
}

#[gpui::test]
fn a_queue_rows_heart_follows_a_like_made_elsewhere(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let (page, vcx) = setup(cx);
    open_queue(&page, vcx);
    page.writes
        .update(vcx, |w, cx| w.set_liked(PLAIN, true, cx));
    vcx.run_until_parked();
    // The row now reads "Unlike": clicking it removes the like.
    click(vcx, "queue-like-2");
    assert_eq!(liked(&page, vcx, PLAIN), Some(false));
}

// ---- not interested --------------------------------------------------------------------------

#[gpui::test]
fn not_interested_is_sent_once_and_a_liked_song_stops_being_liked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.writes
        .update(vcx, |w, cx| w.not_interested(LIKED, cx));
    assert_eq!(liked(&page, vcx, LIKED), Some(false), "the heart goes at once");
    assert!(page.writes.read_with(vcx, |w, _| w.is_disliking(LIKED)));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("dislike", LIKED)]);
    assert_eq!(page.sim.disliked_ids(), [LIKED.to_owned()]);
    assert!(!page.sim.liked_ids().contains(&LIKED.to_owned()), "the service dropped the like");
    assert_eq!(liked(&page, vcx, LIKED), Some(false));
    assert!(page.writes.read_with(vcx, |w, _| w.is_disliked(LIKED) && !w.is_disliking(LIKED)));
    assert_eq!(notice(&page, vcx), None, "success is silent");
    // Asking again sends nothing.
    page.writes
        .update(vcx, |w, cx| w.not_interested(LIKED, cx));
    vcx.run_until_parked();
    assert_eq!(log(&page).len(), 1);
}

#[gpui::test]
fn a_refused_not_interested_puts_the_heart_back_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    page.writes
        .update(vcx, |w, cx| w.not_interested(LIKED, cx));
    vcx.run_until_parked();
    assert_eq!(liked(&page, vcx, LIKED), Some(true), "still liked");
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't send that"));
    assert_eq!(log(&page).len(), 1, "a definite no is not read back or retried");
    assert!(page.sim.disliked_ids().is_empty());
    assert!(!page.writes.read_with(vcx, |w, _| w.is_disliked(LIKED)));
}

#[gpui::test]
fn a_lost_reply_is_read_back_and_a_landed_thumbs_down_stands(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    page.writes
        .update(vcx, |w, cx| w.not_interested(PLAIN, cx));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("dislike", PLAIN)], "sent once, never retried");
    assert!(page.writes.read_with(vcx, |w, _| w.is_disliked(PLAIN)));
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn a_lost_reply_for_a_thumbs_down_that_did_not_land_is_an_error(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Unknown));
    page.writes
        .update(vcx, |w, cx| w.not_interested(LIKED, cx));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("dislike", LIKED)], "sent once, never retried");
    assert_eq!(liked(&page, vcx, LIKED), Some(true));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't send that"));
    assert!(!page.writes.read_with(vcx, |w, _| w.is_disliked(LIKED)));
}

#[gpui::test]
fn not_interested_waits_out_a_like_that_is_still_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    next_track(&page, vcx);
    page.writes.update(vcx, |w, cx| w.toggle_like(PLAIN, cx));
    page.writes
        .update(vcx, |w, cx| w.not_interested(PLAIN, cx));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Try again in a moment"));
    vcx.run_until_parked();
    assert_eq!(log(&page), [entry("like", PLAIN)], "the two never cross");
}
