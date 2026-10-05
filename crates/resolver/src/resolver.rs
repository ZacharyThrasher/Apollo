//! `YtDlpResolver`: implements `apollo_runtime::Resolver` with a contained yt-dlp helper.
//!
//! Per request, on its own thread:
//!   1. wait for a concurrency slot (cancellable);
//!   2. ask the account service for a session snapshot. **No session = no process is spawned**
//!      (AUTH-02: yt-dlp resolves happily with no or fake cookies, so resolver success proves
//!      nothing about the account; the gate is the account's own validation);
//!   3. write the snapshot to a private, user-only, short-lived cookie file;
//!   4. run yt-dlp in a Job Object (timeout, cancel, bounded pipes), then delete the file;
//!   5. parse its JSON and validate it with `UrlPolicy` before a lease exists;
//!   6. publish `Event::Resolved` unless the load was cancelled meanwhile.
//! Nothing here prints or stores cookies, URLs, titles or ids; diagnostics are outcome codes.

use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::Path;
#[cfg(windows)]
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use apollo_domain::model::{Track, VideoId};
use apollo_domain::playback::{Event, Failure, LeaseId, LoadId};
use apollo_runtime::{ResolveRequest, Resolver};
#[cfg(windows)]
use apollo_services_common::win::{self, PrivateDir};
use apollo_services_common::{
    EventSink, Lane, LeaseStore, Pacer, SecretString, SessionProvider, SessionSnapshot,
    SessionUnavailable, Throttle,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

#[cfg(windows)]
use crate::argv::build_args;
use crate::cache::LeaseCache;
use crate::config::ResolverConfig;
use crate::parse::{
    classify_failure, lease_error_code, lease_error_failure, parse_ytdlp_json,
};
#[cfg(windows)]
use crate::runner::{run_contained, KillSwitch, RunStatus};
use crate::warm::{self, Acquire, Pool, WorkerError, WorkerSpawn};

#[cfg(windows)]
const COOKIE_FILE: &str = "cookies.txt";
const SCRATCH_MAX_AGE: Duration = Duration::from_secs(10 * 60);
const MAX_DIAG: usize = 200;
const MAX_LOG_BYTES: u64 = 256 * 1024;

#[derive(Debug)]
pub enum ResolverError {
    /// The yt-dlp or Deno executable is missing, unreadable, or does not match its pin.
    Tool(String),
    Io(String),
}

impl fmt::Display for ResolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolverError::Tool(e) => write!(f, "resolver tool check failed: {e}"),
            ResolverError::Io(e) => write!(f, "resolver setup failed: {e}"),
        }
    }
}

impl std::error::Error for ResolverError {}

fn sha256_hex(path: &Path) -> std::io::Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Cancellation for one resolution. On Windows the cold path's helper process is killed with it.
#[derive(Default)]
struct Job {
    #[cfg(windows)]
    switch: KillSwitch,
    #[cfg(not(windows))]
    flag: AtomicBool,
}

impl Job {
    fn cancel(&self) {
        #[cfg(windows)]
        self.switch.cancel();
        #[cfg(not(windows))]
        self.flag.store(true, Ordering::SeqCst);
    }

    fn is_cancelled(&self) -> bool {
        #[cfg(windows)]
        return self.switch.is_cancelled();
        #[cfg(not(windows))]
        return self.flag.load(Ordering::SeqCst);
    }
}

/// Speculative requests (prefetch) are numbered above any real load id so the same job table can
/// hold them: `cancel_all` (logout, quit) must stop them too.
const PREFETCH_KEY_BASE: u64 = 1 << 62;
/// At most this many prefetch resolutions at once; more are dropped (hover spam, long queues).
/// Each one is about six YouTube requests, so this is what keeps a burst of hints from becoming
/// a burst of traffic.
const MAX_PREFETCH_PENDING: usize = 2;
/// Prefetches allowed back to back, then one per [`PREFETCH_EVERY`] (10 a minute). Playback
/// itself (a click, the next track) is never counted against this.
const PREFETCH_BURST: u32 = 3;
const PREFETCH_EVERY: Duration = Duration::from_secs(6);
/// A skipped prefetch is written to the log at most this often (with how many were skipped).
const DROP_NOTE_EVERY: Duration = Duration::from_secs(30);
/// Prefetched ids remembered, to tell a track that is being resolved again from a new one.
const RECENT_PREFETCH_MAX: usize = 512;
const RECENT_PREFETCH_AGE: Duration = Duration::from_secs(60 * 60);

