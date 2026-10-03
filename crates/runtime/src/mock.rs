//! Deterministic mock adapters. Completions happen only when a test says so, in
//! whatever order the test chooses, which is how races are reproduced.

use std::collections::HashSet;

use apollo_domain::playback::{EngineEvent, Event, Failure, LeaseId, LoadId};

use crate::ports::{MediaEngine, ResolveRequest, Resolver};

#[derive(Default)]
pub struct MockResolver {
    pub pending: Vec<ResolveRequest>,
    pub cancelled: Vec<LoadId>,
    pub started: Vec<ResolveRequest>,
    next_lease: u64,
}

impl MockResolver {
    /// The load ids of requests that have not completed or been cancelled.
    pub fn pending_loads(&self) -> Vec<LoadId> {
        self.pending.iter().map(|r| r.load).collect()
    }

    /// Complete `load` successfully. Works for cancelled or unknown loads too,
    /// which models a result that was already in flight when it was cancelled.
    pub fn ok(&mut self, load: LoadId) -> Event {
        self.pending.retain(|r| r.load != load);
        self.next_lease += 1;
        Event::Resolved {
            load,
            result: Ok(LeaseId(self.next_lease)),
        }
    }

    pub fn err(&mut self, load: LoadId, failure: Failure) -> Event {
        self.pending.retain(|r| r.load != load);
        Event::Resolved {
            load,
            result: Err(failure),
        }
    }
}

impl Resolver for MockResolver {
    fn start(&mut self, request: ResolveRequest) {
        self.started.push(request.clone());
        self.pending.push(request);
    }

    fn cancel(&mut self, load: LoadId) {
        self.pending.retain(|r| r.load != load);
        self.cancelled.push(load);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineCall {
    Open {
        load: LoadId,
        start_paused: bool,
        start_ms: u64,
    },
    Pause(LoadId),
    Resume(LoadId),
    Seek {
        load: LoadId,
        generation: u64,
        position_ms: u64,
    },
    Close(LoadId),
    Stop,
    SetVolume(u8),
    SetNormalization(bool),
}

/// Models what the audio device is doing.
#[derive(Default)]
pub struct MockEngine {
    pub calls: Vec<EngineCall>,
    /// Load currently holding the output, if any.
    audible: Option<LoadId>,
    paused: bool,
    closed: HashSet<u64>,
}

impl MockEngine {
    pub fn audible_load(&self) -> Option<LoadId> {
        self.audible
    }
    /// True when sound is actually coming out (a load is open and not paused).
    pub fn is_sounding(&self) -> bool {
        self.audible.is_some() && !self.paused
    }
    pub fn opens(&self) -> Vec<LoadId> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                EngineCall::Open { load, .. } => Some(*load),
                _ => None,
            })
            .collect()
    }
    pub fn event(&self, load: LoadId, event: EngineEvent) -> Event {
        Event::Engine { load, event }
    }
}

impl MediaEngine for MockEngine {
    fn open(&mut self, load: LoadId, _lease: LeaseId, start_paused: bool, start_ms: u64) {
        self.calls.push(EngineCall::Open {
            load,
            start_paused,
            start_ms,
        });
        self.audible = Some(load);
        self.paused = start_paused;
    }
    fn pause(&mut self, load: LoadId) {
        self.calls.push(EngineCall::Pause(load));
        if self.audible == Some(load) {
            self.paused = true;
        }
    }
    fn resume(&mut self, load: LoadId) {
        self.calls.push(EngineCall::Resume(load));
        if self.audible == Some(load) {
            self.paused = false;
        }
    }
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        self.calls.push(EngineCall::Seek {
            load,
            generation,
            position_ms,
        });
    }
    fn close(&mut self, load: LoadId) {
        self.calls.push(EngineCall::Close(load));
        self.closed.insert(load.get());
        if self.audible == Some(load) {
            self.audible = None;
        }
    }
    fn stop(&mut self) {
        self.calls.push(EngineCall::Stop);
        self.audible = None;
        self.paused = false;
    }
    fn set_volume(&mut self, percent: u8) {
        self.calls.push(EngineCall::SetVolume(percent));
    }
    fn set_normalization(&mut self, on: bool) {
        self.calls.push(EngineCall::SetNormalization(on));
    }
}
