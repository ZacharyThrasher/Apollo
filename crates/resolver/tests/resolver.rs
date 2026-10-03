#![cfg(windows)]
//! Integration tests for `YtDlpResolver` using a fake yt-dlp (`apollo-fake-ytdlp`). The real
//! process machinery (Job Object, private ACL'd cookie dir, bounded pipes) is exercised for real;
//! only yt-dlp's behaviour is faked.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_domain::lease::UrlPolicy;
use apollo_domain::model::Track;
use apollo_domain::playback::{Coordinator, Effect, Event, Failure, LeaseId, LoadId};
use apollo_resolver::{PinnedTools, ResolverConfig, YtDlpResolver};
use apollo_runtime::{ResolveRequest, Resolver};
use apollo_services_common::{
    random_hex, unix_now, EventSink, LeaseStore, SecretString, SessionProvider, SessionSnapshot,
    SessionUnavailable,
};

const FAKE: &str = env!("CARGO_BIN_EXE_apollo-fake-ytdlp");
const COOKIE_CANARY: &str = "COOKIECANARY-0123456789abcdef";

// ------------------------------------------------------------------ test session

struct TestSession {
    has_session: AtomicBool,
    epoch: AtomicU64,
    jar: Mutex<String>,
    rotations: Mutex<Vec<(u64, bool)>>,
    /// Simulates a newer jar arriving while the helper is running.
    bump_epoch_on_snapshot: AtomicBool,
    unconfirmed: AtomicBool,
}

impl TestSession {
    fn new(has_session: bool) -> Arc<Self> {
        Arc::new(Self {
            has_session: AtomicBool::new(has_session),
            epoch: AtomicU64::new(5),
            jar: Mutex::new(format!(
                "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSAPISID\t{COOKIE_CANARY}\n"
            )),
            rotations: Mutex::new(Vec::new()),
            bump_epoch_on_snapshot: AtomicBool::new(false),
            unconfirmed: AtomicBool::new(false),
        })
    }
}

impl SessionProvider for TestSession {
    fn snapshot(&self) -> Option<SessionSnapshot> {
        if !self.has_session.load(Ordering::SeqCst) {
            return None;
        }
        let snap = SessionSnapshot {
            epoch: self.epoch.load(Ordering::SeqCst),
            cookies_netscape: SecretString::new(self.jar.lock().unwrap().clone()),
        };
        if self.bump_epoch_on_snapshot.load(Ordering::SeqCst) {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        Some(snap)
    }

    fn unavailable_reason(&self) -> SessionUnavailable {
        if self.unconfirmed.load(Ordering::SeqCst) {
            SessionUnavailable::Unconfirmed
        } else {
            SessionUnavailable::NoSession
        }
    }

    fn offer_rotation(&self, epoch: u64, cookies: SecretString) -> bool {
        let ok = epoch == self.epoch.load(Ordering::SeqCst);
        if ok {
            *self.jar.lock().unwrap() = cookies.expose().to_string();
        }
        self.rotations.lock().unwrap().push((epoch, ok));
        ok
    }
}

// --------------------------------------------------------------------------- rig

struct Rig {
    resolver: Option<YtDlpResolver>,
    leases: LeaseStore,
    rx: Receiver<(LoadId, Result<LeaseId, Failure>)>,
    session: Arc<TestSession>,
    root: PathBuf,
    log: PathBuf,
    pidfile: PathBuf,
    reqs: Vec<ResolveRequest>,
}

fn mint_requests(n: usize) -> Vec<ResolveRequest> {
    let mut c = Coordinator::default();
    let tracks: Vec<Track> = (0..n + 1)
        .map(|i| Track::new(format!("aaaaaaaaa{i:02}"), "t"))
        .collect();
    let mut out = Vec::new();
    let mut effects = c.handle(Event::PlayContext { tracks, start: 0 });
    while out.len() < n {
        for e in &effects {
            if let Effect::Resolve {
                load,
                entry,
                track,
                resume_ms,
            } = e
            {
                out.push(ResolveRequest {
                    load: *load,
                    entry: *entry,
                    track: track.clone(),
                    resume_ms: *resume_ms,
                    preload: false,
                });
            }
        }
        if out.len() < n {
            effects = c.handle(Event::Next);
        }
    }
    out.truncate(n);
    out
}

fn config(root: &Path, mode: &str, log: &Path, pidfile: &Path) -> ResolverConfig {
    ResolverConfig {
        ytdlp: PathBuf::from(FAKE),
        deno: PathBuf::from("C:\\fake\\deno.exe"),
        run_dir: root.join("run"),
        timeout: Duration::from_secs(30),
        max_concurrent: 2,
        max_stdout_bytes: 1024 * 1024,
        url_policy: UrlPolicy::production(),
        warm: None,
        cache: None,
        pinned: None,
        extra_env: vec![
            ("APOLLO_FAKE_MODE".into(), mode.into()),
            ("APOLLO_FAKE_LOG".into(), log.display().to_string()),
            ("APOLLO_FAKE_PIDFILE".into(), pidfile.display().to_string()),
        ],
    }
}

fn rig_with(mode: &str, session: Arc<TestSession>, tweak: impl FnOnce(&mut ResolverConfig)) -> Rig {
    let root = std::env::temp_dir().join(format!("apollo-resolver-test-{}", random_hex(6)));
    fs::create_dir_all(&root).unwrap();
    let (log, pidfile) = (root.join("fake.log"), root.join("pids.txt"));
    let mut cfg = config(&root, mode, &log, &pidfile);
    tweak(&mut cfg);
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let Event::Resolved { load, result } = ev {
            let _ = tx.lock().unwrap().send((load, result));
        }
    });
    let leases = LeaseStore::new();
    let resolver = YtDlpResolver::new(
        cfg,
        leases.clone(),
        sink,
        session.clone() as Arc<dyn SessionProvider>,
    )
    .expect("resolver");
    Rig {
        resolver: Some(resolver),
        leases,
        rx,
        session,
        root,
        log,
        pidfile,
        reqs: mint_requests(8),
    }
}

