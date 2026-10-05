//! Ctrl+Z for playlist edits: songs added to a playlist, taken out of one, and a playlist made
//! with its songs, in one line of time with the queue's own undo. The real shell and views on
//! GPUI's test platform, the simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::edits::Edit;
use apollo_app::library::model::Tab;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::library::LibraryView;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{Collection, PlaylistTarget, WriteFailure};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

const ROAD_TRIP: &str = "Road Trip";
const ROAD_TRIP_ID: &str = "PLsimroad";
const EMPTY_ID: &str = "PLsimempty";
const A: &str = "undoTrackA1";
const B: &str = "undoTrackB2";
const C: &str = "undoTrackC3";

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
    player: Entity<PlayerModel>,
    sim: SimLibrary,
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
    let ready = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: ["queueTrack1", "queueTrack2", "queueTrack3"]
            .iter()
            .map(|id| LibraryTrack { track: Track::new(*id, format!("Song {id}")) })
            .collect(),
        art: Arc::new(TestArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    let view = shell.read_with(vcx, |s, _| s.library().clone());
    (Page { shell, view, player, sim }, vcx)
}

fn target(id: &str, title: &str) -> PlaylistTarget {
    PlaylistTarget { id: id.into(), title: title.into(), thumbnail: None, holds_track: false }
}

fn empty_list() -> PlaylistTarget {
    target(EMPTY_ID, "Nothing Yet")
}

fn add(page: &Page, vcx: &mut VisualTestContext, to: &PlaylistTarget, video: &str) {
    page.shell.read_with(vcx, |s, _| s.writes().clone()).update(vcx, |w, cx| {
        w.add_to_playlist(to, video, cx)
    });
    vcx.run_until_parked();
}

fn add_many(page: &Page, vcx: &mut VisualTestContext, to: &PlaylistTarget, videos: &[&str]) {
    let videos: Vec<String> = videos.iter().map(|v| (*v).to_owned()).collect();
    page.shell.read_with(vcx, |s, _| s.writes().clone()).update(vcx, |w, cx| {
        w.add_many_to_playlist(to, &videos, cx)
    });
    vcx.run_until_parked();
}

/// Ctrl+Z, as the shell takes it.
fn undo(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.undo(cx));
    vcx.run_until_parked();
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.writes().read(cx).notice().map(|n| n.text.to_string())
    })
}

fn collection(page: &Page, vcx: &mut VisualTestContext, title: &str) -> Collection {
    page.view.read_with(vcx, |v, cx| {
        v.model()
            .read(cx)
            .list(Tab::Playlists)
            .iter()
            .find(|c| c.title == title)
            .cloned()
            .unwrap_or_else(|| panic!("{title} is in the library"))
    })
}

fn open_playlist(page: &Page, vcx: &mut VisualTestContext, title: &str) {
    let c = collection(page, vcx, title);
    page.view.update_in(vcx, |v, window, cx| v.open(c, window, cx));
    vcx.run_until_parked();
}

