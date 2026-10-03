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
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use apollo_domain::playback::{Event, Failure, LeaseId, LoadId};
use apollo_runtime::{ResolveRequest, Resolver};
use apollo_services_common::win::{self, PrivateDir};
use apollo_services_common::{EventSink, LeaseStore, SecretString, SessionProvider};
use sha2::{Digest, Sha256};

use crate::argv::build_args;
use crate::config::ResolverConfig;
use crate::parse::{
    classify_failure, lease_error_code, lease_error_failure, parse_ytdlp_json,
};
use crate::runner::{run_contained, KillSwitch, RunStatus};

const COOKIE_FILE: &str = "cookies.txt";
const SCRATCH_MAX_AGE: Duration = Duration::from_secs(10 * 60);
const MAX_DIAG: usize = 200;

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

#[derive(Default)]
struct Job {
    switch: KillSwitch,
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
    slots: (Mutex<usize>, Condvar),
    diag: Mutex<VecDeque<String>>,
    closed: AtomicBool,
}

pub struct YtDlpResolver {
    inner: Arc<Inner>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl YtDlpResolver {
    /// Verifies the pinned tools, prepares the run directories and sweeps leftovers of
    /// processes that died without cleaning up (stale private cookie dirs, old scratch files).
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
        win::sweep_stale(&cookies, "ytdlp");
        sweep_old(&scratch, SCRATCH_MAX_AGE);
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
            }),
            threads: Mutex::new(Vec::new()),
        })
    }

    /// Cancel every in-flight resolution (logout, quit). Their helper trees are killed.
    pub fn cancel_all(&self) {
        let jobs: Vec<Arc<Job>> = self.inner.jobs.lock().unwrap().drain().map(|(_, j)| j).collect();
        for j in jobs {
            j.switch.cancel();
        }
    }

    /// Redacted outcome lines (`load=3 preload=false outcome=ok ms=4800`). Safe to show or log.
    pub fn diagnostics(&self) -> Vec<String> {
        self.inner.diag.lock().unwrap().iter().cloned().collect()
    }

    /// Resolutions currently in flight.
    pub fn in_flight(&self) -> usize {
        self.inner.jobs.lock().unwrap().len()
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
            old.switch.cancel(); // a re-resolve of the same load supersedes the old one
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
            job.switch.cancel();
            self.inner.note(format!("load={} outcome=cancel-requested", load.get()));
        }
    }
}

impl Drop for YtDlpResolver {
    fn drop(&mut self) {
        // Quit: reject new work, cancel everything, wait (bounded) for the helpers to be reaped.
        self.inner.closed.store(true, Ordering::SeqCst);
        self.cancel_all();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let threads: Vec<_> = self.threads.lock().unwrap().drain(..).collect();
        for t in threads {
            while !t.is_finished() && std::time::Instant::now() < deadline {
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

impl Inner {
    fn note(&self, line: String) {
        let mut d = self.diag.lock().unwrap();
        if d.len() == MAX_DIAG {
            d.pop_front();
        }
        d.push_back(line);
    }

    fn run_one(self: Arc<Self>, req: ResolveRequest, job: Arc<Job>) {
        let started = std::time::Instant::now();
        let outcome = self.resolve(&req, &job);
        let (label, result) = match &outcome {
            Outcome::Ok(l) => ("ok".to_owned(), Some(Ok(*l))),
            Outcome::Failed(f, code) => (format!("{code}"), Some(Err(*f))),
            Outcome::Cancelled => ("cancelled".to_owned(), None),
        };
        self.note(format!(
            "load={} preload={} outcome={label} ms={}",
            req.load.get(),
            req.preload,
            started.elapsed().as_millis()
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

    fn acquire_slot(&self, job: &Job) -> Option<SlotGuard<'_>> {
        let max = self.cfg.max_concurrent.max(1);
        let (lock, cv) = &self.slots;
        let mut n = lock.lock().unwrap();
        loop {
            if job.switch.is_cancelled() || self.closed.load(Ordering::SeqCst) {
                return None;
            }
            if *n < max {
                *n += 1;
                return Some(SlotGuard { inner: self });
            }
            n = cv.wait_timeout(n, Duration::from_millis(50)).unwrap().0;
        }
    }

    fn resolve(&self, req: &ResolveRequest, job: &Job) -> Outcome {
        let Some(_slot) = self.acquire_slot(job) else {
            return Outcome::Cancelled;
        };
        // AUTH-02: the gate is the account's own validated session, never the resolver's success.
        let Some(snapshot) = self.session.snapshot() else {
            return Outcome::Failed(Failure::SessionExpired, "no-session");
        };
        let video_id = req.track.video_id.as_str();
        let (cookies_base, scratch) = (self.cfg.run_dir.join("cookies"), self.cfg.run_dir.join("scratch"));
        let Ok(dir) = PrivateDir::create(&cookies_base, "ytdlp") else {
            return Outcome::Failed(Failure::Unsupported, "private-dir-failed");
        };
        let Ok(cookie_path) = dir.create_file(COOKIE_FILE, snapshot.cookies_netscape.expose().as_bytes())
        else {
            return Outcome::Failed(Failure::Unsupported, "cookie-file-failed");
        };
        let Some(args) = build_args(video_id, &cookie_path, &self.cfg.deno) else {
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
                    self.note(format!("load={} cookie-rotation merged={merged}", req.load.get()));
                }
            }
        }
        drop(dir); // zero and delete the cookie file now, before anything else happens

        if job.switch.is_cancelled() || matches!(run.status, RunStatus::Cancelled) {
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
        let lease = match self.cfg.url_policy.validate(&req.track.video_id, candidate) {
            Ok(l) => l,
            Err(e) => return Outcome::Failed(lease_error_failure(&e), lease_error_code(&e)),
        };
        let id = self.leases.insert(lease);
        if job.switch.is_cancelled() {
            self.leases.remove(id); // cancelled while parsing: leave nothing behind
            return Outcome::Cancelled;
        }
        Outcome::Ok(id)
    }
}

struct SlotGuard<'a> {
    inner: &'a Inner,
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        let (lock, cv) = &self.inner.slots;
        *lock.lock().unwrap() -= 1;
        cv.notify_one();
    }
}

/// A minimal, explicit environment: no proxy variables, no user profile, no config discovery.
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
