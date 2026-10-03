//! Warm yt-dlp workers: long-lived helper processes that resolve many tracks.
//!
//! Starting `yt-dlp.exe` for every track throws away everything that makes the next resolution
//! cheap: the unpacked executable, the imported Python modules, the TLS connections, YouTube's
//! visitor session, and the parsed player script (a Deno run of 1-2.5 s per track on its own).
//! A worker is the *same pinned yt-dlp executable* started once with a tiny extractor plugin
//! (`plugin/apollo_serve.py`, embedded below) that answers one JSON request per line.
//!
//! * Cookies travel over the pipe; no cookie file exists while a worker runs.
//! * A worker handles one request at a time. The pool starts up to `max_workers` of them, and
//!   speculative work may only use `background_limit`, so a click always finds a free one.
//! * A worker that times out, dies or is abandoned is killed (the whole process tree on
//!   Windows) and replaced on demand. A worker that cannot start at all makes the resolver fall
//!   back to a process per track.

use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::WarmConfig;

/// The worker's request loop, written next to yt-dlp's plugins at startup.
const PLUGIN_SOURCE: &str = include_str!("../plugin/apollo_serve.py");
const PLUGIN_PACKAGE: &str = "apollo_worker";

/// Consecutive start-up failures after which the pool gives up (the caller falls back).
const GIVE_UP_AFTER: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerError {
    Spawn(String),
    /// No ready message in time, or the process exited first. Carries the tail of the process's
    /// own error output (a few hundred characters; shown in diagnostics only).
    Startup(String),
    Timeout,
    Died,
    Aborted,
}

/// Everything needed to start a worker.
#[derive(Clone, Debug)]
pub struct WorkerSpawn {
    pub ytdlp: PathBuf,
    pub deno: PathBuf,
    /// Directory holding the plugin package (see [`install_plugin`]).
    pub plugin_root: PathBuf,
    pub scratch: PathBuf,
    pub env: Vec<(OsString, OsString)>,
    /// `youtube:` extractor arguments (key, values) handed to the worker's yt-dlp.
    pub youtube_args: Vec<(String, Vec<String>)>,
}

/// Write the worker plugin under `run_dir/plugins` (only touching the file when it changed) and
/// return the directory to pass to `--plugin-dirs`.
pub fn install_plugin(run_dir: &Path) -> std::io::Result<PathBuf> {
    let root = run_dir.join("plugins");
    let dir = root.join(PLUGIN_PACKAGE).join("yt_dlp_plugins").join("extractor");
    fs::create_dir_all(&dir)?;
    let file = dir.join("apollo_serve.py");
    if fs::read_to_string(&file).ok().as_deref() != Some(PLUGIN_SOURCE) {
        fs::write(&file, PLUGIN_SOURCE)?;
    }
    Ok(root)
}

#[allow(dead_code)]
enum Killer {
    #[cfg(windows)]
    Job(Arc<apollo_services_common::win::JobObject>),
    Plain,
}

/// One resident yt-dlp.
pub struct Worker {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    killer: Killer,
    next_id: u64,
    /// The account epoch whose cookies this worker holds.
    pub loaded_epoch: Option<u64>,
    /// Spawn-to-ready time.
    pub startup_ms: u64,
    /// Requests served (diagnostics).
    pub served: u64,
}

