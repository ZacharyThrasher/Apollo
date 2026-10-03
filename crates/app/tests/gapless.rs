//! Gapless playback in the player model: on by default, handed to the engine at install and on
//! every flip, and remembered between runs.

use std::sync::{mpsc, Arc, Mutex};

use apollo_app::art::{ArtSource, DecodedArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::settings::GaplessStore;
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::playback::{LeaseId, LoadId};
use apollo_provider::SearchResults;
use apollo_runtime::mock::{MockEngine, MockResolver};
use apollo_runtime::MediaEngine;
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

/// The mock engine, plus a record of what it was told about gapless playback.
struct Listening {
    inner: MockEngine,
    told: Arc<Mutex<Vec<bool>>>,
}

impl MediaEngine for Listening {
    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64) {
        self.inner.open(load, lease, start_paused, start_ms)
    }
    fn pause(&mut self, load: LoadId) {
        self.inner.pause(load)
    }
    fn resume(&mut self, load: LoadId) {
        self.inner.resume(load)
    }
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        self.inner.seek(load, generation, position_ms)
    }
    fn close(&mut self, load: LoadId) {
        self.inner.close(load)
    }
    fn stop(&mut self) {
        self.inner.stop()
    }
    fn set_gapless(&mut self, on: bool) {
        self.told.lock().unwrap().push(on);
    }
}

struct Remembered(Mutex<mpsc::Sender<bool>>);
impl GaplessStore for Remembered {
    fn load(&self) -> Option<bool> {
        None
    }
    fn save(&self, on: bool) {
        let _ = self.0.lock().unwrap().send(on);
    }
}

fn install(
    cx: &mut TestAppContext,
    setup: impl FnOnce(&mut PlayerModel),
) -> (Entity<PlayerModel>, Arc<Mutex<Vec<bool>>>) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let told = Arc::new(Mutex::new(Vec::new()));
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, _| setup(m));
    let prepared = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(MockResolver::default()),
            engine: Box::new(Listening {
                inner: MockEngine::default(),
                told: told.clone(),
            }),
        },
        queue: vec![LibraryTrack {
            track: Track::new("track000001", "Song"),
        }],
        art: Arc::new(NoArt),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(NoThumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    player.update(cx, |m, cx| m.install(prepared, cx));
    (player, told)
}

#[gpui::test]
fn it_is_on_by_default_and_the_engine_is_told_at_install(cx: &mut TestAppContext) {
    let (player, told) = install(cx, |_| {});
    assert!(player.read_with(cx, |m, _| m.gapless()));
    assert_eq!(*told.lock().unwrap(), [true]);
}

#[gpui::test]
fn a_saved_off_reaches_the_engine_at_install(cx: &mut TestAppContext) {
    let (player, told) = install(cx, |m| m.restore_gapless(Some(false)));
    assert!(!player.read_with(cx, |m, _| m.gapless()));
    assert_eq!(*told.lock().unwrap(), [false]);
}

#[gpui::test]
fn nothing_saved_leaves_it_on(cx: &mut TestAppContext) {
    let (player, _) = install(cx, |m| m.restore_gapless(None));
    assert!(player.read_with(cx, |m, _| m.gapless()));
}

#[gpui::test]
fn flipping_it_tells_the_engine_and_is_remembered(cx: &mut TestAppContext) {
    let (tx, saved) = mpsc::channel();
    let (player, told) = install(cx, |m| {
        m.set_gapless_store(Some(Arc::new(Remembered(Mutex::new(tx)))))
    });
    player.update(cx, |m, cx| m.set_gapless(false, cx));
    player.update(cx, |m, cx| m.set_gapless(true, cx));
    assert_eq!(*told.lock().unwrap(), [true, false, true]);
    let remembered: Vec<bool> = (0..2)
        .map(|_| saved.recv_timeout(std::time::Duration::from_secs(5)).unwrap())
        .collect();
    assert!(remembered.contains(&false) && remembered.contains(&true));
}

#[gpui::test]
fn flipping_before_the_backend_is_ready_is_kept_for_install(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, cx| m.set_gapless(false, cx));
    assert!(!player.read_with(cx, |m, _| m.gapless()));
}
