#![cfg(windows)]
//! Volume normalization against the real libmpv (`ao=null`, loopback fixtures): the gain a track
//! gets follows its loudness, the switch, and a loudness that arrives late; the user's volume is
//! never touched.

mod support;

use std::time::{Duration, Instant};

use apollo_domain::playback::EngineEvent;
use apollo_media::MediaConfig;
use apollo_runtime::MediaEngine;
use apollo_services_common::LoudnessTable;
use support::*;

const SECS: fn(u64) -> Duration = Duration::from_secs;

fn rig_with_table(table: &LoudnessTable) -> Rig {
    rig_with(MediaConfig {
        audio_output: Some("null".into()),
        loudness: table.clone(),
        ..Default::default()
    })
}

fn gain(r: &Rig) -> f64 {
    let got = r.engine.property_values(&["volume-gain"]);
    let (_, v) = got.first().expect("one answer");
    v.parse::<f64>()
        .unwrap_or_else(|_| panic!("volume-gain is not a number: {v}"))
}

/// Polls until the gain is within 0.1 dB of `want` (a glide takes a moment).
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

fn play(r: &mut Rig) {
    let load = r.loads[0];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.open(load, lease, false, 0);
    r.collect_until(SECS(10), |l, e| {
        l == load && matches!(e, EngineEvent::Opened { .. })
    })
    .expect("track opens");
}

#[test]
fn a_loud_track_is_turned_down_by_its_excess_before_it_plays() {
    let table = LoudnessTable::new();
    table.set(VID, 5.0);
    let mut r = rig_with_table(&table);
    play(&mut r);
    assert!((gain(&r) + 5.0).abs() < 0.01, "gain {}", gain(&r));
}

#[test]
fn a_quiet_track_is_not_boosted() {
    let table = LoudnessTable::new();
    table.set(VID, -6.0);
    let mut r = rig_with_table(&table);
    play(&mut r);
    assert!(gain(&r).abs() < 0.01, "gain {}", gain(&r));
}

#[test]
fn an_unknown_loudness_plays_at_unity() {
    let mut r = rig_with_table(&LoudnessTable::new());
    play(&mut r);
    assert!(gain(&r).abs() < 0.01, "gain {}", gain(&r));
}

#[test]
fn the_switch_applies_to_the_track_that_is_playing() {
    let table = LoudnessTable::new();
    table.set(VID, 4.0);
    let mut r = rig_with_table(&table);
    play(&mut r);
    assert!(settles_at(&r, -4.0));
    r.engine.set_normalization(false);
    assert!(settles_at(&r, 0.0), "off: {}", gain(&r));
    r.engine.set_normalization(true);
    assert!(settles_at(&r, -4.0), "on: {}", gain(&r));
}

#[test]
fn a_loudness_that_arrives_after_the_track_started_is_applied() {
    let table = LoudnessTable::new();
    let mut r = rig_with_table(&table);
    play(&mut r);
    assert!(gain(&r).abs() < 0.01);
    table.set(VID, 3.0);
    assert!(settles_at(&r, -3.0), "late: {}", gain(&r));
}

#[test]
fn the_next_track_gets_its_own_gain_and_the_users_volume_is_untouched() {
    let table = LoudnessTable::new();
    table.set(VID, 6.0);
    let mut r = rig_with_table(&table);
    r.engine.set_volume(40);
    play(&mut r);
    assert!((gain(&r) + 6.0).abs() < 0.01);

    // The same video id is the only one the rig can lease, so change its loudness between
    // loads: the new load looks the table up afresh instead of carrying the old gain over.
    table.set(VID, 2.0);
    r.engine.stop();
    let load = r.loads[1];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.open(load, lease, false, 0);
    r.collect_until(SECS(10), |l, e| {
        l == load && matches!(e, EngineEvent::Opened { .. })
    })
    .expect("second track opens");
    assert!((gain(&r) + 2.0).abs() < 0.01, "gain {}", gain(&r));

    let volume = r.engine.property_values(&["volume"]);
    assert_eq!(volume[0].1.parse::<f64>().unwrap().round() as i64, 40);
}
