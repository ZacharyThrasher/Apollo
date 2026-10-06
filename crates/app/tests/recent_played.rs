//! "Recently played": the Crate page's collapsible section and the dock's Recent tab list the
//! songs of the local play log from the last week, newest first, and act like other song rows
//! (C, the hover button, Enter, multi-select, drag into the Crate). The real shell and views on
//! GPUI's test platform; the play log is a temporary SQLite file.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::crate_list::CrateModel;
use apollo_app::library::sim::SimLibrary;
use apollo_app::page_cache::PageCache;
use apollo_app::player::PlayerModel;
use apollo_app::recent::RecentModel;
use apollo_app::settings::SettingsBackend;
use apollo_app::stats::{PlayLog, PlayRecord};
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::blocklist::BlockedArtist;
use apollo_domain::model::{ArtistRef, BrowseId, Track};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{
    point, AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext,
    VisualTestContext,
};

const ACCOUNT: &str = "Demo Listener";
const HOUR: i64 = 60 * 60;
const DAY: i64 = 24 * HOUR;

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

/// A settings store in memory.
#[derive(Default)]
struct Memory(Mutex<HashMap<String, String>>);

impl SettingsBackend for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) {
        self.0.lock().unwrap().insert(key.into(), value.into());
    }
}

struct Rig {
    log: PlayLog,
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    crate_model: Entity<CrateModel>,
    recent: Entity<RecentModel>,
    /// Keeps the log's file until the test ends.
    _dir: tempfile::TempDir,
}

fn rig(cx: &mut TestAppContext) -> (Rig, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    let log = PlayLog::new(Some(dir.path().join("apollo.db")));
    let cache = PageCache::in_memory();
    cache.set_account(Some(ACCOUNT.into()));
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
        cx.set_reduce_motion(true);
        cache.clone().install(cx);
        log.clone().install(cx);
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
        queue: vec![LibraryTrack {
            track: Track::new("startTrack01", "Start"),
        }],
        art: Arc::new(TestArt),
        search: Arc::new(apollo_app::search::Unavailable),
        thumbs: Arc::new(apollo_app::url_thumbs::NoThumbs),
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
    let recent = vcx.update(|_, cx| apollo_app::recent::handle(cx).expect("the shell makes the list"));
    (
        Rig {
            log,
            player,
            shell,
            crate_model,
            recent,
            _dir: dir,
        },
        vcx,
    )
}

fn track(video: &str, artist: Option<(&str, &str)>) -> Track {
    let mut t = Track::new(video, format!("Song {video}"));
    if let Some((name, id)) = artist {
        t.artists = vec![ArtistRef {
            name: name.into(),
            id: Some(BrowseId::new(id)),
        }];
    }
    t.duration = Some(Duration::from_secs(200));
    t
}

/// Writes one counted play of `video`, `ago` seconds ago.
fn play(rig: &Rig, video: &str, ago: i64, key: u64) {
    play_by(rig, track(video, None), ago, key);
}

fn play_by(rig: &Rig, track: Track, ago: i64, key: u64) {
    let now = apollo_services_common::unix_now() as i64;
    rig.log.record(
        ACCOUNT.into(),
        key,
        PlayRecord {
            art: Some(format!("https://example.com/{}.jpg", track.video_id.as_str())),
            track,
            played_unix: now - ago,
            listened_ms: 180_000,
        },
    );
}