/// What one resolution produced, shareable between the callers waiting for it.
#[derive(Clone, Copy)]
enum Produced {
    Ok(LeaseId),
    Failed(Failure, &'static str),
}

/// One resolution in progress for a video id. A second caller (a click on a track that is being
/// prefetched) waits for it instead of starting another.
struct Flight {
    /// `None` = running. `Some(None)` = abandoned by its leader before it started (retry).
    done: Mutex<Option<Option<Produced>>>,
    cv: Condvar,
    /// Someone is actually waiting to play this: workers should not treat it as speculative.
    foreground: AtomicBool,
}

/// Facts about one resolution for the diagnostics line. Static labels and numbers only.
#[derive(Default)]
struct Note {
    src: &'static str,
    worker_ms: Option<u64>,
    phases: Vec<(String, u64)>,
    /// Which YouTube client served the stream (the `c=` parameter of the URL) and the format id.
    client: Option<String>,
    /// The chosen format's id and audio bitrate (kbps), to see what quality a setting yields.
    format: Option<String>,
    /// yt-dlp met a 429 on the way, even if another client then got the track.
    throttled: bool,
}

impl Note {
    fn suffix(&self) -> String {
        let mut s = format!(" src={}", if self.src.is_empty() { "cold" } else { self.src });
        if let Some(ms) = self.worker_ms {
            s.push_str(&format!(" worker_ms={ms}"));
        }
        if let Some(c) = &self.client {
            s.push_str(&format!(" client={c}"));
        }
        if let Some(f) = &self.format {
            s.push_str(&format!(" {f}"));
        }
        if !self.phases.is_empty() {
            let p: Vec<String> = self
                .phases
                .iter()
                .map(|(l, ms)| format!("{}@{ms}", l.replace(' ', "_")))
                .collect();
            s.push_str(&format!(" phases={}", p.join(",")));
        }
        s
    }
}

enum Outcome {
    Ok(LeaseId),
    Failed(Failure, &'static str),
    Cancelled,
}

struct Inner {
    cfg: ResolverConfig,
    leases: LeaseStore,
    sink: EventSink,
    session: Arc<dyn SessionProvider>,
    jobs: Mutex<HashMap<u64, Arc<Job>>>,
    #[cfg_attr(not(windows), allow(dead_code))]
    slots: (Mutex<usize>, Condvar),
    diag: Mutex<VecDeque<String>>,
    closed: AtomicBool,
    pool: Option<Arc<Pool>>,
    cache: Option<LeaseCache>,
    flights: Mutex<HashMap<String, Arc<Flight>>>,
    prefetch_pending: AtomicUsize,
    prefetch_seq: AtomicU64,
    prefetch_pacer: Pacer,
    /// Prefetches skipped since the last time that was written to the log, and when it was.
    dropped: Mutex<(u64, Option<Instant>)>,
    /// When each id was last prefetched here (bounded), to log `repeat=true` for a track that is
    /// resolved again: the sign of a cache that does not hold or a trigger that does not stop.
    recent_prefetch: Mutex<HashMap<String, Instant>>,
    /// Shared with the other YouTube callers (see `apollo_services_common::throttle`).
    throttle: Mutex<Throttle>,
    log_file: Mutex<Option<std::path::PathBuf>>,
}

/// A cheap handle for things that outlive the resolver's owner (logout hooks).
#[derive(Clone)]
pub struct ResolverHandle {
    inner: Arc<Inner>,
}

impl ResolverHandle {
    /// Logout / identity change: cancel everything in flight, forget every cached URL and drop
    /// every lease. Warm workers stay (they hold no account data once their next request
    /// reloads the cookies), but their cookie state is marked stale.
    pub fn invalidate(&self) {
        self.inner.invalidate();
    }

    /// Write a line (outcome codes and numbers only) to the resolver's log.
    pub fn note(&self, line: String) {
        self.inner.note(line);
    }
}

pub struct YtDlpResolver {
    inner: Arc<Inner>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl YtDlpResolver {
    /// Verifies the pinned tools, prepares the run directories and sweeps leftovers of
    /// processes that died without cleaning up (stale private cookie dirs, old scratch files).
    /// With warm workers configured it also starts them in the background.
    pub fn new(
        cfg: ResolverConfig,
        leases: LeaseStore,
        sink: EventSink,
        session: Arc<dyn SessionProvider>,
    ) -> Result<Self, ResolverError> {
        if let Some(pins) = &cfg.pinned {
            for (path, want, name) in [
                (&cfg.ytdlp, &pins.ytdlp_sha256, "yt-dlp"),
                (&cfg.deno, &pins.deno_sha256, "deno"),
            ] {
                let got = sha256_hex(path)
                    .map_err(|e| ResolverError::Tool(format!("{name} unreadable: {e}")))?;
                if !got.eq_ignore_ascii_case(want) {
                    return Err(ResolverError::Tool(format!(
                        "{name} does not match its pinned SHA-256 (expected {want}, found {got})"
                    )));
                }
            }
        }
        let cookies = cfg.run_dir.join("cookies");
        let scratch = cfg.run_dir.join("scratch");
        fs::create_dir_all(&cookies).map_err(|e| ResolverError::Io(e.to_string()))?;
        fs::create_dir_all(&scratch).map_err(|e| ResolverError::Io(e.to_string()))?;
        #[cfg(windows)]
        win::sweep_stale(&cookies, "ytdlp");
        sweep_old(&scratch, SCRATCH_MAX_AGE);

        let pool = match &cfg.warm {
            Some(warm_cfg) => {
                let plugin_root = warm::install_plugin(&cfg.run_dir)
                    .map_err(|e| ResolverError::Io(format!("worker plugin: {e}")))?;
                #[allow(unused_mut)]
                let mut env = helper_env(&scratch);
                #[cfg(feature = "test-support")]
                for (k, v) in &cfg.extra_env {
                    env.push((k.into(), v.into()));
                }
                let pool = Pool::new(
                    warm_cfg.clone(),
                    WorkerSpawn {
                        ytdlp: cfg.ytdlp.clone(),
                        deno: cfg.deno.clone(),
                        plugin_root,
                        scratch: scratch.clone(),
                        env,
                        youtube_args: warm_cfg.youtube_args.clone(),
                    },
                );
                pool.prewarm(warm_cfg.prewarm);
                Some(pool)
            }
            None => None,
        };
        let cache = cfg.cache.clone().map(LeaseCache::new);
        Ok(Self {
            inner: Arc::new(Inner {
                cfg,
                leases,
                sink,
                session,
                jobs: Mutex::new(HashMap::new()),
                slots: (Mutex::new(0), Condvar::new()),
                diag: Mutex::new(VecDeque::new()),
                closed: AtomicBool::new(false),
                pool,
                cache,
                flights: Mutex::new(HashMap::new()),
                prefetch_pending: AtomicUsize::new(0),
                prefetch_seq: AtomicU64::new(0),
                prefetch_pacer: Pacer::new(PREFETCH_BURST, PREFETCH_EVERY),
                dropped: Mutex::new((0, None)),
                recent_prefetch: Mutex::new(HashMap::new()),
                throttle: Mutex::new(Throttle::new()),
                log_file: Mutex::new(None),
            }),
            threads: Mutex::new(Vec::new()),
        })
    }

    /// Also append every diagnostics line (they hold only outcome codes and timings) to `path`,
    /// time-stamped, so a run can be read afterwards. An oversized old file is moved aside.
    pub fn set_log_file(&self, path: std::path::PathBuf) {
        if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            let _ = fs::rename(&path, path.with_extension("old"));
        }
        *self.inner.log_file.lock().unwrap() = Some(path);
    }

