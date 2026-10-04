//! Autoplay: when the queue is about to run out, a radio seeded by the last track is queued
//! behind it (unless repeat-all is looping the queue or the switch is off), and the switch is
//! remembered. The player on the deterministic mocks, the simulated library as the radio.

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::{PlayerModel, QueueHeading};
use apollo_app::queue_source;
use apollo_app::search::SearchSource;
use apollo_app::settings::AutoplayStore;
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event};
use apollo_domain::queue::{Origin, Repeat};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use gpui::{AppContext, Entity, TestAppContext};

struct NoArt;
impl ArtSource for NoArt {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}
struct NoThumbs;
impl ThumbSource for NoThumbs {
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

fn id(n: usize) -> String {
    format!("track{n:06}")
}

fn tracks(n: usize) -> Vec<Track> {
    (0..n).map(|i| Track::new(id(i), format!("Song {i}"))).collect()
}

/// A player with one queued track and a library whose radio answers are recorded.
fn player(cx: &mut TestAppContext) -> (Entity<PlayerModel>, SimLibrary) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let library = SimLibrary::instant();
    let player = cx.new(|_| PlayerModel::new());
    let prepared = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![apollo_app::backend::LibraryTrack {
            track: Track::new(id(99), "Start"),
        }],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(library.clone()),
    };
    player.update(cx, |m, cx| m.install(prepared, cx));
    (player, library)
}

/// Origins of everything up next, in play order.
fn upcoming(player: &Entity<PlayerModel>, cx: &mut TestAppContext) -> Vec<(String, Origin)> {
    player.read_with(cx, |m, _| {
        m.coordinator()
            .unwrap()
            .queue()
            .upcoming()
            .map(|e| (e.track.video_id.as_str().to_owned(), e.origin))
            .collect()
    })
}

fn radio_count(player: &Entity<PlayerModel>, cx: &mut TestAppContext) -> usize {
    upcoming(player, cx)
        .iter()
        .filter(|(_, origin)| *origin == Origin::Radio)
        .count()
}