impl Worker {
    pub fn spawn(spec: &WorkerSpawn, startup_timeout: Duration, abort: &dyn Fn() -> bool) -> Result<Self, WorkerError> {
        let started = Instant::now();
        let mut cmd = Command::new(&spec.ytdlp);
        cmd.args([
            "--ignore-config",
            "--quiet",
            "--no-warnings",
            "--no-update",
            "--no-cache-dir",
            "--no-remote-components",
            "--plugin-dirs",
        ])
        .arg(&spec.plugin_root)
        .arg("apollo-serve:")
        .env_clear()
        .current_dir(&spec.scratch)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        #[cfg(windows)]
        let (mut child, killer) = {
            use apollo_services_common::win::{spawn_in_job, JobObject};
            let job = Arc::new(
                JobObject::new_kill_on_close().map_err(|e| WorkerError::Spawn(e.to_string()))?,
            );
            let child = spawn_in_job(&mut cmd, &job).map_err(|e| WorkerError::Spawn(e.to_string()))?;
            (child, Killer::Job(job))
        };
        #[cfg(not(windows))]
        let (mut child, killer) = (
            cmd.spawn().map_err(|e| WorkerError::Spawn(e.to_string()))?,
            Killer::Plain,
        );

        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        // stderr is drained so the process never blocks on it. Only a short tail is kept, and
        // only ever surfaces when the worker fails to start (it can hold paths and URLs).
        let err_tail = Arc::new(Mutex::new(String::new()));
        {
            let tail = err_tail.clone();
            thread::spawn(move || {
                let mut r = BufReader::new(stderr);
                let mut line = Vec::new();
                loop {
                    line.clear();
                    match r.read_until(b'\n', &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let mut t = tail.lock().unwrap();
                            t.push_str(&String::from_utf8_lossy(&line));
                            if t.len() > 600 {
                                let cut = t.len() - 600;
                                let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(0);
                                t.drain(..cut);
                            }
                        }
                    }
                }
            });
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let r = BufReader::new(stdout);
            for line in r.split(b'\n') {
                let Ok(line) = line else { break };
                // Anything that is not one JSON object per line is noise from the host.
                if let Ok(v @ Value::Object(_)) = serde_json::from_slice::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });

        let mut w = Worker {
            child,
            stdin,
            rx,
            killer,
            next_id: 1,
            loaded_epoch: None,
            startup_ms: 0,
            served: 0,
        };
        let mut init = json!({"op": "init", "deno": spec.deno.to_string_lossy()});
        if !spec.youtube_args.is_empty() {
            let args: serde_json::Map<String, Value> = spec
                .youtube_args
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect();
            init["extractor_args"] = json!({ "youtube": args });
        }
        let startup_error = |w: &mut Worker| {
            w.kill();
            // Give the stderr reader a moment to pick up the last words of a dying process.
            thread::sleep(Duration::from_millis(100));
            let tail = err_tail.lock().unwrap().trim().replace(['\r', '\n'], " | ");
            WorkerError::Startup(tail.chars().take(300).collect())
        };
        if w.send(&init).is_err() {
            return Err(startup_error(&mut w));
        }
        let deadline = started + startup_timeout;
        loop {
            if abort() {
                w.kill();
                return Err(WorkerError::Aborted);
            }
            match w.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(v) if v["event"] == "ready" => break,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => {}
                Err(_) => return Err(startup_error(&mut w)),
            }
        }
        w.startup_ms = started.elapsed().as_millis() as u64;
        Ok(w)
    }

    fn send(&mut self, msg: &Value) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(msg).unwrap_or_default();
        line.push(b'\n');
        self.stdin.write_all(&line)?;
        self.stdin.flush()
    }

    /// One request, one reply. Replies to earlier abandoned requests are skipped by id.
    pub fn request(
        &mut self,
        mut msg: Value,
        timeout: Duration,
        abort: &dyn Fn() -> bool,
    ) -> Result<Value, WorkerError> {
        let id = self.next_id;
        self.next_id += 1;
        msg["id"] = json!(id);
        if self.send(&msg).is_err() {
            return Err(WorkerError::Died);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if abort() {
                return Err(WorkerError::Aborted);
            }
            match self.rx.recv_timeout(Duration::from_millis(25)) {
                Ok(v) if v["id"] == id => {
                    self.served += 1;
                    return Ok(v);
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        return Err(WorkerError::Timeout);
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return Err(WorkerError::Died),
            }
        }
    }

    /// Ask the worker to exit by itself. A PyInstaller executable cleans up its unpacked files
    /// only when it ends on its own; killing it leaves ~tens of MB behind in the scratch folder.
    fn request_quit(&mut self) {
        let _ = self.send(&json!({"op": "quit"}));
    }

    fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Kill the whole tree now (stuck or abandoned requests, timeouts).
    pub fn kill(&mut self) {
        match &self.killer {
            #[cfg(windows)]
            Killer::Job(job) => job.terminate(),
            Killer::Plain => {}
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if !self.exited() {
            self.kill();
        }
    }
}

/// End idle workers politely, all at once, then kill whatever has not left within `patience`.
fn retire(mut workers: Vec<Worker>, patience: Duration) {
    for w in &mut workers {
        w.request_quit();
    }
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline && workers.iter_mut().any(|w| !w.exited()) {
        thread::sleep(Duration::from_millis(20));
    }
    drop(workers); // Drop kills any that are still running
}

// ------------------------------------------------------------------------------------ pool

#[derive(Default)]
struct PoolState {
    idle: Vec<Worker>,
    /// Workers alive or being started (idle + checked out + starting).
    total: usize,
    /// Checked-out workers doing speculative work.
    bg_busy: usize,
}

/// Counters for diagnostics (no data from any request).
#[derive(Default)]
pub struct PoolStats {
    pub spawned: AtomicU64,
    pub spawn_failures: AtomicU64,
    pub killed: AtomicU64,
}

pub struct Pool {
    cfg: WarmConfig,
    spec: WorkerSpawn,
    st: Mutex<PoolState>,
    cv: Condvar,
    closed: AtomicBool,
    consecutive_failures: AtomicU32,
    /// Bumped by `recycle`: workers checked out under an older generation are killed on return.
    generation: AtomicU64,
    last_error: Mutex<Option<String>>,
    pub stats: PoolStats,
}

pub enum Acquire<'a> {
    Got(Checkout<'a>),
    /// The caller stopped waiting, or the pool is shutting down.
    Cancelled,
    /// Workers cannot be started here; use the fallback.
    Unavailable,
}

/// A worker on loan. Returned to the pool on drop unless `discard` was called.
pub struct Checkout<'a> {
    pool: &'a Pool,
    worker: Option<Worker>,
    bg: bool,
    healthy: bool,
    generation: u64,
}