    /// Share one throttle with the other things that talk to YouTube as this account, so a 429
    /// seen by any of them slows all of them. Without this the resolver has a throttle of its own.
    pub fn set_throttle(&self, throttle: Throttle) {
        *self.inner.throttle.lock().unwrap() = throttle;
    }

    /// How long a click would have to wait before the resolver may ask YouTube again (`None` =
    /// not at all). While this is set a resolution that needs the network fails at once with
    /// `Failure::Throttled` instead of making another doomed request.
    pub fn cooldown_remaining(&self) -> Option<Duration> {
        self.inner.throttle().blocked(Lane::Foreground)
    }

    pub fn handle(&self) -> ResolverHandle {
        ResolverHandle {
            inner: self.inner.clone(),
        }
    }

    /// Cancel every in-flight resolution (logout, quit). Their helper trees are killed.
    pub fn cancel_all(&self) {
        self.inner.cancel_all();
    }

    /// Redacted outcome lines (`load=3 preload=false outcome=ok ms=4800 src=warm ...`). Safe to
    /// show or log.
    pub fn diagnostics(&self) -> Vec<String> {
        self.inner.diag.lock().unwrap().iter().cloned().collect()
    }

    /// Resolutions currently in flight.
    pub fn in_flight(&self) -> usize {
        self.inner.jobs.lock().unwrap().len()
    }

    /// Resolve `track` ahead of need so that playing it later starts from the cache. Silent: no
    /// event is produced, nothing is cancelled, and a track already cached or being resolved is
    /// left alone. Never waits for a worker that a click could need.
    pub fn prefetch(&self, track: &Track) {
        self.inner.prefetch(&track.video_id);
    }

    /// Warm worker pool state for diagnostics: (alive, idle, started, failed starts).
    pub fn warm_status(&self) -> Option<(usize, usize, u64, u64)> {
        self.inner.pool.as_ref().map(|p| {
            (
                p.alive(),
                p.idle(),
                p.stats.spawned.load(Ordering::Relaxed),
                p.stats.spawn_failures.load(Ordering::Relaxed),
            )
        })
    }

    /// Why the warm workers could not start, if they could not (diagnostics only).
    pub fn warm_last_error(&self) -> Option<String> {
        self.inner.pool.as_ref().and_then(|p| p.last_error())
    }

    /// Cached resolutions held right now.
    pub fn cached(&self) -> usize {
        self.inner.cache.as_ref().map_or(0, LeaseCache::len)
    }
}

impl Resolver for YtDlpResolver {
    fn start(&mut self, request: ResolveRequest) {
        if self.inner.closed.load(Ordering::SeqCst) {
            return;
        }
        let job = Arc::new(Job::default());
        if let Some(old) = self
            .inner
            .jobs
            .lock()
            .unwrap()
            .insert(request.load.get(), job.clone())
        {
            old.cancel(); // a re-resolve of the same load supersedes the old one
        }
        let inner = self.inner.clone();
        let handle = thread::Builder::new()
            .name("apollo-resolve".into())
            .spawn(move || inner.run_one(request, job))
            .expect("spawn resolver thread");
        let mut t = self.threads.lock().unwrap();
        t.retain(|h| !h.is_finished());
        t.push(handle);
    }

    fn cancel(&mut self, load: LoadId) {
        if let Some(job) = self.inner.jobs.lock().unwrap().remove(&load.get()) {
            job.cancel();
            self.inner.note(format!("load={} outcome=cancel-requested", load.get()));
        }
    }

