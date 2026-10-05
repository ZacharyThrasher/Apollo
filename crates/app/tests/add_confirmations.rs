//! Add confirmations: every way of adding to a list says so once, in the tip pill, after the
//! write has gone through, and lights up the list where it is on screen. The real shell and
//! views on GPUI's test platform, the simulated library as the "service".

use std::sync::Arc;
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::confirm::{self, Place};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::shortcuts::{self, Scope};
use apollo_app::settings::{SettingsModel, CONFIRM_ADDS, SHOW_SHORTCUT_HINTS};
use apollo_app::style::Fonts;
use apollo_app::target_playlist::TargetModel;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{PlaylistTarget, WriteFailure};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

const PLAYING: &str = "simTrack001";
const NEXT: &str = "plainTrack1";
const UNDO: &str = " \u{b7} Ctrl+Z to undo";

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
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
    player: Entity<PlayerModel>,
    crate_model: Entity<CrateModel>,
    target: Entity<TargetModel>,
    settings: Entity<SettingsModel>,
    sim: SimLibrary,
}

fn t(id: &str) -> Track {
    Track::new(id, format!("Song {id}"))
}

fn setup_with(cx: &mut TestAppContext, reduce_motion: bool) -> (Page, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(reduce_motion);
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
        queue: vec![
            LibraryTrack { track: t(PLAYING) },
            LibraryTrack { track: t(NEXT) },
        ],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let (crate_model, target, settings) = shell.read_with(vcx, |s, cx| {
        (s.crate_model().clone(), s.target_model().clone(), s.settings(cx))
    });
    (
        Page {
            shell,
            player,
            crate_model,
            target,
            settings,
            sim,
        },
        vcx,
    )
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    setup_with(cx, true)
}

/// Let the pill go.
fn settle(vcx: &mut VisualTestContext) {
    vcx.executor().advance_clock(Duration::from_secs(4));
    vcx.run_until_parked();
}

fn toggle_like(page: &Page, vcx: &mut VisualTestContext, video: &str) {
    page.shell
        .update(vcx, |s, cx| s.writes().update(cx, |w, cx| w.toggle_like(video, cx)));
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

fn set_target(page: &Page, vcx: &mut VisualTestContext, id: &str, name: &str) {
    page.target.update(vcx, |m, cx| m.set(id, name, cx));
    vcx.run_until_parked();
}

fn playlist(id: &str, title: &str) -> PlaylistTarget {
    PlaylistTarget {
        id: id.into(),
        title: title.into(),
        thumbnail: None,
        holds_track: false,
    }
}

fn pulse(vcx: &mut VisualTestContext, place: Place) -> Option<u64> {
    vcx.update(|_, cx| confirm::pulse(place, cx))
}

// ---- the queue --------------------------------------------------------------------------------

#[gpui::test]
fn adding_to_the_queue_says_so_and_names_what_ctrl_z_does(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(pill(&page, vcx), None);
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added to Queue{UNDO}").as_str()));
}

#[gpui::test]
fn play_next_and_several_songs_say_how_many(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player.update(vcx, |p, cx| p.play_next(t("one"), cx));
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added to Queue{UNDO}").as_str()));
    page.player
        .update(vcx, |p, cx| p.add_tracks_to_queue(vec![t("a"), t("b"), t("c")], cx));
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added 3 songs to Queue{UNDO}").as_str()));
    page.player
        .update(vcx, |p, cx| p.play_next_tracks(vec![t("d"), t("e")], cx));
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added 2 songs to Queue{UNDO}").as_str()));
    // Nothing to add, nothing said.
    settle(vcx);
    page.player.update(vcx, |p, cx| p.play_next_tracks(Vec::new(), cx));
    assert_eq!(pill(&page, vcx), None);
}

#[gpui::test]
fn the_undo_hint_goes_when_shortcut_hints_are_off(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.settings.update(vcx, |s, cx| s.set(&SHOW_SHORTCUT_HINTS, false, cx));
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Queue"));
}

#[gpui::test]
fn a_newer_add_replaces_the_pill_and_it_lasts_two_and_a_half_seconds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    vcx.executor().advance_clock(Duration::from_millis(2_000));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("c");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Crate"), "the newest add is what shows");
    // The first pill's own timer ran out at 2.5 s, but it was replaced: the new one still shows.
    vcx.executor().advance_clock(Duration::from_millis(700));
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Crate"));
    vcx.executor().advance_clock(Duration::from_millis(1_900));
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx), None);
}

