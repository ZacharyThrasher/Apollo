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
use libmpv2::Mpv;

use apollo_domain::loudness::gain_db;
use apollo_services_common::LoudnessTable;

use crate::classify::{classify_end_file_error, classify_log};
use crate::coalesce::PositionCoalescer;
use crate::config::{core_options, MediaConfig};
use crate::ramp::GainRamp;

/// Headers a media request may carry. `UrlPolicy` already drops everything else; this is the
/// engine's own last line of defence, so a bug upstream cannot put Cookie/Authorization on a CDN.
const ALLOWED_HEADERS: &[&str] = &["user-agent", "accept", "accept-language", "referer", "origin"];

const MAX_LOG_LINES: usize = 16;
/// How long one seek may stay unanswered before the load is declared broken.
const SEEK_WATCHDOG: Duration = Duration::from_secs(15);
/// How long a change of normalization gain on a track that is already playing takes: the switch
/// flipped, or the track's loudness arriving after it started.
const GAIN_GLIDE: Duration = Duration::from_millis(400);

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
    SetVolume(u8),
    SetNormalization(bool),
    /// Test/diagnostic hook: current value of mpv options (names are fixed by the caller).
    Query(Vec<String>, Sender<Vec<(String, String)>>),
    /// Test/diagnostic hook: current value of mpv properties (live state, not options).
    QueryProperty(Vec<String>, Sender<Vec<(String, String)>>),
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

    /// Current values of mpv properties (live state such as `volume-gain`), for tests and
    /// diagnostics.
    pub fn property_values(&self, names: &[&str]) -> Vec<(String, String)> {
        let (rtx, rrx) = mpsc::channel();
        let names = names.iter().map(|s| s.to_string()).collect();
        if self.tx.send(Cmd::QueryProperty(names, rtx)).is_err() {
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
    fn set_volume(&mut self, percent: u8) {
        self.send(Cmd::SetVolume(percent.min(100)));
    }
    fn set_normalization(&mut self, on: bool) {
        self.send(Cmd::SetNormalization(on));
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
    EndFile { reason: u32, error: i32 },
    Other,
}

/// Reads the next event straight from the C API. libmpv2's `wait_event` turns an end-of-file
/// *error* into `Err(..)` and drops both the reason and the error code, so failed loads would be
/// invisible. `None` = timeout/no event.
fn next_event(mpv: &Mpv, timeout: f64) -> Option<OwnedEvent> {
    use libmpv2_sys as sys;
    unsafe {
        let ev = &*sys::mpv_wait_event(mpv.ctx.as_ptr(), timeout);
        let cstr = |p: *const std::os::raw::c_char| {
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        };
        Some(match ev.event_id {
            sys::mpv_event_id_MPV_EVENT_NONE => return None,
            sys::mpv_event_id_MPV_EVENT_LOG_MESSAGE => {
                let m = &*(ev.data as *const sys::mpv_event_log_message);
                OwnedEvent::Log {
                    level: cstr(m.level),
                    text: cstr(m.text),
                }
            }
            sys::mpv_event_id_MPV_EVENT_START_FILE => OwnedEvent::StartFile,
            sys::mpv_event_id_MPV_EVENT_FILE_LOADED => OwnedEvent::FileLoaded,
            sys::mpv_event_id_MPV_EVENT_SEEK => OwnedEvent::Seek,
            sys::mpv_event_id_MPV_EVENT_PLAYBACK_RESTART => OwnedEvent::PlaybackRestart,
            sys::mpv_event_id_MPV_EVENT_END_FILE => {
                let e = &*(ev.data as *const sys::mpv_event_end_file);
                OwnedEvent::EndFile {
                    reason: e.reason as u32,
                    error: e.error,
                }
            }
            _ => OwnedEvent::Other,
        })
    }
}

// ------------------------------------------------------------------- the pump

struct Active {
    load: LoadId,
    /// The video this load plays, for looking up its loudness.
    video_id: String,
    /// What YouTube measured for it, once known. Looked up again while it is `None`.
    loudness_db: Option<f32>,
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
    /// Volume normalization: the switch, the measurements and the level applied right now.
    normalize: bool,
    loudness: LoudnessTable,
    gain: GainRamp,
}

impl Pump {
    fn new(mpv: Mpv, cfg: MediaConfig, leases: LeaseStore, sink: EventSink) -> Self {
        let position = PositionCoalescer::new(cfg.position_interval);
        let loudness = cfg.loudness.clone();
        Self {
            mpv,
            cfg,
            leases,
            sink,
            active: None,
            position,
            normalize: true,
            loudness,
            gain: GainRamp::new(Instant::now()),
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
            // Short waits while the gain glides, so the steps stay small.
            let mut timeout = if self.gain.is_ramping() { 0.01 } else { 0.05 };
            while let Some(ev) = next_event(&self.mpv, timeout) {
                timeout = 0.0;
                self.on_mpv_event(ev);
            }
            self.update_gain();
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
            Cmd::SetVolume(percent) => {
                let _ = self.mpv.set_property("volume", i64::from(percent));
            }
            Cmd::SetNormalization(on) => {
                self.normalize = on;
                // The track that is playing follows the switch, smoothly.
                self.glide_to_desired_gain();
            }
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
            Cmd::QueryProperty(names, reply) => {
                let out = names
                    .into_iter()
                    .map(|n| {
                        let v = self
                            .mpv
                            .get_property::<String>(&n)
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
        let video_id = lease.video_id().as_str().to_owned();
        self.active = Some(Active {
            load,
            loudness_db: self.loudness.get(&video_id),
            video_id,
            started: false,
            loaded: false,
            paused: start_paused,
            seek: None,
            buffering: false,
            logs: VecDeque::new(),
            device_reported: false,
            ao_checked: false,
        });
        // This track's gain, in place before its first sample (the old track is being replaced).
        self.track_changed();
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

    // ----------------------------------------------------------------- gain

    /// The gain in dB the playing track should have right now: its normalization when the switch
    /// is on and its loudness is known, else unity.
    fn desired_gain_db(&self) -> f32 {
        match self.active.as_ref() {
            Some(a) if self.normalize => gain_db(a.loudness_db),
            _ => 0.0,
        }
    }

    /// The track playing changed (a load opened). Its gain is a property of the track, so it is
    /// looked up and applied here, at once, rather than carried over from the track before. A
    /// gapless advance calls this at the boundary.
    fn track_changed(&mut self) {
        let db = self.desired_gain_db();
        self.gain.jump(Instant::now(), db);
        self.apply_gain();
    }

    /// Start gliding to what the playing track should now have.
    fn glide_to_desired_gain(&mut self) {
        if self.active.is_some() {
            let db = self.desired_gain_db();
            self.gain.glide(Instant::now(), db, GAIN_GLIDE);
        }
    }

    /// Once per loop: pick up a loudness that arrived after the track started, then move the
    /// level one step along its glide.
    fn update_gain(&mut self) {
        let learned = match self.active.as_mut() {
            Some(a) if a.loudness_db.is_none() => {
                a.loudness_db = self.loudness.get(&a.video_id);
                a.loudness_db.is_some()
            }
            _ => false,
        };
        if learned {
            self.glide_to_desired_gain();
        }
        self.apply_gain();
    }

    /// Writes the level to mpv when it has moved. `volume-gain` is separate from `volume`, so
    /// the user's volume is untouched.
    fn apply_gain(&mut self) {
        if let Some(db) = self.gain.step(Instant::now()) {
            let _ = self.mpv.set_property("volume-gain", f64::from(db));
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
            OwnedEvent::EndFile { reason, error } => self.on_end_file(reason, error),
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

    fn on_end_file(&mut self, reason: u32, error: i32) {
        use libmpv2_sys::{
            mpv_end_file_reason_MPV_END_FILE_REASON_EOF as EOF,
            mpv_end_file_reason_MPV_END_FILE_REASON_ERROR as ERROR,
        };
        let Some(a) = self.active.as_ref() else { return };
        if !a.started {
            return; // end of the file this load replaced
        }
        let load = a.load;
        if reason == EOF as u32 {
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
        } else if reason == ERROR as u32 {
            let failure = classify_end_file_error(error, a.logs.iter().map(String::as_str));
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
