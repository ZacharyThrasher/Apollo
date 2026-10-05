//! Queue undo (Ctrl+Z / Ctrl+Shift+Z) and the Crate (C, "Add to Crate", the Crate page). The real
//! shell and views on GPUI's test platform with the deterministic mock playback adapters.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::{CrateModel, CrateStore};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{self, Anchor, Command, Subject};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, Phase};
use apollo_domain::queue::{QueueSource, SourceKind};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use apollo_store::CrateRow;
use gpui::{point, px, AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

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
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    crate_model: Entity<CrateModel>,
}

fn t(id: &str) -> Track {
    Track::new(id, format!("Song {id}"))
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
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
        library: Arc::new(SimLibrary::instant()),
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
            player,
            shell,
            crate_model,
        },
        vcx,
    )
}

/// Open the current load and report the engine at `position_ms`: the track is playing.
fn play_to(page: &Page, vcx: &mut VisualTestContext, position_ms: u64) {
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    page.player.update(vcx, |m, cx| {
        m.handle(
            Event::Resolved {
                load,
                result: Ok(LeaseId(1)),
            },
            cx,
        );
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Opened {
                    duration_ms: Some(240_000),
                },
            },
            cx,
        );
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Position(position_ms),
            },
            cx,
        );
    });
}

fn queue(page: &Page, vcx: &mut VisualTestContext) -> (String, Vec<String>) {
    page.player.read_with(vcx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        (
            q.current().unwrap().track.video_id.as_str().to_owned(),
            q.upcoming()
                .map(|e| e.track.video_id.as_str().to_owned())
                .collect(),
        )
    })
}

fn state(page: &Page, vcx: &mut VisualTestContext) -> (Intent, Phase, u64) {
    page.player.read_with(vcx, |m, _| {
        let c = m.coordinator().unwrap();
        (c.intent(), c.phase(), c.position_ms())
    })
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
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

// ---- queue undo ------------------------------------------------------------------------------

#[gpui::test]
fn ctrl_z_takes_back_clear_and_ctrl_shift_z_does_it_again(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    play_to(&page, vcx, 12_000);
    let before = queue(&page, vcx);
    assert_eq!(before.1.len(), 3);

    page.player.update(vcx, |m, cx| m.clear_upcoming(cx));
    assert!(queue(&page, vcx).1.is_empty());

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(queue(&page, vcx), before, "the up-next list is back");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"));
    // What was playing never noticed.
    assert_eq!(state(&page, vcx), (Intent::Playing, Phase::Playing, 12_000));

    vcx.simulate_keystrokes("ctrl-shift-z");
    assert!(queue(&page, vcx).1.is_empty(), "cleared again");
    assert_eq!(state(&page, vcx).1, Phase::Playing);
}

#[gpui::test]
fn the_pill_goes_away_after_two_seconds(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player.update(vcx, |m, cx| m.clear_upcoming(cx));
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(pill(&page, vcx).as_deref(), Some("Queue restored"));
    assert!(vcx.debug_bounds("queue-pill").is_some(), "it is on screen");
    vcx.executor().advance_clock(Duration::from_millis(1_900));
    vcx.run_until_parked();
    assert!(pill(&page, vcx).is_some());
    vcx.executor().advance_clock(Duration::from_millis(200));
    vcx.run_until_parked();
    assert_eq!(pill(&page, vcx), None);
}

#[gpui::test]
fn nothing_to_undo_says_nothing_and_changes_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let before = queue(&page, vcx);
    vcx.simulate_keystrokes("ctrl-z ctrl-shift-z");
    assert_eq!(queue(&page, vcx), before);
    assert_eq!(pill(&page, vcx), None);
}

#[gpui::test]
fn undoing_a_replaced_queue_brings_back_the_old_track_paused_where_it_was(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    play_to(&page, vcx, 95_000);
    let before = queue(&page, vcx);

    // The classic accident: a click on a song in another list replaces the whole queue.
    page.player.update(vcx, |m, cx| {
        m.play_tracks(vec![t("otherXXXX01"), t("otherYYYY02")], Some(0), None, cx)
    });
    assert_eq!(queue(&page, vcx).0, "otherXXXX01");

    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(queue(&page, vcx), before, "the old queue is back");
    // The track that was playing comes back, held and ready where it was.
    assert_eq!(state(&page, vcx), (Intent::Paused, Phase::Idle, 95_000));

    // Redo goes forward again.
    vcx.simulate_keystrokes("ctrl-shift-z");
    assert_eq!(queue(&page, vcx).0, "otherXXXX01");
}

