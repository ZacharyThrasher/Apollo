//! Pure playback coordinator (BUILD_SPEC PLAY-01, ARCH-03).
//!
//! A reducer: `handle(Event) -> Vec<Effect>`. It performs no I/O. The app layer
//! executes the effects (resolver, mpv, persistence) and feeds completions back
//! as events. Every asynchronous completion carries a [`LoadId`]; completions
//! whose load is no longer owned by the coordinator are ignored, so a late
//! result can never start sound or overwrite newer state.
//!
//! User *intent* (play/pause) is tracked separately from the engine *phase*.

use crate::model::{QueueEntryId, Track};
use crate::queue::{Advance, AdvanceKind, Previous, Queue};

/// Identifies one attempt to load a queue entry into the engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LoadId(u64);

impl LoadId {
    pub fn get(self) -> u64 {
        self.0
    }

    /// Adapter tests need ids without driving a coordinator. Production ids are minted only
    /// by the coordinator, so this is not available without the `test-support` feature.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests(raw: u64) -> Self {
        Self(raw)
    }
}

/// Opaque handle for a memory-only stream lease held by the services layer.
/// The domain never sees URLs, headers or cookies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LeaseId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    Playing,
    Paused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Resolving,
    Opening,
    Buffering,
    Playing,
    Paused,
    Seeking,
    Failed,
}

/// Coarse failure classes; recovery differs per class (NET-02).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    SessionExpired,
    Restricted,
    Throttled,
    Network,
    Unsupported,
    /// Authorization for the media URL likely expired; one re-resolution is allowed.
    LeaseExpired,
    Media,
    /// Output device lost. Never skips a track.
    Device,
}

/// Automatic progression skips at most this many consecutive failures.
pub const MAX_CONSECUTIVE_AUTO_FAILURES: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineEvent {
    Opened {
        duration_ms: Option<u64>,
    },
    Buffering(bool),
    Position(u64),
    Seeked {
        gen: u64,
        position_ms: u64,
    },
    /// End of media for this load.
    Eof,
    /// The engine itself moved on to the preloaded entry with no gap (it was handed the lease by
    /// [`Effect::Preload`]); the event's load is the *new* load. An `Opened` for that load, with
    /// its duration, follows.
    Transitioned,
    Error(Failure),
}

#[derive(Clone, Debug)]
pub enum Event {
    // --- user commands ---
    PlayContext {
        tracks: Vec<Track>,
        start: usize,
    },
    /// The user picked an entry in the queue.
    PlayEntry {
        id: QueueEntryId,
    },
    Play,
    Pause,
    Next,
    Previous,
    Seek {
        position_ms: u64,
    },
    Stop,
    Logout,
    /// The sound output went away or changed under the player: the default device changed or
    /// vanished (headphones unplugged, Bluetooth dropped), or the PC slept. Pauses in place at
    /// the same position, never skips and never touches the queue; the next Play opens the track
    /// afresh at that position so the audio comes out of whatever output exists then.
    OutputLost,
    // --- asynchronous completions ---
    Resolved {
        load: LoadId,
        result: Result<LeaseId, Failure>,
    },
    Engine {
        load: LoadId,
        event: EngineEvent,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Resolve {
        load: LoadId,
        entry: QueueEntryId,
        track: Track,
        resume_ms: u64,
    },
    ResolvePreload {
        load: LoadId,
        entry: QueueEntryId,
        track: Track,
    },
    /// The preloaded next track's lease is ready: hand it to the engine so it can start it the
    /// moment the current track ends, with no gap (gapless playback). The engine may decline, in
    /// which case the track is opened normally at the end of the current one. A later `Cancel` of
    /// the same load takes it back.
    Preload {
        load: LoadId,
        lease: LeaseId,
    },
    /// Cancel resolution and/or close the load in the engine.
    Cancel {
        load: LoadId,
    },
    Open {
        load: LoadId,
        lease: LeaseId,
        start_paused: bool,
        start_ms: u64,
    },
    Pause {
        load: LoadId,
    },
    Resume {
        load: LoadId,
    },
    Seek {
        load: LoadId,
        gen: u64,
        position_ms: u64,
    },
    Stop,
}

struct Active {
    load: LoadId,
    entry: QueueEntryId,
    /// Began from automatic progression (not a direct user request).
    auto: bool,
    /// What the engine was last told (via Open/Pause/Resume) about pausing.
    engine_paused: bool,
    reresolved: bool,
    pending_start_ms: u64,
    /// Adopted through a gapless transition: the engine's `Opened` for it, which carries the
    /// duration, is still to come.
    awaiting_duration: bool,
}

enum PreloadState {
    Pending,
    Ready(LeaseId),
}

struct Preload {
    load: LoadId,
    entry: QueueEntryId,
    state: PreloadState,
}

pub struct Coordinator {
    queue: Queue,
    intent: Intent,
    phase: Phase,
    active: Option<Active>,
    preload: Option<Preload>,
    preload_failed: Option<QueueEntryId>,
    position_ms: u64,
    duration_ms: Option<u64>,
    failures: u8,
    skipped: Vec<(QueueEntryId, Failure)>,
    error: Option<Failure>,
    /// The output was lost since the engine last opened a stream: the next Play opens the
    /// current track again (at the paused position) instead of resuming a stream whose output
    /// may be gone.
    reopen_on_play: bool,
    next_load: u64,
    seek_gen: u64,
}

impl Default for Coordinator {
    fn default() -> Self {
        Self::new(Queue::new())
    }
}

impl Coordinator {
    pub fn new(queue: Queue) -> Self {
        Self {
            queue,
            intent: Intent::Paused,
            phase: Phase::Idle,
            active: None,
            preload: None,
            preload_failed: None,
            position_ms: 0,
            duration_ms: None,
            failures: 0,
            skipped: Vec::new(),
            error: None,
            reopen_on_play: false,
            next_load: 1,
            seek_gen: 0,
        }
    }

    /// Rebuild after a restart: paused, nothing audible, and the first Play
    /// resolves a fresh stream and resumes near `position_ms`.
    pub fn restored(queue: Queue, position_ms: u64) -> Self {
        let mut c = Self::new(queue);
        c.position_ms = position_ms;
        c
    }

    // ---- read access -------------------------------------------------

