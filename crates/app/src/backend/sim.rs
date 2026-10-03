//! Simulated backend: fixture tracks, a resolver that "resolves" after a short delay and an
//! engine that keeps a clock. Both follow the port contracts (completions come back as
//! coordinator events carrying the load id and seek generation they were given), so the UI is
//! exercised against the real coordinator without a network or audio device.

use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use apollo_domain::model::{AlbumRef, ArtistRef, Track};
use apollo_domain::playback::{EngineEvent, Event, LeaseId, LoadId};
use apollo_runtime::{MediaEngine, ResolveRequest, Resolver};
use apollo_services_common::EventSink;

use super::{LibraryTrack, Ports, Prepared};
use crate::art::{ArtSource, DecodedArt, SyntheticArt};

const RESOLVE_DELAY: Duration = Duration::from_millis(250);
const OPEN_DELAY: Duration = Duration::from_millis(150);
const SEEK_DELAY: Duration = Duration::from_millis(80);
const POSITION_INTERVAL: Duration = Duration::from_millis(250);
const TICK: Duration = Duration::from_millis(40);
const UNKNOWN_DURATION_MS: u64 = 180_000;

/// One fixture: the track and how its artwork looks.
pub struct Fixture {
    pub id: &'static str,
    pub title: &'static str,
    pub artist: &'static str,
    pub album: &'static str,
    pub secs: u64,
    pub art: Option<SyntheticArt>,
}

/// Covers the shapes the layout has to survive: long text, grayscale art, no art, a track
/// short enough to watch it end.
pub const FIXTURES: &[Fixture] = &[
    Fixture {
        id: "simTrack001",
        title: "Midnight Drive",
        artist: "Neon Arcade",
        album: "Night Shift",
        secs: 192,
        art: Some(SyntheticArt::Hue(35.0)),
    },
    Fixture {
        id: "simTrack002",
        title: "Glass Garden",
        artist: "Soft Lantern",
        album: "Greenhouse",
        secs: 168,
        art: Some(SyntheticArt::Hue(215.0)),
    },
    Fixture {
        id: "simTrack003",
        title: "A Very Long Track Title That Keeps Going Well Past The Edge Of Its Container",
        artist: "An Artist With A Similarly Long Name, Featuring Several Other Collaborators",
        album: "Long Names",
        secs: 245,
        art: Some(SyntheticArt::Hue(320.0)),
    },
    Fixture {
        id: "simTrack004",
        title: "Ash",
        artist: "Monochrome",
        album: "Greys",
        secs: 150,
        art: Some(SyntheticArt::Gray),
    },
    Fixture {
        id: "simTrack005",
        title: "No Cover",
        artist: "Unknown",
        album: "Singles",
        secs: 220,
        art: None,
    },
    Fixture {
        id: "simTrack006",
        title: "Fern",
        artist: "Verdant",
        album: "Moss",
        secs: 25,
        art: Some(SyntheticArt::Hue(140.0)),
    },
];

fn track_of(f: &Fixture) -> Track {
    let mut t = Track::new(f.id, f.title);
    t.artists = vec![ArtistRef {
        name: f.artist.into(),
        id: None,
    }];
    t.album = Some(AlbumRef {
        title: f.album.into(),
        id: None,
    });
    t.duration = Some(Duration::from_secs(f.secs));
    t
}

pub struct SimArt;

impl ArtSource for SimArt {
    fn fetch(&self, video_id: &str) -> Option<DecodedArt> {
        // Library tracks beyond the fixtures get a hue from their position.
        let generated = video_id
            .strip_prefix(crate::library::sim::GENERATED_PREFIX)
            .and_then(|n| n.parse::<usize>().ok())
            .map(|n| SyntheticArt::Hue((n % 12) as f32 * 30.0));
        let kind = match FIXTURES.iter().find(|f| f.id == video_id) {
            Some(f) => f.art?,
            None => generated?,
        };
        Some(DecodedArt::from_rgba(kind.render(256)))
    }
}

pub fn prepare(sink: EventSink) -> Prepared {
    let (resolver, engine) = pair(sink);
    Prepared {
        label: "simulated",
        ports: Ports {
            resolver: Box::new(resolver),
            engine: Box::new(engine),
        },
        queue: FIXTURES
            .iter()
            .map(|f| LibraryTrack { track: track_of(f) })
            .collect(),
        art: Arc::new(SimArt),
        library: Arc::new(crate::library::sim::SimLibrary::new()),
    }
}

/// Resolver and engine sharing the lease table (a lease carries the track length).
pub fn pair(sink: EventSink) -> (SimResolver, SimEngine) {
    let leases: Leases = Arc::default();
    (
        SimResolver::new(sink.clone(), leases.clone()),
        SimEngine::new(sink, leases),
    )
}

type Leases = Arc<Mutex<HashMap<u64, u64>>>;

// -------------------------------------------------------------------------- resolver

