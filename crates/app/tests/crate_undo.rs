//! Ctrl+Z for the Crate (add, remove, bulk remove, Clear, moves) in one line of time with the
//! queue's and the playlists' undo, and "file and forget": with Settings > Playlists > "Take
//! songs out of the Crate once filed" on, songs the service confirms in a playlist leave the
//! Crate and Ctrl+Z puts them back. The real shell and views on GPUI's test platform, the
//! simulated library as the "service".

use std::sync::Arc;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::sim::{SimLibrary, WriteBehaviour};
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::CRATE_FILE_AND_FORGET;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::playlist_picker::PlaylistPicker;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::library::{PlaylistTarget, WriteFailure};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

const A: &str = "crateAAAA01";
const B: &str = "crateBBBB02";
const C: &str = "crateCCCC03";
const D: &str = "crateDDDD04";

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
        queue: ["queueTrack1", "queueTrack2", "queueTrack3"]
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
    (Page { shell, player, crate_model, sim }, vcx)
}

fn ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    page.crate_model.read_with(vcx, |m, _| {
        m.items().iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    })
}

fn fill(page: &Page, vcx: &mut VisualTestContext, songs: &[&str]) {
    page.crate_model.update(vcx, |m, cx| {
        for id in songs {
            m.add(t(id), None, cx);
        }
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

fn file_and_forget(page: &Page, vcx: &mut VisualTestContext, on: bool) {
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&CRATE_FILE_AND_FORGET, on, cx));
    vcx.run_until_parked();
}

fn target(id: &str, title: &str) -> PlaylistTarget {
    PlaylistTarget { id: id.into(), title: title.into(), thumbnail: None, holds_track: false }
}

fn add_to_playlist(page: &Page, vcx: &mut VisualTestContext, to: &PlaylistTarget, video: &str) {
    page.shell.read_with(vcx, |s, _| s.writes().clone()).update(vcx, |w, cx| {
        w.add_to_playlist(to, video, cx)
    });
    vcx.run_until_parked();
}

fn add_many_to_playlist(
    page: &Page,
    vcx: &mut VisualTestContext,
    to: &PlaylistTarget,
    videos: &[&str],
) {
    let videos: Vec<String> = videos.iter().map(|v| (*v).to_owned()).collect();
    page.shell.read_with(vcx, |s, _| s.writes().clone()).update(vcx, |w, cx| {
        w.add_many_to_playlist(to, &videos, cx)
    });
    vcx.run_until_parked();
}

fn picker_of(page: &Page, vcx: &mut VisualTestContext) -> Entity<PlaylistPicker> {
    page.shell.read_with(vcx, |s, _| s.picker().clone())
}

fn owned() -> PlaylistTarget {
    target("PLsimempty", "Nothing Yet")
}

fn v(songs: &[&str]) -> Vec<String> {
    songs.iter().map(|s| (*s).to_owned()).collect()
}

// ---- Ctrl+Z over the Crate -----------------------------------------------------------------

#[gpui::test]
fn adding_a_song_is_a_step_and_ctrl_z_takes_it_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[A, B]);
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: added a song to the Crate"));
    undo(&page, vcx);
    assert!(ids(&page, vcx).is_empty());
    // Nothing left: Ctrl+Z says nothing and changes nothing.
    undo(&page, vcx);
    assert!(ids(&page, vcx).is_empty());
}

#[gpui::test]
fn removing_one_or_several_songs_is_one_step_each(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[A, B, C, D]);

    page.crate_model.update(vcx, |m, cx| {
        assert!(m.remove(B, cx));
    });
    assert_eq!(ids(&page, vcx), [A, C, D]);
    page.crate_model.update(vcx, |m, cx| {
        assert_eq!(m.remove_many(&v(&[A, C, D]), cx), 3);
    });
    assert!(ids(&page, vcx).is_empty());

    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, C, D], "the three come back in their places");
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: removed 3 songs from the Crate")
    );
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B, C, D], "the song comes back where it was");
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: removed a song from the Crate")
    );
}

#[gpui::test]
fn clear_is_one_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[A, B, C]);
    page.crate_model.update(vcx, |m, cx| m.clear(cx));
    assert!(ids(&page, vcx).is_empty());
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B, C]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: cleared the Crate"));
}

#[gpui::test]
fn moves_and_drops_are_steps(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[A, B, C]);
    // Alt+Up / Alt+Down, and a drag.
    page.crate_model.update(vcx, |m, cx| {
        assert!(m.nudge(C, -1, cx));
        assert!(m.move_row(0, 3, cx));
    });
    assert_eq!(ids(&page, vcx), [C, B, A]);
    // A drop from outside: one new song, one that was already there.
    page.crate_model.update(vcx, |m, cx| {
        assert!(m.insert_at(vec![(t(D), None), (t(A), None)], 0, cx));
    });
    assert_eq!(ids(&page, vcx), [D, A, C, B]);

    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [C, B, A]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: added a song to the Crate"));
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, C, B]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: moved a song in the Crate"));
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B, C]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: moved a song in the Crate"));
}