/// The video ids of the rows the open page shows.
fn shown(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
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

/// The video ids of a playlist as the service holds it.
fn held(page: &Page, bare_id: &str) -> Vec<String> {
    use apollo_app::library::LibrarySource;
    let collection = Collection {
        kind: apollo_domain::library::CollectionKind::Playlist,
        id: apollo_domain::model::BrowseId::new(format!("VL{bare_id}")),
        title: String::new(),
        subtitle: None,
        thumbnail: None,
    };
    page.sim
        .tracks(&collection)
        .expect("the playlist reads")
        .items
        .into_iter()
        .map(|r| r.track.video_id.as_str().to_owned())
        .collect()
}

fn queue_upcoming(page: &Page, vcx: &mut VisualTestContext) -> usize {
    page.player
        .read_with(vcx, |m, _| m.coordinator().unwrap().queue().upcoming().count())
}

// ---- adds -----------------------------------------------------------------------------------

#[gpui::test]
fn ctrl_z_takes_a_song_back_out_of_the_playlist_it_was_added_to(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let list = empty_list();
    add(&page, vcx, &list, A);
    assert_eq!(held(&page, EMPTY_ID), [A]);

    vcx.simulate_keystrokes("ctrl-z");
    vcx.run_until_parked();
    assert!(held(&page, EMPTY_ID).is_empty(), "the service holds nothing again");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: added a song to Nothing Yet"));
    assert_eq!(notice(&page, vcx), None);

    // The playlist no longer counts as holding it, so it can be added again.
    add(&page, vcx, &list, A);
    assert_eq!(held(&page, EMPTY_ID), [A]);
}

#[gpui::test]
fn several_songs_added_together_come_out_together(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    add_many(&page, vcx, &empty_list(), &[A, B, C]);
    assert_eq!(held(&page, EMPTY_ID), [A, B, C]);

    undo(&page, vcx);
    assert!(held(&page, EMPTY_ID).is_empty());
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: added 3 songs to Nothing Yet"));
}

#[gpui::test]
fn an_undone_add_leaves_other_copies_of_the_song_alone(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // Road Trip already holds three songs; one more is added and then taken back.
    add(&page, vcx, &target(ROAD_TRIP_ID, ROAD_TRIP), A);
    let with_a = held(&page, ROAD_TRIP_ID);
    assert_eq!(with_a.len(), 4);
    undo(&page, vcx);
    let after = held(&page, ROAD_TRIP_ID);
    assert_eq!(after.len(), 3);
    assert_eq!(after[..], with_a[..3], "the rows that were there stay, in order");
}

#[gpui::test]
fn the_open_playlist_page_follows_an_undone_add(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = shown(&page, vcx);
    add(&page, vcx, &target(ROAD_TRIP_ID, ROAD_TRIP), A);
    assert_eq!(shown(&page, vcx).len(), before.len() + 1);
    undo(&page, vcx);
    assert_eq!(shown(&page, vcx), before);
    assert_eq!(held(&page, ROAD_TRIP_ID), before);
}

#[gpui::test]
fn an_add_still_out_is_not_undone_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let writes = page.shell.read_with(vcx, |s, _| s.writes().clone());
    let list = empty_list();
    // Nothing runs between these calls: the add has not been answered.
    writes.update(vcx, |w, cx| w.add_to_playlist(&list, A, cx));
    page.shell.update(vcx, |s, cx| s.undo(cx));
    assert_eq!(notice(&page, vcx).as_deref(), Some("Try again in a moment"));
    assert_eq!(pill(&page, vcx), None);
    vcx.run_until_parked();
    assert_eq!(held(&page, EMPTY_ID), [A], "the add went through");
    // Once answered, it can be taken back.
    undo(&page, vcx);
    assert!(held(&page, EMPTY_ID).is_empty());
}

#[gpui::test]
fn an_add_whose_reply_was_lost_cannot_be_taken_back_exactly_so_it_is_not(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    page.sim.set_write_behaviour(WriteBehaviour::LandsReplyLost);
    add(&page, vcx, &empty_list(), A);
    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    assert_eq!(held(&page, EMPTY_ID), [A], "it landed");

    undo(&page, vcx);
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't undo that"));
    assert_eq!(held(&page, EMPTY_ID), [A], "nothing was guessed at");
    assert_eq!(pill(&page, vcx), None);
}

#[gpui::test]
fn a_refused_undo_says_so_and_shows_the_services_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    add(&page, vcx, &target(ROAD_TRIP_ID, ROAD_TRIP), A);
    let with_a = held(&page, ROAD_TRIP_ID);
    page.sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    undo(&page, vcx);
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't undo that"));
    assert_eq!(held(&page, ROAD_TRIP_ID), with_a);
    assert_eq!(shown(&page, vcx), with_a, "the page shows what the service holds");
}