fn rig(mode: &str) -> Rig {
    rig_with(mode, TestSession::new(true), |_| {})
}

impl Rig {
    fn start(&mut self, i: usize) {
        let req = self.reqs[i].clone();
        self.rs().start(req);
    }
    fn rs(&mut self) -> &mut YtDlpResolver {
        self.resolver.as_mut().expect("resolver")
    }
    fn result(&self, timeout: Duration) -> Option<(LoadId, Result<LeaseId, Failure>)> {
        self.rx.recv_timeout(timeout).ok()
    }
    fn cookie_dirs(&self) -> Vec<PathBuf> {
        fs::read_dir(self.root.join("run").join("cookies"))
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default()
    }
    fn log_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
    fn pids(&self) -> Vec<u32> {
        fs::read_to_string(&self.pidfile)
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|p| p.parse().ok())
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        drop(self.resolver.take());
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(40));
    }
    f()
}

const MARGIN: Duration = Duration::from_secs(60);
const MAXAGE: Duration = Duration::from_secs(300);

// ------------------------------------------------------------------------- tests

#[test]
fn resolves_into_a_validated_memory_only_lease() {
    let mut r = rig("ok");
    let req = r.reqs[0].clone();
    r.rs().start(req.clone());
    let (load, res) = r.result(Duration::from_secs(20)).expect("a result");
    assert_eq!(load, req.load);
    let id = res.expect("lease");
    let lease = r
        .leases
        .get_fresh(id, unix_now(), MARGIN, MAXAGE)
        .expect("fresh lease in the store");
    assert_eq!(lease.video_id(), &req.track.video_id);
    assert!(lease.url().host_str().unwrap().ends_with(".googlevideo.com"));
    assert!(lease.expires_at_unix.unwrap() > unix_now());
    // yt-dlp reported a Cookie header; UrlPolicy must have dropped it, keeping non-secret ones.
    let names: Vec<String> = lease
        .headers_for(&lease.url().clone())
        .iter()
        .map(|(n, _)| n.to_ascii_lowercase())
        .collect();
    assert!(names.contains(&"user-agent".into()), "{names:?}");
    assert!(!names.contains(&"cookie".into()), "{names:?}");
    // Nothing is left on disk: the private cookie dir is gone.
    assert!(
        wait_until(Duration::from_secs(2), || r.cookie_dirs().is_empty()),
        "{:?}",
        r.cookie_dirs()
    );
}

