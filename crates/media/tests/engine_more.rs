#![cfg(windows)]
//! More integration tests for `MpvEngine`: pause/seek semantics, stale loads, failure classes,
//! lease checks, header policy, hardening and shutdown. Real libmpv, loopback server, silent
//! `null` audio output.

mod support;

use std::time::Duration;

use apollo_domain::playback::{EngineEvent, Failure, LeaseId, LoadId};
use apollo_runtime::MediaEngine;
use support::*;

const SECS: fn(u64) -> Duration = Duration::from_secs;

fn positions(evs: &[(LoadId, EngineEvent)]) -> Vec<u64> {
    evs.iter()
        .filter_map(|(_, e)| match e {
            EngineEvent::Position(p) => Some(*p),
            _ => None,
        })
        .collect()
}

fn opened(load: LoadId) -> impl Fn(LoadId, &EngineEvent) -> bool {
    move |l, e| l == load && matches!(e, EngineEvent::Opened { .. })
}

// TEST-10 (engine half): neither opening paused nor seeking while paused makes sound/progress.
#[test]
fn paused_open_and_paused_seek_never_advance() {
    let mut r = rig();
    let load = r.loads[0];
    let lease = r.lease_path("/tone_opus.opus");
    r.engine.open(load, lease, true, 3_000);
    r.collect_until(SECS(10), opened(load)).expect("Opened");
    let quiet = r.drain_for(Duration::from_millis(1200));
    assert!(positions(&quiet).is_empty(), "paused open emitted positions: {quiet:?}");
    let opts = r.engine.option_values(&["pause"]);
    assert_eq!(opts[0].1, "yes", "{opts:?}");

    // Seek while paused: a Seeked reply, still no progress, still paused.
    r.engine.seek(load, 1, 8_000);
    let evs = r
        .collect_until(SECS(10), |l, e| l == load && matches!(e, EngineEvent::Seeked { .. }))
        .expect("Seeked");
    let Some((_, EngineEvent::Seeked { gen, position_ms })) = evs.last() else {
        unreachable!()
    };
    assert_eq!(*gen, 1);
    assert!((7_950..=8_200).contains(position_ms), "{position_ms}");
    let quiet = r.drain_for(Duration::from_millis(1000));
    assert!(positions(&quiet).is_empty(), "positions while paused: {quiet:?}");
    assert_eq!(r.engine.option_values(&["pause"])[0].1, "yes");

    // Resume: progress starts from the sought position; pause again: it stops.
    r.engine.resume(load);
    let evs = r.drain_for(Duration::from_millis(1200));
    let p = positions(&evs);
    assert!(!p.is_empty() && p[0] >= 7_900, "after resume: {p:?}");
    r.engine.pause(load);
    r.drain_for(Duration::from_millis(400)); // let in-flight samples land
    let quiet = r.drain_for(Duration::from_millis(1000));
    assert!(positions(&quiet).is_empty(), "positions after pause: {quiet:?}");
}

// TEST-05 / TEST-06 (engine half): opening a new load replaces the old one and the old one
// never reports anything again, even when the replacement is rapid and repeated.
#[test]
fn replaced_loads_stay_silent() {
    let mut r = rig();
    // Load 0 is slow to start; load 1 replaces it before it can open.
    let slow = r.lease_path("/slow/1500/tone_aac.m4a");
    let fast = r.lease_path("/tone_opus.opus");
    r.engine.open(r.loads[0], slow, false, 0);
    r.engine.open(r.loads[1], fast, false, 0);
    let evs = r
        .collect_until(SECS(10), opened(r.loads[1]))
        .expect("load 1 opens");
    assert!(evs.iter().all(|(l, _)| *l != r.loads[0]), "stale load spoke: {evs:?}");
    let later = r.drain_for(SECS(2));
    assert!(
        later.iter().all(|(l, _)| *l != r.loads[0]),
        "stale load spoke later: {later:?}"
    );

    // Twenty rapid replacements: exactly one owner at the end.
    for i in 0..20 {
        let lease = r.lease_path("/tone_opus.webm");
        r.engine.open(r.loads[i + 2], lease, false, 0);
    }
    let last = r.loads[21];
    r.collect_until(SECS(15), opened(last)).expect("last load opens");
    let tail = r.drain_for(SECS(1));
    assert!(
        tail.iter().all(|(l, _)| *l == last),
        "events from replaced loads after the last open: {tail:?}"
    );
}

