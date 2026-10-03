#![cfg(windows)]
//! Gapless playback against the real libmpv (`ao=null`, loopback fixtures): a track lined up
//! behind the one playing carries on by itself, with no end reported and no second open; the
//! gain of the new track is applied at the boundary; and everything that declines or takes the
//! track back falls through to the ordinary end of track.
//!
//! The fixture tone is 12 s long, so every test starts the first track near its end.

mod support;

use std::time::{Duration, Instant};

use apollo_domain::playback::{EngineEvent, LoadId};
use apollo_media::MediaConfig;
use apollo_runtime::MediaEngine;
use apollo_services_common::LoudnessTable;
use support::*;

const SECS: fn(u64) -> Duration = Duration::from_secs;
/// Close enough to the end of the 12 s fixture that the test does not wait long.
const NEAR_END_MS: u64 = 10_500;

fn opened(load: LoadId) -> impl Fn(LoadId, &EngineEvent) -> bool {
    move |l, e| l == load && matches!(e, EngineEvent::Opened { .. })
}

/// Opens the first track near its end and waits until it is playing.
fn play_first(r: &mut Rig) -> LoadId {
    let load = r.loads[0];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.open(load, lease, false, NEAR_END_MS);
    r.collect_until(SECS(10), opened(load)).expect("first track opens");
    load
}

fn gain(r: &Rig) -> f64 {
    let got = r.engine.property_values(&["volume-gain"]);
    got[0].1.parse::<f64>().unwrap_or_else(|_| panic!("volume-gain is not a number: {got:?}"))
}

fn settles_at(r: &Rig, want: f64) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < SECS(5) {
        if (gain(r) - want).abs() < 0.1 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn a_lined_up_track_carries_on_with_no_end_and_no_second_open() {
    let mut r = rig();
    let first = play_first(&mut r);
    let second = r.loads[1];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.preload(second, lease);

    let evs = r
        .collect_until(SECS(20), opened(second))
        .expect("the second track opens on its own");
    let first_events: Vec<_> = evs.iter().filter(|(l, _)| *l == first).collect();
    assert!(
        first_events.iter().all(|(_, e)| !matches!(e, EngineEvent::Eof | EngineEvent::Error(_))),
        "the first track must not report an end: {first_events:?}"
    );
    let second_events: Vec<_> = evs.iter().filter(|(l, _)| *l == second).map(|(_, e)| *e).collect();
    let transitioned = second_events
        .iter()
        .position(|e| matches!(e, EngineEvent::Transitioned))
        .expect("Transitioned for the new track");
    let opened_at = second_events
        .iter()
        .position(|e| matches!(e, EngineEvent::Opened { .. }))
        .expect("Opened for the new track");
    assert!(transitioned < opened_at, "Transitioned comes first: {second_events:?}");
    match second_events[opened_at] {
        EngineEvent::Opened { duration_ms } => {
            let d = duration_ms.expect("a duration") as i64;
            assert!((d - 12_000).abs() < 300, "duration {d}");
        }
        _ => unreachable!(),
    }

    // The new track plays on and its positions are reported under its own id.
    let later = r.drain_for(Duration::from_millis(1500));
    assert!(later.iter().any(|(l, e)| *l == second && matches!(e, EngineEvent::Position(_))));
    assert!(later.iter().all(|(l, _)| *l != first), "the first track spoke again: {later:?}");
}

#[test]
fn the_new_tracks_gain_is_applied_at_the_boundary() {
    let table = LoudnessTable::new();
    table.set(VID, 6.0);
    let mut r = rig_with(MediaConfig {
        audio_output: Some("null".into()),
        loudness: table.clone(),
        ..Default::default()
    });
    play_first(&mut r);
    assert!(settles_at(&r, -6.0));
    // Both tracks share the one video id the rig can lease, so change its loudness before the
    // boundary: the track that follows must look its own value up, not keep the old gain.
    table.set(VID, 2.0);
    let second = r.loads[1];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.preload(second, lease);
    r.collect_until(SECS(20), opened(second)).expect("the second track opens on its own");
    assert!(settles_at(&r, -2.0), "gain after the boundary: {}", gain(&r));
}

#[test]
fn with_gapless_off_the_track_just_ends() {
    let mut r = rig();
    r.engine.set_gapless(false);
    let first = play_first(&mut r);
    let second = r.loads[1];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.preload(second, lease);
    let evs = r
        .collect_until(SECS(20), |l, e| l == first && matches!(e, EngineEvent::Eof))
        .expect("the first track ends");
    assert!(evs.iter().all(|(l, _)| *l != second), "nothing of the second track: {evs:?}");
    // And nothing plays on by itself afterwards.
    let later = r.drain_for(Duration::from_secs(2));
    assert!(later.iter().all(|(l, _)| *l != second), "{later:?}");
}

#[test]
fn a_lined_up_track_taken_back_is_not_played() {
    let mut r = rig();
    let first = play_first(&mut r);
    let second = r.loads[1];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.preload(second, lease);
    r.engine.close(second);
    let evs = r
        .collect_until(SECS(20), |l, e| l == first && matches!(e, EngineEvent::Eof))
        .expect("the first track ends as usual");
    assert!(evs.iter().all(|(l, _)| *l != second), "{evs:?}");
    let later = r.drain_for(Duration::from_secs(2));
    assert!(later.iter().all(|(l, _)| *l != second), "{later:?}");
}

#[test]
fn a_stream_that_needs_other_headers_is_declined_and_opens_normally() {
    let mut r = rig();
    // The playing track's stream is opened with these headers ...
    let first = r.loads[0];
    let lease = r.lease_full(r.server.url("/tone_opus.opus"), vec![("Referer", "https://a.example/")], None);
    r.engine.open(first, lease, false, NEAR_END_MS);
    r.collect_until(SECS(10), opened(first)).expect("first track opens");
    // ... and the next one wants different ones: mpv has only one header list, so it is declined.
    let second = r.loads[1];
    let other = r.lease_full(r.server.url("/tone_opus.opus"), vec![("Referer", "https://b.example/")], None);
    r.engine.preload(second, other);
    let evs = r
        .collect_until(SECS(20), |l, e| l == first && matches!(e, EngineEvent::Eof))
        .expect("the first track ends as usual");
    assert!(evs.iter().all(|(l, _)| *l != second), "{evs:?}");
}

#[test]
fn skipping_to_another_track_replaces_the_lined_up_one() {
    let mut r = rig();
    let first = play_first(&mut r);
    let second = r.loads[1];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.preload(second, lease);
    // The user picks a different track: the coordinator cancels the lined-up load and opens another.
    let third = r.loads[2];
    r.engine.close(second);
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.open(third, lease, false, 0);
    let evs = r.collect_until(SECS(10), opened(third)).expect("the new track opens");
    assert!(evs.iter().all(|(l, e)| *l != second && !(*l == first && matches!(e, EngineEvent::Eof))), "{evs:?}");
    let later = r.drain_for(Duration::from_secs(1));
    assert!(later.iter().all(|(l, _)| *l != second), "{later:?}");
}