// SEC-02: the cookie file exists only while yt-dlp runs, is user-only, and is never in argv/env.
#[test]
fn cookie_file_is_private_short_lived_and_never_on_the_command_line() {
    let mut r = rig("ok");
    r.start(0);
    r.result(Duration::from_secs(20)).expect("result").1.expect("ok");
    let log = r.log_text();
    // The helper saw the file while it ran...
    let line = log
        .lines()
        .find(|l| l.starts_with("COOKIEFILE path="))
        .expect("cookie file line");
    assert!(line.contains("exists=true"), "{line}");
    // Positive control for the leak scans: the jar really was handed over (non-trivial length).
    let len: usize = line.rsplit("len=").next().unwrap().trim().parse().unwrap();
    assert!(len > COOKIE_CANARY.len(), "cookie file was empty: {line}");
    assert!(log.contains("COOKIEFILE readable=true"));
    let path = line
        .trim_start_matches("COOKIEFILE path=")
        .split(" exists=")
        .next()
        .unwrap()
        .to_string();
    // ...and it is gone afterwards, together with its directory.
    assert!(
        wait_until(Duration::from_secs(2), || !Path::new(&path).exists()),
        "{path} still exists"
    );
    assert!(r.cookie_dirs().is_empty());
    // The secret was never on the command line or in the environment.
    for l in log
        .lines()
        .filter(|l| l.starts_with("ARGS ") || l.starts_with("ENV "))
    {
        assert!(!l.contains(COOKIE_CANARY), "cookie value leaked into a command line or env");
    }
    // Safety flags are present; no browser-cookie import.
    let args = log.lines().find(|l| l.starts_with("ARGS ")).unwrap();
    for must in [
        "--ignore-config",
        "--no-remote-components",
        "--no-plugin-dirs",
        "--no-cache-dir",
        "--js-runtimes",
        "--skip-download",
    ] {
        assert!(args.contains(must), "{must} missing: {args}");
    }
    assert!(!args.contains("cookies-from-browser"));
}

// AUTH-02 / M0 finding: no validated session means no helper process at all.
#[test]
fn no_session_means_no_process_and_no_guest_fallback() {
    let mut r = rig_with("ok", TestSession::new(false), |_| {});
    r.start(0);
    let (_, res) = r.result(Duration::from_secs(5)).expect("a result");
    assert_eq!(res.unwrap_err(), Failure::SessionExpired);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!r.log.exists(), "yt-dlp was started without a session");
    assert!(r.leases.is_empty());
}

#[test]
fn helper_failures_map_to_failure_classes() {
    for (mode, want) in [
        ("fail-bot", Failure::SessionExpired),
        ("fail-premium", Failure::Restricted),
        ("fail-429", Failure::Throttled),
        ("fail-format", Failure::Unsupported),
        ("fail-net", Failure::Network),
        ("fail-unknown", Failure::Unsupported),
        ("garbage", Failure::Unsupported),
    ] {
        let mut r = rig(mode);
        r.start(0);
        let (_, res) = r.result(Duration::from_secs(20)).expect(mode);
        assert_eq!(res.unwrap_err(), want, "{mode}");
        assert!(r.leases.is_empty(), "{mode}: a failed resolve left a lease");
        assert!(
            wait_until(Duration::from_secs(2), || r.cookie_dirs().is_empty()),
            "{mode}"
        );
    }
}

// TEST-14 / TEST-19: nothing unvalidated reaches the engine; unsupported formats are explicit.
#[test]
fn refused_candidates_never_become_leases() {
    for mode in ["wrong-id", "drm", "sabr", "bad-host", "http-url", "no-audio"] {
        let mut r = rig(mode);
        r.start(0);
        let (_, res) = r.result(Duration::from_secs(20)).expect(mode);
        assert_eq!(res.unwrap_err(), Failure::Unsupported, "{mode}");
        assert!(r.leases.is_empty(), "{mode}");
        let d = r.rs().diagnostics().join("\n");
        assert!(d.contains("outcome="), "{d}");
    }
}

