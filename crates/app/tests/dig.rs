//! Dig from here, headless: D asks for the radio of a song and Skims it; K, Enter and Esc behave as
//! in any skim; the radio leaves out what is already had; one radio request per dig; the Settings
//! switches. The real shell on GPUI's test platform with the deterministic mock adapters, the
//! simulated library as the "service".

use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::library::source::TrackRow;
use apollo_app::library::LibrarySource;
use apollo_app::page_cache::{browse_id, keys, PageCache};
use apollo_app::player::{PlayerModel, DIG_NOTHING_NEW, DIG_FAILED, DIG_BUSY};
use apollo_app::search::SearchSource;
use apollo_app::settings::{DIG_KEY, DIG_LEAVE_OUT};
use apollo_app::stats::{PlayLog, PlayRecord};
use apollo_app::style::Fonts;
use apollo_app::ui::shell::{self, Shell};
use apollo_app::ui::track_menu::{self, Anchor, Command, Subject};
use apollo_app::url_thumbs::NoThumbs;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{point, px, AppContext, Entity, TestAppContext, VisualTestContext};

const SEED: &str = "digSeed0001";
const SEED_TITLE: &str = "Midnight Drive";
const TARGET: &str = "PLdigtarget";
/// Every song of the simulated radio is three minutes long (its rows say 120 to 360 s; the engine
/// is told this one).
const SONG_MS: u64 = 180_000;

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
    sim: SimLibrary,
    cache: PageCache,
    log: PlayLog,
    _dir: tempfile::TempDir,
}

fn song(id: &str, title: &str) -> Track {
    Track::new(id, title)
}

/// The ids the simulated radio answers with the first time it is asked.
fn first_radio() -> Vec<String> {
    SimLibrary::instant()
        .radio("whatever")
        .unwrap()
        .into_iter()
        .map(|r| r.track.video_id.as_str().to_owned())
        .collect()
}

fn setup(cx: &mut TestAppContext) -> (Page, &mut VisualTestContext) {
    let dir = tempfile::tempdir().unwrap();
    let log = PlayLog::new(Some(dir.path().join("apollo.db")));
    let cache = PageCache::in_memory();
    cache.set_account(Some("Dig Listener".into()));
    cx.update(|cx| {
        Fonts::install(cx);
        cx.set_reduce_motion(true);
        shell::bind_keys(cx);
        cache.clone().install(cx);
        log.clone().install(cx);
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
            LibraryTrack { track: song(SEED, SEED_TITLE) },
            LibraryTrack { track: song("digNext0001", "Next one") },
        ],
        art: std::sync::Arc::new(NoArt),
        search: std::sync::Arc::new(NoSearch),
        thumbs: std::sync::Arc::new(NoThumbs),
        library: std::sync::Arc::new(sim.clone()),
    };
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    (
        Page { player, shell, sim, cache, log, _dir: dir },
        vcx,
    )
}

fn press_d(vcx: &mut VisualTestContext) {
    vcx.simulate_keystrokes("d");
    vcx.run_until_parked();
}

fn pill(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.shell.read_with(vcx, |s, _| s.pill_text())
}

fn title(page: &Page, vcx: &mut VisualTestContext) -> String {
    page.player.read_with(vcx, |m, _| m.view().title.to_string())
}

fn skim_total(page: &Page, vcx: &mut VisualTestContext) -> Option<usize> {
    page.player
        .read_with(vcx, |m, _| m.view().skim.map(|s| s.total))
}

fn dig_seed(page: &Page, vcx: &mut VisualTestContext) -> Option<String> {
    page.player
        .read_with(vcx, |m, _| m.view().dig.map(|s| s.to_string()))
}

fn crate_ids(page: &Page, vcx: &mut VisualTestContext) -> Vec<String> {
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    model.read_with(vcx, |m, _| {
        m.items().iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    })
}

fn put_in_crate(page: &Page, vcx: &mut VisualTestContext, ids: &[String]) {
    let model = page.shell.read_with(vcx, |s, _| s.crate_model().clone());
    for id in ids {
        model.update(vcx, |m, cx| {
            m.add(song(id, id), None, cx);
        });
    }
}

fn row(id: &str) -> TrackRow {
    TrackRow {
        track: song(id, id),
        occurrence: None,
        thumbnail: None,
        explicit: false,
        liked: None,
    }
}

/// The engine opens the current load and lands on the requested start.
fn open_at(page: &Page, vcx: &mut VisualTestContext, position_ms: u64) {
    let load = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    page.player.update(vcx, |m, cx| {
        m.handle(Event::Resolved { load, result: Ok(LeaseId(1)) }, cx);
        m.handle(
            Event::Engine { load, event: EngineEvent::Opened { duration_ms: Some(SONG_MS) } },
            cx,
        );
        for gen in 1..=40 {
            m.handle(Event::Engine { load, event: EngineEvent::Seeked { gen, position_ms } }, cx);
        }
    });
}