    pub fn queue(&self) -> &Queue {
        &self.queue
    }
    pub fn intent(&self) -> Intent {
        self.intent
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn position_ms(&self) -> u64 {
        self.position_ms
    }
    pub fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }
    pub fn error(&self) -> Option<Failure> {
        self.error
    }
    pub fn skipped(&self) -> &[(QueueEntryId, Failure)] {
        &self.skipped
    }
    /// The load that currently owns audio output, if any.
    pub fn active_load(&self) -> Option<LoadId> {
        self.active.as_ref().map(|a| a.load)
    }
    /// The queue entry the active load belongs to.
    pub fn active_entry(&self) -> Option<QueueEntryId> {
        self.active.as_ref().map(|a| a.entry)
    }
    #[cfg(test)]
    pub(crate) fn repeat_is_one_with_preload(&self) -> bool {
        self.queue.repeat() == crate::queue::Repeat::One && self.preload.is_some()
    }
    pub fn preload_load(&self) -> Option<LoadId> {
        self.preload.as_ref().map(|p| p.load)
    }
    /// The next track's lease, if it is resolved and something is playing for it to follow.
    pub fn ready_preload(&self) -> Option<(LoadId, LeaseId)> {
        match &self.preload {
            Some(Preload {
                load,
                state: PreloadState::Ready(lease),
                ..
            }) if self.is_loaded() => Some((*load, *lease)),
            _ => None,
        }
    }

    // ---- queue edits --------------------------------------------------

    /// Apply a queue edit, then drop/replace any preload the edit invalidated.
    pub fn edit_queue(&mut self, f: impl FnOnce(&mut Queue)) -> Vec<Effect> {
        f(&mut self.queue);
        let mut fx = Vec::new();
        self.reconcile_preload(&mut fx);
        fx
    }

    /// Put `queue` in place of the live one (undo and redo). When it has the same entry current
    /// as the live queue, nothing audible changes: playback carries on and only what is up next
    /// differs. When it names another track (or none), what was playing is stopped and that
    /// track is left paused at `position_ms`, the way a restart leaves it: the next Play opens
    /// it there.
    pub fn replace_queue(&mut self, queue: Queue, position_ms: u64) -> Vec<Effect> {
        let same = queue.current().map(|e| e.id) == self.queue.current().map(|e| e.id);
        let mut fx = Vec::new();
        if same {
            self.queue = queue;
            self.reconcile_preload(&mut fx);
            return fx;
        }
        self.stop_all(&mut fx);
        self.queue = queue;
        self.skipped.clear();
        self.error = None;
        self.preload_failed = None;
        if self.queue.current().is_some() {
            self.position_ms = position_ms;
        }
        fx
    }

    // ---- reducer ------------------------------------------------------

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        let mut fx = Vec::new();
        match event {
            Event::PlayContext { tracks, start } => match self.queue.play_context(tracks, start) {
                Some(id) => self.user_start(id, &mut fx),
                None => self.stop_all(&mut fx),
            },
            Event::PlayEntry { id } => {
                if self.queue.jump_to(id) {
                    self.user_start(id, &mut fx);
                }
            }
            Event::Play => self.on_play(&mut fx),
            Event::Pause => self.on_pause(&mut fx),
            Event::Next => {
                if let Advance::Started(id) = self.queue.advance(AdvanceKind::Explicit) {
                    self.user_start(id, &mut fx);
                }
            }
            Event::Previous => match self.queue.previous(self.position_ms) {
                Previous::SeekToStart => self.on_seek(0, &mut fx),
                Previous::Started(id) => self.user_start(id, &mut fx),
            },
            Event::Seek { position_ms } => self.on_seek(position_ms, &mut fx),
            Event::Stop => self.stop_all(&mut fx),
            Event::OutputLost => self.on_output_lost(&mut fx),
            Event::Logout => {
                self.stop_all(&mut fx);
                self.queue = Queue::new();
                self.skipped.clear();
                self.error = None;
                self.preload_failed = None;
            }
            Event::Resolved { load, result } => self.on_resolved(load, result, &mut fx),
            Event::Engine { load, event } => self.on_engine(load, event, &mut fx),
        }
        fx
    }

    fn mint_load(&mut self) -> LoadId {
        let id = LoadId(self.next_load);
        self.next_load += 1;
        id
    }

    fn is_loaded(&self) -> bool {
        matches!(
            self.phase,
            Phase::Buffering | Phase::Playing | Phase::Paused | Phase::Seeking
        )
    }

    fn stop_all(&mut self, fx: &mut Vec<Effect>) {
        if let Some(a) = self.active.take() {
            fx.push(Effect::Cancel { load: a.load });
        }
        if let Some(p) = self.preload.take() {
            fx.push(Effect::Cancel { load: p.load });
        }
        fx.push(Effect::Stop);
        self.reopen_on_play = false;
        self.phase = Phase::Idle;
        self.intent = Intent::Paused;
        self.position_ms = 0;
        self.duration_ms = None;
    }

    /// A direct user navigation (play context / next / previous-to-entry):
    /// implies play, resets the automatic-failure budget and clears errors.
    fn user_start(&mut self, _id: QueueEntryId, fx: &mut Vec<Effect>) {
        self.intent = Intent::Playing;
        self.failures = 0;
        self.skipped.clear();
        self.start_load(false, 0, fx);
    }

    /// Begin loading the queue's current entry, invalidating any previous load.
    fn start_load(&mut self, auto: bool, resume_ms: u64, fx: &mut Vec<Effect>) {
        if let Some(a) = self.active.take() {
            fx.push(Effect::Cancel { load: a.load });
        }
        self.error = None;
        self.reopen_on_play = false;
        self.position_ms = resume_ms;
        self.duration_ms = None;
        let Some(entry) = self.queue.current() else {
            self.phase = Phase::Idle;
            return;
        };
        let (entry_id, track) = (entry.id, entry.track.clone());

        // Adopt a matching preload instead of starting a duplicate resolution.
        if let Some(p) = self.preload.take() {
            if p.entry == entry_id {
                let mut a = Active {
                    load: p.load,
                    entry: entry_id,
                    auto,
                    engine_paused: self.intent == Intent::Paused,
                    reresolved: false,
                    pending_start_ms: resume_ms,
                    awaiting_duration: false,
                };
                match p.state {
                    PreloadState::Ready(lease) => {
                        fx.push(Effect::Open {
                            load: p.load,
                            lease,
                            start_paused: a.engine_paused,
                            start_ms: a.pending_start_ms,
                        });
                        a.pending_start_ms = 0;
                        self.phase = Phase::Opening;
                    }
                    PreloadState::Pending => self.phase = Phase::Resolving,
                }
                self.active = Some(a);
                return;
            }
            fx.push(Effect::Cancel { load: p.load });
        }

        let load = self.mint_load();
        self.active = Some(Active {
            load,
            entry: entry_id,
            auto,
            engine_paused: false,
            reresolved: false,
            pending_start_ms: resume_ms,
            awaiting_duration: false,
        });
        self.phase = Phase::Resolving;
        fx.push(Effect::Resolve {
            load,
            entry: entry_id,
            track,
            resume_ms,
        });
    }