// TEST-28: a hung helper is killed at the timeout, with its grandchild, and nothing is left.
#[test]
fn timeout_kills_the_whole_tree_and_cleans_up() {
    let mut r = rig_with("hang-tree", TestSession::new(true), |c| {
        c.timeout = Duration::from_millis(1500)
    });
    let t0 = Instant::now();
    r.start(0);
    let (_, res) = r.result(Duration::from_secs(20)).expect("a result");
    assert_eq!(res.unwrap_err(), Failure::Network);
    assert!(t0.elapsed() < Duration::from_secs(8), "took {:?}", t0.elapsed());
    let pids = r.pids();
    assert_eq!(pids.len(), 2, "parent and grandchild were started: {pids:?}");
    assert!(
        wait_until(Duration::from_secs(5), || pids.iter().all(|p| !alive(*p))),
        "orphan survived: {:?}",
        pids.iter().map(|p| (*p, alive(*p))).collect::<Vec<_>>()
    );
    assert!(wait_until(Duration::from_secs(2), || r.cookie_dirs().is_empty()));
    assert_eq!(r.rs().in_flight(), 0);
}

// TEST-05 / TEST-28: cancelling stops the work and reaps the tree; no result is ever published.
#[test]
fn cancel_reaps_the_tree_and_publishes_nothing() {
    let mut r = rig("hang-tree");
    let req = r.reqs[0].clone();
    r.rs().start(req.clone());
    assert!(
        wait_until(Duration::from_secs(10), || r.pids().len() == 2),
        "helper never started"
    );
    let pids = r.pids();
    r.rs().cancel(req.load);
    assert!(
        wait_until(Duration::from_secs(5), || pids.iter().all(|p| !alive(*p))),
        "orphan survived cancel"
    );
    assert!(
        r.result(Duration::from_millis(800)).is_none(),
        "a cancelled load published a result"
    );
    assert!(wait_until(Duration::from_secs(2), || r.cookie_dirs().is_empty()));
    assert!(r.leases.is_empty());
    assert_eq!(r.rs().in_flight(), 0);
}

// TEST-28: Quit with a resolution in flight is bounded and leaves nothing behind.
#[test]
fn dropping_the_resolver_mid_run_is_bounded_and_clean() {
    let mut r = rig("hang-tree");
    r.start(0);
    assert!(wait_until(Duration::from_secs(10), || r.pids().len() == 2));
    let pids = r.pids();
    let resolver = r.resolver.take().unwrap();
    let t0 = Instant::now();
    drop(resolver);
    assert!(t0.elapsed() < Duration::from_secs(6), "shutdown took {:?}", t0.elapsed());
    assert!(wait_until(Duration::from_secs(5), || pids.iter().all(|p| !alive(*p))));
    assert!(r.cookie_dirs().is_empty());
}

// TEST-15 (resolver half): the helper's rewritten jar is offered back under the epoch of the
// snapshot it started from, so a newer jar can never be overwritten by an older run.
#[test]
fn rotated_cookies_are_offered_back_under_the_snapshot_epoch() {
    let mut r = rig("rotate");
    r.start(0);
    r.result(Duration::from_secs(20)).expect("result").1.expect("ok");
    assert_eq!(*r.session.rotations.lock().unwrap(), vec![(5, true)]);
    assert!(r.session.jar.lock().unwrap().contains("# rotated-by-helper"));

    // A newer jar appears while the helper runs: the old run's rotation is refused.
    let session = TestSession::new(true);
    session.bump_epoch_on_snapshot.store(true, Ordering::SeqCst);
    let mut r2 = rig_with("rotate", session.clone(), |_| {});
    r2.start(0);
    r2.result(Duration::from_secs(20))
        .expect("result")
        .1
        .expect("still resolves");
    assert_eq!(*session.rotations.lock().unwrap(), vec![(5, false)]);
    assert!(
        !session.jar.lock().unwrap().contains("# rotated-by-helper"),
        "stale rotation overwrote a newer jar"
    );
}

// Concurrency is bounded: active load + one speculative preload, never more.
#[test]
fn at_most_max_concurrent_helpers_run() {
    let mut r = rig("hang-tree"); // never finishes by itself
    for i in 0..4 {
        r.start(i);
    }
    assert!(wait_until(Duration::from_secs(10), || r.pids().len() >= 4)); // 2 parents + 2 children
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(r.pids().len(), 4, "a third helper started: {:?}", r.pids());
    assert_eq!(r.rs().in_flight(), 4, "all four are tracked (two waiting for a slot)");
    r.rs().cancel_all();
    assert!(wait_until(Duration::from_secs(8), || r
        .pids()
        .iter()
        .all(|p| !alive(*p))));
    assert!(wait_until(Duration::from_secs(3), || r.cookie_dirs().is_empty()));
}

