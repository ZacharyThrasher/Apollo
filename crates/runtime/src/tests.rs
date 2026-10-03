//! Integration tests: coordinator + driver + mock adapters (no real network or audio).

use apollo_domain::model::Track;
use apollo_domain::playback::{EngineEvent, Event, Failure, Phase};
use apollo_domain::queue::Repeat;

use crate::mock::{EngineCall, MockEngine, MockResolver};
use crate::Driver;

type D = Driver<MockResolver, MockEngine>;

fn driver() -> D {
    Driver::new(MockResolver::default(), MockEngine::default())
}

fn tracks(ids: &[&str]) -> Vec<Track> {
    ids.iter().map(|i| Track::new(*i, *i)).collect()
}

fn current(d: &D) -> String {
    d.coordinator()
        .queue()
        .current()
        .unwrap()
        .track
        .video_id
        .as_str()
        .to_owned()
}

/// Resolve and open the active load; returns it.
fn open_active(d: &mut D) -> apollo_domain::playback::LoadId {
    let load = d.coordinator().active_load().expect("an active load");
    let ev = d.resolver.ok(load);
    d.handle(ev);
    let ev = d.engine.event(
        load,
        EngineEvent::Opened {
            duration_ms: Some(180_000),
        },
    );
    d.handle(ev);
    load
}

// TEST-05 (integration)
#[test]
fn slow_resolution_of_a_never_reaches_the_engine() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    let a = d.coordinator().active_load().unwrap();
    d.handle(Event::Next);
    let b = d.coordinator().active_load().unwrap();
    assert!(d.resolver.cancelled.contains(&a));

    let late = d.resolver.ok(a);
    d.handle(late);
    assert!(d.engine.opens().is_empty(), "stale A must never open");
    assert!(!d.engine.is_sounding());

    open_active(&mut d);
    assert_eq!(d.engine.opens(), [b]);
    assert_eq!(d.engine.audible_load(), Some(b));
    assert_eq!(current(&d), "b");
}

// TEST-06 (integration)
#[test]
fn twenty_rapid_nexts_leave_one_audible_owner_and_no_pending_work() {
    let mut d = driver();
    let ids: Vec<String> = (0..30).map(|i| format!("t{i}")).collect();
    let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
    d.handle(Event::PlayContext {
        tracks: tracks(&refs),
        start: 0,
    });
    let mut seen = vec![d.coordinator().active_load().unwrap()];
    for _ in 0..20 {
        d.handle(Event::Next);
        seen.push(d.coordinator().active_load().unwrap());
    }
    // Only the latest resolution is still wanted.
    assert_eq!(d.resolver.pending_loads(), [*seen.last().unwrap()]);
    // Late completions for every superseded load arrive, newest-stale first.
    for load in seen.iter().rev().skip(1).copied().collect::<Vec<_>>() {
        let ev = d.resolver.ok(load);
        d.handle(ev);
    }
    assert!(d.engine.opens().is_empty());
    let latest = open_active(&mut d);
    assert_eq!(d.engine.opens(), [latest]);
    assert!(d.engine.is_sounding());
    assert_eq!(current(&d), "t20");
}

// TEST-10 (integration)
#[test]
fn pause_during_load_never_makes_sound() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    d.handle(Event::Pause);
    open_active(&mut d);
    assert!(
        !d.engine.is_sounding(),
        "a pause entered while loading must hold"
    );
    assert_eq!(d.coordinator().phase(), Phase::Paused);

    // Seek while paused stays silent too.
    d.handle(Event::Seek {
        position_ms: 90_000,
    });
    let load = d.coordinator().active_load().unwrap();
    let gen = d
        .engine
        .calls
        .iter()
        .rev()
        .find_map(|c| match c {
            EngineCall::Seek { generation, .. } => Some(*generation),
            _ => None,
        })
        .unwrap();
    let ev = d.engine.event(
        load,
        EngineEvent::Seeked {
            gen,
            position_ms: 90_000,
        },
    );
    d.handle(ev);
    assert!(!d.engine.is_sounding());
}

// TEST-11 (integration)
#[test]
fn preload_failure_does_not_interrupt_the_audible_track() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c"]),
        start: 0,
    });
    let active = open_active(&mut d);
    let preload = d
        .resolver
        .pending
        .iter()
        .find(|r| r.preload)
        .expect("next track preloading")
        .load;
    let ev = d.resolver.err(preload, Failure::Network);
    d.handle(ev);
    assert!(d.engine.is_sounding());
    assert_eq!(d.engine.audible_load(), Some(active));
    assert_eq!(d.coordinator().phase(), Phase::Playing);
    // The failure is remembered: the same entry is not re-requested in a loop.
    assert_eq!(d.resolver.started.iter().filter(|r| r.preload).count(), 1);
}