#[gpui::test]
fn a_refused_add_is_not_a_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    add(&page, vcx, &empty_list(), A);
    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    page.shell.read_with(vcx, |s, _| s.writes().clone()).update(vcx, |w, cx| w.dismiss_notice(cx));
    undo(&page, vcx);
    assert_eq!(pill(&page, vcx), None);
    assert_eq!(notice(&page, vcx), None);
}

// ---- removals -------------------------------------------------------------------------------

#[gpui::test]
fn ctrl_z_puts_a_removed_song_back_where_it_was(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = shown(&page, vcx);
    let tokens = page.sim.occurrences(ROAD_TRIP_ID);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    model.update(vcx, |m, cx| m.remove_row(ROAD_TRIP_ID, &tokens[1], cx));
    vcx.run_until_parked();
    assert_eq!(held(&page, ROAD_TRIP_ID).len(), 2);

    undo(&page, vcx);
    assert_eq!(held(&page, ROAD_TRIP_ID), before, "back in the middle, not at the end");
    assert_eq!(shown(&page, vcx), before);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: removed a song from Road Trip")
    );
    assert_eq!(notice(&page, vcx), None);
    let log = page.sim.write_log();
    assert!(log.iter().any(|(what, _)| what == &format!("add-again:{ROAD_TRIP_ID}")));
    assert!(log.iter().any(|(what, _)| what == &format!("move:{ROAD_TRIP_ID}")));
}

#[gpui::test]
fn removing_several_is_one_step_and_each_returns_to_its_place(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = shown(&page, vcx);
    let tokens = page.sim.occurrences(ROAD_TRIP_ID);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    // The first and the last: one goes back in front of the middle row, one to the end.
    let wanted = vec![tokens[0].clone(), tokens[2].clone()];
    assert_eq!(model.update(vcx, |m, cx| m.remove_rows(ROAD_TRIP_ID, &wanted, cx)), 2);
    vcx.run_until_parked();
    assert_eq!(held(&page, ROAD_TRIP_ID).len(), 1);

    undo(&page, vcx);
    assert_eq!(held(&page, ROAD_TRIP_ID), before);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: removed 2 songs from Road Trip")
    );
    // One step: a second Ctrl+Z has nothing to take back.
    undo(&page, vcx);
    assert_eq!(held(&page, ROAD_TRIP_ID), before);
}

#[gpui::test]
fn a_song_listed_twice_comes_back_even_though_the_playlist_still_holds_it(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    page.sim.preset_playlist(EMPTY_ID, &[A, B, A, C]);
    open_playlist(&page, vcx, "Nothing Yet");
    let tokens = page.sim.occurrences(EMPTY_ID);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    // The second copy of A goes (what Remove duplicates does).
    model.update(vcx, |m, cx| m.remove_rows(EMPTY_ID, &[tokens[2].clone()], cx));
    vcx.run_until_parked();
    assert_eq!(held(&page, EMPTY_ID), [A, B, C]);

    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), [A, B, A, C], "the repeat is back in its place");
}

#[gpui::test]
fn a_song_whose_neighbour_is_gone_goes_back_at_the_end_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.preset_playlist(EMPTY_ID, &[A, B, C]);
    open_playlist(&page, vcx, "Nothing Yet");
    let tokens = page.sim.occurrences(EMPTY_ID);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    // A goes (it sat in front of B), then B goes too.
    model.update(vcx, |m, cx| m.remove_rows(EMPTY_ID, &[tokens[0].clone()], cx));
    vcx.run_until_parked();
    model.update(vcx, |m, cx| m.remove_rows(EMPTY_ID, &[tokens[1].clone()], cx));
    vcx.run_until_parked();
    assert_eq!(held(&page, EMPTY_ID), [C]);

    // B comes back first (in front of C), then A, whose neighbour is no longer there.
    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), [B, C]);
    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), [B, C, A], "A could not go before B: it is at the end");
    assert_eq!(notice(&page, vcx).as_deref(), Some("Put back at the end instead"));
}

