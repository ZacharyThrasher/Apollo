//! The Crate page's "Save as playlist…" and "Add all to…". The real shell and views on GPUI's test
//! platform, the simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::edits::{failure_text, Edit};
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::playlist_dialog::{Mode, PlaylistDialog};
use apollo_app::ui::playlist_picker::PlaylistPicker;
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::library::WriteFailure;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

const LIST: [&str; 4] = ["trackAAAA01", "trackBBBB02", "trackCCCC03", "trackDDDD04"];

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
    crate_model: Entity<CrateModel>,
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
    let crate_model = shell.read_with(vcx, |s, _| s.crate_model().clone());
    (
        Page {
            shell,
            crate_model,
            sim,
        },
        vcx,
    )
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
}

/// The crate, newest first as the page shows it.
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

fn dialog_of(page: &Page, vcx: &mut VisualTestContext) -> Entity<PlaylistDialog> {
    page.shell.read_with(vcx, |s, _| s.dialog().clone())
}

fn picker_of(page: &Page, vcx: &mut VisualTestContext) -> Entity<PlaylistPicker> {
    page.shell.read_with(vcx, |s, _| s.picker().clone())
}

fn notice(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, cx| {
        s.writes().read(cx).notice().map(|n| n.text.to_string())
    })
}

const SAVED: [&str; 3] = ["crateCCCC03", "crateBBBB02", "crateAAAA01"];

#[gpui::test]
fn save_as_playlist_opens_the_dialog_filled_in_with_the_whole_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02", "crateCCCC03"]);
    open_crate(&page, vcx);
    click(vcx, "crate-save-playlist");
    let dialog = dialog_of(&page, vcx);
    assert_eq!(
        dialog.read_with(vcx, |d, _| d.mode().cloned()),
        Some(Mode::FromCrate {
            name: "My Crate".into(),
            video_ids: SAVED.map(str::to_owned).to_vec(),
        })
    );
    assert_eq!(
        dialog.read_with(vcx, |d, cx| d.field().read(cx).text().to_owned()),
        "My Crate"
    );
    assert!(!dialog.read_with(vcx, |d, _| d.clear_crate()), "Clear Crate starts off");
    assert!(vcx.debug_bounds("playlist-dialog-clear-crate").is_some());
}

#[gpui::test]
fn saving_makes_one_private_playlist_and_keeps_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02", "crateCCCC03"]);
    open_crate(&page, vcx);
    click(vcx, "crate-save-playlist");
    let dialog = dialog_of(&page, vcx);
    let field = dialog.read_with(vcx, |d, _| d.field().clone());
    field.update(vcx, |f, cx| f.set_text("Gym", cx));
    dialog.update(vcx, |d, cx| d.submit(cx));
    vcx.run_until_parked();

    assert!(!dialog.read_with(vcx, |d, _| d.is_open()));
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), SAVED);
    assert_eq!(
        page.sim.write_log(),
        vec![("create-with:3".to_owned(), "Gym".to_owned())],
        "one write holds the playlist and its songs"
    );
    assert_eq!(crate_ids(&page, vcx), SAVED, "the Crate stays unless asked to clear");
    assert_eq!(notice(&page, vcx), None, "success is silent");
}

#[gpui::test]
fn clear_crate_after_saving_empties_it_once_the_playlist_exists(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02", "crateCCCC03"]);
    open_crate(&page, vcx);
    click(vcx, "crate-save-playlist");
    click(vcx, "playlist-dialog-clear-crate");
    let dialog = dialog_of(&page, vcx);
    assert!(dialog.read_with(vcx, |d, _| d.clear_crate()));
    dialog.update(vcx, |d, cx| d.submit(cx));
    vcx.run_until_parked();
    assert!(crate_ids(&page, vcx).is_empty());
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), SAVED);
}