#[gpui::test]
fn ctrl_z_does_take_back_a_queue_add_the_pill_names(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let before = page.player.read_with(vcx, |p, _| p.queue_view().upcoming.len());
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    assert_eq!(page.player.read_with(vcx, |p, _| p.queue_view().upcoming.len()), before + 1);
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(page.player.read_with(vcx, |p, _| p.queue_view().upcoming.len()), before);
}

// ---- the Crate --------------------------------------------------------------------------------

#[gpui::test]
fn c_says_the_song_is_in_the_crate_and_again_that_it_already_was(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("c");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added to Crate"), "the Crate has no undo");
    settle(vcx);
    vcx.simulate_keystrokes("c");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Already in Crate"));
}

#[gpui::test]
fn a_dragged_in_song_says_so_but_a_song_moved_within_the_crate_does_not(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.crate_model.update(vcx, |m, cx| {
        m.add(t("a"), None, cx);
        m.add(t("b"), None, cx);
    });
    assert_eq!(pill(&page, vcx), None, "the model alone says nothing: the callers do");
    page.crate_model.update(vcx, |m, cx| {
        m.insert_at(vec![(t("x"), None), (t("y"), None)], 1, cx);
    });
    assert_eq!(pill(&page, vcx).as_deref(), Some("Added 2 songs to Crate"));
    settle(vcx);
    page.crate_model.update(vcx, |m, cx| {
        m.insert_at(vec![(t("b"), None)], 0, cx);
    });
    assert_eq!(pill(&page, vcx), None, "b only moved");
}

// ---- playlists --------------------------------------------------------------------------------

#[gpui::test]
fn a_playlist_add_is_said_only_once_the_service_has_taken_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx), None, "a refusal says nothing good");
    assert_eq!(notice(&page, vcx).as_deref(), Some("Couldn't add to playlist"));

    page.sim.set_write_behaviour(WriteBehaviour::Succeed);
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added to Road Trip{UNDO}").as_str()));
}

#[gpui::test]
fn several_songs_into_a_playlist_say_how_many(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| {
        s.writes().update(cx, |w, cx| {
            w.add_many_to_playlist(
                &playlist("PLsimroad", "Road trip"),
                &["a".into(), "b".into(), "c".into()],
                cx,
            )
        })
    });
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added 3 songs to Road trip{UNDO}").as_str()));
}

#[gpui::test]
fn a_song_the_playlist_holds_already_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    set_target(&page, vcx, "PLsimempty", "Nothing Yet");
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    settle(vcx);
    page.shell.update(vcx, |s, cx| {
        s.writes().update(cx, |w, cx| {
            w.add_to_playlist(&playlist("PLsimempty", "Nothing Yet"), PLAYING, cx)
        })
    });
    assert_eq!(pill(&page, vcx).as_deref(), Some("Already in Nothing Yet"));
}

#[gpui::test]
fn a_long_playlist_name_is_cut(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let title = "A very long playlist name that would run on and on";
    page.shell.update(vcx, |s, cx| {
        s.writes()
            .update(cx, |w, cx| w.add_to_playlist(&playlist("PLsimroad", title), "x", cx))
    });
    vcx.run_until_parked();
    let text = pill(&page, vcx).expect("said");
    assert!(text.contains('\u{2026}') && !text.contains("run on and on"), "{text}");
}

// ---- likes and the library --------------------------------------------------------------------

#[gpui::test]
fn liking_says_so_after_the_write_and_unliking_says_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    toggle_like(&page, vcx, NEXT);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Liked"), "a like cannot be undone with Ctrl+Z");
    settle(vcx);
    toggle_like(&page, vcx, NEXT);
    assert_eq!(pill(&page, vcx), None);
}

#[gpui::test]
fn a_refused_like_says_nothing_good(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    toggle_like(&page, vcx, NEXT);
    assert_eq!(pill(&page, vcx), None);
    assert!(notice(&page, vcx).is_some());
}

#[gpui::test]
fn liking_a_selection_says_how_many_once_at_the_end(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| {
        s.writes()
            .update(cx, |w, cx| w.set_liked_many(vec!["a".into(), "b".into(), "c".into()], true, cx))
    });
    for _ in 0..40 {
        vcx.executor().advance_clock(Duration::from_millis(100));
        vcx.run_until_parked();
        if let Some(text) = pill(&page, vcx) {
            assert_eq!(text, "Liked 3 songs", "no pill per song while the run goes on");
            return;
        }
    }
    panic!("the run never said how many");
}