    fn on_play(&mut self, fx: &mut Vec<Effect>) {
        let was_paused = self.intent == Intent::Paused;
        self.intent = Intent::Playing;
        // The output was lost while this track was loaded: open it again where it stopped.
        if self.reopen_on_play && matches!(self.phase, Phase::Paused | Phase::Seeking) {
            self.failures = 0;
            self.skipped.clear();
            let resume = self.position_ms;
            self.start_load(false, resume, fx);
            return;
        }
        match self.phase {
            Phase::Paused => {
                if let Some(a) = &mut self.active {
                    fx.push(Effect::Resume { load: a.load });
                    a.engine_paused = false;
                    self.phase = Phase::Playing;
                }
            }
            // Pause was applied to the engine before the seek finished; undo it now,
            // otherwise Seeked would report Playing while the engine stays paused.
            Phase::Seeking if was_paused => {
                if let Some(a) = &mut self.active {
                    fx.push(Effect::Resume { load: a.load });
                    a.engine_paused = false;
                }
            }
            Phase::Idle | Phase::Failed if self.queue.current().is_some() => {
                self.failures = 0;
                self.skipped.clear();
                let resume = self.position_ms;
                self.start_load(false, resume, fx);
            }
            // Loading / playing / seeking: intent alone is enough.
            _ => {}
        }
    }

    fn on_pause(&mut self, fx: &mut Vec<Effect>) {
        self.intent = Intent::Paused;
        match self.phase {
            Phase::Playing | Phase::Buffering => {
                if let Some(a) = &mut self.active {
                    fx.push(Effect::Pause { load: a.load });
                    a.engine_paused = true;
                    self.phase = Phase::Paused;
                }
            }
            Phase::Seeking => {
                if let Some(a) = &mut self.active {
                    fx.push(Effect::Pause { load: a.load });
                    a.engine_paused = true;
                }
            }
            // Loading: intent is applied when the engine reports Opened.
            _ => {}
        }
    }

    /// Pause where the track is and remember to reopen it on the next Play. A load still in
    /// flight needs no reopening: it opens on whatever output exists by then.
    fn on_output_lost(&mut self, fx: &mut Vec<Effect>) {
        if self.is_loaded() {
            self.on_pause(fx);
            self.reopen_on_play = true;
        } else if matches!(self.phase, Phase::Resolving | Phase::Opening) {
            self.on_pause(fx);
        }
    }

    fn on_seek(&mut self, ms: u64, fx: &mut Vec<Effect>) {
        let ms = self.duration_ms.map_or(ms, |d| ms.min(d));
        let Some(a) = &mut self.active else {
            // Idle/failed (e.g. restored session): remember for the next load.
            self.position_ms = ms;
            return;
        };
        match self.phase {
            Phase::Resolving | Phase::Opening => {
                a.pending_start_ms = ms;
                self.position_ms = ms;
            }
            Phase::Buffering | Phase::Playing | Phase::Paused | Phase::Seeking => {
                self.seek_gen += 1;
                self.position_ms = ms;
                self.phase = Phase::Seeking;
                fx.push(Effect::Seek {
                    load: a.load,
                    gen: self.seek_gen,
                    position_ms: ms,
                });
            }
            Phase::Idle | Phase::Failed => self.position_ms = ms,
        }
    }

    fn on_resolved(
        &mut self,
        load: LoadId,
        result: Result<LeaseId, Failure>,
        fx: &mut Vec<Effect>,
    ) {
        // Active load awaiting its lease.
        if self.phase == Phase::Resolving {
            if let Some(a) = &mut self.active {
                if a.load == load {
                    match result {
                        Ok(lease) => {
                            a.engine_paused = self.intent == Intent::Paused;
                            fx.push(Effect::Open {
                                load,
                                lease,
                                start_paused: a.engine_paused,
                                start_ms: a.pending_start_ms,
                            });
                            a.pending_start_ms = 0;
                            self.phase = Phase::Opening;
                        }
                        Err(f) => self.fail_active(f, fx),
                    }
                    return;
                }
            }
        }
        // Speculative next-track resolution. Its failure never touches playback.
        let loaded = self.is_loaded();
        if let Some(p) = &mut self.preload {
            if p.load == load {
                match result {
                    Ok(lease) => {
                        p.state = PreloadState::Ready(lease);
                        // Something is playing to follow: let the engine line the track up.
                        if loaded {
                            fx.push(Effect::Preload { load, lease });
                        }
                    }
                    Err(_) => {
                        self.preload_failed = Some(p.entry);
                        self.preload = None;
                    }
                }
            }
        }
        // Anything else is stale: ignore.
    }

    fn on_engine(&mut self, load: LoadId, event: EngineEvent, fx: &mut Vec<Effect>) {
        // A transition notice names the *new* load.
        if event == EngineEvent::Transitioned {
            self.on_transitioned(load, fx);
            return;
        }
        let is_active = self.active.as_ref().is_some_and(|a| a.load == load);
        if !is_active {
            // Errors from the speculative load must not stop audible playback.
            if let (EngineEvent::Error(_), Some(p)) = (event, &self.preload) {
                if p.load == load {
                    self.preload_failed = Some(p.entry);
                    self.preload = None;
                    fx.push(Effect::Cancel { load });
                }
            }
            return; // stale
        }
        match event {
            EngineEvent::Opened { duration_ms } => self.on_opened(duration_ms, fx),
            EngineEvent::Buffering(on) => match (self.phase, on) {
                (Phase::Playing, true) => self.phase = Phase::Buffering,
                (Phase::Buffering, false) => self.phase = Phase::Playing,
                _ => {}
            },
            EngineEvent::Position(ms) => {
                if self.is_loaded() && self.phase != Phase::Seeking {
                    self.position_ms = ms;
                }
            }
            EngineEvent::Seeked { gen, position_ms } => {
                if self.phase == Phase::Seeking && gen == self.seek_gen {
                    self.position_ms = position_ms;
                    self.phase = if self.intent == Intent::Paused {
                        Phase::Paused
                    } else {
                        Phase::Playing
                    };
                }
            }
            EngineEvent::Eof => {
                if self.is_loaded() {
                    self.advance_automatically(fx);
                }
            }
            EngineEvent::Error(f) => self.on_engine_error(f, fx),
            EngineEvent::Transitioned => unreachable!("handled above"),
        }
    }

    fn on_opened(&mut self, duration_ms: Option<u64>, fx: &mut Vec<Effect>) {
        if self.phase != Phase::Opening {
            // A gapless transition adopted this load before its duration was known.
            if let Some(a) = &mut self.active {
                if std::mem::take(&mut a.awaiting_duration) {
                    self.duration_ms = duration_ms;
                }
            }
            return;
        }
        let Some(a) = &mut self.active else { return };
        self.duration_ms = duration_ms;
        self.failures = 0;
        let want_paused = self.intent == Intent::Paused;
        // A pause/play pressed while loading must win over what Open requested.
        if want_paused != a.engine_paused {
            fx.push(if want_paused {
                Effect::Pause { load: a.load }
            } else {
                Effect::Resume { load: a.load }
            });
            a.engine_paused = want_paused;
        }
        self.phase = if want_paused {
            Phase::Paused
        } else {
            Phase::Playing
        };
        let pending = std::mem::take(&mut a.pending_start_ms);
        if pending > 0 {
            self.on_seek(pending, fx);
        }
        self.maybe_preload(fx);
    }

