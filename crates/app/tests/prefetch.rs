//! Resolving ahead of the click: what the player asks its resolver to prepare, and when.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::model::Track;
use apollo_provider::SearchResults;
use apollo_runtime::mock::MockEngine;
use apollo_runtime::{ResolveRequest, Resolver};
use gpui::{AppContext, Entity, TestAppContext};

/// Records which tracks were asked to be prefetched, in order.
struct Recorder(Arc<Mutex<Vec<String>>>);

impl Resolver for Recorder {
    fn start(&mut self, _: ResolveRequest) {}
    fn cancel(&mut self, _: apollo_domain::playback::LoadId) {}
    fn prefetch(&mut self, track: &Track) {
        self.0.lock().unwrap().push(track.video_id.as_str().to_owned());
    }
}

struct Art;
impl ArtSource for Art {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        Some(DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(32)))
    }
}
struct Thumbs;
impl ThumbSource for Thumbs {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}
struct NoSearch;
impl SearchSource for NoSearch {
    fn search(&self, _: &str) -> Result<SearchResults, apollo_domain::listing::LoadFailure> {
        Ok(SearchResults::default())
    }
}

fn id(n: usize) -> String {
    format!("track{n:06}")
}

fn tracks(n: usize) -> Vec<Track> {
    (0..n).map(|i| Track::new(id(i), format!("Song {i}"))).collect()
}

fn player(cx: &mut TestAppContext, seen: &Arc<Mutex<Vec<String>>>) -> Entity<PlayerModel> {
    cx.update(|cx| cx.set_reduce_motion(true));
    let player = cx.new(|_| PlayerModel::new());
    let prepared = Prepared {
        label: "test",
        ports: Ports {
            resolver: Box::new(Recorder(seen.clone())),
            engine: Box::new(MockEngine::default()),
        },
        queue: vec![LibraryTrack { track: Track::new(id(99), "Start") }],
        art: Arc::new(Art),
        search: Arc::new(NoSearch),
        thumbs: Arc::new(Thumbs),
        library: Arc::new(SimLibrary::instant()),
    };
    player.update(cx, |m, cx| m.install(prepared, cx));
    player
}

fn seen(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    seen.lock().unwrap().clone()
}

#[gpui::test]
fn the_restored_queue_head_is_ready_before_the_first_play(cx: &mut TestAppContext) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let _player = player(cx, &log);
    assert_eq!(seen(&log), vec![id(99)]);
}

#[gpui::test]
fn starting_a_list_prepares_the_next_two_in_play_order(cx: &mut TestAppContext) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let player = player(cx, &log);
    log.lock().unwrap().clear();
    player.update(cx, |m, cx| m.play_tracks(tracks(6), Some(1), Some(false), cx));
    assert_eq!(seen(&log), vec![id(2), id(3)]);
}

#[gpui::test]
fn a_track_played_now_prepares_what_follows_it(cx: &mut TestAppContext) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let player = player(cx, &log);
    player.update(cx, |m, cx| m.play_tracks(tracks(4), Some(0), Some(false), cx));
    log.lock().unwrap().clear();
    player.update(cx, |m, cx| m.play_now(Track::new(id(50), "Picked"), cx));
    let got = seen(&log);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(!got.contains(&id(50)), "the picked track itself is being resolved for real");
}

#[gpui::test]
fn lingering_prepares_a_track_but_sweeping_past_does_not(cx: &mut TestAppContext) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let player = player(cx, &log);
    log.lock().unwrap().clear();
    let all = tracks(5);

    // The pointer sweeps across three rows faster than the rest delay: nothing.
    for t in &all[..3] {
        player.update(cx, |m, cx| m.hint_play(t, cx));
        cx.executor().advance_clock(Duration::from_millis(80));
    }
    // ...and comes to rest on the last one.
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(seen(&log), vec![id(2)], "only the row it rested on");

    // Hovering the same row again does not ask twice.
    player.update(cx, |m, cx| m.hint_play(&all[2], cx));
    cx.executor().advance_clock(Duration::from_millis(400));
    cx.run_until_parked();
    assert_eq!(seen(&log), vec![id(2)]);
}