// TEST-12 (integration), both orders
#[test]
fn eof_and_transition_in_either_order_advance_exactly_once() {
    for transition_first in [false, true] {
        let mut d = driver();
        d.handle(Event::PlayContext {
            tracks: tracks(&["a", "b", "c"]),
            start: 0,
        });
        let old = open_active(&mut d);
        let pre = d.resolver.pending.iter().find(|r| r.preload).unwrap().load;
        let ev = d.resolver.ok(pre);
        d.handle(ev);

        let eof = d.engine.event(old, EngineEvent::Eof);
        let transitioned = d.engine.event(pre, EngineEvent::Transitioned);
        if transition_first {
            d.handle(transitioned);
            d.handle(eof);
        } else {
            d.handle(eof);
            d.handle(transitioned);
        }
        assert_eq!(current(&d), "b", "transition_first={transition_first}");
        assert_eq!(d.coordinator().queue().history().len(), 1);
        assert_eq!(d.coordinator().active_load(), Some(pre));
    }
}

// TEST-04 (integration)
#[test]
fn logout_silences_everything_and_cancels_pending_work() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    let active = open_active(&mut d);
    assert!(d.engine.is_sounding());
    let preload = d.resolver.pending_loads();
    d.handle(Event::Logout);
    assert!(!d.engine.is_sounding());
    assert!(d.engine.calls.contains(&EngineCall::Stop));
    assert!(d.resolver.pending.is_empty());
    for l in preload.iter().chain([&active]) {
        assert!(d.resolver.cancelled.contains(l));
        // Even if their results were already in flight, nothing starts.
        let ev = d.resolver.ok(*l);
        d.handle(ev);
    }
    assert!(
        d.engine
            .calls
            .iter()
            .filter(|c| matches!(c, EngineCall::Open { .. }))
            .count()
            == 1
    );
    assert!(d.coordinator().queue().current().is_none());
}

#[test]
fn playing_a_short_album_through_to_the_end() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    let a = open_active(&mut d);
    let ev = d.engine.event(a, EngineEvent::Eof);
    d.handle(ev);
    assert_eq!(current(&d), "b");
    let b = open_active(&mut d);
    assert_ne!(a, b);
    let ev = d.engine.event(b, EngineEvent::Eof);
    d.handle(ev);
    assert_eq!(d.coordinator().phase(), Phase::Idle);
    assert!(!d.engine.is_sounding());
    assert!(
        d.resolver.pending.is_empty(),
        "no leaked resolutions at the end"
    );
}

#[test]
fn repeat_one_never_preloads_and_restarts_the_same_track() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    d.edit_queue(|q| q.set_repeat(Repeat::One));
    let a = open_active(&mut d);
    assert!(d.resolver.pending.iter().all(|r| !r.preload));
    let ev = d.engine.event(a, EngineEvent::Eof);
    d.handle(ev);
    assert_eq!(current(&d), "a");
    assert_ne!(
        d.coordinator().active_load(),
        Some(a),
        "a restart is a new load"
    );
}

#[test]
fn failed_resolution_of_the_requested_track_keeps_it_selected_and_silent() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    let a = d.coordinator().active_load().unwrap();
    let ev = d.resolver.err(a, Failure::Restricted);
    d.handle(ev);
    assert_eq!(d.coordinator().phase(), Phase::Failed);
    assert_eq!(current(&d), "a");
    assert!(!d.engine.is_sounding());
}

// Gapless playback (integration)

fn preload_calls(d: &D) -> Vec<apollo_domain::playback::LoadId> {
    d.engine
        .calls
        .iter()
        .filter_map(|c| match c {
            EngineCall::Preload(l) => Some(*l),
            _ => None,
        })
        .collect()
}

fn preload_request(d: &D) -> apollo_domain::playback::LoadId {
    d.resolver
        .pending
        .iter()
        .find(|r| r.preload)
        .expect("next track preloading")
        .load
}

#[test]
fn the_engine_is_handed_the_next_track_only_once_it_is_resolved() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c"]),
        start: 0,
    });
    open_active(&mut d);
    let pre = preload_request(&d);
    assert!(preload_calls(&d).is_empty(), "nothing to hand over while resolving");
    let ev = d.resolver.ok(pre);
    d.handle(ev);
    assert_eq!(preload_calls(&d), vec![pre]);
    assert_eq!(d.engine.lined_up(), Some(pre));
}

#[test]
fn a_failed_preload_hands_the_engine_nothing() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c"]),
        start: 0,
    });
    open_active(&mut d);
    let pre = preload_request(&d);
    let ev = d.resolver.err(pre, Failure::Network);
    d.handle(ev);
    assert!(preload_calls(&d).is_empty());
    assert_eq!(d.engine.lined_up(), None);
}