#[test]
fn close_and_stop_silence_the_engine() {
    let mut r = rig();
    let load = r.loads[0];
    let lease = r.lease_path("/tone_aac.m4a");
    r.engine.open(load, lease, false, 0);
    r.collect_until(SECS(10), opened(load)).unwrap();
    r.engine.close(load);
    r.drain_for(Duration::from_millis(300)); // anything already in flight
    assert!(r.drain_for(SECS(1)).is_empty(), "events after close");

    let load2 = r.loads[1];
    let lease = r.lease_path("/tone_aac.m4a");
    r.engine.open(load2, lease, false, 0);
    r.collect_until(SECS(10), opened(load2)).unwrap();
    r.engine.stop();
    r.drain_for(Duration::from_millis(300));
    assert!(r.drain_for(SECS(1)).is_empty(), "events after stop");

    // A command for a load that is no longer active is ignored (no seek, no resume).
    r.engine.seek(load2, 1, 5_000);
    r.engine.resume(load2);
    assert!(r.drain_for(Duration::from_millis(600)).is_empty());
}

// NET-02: failure classes come from the actual failure, not a generic "blocked".
#[test]
fn failures_are_classified() {
    use Failure::*;
    let cases: &[(&str, &[Failure])] = &[
        ("/status/403", &[LeaseExpired]),
        ("/status/404", &[LeaseExpired]),
        ("/status/410", &[LeaseExpired]),
        ("/status/429", &[Throttled]),
        ("/status/503", &[Network]),
        ("/garbage", &[Unsupported, Media]),
        ("/truncated", &[Network]),
    ];
    for (path, accept) in cases {
        let mut r = rig();
        let load = r.loads[0];
        let lease = r.lease_path(path);
        r.engine.open(load, lease, false, 0);
        let got = r
            .collect_until(SECS(20), |l, e| {
                l == load && matches!(e, EngineEvent::Error(_) | EngineEvent::Eof)
            })
            .unwrap_or_else(|g| panic!("{path}: no terminal event, got {g:?}"));
        match got.last() {
            Some((_, EngineEvent::Error(f))) => {
                eprintln!("{path} -> {f:?}");
                assert!(accept.contains(f), "{path}: classified as {f:?}, expected one of {accept:?}")
            }
            other => panic!("{path}: expected Error, got {other:?}"),
        }
    }
}

#[test]
fn connection_refused_is_a_network_failure() {
    let mut r = rig();
    // A port nothing listens on.
    let free = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let lease = r.lease_url(format!("http://127.0.0.1:{free}/x.m4a"));
    r.engine.open(r.loads[0], lease, false, 0);
    let got = r
        .collect_until(SECS(20), |_, e| matches!(e, EngineEvent::Error(_)))
        .expect("an error");
    assert!(
        matches!(got.last(), Some((_, EngineEvent::Error(Failure::Network)))),
        "{got:?}"
    );
}

// TEST-13 (engine half): a lease that is already stale or gone never touches the network.
#[test]
fn stale_or_missing_leases_fail_fast_without_a_request() {
    let mut r = rig();
    let expired = r.lease_full(r.server.url("/tone_aac.m4a"), vec![], Some(1_000)); // 1970
    r.engine.open(r.loads[0], expired, false, 0);
    r.engine.open(r.loads[1], LeaseId(9_999), false, 0); // never issued
    let got = r.drain_for(SECS(2));
    let errs: Vec<_> = got
        .iter()
        .filter(|(_, e)| *e == EngineEvent::Error(Failure::LeaseExpired))
        .map(|(l, _)| *l)
        .collect();
    assert_eq!(errs, vec![r.loads[0], r.loads[1]], "{got:?}");
    assert!(r.server.requests().is_empty(), "stale lease reached the network");
}

