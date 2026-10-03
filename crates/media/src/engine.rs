//! The libmpv engine (Windows). One dedicated thread owns the mpv handle: it applies commands,
//! pumps events and polls position. Nothing here blocks the caller or touches the UI.

use std::collections::VecDeque;
use std::ffi::{c_void, CString};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use apollo_domain::playback::{EngineEvent, Event, Failure, LeaseId, LoadId};
use apollo_runtime::MediaEngine;
use apollo_services_common::{unix_now, EventSink, LeaseLookup, LeaseStore};
use libmpv2::events::Event as MpvEvent;
use libmpv2::{mpv_end_file_reason, Mpv};

use crate::classify::{classify_failed_load, classify_log};
use crate::coalesce::PositionCoalescer;
use crate::config::{core_options, MediaConfig};

/// Headers a media request may carry. `UrlPolicy` already drops everything else; this is the
/// engine's own last line of defence, so a bug upstream cannot put Cookie/Authorization on a CDN.
const ALLOWED_HEADERS: &[&str] = &["user-agent", "accept", "accept-language", "referer", "origin"];

const MAX_LOG_LINES: usize = 16;
/// How long one seek may stay unanswered before the load is declared broken.
const SEEK_WATCHDOG: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum MediaError {
    /// libmpv could not be created/initialised (missing DLL options, bad option value, ...).
    Init(String),
    ThreadSpawn(String),
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MediaError::Init(e) => write!(f, "media engine init failed: {e}"),
            MediaError::ThreadSpawn(e) => write!(f, "media thread failed: {e}"),
        }
    }
}

impl std::error::Error for MediaError {}

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
    /// Test/diagnostic hook: current value of mpv options (names are fixed by the caller).
    Query(Vec<String>, Sender<Vec<(String, String)>>),
    Shutdown,
}

pub struct MpvEngine {
    tx: Sender<Cmd>,
    thread: Option<JoinHandle<()>>,
}

impl MpvEngine {
    /// Creates the libmpv instance (on the calling thread, so option errors surface here) and
    /// starts the pump thread. `leases` is the store the resolver fills; `sink` receives events.
    pub fn new(cfg: MediaConfig, leases: LeaseStore, sink: EventSink) -> Result<Self, MediaError> {
        let options = core_options(&cfg);
        let mpv = Mpv::with_initializer(|init| {
            for (name, value) in &options {
                init.set_property(name, value.as_str())?;
            }
            Ok(())
        })
        .map_err(|e| MediaError::Init(format!("{e:?}")))?;
        // Diagnostics (never surfaced; only classified) for failed loads and device trouble.
        let level = CString::new("warn").unwrap();
        unsafe {
            libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), level.as_ptr());
        }
        let (tx, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("apollo-media".into())
            .spawn(move || Pump::new(mpv, cfg, leases, sink).run(rx))
            .map_err(|e| MediaError::ThreadSpawn(e.to_string()))?;
        Ok(Self {
            tx,
            thread: Some(thread),
        })
    }

    /// Current values of mpv options, for tests and diagnostics (never contains URLs).
    pub fn option_values(&self, names: &[&str]) -> Vec<(String, String)> {
        let (rtx, rrx) = mpsc::channel();
        let names = names.iter().map(|s| s.to_string()).collect();
        if self.tx.send(Cmd::Query(names, rtx)).is_err() {
            return Vec::new();
        }
        rrx.recv_timeout(Duration::from_secs(5)).unwrap_or_default()
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

impl MediaEngine for MpvEngine {
    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64) {
        self.send(Cmd::Open {
            load,
            lease,
            start_paused,
            start_ms,
        });
    }
    fn pause(&mut self, load: LoadId) {
        self.send(Cmd::Pause(load));
    }
    fn resume(&mut self, load: LoadId) {
        self.send(Cmd::Resume(load));
    }
    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        self.send(Cmd::Seek {
            load,
            generation,
            position_ms,
        });
    }
    fn close(&mut self, load: LoadId) {
        self.send(Cmd::Close(load));
    }
    fn stop(&mut self) {
        self.send(Cmd::Stop);
    }
}

impl Drop for MpvEngine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

enum OwnedEvent {
    Log { level: String, text: String },
    StartFile,
    FileLoaded,
    Seek,
    PlaybackRestart,
    EndFile(libmpv2::EndFileReason),
    Other,
}

impl From<MpvEvent<'_>> for OwnedEvent {
    fn from(ev: MpvEvent<'_>) -> Self {
        match ev {
            MpvEvent::LogMessage { level, text, .. } => OwnedEvent::Log {
                level: level.to_string(),
                text: text.to_string(),
            },
            MpvEvent::StartFile => OwnedEvent::StartFile,
            MpvEvent::FileLoaded => OwnedEvent::FileLoaded,
            MpvEvent::Seek => OwnedEvent::Seek,
            MpvEvent::PlaybackRestart => OwnedEvent::PlaybackRestart,
            MpvEvent::EndFile(r) => OwnedEvent::EndFile(r),
            _ => OwnedEvent::Other,
        }
    }
}