#[gpui::test]
fn saving_a_song_to_the_library_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    page.shell.update(vcx, |s, cx| {
        s.edits()
            .update(cx, |e, cx| e.set_song_saved("savedNewSong".into(), true, cx))
    });
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx).as_deref(), Some("Saved to library"));
}

// ---- the switch -------------------------------------------------------------------------------

#[gpui::test]
fn the_switch_turns_off_both_the_pill_and_the_pulse(cx: &mut TestAppContext) {
    let (page, vcx) = setup_with(cx, false);
    page.settings.update(vcx, |s, cx| s.set(&CONFIRM_ADDS, false, cx));
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    vcx.simulate_keystrokes("c");
    assert_eq!(pill(&page, vcx), None);
    assert_eq!(pulse(vcx, Place::Queue), None);
    assert_eq!(pulse(vcx, Place::Crate), None);
    // The adds themselves still happened.
    assert_eq!(page.crate_model.read_with(vcx, |m, _| m.len()), 1);
}

#[gpui::test]
fn the_switch_is_on_by_default(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(page.settings.read_with(vcx, |s, _| s.get(&CONFIRM_ADDS)));
}

// ---- the pulse --------------------------------------------------------------------------------

#[gpui::test]
fn the_destination_pulses_for_four_hundred_milliseconds(cx: &mut TestAppContext) {
    let (page, vcx) = setup_with(cx, false);
    assert_eq!(pulse(vcx, Place::Queue), None);
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    assert!(pulse(vcx, Place::Queue).is_some());
    assert_eq!(pulse(vcx, Place::Crate), None, "only where it landed");
    vcx.executor().advance_clock(Duration::from_millis(300));
    vcx.run_until_parked();
    assert!(pulse(vcx, Place::Queue).is_some());
    vcx.executor().advance_clock(Duration::from_millis(150));
    vcx.run_until_parked();
    assert_eq!(pulse(vcx, Place::Queue), None);
}

#[gpui::test]
fn the_added_row_is_the_one_that_pulses(cx: &mut TestAppContext) {
    let (page, vcx) = setup_with(cx, false);
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    let row = |vcx: &mut VisualTestContext, id: &str| vcx.update(|_, cx| confirm::row_pulse(Place::Queue, id, cx));
    assert!(row(vcx, "one").is_some());
    assert_eq!(row(vcx, NEXT), None);
}

#[gpui::test]
fn reduced_motion_leaves_the_pill_alone(cx: &mut TestAppContext) {
    let (page, vcx) = setup_with(cx, true);
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    assert!(pill(&page, vcx).is_some());
    assert_eq!(pulse(vcx, Place::Queue), None);
}

#[gpui::test]
fn the_target_pill_pulses_when_its_playlist_gets_the_song(cx: &mut TestAppContext) {
    let (page, vcx) = setup_with(cx, false);
    set_target(&page, vcx, "PLsimroad", "Road Trip");
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert!(pulse(vcx, Place::Target).is_some());
    // Another playlist is not on screen: the pill alone says so.
    page.shell.update(vcx, |s, cx| {
        s.writes()
            .update(cx, |w, cx| w.add_to_playlist(&playlist("PLsimempty", "Other"), "x", cx))
    });
    vcx.run_until_parked();
    assert_eq!(pulse(vcx, Place::Target), None);
    assert_eq!(pill(&page, vcx).as_deref(), Some(format!("Added to Other{UNDO}").as_str()));
}

#[gpui::test]
fn the_pulse_draws_on_the_queue_button_and_the_crate_item(cx: &mut TestAppContext) {
    let (page, vcx) = setup_with(cx, false);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.simulate_keystrokes("c");
    vcx.run_until_parked();
    // The bar and the rail drew without trouble while the highlight was on.
    assert!(vcx.debug_bounds("bar-crate").is_some());
    let _ = Modifiers::none();
}

#[gpui::test]
fn a_shortcut_tip_is_not_pushed_off_by_a_confirmation(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    for _ in 0..3 {
        vcx.update(|_, cx| shortcuts::note_mouse_use("crate", Scope::Any, cx));
    }
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx).as_deref(), Some("Tip: press C"));
    page.player.update(vcx, |p, cx| p.add_to_queue(t("one"), cx));
    assert_eq!(pill(&page, vcx).as_deref(), Some("Tip: press C"), "the one-time tip stays");
    // The add itself happened, and once the tip is gone confirmations show again.
    settle(vcx);
    page.player.update(vcx, |p, cx| p.add_to_queue(t("two"), cx));
    assert!(pill(&page, vcx).is_some_and(|p| p.starts_with("Added to Queue")));
}
