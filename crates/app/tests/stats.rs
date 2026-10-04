//! Local stats, headless: a play that counts reaches the play log, the Stats tab shows the log
//! for the chosen period, and Home's personal shelves come from it and follow their switch. The
//! real models and views on GPUI's test platform, the simulated library, a play log in a
//! temporary SQLite file.

use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::home::HomeModel;
use apollo_app::library::model::{LibraryModel, Load, Tab};
use apollo_app::library::sim::SimLibrary;
use apollo_app::page_cache::PageCache;
use apollo_app::player::PlayerModel;
use apollo_app::settings::{SettingsModel, PERSONAL_SHELVES};
use apollo_app::stats::report::Period;
use apollo_app::stats::{PlayLog, PlayRecord};
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_domain::playback::{EngineEvent, Event, LeaseId};
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, Modifiers, TestAppContext, VisualTestContext};

const ACCOUNT: &str = "Demo Listener";
const DAY: i64 = 24 * 60 * 60;

struct TestArt;
impl ArtSource for TestArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(64)))
    }
}

fn prepared() -> Prepared {
    Prepared {
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
    }
}

struct Rig {
    log: PlayLog,
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    home: Entity<HomeModel>,
    model: Entity<LibraryModel>,
    settings: Entity<SettingsModel>,
    /// Keeps the log's file until the test ends.
    _dir: tempfile::TempDir,
}

/// A shell over the simulated account with the page cache and a play log installed, and the
/// simulated backend ready.
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
    let ready = prepared();
    let (search, thumbs) = (ready.search.clone(), ready.thumbs.clone());
    player.update(vcx, |m, cx| {
        m.restore_autoplay(Some(false));
        m.install(ready, cx)
    });
    shell.update(vcx, |s, cx| s.install_search(search, thumbs, cx));
    vcx.run_until_parked();
    let home = shell.read_with(vcx, |s, _| s.home().clone());
    let model = shell.read_with(vcx, |s, cx| s.library().read(cx).model().clone());
    let settings = shell.read_with(vcx, |s, cx| s.settings(cx));
    (
        Rig {
            log,
            player,
            shell,
            home,
            model,
            settings,
            _dir: dir,
        },
        vcx,
    )
}

fn track(video: &str, title: &str, artist: (&str, Option<&str>), album: (&str, Option<&str>)) -> Track {
    let mut t = Track::new(video, title);
    t.artists = vec![ArtistRef {
        name: artist.0.into(),
        id: artist.1.map(BrowseId::new),
    }];
    t.album = Some(AlbumRef {
        title: album.0.into(),
        id: album.1.map(BrowseId::new),
    });
    t.duration = Some(Duration::from_secs(200));
    t
}

/// Writes `plays` plays of `track`, the latest `days_ago` days ago, a day apart.
fn seed(rig: &Rig, track: &Track, plays: u32, days_ago: i64) {
    let now = apollo_services_common::unix_now() as i64;
    for i in 0..plays {
        rig.log.record(
            ACCOUNT.into(),
            u64::from(i),
            PlayRecord {
                track: track.clone(),
                art: Some(format!("https://example.com/{}.jpg", track.video_id.as_str())),
                played_unix: now - (days_ago + i64::from(i)) * DAY,
                listened_ms: 180_000,
            },
        );
    }
}

fn click(vcx: &mut VisualTestContext, id: &'static str) {
    let at = vcx
        .debug_bounds(id)
        .unwrap_or_else(|| panic!("{id} is on screen"))
        .center();
    vcx.simulate_click(at, Modifiers::none());
    vcx.run_until_parked();
}

/// Opens the Library on its Stats tab and lets the report load.
fn open_stats(rig: &Rig, vcx: &mut VisualTestContext) {
    rig.log.flush();
    rig.shell.update(vcx, |s, cx| s.show_route(Route::Library, cx));
    rig.model.update(vcx, |m, cx| m.set_tab(Tab::Stats, cx));
    vcx.run_until_parked();
}