#[gpui::test]
fn undoing_an_edit_never_interrupts_another_track_that_is_playing_now(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("space");
    play_to(&page, vcx, 1_000);
    // Remove the second upcoming entry, then move on to the first.
    let removed = page.player.read_with(vcx, |m, _| {
        m.coordinator().unwrap().queue().upcoming_ids()[1]
    });
    page.player.update(vcx, |m, cx| m.remove_entry(removed, cx));
    page.player.update(vcx, |m, cx| m.next(cx));
    play_to(&page, vcx, 7_000);
    assert_eq!(queue(&page, vcx).0, LIST[1]);
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load());

    vcx.simulate_keystrokes("ctrl-z");
    let (current, upcoming) = queue(&page, vcx);
    assert_eq!(current, LIST[1], "still the same song");
    assert_eq!(upcoming, [LIST[2], LIST[3]], "the removed one is back");
    assert_eq!(state(&page, vcx), (Intent::Playing, Phase::Playing, 7_000));
    assert_eq!(
        page.player
            .read_with(vcx, |m, _| m.coordinator().unwrap().active_load()),
        load,
        "the same load keeps the output"
    );
}

#[gpui::test]
fn more_than_twenty_steps_can_be_undone(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    for i in 0..25 {
        page.player
            .update(vcx, |m, cx| m.add_to_queue(t(&format!("added{i:05}")), cx));
    }
    assert_eq!(queue(&page, vcx).1.len(), 3 + 25);
    for _ in 0..25 {
        vcx.simulate_keystrokes("ctrl-z");
    }
    assert_eq!(queue(&page, vcx).1.len(), 3, "every add was taken back");
}

#[gpui::test]
fn start_radio_is_one_step_and_so_is_everything_else_that_changes_the_queue(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    let before = queue(&page, vcx);
    page.player
        .update(vcx, |m, cx| m.start_radio(t("seedSEED001"), cx));
    page.player.update(vcx, |m, cx| {
        m.finish_radio(vec![t("radioRADIO1"), t("radioRADIO2")], QueueSource::new(SourceKind::Radio, "seed"), cx)
    });
    assert_eq!(queue(&page, vcx).0, "seedSEED001");
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(queue(&page, vcx), before, "one undo takes the whole radio back");

    // Shuffle, play next, add and remove are steps too.
    page.player.update(vcx, |m, cx| m.toggle_shuffle(cx));
    assert!(page.player.read_with(vcx, |m, _| m.view().shuffle));
    vcx.simulate_keystrokes("ctrl-z");
    assert!(!page.player.read_with(vcx, |m, _| m.view().shuffle));
    page.player.update(vcx, |m, cx| m.play_next(t("nextNEXT001"), cx));
    assert_eq!(queue(&page, vcx).1[0], "nextNEXT001");
    vcx.simulate_keystrokes("ctrl-z");
    assert_eq!(queue(&page, vcx), before);
}

#[gpui::test]
fn undo_keys_are_left_to_text_fields(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player.update(vcx, |m, cx| m.clear_upcoming(cx));
    vcx.simulate_keystrokes("ctrl-k");
    vcx.simulate_keystrokes("ctrl-z");
    assert!(queue(&page, vcx).1.is_empty(), "typing context: no undo");
    assert_eq!(pill(&page, vcx), None);
}

// ---- the Crate -------------------------------------------------------------------------------

#[gpui::test]
fn c_sets_the_playing_song_aside_once(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(crate_ids(&page, vcx).is_empty());
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [LIST[0]]);
    vcx.simulate_keystrokes("c");
    assert_eq!(crate_ids(&page, vcx), [LIST[0]], "pressing it again adds nothing");
    vcx.simulate_keystrokes("shift-right c");
    assert_eq!(crate_ids(&page, vcx), [LIST[0], LIST[1]], "the new song goes below");
    // The queue was not touched.
    assert_eq!(queue(&page, vcx).0, LIST[1]);
}