    fn prefetch(&mut self, track: &Track) {
        YtDlpResolver::prefetch(self, track);
    }
}

impl Drop for YtDlpResolver {
    fn drop(&mut self) {
        // Quit: reject new work, cancel everything, wait (bounded) for the helpers to be reaped.
        self.inner.closed.store(true, Ordering::SeqCst);
        self.cancel_all();
        if let Some(pool) = &self.inner.pool {
            pool.close(); // kills idle workers; busy ones die when their request returns
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        let threads: Vec<_> = self.threads.lock().unwrap().drain(..).collect();
        for t in threads {
            while !t.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if t.is_finished() {
                let _ = t.join();
            }
        }
    }
}

fn sweep_old(dir: &Path, max_age: Duration) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let p = e.path();
            let _ = if p.is_dir() { fs::remove_dir_all(&p) } else { fs::remove_file(&p) };
        }
    }
}

/// What `produce_warm` decided.
enum Warm {
    Done(Outcome),
    /// Workers are unavailable: use a process per track.
    Fallback,
}

impl Inner {
    fn throttle(&self) -> Throttle {
        self.throttle.lock().unwrap().clone()
    }

    /// A prefetch that was not started. Counted, and written to the log at most every
    /// [`DROP_NOTE_EVERY`], so a storm of hints is visible without flooding the log.
    fn note_dropped(&self, reason: &str) {
        let mut d = self.dropped.lock().unwrap();
        d.0 += 1;
        if d.1.is_some_and(|at| at.elapsed() < DROP_NOTE_EVERY) {
            return;
        }
        let n = std::mem::replace(&mut d.0, 0);
        d.1 = Some(Instant::now());
        drop(d);
        self.note(format!("prefetch skipped reason={reason} n={n}"));
    }

    /// Whether `vid` was already prefetched here within the last hour, remembering it now.
    fn seen_before(&self, vid: &str) -> bool {
        let mut m = self.recent_prefetch.lock().unwrap();
        if m.len() >= RECENT_PREFETCH_MAX {
            m.retain(|_, at| at.elapsed() < RECENT_PREFETCH_AGE);
            if m.len() >= RECENT_PREFETCH_MAX {
                m.clear();
            }
        }
        m.insert(vid.to_owned(), Instant::now())
            .is_some_and(|at| at.elapsed() < RECENT_PREFETCH_AGE)
    }

    /// Feed what a finished resolution says about YouTube's mood into the shared throttle.
    fn learn(&self, outcome: &Outcome, note: &Note) {
        let throttle = self.throttle();
        if matches!(outcome, Outcome::Failed(Failure::Throttled, _)) || note.throttled {
            let wait = throttle.report_throttled(None);
            let bg = throttle.blocked(Lane::Background).unwrap_or_default();
            self.note(format!(
                "throttled strikes={} wait_s={} background_wait_s={}",
                throttle.strikes(),
                wait.as_secs(),
                bg.as_secs()
            ));
        } else if matches!(outcome, Outcome::Ok(_)) {
            throttle.report_ok();
        }
    }