impl Checkout<'_> {
    pub fn worker(&mut self) -> &mut Worker {
        self.worker.as_mut().expect("worker present until drop")
    }

    /// The worker is in an unknown state (timeout, death, abort): kill it instead of reusing it.
    pub fn discard(&mut self) {
        self.healthy = false;
    }
}

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        let worker = self.worker.take();
        let mut st = self.pool.st.lock().unwrap();
        if self.bg {
            st.bg_busy -= 1;
        }
        match worker {
            Some(w)
                if self.healthy
                    && !self.pool.closed.load(Ordering::SeqCst)
                    && self.generation == self.pool.generation.load(Ordering::SeqCst) =>
            {
                st.idle.push(w)
            }
            Some(mut w) => {
                st.total -= 1;
                self.pool.stats.killed.fetch_add(1, Ordering::Relaxed);
                drop(st);
                w.kill();
                self.pool.cv.notify_all();
                return;
            }
            None => st.total -= 1,
        }
        drop(st);
        self.pool.cv.notify_all();
    }
}

impl Pool {
    pub fn new(cfg: WarmConfig, spec: WorkerSpawn) -> Arc<Self> {
        Arc::new(Self {
            cfg,
            spec,
            st: Mutex::new(PoolState::default()),
            cv: Condvar::new(),
            closed: AtomicBool::new(false),
            consecutive_failures: AtomicU32::new(0),
            generation: AtomicU64::new(0),
            last_error: Mutex::new(None),
            stats: PoolStats::default(),
        })
    }

    pub fn config(&self) -> &WarmConfig {
        &self.cfg
    }

    /// False once starting workers has failed repeatedly: use the fallback.
    pub fn usable(&self) -> bool {
        !self.closed.load(Ordering::SeqCst)
            && self.consecutive_failures.load(Ordering::SeqCst) < GIVE_UP_AFTER
    }

    /// Start `n` workers in the background so the first click finds them running.
    pub fn prewarm(self: &Arc<Self>, n: usize) {
        for _ in 0..n.min(self.cfg.max_workers) {
            let pool = self.clone();
            thread::Builder::new()
                .name("apollo-prewarm".into())
                .spawn(move || {
                    {
                        let mut st = pool.st.lock().unwrap();
                        if st.total >= pool.cfg.max_workers || pool.closed.load(Ordering::SeqCst) {
                            return;
                        }
                        st.total += 1;
                    }
                    match pool.start_worker() {
                        Ok(w) => {
                            let mut st = pool.st.lock().unwrap();
                            if pool.closed.load(Ordering::SeqCst) {
                                st.total -= 1;
                            } else {
                                st.idle.push(w);
                            }
                        }
                        Err(_) => pool.st.lock().unwrap().total -= 1,
                    }
                    pool.cv.notify_all();
                })
                .ok();
        }
    }

