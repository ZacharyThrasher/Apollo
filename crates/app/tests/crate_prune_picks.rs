//! Crate pruning and picks: Skim the Crate (Delete tosses the song, K keeps it), Save and Add all
//! acting on the picked songs only, and the filed markers with "Hide filed". The real shell and
//! views on GPUI's test platform, the simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::{CrateModel, SKIM_SOURCE};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::{CRATE_MARK_FILED, SKIM};
use apollo_app::style::Fonts;
use apollo_app::target_playlist::TargetModel;
use apollo_app::ui::nav::Route;
use apollo_app::ui::playlist_dialog::Mode;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::library::PlaylistTarget;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

const LIST: [&str; 2] = ["trackAAAA01", "trackBBBB02"];
const CRATED: [&str; 4] = ["crateAAAA01", "crateBBBB02", "crateCCCC03", "crateDDDD04"];

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
    sim: SimLibrary,
}

fn t(id: &str) -> Track {
    Track::new(id, format!("Song {id}"))
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    let sim = SimLibrary::instant();
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
    });
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
        queue: LIST
            .iter()
            .map(|id| LibraryTrack { track: t(id) })
            .collect(),
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
    let (crate_model, target) =
        shell.read_with(vcx, |s, _| (s.crate_model().clone(), s.target_model().clone()));
    (
        Page {
            shell,
            player,
            crate_model,
            target,
            sim,
        },
        vcx,
    )
}

fn fill(page: &Page, vcx: &mut VisualTestContext, ids: &[&str]) {
    page.crate_model.update(vcx, |m, cx| {
        for id in ids {
            m.add(t(id), None, cx);
        }
    });
    vcx.run_until_parked();
}

fn open_crate(page: &Page, vcx: &mut VisualTestContext) {
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

/// What the Crate page lists now.
fn shown(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    crate_page.read_with(vcx, |p, cx| p.shown_ids(cx))
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
}

const ROWS: [&str; 4] = ["crate-row-0", "crate-row-1", "crate-row-2", "crate-row-3"];

fn ctrl_click_row(vcx: &mut VisualTestContext, ix: usize) {
    let at = vcx
        .debug_bounds(ROWS[ix])
        .unwrap_or_else(|| panic!("{} is on screen", ROWS[ix]))
        .center();
    vcx.simulate_click(at, Modifiers::control());
}

fn playing(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.player.read_with(vcx, |m, _| m.current_video())
}

fn crate_skim(page: &Page, vcx: &mut VisualTestContext) -> bool {
    page.player
        .read_with(vcx, |m, _| m.skimming_from(SKIM_SOURCE))
}

fn undo(page: &Page, vcx: &mut VisualTestContext) {
    page.shell.update(vcx, |s, cx| s.undo(cx));
    vcx.run_until_parked();
}

fn strings(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

// ---- Skim the Crate ----------------------------------------------------------------------------

#[gpui::test]
fn the_skim_button_auditions_the_crate_from_the_top(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    assert!(!crate_skim(&page, vcx));
    click(vcx, "crate-skim");
    assert!(crate_skim(&page, vcx));
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[0]));
    let view = page.player.read_with(vcx, |m, _| m.view().skim);
    assert_eq!(view.map(|s| (s.index, s.total)), Some((1, 4)));
}

#[gpui::test]
fn skim_is_hidden_when_switched_off_and_with_no_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_crate(&page, vcx);
    // The button is there but cannot start anything with no songs.
    click(vcx, "crate-skim");
    assert!(!page.player.read_with(vcx, |m, _| m.skimming()));
    fill(&page, vcx, &CRATED);
    // The app applies the Skim switch to the player (see `app.rs`); here it is done by hand.
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&SKIM, false, cx));
    page.player.update(vcx, |m, cx| m.set_skim_enabled(false, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("crate-skim").is_none(), "Settings > Playback > Skim off hides it");
}

#[gpui::test]
fn k_on_the_list_skims_from_the_cursor_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    ctrl_click_row(vcx, 2);
    vcx.simulate_keystrokes("escape");
    vcx.simulate_keystrokes("k");
    assert!(crate_skim(&page, vcx));
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[2]));
}