// ---- recording ------------------------------------------------------------------------------------

/// Press play and let the engine report audio, then keep reporting a position a moment later.
fn listen(rig: &Rig, vcx: &mut VisualTestContext) {
    rig.player.update(vcx, |m, cx| {
        m.count_plays_after(100);
        m.toggle_play(cx)
    });
    let load = rig
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    for event in [
        Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        },
        Event::Engine {
            load,
            event: EngineEvent::Opened {
                duration_ms: Some(200_000),
            },
        },
        Event::Engine {
            load,
            event: EngineEvent::Position(1_000),
        },
    ] {
        rig.player.update(vcx, |m, cx| m.handle(event, cx));
    }
    // The player hands over what it is doing at most once a second.
    std::thread::sleep(Duration::from_millis(1_150));
    rig.player.update(vcx, |m, cx| {
        m.handle(
            Event::Engine {
                load,
                event: EngineEvent::Position(2_200),
            },
            cx,
        )
    });
}

#[gpui::test]
fn a_play_that_counts_is_written_for_the_chosen_account(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    listen(&rig, vcx);
    rig.log.flush();
    let totals = rig.log.totals(ACCOUNT, None);
    assert_eq!(totals.len(), 1, "{totals:?}");
    assert_eq!(totals[0].video_id, "startTrack01");
    assert_eq!(totals[0].plays, 1);
    assert!(rig.log.totals("someone else", None).is_empty());
}

#[gpui::test]
fn nothing_is_written_while_no_account_is_chosen(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    // Signed out by choice: the page cache serves nobody.
    vcx.update(|_, cx| PageCache::try_global(cx).unwrap().sign_out());
    listen(&rig, vcx);
    rig.log.flush();
    assert!(rig.log.totals(ACCOUNT, None).is_empty());
}

#[gpui::test]
fn a_skipped_track_is_not_a_play(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    // Thirty seconds is the real rule; a quick skip is far short of it.
    rig.player.update(vcx, |m, cx| m.toggle_play(cx));
    let load = rig
        .player
        .read_with(vcx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    for event in [
        Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        },
        Event::Engine {
            load,
            event: EngineEvent::Position(500),
        },
    ] {
        rig.player.update(vcx, |m, cx| m.handle(event, cx));
    }
    rig.log.flush();
    assert!(rig.log.totals(ACCOUNT, None).is_empty());
}

// ---- the Stats tab --------------------------------------------------------------------------------

fn neon() -> (&'static str, Option<&'static str>) {
    ("Neon Arcade", Some("UCsim_Neon Arcade"))
}

#[gpui::test]
fn the_stats_tab_is_the_last_tab_and_says_so_when_nothing_was_played(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    assert_eq!(Tab::ALL.last(), Some(&Tab::Stats));
    assert_eq!(Tab::Stats.label(), "Stats");
    open_stats(&rig, vcx);
    assert!(vcx.debug_bounds("library-tab-stats").is_some());
    assert!(
        vcx.debug_bounds("library-stats-empty").is_some(),
        "the one-line empty state"
    );
    assert!(vcx.debug_bounds("stats-card-time").is_none());
}