#[test]
fn a_gapless_boundary_moves_on_without_a_second_open_and_lines_up_the_next() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c"]),
        start: 0,
    });
    let a = open_active(&mut d);
    let b = preload_request(&d);
    let ev = d.resolver.ok(b);
    d.handle(ev);

    for ev in d.engine.gapless_boundary(200_000) {
        d.handle(ev);
    }
    assert_eq!(current(&d), "b");
    assert_eq!(d.coordinator().active_load(), Some(b));
    assert_eq!(d.coordinator().phase(), Phase::Playing);
    assert_eq!(d.coordinator().duration_ms(), Some(200_000), "duration comes with Opened");
    assert_eq!(d.engine.opens(), vec![a], "the engine was never re-opened");
    assert!(d.engine.is_sounding());
    assert_eq!(d.engine.audible_load(), Some(b));

    // The track after it is being resolved, and is handed over in turn.
    let c = preload_request(&d);
    let ev = d.resolver.ok(c);
    d.handle(ev);
    assert_eq!(preload_calls(&d), vec![b, c]);
}

#[test]
fn without_gapless_the_next_track_opens_at_the_end_of_the_current_one() {
    // An engine that declines the preload (gapless off) just reports the end of the track.
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    let a = open_active(&mut d);
    let b = preload_request(&d);
    let ev = d.resolver.ok(b);
    d.handle(ev);
    let ev = d.engine.event(a, EngineEvent::Eof);
    d.handle(ev);
    assert_eq!(current(&d), "b");
    assert_eq!(d.engine.opens(), vec![a, b], "the resolved preload is opened as it was");
}

#[test]
fn editing_the_queue_takes_the_lined_up_track_back() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c"]),
        start: 0,
    });
    open_active(&mut d);
    let b = preload_request(&d);
    let ev = d.resolver.ok(b);
    d.handle(ev);
    assert_eq!(d.engine.lined_up(), Some(b));

    d.edit_queue(|q| {
        q.play_next(tracks(&["x"]));
    });
    assert_eq!(d.engine.lined_up(), None, "the old next track is taken back");
    assert!(d.engine.calls.contains(&EngineCall::Close(b)));
    // The new next track is resolved and handed over instead.
    let x = preload_request(&d);
    assert_ne!(x, b);
    let ev = d.resolver.ok(x);
    d.handle(ev);
    assert_eq!(d.engine.lined_up(), Some(x));
}

#[test]
fn shuffling_and_skipping_never_leave_a_stale_track_lined_up() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c", "d"]),
        start: 0,
    });
    open_active(&mut d);
    let b = preload_request(&d);
    let ev = d.resolver.ok(b);
    d.handle(ev);

    // Whatever the shuffle did to the order, only the coordinator's own preload is lined up.
    d.edit_queue(|q| q.set_shuffle(true, 7));
    let lined = d.engine.lined_up();
    assert!(lined.is_none() || lined == d.coordinator().preload_load());

    // Skipping hands the old preload back (or opens it); nothing stays lined up behind it.
    d.handle(Event::Next);
    assert_eq!(d.engine.lined_up(), None);
}

#[test]
fn repeat_one_lines_nothing_up() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    d.edit_queue(|q| q.set_repeat(Repeat::One));
    open_active(&mut d);
    assert!(preload_calls(&d).is_empty());
    assert_eq!(d.engine.lined_up(), None);
}

#[test]
fn turning_gapless_on_lines_up_a_track_that_was_resolved_while_it_was_off() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    open_active(&mut d);
    d.set_gapless(false);
    let b = preload_request(&d);
    let ev = d.resolver.ok(b);
    d.handle(ev);
    // An engine with gapless off would have declined; the mock records the hand-over either way,
    // so check the switch re-offers it.
    let before = preload_calls(&d).len();
    d.set_gapless(true);
    assert!(d.engine.calls.contains(&EngineCall::SetGapless(false)));
    assert!(d.engine.calls.contains(&EngineCall::SetGapless(true)));
    assert_eq!(preload_calls(&d).len(), before + 1);
    assert_eq!(*preload_calls(&d).last().unwrap(), b);
}

#[test]
fn turning_gapless_on_with_nothing_resolved_hands_over_nothing() {
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b"]),
        start: 0,
    });
    open_active(&mut d);
    d.set_gapless(true);
    assert!(preload_calls(&d).is_empty());
}

#[test]
fn a_transition_the_coordinator_no_longer_wants_does_not_skip_the_current_track() {
    // The engine moved on to a track the queue has since dropped: the coordinator keeps its own
    // state (the engine is told the load is gone when the preload is cancelled).
    let mut d = driver();
    d.handle(Event::PlayContext {
        tracks: tracks(&["a", "b", "c"]),
        start: 0,
    });
    let a = open_active(&mut d);
    let b = preload_request(&d);
    let ev = d.resolver.ok(b);
    d.handle(ev);
    d.edit_queue(|q| {
        q.play_next(tracks(&["x"]));
    });
    let ev = d.engine.event(b, EngineEvent::Transitioned);
    d.handle(ev);
    assert_eq!(current(&d), "a");
    assert_eq!(d.coordinator().active_load(), Some(a));
}