#[gpui::test]
fn a_refused_removal_is_not_something_to_put_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = shown(&page, vcx);
    let tokens = page.sim.occurrences(ROAD_TRIP_ID);
    page.sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    model.update(vcx, |m, cx| m.remove_rows(ROAD_TRIP_ID, &tokens[..2], cx));
    vcx.run_until_parked();
    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    assert_eq!(held(&page, ROAD_TRIP_ID), before, "nothing was removed");

    page.shell
        .read_with(vcx, |s, _| s.writes().clone())
        .update(vcx, |w, cx| w.dismiss_notice(cx));
    undo(&page, vcx);
    assert_eq!(held(&page, ROAD_TRIP_ID), before, "no copy was added");
    assert_eq!(pill(&page, vcx), None);
}

// ---- a playlist made with its songs -------------------------------------------------------------

#[gpui::test]
fn ctrl_z_takes_back_a_playlist_that_was_just_made(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let edits = page.shell.read_with(vcx, |s, _| s.edits().clone());
    let known = page.sim.owned_ids();
    edits.update(vcx, |e, cx| {
        e.create_songs(Edit::Create, "Gym Mix".into(), vec![A.into(), B.into()], cx)
    });
    vcx.run_until_parked();
    let made: Vec<String> = page
        .sim
        .owned_ids()
        .into_iter()
        .filter(|id| !known.contains(id))
        .collect();
    assert_eq!(made.len(), 1);
    assert_eq!(held(&page, &made[0]), [A, B]);

    undo(&page, vcx);
    assert!(
        !page.sim.owned_ids().contains(&made[0]),
        "the playlist is gone from the account"
    );
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: created Gym Mix"));
    assert_eq!(notice(&page, vcx), None);
}

// ---- one line in time with the queue ------------------------------------------------------------

#[gpui::test]
fn ctrl_z_takes_back_whichever_of_queue_and_playlist_came_last(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let upcoming = queue_upcoming(&page, vcx);
    // Queue, then playlist, then queue again.
    page.player.update(vcx, |m, cx| m.add_to_queue(Track::new("extraOne01", "One"), cx));
    add(&page, vcx, &empty_list(), A);
    page.player.update(vcx, |m, cx| m.add_to_queue(Track::new("extraTwo02", "Two"), cx));
    assert_eq!(queue_upcoming(&page, vcx), upcoming + 2);

    undo(&page, vcx);
    assert_eq!(queue_upcoming(&page, vcx), upcoming + 1, "the newest queue change first");
    assert_eq!(held(&page, EMPTY_ID), [A], "the add is still there");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"));

    undo(&page, vcx);
    assert!(held(&page, EMPTY_ID).is_empty(), "then the add");
    assert_eq!(queue_upcoming(&page, vcx), upcoming + 1);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: added a song to Nothing Yet"));

    undo(&page, vcx);
    assert_eq!(queue_upcoming(&page, vcx), upcoming, "and the oldest last");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"));
}

#[gpui::test]
fn a_playlist_edit_made_first_is_undone_after_a_later_queue_change(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    add(&page, vcx, &empty_list(), A);
    page.player.update(vcx, |m, cx| m.add_to_queue(Track::new("extraOne01", "One"), cx));
    let upcoming = queue_upcoming(&page, vcx);

    undo(&page, vcx);
    assert_eq!(queue_upcoming(&page, vcx), upcoming - 1);
    assert_eq!(held(&page, EMPTY_ID), [A]);
    undo(&page, vcx);
    assert!(held(&page, EMPTY_ID).is_empty());
}