#[gpui::test]
fn the_report_counts_the_period_and_the_period_can_be_changed(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let a = track("aaaaaaaaaaa", "Alpha", neon(), ("First Light", Some("MPREb_sim_First")));
    let b = track("bbbbbbbbbbb", "Beta", ("Other", None), ("Second", None));
    let old = track("ooooooooooo", "Old Favourite", ("Other", None), ("Second", None));
    seed(&rig, &a, 3, 1); // three plays in the last 4 weeks
    seed(&rig, &b, 1, 2);
    seed(&rig, &old, 2, 100); // outside 4 weeks, inside 6 months
    open_stats(&rig, vcx);

    let (period, plays, tracks, top_artist) = rig.model.read_with(vcx, |m, _| match m.stats() {
        Load::Ready(r) => (
            m.stats_period(),
            r.plays,
            r.tracks.iter().map(|t| t.track.title.clone()).collect::<Vec<_>>(),
            r.top_artist().map(|a| a.name.clone()),
        ),
        other => panic!("{other:?}"),
    });
    assert_eq!(period, Period::Weeks4, "the default period");
    assert_eq!(plays, 4);
    assert_eq!(tracks, ["Alpha", "Beta"]);
    assert_eq!(top_artist.as_deref(), Some("Neon Arcade"));
    for id in ["stats-card-time", "stats-card-plays", "stats-card-artist", "library-row-0"] {
        assert!(vcx.debug_bounds(id).is_some(), "{id} is on screen");
    }

    // The period popover: Last 4 weeks, Last 6 months, All time.
    click(vcx, "library-sort");
    click(vcx, "library-sort-item-1");
    let (period, plays) = rig.model.read_with(vcx, |m, _| match m.stats() {
        Load::Ready(r) => (m.stats_period(), r.plays),
        other => panic!("{other:?}"),
    });
    assert_eq!((period, plays), (Period::Months6, 6));

    // Everything that was ever played: add one from a year ago.
    let ancient = track("xxxxxxxxxxx", "Ancient", ("Other", None), ("Second", None));
    seed(&rig, &ancient, 1, 400);
    rig.log.flush();
    rig.model
        .update(vcx, |m, cx| m.set_stats_period(Period::AllTime, cx));
    vcx.run_until_parked();
    let plays = rig.model.read_with(vcx, |m, _| match m.stats() {
        Load::Ready(r) => r.plays,
        other => panic!("{other:?}"),
    });
    assert_eq!(plays, 7);
}

#[gpui::test]
fn clicking_a_top_track_plays_the_list_from_it_and_an_artist_or_album_opens(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let a = track("aaaaaaaaaaa", "Alpha", neon(), ("First Light", Some("MPREb_sim_First")));
    let b = track("bbbbbbbbbbb", "Beta", ("Other", None), ("Second", None));
    seed(&rig, &a, 3, 1);
    seed(&rig, &b, 2, 1);
    open_stats(&rig, vcx);

    // Rows: 0-1 tracks, 2-3 artists (Neon Arcade has an id, Other has none), 4-5 albums.
    click(vcx, "library-row-1");
    let current = rig
        .player
        .read_with(vcx, |m, _| m.view().title.to_string());
    assert_eq!(current, "Beta");

    // An artist without an id has no page to open: nothing happens.
    click(vcx, "library-row-3");
    assert_eq!(rig.model.read_with(vcx, |m, _| m.depth()), 0);

    click(vcx, "library-row-2");
    assert!(
        rig.model.read_with(vcx, |m, _| m.artist().is_some()),
        "Neon Arcade opens its page"
    );
}

#[gpui::test]
fn another_account_does_not_see_the_plays(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let a = track("aaaaaaaaaaa", "Alpha", neon(), ("First Light", None));
    seed(&rig, &a, 2, 1);
    vcx.update(|_, cx| PageCache::try_global(cx).unwrap().set_account(Some("Somebody Else".into())));
    open_stats(&rig, vcx);
    assert!(vcx.debug_bounds("library-stats-empty").is_some());
}

// ---- Home's personal shelves ----------------------------------------------------------------------

fn titles(rig: &Rig, vcx: &mut VisualTestContext) -> Vec<String> {
    rig.home.read_with(vcx, |m, _| {
        m.display_shelves().iter().map(|s| s.title.clone()).collect()
    })
}

fn six_on_repeat(rig: &Rig) {
    for i in 0..6 {
        let t = track(
            &format!("repeat{i:05}"),
            &format!("Repeat {i}"),
            ("Neon Arcade", None),
            ("Loop", None),
        );
        seed(rig, &t, 2, 1);
    }
    rig.log.flush();
}