    fn note(&self, line: String) {
        if let Some(path) = self.log_file.lock().unwrap().as_ref() {
            use std::io::Write;
            let stamp = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis());
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
                let _ = f.write_all(format!("t={stamp} resolver {line}\n").as_bytes());
            }
        }
        let mut d = self.diag.lock().unwrap();
        if d.len() == MAX_DIAG {
            d.pop_front();
        }
        d.push_back(line);
    }

    fn cancel_all(&self) {
        let jobs: Vec<Arc<Job>> = self.jobs.lock().unwrap().drain().map(|(_, j)| j).collect();
        for j in jobs {
            j.cancel();
        }
    }

    fn invalidate(&self) {
        self.cancel_all();
        if let Some(cache) = &self.cache {
            cache.clear();
        }
        self.leases.clear();
        if let Some(pool) = &self.pool {
            pool.recycle(); // cookies live in worker memory: end those processes too
        }
    }

    fn run_one(self: Arc<Self>, req: ResolveRequest, job: Arc<Job>) {
        let started = Instant::now();
        let mut note = Note::default();
        let outcome = self.resolve(&req, &job, &mut note);
        let (label, result) = match &outcome {
            Outcome::Ok(l) => ("ok".to_owned(), Some(Ok(*l))),
            Outcome::Failed(f, code) => (format!("{code}"), Some(Err(*f))),
            Outcome::Cancelled => ("cancelled".to_owned(), None),
        };
        self.note(format!(
            "load={} preload={} outcome={label} ms={}{}",
            req.load.get(),
            req.preload,
            started.elapsed().as_millis(),
            note.suffix()
        ));
        // Forget this job unless a newer one replaced it.
        {
            let mut jobs = self.jobs.lock().unwrap();
            if jobs.get(&req.load.get()).is_some_and(|j| Arc::ptr_eq(j, &job)) {
                jobs.remove(&req.load.get());
            }
        }
        if let Some(result) = result {
            (self.sink)(Event::Resolved {
                load: req.load,
                result,
            });
        }
    }

    // ------------------------------------------------------------------ speculative

    fn prefetch(self: &Arc<Self>, video_id: &VideoId) {
        if self.closed.load(Ordering::SeqCst) || (self.cache.is_none() && self.pool.is_none()) {
            return;
        }
        let vid = video_id.as_str().to_owned();
        if !crate::argv::valid_video_id(&vid) {
            return;
        }
        if self.cache.as_ref().is_some_and(|c| c.get(&vid, &self.leases).is_some())
            || self.flights.lock().unwrap().contains_key(&vid)
        {
            return;
        }
        if self.prefetch_pending.fetch_add(1, Ordering::SeqCst) >= MAX_PREFETCH_PENDING {
            self.prefetch_pending.fetch_sub(1, Ordering::SeqCst);
            self.note_dropped("busy");
            return;
        }
        // Speculation is the first thing to stop when YouTube pushes back, and it is paced
        // even when it does not.
        if self.throttle().blocked(Lane::Background).is_some() {
            self.prefetch_pending.fetch_sub(1, Ordering::SeqCst);
            self.note_dropped("cooldown");
            return;
        }
        if !self.prefetch_pacer.try_take() {
            self.prefetch_pending.fetch_sub(1, Ordering::SeqCst);
            self.note_dropped("budget");
            return;
        }
        let repeat = self.seen_before(&vid);
        let key = PREFETCH_KEY_BASE + self.prefetch_seq.fetch_add(1, Ordering::SeqCst);
        let job = Arc::new(Job::default());
        self.jobs.lock().unwrap().insert(key, job.clone());
        let me = self.clone();
        let spawned = thread::Builder::new()
            .name("apollo-prefetch".into())
            .spawn(move || {
                let started = Instant::now();
                let mut note = Note::default();
                let outcome = me.resolve_video(&VideoId::new(&vid), false, &job, &mut note);
                let label = match &outcome {
                    Outcome::Ok(_) => "ok".to_owned(),
                    Outcome::Failed(_, code) => (*code).to_owned(),
                    Outcome::Cancelled => "cancelled".to_owned(),
                };
                me.note(format!(
                    "prefetch outcome={label} ms={}{}{}",
                    started.elapsed().as_millis(),
                    note.suffix(),
                    if repeat { " repeat=true" } else { "" }
                ));
                me.jobs.lock().unwrap().remove(&key);
                me.prefetch_pending.fetch_sub(1, Ordering::SeqCst);
            });
        if spawned.is_err() {
            self.jobs.lock().unwrap().remove(&key);
            self.prefetch_pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    // ------------------------------------------------------------------- resolving

    fn resolve(&self, req: &ResolveRequest, job: &Job, note: &mut Note) -> Outcome {
        if self.cache.is_none() && self.pool.is_none() {
            // The original behaviour, untouched: a process per request.
            #[cfg(windows)]
            return self.resolve_cold(&req.track.video_id, req.load.get(), job);
            #[cfg(not(windows))]
            return Outcome::Failed(Failure::Unsupported, "no-worker");
        }
        let out = self.resolve_video(&req.track.video_id, !req.preload, job, note);
        match out {
            // The load was cancelled while the helper worked: the URL stays cached for the next
            // request, but this load gets nothing.
            Outcome::Ok(_) if job.is_cancelled() => Outcome::Cancelled,
            other => other,
        }
    }

    /// The session gate, then cache, then (shared) resolution.
    fn resolve_video(&self, video_id: &VideoId, foreground: bool, job: &Job, note: &mut Note) -> Outcome {
        let vid = video_id.as_str();
        loop {
            if job.is_cancelled() || self.closed.load(Ordering::SeqCst) {
                return Outcome::Cancelled;
            }
            // AUTH-02: the gate is the account's own validated session, never the resolver's
            // success, and never the cache (no session = no cached URLs handed out).
            let Some(snapshot) = self.session.snapshot() else {
                return match self.session.unavailable_reason() {
                    SessionUnavailable::NoSession => {
                        Outcome::Failed(Failure::SessionExpired, "no-session")
                    }
                    SessionUnavailable::Unconfirmed => {
                        Outcome::Failed(Failure::Network, "session-unconfirmed")
                    }
                };
            };
            if let Some(id) = self.cache.as_ref().and_then(|c| c.get(vid, &self.leases)) {
                note.src = "cache";
                return Outcome::Ok(id);
            }
            let (flight, leader) = {
                let mut flights = self.flights.lock().unwrap();
                match flights.get(vid) {
                    Some(f) => (f.clone(), false),
                    None => {
                        // Nothing in flight and YouTube said slow down: do not ask again. The
                        // cache was already consulted above, so only a real request is refused.
                        let lane = if foreground { Lane::Foreground } else { Lane::Background };
                        if let Some(wait) = self.throttle().blocked(lane) {
                            drop(flights);
                            if foreground {
                                self.note(format!("load held back: cooldown wait_s={}", wait.as_secs()));
                            }
                            note.src = "cooldown";
                            return Outcome::Failed(Failure::Throttled, "cooldown");
                        }
                        let f = Arc::new(Flight {
                            done: Mutex::new(None),
                            cv: Condvar::new(),
                            foreground: AtomicBool::new(foreground),
                        });
                        flights.insert(vid.to_owned(), f.clone());
                        (f, true)
                    }
                }
            };
            if !leader {
                if foreground {
                    flight.foreground.store(true, Ordering::SeqCst);
                }
                note.src = "joined";
                let mut g = flight.done.lock().unwrap();
                let finished = loop {
                    if let Some(r) = *g {
                        break r;
                    }
                    if job.is_cancelled() || self.closed.load(Ordering::SeqCst) {
                        return Outcome::Cancelled;
                    }
                    g = flight.cv.wait_timeout(g, Duration::from_millis(25)).unwrap().0;
                };
                match finished {
                    Some(Produced::Ok(id)) => return Outcome::Ok(id),
                    Some(Produced::Failed(f, code)) => return Outcome::Failed(f, code),
                    None => continue, // the leader gave up before starting: take over
                }
            }

            let outcome = self.produce(video_id, &snapshot, &flight.foreground, job, note);
            self.learn(&outcome, note);
            let produced = match &outcome {
                Outcome::Ok(id) => {
                    if let Some(c) = &self.cache {
                        c.insert(vid, *id);
                    }
                    Some(Produced::Ok(*id))
                }
                Outcome::Failed(f, code) => Some(Produced::Failed(*f, code)),
                Outcome::Cancelled => None,
            };
            *flight.done.lock().unwrap() = Some(produced);
            flight.cv.notify_all();
            {
                let mut flights = self.flights.lock().unwrap();
                if flights.get(vid).is_some_and(|f| Arc::ptr_eq(f, &flight)) {
                    flights.remove(vid);
                }
            }
            return outcome;
        }
    }

    /// Actually resolve: a warm worker if one can run, else (Windows) a process per track.
    fn produce(
        &self,
        video_id: &VideoId,
        snapshot: &SessionSnapshot,
        foreground: &AtomicBool,
        job: &Job,
        note: &mut Note,
    ) -> Outcome {
        if let Some(pool) = &self.pool {
            if pool.usable() {
                match self.produce_warm(pool, video_id, snapshot, foreground, job, note) {
                    Warm::Done(o) => return o,
                    Warm::Fallback => {}
                }
            }
            // Workers cannot run here: say why, then fall back to a process per track.
            self.note(format!(
                "warm workers unavailable ({}); using a process per track",
                pool.last_error().unwrap_or_else(|| "unknown".into())
            ));
        }
        #[cfg(windows)]
        {
            note.src = "cold";
            self.resolve_cold(video_id, 0, job)
        }
        #[cfg(not(windows))]
        {
            let _ = job;
            Outcome::Failed(Failure::Unsupported, "no-worker")
        }
    }

    /// One warm resolution. A click uses the fast arguments when configured; if those yield no
    /// usable stream (an embedding-restricted track, a format they cannot give) the same track is
    /// resolved again with the full arguments.
    fn produce_warm(
        &self,
        pool: &Pool,
        video_id: &VideoId,
        snapshot: &SessionSnapshot,
        foreground: &AtomicBool,
        job: &Job,
        note: &mut Note,
    ) -> Warm {
        let fast = pool.config().foreground_youtube_args.is_some() && foreground.load(Ordering::SeqCst);
        let first = self.warm_once(pool, video_id, snapshot, foreground, job, note, fast);
        if fast {
            if let Warm::Done(Outcome::Failed(Failure::Unsupported | Failure::Restricted, code)) = &first {
                self.note(format!("fast resolve gave {code}; retrying with the full settings"));
                return self.warm_once(pool, video_id, snapshot, foreground, job, note, false);
            }
        }
        first
    }

    #[allow(clippy::too_many_arguments)]
    fn warm_once(
        &self,
        pool: &Pool,
        video_id: &VideoId,
        snapshot: &SessionSnapshot,
        foreground: &AtomicBool,
        job: &Job,
        note: &mut Note,
        fast: bool,
    ) -> Warm {
        let cfg = pool.config();
        let args: &[(String, Vec<String>)] = match (&cfg.foreground_youtube_args, fast) {
            (Some(a), true) => a,
            _ => &cfg.youtube_args,
        };
        let watch_url = if fast && cfg.foreground_youtube_args.is_some() {
            cfg.foreground_watch_url
        } else {
            cfg.watch_url
        };
        let extractor_args = {
            let youtube: serde_json::Map<String, Value> = args.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            json!({ "youtube": youtube })
        };
        let closed = || self.closed.load(Ordering::SeqCst);
        let waiting_cancelled = || job.is_cancelled() || self.closed.load(Ordering::SeqCst);
        for attempt in 0..2 {
            let mut co = match pool.acquire(foreground, &waiting_cancelled) {
                Acquire::Got(c) => c,
                Acquire::Cancelled => return Warm::Done(Outcome::Cancelled),
                Acquire::Unavailable => return Warm::Fallback,
            };
            let w = co.worker();
            if w.loaded_epoch != Some(snapshot.epoch) {
                let msg = json!({"op": "cookies", "netscape": snapshot.cookies_netscape.expose()});
                match w.request(msg, Duration::from_secs(10), &closed) {
                    Ok(v) if v["ok"] == true => w.loaded_epoch = Some(snapshot.epoch),
                    Ok(_) => {
                        co.discard();
                        return Warm::Done(Outcome::Failed(Failure::Unsupported, "cookie-load-failed"));
                    }
                    Err(_) => {
                        co.discard();
                        continue; // a dead worker: one retry on a fresh one
                    }
                }
            }
            let w = co.worker();
            let sent = Instant::now();
            let msg = json!({"op": "resolve", "video_id": video_id.as_str(), "extractor_args": extractor_args,
                "site": if watch_url { "www" } else { "music" }});
            let reply = match w.request(msg, cfg.request_timeout, &closed) {
                Ok(v) => v,
                Err(WorkerError::Timeout) => {
                    co.discard();
                    return Warm::Done(Outcome::Failed(Failure::Network, "timeout"));
                }
                Err(WorkerError::Aborted) => {
                    co.discard();
                    return Warm::Done(Outcome::Cancelled);
                }
                Err(_) => {
                    co.discard();
                    if attempt == 0 {
                        continue;
                    }
                    return Warm::Done(Outcome::Failed(Failure::Network, "worker-died"));
                }
            };
            note.src = if fast { "warm-fast" } else { "warm" };
            note.worker_ms = Some(sent.elapsed().as_millis() as u64);
            note.throttled |= reply["throttled"] == true;
            note.client = reply["client"]
                .as_str()
                .map(|c| c.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.').take(24).collect());
            let info = &reply["info"];
            let fid: String = info["format_id"]
                .as_str()
                .unwrap_or("")
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(16)
                .collect();
            if !fid.is_empty() {
                let abr = info["abr"].as_f64().map_or(String::new(), |a| format!(" abr={}", a.round() as i64));
                let codec: String = info["acodec"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '.')
                    .take(12)
                    .collect();
                note.format = Some(format!("fmt={fid}{abr} codec={codec}"));
            }
            if let Some(phases) = reply["phases"].as_array() {
                note.phases = phases
                    .iter()
                    .filter_map(|p| {
                        let label = p[0].as_str()?;
                        let ms = p[1].as_u64()?;
                        // "[youtube] Downloading webpage" -> "Downloading webpage"
                        let label = match (label.strip_prefix('['), label.find(']')) {
                            (Some(_), Some(end)) => &label[end + 1..],
                            _ => label,
                        };
                        let label: String = label
                            .trim()
                            .chars()
                            .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-')
                            .take(40)
                            .collect();
                        (!label.is_empty()).then_some((label, ms))
                    })
                    .collect();
            }
            // Cookie rotation: yt-dlp may rewrite the jar. Offer it back under the epoch the worker
            // loaded; the account service refuses it if a newer jar exists (TEST-15). A merge bumps
            // the epoch, so the next request reloads the merged jar.
            if let (Some(jar), Some(epoch)) = (reply["jar"].as_str(), w.loaded_epoch) {
                let merged = self.session.offer_rotation(epoch, SecretString::new(jar.to_owned()));
                self.note(format!("cookie-rotation merged={merged}"));
            }
            drop(co);
            return Warm::Done(self.finish_reply(video_id, &reply));
        }
        Warm::Done(Outcome::Failed(Failure::Network, "worker-died"))
    }

    /// A worker's reply to the validated lease (or the failure class), the same checks as the
    /// per-process path.
    fn finish_reply(&self, video_id: &VideoId, reply: &Value) -> Outcome {
        if reply["ok"] != true {
            let text = reply["error"].as_str().unwrap_or("");
            let (f, code) = classify_failure(text);
            return Outcome::Failed(f, code);
        }
        let Ok(bytes) = serde_json::to_vec(&reply["info"]) else {
            return Outcome::Failed(Failure::Unsupported, "unparseable-output");
        };
        let candidate = match parse_ytdlp_json(&bytes) {
            Ok(c) => c,
            Err(_) => return Outcome::Failed(Failure::Unsupported, "unparseable-output"),
        };
        match self.cfg.url_policy.validate(video_id, candidate) {
            Ok(lease) => Outcome::Ok(self.leases.insert(lease)),
            Err(e) => Outcome::Failed(lease_error_failure(&e), lease_error_code(&e)),
        }
    }

    // ---------------------------------------------------------- one process per track

    #[cfg(windows)]
    fn acquire_slot(&self, job: &Job) -> Option<SlotGuard<'_>> {
        let max = self.cfg.max_concurrent.max(1);
        let (lock, cv) = &self.slots;
        let mut n = lock.lock().unwrap();
        loop {
            if job.is_cancelled() || self.closed.load(Ordering::SeqCst) {
                return None;
            }
            if *n < max {
                *n += 1;
                return Some(SlotGuard { inner: self });
            }
            n = cv.wait_timeout(n, Duration::from_millis(50)).unwrap().0;
        }
    }

    #[cfg(windows)]
    fn resolve_cold(&self, video_id: &VideoId, load: u64, job: &Job) -> Outcome {
        let Some(_slot) = self.acquire_slot(job) else {
            return Outcome::Cancelled;
        };
        // AUTH-02: the gate is the account's own validated session, never the resolver's success.
        let Some(snapshot) = self.session.snapshot() else {
            return match self.session.unavailable_reason() {
                SessionUnavailable::NoSession => {
                    Outcome::Failed(Failure::SessionExpired, "no-session")
                }
                // Cookies exist but the service has not confirmed them yet: not a re-login case.
                SessionUnavailable::Unconfirmed => {
                    Outcome::Failed(Failure::Network, "session-unconfirmed")
                }
            };
        };
        let video_str = video_id.as_str();
        let (cookies_base, scratch) = (self.cfg.run_dir.join("cookies"), self.cfg.run_dir.join("scratch"));
        let Ok(dir) = PrivateDir::create(&cookies_base, "ytdlp") else {
            return Outcome::Failed(Failure::Unsupported, "private-dir-failed");
        };
        let Ok(cookie_path) = dir.create_file(COOKIE_FILE, snapshot.cookies_netscape.expose().as_bytes())
        else {
            return Outcome::Failed(Failure::Unsupported, "cookie-file-failed");
        };
        let Some(args) = build_args(video_str, &cookie_path, &self.cfg.deno) else {
            return Outcome::Failed(Failure::Unsupported, "bad-video-id");
        };

        let mut cmd = Command::new(&self.cfg.ytdlp);
        cmd.args(&args).env_clear().current_dir(&scratch);
        for (k, v) in helper_env(&scratch) {
            cmd.env(k, v);
        }
        #[cfg(feature = "test-support")]
        for (k, v) in &self.cfg.extra_env {
            cmd.env(k, v);
        }

        let run = run_contained(cmd, self.cfg.timeout, self.cfg.max_stdout_bytes, &job.switch);

        // Cookie rotation: yt-dlp may rewrite the jar. Offer it back under the snapshot's epoch;
        // the account service refuses it if a newer jar exists (TEST-15).
        if matches!(run.status, RunStatus::Exited(_)) {
            if let Ok(after) = fs::read_to_string(&cookie_path) {
                if after != snapshot.cookies_netscape.expose() && !after.is_empty() {
                    let merged = self.session.offer_rotation(snapshot.epoch, SecretString::new(after));
                    self.note(format!("load={load} cookie-rotation merged={merged}"));
                }
            }
        }
        drop(dir); // zero and delete the cookie file now, before anything else happens

        if job.is_cancelled() || matches!(run.status, RunStatus::Cancelled) {
            return Outcome::Cancelled;
        }
        match run.status {
            RunStatus::TimedOut => return Outcome::Failed(Failure::Network, "timeout"),
            RunStatus::OutputTooLarge => return Outcome::Failed(Failure::Unsupported, "output-too-large"),
            RunStatus::SpawnFailed => return Outcome::Failed(Failure::Unsupported, "spawn-failed"),
            RunStatus::Exited(0) => {}
            RunStatus::Exited(_) => {
                let (f, code) = classify_failure(&run.stderr);
                return Outcome::Failed(f, code);
            }
            RunStatus::Cancelled => unreachable!(),
        }
        let candidate = match parse_ytdlp_json(&run.stdout) {
            Ok(c) => c,
            Err(_) => return Outcome::Failed(Failure::Unsupported, "unparseable-output"),
        };
        let lease = match self.cfg.url_policy.validate(video_id, candidate) {
            Ok(l) => l,
            Err(e) => return Outcome::Failed(lease_error_failure(&e), lease_error_code(&e)),
        };
        let id = self.leases.insert(lease);
        if job.is_cancelled() {
            self.leases.remove(id); // cancelled while parsing: leave nothing behind
            return Outcome::Cancelled;
        }
        Outcome::Ok(id)
    }
}

#[cfg(windows)]
struct SlotGuard<'a> {
    inner: &'a Inner,
}

