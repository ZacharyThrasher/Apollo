//! The queue comes back after a restart: the current track, what is up next, shuffle and
//! repeat, paused at the position it was left at. Nothing plays by itself, and the head is
//! resolved ahead so the first Play is instant. The player on mock adapters, the real SQLite
//! store in a temporary directory.

use std::sync::{Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::{PlayerModel, QueueHeading};
use apollo_app::queue_source;
use apollo_app::search::SearchSource;
use apollo_app::settings::{QueueStore, SqliteSettings};
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Intent, LeaseId, LoadId, Phase};
use apollo_domain::queue::Repeat;
use apollo_provider::SearchResults;
use apollo_runtime::mock::MockEngine;
use apollo_runtime::{ResolveRequest, Resolver};
use gpui::{AppContext, Entity, TestAppContext};

/// What the resolver was asked: `prefetch:<id>` for a warm-up, `start:<id>@<resume ms>` for a load.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Recorder(Log);

impl Resolver for Recorder {
    fn start(&mut self, request: ResolveRequest) {
        let line = format!(
            "start:{}@{}",
            request.track.video_id.as_str(),
            request.resume_ms
        );
        self.0 .0.lock().unwrap().push(line);
    }
    fn cancel(&mut self, _: LoadId) {}
    fn prefetch(&mut self, track: &Track) {
        self.0
             .0
            .lock()
            .unwrap()
            .push(format!("prefetch:{}", track.video_id.as_str()));
    }
}

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

fn prepared(log: &Log, default: &[&str]) -> Prepared {
    Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(Recorder(log.clone())),
            engine: Box::new(MockEngine::default()),
        },
        queue: default
            .iter()
            .map(|id| LibraryTrack {
                track: Track::new(*id, *id),
            })
            .collect(),
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    }
}

/// A player that saves under `key` into `store`, started on the default queue `default`.
fn launch(
    cx: &mut TestAppContext,
    store: &Arc<SqliteSettings>,
    key: Option<&str>,
    default: &[&str],
) -> (Entity<PlayerModel>, Log) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let log = Log::default();
    let player = cx.new(|_| PlayerModel::new());
    // What start-up does: read the saved queue, then install with it.
    let saved = key.and_then(|key| QueueStore::load(store.as_ref(), key));
    player.update(cx, |m, cx| {
        let q: Arc<dyn QueueStore> = store.clone();
        m.set_queue_store(Some(q), key.map(str::to_owned));
        m.restore_autoplay(Some(false)); // keep the radio out of these tests
        m.install_with(prepared(&log, default), saved, cx);
    });
    (player, log)
}

fn store(dir: &tempfile::TempDir) -> Arc<SqliteSettings> {
    Arc::new(SqliteSettings::at(dir.path().join("apollo.db")))
}

/// Resolve and open the active load, then report the engine at `position_ms`.
fn play_to(player: &Entity<PlayerModel>, cx: &mut TestAppContext, position_ms: u64) {
    let load = player.read_with(cx, |m, _| m.coordinator().unwrap().active_load().unwrap());
    player.update(cx, |m, cx| {
        m.handle(Event::Resolved { load, result: Ok(LeaseId(1)) }, cx);
        m.handle(
            Event::Engine { load, event: EngineEvent::Opened { duration_ms: Some(240_000) } },
            cx,
        );
        m.handle(Event::Engine { load, event: EngineEvent::Position(position_ms) }, cx);
    });
}

fn current_and_upcoming(player: &Entity<PlayerModel>, cx: &mut TestAppContext) -> (String, Vec<String>) {
    player.read_with(cx, |m, _| {
        let q = m.coordinator().unwrap().queue();
        (
            q.current().unwrap().track.video_id.as_str().to_owned(),
            q.upcoming().map(|e| e.track.video_id.as_str().to_owned()).collect(),
        )
    })
}

#[gpui::test]
fn the_queue_comes_back_paused_at_the_saved_position(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let before = {
        let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
        player.update(cx, |m, cx| {
            m.play_tracks(tracks(6), Some(1), Some(false), cx)
        });
        player.update(cx, |m, cx| m.next(cx));
        player.update(cx, |m, cx| m.cycle_repeat(cx)); // repeat all
        play_to(&player, cx, 95_000);
        player.update(cx, |m, cx| m.toggle_play(cx)); // pause: the position is saved at once
        let before = current_and_upcoming(&player, cx);
        assert_eq!(before.0, id(2));
        // Quit: the last write lands before the process goes.
        player.update(cx, |m, _| m.shutdown());
        before
    };

    let (player, log) = launch(cx, &store, Some("acct"), &["default0001"]);
    assert_eq!(current_and_upcoming(&player, cx), before, "same track, same up-next order");
    player.read_with(cx, |m, _| {
        let c = m.coordinator().unwrap();
        assert_eq!((c.intent(), c.phase()), (Intent::Paused, Phase::Idle), "never autoplay");
        assert_eq!(c.queue().repeat(), Repeat::All);
        assert_eq!(c.queue().history().len(), 2, "history is back too");
        assert_eq!(m.view().position_ms, 95_000);
    });
    assert_eq!(log.lines(), [format!("prefetch:{}", id(2))], "the restored head is warmed, nothing else");

    // Pressing Play resolves a fresh stream and resumes where it was left.
    player.update(cx, |m, cx| m.toggle_play(cx));
    assert_eq!(log.lines().last().unwrap(), &format!("start:{}@95000", id(2)));
}