#[gpui::test]
fn an_undone_crate_is_what_is_stored(cx: &mut TestAppContext) {
    // The store gets the same changes as the screen: a fresh model for the account reads back
    // what Ctrl+Z left.
    use apollo_app::crate_list::CrateStore;
    use apollo_store::CrateRow;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Memory(Mutex<Vec<CrateRow>>);
    impl CrateStore for Memory {
        fn load(&self, _: &str) -> Vec<CrateRow> {
            self.0.lock().unwrap().clone()
        }
        fn add(&self, _: &str, row: &CrateRow) {
            self.0.lock().unwrap().push(row.clone());
        }
        fn reorder(&self, _: &str, ids: &[String]) {
            let mut rows = self.0.lock().unwrap();
            rows.sort_by_key(|r| {
                ids.iter().position(|i| i == r.track.video_id.as_str()).unwrap_or(usize::MAX)
            });
        }
        fn remove(&self, _: &str, id: &str) {
            self.0.lock().unwrap().retain(|r| r.track.video_id.as_str() != id);
        }
        fn clear(&self, _: &str) {
            self.0.lock().unwrap().clear();
        }
        fn purge(&self) {
            self.0.lock().unwrap().clear();
        }
    }

    let store = Arc::new(Memory::default());
    let model = cx.new(|_| CrateModel::new());
    model.update(cx, |m, cx| {
        m.install(Some(store.clone()), cx);
        m.set_account(Some("acct".into()), cx);
        for id in [A, B, C] {
            m.add(t(id), None, cx);
        }
        m.clear(cx);
        assert_eq!(m.undo(cx).as_deref(), Some("cleared the Crate"));
        m.remove(A, cx);
        m.move_row(0, 2, cx);
    });
    let stored = |store: &Memory| -> Vec<String> {
        store.0.lock().unwrap().iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    };
    let settle = |expected: Vec<String>| {
        for _ in 0..400 {
            if stored(&store) == expected {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(stored(&store), expected);
    };
    settle(v(&[C, B]));
    model.update(cx, |m, cx| {
        m.undo(cx);
        m.undo(cx);
    });
    settle(v(&[A, B, C]));
    model.read_with(cx, |m, _| assert_eq!(m.len(), 3));
}

// ---- one line of time ----------------------------------------------------------------------

#[gpui::test]
fn ctrl_z_takes_the_newest_of_queue_crate_and_playlist(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let up_next = |page: &Page, vcx: &mut VisualTestContext| {
        page.player
            .read_with(vcx, |p, _| p.coordinator().unwrap().queue().upcoming().count())
    };
    assert_eq!(up_next(&page, vcx), 2);
    fill(&page, vcx, &[A, B]);
    let list = owned();
    add_to_playlist(&page, vcx, &list, "otherSong0001");
    page.player.update(vcx, |m, cx| m.clear_upcoming(cx));
    page.crate_model.update(vcx, |m, cx| {
        m.remove(A, cx);
    });
    assert_eq!(up_next(&page, vcx), 0, "the up-next list is cleared");

    // Newest first: the Crate removal, then the queue, then the playlist add, then the Crate adds.
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B], "the Crate came first");
    assert_eq!(up_next(&page, vcx), 0);
    undo(&page, vcx);
    assert_eq!(up_next(&page, vcx), 2, "then the queue");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"));
    undo(&page, vcx);
    assert_eq!(
        pill(&page, vcx).as_deref(),
        Some("Undid: added a song to Nothing Yet"),
        "then the playlist edit"
    );
    assert_eq!(ids(&page, vcx), [A, B], "the playlist undo left the Crate alone");
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A]);
    undo(&page, vcx);
    assert!(ids(&page, vcx).is_empty());
}

#[gpui::test]
fn the_key_works_with_the_crate_page_open(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[A, B, C]);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    page.crate_model.update(vcx, |m, cx| {
        m.remove_many(&v(&[A, B]), cx);
    });
    vcx.run_until_parked();
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(ids(&page, vcx), [A, B, C]);
    assert!(vcx.debug_bounds("crate-row-2").is_some(), "the page shows them again");
}

// ---- file and forget -----------------------------------------------------------------------

#[gpui::test]
fn off_by_default_nothing_leaves_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &[A, B]);
    assert!(!page.shell.read_with(vcx, |s, cx| s.settings(cx).read(cx).get(&CRATE_FILE_AND_FORGET)));
    add_to_playlist(&page, vcx, &owned(), A);
    add_many_to_playlist(&page, vcx, &owned(), &[A, B]);
    assert_eq!(ids(&page, vcx), [A, B]);
}