#[cfg(windows)]
impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let (lock, cv) = &self.inner.slots;
        *lock.lock().unwrap() -= 1;
        cv.notify_one();
    }
}

/// A minimal, explicit environment: no proxy variables, no user profile, no config discovery.
#[cfg(windows)]
fn helper_env(scratch: &Path) -> Vec<(OsString, OsString)> {
    let sys_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let mut path = OsString::from(&sys_root);
    path.push("\\System32");
    let s = scratch.as_os_str().to_owned();
    vec![
        ("SystemRoot".into(), sys_root.clone()),
        ("windir".into(), sys_root),
        ("PATH".into(), path),
        ("TEMP".into(), s.clone()),
        ("TMP".into(), s.clone()),
        ("USERPROFILE".into(), s.clone()),
        ("APPDATA".into(), s.clone()),
        ("LOCALAPPDATA".into(), s.clone()),
        ("HOME".into(), s),
        ("PYTHONIOENCODING".into(), "utf-8".into()),
        ("NO_COLOR".into(), "1".into()),
    ]
}

/// Non-Windows builds only exist for development and tests: keep PATH so a script yt-dlp runs.
#[cfg(not(windows))]
fn helper_env(scratch: &Path) -> Vec<(OsString, OsString)> {
    let s = scratch.as_os_str().to_owned();
    vec![
        ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
        ("TMPDIR".into(), s.clone()),
        ("HOME".into(), s),
        ("PYTHONIOENCODING".into(), "utf-8".into()),
        ("NO_COLOR".into(), "1".into()),
    ]
}
