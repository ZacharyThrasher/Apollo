//! Randomised event-sequence tests for the playback coordinator.
//!
//! Each run drives the coordinator with random user commands, queue edits and
//! asynchronous completions delivered in arbitrary order (including for loads
//! that were long since cancelled). A tiny model of the engine checks the
//! invariants the spec treats as release blockers: one audible owner, no stale
//! load ever opened, and intent/engine agreement.

use std::collections::HashSet;

use crate::model::{QueueEntryId, Track};
use crate::playback::{
    Coordinator, Effect, EngineEvent, Event, Failure, Intent, LeaseId, LoadId, Phase,
};
use crate::queue::Repeat;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
}

fn track(rng: &mut Rng) -> Track {
    // Small ID space so duplicates are common (distinct occurrences must stay distinct).
    let id = format!("v{}", rng.below(5));
    Track::new(id.clone(), id)
}

fn tracks(rng: &mut Rng, max: usize) -> Vec<Track> {
    (0..rng.below(max + 1)).map(|_| track(rng)).collect()
}

fn failure(rng: &mut Rng) -> Failure {
    [
        Failure::SessionExpired,
        Failure::Restricted,
        Failure::Throttled,
        Failure::Network,
        Failure::Unsupported,
        Failure::LeaseExpired,
        Failure::Media,
        Failure::Device,
    ][rng.below(8)]
}

fn engine_event(rng: &mut Rng) -> EngineEvent {
    match rng.below(8) {
        0 => EngineEvent::Opened {
            duration_ms: if rng.chance(70) {
                Some(rng.next() % 300_000)
            } else {
                None
            },
        },
        1 => EngineEvent::Buffering(rng.chance(50)),
        2 => EngineEvent::Position(rng.next() % 300_000),
        3 => EngineEvent::Seeked {
            gen: rng.next() % 6,
            position_ms: rng.next() % 300_000,
        },
        4 => EngineEvent::Eof,
        5 => EngineEvent::Transitioned,
        _ => EngineEvent::Error(failure(rng)),
    }
}

struct Harness {
    c: Coordinator,
    /// Every load ever seen, never pruned: stale completions keep arriving.
    loads: Vec<LoadId>,
    cancelled: HashSet<LoadId>,
    /// Model of the engine's pause state for the active load.
    engine_paused: Option<bool>,
    seed: u64,
    step: usize,
}

impl Harness {
    fn fail(&self, what: &str) -> ! {
        panic!("seed {} step {}: {what}", self.seed, self.step)
    }