fn setting(page: &Page, vcx: &mut VisualTestContext, on: bool, which: &'static apollo_app::settings::BoolSetting) {
    let settings = page.shell.read_with(vcx, |s, cx| s.settings(cx));
    settings.update(vcx, |m, cx| m.set(which, on, cx));
}

// ---- the dig ---------------------------------------------------------------------------------

#[gpui::test]
fn d_asks_for_the_radio_once_and_skims_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert_eq!(skim_total(&page, vcx), None);
    press_d(vcx);
    assert_eq!(page.sim.radio_seeds(), [SEED.to_owned()], "one radio request, for the playing song");
    assert_eq!(skim_total(&page, vcx), Some(10));
    assert_eq!(dig_seed(&page, vcx).as_deref(), Some(SEED_TITLE));
    assert!(title(&page, vcx).starts_with("Generated Track"), "the radio is what plays");
    assert!(vcx.debug_bounds("skim-pill").is_some(), "the Skim pill is the dig's pill");
    // Nothing else asks for a radio on its own, however long it runs.
    vcx.executor().advance_clock(Duration::from_secs(120));
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds().len(), 1);
}

#[gpui::test]
fn the_radio_leaves_out_the_crate_the_target_the_songs_list_and_the_play_log(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let ids = first_radio();
    assert_eq!(ids.len(), 10);
    put_in_crate(&page, vcx, &ids[0..1]);
    // The target playlist and the Songs list, as the page cache holds them.
    page.shell.update(vcx, |s, cx| {
        s.target_model().update(cx, |t, cx| t.set(TARGET, "Dig target", cx))
    });
    let ticket = page.cache.ticket();
    page.cache.put(
        &ticket,
        keys::collection(&BrowseId::new(browse_id(TARGET))),
        vec![row(&ids[1])],
    );
    page.cache.put(&ticket, keys::SONGS.to_owned(), vec![row(&ids[2])]);
    // One that was played here.
    // The shell chooses the simulated account when it starts; the log is kept per account.
    let account = page.cache.account().expect("an account is chosen");
    page.log.record(
        account,
        1,
        PlayRecord {
            track: song(&ids[3], "played"),
            art: None,
            played_unix: apollo_services_common::unix_now() as i64,
            listened_ms: 120_000,
        },
    );
    page.log.flush();
    press_d(vcx);
    assert_eq!(skim_total(&page, vcx), Some(6), "10 songs minus the 4 already had");
    // The first song skimmed is the first the radio gave that was not had.
    assert_eq!(title(&page, vcx), "Generated Track 5005");
}

#[gpui::test]
fn with_the_switch_off_nothing_is_left_out(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(page.shell.read_with(vcx, |s, cx| s.settings(cx).read(cx).get(&DIG_LEAVE_OUT)), "on by default");
    put_in_crate(&page, vcx, &first_radio());
    setting(&page, vcx, false, &DIG_LEAVE_OUT);
    press_d(vcx);
    assert_eq!(skim_total(&page, vcx), Some(10));
}

#[gpui::test]
fn when_everything_is_had_the_pill_says_so_and_nothing_changes(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    put_in_crate(&page, vcx, &first_radio());
    press_d(vcx);
    assert_eq!(pill(&page, vcx).as_deref(), Some(DIG_NOTHING_NEW));
    assert_eq!(skim_total(&page, vcx), None);
    assert_eq!(title(&page, vcx), SEED_TITLE, "the queue is as it was");
}

#[gpui::test]
fn a_radio_that_fails_says_so_and_the_next_dig_works(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.sim.set_radio_failure(Some(LoadFailure::Network));
    press_d(vcx);
    assert_eq!(pill(&page, vcx).as_deref(), Some(DIG_FAILED));
    assert_eq!(skim_total(&page, vcx), None);
    // A 429 from the service reads as busy.
    vcx.executor().advance_clock(Duration::from_secs(4));
    vcx.run_until_parked();
    page.sim.set_radio_failure(Some(LoadFailure::Throttled));
    press_d(vcx);
    assert_eq!(pill(&page, vcx).as_deref(), Some(DIG_BUSY));
    // And a failure does not leave D stuck.
    vcx.executor().advance_clock(Duration::from_secs(4));
    vcx.run_until_parked();
    page.sim.set_radio_failure(None);
    press_d(vcx);
    assert_eq!(skim_total(&page, vcx), Some(10));
}

// ---- keys while digging ----------------------------------------------------------------------