    fn on_engine_error(&mut self, f: Failure, fx: &mut Vec<Effect>) {
        match f {
            Failure::Device => {
                // Pause conservatively; a device event is not an instruction to skip.
                self.intent = Intent::Paused;
                if let Some(a) = &mut self.active {
                    fx.push(Effect::Pause { load: a.load });
                    a.engine_paused = true;
                }
                if self.is_loaded() {
                    self.phase = Phase::Paused;
                    self.reopen_on_play = true;
                }
                self.error = Some(Failure::Device);
            }
            Failure::LeaseExpired => {
                let can_retry = self.active.as_ref().is_some_and(|a| !a.reresolved);
                if can_retry && (self.is_loaded() || self.phase == Phase::Opening) {
                    let (auto, pos) = (
                        self.active.as_ref().is_some_and(|a| a.auto),
                        self.position_ms,
                    );
                    self.start_load(auto, pos, fx);
                    if let Some(a) = &mut self.active {
                        a.reresolved = true; // exactly one bounded re-resolution
                    }
                } else {
                    self.fail_active(f, fx);
                }
            }
            other => self.fail_active(other, fx),
        }
    }

    fn on_transitioned(&mut self, load: LoadId, fx: &mut Vec<Effect>) {
        if self.active.as_ref().is_some_and(|a| a.load == load) {
            return; // already adopted via Eof; one logical transition only
        }
        let ready = matches!(
            &self.preload,
            Some(p) if p.load == load
                && matches!(p.state, PreloadState::Ready(_))
                && self.queue.peek_next().is_some_and(|e| e.id == p.entry)
        );
        if !ready {
            return;
        }
        let p = self.preload.take().expect("checked above");
        if let Some(old) = self.active.take() {
            // The engine already left it; just make sure nothing lingers.
            fx.push(Effect::Cancel { load: old.load });
        }
        if let Advance::Started(id) = self.queue.advance(AdvanceKind::Automatic) {
            debug_assert_eq!(id, p.entry);
            self.active = Some(Active {
                load: p.load,
                entry: id,
                auto: true,
                engine_paused: false,
                reresolved: false,
                pending_start_ms: 0,
                awaiting_duration: true,
            });
            self.position_ms = 0;
            self.duration_ms = None;
            self.failures = 0;
            // The engine moved on by itself and is playing; honor a pause the user
            // asked for in the meantime.
            if self.intent == Intent::Paused {
                fx.push(Effect::Pause { load: p.load });
                if let Some(a) = &mut self.active {
                    a.engine_paused = true;
                }
                self.phase = Phase::Paused;
            } else {
                self.phase = Phase::Playing;
            }
            self.maybe_preload(fx);
        } else {
            // Cannot happen when the preload matched `peek_next`; fail safe.
            self.phase = Phase::Idle;
            self.intent = Intent::Paused;
        }
    }

    fn advance_automatically(&mut self, fx: &mut Vec<Effect>) {
        match self.queue.advance(AdvanceKind::Automatic) {
            Advance::Started(_) | Advance::RepeatedCurrent(_) => self.start_load(true, 0, fx),
            Advance::EndOfQueue => {
                if let Some(a) = self.active.take() {
                    fx.push(Effect::Cancel { load: a.load });
                }
                if let Some(p) = self.preload.take() {
                    fx.push(Effect::Cancel { load: p.load });
                }
                self.phase = Phase::Idle;
                self.intent = Intent::Paused;
                self.position_ms = 0;
            }
        }
    }

    /// The active load failed. Direct requests stay selected with an error;
    /// automatic progression skips at most three consecutive failures.
    fn fail_active(&mut self, f: Failure, fx: &mut Vec<Effect>) {
        let Some(a) = self.active.take() else { return };
        fx.push(Effect::Cancel { load: a.load });
        if a.auto && self.failures < MAX_CONSECUTIVE_AUTO_FAILURES {
            self.failures += 1;
            self.skipped.push((a.entry, f));
            if let Advance::Started(_) = self.queue.advance(AdvanceKind::Explicit) {
                self.start_load(true, 0, fx);
                return;
            }
        }
        self.phase = Phase::Failed;
        self.error = Some(f);
    }

    // ---- preload ------------------------------------------------------

    fn maybe_preload(&mut self, fx: &mut Vec<Effect>) {
        if self.preload.is_some()
            || !self.is_loaded()
            || self.queue.repeat() == crate::queue::Repeat::One
        {
            return;
        }
        let Some(next) = self.queue.peek_next() else {
            return;
        };
        if self.preload_failed == Some(next.id) {
            return;
        }
        let (entry, track) = (next.id, next.track.clone());
        let load = self.mint_load();
        self.preload = Some(Preload {
            load,
            entry,
            state: PreloadState::Pending,
        });
        fx.push(Effect::ResolvePreload { load, entry, track });
    }