#[gpui::test]
fn a_radio_follows_a_queue_that_is_about_to_end(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    // Three songs: two are still up next, so nothing is asked for yet.
    player.update(cx, |m, cx| m.play_tracks(tracks(3), Some(0), Some(false), cx));
    cx.run_until_parked();
    assert!(library.radio_seeds().is_empty());

    // Two songs: one up next. The radio is seeded by the last song and queued behind it.
    player.update(cx, |m, cx| m.play_tracks(tracks(2), Some(0), Some(false), cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds(), [id(1)]);
    let up = upcoming(&player, cx);
    assert_eq!(up[0], (id(1), Origin::Context), "what was queued stays first");
    assert_eq!(up.len(), 1 + 10);
    assert!(up[1..].iter().all(|(_, origin)| *origin == Origin::Radio));
}

#[gpui::test]
fn past_the_source_the_queue_header_reads_autoplay_with_the_seed(cx: &mut TestAppContext) {
    let (player, _library) = player(cx);
    let heading = |player: &Entity<PlayerModel>, cx: &mut TestAppContext| {
        player.read_with(cx, |m, _| m.queue_view().heading)
    };
    player.update(cx, |m, cx| {
        m.play_tracks_from(
            tracks(2),
            Some(0),
            Some(false),
            Some(queue_source::named("Quick picks")),
            cx,
        )
    });
    cx.run_until_parked();
    // The radio is queued behind the source, which is still what plays.
    assert_eq!(radio_count(&player, cx), 10);
    assert!(matches!(heading(&player, cx), Some(QueueHeading::Source(_))));

    player.update(cx, |m, cx| m.next(cx)); // the source's last song
    assert!(matches!(heading(&player, cx), Some(QueueHeading::Source(_))));
    player.update(cx, |m, cx| m.next(cx)); // the first radio song
    assert_eq!(heading(&player, cx), Some(QueueHeading::Autoplay("Song 1".into())));

    // A new list is its own place again.
    player.update(cx, |m, cx| m.play_tracks(tracks(5), Some(0), Some(false), cx));
    assert_eq!(heading(&player, cx), Some(QueueHeading::Yours));
}

#[gpui::test]
fn a_single_track_is_seeded_by_itself_and_the_radio_keeps_coming(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    player.update(cx, |m, cx| m.play_tracks(tracks(1), Some(0), Some(false), cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds(), [id(0)]);
    assert_eq!(radio_count(&player, cx), 10);

    // Playing through the radio asks for more once two are left, seeded by the new last song.
    for _ in 0..8 {
        player.update(cx, |m, cx| m.next(cx));
    }
    cx.run_until_parked();
    assert_eq!(library.radio_seeds().len(), 1, "two left is still enough");
    player.update(cx, |m, cx| m.next(cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds().len(), 2, "{:?}", library.radio_seeds());
    assert_eq!(radio_count(&player, cx), 1 + 10);
}

#[gpui::test]
fn nothing_is_asked_for_while_paused_or_when_autoplay_is_off(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    // The restored/default queue is paused: no network at launch.
    cx.run_until_parked();
    assert!(library.radio_seeds().is_empty());

    player.update(cx, |m, cx| m.set_autoplay(false, cx));
    player.update(cx, |m, cx| m.play_tracks(tracks(1), Some(0), Some(false), cx));
    cx.run_until_parked();
    assert!(library.radio_seeds().is_empty(), "switched off");
    assert_eq!(radio_count(&player, cx), 0);

    // Switching it on while the last track plays queues the radio.
    player.update(cx, |m, cx| m.set_autoplay(true, cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds(), [id(0)]);
    assert_eq!(radio_count(&player, cx), 10);
}

#[gpui::test]
fn pressing_play_on_the_last_paused_track_starts_the_radio(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    cx.run_until_parked();
    assert!(library.radio_seeds().is_empty());
    player.update(cx, |m, cx| m.toggle_play(cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds(), [id(99)]);
    assert_eq!(radio_count(&player, cx), 10);
}

#[gpui::test]
fn repeat_all_wins_over_autoplay(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    player.update(cx, |m, cx| {
        m.cycle_repeat(cx);
        assert_eq!(m.view().repeat, Repeat::All);
        m.play_tracks(tracks(1), Some(0), Some(false), cx);
    });
    cx.run_until_parked();
    assert!(library.radio_seeds().is_empty());
    assert_eq!(radio_count(&player, cx), 0);
}

#[gpui::test]
fn tracks_already_in_the_queue_are_not_queued_again(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    // The simulated radio's first answer is simLib05000..05009; two of them are queued already.
    let mut list = vec![
        Track::new("simLib05003", "Known three"),
        Track::new("simLib05007", "Known seven"),
    ];
    list.truncate(2);
    player.update(cx, |m, cx| m.play_tracks(list, Some(0), Some(false), cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds(), ["simLib05007"]);
    assert_eq!(radio_count(&player, cx), 8);
    let ids: Vec<String> = upcoming(&player, cx).into_iter().map(|(id, _)| id).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(ids.len(), unique.len(), "no repeats up next");
    assert!(!ids.contains(&"simLib05003".to_owned()), "the current one is not repeated");
}

#[gpui::test]
fn a_failed_radio_is_retried_later_not_on_every_tick(cx: &mut TestAppContext) {
    let (player, library) = player(cx);
    library.set_radio_failure(Some(LoadFailure::Network));
    player.update(cx, |m, cx| m.play_tracks(tracks(1), Some(0), Some(false), cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds().len(), 1);
    assert_eq!(radio_count(&player, cx), 0, "the queue is left as it was");

    // Position ticks come all the time while playing: none of them asks again.
    let load = player.read_with(cx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    for ms in [1_000, 2_000, 3_000] {
        player.update(cx, |m, cx| {
            m.handle(
                Event::Engine {
                    load,
                    event: EngineEvent::Position(ms),
                },
                cx,
            )
        });
    }
    cx.run_until_parked();
    assert_eq!(library.radio_seeds().len(), 1);

    // After the retry delay it tries again by itself, and this time it works.
    library.set_radio_failure(None);
    cx.executor().advance_clock(Duration::from_secs(31));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds().len(), 2);
    assert_eq!(radio_count(&player, cx), 10);
}

#[derive(Clone)]
struct Remembered(Arc<Mutex<mpsc::Sender<bool>>>);
impl AutoplayStore for Remembered {
    fn load(&self) -> Option<bool> {
        None
    }
    fn save(&self, on: bool) {
        let _ = self.0.lock().unwrap().send(on);
    }
}

#[gpui::test]
fn the_switch_defaults_on_and_is_remembered(cx: &mut TestAppContext) {
    let (player, _library) = player(cx);
    assert!(player.read_with(cx, |m, _| m.view().autoplay), "on by default");

    let (tx, rx) = mpsc::channel();
    player.update(cx, |m, cx| {
        m.set_autoplay_store(Some(Arc::new(Remembered(Arc::new(Mutex::new(tx))))));
        m.set_autoplay(false, cx);
    });
    assert!(!player.read_with(cx, |m, _| m.view().autoplay));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(false));
    player.update(cx, |m, cx| m.set_autoplay(true, cx));
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true));

    // A saved "off" is applied before the queue is installed.
    let (fresh, _library) = player_off(cx);
    assert!(!fresh.read_with(cx, |m, _| m.view().autoplay));
}

fn player_off(cx: &mut TestAppContext) -> (Entity<PlayerModel>, SimLibrary) {
    let library = SimLibrary::instant();
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, _| m.restore_autoplay(Some(false)));
    (player, library)
}