pub struct SimResolver {
    sink: EventSink,
    leases: Leases,
    next_lease: u64,
}

impl SimResolver {
    fn new(sink: EventSink, leases: Leases) -> Self {
        Self {
            sink,
            leases,
            next_lease: 0,
        }
    }
}

impl Resolver for SimResolver {
    fn start(&mut self, request: ResolveRequest) {
        self.next_lease += 1;
        let lease = LeaseId(self.next_lease);
        let ms = request
            .track
            .duration
            .map_or(UNKNOWN_DURATION_MS, |d| d.as_millis() as u64);
        self.leases.lock().unwrap().insert(lease.0, ms);
        let (sink, load) = (self.sink.clone(), request.load);
        // A completion for a load the coordinator has since dropped is ignored by the
        // coordinator, which is the behaviour a real resolver relies on too.
        thread::spawn(move || {
            thread::sleep(RESOLVE_DELAY);
            sink(Event::Resolved {
                load,
                result: Ok(lease),
            });
        });
    }

    fn cancel(&mut self, _load: LoadId) {}
}

// ---------------------------------------------------------------------------- engine

enum Cmd {
    Open {
        load: LoadId,
        lease: LeaseId,
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
    Shutdown,
}

pub struct SimEngine {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
    volume: Arc<Mutex<u8>>,
}

impl SimEngine {
    fn new(sink: EventSink, leases: Leases) -> Self {
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("apollo-sim-engine".into())
            .spawn(move || {
                let mut clock = Clock {
                    sink,
                    leases,
                    active: None,
                };
                loop {
                    match rx.recv_timeout(TICK) {
                        Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                        Ok(cmd) => clock.apply(cmd, Instant::now()),
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    clock.tick(Instant::now());
                }
            })
            .expect("spawn sim engine thread");
        Self {
            tx,
            thread: Some(thread),
            volume: Arc::default(),
        }
    }

    /// Last level set through the port (the simulation is silent, so this is all it does).
    pub fn volume(&self) -> u8 {
        *self.volume.lock().unwrap()
    }
}

impl MediaEngine for SimEngine {
    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64) {
        let _ = self.tx.send(Cmd::Open {
            load,
            lease,
            start_paused,
            start_ms,
        });
    }
    fn pause(&mut self, load: LoadId) {
        let _ = self.tx.send(Cmd::Pause(load));
    }
    fn resume(&mut self, load: LoadId) {
        let _ = self.tx.send(Cmd::Resume(load));
    }
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        let _ = self.tx.send(Cmd::Seek {
            load,
            generation,
            position_ms,
        });
    }
    fn close(&mut self, load: LoadId) {
        let _ = self.tx.send(Cmd::Close(load));
    }
    fn stop(&mut self) {
        let _ = self.tx.send(Cmd::Stop);
    }
    fn set_volume(&mut self, percent: u8) {
        *self.volume.lock().unwrap() = percent;
    }
}

impl Drop for SimEngine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Active {
    load: LoadId,
    duration_ms: u64,
    position_ms: f64,
    paused: bool,
    opened: bool,
    open_at: Instant,
    seek: Option<(u64, u64, Instant)>,
    last_tick: Instant,
    last_report: Instant,
}

struct Clock {
    sink: EventSink,
    leases: Leases,
    active: Option<Active>,
}

impl Clock {
    fn emit(&self, load: LoadId, event: EngineEvent) {
        (self.sink)(Event::Engine { load, event });
    }

    fn apply(&mut self, cmd: Cmd, now: Instant) {
        match cmd {
            Cmd::Open {
                load,
                lease,
                start_paused,
                start_ms,
            } => {
                let duration_ms = self
                    .leases
                    .lock()
                    .unwrap()
                    .get(&lease.0)
                    .copied()
                    .unwrap_or(UNKNOWN_DURATION_MS);
                self.active = Some(Active {
                    load,
                    duration_ms,
                    position_ms: start_ms as f64,
                    paused: start_paused,
                    opened: false,
                    open_at: now + OPEN_DELAY,
                    seek: None,
                    last_tick: now,
                    last_report: now,
                });
            }
            Cmd::Pause(load) => self.with(load, |a| a.paused = true),
            Cmd::Resume(load) => self.with(load, |a| {
                a.paused = false;
                a.last_tick = now;
            }),
            Cmd::Seek {
                load,
                generation,
                position_ms,
            } => self.with(load, |a| {
                if a.opened {
                    a.seek = Some((generation, position_ms.min(a.duration_ms), now + SEEK_DELAY));
                }
            }),
            Cmd::Close(load) => {
                if self.active.as_ref().is_some_and(|a| a.load == load) {
                    self.active = None;
                }
            }
            Cmd::Stop => self.active = None,
            Cmd::Shutdown => unreachable!("handled by the loop"),
        }
    }

    fn with(&mut self, load: LoadId, f: impl FnOnce(&mut Active)) {
        if let Some(a) = self.active.as_mut().filter(|a| a.load == load) {
            f(a);
        }
    }