#[gpui::test]
fn delete_tosses_the_playing_song_and_the_skim_goes_on(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    click(vcx, "crate-skim");
    vcx.simulate_keystrokes("delete");
    assert_eq!(crate_ids(&page, vcx), strings(&CRATED[1..]));
    assert!(crate_skim(&page, vcx), "the skim carries on");
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[1]));
    // Ctrl+Z puts the song back where it was.
    undo(&page, vcx);
    assert_eq!(crate_ids(&page, vcx), strings(&CRATED));
}

#[gpui::test]
fn delete_with_the_list_focused_tosses_the_playing_song_not_the_cursor_row(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    // Skim from the top with the Skim button, then the list takes the keys back with its cursor
    // on the last row (one pick, which Delete leaves alone while a skim runs).
    click(vcx, "crate-skim");
    ctrl_click_row(vcx, 3);
    assert!(crate_skim(&page, vcx));
    vcx.simulate_keystrokes("delete");
    assert_eq!(
        crate_ids(&page, vcx),
        strings(&[CRATED[1], CRATED[2], CRATED[3]]),
        "the song that was playing went, not the row the cursor was on"
    );
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[1]));
}

#[gpui::test]
fn k_keeps_the_song_and_goes_on(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    click(vcx, "crate-skim");
    vcx.simulate_keystrokes("k");
    assert_eq!(crate_ids(&page, vcx), strings(&CRATED), "nothing leaves");
    assert!(crate_skim(&page, vcx));
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[1]));
}

#[gpui::test]
fn enter_plays_the_song_and_ends_the_skim_and_esc_goes_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    click(vcx, "crate-skim");
    vcx.simulate_keystrokes("k");
    vcx.simulate_keystrokes("enter");
    assert!(!page.player.read_with(vcx, |m, _| m.skimming()));
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[1]));

    // A second skim, left with Esc: what was playing comes back.
    click(vcx, "crate-skim");
    assert!(crate_skim(&page, vcx));
    vcx.simulate_keystrokes("escape");
    assert!(!page.player.read_with(vcx, |m, _| m.skimming()));
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[1]));
}

#[gpui::test]
fn tossing_the_last_song_ends_the_skim(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED[..2]);
    open_crate(&page, vcx);
    click(vcx, "crate-skim");
    vcx.simulate_keystrokes("delete");
    assert!(crate_skim(&page, vcx));
    vcx.simulate_keystrokes("delete");
    assert!(crate_ids(&page, vcx).is_empty());
    assert!(!page.player.read_with(vcx, |m, _| m.skimming()), "nothing left to skim");
}

#[gpui::test]
fn delete_while_skimming_something_else_is_not_the_crates(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    page.player.update(vcx, |m, cx| {
        m.start_skim(vec![t("otherAAAA01"), t("otherBBBB02")], None, cx)
    });
    vcx.run_until_parked();
    assert!(!crate_skim(&page, vcx));
    vcx.simulate_keystrokes("delete");
    assert_eq!(crate_ids(&page, vcx), strings(&CRATED), "the Crate is untouched");
    assert_eq!(playing(&page, vcx).as_deref(), Some("otherAAAA01"));
}

#[gpui::test]
fn the_cursor_follows_the_crate_skim(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    click(vcx, "crate-skim");
    vcx.run_until_parked();
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    assert_eq!(crate_page.read_with(vcx, |p, _| p.cursor().map(str::to_owned)).as_deref(), Some(CRATED[0]));
    vcx.simulate_keystrokes("k");
    vcx.run_until_parked();
    assert_eq!(crate_page.read_with(vcx, |p, _| p.cursor().map(str::to_owned)).as_deref(), Some(CRATED[1]));
}

// ---- Peel off picked songs ---------------------------------------------------------------------

#[gpui::test]
fn save_as_playlist_takes_only_the_picked_songs(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    ctrl_click_row(vcx, 3);
    ctrl_click_row(vcx, 1);
    click(vcx, "crate-save-playlist");
    let dialog = page.shell.read_with(vcx, |s, _| s.dialog().clone());
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.mode().cloned()),
        Some(Mode::FromCrate {
            name: "My Crate".into(),
            video_ids: strings(&[CRATED[1], CRATED[3]]),
        }),
        "the picks, in the Crate's order"
    );
    // Clearing afterwards takes out only what was saved.
    click(vcx, "playlist-dialog-clear-crate");
    dialog.update(vcx, |d, cx| d.submit(cx));
    vcx.run_until_parked();
    assert_eq!(crate_ids(&page, vcx), strings(&[CRATED[0], CRATED[2]]));
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), strings(&[CRATED[1], CRATED[3]]));
}