#[gpui::test]
fn c_is_a_letter_in_a_text_field(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    vcx.simulate_keystrokes("ctrl-k c");
    assert!(crate_ids(&page, vcx).is_empty());
}

#[gpui::test]
fn the_song_menu_offers_add_to_crate_and_then_remove(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let open = |vcx: &mut VisualTestContext, page: &Page| {
        page.shell.update_in(vcx, |_, window, cx| {
            track_menu::open(
                Subject::track(t("menuTrack01"), None),
                Anchor::Point(point(px(300.), px(300.))),
                window,
                cx,
            );
        });
        vcx.run_until_parked();
        page.shell
            .read_with(vcx, |s, cx| s.menu().read(cx).commands(cx))
    };
    let commands = open(vcx, &page);
    assert!(commands.contains(&Command::AddToCrate));
    assert!(!commands.contains(&Command::RemoveFromCrate));

    let menu = page.shell.read_with(vcx, |s, _| s.menu().clone());
    let index = commands
        .iter()
        .position(|c| *c == Command::AddToCrate)
        .unwrap();
    menu.update_in(vcx, |m, window, cx| m.choose(index, window, cx));
    vcx.run_until_parked();
    assert_eq!(crate_ids(&page, vcx), ["menuTrack01"]);

    let commands = open(vcx, &page);
    assert!(commands.contains(&Command::RemoveFromCrate));
    assert!(!commands.contains(&Command::AddToCrate));
    let index = commands
        .iter()
        .position(|c| *c == Command::RemoveFromCrate)
        .unwrap();
    menu.update_in(vcx, |m, window, cx| m.choose(index, window, cx));
    vcx.run_until_parked();
    assert!(crate_ids(&page, vcx).is_empty());
}

fn fill(page: &Page, vcx: &mut VisualTestContext, ids: &[&str]) {
    page.crate_model.update(vcx, |m, cx| {
        for id in ids {
            m.add(t(id), None, cx);
        }
    });
    vcx.run_until_parked();
}

#[gpui::test]
fn the_crate_page_lists_plays_and_queues_the_crate(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02", "crateCCCC03"]);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("crate-page").is_some());
    for ix in 0..3 {
        let id: &'static str = Box::leak(format!("crate-row-{ix}").into_boxed_str());
        assert!(vcx.debug_bounds(id).is_some(), "row {ix} is listed");
    }

    // Add all to queue: the crate, top to bottom, goes up next like any other "add to queue".
    click(vcx, "crate-add-all");
    assert_eq!(
        queue(&page, vcx).1[..3],
        ["crateAAAA01", "crateBBBB02", "crateCCCC03"]
    );

    // A row plays with the crate as the queue.
    click(vcx, "crate-row-1");
    assert_eq!(
        queue(&page, vcx),
        ("crateBBBB02".to_owned(), vec!["crateCCCC03".to_owned()])
    );
    assert_eq!(state(&page, vcx).0, Intent::Playing);

    // Play starts the crate from the top.
    click(vcx, "crate-play");
    assert_eq!(queue(&page, vcx).0, "crateAAAA01");
}

#[gpui::test]
fn clear_asks_first_and_cancel_keeps_everything(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02"]);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();

    click(vcx, "crate-clear");
    assert_eq!(crate_ids(&page, vcx).len(), 2, "pressing Clear alone clears nothing");
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    assert!(crate_page.read_with(vcx, |p, _| p.confirming()));

    click(vcx, "crate-clear-no");
    assert_eq!(crate_ids(&page, vcx).len(), 2);
    assert!(!crate_page.read_with(vcx, |p, _| p.confirming()));

    click(vcx, "crate-clear");
    click(vcx, "crate-clear-yes");
    assert!(crate_ids(&page, vcx).is_empty());
    assert!(!crate_page.read_with(vcx, |p, _| p.confirming()));
    assert!(vcx.debug_bounds("crate-search").is_some(), "the empty page offers Search");
}

#[gpui::test]
fn a_row_can_be_removed_from_the_crate_page(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    fill(&page, vcx, &["crateAAAA01", "crateBBBB02"]);
    page.shell
        .update(vcx, |s, cx| s.show_route(Route::Crate, cx));
    vcx.run_until_parked();
    // The button is there (hidden until hovered); the page removes through the same call.
    assert!(vcx.debug_bounds("crate-remove-0").is_some());
    let crate_page = page.shell.read_with(vcx, |s, _| s.crate_page().clone());
    crate_page.update(vcx, |p, cx| p.remove("crateBBBB02", cx));
    assert_eq!(crate_ids(&page, vcx), ["crateAAAA01"]);
}

