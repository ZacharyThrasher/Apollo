//! Retry after "Too many requests": it waits out YouTube's cooldown instead of asking again.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_app::art::{ArtSource, DecodedArt, SyntheticArt};
use apollo_app::backend::{LibraryTrack, Ports, Prepared};
use apollo_app::library::sim::SimLibrary;
use apollo_app::player::PlayerModel;
use apollo_app::search::SearchSource;
use apollo_app::url_thumbs::ThumbSource;
use apollo_domain::model::Track;
use apollo_domain::playback::{Event, Failure};
use apollo_services_common::Throttle;
use apollo_provider::SearchResults;
use apollo_runtime::mock::MockEngine;
use apollo_runtime::{ResolveRequest, Resolver};
use gpui::{AppContext, Entity, TestAppContext};

/// Records the loads the player started, in order.
struct Recorder(Arc<Mutex<Vec<ResolveRequest>>>);

impl Resolver for Recorder {
    fn start(&mut self, request: ResolveRequest) {
        self.0.lock().unwrap().push(request);
    }
    fn cancel(&mut self, _: apollo_domain::playback::LoadId) {}
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

fn player(cx: &mut TestAppContext, seen: &Arc<Mutex<Vec<ResolveRequest>>>) -> Entity<PlayerModel> {
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


fn fail_the_last_load(player: &Entity<PlayerModel>, log: &Arc<Mutex<Vec<ResolveRequest>>>, cx: &mut TestAppContext) {
    let load = log.lock().unwrap().last().expect("a load was started").load;
    player.update(cx, |m, cx| {
        m.handle(Event::Resolved { load, result: Err(Failure::Throttled) }, cx)
    });
    assert_eq!(player.read_with(cx, |m, _| m.view().error), Some(Failure::Throttled));
}

// One test: the throttle is shared by the whole process, so these steps must not interleave
// with another test's.
#[gpui::test]
fn retry_after_too_many_requests_waits_out_the_cooldown(cx: &mut TestAppContext) {
    let log = Arc::new(Mutex::new(Vec::new()));
    let player = player(cx, &log);
    player.update(cx, |m, cx| m.play_tracks(tracks(3), Some(0), Some(false), cx));
    fail_the_last_load(&player, &log, cx);

    // No cooldown running (the failure came from somewhere that does not set one): Retry asks
    // again at once, as it always did.
    let started = log.lock().unwrap().len();
    player.update(cx, |m, cx| m.retry(cx));
    assert_eq!(log.lock().unwrap().len(), started + 1, "retried at once");
    fail_the_last_load(&player, &log, cx);

    // YouTube said 429: Retry is held, however often it is pressed...
    Throttle::global().report_throttled(None);
    let started = log.lock().unwrap().len();
    player.update(cx, |m, cx| m.retry(cx));
    player.update(cx, |m, cx| m.retry(cx));
    cx.run_until_parked();
    assert_eq!(log.lock().unwrap().len(), started, "nothing asked during the cooldown");
    assert_eq!(player.read_with(cx, |m, _| m.view().error), Some(Failure::Throttled), "the bar still says why");
    cx.executor().advance_clock(Duration::from_secs(20));
    cx.run_until_parked();
    assert_eq!(log.lock().unwrap().len(), started, "still waiting");

    // ...and goes ahead once, by itself, when the cooldown has ended.
    cx.executor().advance_clock(Duration::from_secs(11));
    cx.run_until_parked();
    assert_eq!(log.lock().unwrap().len(), started + 1, "one retry, not one per press");
}