#[gpui::test]
fn one_pick_is_not_a_peel_and_the_whole_crate_is_saved(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    ctrl_click_row(vcx, 1);
    click(vcx, "crate-save-playlist");
    let dialog = page.shell.read_with(vcx, |s, _| s.dialog().clone());
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.mode().cloned()),
        Some(Mode::FromCrate {
            name: "My Crate".into(),
            video_ids: strings(&CRATED),
        })
    );
}

#[gpui::test]
fn add_all_to_files_only_the_picked_songs_in_one_write(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    ctrl_click_row(vcx, 0);
    ctrl_click_row(vcx, 2);
    click(vcx, "crate-add-to-playlist");
    vcx.run_until_parked();
    let picker = page.shell.read_with(vcx, |s, _| s.picker().clone());
    let target = picker.read_with(vcx, |p, _| match p.playlists() {
        Some(apollo_app::library::model::Load::Ready(list)) => list[0].clone(),
        _ => panic!("playlists are listed"),
    });
    vcx.update(|window, cx| picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();
    assert_eq!(
        page.sim.write_log(),
        vec![(
            format!("add-many:{}", target.id),
            [CRATED[0], CRATED[2]].join(",")
        )]
    );
    assert_eq!(crate_ids(&page, vcx), strings(&CRATED), "the Crate keeps everything");
}

// ---- Filed markers -----------------------------------------------------------------------------

fn set_target(page: &Page, vcx: &mut VisualTestContext) {
    page.sim.preset_playlist("PLsimroad", &[CRATED[0], CRATED[2]]);
    page.target
        .update(vcx, |m, cx| m.set("PLsimroad", "Road Trip", cx));
    vcx.run_until_parked();
}

fn is_filed(page: &Page, vcx: &mut VisualTestContext, id: &str) -> bool {
    page.target.read_with(vcx, |m, cx| m.is_filed(id, cx))
}

#[gpui::test]
fn rows_the_target_already_holds_are_marked(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    open_crate(&page, vcx);
    assert!(vcx.debug_bounds("crate-filed-0").is_none(), "no target, no marks");
    assert!(vcx.debug_bounds("crate-hide-filed").is_none(), "and no Hide filed");

    set_target(&page, vcx);
    open_crate(&page, vcx);
    assert!(vcx.debug_bounds("crate-filed-0").is_some());
    assert!(is_filed(&page, vcx, CRATED[0]));
    assert!(!is_filed(&page, vcx, CRATED[1]));
    assert!(is_filed(&page, vcx, CRATED[2]));
    assert!(!is_filed(&page, vcx, CRATED[3]));
    assert!(vcx.debug_bounds("crate-hide-filed").is_some());
}

#[gpui::test]
fn the_target_is_read_once_however_often_the_crate_is_looked_at(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    set_target(&page, vcx);
    for _ in 0..3 {
        open_crate(&page, vcx);
        page.shell
            .update(vcx, |s, cx| s.show_route(Route::Home, cx));
        vcx.run_until_parked();
    }
    open_crate(&page, vcx);
    // Songs added here afterwards are marked without reading the playlist again.
    let road = PlaylistTarget {
        id: "PLsimroad".into(),
        title: "Road Trip".into(),
        thumbnail: None,
        holds_track: false,
    };
    page.shell.read_with(vcx, |s, _| s.writes().clone()).update(vcx, |w, cx| {
        w.add_to_playlist(&road, CRATED[1], cx)
    });
    vcx.run_until_parked();
    assert!(is_filed(&page, vcx, CRATED[1]), "an add made here is known at once");
    assert_eq!(
        page.sim.track_reads(),
        vec!["VLPLsimroad".to_owned()],
        "one read of the target, ever"
    );
}

#[gpui::test]
fn a_failed_read_is_not_tried_again_and_marks_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    // A playlist the simulation cannot read: it answers with a network failure.(it answers with a network failure).
    page.target
        .update(vcx, |m, cx| m.set("PLsimbroken", "Broken", cx));
    for _ in 0..3 {
        open_crate(&page, vcx);
    }
    assert!(!is_filed(&page, vcx, CRATED[0]));
    assert!(vcx.debug_bounds("crate-filed-0").is_none());
    let reads = page
        .sim
        .track_reads()
        .iter()
        .filter(|r| r.as_str() == "VLPLsimbroken")
        .count();
    assert_eq!(reads, 1);
}