// ---- keeping it ------------------------------------------------------------------------------

#[derive(Default)]
struct Memory {
    rows: Mutex<Vec<(String, CrateRow)>>,
}

impl CrateStore for Memory {
    fn load(&self, key: &str) -> Vec<CrateRow> {
        let mut rows = self.rows.lock().unwrap();
        rows.retain(|(k, _)| k == key);
        rows.iter().map(|(_, r)| r.clone()).collect()
    }
    fn add(&self, key: &str, row: &CrateRow) {
        self.rows.lock().unwrap().push((key.into(), row.clone()));
    }
    fn reorder(&self, key: &str, video_ids: &[String]) {
        let mut rows = self.rows.lock().unwrap();
        rows.sort_by_key(|(k, r)| {
            if k == key {
                video_ids
                    .iter()
                    .position(|id| id == r.track.video_id.as_str())
                    .unwrap_or(usize::MAX)
            } else {
                0
            }
        });
    }
    fn remove(&self, key: &str, video_id: &str) {
        self.rows
            .lock()
            .unwrap()
            .retain(|(k, r)| !(k == key && r.track.video_id.as_str() == video_id));
    }
    fn swap(&self, key: &str, old_video_id: &str, row: &CrateRow) {
        let mut rows = self.rows.lock().unwrap();
        let taken = row.track.video_id.as_str() != old_video_id
            && rows
                .iter()
                .any(|(k, r)| k == key && r.track.video_id == row.track.video_id);
        if taken {
            return;
        }
        if let Some((_, r)) = rows
            .iter_mut()
            .find(|(k, r)| k == key && r.track.video_id.as_str() == old_video_id)
        {
            *r = row.clone();
        }
    }
    fn clear(&self, key: &str) {
        self.rows.lock().unwrap().retain(|(k, _)| k != key);
    }
    fn purge(&self) {
        self.rows.lock().unwrap().clear();
    }
}

#[gpui::test]
fn the_crate_survives_a_restart_for_the_same_account_only(cx: &mut TestAppContext) {
    let store = Arc::new(Memory::default());
    let key = {
        let (page, vcx) = setup(cx);
        page.crate_model
            .update(vcx, |m, cx| m.install(Some(store.clone()), cx));
        let key = page
            .crate_model
            .read_with(vcx, |m, _| m.account().map(str::to_owned))
            .expect("the shell gave the crate its account");
        page.crate_model.update(vcx, |m, cx| {
            m.add(t("keepKEEP001"), None, cx);
        });
        for _ in 0..200 {
            if store.rows.lock().unwrap().len() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(store.rows.lock().unwrap()[0].0, key, "kept under the account");
        key
    };
    let (page, vcx) = setup(cx);
    page.crate_model
        .update(vcx, |m, cx| m.install(Some(store.clone()), cx));
    vcx.run_until_parked();
    assert_eq!(
        page.crate_model
            .read_with(vcx, |m, _| m.account().map(str::to_owned)),
        Some(key)
    );
    assert_eq!(crate_ids(&page, vcx), ["keepKEEP001"]);
    page.crate_model
        .update(vcx, |m, cx| m.set_account(Some("someone-else".into()), cx));
    vcx.run_until_parked();
    assert!(crate_ids(&page, vcx).is_empty(), "another account sees nothing");
}

#[gpui::test]
fn c_in_the_queue_list_sets_the_row_under_the_cursor_aside(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.shell.update(vcx, |s, cx| s.toggle_queue(cx));
    vcx.run_until_parked();
    let panel = page.shell.read_with(vcx, |s, _| s.queue_panel().clone());
    vcx.update(|window, cx| {
        let focus = gpui::Focusable::focus_handle(panel.read(cx), cx);
        window.focus(&focus, cx);
    });
    // The first press shows the cursor on the playing row; two more reach the second up next.
    vcx.simulate_keystrokes("down down down c");
    assert_eq!(crate_ids(&page, vcx), [LIST[2]], "the row, not the playing song");
}