    fn reconcile_preload(&mut self, fx: &mut Vec<Effect>) {
        let next = self.queue.peek_next().map(|e| e.id);
        if let Some(p) = &self.preload {
            if Some(p.entry) != next || self.queue.repeat() == crate::queue::Repeat::One {
                fx.push(Effect::Cancel { load: p.load });
                self.preload = None;
            }
        }
        if self.preload_failed.is_some() && self.preload_failed != next {
            self.preload_failed = None;
        }
        self.maybe_preload(fx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::Repeat;

    fn tracks(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|i| Track::new(*i, *i)).collect()
    }
    fn resolve_load(fx: &[Effect]) -> LoadId {
        fx.iter()
            .find_map(|e| match e {
                Effect::Resolve { load, .. } => Some(*load),
                _ => None,
            })
            .expect("a Resolve effect")
    }
    fn preload_load(fx: &[Effect]) -> Option<LoadId> {
        fx.iter().find_map(|e| match e {
            Effect::ResolvePreload { load, .. } => Some(*load),
            _ => None,
        })
    }
    fn has_open(fx: &[Effect]) -> bool {
        fx.iter().any(|e| matches!(e, Effect::Open { .. }))
    }
    fn cur(c: &Coordinator) -> String {
        c.queue()
            .current()
            .unwrap()
            .track
            .video_id
            .as_str()
            .to_owned()
    }

    /// PlayContext -> Resolved -> Opened. Returns the active load and the
    /// effects emitted by Opened (which include the next-track preload).
    fn start_playing(c: &mut Coordinator, ids: &[&str]) -> (LoadId, Vec<Effect>) {
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(ids),
            start: 0,
        });
        let load = resolve_load(&fx);
        let fx = c.handle(Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        });
        assert!(has_open(&fx));
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Opened {
                duration_ms: Some(200_000),
            },
        });
        assert_eq!(c.phase(), Phase::Playing);
        (load, fx)
    }

    // TEST-05
    #[test]
    fn late_resolution_of_a_never_seizes_audio() {
        let mut c = Coordinator::default();
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(&["a", "b"]),
            start: 0,
        });
        let a = resolve_load(&fx);
        let fx = c.handle(Event::Next);
        let b = resolve_load(&fx);
        assert!(fx.contains(&Effect::Cancel { load: a }));
        assert_eq!(c.active_load(), Some(b));

        // A completes late: ignored, no Open.
        let fx = c.handle(Event::Resolved {
            load: a,
            result: Ok(LeaseId(9)),
        });
        assert!(fx.is_empty());
        assert_eq!(c.phase(), Phase::Resolving);
        assert_eq!(cur(&c), "b");

        let fx = c.handle(Event::Resolved {
            load: b,
            result: Ok(LeaseId(2)),
        });
        assert!(has_open(&fx));
    }

    #[test]
    fn replacing_the_queue_around_the_same_track_does_not_touch_playback() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b", "c"]);
        let mut q = Queue::restore(c.queue().snapshot()).unwrap();
        q.clear_upcoming();
        let fx = c.replace_queue(q, 0);
        assert!(!fx.contains(&Effect::Stop));
        assert!(!fx.contains(&Effect::Cancel { load }));
        assert_eq!(c.phase(), Phase::Playing);
        assert_eq!(c.active_load(), Some(load));
        assert_eq!(c.queue().upcoming().count(), 0);
    }

    #[test]
    fn replacing_the_queue_with_another_current_track_leaves_it_paused_at_its_position() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b"]);
        let mut other = Queue::new();
        other.play_context(tracks(&["x", "y"]), 0);
        let fx = c.replace_queue(other, 42_000);
        assert!(fx.contains(&Effect::Cancel { load }));
        assert!(fx.contains(&Effect::Stop));
        assert_eq!(c.intent(), Intent::Paused);
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(c.position_ms(), 42_000);
        assert_eq!(cur(&c), "x");
        // The next Play resolves it and opens it where it was left.
        let fx = c.handle(Event::Play);
        assert!(fx.iter().any(|e| matches!(
            e,
            Effect::Resolve { resume_ms: 42_000, .. }
        )));
    }

    #[test]
    fn picking_a_queue_entry_starts_it_and_cancels_the_old_load() {
        let mut c = Coordinator::default();
        let (old, _) = start_playing(&mut c, &["a", "b", "c", "d"]);
        let target = c.queue().context()[1].id; // "c"
        let fx = c.handle(Event::PlayEntry { id: target });
        assert!(fx.contains(&Effect::Cancel { load: old }));
        let new = resolve_load(&fx);
        assert_ne!(new, old);
        assert_eq!(cur(&c), "c");
        assert_eq!(c.intent(), Intent::Playing);
        assert_eq!(c.active_load(), Some(new));
    }

    #[test]
    fn picking_the_current_entry_or_an_unknown_one_changes_nothing() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b"]);
        let current = c.queue().current().unwrap().id;
        assert!(c.handle(Event::PlayEntry { id: current }).is_empty());
        assert!(c
            .handle(Event::PlayEntry {
                id: QueueEntryId(999)
            })
            .is_empty());
        assert_eq!(c.active_load(), Some(load));
        assert_eq!(cur(&c), "a");
    }

    // TEST-06
    #[test]
    fn twenty_rapid_nexts_leave_exactly_one_owner() {
        let ids: Vec<String> = (0..40).map(|i| format!("t{i}")).collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut c = Coordinator::default();
        let mut loads = vec![resolve_load(&c.handle(Event::PlayContext {
            tracks: tracks(&id_refs),
            start: 0,
        }))];
        for _ in 0..20 {
            let fx = c.handle(Event::Next);
            let prev = *loads.last().unwrap();
            assert_eq!(
                fx.iter()
                    .filter(|e| matches!(e, Effect::Resolve { .. }))
                    .count(),
                1
            );
            assert!(fx.contains(&Effect::Cancel { load: prev }));
            loads.push(resolve_load(&fx));
        }
        let latest = *loads.last().unwrap();
        assert_eq!(c.active_load(), Some(latest));
        assert_eq!(cur(&c), "t20");
        // Completions arrive in the worst order (newest of the stale first, then oldest).
        for &l in loads.iter().rev().skip(1) {
            let fx = c.handle(Event::Resolved {
                load: l,
                result: Ok(LeaseId(l.0)),
            });
            assert!(fx.is_empty(), "stale load {l:?} must be ignored");
        }
        let fx = c.handle(Event::Resolved {
            load: latest,
            result: Ok(LeaseId(1)),
        });
        assert!(has_open(&fx));
    }

    // TEST-10
    #[test]
    fn pause_during_load_is_not_overridden_by_opened() {
        let mut c = Coordinator::default();
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(&["a", "b"]),
            start: 0,
        });
        let load = resolve_load(&fx);
        c.handle(Event::Pause);
        let fx = c.handle(Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        });
        assert!(fx.contains(&Effect::Open {
            load,
            lease: LeaseId(1),
            start_paused: true,
            start_ms: 0
        }));
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Opened {
                duration_ms: Some(1000),
            },
        });
        assert_eq!(c.phase(), Phase::Paused);
        assert!(!fx.iter().any(|e| matches!(e, Effect::Resume { .. })));
    }

    #[test]
    fn pause_pressed_after_open_was_issued_is_corrected_on_opened() {
        let mut c = Coordinator::default();
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(&["a"]),
            start: 0,
        });
        let load = resolve_load(&fx);
        c.handle(Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        }); // Open{start_paused:false}
        c.handle(Event::Pause);
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Opened { duration_ms: None },
        });
        assert!(fx.contains(&Effect::Pause { load }));
        assert_eq!(c.phase(), Phase::Paused);
    }

    #[test]
    fn seek_while_paused_does_not_resume() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b"]);
        c.handle(Event::Pause);
        let fx = c.handle(Event::Seek {
            position_ms: 50_000,
        });
        let gen = match fx[0] {
            Effect::Seek {
                gen, position_ms, ..
            } => {
                assert_eq!(position_ms, 50_000);
                gen
            }
            ref other => panic!("expected Seek, got {other:?}"),
        };
        assert_eq!(c.phase(), Phase::Seeking);
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Seeked {
                gen,
                position_ms: 49_800,
            },
        });
        assert!(!fx.iter().any(|e| matches!(e, Effect::Resume { .. })));
        assert_eq!(c.phase(), Phase::Paused);
        assert_eq!(c.position_ms(), 49_800); // decoder-confirmed position wins
    }

    #[test]
    fn obsolete_seek_completion_is_ignored() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a"]);
        let g1 = match c.handle(Event::Seek {
            position_ms: 10_000,
        })[0]
        {
            Effect::Seek { gen, .. } => gen,
            _ => unreachable!(),
        };
        let g2 = match c.handle(Event::Seek {
            position_ms: 20_000,
        })[0]
        {
            Effect::Seek { gen, .. } => gen,
            _ => unreachable!(),
        };
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Seeked {
                gen: g1,
                position_ms: 10_000,
            },
        });
        assert_eq!(c.phase(), Phase::Seeking);
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Seeked {
                gen: g2,
                position_ms: 20_000,
            },
        });
        assert_eq!(c.phase(), Phase::Playing);
        assert_eq!(c.position_ms(), 20_000);
    }

    #[test]
    fn seek_during_load_is_applied_once_opened_and_clamped() {
        let mut c = Coordinator::default();
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(&["a"]),
            start: 0,
        });
        let load = resolve_load(&fx);
        c.handle(Event::Seek {
            position_ms: 30_000,
        });
        let fx = c.handle(Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        });
        assert!(fx.contains(&Effect::Open {
            load,
            lease: LeaseId(1),
            start_paused: false,
            start_ms: 30_000
        }));
        // Known duration clamps later seeks.
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Opened {
                duration_ms: Some(100_000),
            },
        });
        let fx = c.handle(Event::Seek {
            position_ms: 999_999,
        });
        assert!(matches!(
            fx[0],
            Effect::Seek {
                position_ms: 100_000,
                ..
            }
        ));
    }

    // TEST-11
    #[test]
    fn preload_error_keeps_active_playing_and_is_not_retried() {
        let mut c = Coordinator::default();
        let (active, fx) = start_playing(&mut c, &["a", "b", "c"]);
        let pre = preload_load(&fx).expect("next track preloaded once playing");
        let fx = c.handle(Event::Resolved {
            load: pre,
            result: Err(Failure::Network),
        });
        assert!(fx.is_empty());
        assert_eq!(c.phase(), Phase::Playing);
        assert_eq!(c.active_load(), Some(active));
        assert_eq!(c.preload_load(), None);
        // Same upcoming entry is not hammered again.
        let fx = c.edit_queue(|_| {});
        assert!(preload_load(&fx).is_none());
        // Same via an engine-side error on the speculative load.
        let mut c2 = Coordinator::default();
        let (active2, fx) = start_playing(&mut c2, &["a", "b"]);
        let pre2 = preload_load(&fx).unwrap();
        c2.handle(Event::Engine {
            load: pre2,
            event: EngineEvent::Error(Failure::Media),
        });
        assert_eq!(c2.phase(), Phase::Playing);
        assert_eq!(c2.active_load(), Some(active2));
    }

    #[test]
    fn queue_edit_invalidates_only_the_affected_preload() {
        let mut c = Coordinator::default();
        let (_, fx) = start_playing(&mut c, &["a", "b", "c"]);
        let pre = preload_load(&fx).unwrap();
        let fx = c.edit_queue(|q| {
            q.play_next(tracks(&["z"]));
        });
        assert!(fx.contains(&Effect::Cancel { load: pre }));
        let new_pre = preload_load(&fx).expect("fresh preload for the new next entry");
        assert_ne!(new_pre, pre);
        // Appending far down the queue leaves the preload alone.
        let fx = c.edit_queue(|q| {
            q.add_to_queue(tracks(&["far"]));
        });
        assert!(fx.is_empty());
    }

    // TEST-12
    fn preloaded_ready(c: &mut Coordinator) -> (LoadId, LoadId) {
        let (active, fx) = start_playing(c, &["a", "b", "c"]);
        let pre = preload_load(&fx).unwrap();
        let fx = c.handle(Event::Resolved {
            load: pre,
            result: Ok(LeaseId(2)),
        });
        assert_eq!(
            fx,
            vec![Effect::Preload {
                load: pre,
                lease: LeaseId(2)
            }]
        );
        (active, pre)
    }

    #[test]
    fn a_resolved_preload_is_offered_to_the_engine_only_while_something_plays() {
        let mut c = Coordinator::default();
        let (_, pre) = preloaded_ready(&mut c);
        assert_eq!(c.ready_preload(), Some((pre, LeaseId(2))));
        // Nothing playing (stopped): nothing is waiting to be handed over.
        c.handle(Event::Stop);
        assert_eq!(c.ready_preload(), None);
    }

    #[test]
    fn a_transition_takes_the_duration_from_the_opened_that_follows() {
        let mut c = Coordinator::default();
        let (_, pre) = preloaded_ready(&mut c);
        c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Transitioned,
        });
        assert_eq!(c.phase(), Phase::Playing);
        assert_eq!(c.duration_ms(), None);
        let fx = c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Opened {
                duration_ms: Some(187_000),
            },
        });
        assert_eq!(c.duration_ms(), Some(187_000));
        assert_eq!(c.phase(), Phase::Playing);
        assert!(fx.is_empty(), "nothing to tell the engine: it is already playing");
        // The same notice again changes nothing.
        c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Opened {
                duration_ms: Some(1),
            },
        });
        assert_eq!(c.duration_ms(), Some(187_000));
    }

    #[test]
    fn a_pause_pressed_before_the_boundary_still_wins_after_it() {
        let mut c = Coordinator::default();
        let (_, pre) = preloaded_ready(&mut c);
        // The engine moves on and plays; the user paused in the same instant.
        c.handle(Event::Pause);
        let fx = c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Transitioned,
        });
        assert!(fx.contains(&Effect::Pause { load: pre }));
        assert_eq!(c.phase(), Phase::Paused);
    }

    #[test]
    fn the_next_track_after_a_transition_is_preloaded_and_offered_in_turn() {
        let mut c = Coordinator::default();
        let (_, pre) = preloaded_ready(&mut c);
        let fx = c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Transitioned,
        });
        let next = preload_load(&fx).expect("the track after it is being resolved");
        let fx = c.handle(Event::Resolved {
            load: next,
            result: Ok(LeaseId(4)),
        });
        assert_eq!(
            fx,
            vec![Effect::Preload {
                load: next,
                lease: LeaseId(4)
            }]
        );
    }

    #[test]
    fn eof_then_transitioned_is_one_logical_transition() {
        let mut c = Coordinator::default();
        let (old, pre) = preloaded_ready(&mut c);
        let fx = c.handle(Event::Engine {
            load: old,
            event: EngineEvent::Eof,
        });
        // Preload adopted: opened with its existing lease, same load id.
        assert!(fx
            .iter()
            .any(|e| matches!(e, Effect::Open { load, lease: LeaseId(2), .. } if *load == pre)));
        c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Transitioned,
        });
        assert_eq!(c.queue().history().len(), 1);
        assert_eq!(cur(&c), "b");
    }

    #[test]
    fn transitioned_then_late_eof_is_one_logical_transition() {
        let mut c = Coordinator::default();
        let (old, pre) = preloaded_ready(&mut c);
        c.handle(Event::Engine {
            load: pre,
            event: EngineEvent::Transitioned,
        });
        assert_eq!(cur(&c), "b");
        assert_eq!(c.active_load(), Some(pre));
        let fx = c.handle(Event::Engine {
            load: old,
            event: EngineEvent::Eof,
        });
        assert!(
            fx.is_empty(),
            "late EOF of the old load must not skip again"
        );
        assert_eq!(cur(&c), "b");
        assert_eq!(c.queue().history().len(), 1);
    }

    // TEST-09 at coordinator level
    #[test]
    fn repeat_one_reloads_same_entry_on_eof() {
        let mut c = Coordinator::default();
        let (load, fx) = start_playing(&mut c, &["a", "b"]);
        assert!(preload_load(&fx).is_some());
        c.edit_queue(|q| q.set_repeat(Repeat::One));
        assert_eq!(
            c.preload_load(),
            None,
            "repeat-one needs no next-track preload"
        );
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Eof,
        });
        let again = resolve_load(&fx);
        assert_ne!(again, load);
        assert_eq!(cur(&c), "a");
        // Explicit Next still advances exactly once.
        c.handle(Event::Next);
        assert_eq!(cur(&c), "b");
    }

    #[test]
    fn eof_at_end_of_queue_goes_idle_paused() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a"]);
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Eof,
        });
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(c.intent(), Intent::Paused);
        assert_eq!(c.active_load(), None);
        // Play restarts the last entry.
        let fx = c.handle(Event::Play);
        assert!(fx.iter().any(|e| matches!(e, Effect::Resolve { .. })));
    }

    // Failure handling
    #[test]
    fn direct_request_failure_stays_selected_with_error() {
        let mut c = Coordinator::default();
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(&["a", "b"]),
            start: 0,
        });
        let load = resolve_load(&fx);
        c.handle(Event::Resolved {
            load,
            result: Err(Failure::Restricted),
        });
        assert_eq!(c.phase(), Phase::Failed);
        assert_eq!(c.error(), Some(Failure::Restricted));
        assert_eq!(cur(&c), "a", "never silently plays something else");
    }

    #[test]
    fn automatic_progression_skips_at_most_three_failures() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "f1", "f2", "f3", "f4", "ok"]);
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Eof,
        });
        let mut cur_load = c.active_load().unwrap();
        for expected in ["f1", "f2", "f3"] {
            assert_eq!(cur(&c), expected);
            c.handle(Event::Resolved {
                load: cur_load,
                result: Err(Failure::Restricted),
            });
            cur_load = c.active_load().unwrap();
        }
        assert_eq!(c.skipped().len(), 3);
        // Fourth consecutive failure: stop and surface, do not keep skipping.
        assert_eq!(cur(&c), "f4");
        let fx = c.handle(Event::Resolved {
            load: cur_load,
            result: Err(Failure::Restricted),
        });
        assert!(!fx.iter().any(|e| matches!(e, Effect::Resolve { .. })));
        assert_eq!(c.phase(), Phase::Failed);
        assert_eq!(cur(&c), "f4");
    }

    #[test]
    fn lease_expiry_is_re_resolved_once_preserving_position_and_intent() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b"]);
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Position(42_000),
        });
        c.handle(Event::Pause);
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Error(Failure::LeaseExpired),
        });
        let again = fx
            .iter()
            .find_map(|e| match e {
                Effect::Resolve {
                    load, resume_ms, ..
                } => Some((*load, *resume_ms)),
                _ => None,
            })
            .expect("one bounded re-resolution");
        assert_eq!(again.1, 42_000);
        assert_eq!(c.intent(), Intent::Paused);
        let fx = c.handle(Event::Resolved {
            load: again.0,
            result: Ok(LeaseId(5)),
        });
        assert!(fx.iter().any(|e| matches!(
            e,
            Effect::Open {
                start_paused: true,
                start_ms: 42_000,
                ..
            }
        )));
        c.handle(Event::Engine {
            load: again.0,
            event: EngineEvent::Opened { duration_ms: None },
        });
        // A second expiry on the same entry is surfaced, not looped.
        c.handle(Event::Engine {
            load: again.0,
            event: EngineEvent::Error(Failure::LeaseExpired),
        });
        assert_eq!(c.phase(), Phase::Failed);
        assert_eq!(c.error(), Some(Failure::LeaseExpired));
    }

    #[test]
    fn device_loss_pauses_and_never_skips() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b"]);
        let fx = c.handle(Event::Engine {
            load,
            event: EngineEvent::Error(Failure::Device),
        });
        assert!(fx.contains(&Effect::Pause { load }));
        assert_eq!(c.phase(), Phase::Paused);
        assert_eq!(c.intent(), Intent::Paused);
        assert_eq!(cur(&c), "a");
    }

    /// Plays "a" for a while, so there is a position worth keeping.
    fn playing_at(c: &mut Coordinator, ids: &[&str], at_ms: u64) -> LoadId {
        let (load, _) = start_playing(c, ids);
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Position(at_ms),
        });
        load
    }

    // TEST-20 (logic level)
    #[test]
    fn output_lost_pauses_in_place_and_keeps_the_queue() {
        let mut c = Coordinator::default();
        let load = playing_at(&mut c, &["a", "b", "c"], 73_000);
        let fx = c.handle(Event::OutputLost);
        assert!(fx.contains(&Effect::Pause { load }));
        assert_eq!(c.phase(), Phase::Paused);
        assert_eq!(c.intent(), Intent::Paused);
        assert_eq!(c.position_ms(), 73_000);
        assert_eq!(cur(&c), "a");
        assert_eq!(c.queue().upcoming_ids().len(), 2);
        // Not an error: nothing to show the user.
        assert_eq!(c.error(), None);
        // The engine's own late word about the lost device does not skip anything either.
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Error(Failure::Device),
        });
        assert_eq!(cur(&c), "a");
        assert_eq!(c.position_ms(), 73_000);
    }

    #[test]
    fn play_after_output_lost_opens_the_track_again_at_the_same_position() {
        let mut c = Coordinator::default();
        let load = playing_at(&mut c, &["a", "b"], 73_000);
        c.handle(Event::OutputLost);
        let fx = c.handle(Event::Play);
        // The old stream is dropped and a fresh one is resolved from where it stopped.
        assert!(fx.contains(&Effect::Cancel { load }));
        let (new_load, resume_ms) = fx
            .iter()
            .find_map(|e| match e {
                Effect::Resolve {
                    load, resume_ms, ..
                } => Some((*load, *resume_ms)),
                _ => None,
            })
            .expect("a fresh resolve");
        assert_ne!(new_load, load);
        assert_eq!(resume_ms, 73_000);
        assert_eq!(cur(&c), "a");
        let fx = c.handle(Event::Resolved {
            load: new_load,
            result: Ok(LeaseId(9)),
        });
        assert!(fx.iter().any(|e| matches!(
            e,
            Effect::Open {
                start_paused: false,
                start_ms: 73_000,
                ..
            }
        )));
        // And that is the only reopen: pausing and playing again just resumes.
        c.handle(Event::Engine {
            load: new_load,
            event: EngineEvent::Opened {
                duration_ms: Some(200_000),
            },
        });
        c.handle(Event::Pause);
        let fx = c.handle(Event::Play);
        assert_eq!(fx, vec![Effect::Resume { load: new_load }]);
    }

    #[test]
    fn output_lost_while_already_paused_still_reopens_on_play() {
        let mut c = Coordinator::default();
        let load = playing_at(&mut c, &["a"], 10_000);
        c.handle(Event::Pause);
        let fx = c.handle(Event::OutputLost);
        assert!(fx.is_empty(), "already paused: nothing to tell the engine");
        let fx = c.handle(Event::Play);
        assert!(fx.contains(&Effect::Cancel { load }));
        assert!(fx.iter().any(|e| matches!(e, Effect::Resolve { resume_ms: 10_000, .. })));
    }

    #[test]
    fn output_lost_with_nothing_loaded_does_nothing() {
        let mut c = Coordinator::default();
        assert!(c.handle(Event::OutputLost).is_empty());
        assert_eq!(c.phase(), Phase::Idle);
        // A load still in flight is paused by intent and opens on whatever output exists then.
        let fx = c.handle(Event::PlayContext {
            tracks: tracks(&["a"]),
            start: 0,
        });
        let load = resolve_load(&fx);
        c.handle(Event::OutputLost);
        assert_eq!(c.intent(), Intent::Paused);
        let fx = c.handle(Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        });
        assert!(fx
            .iter()
            .any(|e| matches!(e, Effect::Open { start_paused: true, .. })));
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Opened { duration_ms: None },
        });
        assert_eq!(c.phase(), Phase::Paused);
        // No stale reopen: Play just resumes.
        let fx = c.handle(Event::Play);
        assert_eq!(fx, vec![Effect::Resume { load }]);
    }

    #[test]
    fn a_lost_device_reported_by_the_engine_also_reopens_on_play() {
        let mut c = Coordinator::default();
        let load = playing_at(&mut c, &["a"], 5_000);
        c.handle(Event::Engine {
            load,
            event: EngineEvent::Error(Failure::Device),
        });
        assert_eq!(c.error(), Some(Failure::Device));
        let fx = c.handle(Event::Play);
        assert!(fx.iter().any(|e| matches!(e, Effect::Resolve { resume_ms: 5_000, .. })));
        assert_eq!(c.error(), None);
    }

    #[test]
    fn output_lost_during_a_seek_keeps_the_pause_and_reopens() {
        let mut c = Coordinator::default();
        let _ = playing_at(&mut c, &["a"], 5_000);
        c.handle(Event::Seek { position_ms: 90_000 });
        assert_eq!(c.phase(), Phase::Seeking);
        c.handle(Event::OutputLost);
        assert_eq!(c.intent(), Intent::Paused);
        let fx = c.handle(Event::Play);
        assert!(fx.iter().any(|e| matches!(e, Effect::Resolve { resume_ms: 90_000, .. })));
    }

    // TEST-04 (logic level)
    #[test]
    fn logout_stops_everything_and_pending_loads_cannot_start() {
        let mut c = Coordinator::default();
        let (active, fx) = start_playing(&mut c, &["a", "b"]);
        let pre = preload_load(&fx).unwrap();
        let fx = c.handle(Event::Logout);
        assert!(fx.contains(&Effect::Cancel { load: active }));
        assert!(fx.contains(&Effect::Cancel { load: pre }));
        assert!(fx.contains(&Effect::Stop));
        assert!(c.queue().current().is_none());
        assert_eq!(c.phase(), Phase::Idle);
        for l in [active, pre] {
            assert!(c
                .handle(Event::Resolved {
                    load: l,
                    result: Ok(LeaseId(1))
                })
                .is_empty());
        }
    }

    #[test]
    fn previous_past_threshold_seeks_to_start_within_threshold_goes_back() {
        let mut c = Coordinator::default();
        let (load, _) = start_playing(&mut c, &["a", "b"]);
        c.handle(Event::Next); // adopts the pending preload for "b"
        let b = c.active_load().unwrap();
        c.handle(Event::Resolved {
            load: b,
            result: Ok(LeaseId(2)),
        });
        c.handle(Event::Engine {
            load: b,
            event: EngineEvent::Opened {
                duration_ms: Some(1),
            },
        });
        let _ = load;
        c.handle(Event::Engine {
            load: b,
            event: EngineEvent::Position(10_000),
        });
        let fx = c.handle(Event::Previous);
        assert!(matches!(fx[0], Effect::Seek { position_ms: 0, .. }));
        assert_eq!(cur(&c), "b");
        // Seek completes, then position resets near the start: now Previous goes back.
        let gen = match fx[0] {
            Effect::Seek { gen, .. } => gen,
            _ => unreachable!(),
        };
        c.handle(Event::Engine {
            load: b,
            event: EngineEvent::Seeked {
                gen,
                position_ms: 0,
            },
        });
        let fx = c.handle(Event::Previous);
        assert!(fx.iter().any(|e| matches!(e, Effect::Resolve { .. })));
        assert_eq!(cur(&c), "a");
    }

    #[test]
    fn stale_engine_events_from_a_skipped_load_are_ignored() {
        let mut c = Coordinator::default();
        let (old, _) = start_playing(&mut c, &["a", "b"]);
        c.handle(Event::Next);
        for ev in [
            EngineEvent::Eof,
            EngineEvent::Position(5),
            EngineEvent::Error(Failure::Media),
            EngineEvent::Opened {
                duration_ms: Some(1),
            },
        ] {
            assert!(c
                .handle(Event::Engine {
                    load: old,
                    event: ev
                })
                .is_empty());
        }
        assert_eq!(cur(&c), "b");
        assert_eq!(c.phase(), Phase::Resolving);
    }

    // TEST-22 (logic part)
    #[test]
    fn restored_session_is_paused_and_first_play_resolves_fresh_near_saved_position() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        let mut c = Coordinator::restored(Queue::restore(q.snapshot()).unwrap(), 73_000);
        assert_eq!(c.phase(), Phase::Idle);
        assert_eq!(c.intent(), Intent::Paused);
        assert_eq!(c.active_load(), None, "nothing starts by itself on launch");
        let fx = c.handle(Event::Play);
        let (load, resume) = fx
            .iter()
            .find_map(|e| match e {
                Effect::Resolve {
                    load, resume_ms, ..
                } => Some((*load, *resume_ms)),
                _ => None,
            })
            .expect("fresh resolution on first Play");
        assert_eq!(resume, 73_000);
        let fx = c.handle(Event::Resolved {
            load,
            result: Ok(LeaseId(1)),
        });
        assert!(fx.iter().any(|e| matches!(
            e,
            Effect::Open {
                start_ms: 73_000,
                start_paused: false,
                ..
            }
        )));
    }
}