    fn start_worker(&self) -> Result<Worker, WorkerError> {
        let r = Worker::spawn(&self.spec, self.cfg.startup_timeout, &|| {
            self.closed.load(Ordering::SeqCst)
        });
        match &r {
            Ok(_) => {
                self.consecutive_failures.store(0, Ordering::SeqCst);
                self.stats.spawned.fetch_add(1, Ordering::Relaxed);
            }
            Err(WorkerError::Aborted) => {}
            Err(e) => {
                self.consecutive_failures.fetch_add(1, Ordering::SeqCst);
                self.stats.spawn_failures.fetch_add(1, Ordering::Relaxed);
                *self.last_error.lock().unwrap() = Some(match e {
                    WorkerError::Startup(tail) if !tail.is_empty() => format!("did not start: {tail}"),
                    other => format!("{other:?}"),
                });
            }
        }
        r
    }

    /// Wait for a worker. `foreground` is re-read while waiting, so a speculative request that a
    /// click joins can be promoted. Speculative callers never take the last free worker.
    pub fn acquire(&self, foreground: &AtomicBool, cancelled: &dyn Fn() -> bool) -> Acquire<'_> {
        let bg_cap = self
            .cfg
            .background_limit
            .min(self.cfg.max_workers.saturating_sub(1))
            .max(1);
        let mut st = self.st.lock().unwrap();
        loop {
            if self.closed.load(Ordering::SeqCst) || cancelled() {
                return Acquire::Cancelled;
            }
            if !self.usable() {
                return Acquire::Unavailable;
            }
            let fg = foreground.load(Ordering::SeqCst);
            if fg || st.bg_busy < bg_cap {
                if let Some(w) = st.idle.pop() {
                    if !fg {
                        st.bg_busy += 1;
                    }
                    return Acquire::Got(Checkout {
                        pool: self,
                        worker: Some(w),
                        bg: !fg,
                        healthy: true,
                        generation: self.generation.load(Ordering::SeqCst),
                    });
                }
                if st.total < self.cfg.max_workers {
                    st.total += 1;
                    if !fg {
                        st.bg_busy += 1;
                    }
                    drop(st);
                    return match self.start_worker() {
                        Ok(w) => Acquire::Got(Checkout {
                            pool: self,
                            worker: Some(w),
                            bg: !fg,
                            healthy: true,
                            generation: self.generation.load(Ordering::SeqCst),
                        }),
                        Err(e) => {
                            let mut st = self.st.lock().unwrap();
                            st.total -= 1;
                            if !fg {
                                st.bg_busy -= 1;
                            }
                            drop(st);
                            self.cv.notify_all();
                            if e == WorkerError::Aborted {
                                Acquire::Cancelled
                            } else {
                                Acquire::Unavailable
                            }
                        }
                    };
                }
            }
            st = self.cv.wait_timeout(st, Duration::from_millis(40)).unwrap().0;
        }
    }

    /// Kill every worker and refuse new ones (quit).
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        let idle: Vec<Worker> = {
            let mut st = self.st.lock().unwrap();
            let idle: Vec<Worker> = st.idle.drain(..).collect();
            st.total -= idle.len();
            idle
        };
        retire(idle, Duration::from_millis(1500));
        self.cv.notify_all();
    }

    /// Why the last worker failed to start, if one did.
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().unwrap().clone()
    }

    /// End every worker (idle now, busy as soon as they return): their memory holds cookies.
    /// New ones start on demand.
    pub fn recycle(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let idle: Vec<Worker> = {
            let mut st = self.st.lock().unwrap();
            let idle: Vec<Worker> = st.idle.drain(..).collect();
            st.total -= idle.len();
            idle
        };
        retire(idle, Duration::from_millis(800));
        self.cv.notify_all();
    }

    /// Workers currently alive (idle or on loan), for diagnostics.
    pub fn alive(&self) -> usize {
        self.st.lock().unwrap().total
    }

    pub fn idle(&self) -> usize {
        self.st.lock().unwrap().idle.len()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}