/// Three plays an hour apart: C is the newest.
fn three(rig: &Rig) {
    play(rig, "recentAAA01", 3 * HOUR, 1);
    play(rig, "recentBBB02", 2 * HOUR, 2);
    play(rig, "recentCCC03", HOUR, 3);
    rig.log.flush();
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

fn route(rig: &Rig, vcx: &mut VisualTestContext, route: Route) {
    rig.shell
        .update_in(vcx, |s, window, cx| s.choose_route(route, window, cx));
    vcx.run_until_parked();
}

/// The Crate page with its Recently played section open and the rows read.
fn open_section(rig: &Rig, vcx: &mut VisualTestContext) {
    route(rig, vcx, Route::Crate);
    if !on_screen(vcx, "recent-row-0") {
        click(vcx, "recent-toggle");
    }
    vcx.run_until_parked();
}

fn shown(rig: &Rig, vcx: &mut VisualTestContext) -> Vec<String> {
    rig.recent.read_with(vcx, |m, _| {
        m.rows()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

fn crate_ids(rig: &Rig, vcx: &mut VisualTestContext) -> Vec<String> {
    rig.crate_model.read_with(vcx, |m, _| {
        m.items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    })
}

/// The keyboard on the section's list, on its first row.
fn enter_list(rig: &Rig, vcx: &mut VisualTestContext) {
    let list = rig.shell.read_with(vcx, |s, cx| s.crate_page().read(cx).recent().clone());
    let entered = list.update_in(vcx, |l, window, cx| l.enter(window, cx));
    assert!(entered, "there is a row to land on");
    vcx.run_until_parked();
}

// ---- the section ------------------------------------------------------------------------------

#[gpui::test]
fn the_section_starts_collapsed_and_a_click_opens_it(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    route(&rig, vcx, Route::Crate);
    assert!(on_screen(vcx, "recent-toggle"), "the header line is there");
    assert!(!on_screen(vcx, "recent-row-0"), "collapsed: no rows");

    click(vcx, "recent-toggle");
    for ix in 0..3 {
        assert!(on_screen(vcx, &format!("recent-row-{ix}")), "row {ix}");
    }
    assert!(!on_screen(vcx, "recent-row-3"));
    assert_eq!(
        shown(&rig, vcx),
        ["recentCCC03", "recentBBB02", "recentAAA01"],
        "newest first"
    );

    click(vcx, "recent-toggle");
    assert!(!on_screen(vcx, "recent-row-0"), "a second click closes it");
}

#[gpui::test]
fn the_list_is_the_last_week_with_consecutive_repeats_collapsed(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    play(&rig, "weekOldAA01", 8 * DAY, 1);
    play(&rig, "olderBBBB02", 3 * DAY, 2);
    play(&rig, "repeatCCC03", 3 * HOUR, 3);
    play(&rig, "repeatCCC03", 2 * HOUR, 4);
    play(&rig, "newestDDD04", HOUR, 5);
    play(&rig, "repeatCCC03", 60, 6);
    rig.log.flush();
    open_section(&rig, vcx);
    assert_eq!(
        shown(&rig, vcx),
        ["repeatCCC03", "newestDDD04", "repeatCCC03", "olderBBBB02"],
        "a song heard again after another is a new row; the eight-day-old play is out"
    );
}

#[gpui::test]
fn at_most_two_hundred_rows_are_listed(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    for i in 0..230u64 {
        play(&rig, &format!("bulkSong{i:04}"), 60 + i as i64, i);
    }
    rig.log.flush();
    open_section(&rig, vcx);
    let rows = shown(&rig, vcx);
    assert_eq!(rows.len(), 200);
    assert_eq!(rows[0], "bulkSong0000");
    assert_eq!(rows[199], "bulkSong0199");
}

#[gpui::test]
fn blocked_artists_are_left_out(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    play_by(&rig, track("goodSong0001", Some(("Fine", "UCfine"))), 2 * HOUR, 1);
    play_by(&rig, track("badSong00002", Some(("Banned", "UCban"))), HOUR, 2);
    rig.log.flush();
    let model = vcx.update(|_, cx| apollo_app::blocked_artists::handle(cx).expect("the shell's list"));
    model.update(vcx, |m, cx| {
        m.block(
            BlockedArtist {
                id: Some("UCban".into()),
                name: "Banned".into(),
            },
            cx,
        );
    });
    open_section(&rig, vcx);
    assert_eq!(shown(&rig, vcx), ["goodSong0001"]);
}

#[gpui::test]
fn with_nothing_played_the_section_says_so(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    route(&rig, vcx, Route::Crate);
    click(vcx, "recent-toggle");
    assert!(on_screen(vcx, "recent-empty"));
    assert!(!on_screen(vcx, "recent-row-0"));
}

#[gpui::test]
fn a_play_that_counts_while_it_is_open_appears_without_reopening(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    open_section(&rig, vcx);
    assert_eq!(shown(&rig, vcx).len(), 3);

    play(&rig, "justNowDD004", 5, 4);
    rig.log.flush();
    // Any change the list already watches (here the player's) draws it again, and the new write
    // makes it read the log.
    rig.player.update(vcx, |_, cx| cx.notify());
    vcx.run_until_parked();
    assert_eq!(shown(&rig, vcx)[0], "justNowDD004");
    assert_eq!(shown(&rig, vcx).len(), 4);
}

#[gpui::test]
fn the_open_state_is_remembered_across_restarts(cx: &mut TestAppContext) {
    let store = Arc::new(Memory::default());
    {
        let (rig, vcx) = rig(cx);
        rig.shell
            .update(vcx, |s, cx| s.install_crate_dock(Some(store.clone()), cx));
        vcx.run_until_parked();
        route(&rig, vcx, Route::Crate);
        click(vcx, "recent-toggle");
        // The write goes through the background executor.
        for _ in 0..200 {
            if store.get("crate_recent_open").as_deref() == Some("on") {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(store.get("crate_recent_open").as_deref(), Some("on"));
    }
    let (rig, vcx) = rig(cx);
    three(&rig);
    rig.shell
        .update(vcx, |s, cx| s.install_crate_dock(Some(store.clone()), cx));
    vcx.run_until_parked();
    route(&rig, vcx, Route::Crate);
    assert!(on_screen(vcx, "recent-row-0"), "open as it was left");
}

// ---- the rows ---------------------------------------------------------------------------------

#[gpui::test]
fn c_sets_the_cursor_song_aside_and_the_cursor_steps_on(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    open_section(&rig, vcx);
    enter_list(&rig, vcx);

    vcx.simulate_keystrokes("c");
    vcx.run_until_parked();
    assert_eq!(crate_ids(&rig, vcx), ["recentCCC03"], "the newest row first");
    vcx.simulate_keystrokes("c");
    vcx.run_until_parked();
    assert_eq!(
        crate_ids(&rig, vcx),
        ["recentCCC03", "recentBBB02"],
        "the cursor stepped to the next row"
    );
}

#[gpui::test]
fn the_hover_button_sets_a_song_aside_once(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    open_section(&rig, vcx);
    click(vcx, "recent-add-1");
    assert_eq!(crate_ids(&rig, vcx), ["recentBBB02"]);
    // The song is in the Crate now: its button is gone, so a click there adds nothing.
    click(vcx, "recent-add-1");
    assert_eq!(crate_ids(&rig, vcx), ["recentBBB02"]);
}

#[gpui::test]
fn ctrl_a_then_c_sets_every_row_aside(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    open_section(&rig, vcx);
    enter_list(&rig, vcx);
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_keystrokes("c");
    vcx.run_until_parked();
    assert_eq!(
        crate_ids(&rig, vcx),
        ["recentCCC03", "recentBBB02", "recentAAA01"],
        "every row, in the list's order"
    );
}

#[gpui::test]
fn enter_plays_the_row_with_the_list_as_the_queue(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    open_section(&rig, vcx);
    enter_list(&rig, vcx);
    vcx.simulate_keystrokes("down");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    let playing = rig.player.read_with(vcx, |m, _| m.current_video());
    assert_eq!(playing.as_deref(), Some("recentBBB02"));
}

#[gpui::test]
fn a_row_dragged_onto_the_crate_lands_where_it_is_dropped(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    rig.crate_model.update(vcx, |m, cx| {
        m.add(track("crateAAAA01", None), None, cx);
        m.add(track("crateBBBB02", None), None, cx);
    });
    open_section(&rig, vcx);

    // From the title side of the second row to the top of the Crate's second row.
    let from = {
        let b = bounds(vcx, "recent-row-1");
        point(b.origin.x + gpui::px(120.0), b.center().y)
    };
    let to = {
        let b = bounds(vcx, "crate-row-1");
        point(b.center().x, b.origin.y + gpui::px(8.0))
    };
    drag(vcx, from, to);
    assert_eq!(
        crate_ids(&rig, vcx),
        ["crateAAAA01", "recentBBB02", "crateBBBB02"]
    );
}

fn drag(vcx: &mut VisualTestContext, start: Point<Pixels>, to: Point<Pixels>) {
    vcx.simulate_mouse_move(start, None, Modifiers::none());
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    for step in 1..=8 {
        let t = step as f32 / 8.0;
        let at = point(
            start.x + (to.x - start.x) * t,
            start.y + (to.y - start.y) * t,
        );
        vcx.simulate_mouse_move(at, Some(MouseButton::Left), Modifiers::none());
    }
    vcx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
}

// ---- the dock's Recent tab --------------------------------------------------------------------

#[gpui::test]
fn the_dock_has_a_recent_tab_beside_queue_and_crate(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    route(&rig, vcx, Route::Library);
    click(vcx, "bar-crate");
    for tab in ["dock-tab-Queue", "dock-tab-Crate", "dock-tab-Recent"] {
        assert!(on_screen(vcx, tab), "{tab}");
    }
    assert!(!on_screen(vcx, "recent-dock-row-0"), "the Crate shows first");

    click(vcx, "dock-tab-Recent");
    assert_eq!(bounds(vcx, "crate-dock").size.width, gpui::px(320.0));
    assert!(on_screen(vcx, "recent-dock-row-0") && on_screen(vcx, "recent-dock-row-2"));
    assert!(!on_screen(vcx, "dock-count"), "the Crate's own header is not drawn");
    assert_eq!(
        shown(&rig, vcx),
        ["recentCCC03", "recentBBB02", "recentAAA01"]
    );

    click(vcx, "dock-tab-Crate");
    assert!(on_screen(vcx, "dock-count") && !on_screen(vcx, "recent-dock-row-0"));

    // From the queue, the Recent tab opens the panel on the list.
    click(vcx, "dock-tab-Recent");
    click(vcx, "dock-tab-Queue");
    assert!(on_screen(vcx, "queue-panel") && !on_screen(vcx, "crate-dock"));
    click(vcx, "dock-tab-Recent");
    assert!(on_screen(vcx, "recent-dock-row-0") && !on_screen(vcx, "queue-panel"));
}

#[gpui::test]
fn c_works_in_the_dock_tab_and_the_row_menu_button_is_there(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    route(&rig, vcx, Route::Library);
    click(vcx, "bar-crate");
    click(vcx, "dock-tab-Recent");
    assert!(on_screen(vcx, "recent-dock-more-0"));
    let panel = rig.shell.read_with(vcx, |s, _| s.crate_dock_panel().clone());
    let entered = panel.update_in(vcx, |p, window, cx| p.enter(window, cx));
    assert!(entered, "Ctrl+Shift+C lands in the list");
    vcx.simulate_keystrokes("c");
    vcx.run_until_parked();
    assert_eq!(crate_ids(&rig, vcx), ["recentCCC03"]);
}

#[gpui::test]
fn ctrl_shift_c_lands_in_the_recent_tab_and_esc_goes_back(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    three(&rig);
    rig.crate_model.update(vcx, |m, cx| {
        m.add(track("crateAAAA01", None), None, cx);
    });
    route(&rig, vcx, Route::Library);
    click(vcx, "bar-crate");
    click(vcx, "dock-tab-Recent");
    rig.shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    rig.shell
        .update_in(vcx, |s, window, cx| s.jump_crate_dock(window, cx));
    vcx.run_until_parked();
    let landed = vcx.update(|window, cx| rig.shell.read(cx).crate_dock_focused(window, cx));
    assert!(landed, "the jump landed in the Recent list");
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    let still_in_dock = vcx.update(|window, cx| {
        rig.shell.read(cx).crate_dock_focused(window, cx)
    });
    assert!(!still_in_dock, "Esc went back to where the keyboard was");
}