// ------------------------------------------------------------------- the pump

struct Active {
    load: LoadId,
    /// mpv announced the file for this load. Events before this belong to a replaced file.
    started: bool,
    /// FileLoaded seen and `Opened` sent.
    loaded: bool,
    paused: bool,
    /// An engine seek in flight: generation, whether mpv has begun it, when it was issued.
    seek: Option<PendingSeek>,
    buffering: bool,
    logs: VecDeque<String>,
    device_reported: bool,
    ao_checked: bool,
}

struct PendingSeek {
    generation: u64,
    started: bool,
    issued: Instant,
}

struct Pump {
    mpv: Mpv,
    cfg: MediaConfig,
    leases: LeaseStore,
    sink: EventSink,
    active: Option<Active>,
    position: PositionCoalescer,
}

impl Pump {
    fn new(mpv: Mpv, cfg: MediaConfig, leases: LeaseStore, sink: EventSink) -> Self {
        let position = PositionCoalescer::new(cfg.position_interval);
        Self {
            mpv,
            cfg,
            leases,
            sink,
            active: None,
            position,
        }
    }

    fn emit(&self, load: LoadId, event: EngineEvent) {
        (self.sink)(Event::Engine { load, event });
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            // Commands first so a Close/Stop is never starved by a busy event queue.
            loop {
                match rx.recv_timeout(Duration::ZERO) {
                    Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                        self.stop_playback();
                        return;
                    }
                    Ok(cmd) => self.apply(cmd),
                    Err(RecvTimeoutError::Timeout) => break,
                }
            }
            // Drain every queued mpv event, then wait briefly for the next.
            let mut timeout = 0.05;
            while let Some(ev) = self.mpv.wait_event(timeout) {
                timeout = 0.0;
                // Own the event first: it borrows from the mpv handle we are about to use.
                let owned = match ev {
                    Ok(ev) => OwnedEvent::from(ev),
                    Err(_) => continue,
                };
                self.on_mpv_event(owned);
            }
            self.poll();
        }
    }

    // ------------------------------------------------------------- commands

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open {
                load,
                lease,
                start_paused,
                start_ms,
            } => self.open(load, lease, start_paused, start_ms),
            Cmd::Pause(load) => self.set_pause(load, true),
            Cmd::Resume(load) => self.set_pause(load, false),
            Cmd::Seek {
                load,
                generation,
                position_ms,
            } => self.seek(load, generation, position_ms),
            Cmd::Close(load) => {
                if self.active.as_ref().is_some_and(|a| a.load == load) {
                    self.stop_playback();
                }
            }
            Cmd::Stop => self.stop_playback(),
            Cmd::Query(names, reply) => {
                let out = names
                    .into_iter()
                    .map(|n| {
                        let v = self
                            .mpv
                            .get_property::<String>(&format!("options/{n}"))
                            .unwrap_or_else(|e| format!("<{e:?}>"));
                        (n, v)
                    })
                    .collect();
                let _ = reply.send(out);
            }
            Cmd::Shutdown => unreachable!("handled in run"),
        }
    }

    /// Silence everything and forget the active load; events still in flight are ignored.
    fn stop_playback(&mut self) {
        self.active = None;
        let _ = self.mpv.command("stop", &[]);
        self.clear_request_headers();
    }

    fn open(&mut self, load: LoadId, lease: LeaseId, start_paused: bool, start_ms: u64) {
        let lease = match self.leases.get_fresh(
            lease,
            unix_now(),
            self.cfg.lease_margin,
            self.cfg.max_unknown_lease_age,
        ) {
            Ok(l) => l,
            Err(LeaseLookup::Missing | LeaseLookup::Stale) => {
                // Never touch the network with a lease we already know is unusable.
                self.stop_playback();
                self.emit(load, EngineEvent::Error(Failure::LeaseExpired));
                return;
            }
        };
        // Replace whatever is playing. Old-file events are ignored until StartFile of this load.
        self.position.reset();
        self.active = Some(Active {
            load,
            started: false,
            loaded: false,
            paused: start_paused,
            seek: None,
            buffering: false,
            logs: VecDeque::new(),
            device_reported: false,
            ao_checked: false,
        });
        if let Err(e) = self.set_request_headers(lease.headers_for(lease.url())) {
            self.active = None;
            let _ = e;
            self.emit(load, EngineEvent::Error(Failure::Media));
            return;
        }
        let mut file_opts = vec![format!("pause={}", if start_paused { "yes" } else { "no" })];
        if start_ms > 0 {
            file_opts.push(format!("start={:.3}", start_ms as f64 / 1000.0));
        }
        let opts = file_opts.join(",");
        // The URL goes through the API, never a command line or a log line.
        if self
            .mpv
            .command("loadfile", &[lease.url().as_str(), "replace", "-1", &opts])
            .is_err()
        {
            self.active = None;
            self.emit(load, EngineEvent::Error(Failure::Media));
        }
    }

    fn set_pause(&mut self, load: LoadId, pause: bool) {
        let Some(a) = self.active.as_mut().filter(|a| a.load == load) else {
            return; // stale command for a load that is no longer ours
        };
        a.paused = pause;
        let _ = self.mpv.set_property("pause", pause);
        if pause {
            // Nothing advances while paused; send no further positions until resumed.
            self.position.reset();
        }
    }

    fn seek(&mut self, load: LoadId, generation: u64, position_ms: u64) {
        let Some(a) = self.active.as_mut().filter(|a| a.load == load) else {
            return;
        };
        if !a.loaded {
            return; // the coordinator defers seeks until Opened; ignore anything earlier
        }
        let secs = format!("{:.3}", position_ms as f64 / 1000.0);
        match self.mpv.command("seek", &[&secs, "absolute+exact"]) {
            Ok(()) => {
                a.seek = Some(PendingSeek {
                    generation,
                    started: false,
                    issued: Instant::now(),
                });
            }
            Err(_) => {
                a.seek = None;
                self.emit(load, EngineEvent::Error(Failure::Media));
            }
        }
    }

    // -------------------------------------------------------------- headers

    /// Sets the per-request headers for the *next* open. mpv accepts one global header list, so
    /// every request of this load (including a redirect target) would receive it: see the
    /// redirect test and docs/m2-media-notes.md. Only the allow-listed, non-secret headers can
    /// ever be here; Cookie/Authorization never are.
    fn set_request_headers(&self, headers: &[(String, String)]) -> Result<(), ()> {
        let mut fields: Vec<String> = Vec::new();
        let mut ua: Option<String> = None;
        for (name, value) in headers {
            let lname = name.to_ascii_lowercase();
            if !ALLOWED_HEADERS.contains(&lname.as_str())
                || value.contains(['\r', '\n'])
                || name.contains(['\r', '\n', ':'])
            {
                continue;
            }
            if lname == "user-agent" {
                ua = Some(value.clone());
            } else {
                fields.push(format!("{name}: {value}"));
            }
        }
        let _ = self.mpv.set_property("user-agent", ua.unwrap_or_default().as_str());
        set_string_list(&self.mpv, "http-header-fields", &fields)
    }

    fn clear_request_headers(&self) {
        let _ = set_string_list(&self.mpv, "http-header-fields", &[]);
        let _ = self.mpv.set_property("user-agent", "");
    }

    // --------------------------------------------------------------- events

    fn on_mpv_event(&mut self, ev: OwnedEvent) {
        match ev {
            OwnedEvent::Log { level, text } => {
                if let Some(a) = self.active.as_mut() {
                    if level == "error" || level == "warn" || level == "fatal" {
                        if a.logs.len() == MAX_LOG_LINES {
                            a.logs.pop_front();
                        }
                        a.logs.push_back(text.clone());
                        // Output-device trouble while playing is reported immediately and once.
                        if a.loaded
                            && !a.device_reported
                            && classify_log(&text) == Some(Failure::Device)
                        {
                            a.device_reported = true;
                            let load = a.load;
                            self.emit(load, EngineEvent::Error(Failure::Device));
                        }
                    }
                }
            }
            OwnedEvent::StartFile => {
                if let Some(a) = self.active.as_mut() {
                    a.started = true;
                }
            }
            OwnedEvent::FileLoaded => {
                let Some(a) = self.active.as_mut().filter(|a| a.started && !a.loaded) else {
                    return;
                };
                a.loaded = true;
                let load = a.load;
                let duration_ms = self
                    .mpv
                    .get_property::<f64>("duration")
                    .ok()
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .map(|d| (d * 1000.0).round() as u64);
                self.emit(load, EngineEvent::Opened { duration_ms });
            }
            OwnedEvent::Seek => {
                if let Some(s) = self.active.as_mut().and_then(|a| a.seek.as_mut()) {
                    s.started = true;
                }
            }
            OwnedEvent::PlaybackRestart => self.on_playback_restart(),
            OwnedEvent::EndFile(reason) => self.on_end_file(reason),
            OwnedEvent::Other => {}
        }
    }

    fn on_playback_restart(&mut self) {
        let Some(a) = self.active.as_mut().filter(|a| a.started && a.loaded) else {
            return;
        };
        let load = a.load;
        // First restart after load: audio must actually have an output. mpv will happily play
        // "silently" if no audio output could be opened, which must not look like Playing.
        if !a.ao_checked {
            a.ao_checked = true;
            let ao = self.mpv.get_property::<String>("current-ao").unwrap_or_default();
            if ao.is_empty() && !a.device_reported {
                a.device_reported = true;
                self.emit(load, EngineEvent::Error(Failure::Device));
                return;
            }
        }
        let a = self.active.as_mut().expect("still active");
        if let Some(s) = a.seek.as_ref().filter(|s| s.started) {
            let generation = s.generation;
            a.seek = None;
            let pos = self.position_ms().unwrap_or(0);
            self.position.reset();
            self.emit(
                load,
                EngineEvent::Seeked {
                    gen: generation,
                    position_ms: pos,
                },
            );
        }
    }

    fn on_end_file(&mut self, reason: libmpv2::EndFileReason) {
        let Some(a) = self.active.as_ref() else { return };
        if !a.started {
            return; // end of the file this load replaced
        }
        let load = a.load;
        if reason == mpv_end_file_reason::Eof {
            let loaded = a.loaded;
            self.active = None;
            self.clear_request_headers();
            self.emit(
                load,
                if loaded {
                    EngineEvent::Eof
                } else {
                    EngineEvent::Error(Failure::Media)
                },
            );
        } else if reason == mpv_end_file_reason::Error {
            let failure = classify_failed_load(a.logs.iter().map(String::as_str));
            self.active = None;
            self.clear_request_headers();
            self.emit(load, EngineEvent::Error(failure));
        }
        // STOP/QUIT/REDIRECT: we asked for it (close, stop, replace); say nothing.
    }

    // ----------------------------------------------------------------- poll

    fn position_ms(&self) -> Option<u64> {
        self.mpv
            .get_property::<f64>("time-pos")
            .ok()
            .filter(|p| p.is_finite() && *p >= 0.0)
            .map(|p| (p * 1000.0).round() as u64)
    }

    fn poll(&mut self) {
        let Some(a) = self.active.as_mut() else { return };
        if !a.loaded {
            return;
        }
        let load = a.load;
        if let Some(s) = &a.seek {
            if s.issued.elapsed() > SEEK_WATCHDOG {
                a.seek = None;
                self.emit(load, EngineEvent::Error(Failure::Media));
                return;
            }
            return; // positions are meaningless mid-seek
        }
        // Buffering transitions.
        let cache_paused = self
            .mpv
            .get_property::<bool>("paused-for-cache")
            .unwrap_or(false);
        let a = self.active.as_mut().expect("active");
        if cache_paused != a.buffering {
            a.buffering = cache_paused;
            self.emit(load, EngineEvent::Buffering(cache_paused));
        }
        // Position, about 4 Hz, only while time is moving.
        let a = self.active.as_ref().expect("active");
        if a.paused {
            return;
        }
        if let Some(ms) = self.position_ms() {
            if let Some(ms) = self.position.offer(Instant::now(), ms) {
                self.emit(load, EngineEvent::Position(ms));
            }
        }
    }
}

