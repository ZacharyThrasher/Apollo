#![cfg(windows)]
//! Integration tests: the real libmpv through `MpvEngine`, fed by a loopback HTTP server and
//! the owned AAC/Opus fixtures. Audio goes to mpv's `null` output, so nothing is audible.

mod support;

use std::time::Duration;

use apollo_domain::playback::EngineEvent;
use apollo_runtime::MediaEngine;
use support::*;

const SECS: fn(u64) -> Duration = Duration::from_secs;

fn positions(evs: &[(apollo_domain::playback::LoadId, EngineEvent)]) -> Vec<u64> {
    evs.iter()
        .filter_map(|(_, e)| match e {
            EngineEvent::Position(p) => Some(*p),
            _ => None,
        })
        .collect()
}

// TEST-21 (local fixtures): codecs actually decode, seeking works, end of file is detected.
#[test]
fn aac_and_opus_open_play_seek_and_reach_eof() {
    for file in ["tone_aac.m4a", "tone_opus.opus", "tone_opus.webm"] {
        let mut r = rig();
        let load = r.loads[0];
        let lease = r.lease_path(&format!("/{file}"));
        r.engine.open(load, lease, false, 0);

        let evs = r
            .collect_until(SECS(10), |l, e| l == load && matches!(e, EngineEvent::Opened { .. }))
            .unwrap_or_else(|got| panic!("{file}: no Opened, got {got:?}"));
        let Some((_, EngineEvent::Opened { duration_ms })) = evs.last() else {
            unreachable!()
        };
        let d = duration_ms.expect("duration known");
        assert!((11_900..=12_100).contains(&d), "{file}: duration {d}");

        // About 4 Hz while playing (checked over ~1.3 s).
        let evs = r.drain_for(Duration::from_millis(1300));
        let p = positions(&evs);
        assert!((3..=7).contains(&p.len()), "{file}: {} position events in 1.3s: {p:?}", p.len());
        assert!(p.windows(2).all(|w| w[0] < w[1]), "{file}: positions must advance: {p:?}");

        // Seek near the end with a generation; the reply must carry that generation.
        r.engine.seek(load, 7, 11_000);
        let evs = r
            .collect_until(SECS(10), |l, e| l == load && matches!(e, EngineEvent::Seeked { .. }))
            .unwrap_or_else(|got| panic!("{file}: no Seeked, got {got:?}"));
        let Some((_, EngineEvent::Seeked { gen, position_ms })) = evs.last() else {
            unreachable!()
        };
        assert_eq!(*gen, 7);
        assert!(
            (10_950..=11_400).contains(position_ms),
            "{file}: seeked to {position_ms}"
        );

        // Then a real end-of-file, exactly once, and silence afterwards.
        let evs = r
            .collect_until(SECS(10), |l, e| l == load && *e == EngineEvent::Eof)
            .unwrap_or_else(|got| panic!("{file}: no Eof, got {got:?}"));
        assert!(evs.iter().all(|(l, _)| *l == load));
        let after = r.drain_for(Duration::from_millis(700));
        assert!(after.is_empty(), "{file}: events after Eof: {after:?}");
    }
}