#[gpui::test]
fn queue_changes_stay_undoable_after_the_playlist_edits_before_them_are_undone(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    let upcoming = queue_upcoming(&page, vcx);
    page.player.update(vcx, |m, cx| m.add_to_queue(Track::new("extraOne01", "One"), cx));
    add(&page, vcx, &empty_list(), A);
    // Undo the add (newest), then redo nothing playlist-side: the queue redo is its own line.
    undo(&page, vcx);
    assert!(held(&page, EMPTY_ID).is_empty());
    undo(&page, vcx);
    assert_eq!(queue_upcoming(&page, vcx), upcoming);
    page.player.update(vcx, |m, cx| m.redo_queue(cx));
    assert_eq!(queue_upcoming(&page, vcx), upcoming + 1);
    // Nothing playlist-side is left, so the redone change is what Ctrl+Z takes now.
    undo(&page, vcx);
    assert_eq!(queue_upcoming(&page, vcx), upcoming);
}

#[gpui::test]
fn nothing_to_undo_says_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-z");
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx), None);
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn only_the_last_twenty_playlist_edits_are_kept(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let list = empty_list();
    for n in 0..25 {
        add(&page, vcx, &list, &format!("manyTrack{n:03}"));
    }
    assert_eq!(held(&page, EMPTY_ID).len(), 25);
    for _ in 0..30 {
        undo(&page, vcx);
    }
    assert_eq!(held(&page, EMPTY_ID).len(), 5, "the five oldest adds could not be taken back");
}

#[gpui::test]
fn a_different_account_starts_with_nothing_to_undo(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    add(&page, vcx, &empty_list(), A);
    page.shell
        .read_with(vcx, |s, _| s.writes().clone())
        .update(vcx, |w, cx| w.reset(cx));
    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), [A], "the old account's edit is not touched");
    assert_eq!(pill(&page, vcx), None);
}

// ---- moves ----------------------------------------------------------------------------------

#[gpui::test]
fn ctrl_z_puts_a_dragged_row_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_playlist(&page, vcx, ROAD_TRIP);
    let before = shown(&page, vcx);
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    assert!(model.update(vcx, |m, cx| m.move_row(0, 3, cx)));
    vcx.run_until_parked();
    assert_ne!(held(&page, ROAD_TRIP_ID), before);

    undo(&page, vcx);
    assert_eq!(held(&page, ROAD_TRIP_ID), before);
    assert_eq!(shown(&page, vcx), before);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: reordered Road Trip"));
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn several_rows_arranged_at_once_are_one_step(cx: &mut TestAppContext) {
    use apollo_app::library::model::Arrange;
    use apollo_domain::arrange::Edge;
    let (page, vcx) = setup(cx);
    page.sim.preset_playlist(EMPTY_ID, &[A, B, C, "undoTrackD4", "undoTrackE5"]);
    open_playlist(&page, vcx, "Nothing Yet");
    let before = held(&page, EMPTY_ID);
    let tokens = page.sim.occurrences(EMPTY_ID);
    let picked = vec![tokens[3].clone(), tokens[4].clone()];
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    assert!(model.update(vcx, |m, cx| m.arrange_rows(EMPTY_ID, &picked, Arrange::To(Edge::Top), cx)));
    vcx.run_until_parked();
    assert_eq!(held(&page, EMPTY_ID)[..2], ["undoTrackD4", "undoTrackE5"]);

    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), before, "one Ctrl+Z puts them all back");
}

#[gpui::test]
fn a_move_is_undone_before_the_add_made_after_it_and_not_the_other_way_round(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    page.sim.preset_playlist(EMPTY_ID, &[A, B, C]);
    open_playlist(&page, vcx, "Nothing Yet");
    let model = page.view.read_with(vcx, |v, _| v.model().clone());
    assert!(model.update(vcx, |m, cx| m.move_row(0, 3, cx)));
    vcx.run_until_parked();
    assert_eq!(held(&page, EMPTY_ID), [B, C, A]);
    add(&page, vcx, &empty_list(), "undoTrackD4");
    assert_eq!(held(&page, EMPTY_ID), [B, C, A, "undoTrackD4"]);

    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), [B, C, A], "the add first");
    undo(&page, vcx);
    assert_eq!(held(&page, EMPTY_ID), [A, B, C], "then the move");
}