#[gpui::test]
fn the_marks_can_be_switched_off_and_nothing_is_read(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&CRATE_MARK_FILED, false, cx));
    set_target(&page, vcx);
    open_crate(&page, vcx);
    assert!(page.sim.track_reads().is_empty(), "the target's songs are not read for marks that are off");
    assert!(!is_filed(&page, vcx, CRATED[0]));
    assert!(vcx.debug_bounds("crate-filed-0").is_none());
    assert!(vcx.debug_bounds("crate-hide-filed").is_none());
}

#[gpui::test]
fn hide_filed_leaves_out_what_the_target_holds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    set_target(&page, vcx);
    open_crate(&page, vcx);
    assert_eq!(shown(&page, vcx), strings(&CRATED));
    click(vcx, "crate-hide-filed");
    assert_eq!(shown(&page, vcx), strings(&[CRATED[1], CRATED[3]]));
    assert_eq!(crate_ids(&page, vcx), strings(&CRATED), "hiding changes the view, not the Crate");
    assert!(vcx.debug_bounds("crate-row-1").is_some());
    assert!(vcx.debug_bounds("crate-row-2").is_none());

    // The buttons follow the list shown: Save takes the two that are left.
    click(vcx, "crate-save-playlist");
    let dialog = page.shell.read_with(vcx, |s, _| s.dialog().clone());
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.mode().cloned()),
        Some(Mode::FromCrate {
            name: "My Crate".into(),
            video_ids: strings(&[CRATED[1], CRATED[3]]),
        })
    );
    vcx.update(|window, cx| dialog.update(cx, |d, cx| d.cancel(window, cx)));
    vcx.run_until_parked();

    // Playing from a shown row plays what is shown.
    let at = vcx.debug_bounds("crate-row-1").unwrap().center();
    vcx.simulate_click(at, Modifiers::none());
    assert_eq!(playing(&page, vcx).as_deref(), Some(CRATED[3]));

    // Show filed brings them back.
    click(vcx, "crate-hide-filed");
    assert_eq!(shown(&page, vcx), strings(&CRATED));
}

#[gpui::test]
fn a_song_filed_while_filed_songs_are_hidden_leaves_the_list(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &CRATED);
    set_target(&page, vcx);
    open_crate(&page, vcx);
    click(vcx, "crate-hide-filed");
    ctrl_click_row(vcx, 0);
    vcx.simulate_keystrokes("escape");
    assert_eq!(shown(&page, vcx), strings(&[CRATED[1], CRATED[3]]));
    // A files the song under the cursor into the target: it leaves the list, and the cursor
    // goes to the next row.
    vcx.simulate_keystrokes("a");
    vcx.run_until_parked();
    assert_eq!(shown(&page, vcx), strings(&[CRATED[3]]));
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    vcx.run_until_parked();
    assert_eq!(
        crate_page.read_with(vcx, |p, _| p.cursor().map(str::to_owned)).as_deref(),
        Some(CRATED[3])
    );
}

#[gpui::test]
fn when_everything_is_filed_the_list_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[CRATED[0], CRATED[2]]);
    set_target(&page, vcx);
    open_crate(&page, vcx);
    click(vcx, "crate-hide-filed");
    assert!(shown(&page, vcx).is_empty());
    assert!(vcx.debug_bounds("crate-all-filed").is_some());
    assert!(vcx.debug_bounds("crate-hide-filed").is_some(), "the way back stays");
}

#[gpui::test]
fn the_settings_page_has_the_marks_switch(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("setting-crate-mark-filed").is_some());
}
