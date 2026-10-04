#![cfg(windows)]
//! The audio toolbox against the real libmpv (`ao=null`, loopback fixtures): playback speed with
//! pitch correction, the equalizer's chain and clip guard beside normalization, and the output
//! device falling back to the system default.

mod support;

use std::time::{Duration, Instant};

use apollo_domain::audio::{Equalizer, Preset};
use apollo_domain::playback::EngineEvent;
use apollo_media::MediaConfig;
use apollo_runtime::MediaEngine;
use apollo_services_common::LoudnessTable;
use support::*;

const SECS: fn(u64) -> Duration = Duration::from_secs;

fn prop(r: &Rig, name: &str) -> String {
    let got = r.engine.property_values(&[name]);
    got.first().expect("one answer").1.clone()
}

fn gain(r: &Rig) -> f64 {
    let v = prop(r, "volume-gain");
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

fn play(r: &mut Rig, index: usize) {
    let load = r.loads[index];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.open(load, lease, false, 0);
    r.collect_until(SECS(10), |l, e| {
        l == load && matches!(e, EngineEvent::Opened { .. })
    })
    .expect("track opens");
}

fn on(preset: Preset) -> Equalizer {
    Equalizer {
        enabled: true,
        gains_db: preset.gains(),
    }
}

#[test]
fn speed_is_set_on_the_player_and_pitch_correction_is_on() {
    let mut r = rig();
    play(&mut r, 0);
    r.engine.set_speed(1.25);
    let t0 = Instant::now();
    while t0.elapsed() < SECS(5) && prop(&r, "speed").parse::<f64>().unwrap_or(1.0) != 1.25 {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(prop(&r, "speed").parse::<f64>().unwrap(), 1.25);
    assert_eq!(prop(&r, "options/audio-pitch-correction"), "yes");
    r.engine.set_speed(1.0);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(prop(&r, "speed").parse::<f64>().unwrap(), 1.0);
}

#[test]
fn a_speed_that_is_not_on_offer_is_normal_speed() {
    let mut r = rig();
    play(&mut r, 0);
    r.engine.set_speed(3.0);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(prop(&r, "speed").parse::<f64>().unwrap(), 1.0);
}

#[test]
fn the_position_still_counts_media_time_at_another_speed() {
    let mut r = rig();
    r.engine.set_speed(1.5);
    play(&mut r, 0);
    let load = r.loads[0];
    let events = r
        .collect_until(SECS(10), |l, e| {
            l == load && matches!(e, EngineEvent::Position(ms) if *ms >= 1_000)
        })
        .expect("position advances");
    // 1.5x: a second of media passes in well under a second of wall time, and never jumps.
    let positions: Vec<u64> = events
        .iter()
        .filter_map(|(_, e)| match e {
            EngineEvent::Position(ms) => Some(*ms),
            _ => None,
        })
        .collect();
    assert!(positions.windows(2).all(|w| w[0] <= w[1]), "{positions:?}");
}

#[test]
fn a_flat_or_off_equalizer_puts_nothing_in_the_chain() {
    let mut r = rig();
    play(&mut r, 0);
    r.engine.set_equalizer(Equalizer::flat());
    r.engine.set_equalizer(Equalizer {
        enabled: false,
        gains_db: Preset::BassBoost.gains(),
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(!prop(&r, "af").contains("eq0"), "af: {}", prop(&r, "af"));
    assert!(
        gain(&r).abs() < 0.01,
        "no clip guard without a boost: {}",
        gain(&r)
    );
}

#[test]
fn a_boost_installs_the_chain_and_the_clip_guard_takes_the_largest_boost_off() {
    let mut r = rig();
    play(&mut r, 0);
    r.engine.set_equalizer(on(Preset::BassBoost));
    assert!(settles_at(&r, -6.0), "guard: {}", gain(&r));
    let af = prop(&r, "af");
    assert!(af.contains("eq0") && af.contains("eq9"), "af: {af}");
    // Switching it off lets go of the guard; the curve is remembered by the owner, not here.
    r.engine.set_equalizer(Equalizer {
        enabled: false,
        gains_db: Preset::BassBoost.gains(),
    });
    assert!(settles_at(&r, 0.0), "off: {}", gain(&r));
}

#[test]
fn the_bands_take_their_gains_in_place_without_rebuilding_the_chain() {
    let mut r = rig();
    play(&mut r, 0);
    r.engine.set_equalizer(on(Preset::Flat)); // nothing to install yet
    r.engine.set_equalizer(on(Preset::BassBoost));
    r.engine.set_equalizer(on(Preset::Vocal));
    r.engine.set_equalizer(on(Preset::TrebleBoost));
    assert!(settles_at(&r, -6.0), "guard: {}", gain(&r));
    std::thread::sleep(Duration::from_millis(600));
    // Installed once, flat. A band that refused its gain would have had the chain installed
    // again with the curve's gains in it.
    let af = prop(&r, "af");
    assert!(af.contains("eq0"), "af: {af}");
    assert!(
        !af.contains("g=6.00") && !af.contains("g=4.00"),
        "the chain was rebuilt: {af}"
    );
    // Playback was not disturbed by any of it.
    let load = r.loads[0];
    let events = r.drain_for(Duration::from_millis(300));
    assert!(
        !events
            .iter()
            .any(|(l, e)| *l == load && matches!(e, EngineEvent::Error(_))),
        "{events:?}"
    );
}

#[test]
fn the_clip_guard_rides_with_normalization_and_follows_the_next_track() {
    let table = LoudnessTable::new();
    table.set(VID, 5.0);
    let mut r = rig_with(MediaConfig {
        audio_output: Some("null".into()),
        loudness: table.clone(),
        ..Default::default()
    });
    play(&mut r, 0);
    assert!(settles_at(&r, -5.0), "normalization alone: {}", gain(&r));
    r.engine.set_equalizer(on(Preset::BassBoost));
    assert!(
        settles_at(&r, -11.0),
        "normalization plus guard: {}",
        gain(&r)
    );

    // A track that starts afterwards has both from its first sample.
    r.engine.stop();
    play(&mut r, 1);
    assert!((gain(&r) + 11.0).abs() < 0.01, "next track: {}", gain(&r));
}

#[test]
fn a_curve_set_before_anything_plays_is_in_place_for_the_first_track() {
    let mut r = rig();
    r.engine.set_equalizer(on(Preset::Vocal));
    std::thread::sleep(Duration::from_millis(200));
    assert!(prop(&r, "af").contains("eq0"), "installed while idle");
    play(&mut r, 0);
    assert!(
        (gain(&r) + 4.0).abs() < 0.01,
        "guard from the first sample: {}",
        gain(&r)
    );
}

#[test]
fn an_unlisted_device_plays_on_the_system_default_and_a_listed_one_is_used() {
    let mut r = rig();
    // Nothing is listed under this name, so the default plays.
    r.engine
        .set_audio_device(Some("wasapi/{not-a-real-device}".into()));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(prop(&r, "audio-device"), "auto");

    if let Some(first) = r.engine.audio_devices().first().cloned() {
        r.engine.set_audio_device(Some(first.id.clone()));
        let t0 = Instant::now();
        while t0.elapsed() < SECS(5) && prop(&r, "audio-device") != first.id {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(prop(&r, "audio-device"), first.id);
        assert!(!first.name.is_empty());
        // And back to the default when the owner asks for it.
        r.engine.set_audio_device(None);
        let t0 = Instant::now();
        while t0.elapsed() < SECS(5) && prop(&r, "audio-device") != "auto" {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(prop(&r, "audio-device"), "auto");
    }
}

#[test]
fn the_device_list_never_offers_auto_or_a_nameless_entry() {
    let r = rig();
    for d in r.engine.audio_devices() {
        assert_ne!(d.id, "auto");
        assert!(!d.id.is_empty() && !d.name.is_empty());
    }
}