    fn tick(&mut self, now: Instant) {
        let Some(a) = self.active.as_mut() else {
            return;
        };
        let load = a.load;
        let mut events = Vec::new();
        if !a.opened {
            if now < a.open_at {
                return;
            }
            a.opened = true;
            a.last_tick = now;
            a.last_report = now;
            events.push(EngineEvent::Opened {
                duration_ms: Some(a.duration_ms),
            });
        } else if let Some((generation, to, at)) = a.seek {
            if now >= at {
                a.seek = None;
                a.position_ms = to as f64;
                a.last_tick = now;
                a.last_report = now;
                events.push(EngineEvent::Seeked {
                    gen: generation,
                    position_ms: to,
                });
            }
        } else if !a.paused {
            a.position_ms += now.saturating_duration_since(a.last_tick).as_secs_f64() * 1000.0;
            a.last_tick = now;
            if a.position_ms >= a.duration_ms as f64 {
                events.push(EngineEvent::Eof);
                self.active = None;
            } else if now.saturating_duration_since(a.last_report) >= POSITION_INTERVAL {
                a.last_report = now;
                events.push(EngineEvent::Position(a.position_ms as u64));
            }
        } else {
            a.last_tick = now;
        }
        for event in events {
            self.emit(load, event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use apollo_domain::playback::{Intent, Phase};
    use apollo_runtime::Driver;

    use super::*;
    use crate::backend::channel_sink;

    type D = Driver<SimResolver, SimEngine>;

    fn pump(
        d: &mut D,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
        max: Duration,
        until: impl Fn(&D) -> bool,
    ) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < max {
            if until(d) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
            while let Ok(ev) = rx.try_recv() {
                d.handle(ev);
            }
        }
        until(d)
    }

    fn driver() -> (D, tokio::sync::mpsc::UnboundedReceiver<Event>) {
        let (sink, rx) = channel_sink();
        let (r, e) = pair(sink);
        let mut d = Driver::new(r, e);
        let tracks = FIXTURES.iter().map(track_of).collect();
        d.edit_queue(|q| {
            q.play_context(tracks, 0);
        });
        (d, rx)
    }

    #[test]
    fn play_resolves_opens_and_advances_the_clock() {
        let (mut d, mut rx) = driver();
        d.handle(Event::Play);
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| d
            .coordinator()
            .phase()
            == Phase::Playing));
        assert_eq!(d.coordinator().duration_ms(), Some(192_000));
        let p0 = d.coordinator().position_ms();
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| d
            .coordinator()
            .position_ms()
            > p0 + 500));
    }

    #[test]
    fn pause_holds_position_and_resume_continues() {
        let (mut d, mut rx) = driver();
        d.handle(Event::Play);
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| d
            .coordinator()
            .position_ms()
            > 300));
        d.handle(Event::Pause);
        pump(&mut d, &mut rx, Duration::from_millis(400), |_| false);
        assert_eq!(d.coordinator().phase(), Phase::Paused);
        let held = d.coordinator().position_ms();
        pump(&mut d, &mut rx, Duration::from_millis(800), |_| false);
        assert!(d.coordinator().position_ms().abs_diff(held) < 300);
        d.handle(Event::Play);
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| d
            .coordinator()
            .position_ms()
            > held + 400));
        assert_eq!(d.coordinator().intent(), Intent::Playing);
    }

    #[test]
    fn seek_lands_on_the_requested_position() {
        let (mut d, mut rx) = driver();
        d.handle(Event::Play);
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| d
            .coordinator()
            .phase()
            == Phase::Playing));
        d.handle(Event::Seek {
            position_ms: 100_000,
        });
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| {
            d.coordinator().phase() == Phase::Playing
                && d.coordinator().position_ms().abs_diff(100_000) < 1_000
        }));
    }

    #[test]
    fn next_switches_track_and_end_of_track_advances() {
        let (mut d, mut rx) = driver();
        d.handle(Event::Play);
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| d
            .coordinator()
            .phase()
            == Phase::Playing));
        let first = d.coordinator().queue().current().unwrap().id;
        d.handle(Event::Next);
        assert!(pump(&mut d, &mut rx, Duration::from_secs(3), |d| {
            d.coordinator().phase() == Phase::Playing
                && d.coordinator().queue().current().unwrap().id != first
        }));
        // Seek close to the end of the current track: the engine reports Eof and the
        // coordinator moves on by itself.
        let second = d.coordinator().queue().current().unwrap().id;
        let dur = d.coordinator().duration_ms().unwrap();
        d.handle(Event::Seek {
            position_ms: dur - 600,
        });
        assert!(pump(&mut d, &mut rx, Duration::from_secs(5), |d| {
            d.coordinator().phase() == Phase::Playing
                && d.coordinator().queue().current().unwrap().id != second
        }));
    }
}