#[gpui::test]
fn k_keeps_into_the_crate_and_moves_on_and_enter_plays_for_real(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let ids = first_radio();
    press_d(vcx);
    open_at(&page, vcx, 60_000);
    vcx.simulate_keystrokes("k");
    assert_eq!(crate_ids(&page, vcx), vec![ids[0].clone()]);
    assert_eq!(skim_total(&page, vcx), Some(10), "still digging");
    assert_eq!(title(&page, vcx), "Generated Track 5002", "K moved on to the next song");
    open_at(&page, vcx, 60_000);
    vcx.simulate_keystrokes("enter");
    assert_eq!(skim_total(&page, vcx), None, "Enter ends the dig");
    assert_eq!(dig_seed(&page, vcx), None);
    assert_eq!(title(&page, vcx), "Generated Track 5002", "and plays that song");
    let intent = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().intent());
    assert_eq!(intent, Intent::Playing);
}

#[gpui::test]
fn esc_goes_back_to_the_queue_and_leaves_paused_music_paused(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    press_d(vcx);
    assert!(skim_total(&page, vcx).is_some());
    vcx.simulate_keystrokes("escape");
    assert_eq!(skim_total(&page, vcx), None);
    assert_eq!(dig_seed(&page, vcx), None);
    assert_eq!(title(&page, vcx), SEED_TITLE, "back to the song that was there");
    let intent = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().intent());
    assert_ne!(intent, Intent::Playing, "it was paused and stays paused");
}

#[gpui::test]
fn esc_resumes_music_that_was_playing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    page.player.update(vcx, |m, cx| m.handle(Event::Play, cx));
    press_d(vcx);
    vcx.simulate_keystrokes("escape");
    assert_eq!(title(&page, vcx), SEED_TITLE);
    let intent = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().intent());
    assert_eq!(intent, Intent::Playing, "as with any skim, music that was playing carries on");
}

#[gpui::test]
fn d_while_digging_digs_from_the_song_on_and_esc_still_goes_back_to_the_first_queue(
    cx: &mut TestAppContext,
) {
    let (page, vcx) = setup(cx);
    press_d(vcx);
    let skimmed = page
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().queue().current().unwrap().track.video_id.as_str().to_owned());
    press_d(vcx);
    assert_eq!(page.sim.radio_seeds(), [SEED.to_owned(), skimmed], "the second dig is seeded by the skimmed song");
    assert_eq!(dig_seed(&page, vcx).as_deref().map(|t| t.starts_with("Generated")), Some(true));
    vcx.simulate_keystrokes("escape");
    assert_eq!(title(&page, vcx), SEED_TITLE);
}

// ---- the switches and the menu ---------------------------------------------------------------

#[gpui::test]
fn with_the_dig_key_off_d_does_nothing(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    assert!(page.shell.read_with(vcx, |s, cx| s.settings(cx).read(cx).get(&DIG_KEY)), "on by default");
    setting(&page, vcx, false, &DIG_KEY);
    press_d(vcx);
    assert!(page.sim.radio_seeds().is_empty());
    assert_eq!(skim_total(&page, vcx), None);
}

fn open_menu(page: &Page, vcx: &mut VisualTestContext) -> Vec<(Command, &'static str)> {
    vcx.update(|window, cx| {
        track_menu::open(
            Subject::track(song(SEED, SEED_TITLE), None),
            Anchor::Point(point(px(300.0), px(300.0))),
            window,
            cx,
        )
    });
    vcx.run_until_parked();
    page.shell
        .read_with(vcx, |s, cx| s.menu().read(cx).labels(cx))
}

#[gpui::test]
fn the_song_menu_offers_dig_from_here_and_runs_it(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    let labels = open_menu(&page, vcx);
    assert!(labels.contains(&(Command::Dig, "Dig from here")), "{labels:?}");
    // It sits with Start radio, not apart.
    let at = |c| labels.iter().position(|(x, _)| *x == c).unwrap();
    assert_eq!(at(Command::Dig), at(Command::StartRadio) + 1);
    vcx.update(|window, cx| {
        track_menu::run_now(Command::Dig, Subject::track(song(SEED, SEED_TITLE), None), window, cx)
    });
    vcx.run_until_parked();
    assert_eq!(page.sim.radio_seeds(), [SEED.to_owned()]);
    assert_eq!(skim_total(&page, vcx), Some(10));
}

#[gpui::test]
fn with_skim_off_there_is_no_dig_anywhere(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    // The app copies Settings > Playback > Skim into the player; do the same.
    page.player.update(vcx, |m, cx| m.set_skim_enabled(false, cx));
    press_d(vcx);
    assert!(page.sim.radio_seeds().is_empty());
    let labels = open_menu(&page, vcx);
    assert!(!labels.iter().any(|(c, _)| *c == Command::Dig));
}

#[gpui::test]
fn the_key_with_the_dig_key_off_still_leaves_the_menu_item(cx: &mut TestAppContext) {
    let (page, vcx) = setup(cx);
    setting(&page, vcx, false, &DIG_KEY);
    let labels = open_menu(&page, vcx);
    assert!(labels.contains(&(Command::Dig, "Dig from here")));
}