// RES-03: only non-secret headers are sent; Cookie/Authorization are dropped before the engine.
#[test]
fn only_allowed_headers_reach_the_server() {
    let mut r = rig();
    let load = r.loads[0];
    let lease = r.lease_full(
        r.server.url("/tone_opus.opus"),
        vec![
            ("User-Agent", "ApolloTestUA/1.0"),
            ("Referer", "https://music.youtube.com/"),
            ("Cookie", "SID=canary-cookie-value"),
            ("Authorization", "Bearer canary-bearer"),
            ("X-Custom", "canary-custom"),
        ],
        None,
    );
    r.engine.open(load, lease, false, 0);
    r.collect_until(SECS(10), opened(load)).unwrap();
    let reqs = r.server.requests();
    assert!(!reqs.is_empty());
    for q in &reqs {
        assert_eq!(q.header("user-agent"), Some("ApolloTestUA/1.0"), "{q:?}");
        assert_eq!(q.header("referer"), Some("https://music.youtube.com/"), "{q:?}");
        for forbidden in ["cookie", "authorization", "x-custom"] {
            assert!(q.header(forbidden).is_none(), "{forbidden} leaked: {q:?}");
        }
        let all = format!("{:?}", q.headers);
        assert!(!all.contains("canary"), "{all}");
    }
}

// The binding sets ONE header list for the load. This test records what mpv/ffmpeg do on a
// cross-origin redirect (RES-03 asks for same-origin-only headers). The invariant we can
// guarantee is that nothing secret can ever be in the list.
#[test]
fn cross_origin_redirect_header_behaviour_is_recorded() {
    let mut r = rig();
    let other = TestServer::start(); // a different origin (different port)
    let target = other.url("/tone_opus.opus");
    let lease = r.lease_full(
        r.server.url(&format!("/redirect?to={target}")),
        vec![
            ("User-Agent", "ApolloTestUA/1.0"),
            ("Referer", "https://music.youtube.com/"),
        ],
        None,
    );
    let load = r.loads[0];
    r.engine.open(load, lease, false, 0);
    let _ = r.collect_until(SECS(10), |l, e| {
        l == load && matches!(e, EngineEvent::Opened { .. } | EngineEvent::Error(_))
    });
    let origin_a = r.server.requests();
    let origin_b = other.requests();
    eprintln!("REDIRECT FOLLOWED: {}", !origin_b.is_empty());
    for q in &origin_b {
        eprintln!(
            "REDIRECT TARGET RECEIVED: ua={:?} referer={:?} cookie={:?}",
            q.header("user-agent"),
            q.header("referer"),
            q.header("cookie")
        );
        assert!(q.header("cookie").is_none() && q.header("authorization").is_none());
    }
    assert!(!origin_a.is_empty());
}

#[test]
fn engine_is_hardened_against_user_config_and_disk() {
    let r = rig();
    let got = r.engine.option_values(&[
        "config",
        "load-scripts",
        "ytdl",
        "vo",
        "vid",
        "cache-on-disk",
        "input-media-keys",
        "load-unsafe-playlists",
        "tls-verify",
        "ao",
    ]);
    let get = |k: &str| got.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    for (k, v) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("vo", "null"),
        ("vid", "no"),
        ("cache-on-disk", "no"),
        ("input-media-keys", "no"),
        ("load-unsafe-playlists", "no"),
        ("tls-verify", "yes"),
        ("ao", "null"),
    ] {
        assert_eq!(get(k), Some(v), "{k}: {got:?}");
    }
}

#[test]
fn dropping_the_engine_mid_playback_is_prompt() {
    let mut r = rig();
    let load = r.loads[0];
    let lease = r.lease_path("/tone_aac.m4a");
    r.engine.open(load, lease, false, 0);
    r.collect_until(SECS(10), opened(load)).unwrap();
    let t0 = std::time::Instant::now();
    drop(r.engine);
    assert!(t0.elapsed() < SECS(3), "shutdown took {:?}", t0.elapsed());
}
