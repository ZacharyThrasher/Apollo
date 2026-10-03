use apollo_domain::model::{QueueEntryId, Track};
use apollo_domain::playback::{Coordinator, Effect, Event, LeaseId, LoadId};
use apollo_domain::queue::Queue;

#[derive(Clone, Debug, PartialEq)]
pub struct ResolveRequest {
    pub load: LoadId,
    pub entry: QueueEntryId,
    pub track: Track,
    pub resume_ms: u64,
    /// Speculative next-track resolution (never allowed to disturb playback).
    pub preload: bool,
}

/// Turns a track identity into a media lease. It never edits playlists,
/// history or the queue (BUILD_SPEC section 6).
pub trait Resolver {
    fn start(&mut self, request: ResolveRequest);
    fn cancel(&mut self, load: LoadId);
}

/// Decodes and outputs audio. It never obtains credentials or decides what plays next.
pub trait MediaEngine {
    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64);
    fn pause(&mut self, load: LoadId);
    fn resume(&mut self, load: LoadId);
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64);
    /// Drop a load (active or lookahead).
    fn close(&mut self, load: LoadId);
    /// Silence everything.
    fn stop(&mut self);
    /// Output level as a percentage (0 = silent, 100 = full). This is a user setting, not
    /// playback state, so it never goes through the coordinator. Engines without a
    /// volume control may ignore it.
    fn set_volume(&mut self, _percent: u8) {}
}

impl<T: Resolver + ?Sized> Resolver for Box<T> {
    fn start(&mut self, request: ResolveRequest) {
        (**self).start(request)
    }
    fn cancel(&mut self, load: LoadId) {
        (**self).cancel(load)
    }
}

impl<T: MediaEngine + ?Sized> MediaEngine for Box<T> {
    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64) {
        (**self).open(load, lease, start_paused, start_ms)
    }
    fn pause(&mut self, load: LoadId) {
        (**self).pause(load)
    }
    fn resume(&mut self, load: LoadId) {
        (**self).resume(load)
    }
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        (**self).seek(load, generation, position_ms)
    }
    fn close(&mut self, load: LoadId) {
        (**self).close(load)
    }
    fn stop(&mut self) {
        (**self).stop()
    }
    fn set_volume(&mut self, percent: u8) {
        (**self).set_volume(percent)
    }
}

/// Owns the coordinator and runs its effects. All inputs funnel through
/// [`Driver::handle`], so there is exactly one place where state changes.
pub struct Driver<R, M> {
    coordinator: Coordinator,
    pub resolver: R,
    pub engine: M,
}

impl<R: Resolver, M: MediaEngine> Driver<R, M> {
    pub fn new(resolver: R, engine: M) -> Self {
        Self {
            coordinator: Coordinator::default(),
            resolver,
            engine,
        }
    }

    pub fn coordinator(&self) -> &Coordinator {
        &self.coordinator
    }

    pub fn handle(&mut self, event: Event) {
        let effects = self.coordinator.handle(event);
        self.run(effects);
    }

    pub fn edit_queue(&mut self, f: impl FnOnce(&mut Queue)) {
        let effects = self.coordinator.edit_queue(f);
        self.run(effects);
    }

    fn run(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Resolve {
                    load,
                    entry,
                    track,
                    resume_ms,
                } => self.resolver.start(ResolveRequest {
                    load,
                    entry,
                    track,
                    resume_ms,
                    preload: false,
                }),
                Effect::ResolvePreload { load, entry, track } => {
                    self.resolver.start(ResolveRequest {
                        load,
                        entry,
                        track,
                        resume_ms: 0,
                        preload: true,
                    })
                }
                Effect::Cancel { load } => {
                    self.resolver.cancel(load);
                    self.engine.close(load);
                }
                Effect::Open {
                    load,
                    lease,
                    start_paused,
                    start_ms,
                } => self.engine.open(load, lease, start_paused, start_ms),
                Effect::Pause { load } => self.engine.pause(load),
                Effect::Resume { load } => self.engine.resume(load),
                Effect::Seek {
                    load,
                    gen,
                    position_ms,
                } => self.engine.seek(load, gen, position_ms),
                Effect::Stop => self.engine.stop(),
            }
        }
    }
}