#[gpui::test]
fn a_song_filed_with_a_single_add_leaves_and_ctrl_z_brings_it_back(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    fill(&page, vcx, &[A, B, C]);
    add_to_playlist(&page, vcx, &owned(), B);
    assert_eq!(ids(&page, vcx), [A, C]);

    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B, C], "back in its place");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: took a song out of the Crate"));
    // The playlist add is its own step, taken back by the next Ctrl+Z.
    undo(&page, vcx);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: added a song to Nothing Yet"));
}

#[gpui::test]
fn songs_filed_together_leave_together_as_one_step(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    fill(&page, vcx, &[A, B, C, D]);
    // Songs not in the Crate (here, X) are none of its business.
    add_many_to_playlist(&page, vcx, &owned(), &[D, "elsewhere001", B]);
    assert_eq!(ids(&page, vcx), [A, C]);
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B, C, D]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: took 2 songs out of the Crate"));
}

#[gpui::test]
fn a_refused_add_takes_nothing_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    fill(&page, vcx, &[A, B]);
    page.sim.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
    add_to_playlist(&page, vcx, &owned(), A);
    add_many_to_playlist(&page, vcx, &owned(), &[A, B]);
    assert_eq!(ids(&page, vcx), [A, B], "nothing is filed until the service says so");
}

#[gpui::test]
fn the_picker_files_when_it_closes_not_before(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    // Keep picker open: one song can go into several playlists in one visit.
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |s, cx| s.set(&apollo_app::settings::PICKER_STAY_OPEN, true, cx));
    fill(&page, vcx, &[A, B, C]);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let at = vcx.debug_bounds("crate-add-to-playlist").expect("on screen").center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    let picker = picker_of(&page, vcx);
    assert!(picker.read_with(vcx, |p, _| p.is_open()));

    vcx.update(|window, cx| picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();
    assert!(picker.read_with(vcx, |p, _| p.is_open()), "it stays open for the next playlist");
    assert_eq!(page.sim.added().len(), 3, "the add went through");
    assert_eq!(ids(&page, vcx), [A, B, C], "the songs wait in the Crate while it is open");

    vcx.update(|window, cx| picker.update(cx, |p, cx| p.close(window, cx)));
    vcx.run_until_parked();
    assert!(ids(&page, vcx).is_empty(), "closing the picker files them");
    undo(&page, vcx);
    assert_eq!(ids(&page, vcx), [A, B, C]);
    assert_eq!(pill(&page, vcx).as_deref(), Some("Undid: took 3 songs out of the Crate"));
}

#[gpui::test]
fn a_picker_that_closes_at_once_files_the_songs_once_the_service_answers(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    fill(&page, vcx, &[A, B]);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let at = vcx.debug_bounds("crate-add-to-playlist").expect("on screen").center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    let picker = picker_of(&page, vcx);
    vcx.update(|window, cx| picker.update(cx, |p, cx| p.choose(0, window, cx)));
    vcx.run_until_parked();
    assert!(!picker.read_with(vcx, |p, _| p.is_open()));
    assert!(ids(&page, vcx).is_empty());
}

#[gpui::test]
fn a_playlist_made_in_the_picker_files_its_songs_too(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    fill(&page, vcx, &[A, B, C]);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let at = vcx.debug_bounds("crate-add-to-playlist").expect("on screen").center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
    let picker = picker_of(&page, vcx);
    let field = picker.read_with(vcx, |p, _| p.field().clone());
    field.update(vcx, |f, cx| f.set_text("Fresh", cx));
    vcx.run_until_parked();
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let made = page.sim.owned_ids()[0].clone();
    assert_eq!(page.sim.created_with(&made), v(&[A, B, C]));
    assert!(ids(&page, vcx).is_empty());
}

#[gpui::test]
fn save_as_playlist_still_asks_in_its_own_dialog_and_the_crate_stays_otherwise(
    cx: &mut TestAppContext,
) {
    // Saving makes the playlist in one write that is not an "add": it files nothing by itself.
    let (page, vcx) = setup(cx);
    file_and_forget(&page, vcx, true);
    fill(&page, vcx, &[A, B]);
    page.shell.update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    let at = vcx.debug_bounds("crate-save-playlist").expect("on screen").center();
    vcx.simulate_click(at, Modifiers::none());
    let dialog = page.shell.read_with(vcx, |s, _| s.dialog().clone());
    dialog.update(vcx, |d, cx| d.submit(cx));
    vcx.run_until_parked();
    assert_eq!(ids(&page, vcx), [A, B]);
}