/// Sets a string-list option from a real list (no comma/escape ambiguity).
fn set_string_list(mpv: &Mpv, name: &str, items: &[String]) -> Result<(), ()> {
    use libmpv2_sys::{mpv_format_MPV_FORMAT_NODE, mpv_format_MPV_FORMAT_NODE_ARRAY, mpv_format_MPV_FORMAT_STRING, mpv_node, mpv_node__bindgen_ty_1, mpv_node_list};
    let cstrings: Vec<CString> = items
        .iter()
        .map(|s| CString::new(s.as_str()).map_err(|_| ()))
        .collect::<Result<_, _>>()?;
    let mut nodes: Vec<mpv_node> = cstrings
        .iter()
        .map(|c| mpv_node {
            u: mpv_node__bindgen_ty_1 {
                string: c.as_ptr() as *mut _,
            },
            format: mpv_format_MPV_FORMAT_STRING,
        })
        .collect();
    let mut list = mpv_node_list {
        num: nodes.len() as i32,
        values: nodes.as_mut_ptr(),
        keys: std::ptr::null_mut(),
    };
    let mut root = mpv_node {
        u: mpv_node__bindgen_ty_1 { list: &mut list },
        format: mpv_format_MPV_FORMAT_NODE_ARRAY,
    };
    let cname = CString::new(name).map_err(|_| ())?;
    let rc = unsafe {
        libmpv2_sys::mpv_set_property(
            mpv.ctx.as_ptr(),
            cname.as_ptr(),
            mpv_format_MPV_FORMAT_NODE,
            &mut root as *mut _ as *mut c_void,
        )
    };
    if rc >= 0 {
        Ok(())
    } else {
        Err(())
    }
}