    fn apply(&mut self, event: Event) {
        let transitioned = matches!(
            &event,
            Event::Engine {
                event: EngineEvent::Transitioned,
                ..
            }
        );
        let before = self.c.active_load();
        let trace = std::env::var("FUZZ_TRACE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            == Some(self.seed);
        if trace {
            eprintln!(
                "step {} EVENT {:?} (phase {:?}, intent {:?}, active {:?})",
                self.step,
                event,
                self.c.phase(),
                self.c.intent(),
                before
            );
        }
        let fx = self.c.handle(event);
        if trace {
            eprintln!(
                "    -> {:?} phase {:?} intent {:?}",
                fx,
                self.c.phase(),
                self.c.intent()
            );
        }
        let after = self.c.active_load();
        let opened_active = fx
            .iter()
            .any(|e| matches!(e, Effect::Open { load, .. } if Some(*load) == after));
        if transitioned && after != before && !opened_active {
            // The engine itself moved to the preloaded entry and is playing.
            self.engine_paused = Some(false);
        }
        self.process(fx);
    }

    fn process(&mut self, fx: Vec<Effect>) {
        for e in fx {
            match e {
                Effect::Resolve { load, .. } | Effect::ResolvePreload { load, .. } => {
                    if self.cancelled.contains(&load) {
                        self.fail("resolve issued for a cancelled load id");
                    }
                    self.loads.push(load);
                }
                Effect::Open {
                    load, start_paused, ..
                } => {
                    if Some(load) != self.c.active_load() {
                        self.fail("Open for a load that does not own audio");
                    }
                    if self.cancelled.contains(&load) {
                        self.fail("Open for a cancelled load");
                    }
                    if self.c.phase() != Phase::Opening {
                        self.fail("Open outside the Opening phase");
                    }
                    self.engine_paused = Some(start_paused);
                }
                Effect::Pause { load } | Effect::Resume { load } | Effect::Seek { load, .. } => {
                    if Some(load) != self.c.active_load() {
                        self.fail("engine command for a load that does not own audio");
                    }
                    match e {
                        Effect::Pause { .. } => self.engine_paused = Some(true),
                        Effect::Resume { .. } => self.engine_paused = Some(false),
                        _ => {}
                    }
                }
                Effect::Cancel { load } => {
                    self.cancelled.insert(load);
                    if Some(load) == self.c.active_load() {
                        self.fail("active load cancelled but still owned");
                    }
                }
                Effect::Stop => self.engine_paused = None,
            }
        }
    }

    fn check(&self) {
        let c = &self.c;
        let q = c.queue();

        // Queue: IDs unique, current appears in no other segment.
        let mut ids: Vec<QueueEntryId> = q.history().iter().map(|e| e.id).collect();
        ids.extend(q.current().map(|e| e.id));
        ids.extend(q.upcoming().map(|e| e.id));
        let unique: HashSet<_> = ids.iter().collect();
        if unique.len() != ids.len() {
            self.fail("queue entry ID appears twice");
        }

        // Exactly one audible owner, tied to the queue's current entry.
        let loading_or_loaded = matches!(
            c.phase(),
            Phase::Resolving
                | Phase::Opening
                | Phase::Buffering
                | Phase::Playing
                | Phase::Paused
                | Phase::Seeking
        );
        if loading_or_loaded != c.active_load().is_some() {
            self.fail("active load and phase disagree");
        }
        if let Some(entry) = c.active_entry() {
            if q.current().map(|e| e.id) != Some(entry) {
                self.fail("active load belongs to an entry that is no longer current");
            }
        }
        if let (Some(p), Some(a)) = (c.preload_load(), c.active_load()) {
            if p == a {
                self.fail("preload and active share a load");
            }
        }

        // Intent and engine agree once loaded.
        match c.phase() {
            Phase::Playing => {
                if c.intent() != Intent::Playing {
                    self.fail("Playing phase with Paused intent");
                }
                if self.engine_paused != Some(false) {
                    self.fail("Playing phase but engine is paused");
                }
            }
            Phase::Paused => {
                if c.intent() != Intent::Paused {
                    self.fail("Paused phase with Playing intent");
                }
                if self.engine_paused != Some(true) {
                    self.fail("Paused phase but engine is playing");
                }
            }
            _ => {}
        }
        if c.repeat_is_one_with_preload() {
            self.fail("repeat-one must not keep a preload");
        }
    }

    fn random_event(&mut self, rng: &mut Rng) {
        match rng.below(100) {
            0..=9 => self.apply(Event::PlayContext {
                tracks: tracks(rng, 6),
                start: rng.below(4),
            }),
            10..=17 => self.apply(Event::Play),
            18..=25 => self.apply(Event::Pause),
            26..=33 => self.apply(Event::Next),
            34..=37 => self.apply(Event::Previous),
            38..=43 => self.apply(Event::Seek {
                position_ms: rng.next() % 400_000,
            }),
            44..=45 => self.apply(Event::Stop),
            46 => self.apply(Event::Logout),
            47..=54 => {
                let kind = rng.below(5);
                let batch = tracks(rng, 3);
                let target = rng.below(8);
                let repeat = [Repeat::Off, Repeat::One, Repeat::All][rng.below(3)];
                let shuffle = rng.chance(50);
                let seed = rng.next();
                let fx = self.c.edit_queue(|q| match kind {
                    0 => {
                        q.play_next(batch);
                    }
                    1 => {
                        q.add_to_queue(batch);
                    }
                    2 => {
                        if let Some(id) = q.upcoming_ids().get(target).copied() {
                            q.remove(id);
                        }
                    }
                    3 => q.set_repeat(repeat),
                    _ => q.set_shuffle(shuffle, seed),
                });
                self.process(fx);
            }
            55..=77 if !self.loads.is_empty() => {
                let load = self.loads[rng.below(self.loads.len())];
                let result = if rng.chance(75) {
                    Ok(LeaseId(rng.next()))
                } else {
                    Err(failure(rng))
                };
                self.apply(Event::Resolved { load, result });
            }
            _ if !self.loads.is_empty() => {
                // Bias towards the active load so playback actually progresses.
                let load = match self.c.active_load() {
                    Some(a) if rng.chance(60) => a,
                    _ => self.loads[rng.below(self.loads.len())],
                };
                self.apply(Event::Engine {
                    load,
                    event: engine_event(rng),
                });
            }
            _ => self.apply(Event::Play),
        }
    }
}

#[test]
fn random_event_sequences_hold_invariants() {
    for seed in 0..3000u64 {
        let mut rng = Rng(seed);
        let mut h = Harness {
            c: Coordinator::default(),
            loads: Vec::new(),
            cancelled: HashSet::new(),
            engine_paused: None,
            seed,
            step: 0,
        };
        for step in 0..250 {
            h.step = step;
            h.random_event(&mut rng);
            h.check();
        }
    }
}