#[gpui::test]
fn on_repeat_sits_right_after_the_first_shelf_once_it_has_six_tracks(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    let plain = titles(&rig, vcx);
    assert!(plain.len() >= 2, "the simulated feed has shelves: {plain:?}");
    assert!(!plain.contains(&"On repeat".to_string()));

    six_on_repeat(&rig);
    rig.home.update(vcx, |h, cx| h.show(Instant::now(), cx));
    vcx.run_until_parked();
    let shown = titles(&rig, vcx);
    assert_eq!(shown[0], plain[0], "the first feed shelf stays first");
    assert_eq!(shown[1], "On repeat");
    assert_eq!(shown[2], plain[1]);
    assert_eq!(shown.len(), plain.len() + 1);
    // Forgotten favourites needs 60 quiet days: not yet.
    assert!(!shown.contains(&"Forgotten favourites".to_string()));
}

#[gpui::test]
fn five_tracks_are_not_enough_for_a_shelf(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    for i in 0..5 {
        let t = track(&format!("few{i:08}"), &format!("Few {i}"), ("A", None), ("B", None));
        seed(&rig, &t, 3, 1);
    }
    rig.log.flush();
    rig.home.update(vcx, |h, cx| h.show(Instant::now(), cx));
    vcx.run_until_parked();
    assert!(!titles(&rig, vcx).contains(&"On repeat".to_string()));
}

#[gpui::test]
fn forgotten_favourites_are_well_played_tracks_unplayed_for_sixty_days(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    for i in 0..6 {
        let t = track(&format!("old{i:08}"), &format!("Old {i}"), ("A", None), ("B", None));
        // Five plays, the latest 90 days ago.
        seed(&rig, &t, 5, 90);
    }
    rig.log.flush();
    rig.home.update(vcx, |h, cx| h.show(Instant::now(), cx));
    vcx.run_until_parked();
    let shown = titles(&rig, vcx);
    assert_eq!(shown[1], "Forgotten favourites", "{shown:?}");
    assert!(!shown.contains(&"On repeat".to_string()), "nothing was played lately");
}

#[gpui::test]
fn the_personal_shelves_switch_hides_them_and_brings_them_back(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    six_on_repeat(&rig);
    rig.home.update(vcx, |h, cx| h.show(Instant::now(), cx));
    vcx.run_until_parked();
    assert!(titles(&rig, vcx).contains(&"On repeat".to_string()));

    assert!(rig.settings.read_with(vcx, |m, _| m.get(&PERSONAL_SHELVES)), "on by default");
    rig.settings
        .update(vcx, |m, cx| m.set(&PERSONAL_SHELVES, false, cx));
    vcx.run_until_parked();
    assert!(!titles(&rig, vcx).contains(&"On repeat".to_string()));

    rig.settings
        .update(vcx, |m, cx| m.set(&PERSONAL_SHELVES, true, cx));
    vcx.run_until_parked();
    assert!(titles(&rig, vcx).contains(&"On repeat".to_string()));
}

#[gpui::test]
fn a_personal_shelf_song_plays_from_the_shelf(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    six_on_repeat(&rig);
    rig.home.update(vcx, |h, cx| h.show(Instant::now(), cx));
    vcx.run_until_parked();
    let shelf = rig.home.read_with(vcx, |m, _| m.personal_shelves()[0].clone());
    assert_eq!(shelf.title, "On repeat");
    assert!(shelf.is_songs());
    assert_eq!(shelf.items.len(), 6);
}

#[gpui::test]
fn another_accounts_plays_do_not_make_shelves(cx: &mut TestAppContext) {
    let (rig, vcx) = rig(cx);
    six_on_repeat(&rig);
    vcx.update(|_, cx| PageCache::try_global(cx).unwrap().set_account(Some("Somebody Else".into())));
    rig.home.update(vcx, |h, cx| h.show(Instant::now(), cx));
    vcx.run_until_parked();
    assert!(!titles(&rig, vcx).contains(&"On repeat".to_string()));
}