#[gpui::test]
fn songs_set_aside_while_saving_are_not_cleared(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02"]);
    open_crate(&page, vcx);
    click(vcx, "crate-save-playlist");
    let dialog = dialog_of(&page, vcx);
    dialog.update(vcx, |d, cx| d.set_clear_crate(true, cx));
    // A song arrives while the dialog is open.
    fill(&page, vcx, &["crateNEW0004"]);
    dialog.update(vcx, |d, cx| d.submit(cx));
    vcx.run_until_parked();
    assert_eq!(crate_ids(&page, vcx), ["crateNEW0004"]);
}

#[gpui::test]
fn a_refused_save_keeps_the_dialog_and_the_crate_and_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02"]);
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    open_crate(&page, vcx);
    click(vcx, "crate-save-playlist");
    let dialog = dialog_of(&page, vcx);
    dialog.update(vcx, |d, cx| {
        d.set_clear_crate(true, cx);
        d.submit(cx)
    });
    vcx.run_until_parked();
    assert!(dialog.read_with(vcx, |d, _| d.is_open()), "the name is not lost");
    assert_eq!(crate_ids(&page, vcx).len(), 2, "nothing is cleared on a failure");
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some(failure_text(Edit::SaveCrate, WriteFailure::Rejected))
    );
}

#[gpui::test]
fn an_empty_crate_offers_neither_button(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    open_crate(&page, vcx);
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    vcx.update(|window, cx| {
        crate_page.update(cx, |p, cx| {
            p.save_as_playlist(window, cx);
            p.add_all_to_playlist(window, cx);
        })
    });
    assert!(!dialog_of(&page, vcx).read_with(vcx, |d, _| d.is_open()));
    assert!(!picker_of(&page, vcx).read_with(vcx, |p, _| p.is_open()));
}

#[gpui::test]
fn add_all_to_appends_the_crate_to_a_playlist_in_one_write(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02", "crateCCCC03"]);
    open_crate(&page, vcx);
    click(vcx, "crate-add-to-playlist");
    vcx.run_until_parked();
    let picker = picker_of(&page, vcx);
    assert!(picker.read_with(vcx, |p, _| p.is_open()));
    let targets = picker.read_with(vcx, |p, _| match p.playlists() {
        Some(apollo_app::library::model::Load::Ready(list)) => list.clone(),
        other => panic!("playlists are listed: {}", other.is_some()),
    });
    let target = targets[0].clone();
    vcx.update(|window, cx| picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();

    assert!(!picker.read_with(vcx, |p, _| p.is_open()));
    assert_eq!(
        page.sim.write_log(),
        vec![(format!("add-many:{}", target.id), SAVED.join(","))],
        "one request carries the whole crate, in order"
    );
    let added: Vec<String> = page.sim.added().into_iter().map(|(_, v)| v).collect();
    assert_eq!(added, SAVED);
    assert_eq!(crate_ids(&page, vcx), SAVED, "Add all keeps the Crate");
    assert_eq!(notice(&page, vcx), None);
}

#[gpui::test]
fn a_refused_add_all_says_so(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02"]);
    open_crate(&page, vcx);
    click(vcx, "crate-add-to-playlist");
    vcx.run_until_parked();
    page.sim
        .set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    let picker = picker_of(&page, vcx);
    vcx.update(|window, cx| picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();
    assert!(page.sim.added().is_empty());
    assert_eq!(
        notice(&page, vcx).as_deref(),
        Some("Couldn't add to playlist")
    );
}

#[gpui::test]
fn new_playlist_in_the_add_all_picker_saves_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02", "crateCCCC03"]);
    open_crate(&page, vcx);
    click(vcx, "crate-add-to-playlist");
    vcx.run_until_parked();
    let picker = picker_of(&page, vcx);
    vcx.update(|window, cx| picker.update(cx, |p, cx| p.new_playlist(window, cx)));
    vcx.run_until_parked();
    assert_eq!(
        dialog_of(&page, vcx).read_with(vcx, |d, _| d.mode().cloned()),
        Some(Mode::FromCrate {
            name: "My Crate".into(),
            video_ids: SAVED.map(str::to_owned).to_vec(),
        })
    );
}