// Pipes are bounded: a helper that floods stdout is killed instead of exhausting memory.
#[test]
fn flooding_stdout_is_cut_off() {
    let mut r = rig("big-stdout"); // 32 MiB, then sleeps 60 s; cap is 1 MiB
    let t0 = Instant::now();
    r.start(0);
    let (_, res) = r.result(Duration::from_secs(20)).expect("a result");
    assert_eq!(res.unwrap_err(), Failure::Unsupported);
    assert!(t0.elapsed() < Duration::from_secs(15), "took {:?}", t0.elapsed());
    assert!(r.rs().diagnostics().join("\n").contains("output-too-large"));
}

// SEC-02: stale private dirs from a crashed run are swept at startup.
#[test]
fn startup_sweeps_stale_private_dirs() {
    let root = std::env::temp_dir().join(format!("apollo-resolver-test-{}", random_hex(6)));
    let cookies = root.join("run").join("cookies");
    let dead = cookies.join("ytdlp-4294967-deadbeef");
    fs::create_dir_all(&dead).unwrap();
    fs::write(dead.join("cookies.txt"), b"leftover").unwrap();
    let cfg = config(&root, "ok", &root.join("l"), &root.join("p"));
    let sink: EventSink = Arc::new(|_| {});
    let _resolver = YtDlpResolver::new(cfg, LeaseStore::new(), sink, TestSession::new(true)).unwrap();
    assert!(!dead.exists(), "stale cookie dir survived startup");
    let _ = fs::remove_dir_all(&root);
}

// RES-01: the helper executables are verified against their pins before anything runs.
#[test]
fn pinned_tools_are_verified() {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools");
    let root = std::env::temp_dir().join(format!("apollo-resolver-test-{}", random_hex(6)));
    let sink: EventSink = Arc::new(|_| {});
    // Wrong pin: must refuse, and say so.
    let mut cfg = config(&root, "ok", &root.join("l"), &root.join("p"));
    cfg.ytdlp = tools.join("yt-dlp").join("yt-dlp.exe");
    cfg.deno = tools.join("deno").join("deno.exe");
    cfg.pinned = Some(PinnedTools {
        ytdlp_sha256: "00".repeat(32),
        deno_sha256: "00".repeat(32),
    });
    if cfg.ytdlp.exists() && cfg.deno.exists() {
        let err = YtDlpResolver::new(cfg, LeaseStore::new(), sink.clone(), TestSession::new(true))
            .err()
            .expect("must refuse a mismatching binary");
        assert!(err.to_string().contains("pinned SHA-256"), "{err}");

        // The real M0 baseline tools match their recorded pins.
        let cfg = ResolverConfig::production(&tools, root.join("run")).legacy();
        assert_eq!(cfg.timeout, Duration::from_secs(30));
        YtDlpResolver::new(cfg, LeaseStore::new(), sink, TestSession::new(true))
            .expect("M0 baseline yt-dlp/deno must match their pins");
    } else {
        eprintln!("NOT RUN: tools/yt-dlp and tools/deno not present");
    }
    let _ = fs::remove_dir_all(&root);
}

// TEST-23: canary scan. A cookie canary and a URL canary go through every path; none may show up
// in argv, environment, diagnostics, Debug output of events/leases, or files left on disk.
#[test]
fn canary_secrets_never_leak() {
    let mut surfaces: Vec<String> = Vec::new();
    for mode in ["ok", "rotate", "fail-bot", "fail-429", "wrong-id", "drm", "garbage"] {
        let mut r = rig(mode);
        r.start(0);
        let (load, res) = r.result(Duration::from_secs(20)).expect(mode);
        surfaces.push(format!("{load:?} {res:?}"));
        if let Ok(id) = res {
            let lease = r.leases.get_fresh(id, unix_now(), MARGIN, MAXAGE).unwrap();
            surfaces.push(format!("{lease:?}"));
        }
        surfaces.push(r.rs().diagnostics().join("\n"));
        for l in r
            .log_text()
            .lines()
            .filter(|l| l.starts_with("ARGS ") || l.starts_with("ENV "))
        {
            surfaces.push(l.to_string());
        }
        // Files under the run dir (cookie dirs must be empty; scratch holds nothing of ours).
        assert!(
            wait_until(Duration::from_secs(2), || r.cookie_dirs().is_empty()),
            "{mode}"
        );
        let mut stack = vec![r.root.join("run")];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(t) = fs::read_to_string(&p) {
                    surfaces.push(t);
                }
            }
        }
    }
    let encoded = COOKIE_CANARY.replace('-', "%2D");
    for s in &surfaces {
        for canary in [COOKIE_CANARY, encoded.as_str(), "URLCANARY", "HEADERCANARY"] {
            // yt-dlp's own stdout holds the URL/header canaries; we parse and drop it.
            assert!(
                !s.contains(canary),
                "canary {canary} leaked in: {}",
                &s[..s.len().min(80)]
            );
        }
    }
}