#[gpui::test]
fn where_the_queue_came_from_comes_back_with_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    {
        let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
        player.update(cx, |m, cx| {
            m.play_tracks_from(
                tracks(4),
                Some(0),
                Some(false),
                Some(queue_source::named("Quick picks")),
                cx,
            )
        });
        // Quit: the last write lands before the process goes.
        player.update(cx, |m, _| m.shutdown());
    }
    let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
    let heading = player.read_with(cx, |m, _| m.queue_view().heading);
    match heading {
        Some(QueueHeading::Source(source)) => assert_eq!(source.label(), "Quick picks"),
        other => panic!("expected the saved source, got {other:?}"),
    }
}

#[gpui::test]
fn without_a_saved_queue_the_default_one_is_used_and_saved_once_it_is_played(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    {
        let (player, _log) = launch(cx, &store, Some("acct"), &["default0001", "default0002"]);
        assert_eq!(current_and_upcoming(&player, cx).0, "default0001");
        player.update(cx, |m, _| m.shutdown());
    }
    assert!(
        QueueStore::load(store.as_ref(), "acct").is_none(),
        "an untouched default queue is not saved (it would hide new likes)"
    );

    {
        let (player, _log) = launch(cx, &store, Some("acct"), &["default0001", "default0002"]);
        player.update(cx, |m, cx| m.toggle_play(cx));
        play_to(&player, cx, 12_000);
        player.update(cx, |m, _| m.shutdown());
    }
    let (player, _log) = launch(cx, &store, Some("acct"), &["other000001"]);
    assert_eq!(current_and_upcoming(&player, cx), ("default0001".into(), vec!["default0002".into()]));
}

#[gpui::test]
fn another_accounts_queue_is_never_restored_and_signed_out_saves_nothing(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    {
        let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
        player.update(cx, |m, cx| m.play_tracks(tracks(3), Some(0), Some(false), cx));
        player.update(cx, |m, _| m.shutdown());
    }
    // Someone else signs in on this machine: they get their own default queue.
    {
        let (player, _log) = launch(cx, &store, Some("someone-else"), &["theirs000001"]);
        assert_eq!(current_and_upcoming(&player, cx).0, "theirs000001");
        player.update(cx, |m, cx| m.toggle_play(cx));
        player.update(cx, |m, _| m.shutdown());
    }
    // (Their start-up read already purged the first account's queue: a queue is only ever
    // replayed for the account that made it.)

    // Nobody signed in: nothing is restored and nothing is written over theirs.
    let (player, _log) = launch(cx, &store, None, &["default0001"]);
    assert_eq!(current_and_upcoming(&player, cx).0, "default0001");
    player.update(cx, |m, cx| m.play_tracks(tracks(3), Some(0), Some(false), cx));
    player.update(cx, |m, _| m.shutdown());
    let (queue, _) = QueueStore::load(store.as_ref(), "someone-else").expect("still theirs");
    assert_eq!(queue.current().unwrap().track.video_id.as_str(), "theirs000001");
}

#[gpui::test]
fn signing_out_deletes_the_saved_queue(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
    player.update(cx, |m, cx| m.play_tracks(tracks(3), Some(0), Some(false), cx));
    player.update(cx, |m, _| m.shutdown());
    assert!(QueueStore::load(store.as_ref(), "acct").is_some());

    let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
    player.update(cx, |m, cx| {
        m.forget_saved_queue();
        m.play_tracks(tracks(2), Some(0), Some(false), cx);
        m.shutdown();
    });
    assert!(QueueStore::load(store.as_ref(), "acct").is_none(), "gone, and not saved again");
}

#[gpui::test]
fn a_restored_queue_waits_for_play_before_asking_for_a_radio(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    {
        let (player, _log) = launch(cx, &store, Some("acct"), &["default0001"]);
        player.update(cx, |m, cx| m.play_tracks(tracks(1), Some(0), Some(false), cx));
        player.update(cx, |m, _| m.shutdown());
    }
    let library = SimLibrary::instant();
    cx.update(|cx| cx.set_reduce_motion(true));
    let log = Log::default();
    let player = cx.new(|_| PlayerModel::new());
    let saved = QueueStore::load(store.as_ref(), "acct");
    player.update(cx, |m, cx| {
        let mut ready = prepared(&log, &["default0001"]);
        ready.library = Arc::new(library.clone());
        m.install_with(ready, saved, cx); // autoplay is on by default
    });
    cx.run_until_parked();
    assert!(library.radio_seeds().is_empty(), "launching asks the network for nothing");
    player.update(cx, |m, cx| m.toggle_play(cx));
    cx.run_until_parked();
    assert_eq!(library.radio_seeds(), [id(0)]);
}