/// Opt-in, needs internet: the REAL pinned yt-dlp + Deno through the real contained runner
/// (cleared environment, Job Object, private cookie dir), with PLACEHOLDER cookie values and a
/// public video. Proves the process machinery works with the real PyInstaller one-file exe, not
/// only with the fake. Not an account test: yt-dlp resolves as a guest here (M0 finding).
///   cargo test -p apollo-resolver --features test-support --test resolver -- --ignored --nocapture
#[test]
#[ignore = "needs internet and the real tools"]
fn real_ytdlp_through_the_contained_runner_with_placeholder_cookies() {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools");
    if !tools.join("yt-dlp").join("yt-dlp.exe").exists() {
        eprintln!("NOT RUN: tools not installed");
        return;
    }
    let root = std::env::temp_dir().join(format!("apollo-resolver-test-{}", random_hex(6)));
    fs::create_dir_all(&root).unwrap();
    let cfg = ResolverConfig::production(&tools, root.join("run")).legacy();
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let Event::Resolved { load, result } = ev {
            let _ = tx.lock().unwrap().send((load, result));
        }
    });
    let leases = LeaseStore::new();
    let session = TestSession::new(true); // placeholder cookie values, not a real account
    let mut resolver = YtDlpResolver::new(cfg, leases.clone(), sink, session as Arc<dyn SessionProvider>)
        .expect("pinned tools verified");

    let mut c = Coordinator::default();
    let effects = c.handle(Event::PlayContext {
        tracks: vec![Track::new("dQw4w9WgXcQ", "t"), Track::new("jNQXAC9IVRw", "t")],
        start: 0,
    });
    let Effect::Resolve { load, entry, track, resume_ms } = effects
        .iter()
        .find(|e| matches!(e, Effect::Resolve { .. }))
        .cloned()
        .expect("resolve effect")
    else {
        unreachable!()
    };
    let t0 = Instant::now();
    resolver.start(ResolveRequest { load, entry, track, resume_ms, preload: false });
    let (_, res) = rx.recv_timeout(Duration::from_secs(60)).expect("a result within 60 s");
    eprintln!(
        "REAL yt-dlp via contained runner: {:?} in {:.1}s; diag: {:?}",
        res.as_ref().map(|_| "lease"),
        t0.elapsed().as_secs_f64(),
        resolver.diagnostics()
    );
    let id = res.expect("real yt-dlp must resolve through the contained runner");
    let lease = leases.get_fresh(id, unix_now(), MARGIN, MAXAGE).expect("fresh lease");
    assert!(lease.url().host_str().unwrap().ends_with(".googlevideo.com"));
    assert_eq!(lease.url().scheme(), "https");
    assert!(lease.expires_at_unix.is_some(), "googlevideo URLs carry an expiry");
    // Nothing left behind and no stray helper processes of the real kind.
    drop(resolver);
    let leftovers = fs::read_dir(root.join("run").join("cookies")).map(|r| r.count()).unwrap_or(0);
    assert_eq!(leftovers, 0);
    let _ = fs::remove_dir_all(&root);
}

// An unconfirmed session (cookies present, service not yet confirmed) is not a "sign in again"
// case: report a network-class failure, and still never start a process.
#[test]
fn an_unconfirmed_session_fails_as_network_not_as_expired() {
    let session = TestSession::new(false);
    session.unconfirmed.store(true, Ordering::SeqCst);
    let mut r = rig_with("ok", session, |_| {});
    r.start(0);
    let (_, res) = r.result(Duration::from_secs(5)).expect("a result");
    assert_eq!(res.unwrap_err(), Failure::Network);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!r.log.exists(), "yt-dlp was started for an unconfirmed session");
    assert!(r.rs().diagnostics().join("\n").contains("session-unconfirmed"));
}
