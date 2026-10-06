//! Warm workers, the resolved-URL cache and prefetch, against the fake yt-dlp's `apollo-serve:`
//! mode (the real protocol over real pipes and processes). Runs on every platform.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_domain::lease::UrlPolicy;
use apollo_domain::model::Track;
use apollo_domain::playback::{Coordinator, Effect, Event, Failure, LeaseId, LoadId};
use apollo_resolver::{CacheConfig, ResolverConfig, WarmConfig, YtDlpResolver};
use apollo_runtime::{ResolveRequest, Resolver};
use apollo_services_common::{
    random_hex, unix_now, EventSink, LeaseStore, SecretString, SessionProvider, SessionSnapshot,
    SessionUnavailable,
};

const FAKE: &str = env!("CARGO_BIN_EXE_apollo-fake-ytdlp");
const COOKIE_CANARY: &str = "COOKIECANARY-0123456789abcdef";

// ------------------------------------------------------------------ test session

struct TestSession {
    has_session: Mutex<bool>,
    epoch: AtomicU64,
    rotations: Mutex<Vec<(u64, bool)>>,
}

impl TestSession {
    fn new(has_session: bool) -> Arc<Self> {
        Arc::new(Self {
            has_session: Mutex::new(has_session),
            epoch: AtomicU64::new(5),
            rotations: Mutex::new(Vec::new()),
        })
    }
}

impl SessionProvider for TestSession {
    fn snapshot(&self) -> Option<SessionSnapshot> {
        if !*self.has_session.lock().unwrap() {
            return None;
        }
        Some(SessionSnapshot {
            epoch: self.epoch.load(Ordering::SeqCst),
            cookies_netscape: SecretString::new(format!(
                "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSAPISID\t{COOKIE_CANARY}\n"
            )),
        })
    }

    fn unavailable_reason(&self) -> SessionUnavailable {
        SessionUnavailable::NoSession
    }

    fn offer_rotation(&self, epoch: u64, _cookies: SecretString) -> bool {
        let ok = epoch == self.epoch.load(Ordering::SeqCst);
        self.rotations.lock().unwrap().push((epoch, ok));
        ok
    }
}

// --------------------------------------------------------------------------- rig

struct Rig {
    resolver: Option<YtDlpResolver>,
    leases: LeaseStore,
    rx: Receiver<(LoadId, Result<LeaseId, Failure>)>,
    root: PathBuf,
    log: PathBuf,
    next: u64,
    coordinator_requests: Vec<ResolveRequest>,
}

fn mint_requests(n: usize) -> Vec<ResolveRequest> {
    let mut c = Coordinator::default();
    let tracks: Vec<Track> = (0..n + 1).map(|i| Track::new(vid(i), "t")).collect();
    let mut out = Vec::new();
    let mut effects = c.handle(Event::PlayContext { tracks, start: 0 });
    while out.len() < n {
        for e in &effects {
            if let Effect::Resolve { load, entry, track, resume_ms } = e {
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

/// The i-th test video id (11 valid characters).
fn vid(i: usize) -> String {
    format!("aaaaaaaaa{i:02}")
}

struct Opts {
    mode: &'static str,
    delay_ms: u64,
    warm: WarmConfig,
    session: Arc<TestSession>,
    ytdlp: Option<PathBuf>,
    cache: bool,
}

impl Opts {
    fn new(mode: &'static str) -> Self {
        Self {
            mode,
            delay_ms: 0,
            warm: WarmConfig {
                max_workers: 2,
                background_limit: 1,
                prewarm: 0,
                startup_timeout: Duration::from_secs(10),
                request_timeout: Duration::from_secs(10),
                youtube_args: Vec::new(),
                foreground_youtube_args: None,
                watch_url: false,
                foreground_watch_url: false,
            },
            session: TestSession::new(true),
            ytdlp: None,
            cache: true,
        }
    }
}

fn rig(o: Opts) -> Rig {
    let root = std::env::temp_dir().join(format!("apollo-warm-test-{}", random_hex(6)));
    fs::create_dir_all(&root).unwrap();
    let log = root.join("fake.log");
    let cfg = ResolverConfig {
        ytdlp: o.ytdlp.clone().unwrap_or_else(|| PathBuf::from(FAKE)),
        deno: PathBuf::from("/nonexistent/deno"),
        run_dir: root.join("run"),
        timeout: Duration::from_secs(30),
        max_concurrent: 2,
        max_stdout_bytes: 1024 * 1024,
        url_policy: UrlPolicy::production(),
        warm: Some(o.warm.clone()),
        cache: o.cache.then(CacheConfig::default),
        pinned: None,
        extra_env: vec![
            ("APOLLO_FAKE_MODE".into(), o.mode.into()),
            ("APOLLO_FAKE_DELAY_MS".into(), o.delay_ms.to_string()),
            ("APOLLO_FAKE_LOG".into(), log.display().to_string()),
            ("APOLLO_FAKE_FLAG".into(), root.join("flag").display().to_string()),
        ],
    };
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let Event::Resolved { load, result } = ev {
            let _ = tx.lock().unwrap().send((load, result));
        }
    });
    let leases = LeaseStore::new();
    let resolver = YtDlpResolver::new(cfg, leases.clone(), sink, o.session.clone() as Arc<dyn SessionProvider>)
        .expect("resolver");
    Rig {
        resolver: Some(resolver),
        leases,
        rx,
        root,
        log,
        next: 0,
        coordinator_requests: mint_requests(12),
    }
}

impl Rig {
    fn rs(&mut self) -> &mut YtDlpResolver {
        self.resolver.as_mut().expect("resolver")
    }

    /// A request to play video `i` (a new load each time, like a new click).
    fn request(&mut self, i: usize) -> ResolveRequest {
        let mut r = self.coordinator_requests[i].clone();
        r.load = LoadId::for_tests(1000 + self.next);
        self.next += 1;
        r
    }

    fn start(&mut self, i: usize) -> LoadId {
        let r = self.request(i);
        let load = r.load;
        self.rs().start(r);
        load
    }

    fn result(&self, timeout: Duration) -> Option<(LoadId, Result<LeaseId, Failure>)> {
        self.rx.recv_timeout(timeout).ok()
    }

    fn log_text(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn count(&self, needle: &str) -> usize {
        self.log_text().lines().filter(|l| l.contains(needle)).count()
    }

    fn diag(&mut self) -> String {
        self.rs().diagnostics().join("\n")
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        drop(self.resolver.take());
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < timeout {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    f()
}

const WAIT: Duration = Duration::from_secs(20);

// ------------------------------------------------------------------------- tests

#[test]
fn one_worker_serves_many_tracks_and_leases_are_valid() {
    let mut r = rig(Opts::new("ok"));
    for i in 0..3 {
        let load = r.start(i);
        let (got, res) = r.result(WAIT).expect("result");
        assert_eq!(got, load);
        let id = res.expect("lease");
        r.leases
            .get_fresh(id, unix_now(), Duration::from_secs(60), Duration::from_secs(300))
            .expect("fresh, validated lease in the store");
    }
    assert_eq!(r.count("SERVE START"), 1, "the same worker handled all three");
    assert_eq!(r.count("SERVE RESOLVE START"), 3);
    let d = r.diag();
    assert!(d.contains("src=warm"), "{d}");
    assert!(d.contains("phases=Downloading_webpage@0"), "{d}");
}

#[test]
fn noise_on_the_worker_channel_does_not_break_the_protocol() {
    // The fake prints a non-JSON banner before its ready message, like yt-dlp's own output.
    let mut r = rig(Opts::new("ok"));
    r.start(0);
    assert!(r.result(WAIT).expect("result").1.is_ok());
}

#[test]
fn a_second_request_for_the_same_track_comes_from_the_cache() {
    let mut r = rig(Opts::new("ok"));
    r.start(0);
    let first = r.result(WAIT).unwrap().1.unwrap();
    let t0 = Instant::now();
    r.start(0);
    let second = r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(first, second, "the very same lease is reused");
    assert!(t0.elapsed() < Duration::from_millis(500), "{:?}", t0.elapsed());
    assert_eq!(r.count("SERVE RESOLVE START"), 1, "no second resolution");
    assert!(r.diag().contains("src=cache"));
    assert_eq!(r.rs().cached(), 1);
}

#[test]
fn without_the_cache_every_request_resolves() {
    let mut o = Opts::new("ok");
    o.cache = false;
    let mut r = rig(o);
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(r.count("SERVE RESOLVE START"), 2);
}

#[test]
fn prefetch_then_click_shares_one_resolution() {
    let mut o = Opts::new("ok");
    o.delay_ms = 400;
    let mut r = rig(o);
    let track = r.coordinator_requests[2].track.clone();
    r.rs().prefetch(&track);
    std::thread::sleep(Duration::from_millis(100)); // the prefetch is under way
    let load = r.start(2);
    let (got, res) = r.result(WAIT).expect("result");
    assert_eq!(got, load);
    res.expect("lease");
    assert_eq!(r.count("SERVE RESOLVE START"), 1, "the click joined the prefetch");
    assert!(wait_until(Duration::from_secs(2), || r.diag().contains("src=joined")));
}

#[test]
fn prefetch_alone_produces_no_event_but_fills_the_cache() {
    let mut r = rig(Opts::new("ok"));
    let track = r.coordinator_requests[1].track.clone();
    r.rs().prefetch(&track);
    assert!(wait_until(WAIT, || r.rs().cached() == 1));
    assert!(r.result(Duration::from_millis(200)).is_none(), "no Resolved event for a prefetch");
    // Asking again is a no-op: already cached.
    r.rs().prefetch(&track);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(r.count("SERVE RESOLVE START"), 1);
    // The click is instant.
    let t0 = Instant::now();
    r.start(1);
    r.result(WAIT).unwrap().1.unwrap();
    assert!(t0.elapsed() < Duration::from_millis(500));
}

#[test]
fn speculative_work_never_takes_the_last_worker() {
    let mut o = Opts::new("ok");
    o.delay_ms = 700;
    o.warm.max_workers = 2;
    o.warm.background_limit = 2; // capped to max_workers - 1
    let mut r = rig(o);
    for i in 0..4 {
        let t = r.coordinator_requests[i].track.clone();
        r.rs().prefetch(&t);
    }
    std::thread::sleep(Duration::from_millis(150)); // one prefetch holds a worker
    let t0 = Instant::now();
    r.start(9);
    let (_, res) = r.result(WAIT).expect("result");
    res.expect("lease");
    let waited = t0.elapsed();
    assert!(
        waited < Duration::from_millis(1700),
        "a click must not queue behind prefetches: {waited:?}"
    );
}

#[test]
fn a_cancelled_load_that_never_got_a_worker_is_abandoned() {
    let mut o = Opts::new("ok");
    o.delay_ms = 500;
    o.warm.max_workers = 1;
    o.warm.background_limit = 1;
    let mut r = rig(o);
    let a = r.start(0); // takes the only worker
    std::thread::sleep(Duration::from_millis(150));
    let b = r.start(1); // waits for it
    r.rs().cancel(b);
    let c = r.start(2);
    let mut got = Vec::new();
    while let Some((load, res)) = r.result(Duration::from_secs(5)) {
        got.push((load, res.is_ok()));
        if got.len() == 2 {
            break;
        }
    }
    assert!(got.contains(&(a, true)), "{got:?}");
    assert!(got.contains(&(c, true)), "{got:?}");
    assert!(!got.iter().any(|(l, _)| *l == b), "the cancelled load gets no result");
    assert_eq!(r.count(&format!("SERVE RESOLVE START {}", vid(1))), 0, "never started");
}

#[test]
fn cancelling_a_running_resolution_keeps_its_result_for_next_time() {
    let mut o = Opts::new("ok");
    o.delay_ms = 400;
    let mut r = rig(o);
    let a = r.start(0);
    std::thread::sleep(Duration::from_millis(150));
    r.rs().cancel(a);
    assert!(r.result(Duration::from_millis(900)).is_none(), "no event for the cancelled load");
    assert!(wait_until(WAIT, || r.rs().cached() == 1), "the URL was kept");
    let t0 = Instant::now();
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(r.count("SERVE RESOLVE START"), 1);
}

#[test]
fn a_worker_that_dies_is_replaced_and_the_request_retried_once() {
    let mut r = rig(Opts::new("crash-once"));
    r.start(0);
    let (_, res) = r.result(WAIT).expect("result");
    res.expect("the retry on a fresh worker succeeded");
    assert_eq!(r.count("SERVE START"), 2);
}

#[test]
fn a_worker_that_keeps_dying_is_reported_not_retried_forever() {
    let mut r = rig(Opts::new("crash-always"));
    r.start(0);
    let (_, res) = r.result(WAIT).expect("result");
    assert_eq!(res, Err(Failure::Network));
    assert!(r.diag().contains("worker-died"));
    assert_eq!(r.count("SERVE START"), 2, "one retry, no more");
}

#[test]
fn a_hung_worker_is_killed_at_the_timeout() {
    let mut o = Opts::new("hang");
    o.warm.request_timeout = Duration::from_millis(500);
    let mut r = rig(o);
    let t0 = Instant::now();
    r.start(0);
    let (_, res) = r.result(WAIT).expect("result");
    assert_eq!(res, Err(Failure::Network));
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert!(r.diag().contains("timeout"));
    assert!(wait_until(Duration::from_secs(2), || r.rs().warm_status().unwrap().0 == 0), "worker gone");
}

#[test]
fn no_session_means_no_worker_request_and_no_cached_url() {
    let session = TestSession::new(true);
    let mut o = Opts::new("ok");
    o.session = session.clone();
    let mut r = rig(o);
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    *session.has_session.lock().unwrap() = false;
    r.start(0); // would be a cache hit, but there is no session
    let (_, res) = r.result(WAIT).unwrap();
    assert_eq!(res, Err(Failure::SessionExpired));
    assert_eq!(r.count("SERVE RESOLVE START"), 1);
}

#[test]
fn a_signed_out_resolver_never_starts_a_worker_for_a_request() {
    let mut o = Opts::new("ok");
    o.session = TestSession::new(false);
    let mut r = rig(o);
    r.start(0);
    assert_eq!(r.result(WAIT).unwrap().1, Err(Failure::SessionExpired));
    assert_eq!(r.count("SERVE START"), 0);
}

#[test]
fn cookies_travel_over_the_pipe_and_never_touch_argv_env_or_disk() {
    let mut r = rig(Opts::new("ok"));
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    let log = r.log_text();
    assert!(log.contains("SERVE COOKIES lines=2"), "{log}");
    assert!(!log.contains(COOKIE_CANARY), "argv/env must not carry cookies");
    let d = r.diag();
    assert!(!d.contains(COOKIE_CANARY) && !d.contains("URLCANARY"), "{d}");
    // Nothing on disk under the run dir mentions the canary.
    fn scan(dir: &Path, canary: &str) -> bool {
        fs::read_dir(dir).into_iter().flatten().flatten().any(|e| {
            let p = e.path();
            if p.is_dir() {
                scan(&p, canary)
            } else {
                fs::read_to_string(&p).is_ok_and(|t| t.contains(canary))
            }
        })
    }
    assert!(!scan(&r.root.join("run"), COOKIE_CANARY));
}

#[test]
fn cookies_are_loaded_again_when_the_account_epoch_changes() {
    let session = TestSession::new(true);
    let mut o = Opts::new("ok");
    o.session = session.clone();
    o.cache = false;
    let mut r = rig(o);
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    r.start(1);
    r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(r.count("SERVE COOKIES"), 1, "same epoch: loaded once");
    session.epoch.fetch_add(1, Ordering::SeqCst);
    r.start(2);
    r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(r.count("SERVE COOKIES"), 2, "new epoch: new cookies");
}

#[test]
fn a_rewritten_jar_is_offered_back_under_the_epoch_the_worker_loaded() {
    let session = TestSession::new(true);
    let mut o = Opts::new("rotate");
    o.session = session.clone();
    let mut r = rig(o);
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(*session.rotations.lock().unwrap(), vec![(5, true)]);
}

#[test]
fn invalidate_forgets_urls_leases_and_ends_the_workers() {
    let mut r = rig(Opts::new("ok"));
    r.start(0);
    let id = r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(r.rs().cached(), 1);
    assert_eq!(r.rs().warm_status().unwrap().0, 1);
    r.rs().handle().invalidate();
    assert_eq!(r.rs().cached(), 0);
    assert!(r.leases.get_fresh(id, unix_now(), Duration::ZERO, Duration::from_secs(300)).is_err());
    assert!(wait_until(Duration::from_secs(2), || r.rs().warm_status().unwrap().0 == 0));
    // Signing in again works: a new worker starts on demand.
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert_eq!(r.count("SERVE START"), 2);
    assert_eq!(r.count("SERVE RESOLVE START"), 2);
}

#[test]
fn refused_candidates_are_refused_exactly_like_the_per_process_path() {
    for (mode, want) in [("wrong-id", Failure::Unsupported), ("drm", Failure::Unsupported)] {
        let mut r = rig(Opts::new(mode));
        r.start(0);
        let (_, res) = r.result(WAIT).unwrap();
        assert_eq!(res, Err(want), "{mode}");
        assert_eq!(r.rs().cached(), 0, "{mode}: nothing refused is cached");
    }
}

#[test]
fn yt_dlp_error_text_is_classified_like_the_per_process_path() {
    let mut r = rig(Opts::new("fail-bot"));
    r.start(0);
    assert_eq!(r.result(WAIT).unwrap().1, Err(Failure::SessionExpired));
}

#[test]
fn youtube_arguments_reach_the_worker_and_default_to_none() {
    let mut r = rig(Opts::new("ok"));
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert!(r.log_text().contains("SERVE INIT extractor_args=none"), "{}", r.log_text());

    let mut o = Opts::new("ok");
    o.warm = o.warm.with_youtube_arg("player_skip", &["webpage", "initial_data"]);
    let mut r = rig(o);
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert!(
        r.log_text().contains(r#"SERVE INIT extractor_args={"youtube":{"player_skip":["webpage","initial_data"]}}"#),
        "{}",
        r.log_text()
    );
}

fn skip_args(mut o: Opts) -> Opts {
    o.warm.foreground_youtube_args = Some(vec![("player_skip".into(), vec!["webpage".into()])]);
    o.warm.foreground_watch_url = true;
    o
}

#[test]
fn a_click_uses_the_fast_arguments_and_background_work_keeps_the_full_ones() {
    let mut r = rig(skip_args(Opts::new("ok")));
    let ahead = r.coordinator_requests[1].track.clone();
    r.rs().prefetch(&ahead);
    assert!(wait_until(WAIT, || r.rs().cached() == 1));
    r.start(2);
    r.result(WAIT).unwrap().1.unwrap();
    let log = r.log_text();
    assert!(log.contains(&format!("SERVE RESOLVE ARGS {} skip=false", ahead.video_id.as_str())), "{log}");
    assert_eq!(r.count("skip=true"), 1, "{log}");
    assert_eq!(r.count("site=www"), 1, "only the click uses the plain watch URL: {log}");
    assert!(r.diag().contains("src=warm-fast"), "{}", r.diag());
}

#[test]
fn a_fast_resolve_that_gives_no_stream_is_retried_once_with_the_full_arguments() {
    let mut r = rig(skip_args(Opts::new("skip-unsupported")));
    r.start(0);
    let (_, res) = r.result(WAIT).expect("result");
    res.expect("the retry succeeded");
    assert_eq!(r.count("SERVE RESOLVE START"), 2);
    assert_eq!(r.count("skip=true"), 1);
    assert_eq!(r.count("skip=false"), 1);
    assert!(r.diag().contains("retrying with the full settings"), "{}", r.diag());
}

#[test]
fn prewarmed_workers_are_ready_before_the_first_request() {
    let mut o = Opts::new("ok");
    o.warm.prewarm = 2;
    let mut r = rig(o);
    assert!(wait_until(WAIT, || r.rs().warm_status().unwrap().1 == 2), "two idle workers");
    let t0 = Instant::now();
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(r.count("SERVE START"), 2);
}

#[test]
fn workers_that_cannot_start_do_not_hang_requests() {
    let mut o = Opts::new("ok");
    o.ytdlp = Some(PathBuf::from("/nonexistent/yt-dlp"));
    let mut r = rig(o);
    let t0 = Instant::now();
    r.start(0);
    let (_, res) = r.result(WAIT).expect("an answer, not a hang");
    assert!(res.is_err());
    assert!(t0.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_worker_that_exits_at_start_is_explained_and_the_request_falls_back() {
    let mut r = rig(Opts::new("startup-fail"));
    let t0 = Instant::now();
    r.start(0);
    let (_, res) = r.result(WAIT).expect("an answer, not a hang");
    assert!(res.is_err());
    assert!(t0.elapsed() < Duration::from_secs(5));
    let d = r.diag();
    assert!(d.contains("warm workers unavailable"), "{d}");
    assert!(d.contains("Unsupported URL"), "the reason is shown: {d}");
    assert!(r.rs().warm_last_error().is_some());
}

#[test]
fn quitting_lets_idle_workers_exit_on_their_own() {
    // A worker that exits by itself cleans up after itself; a killed one cannot.
    let mut r = rig(Opts::new("ok"));
    r.start(0);
    r.result(WAIT).unwrap().1.unwrap();
    drop(r.resolver.take());
    assert!(r.log_text().contains("SERVE QUIT"), "{}", r.log_text());
}

// ------------------------------------------------------------- 429: back off, do not hammer

#[test]
fn a_429_holds_back_the_next_load_without_asking_youtube_again() {
    let mut r = rig(Opts::new("fail-429"));
    r.start(0);
    assert_eq!(r.result(WAIT).unwrap().1, Err(Failure::Throttled));
    assert_eq!(r.count("SERVE RESOLVE START"), 1);
    assert!(r.rs().cooldown_remaining().is_some());

    // The click that follows (or a Retry) is refused on the spot: no request, still the same
    // failure the player bar already knows how to say.
    r.start(1);
    assert_eq!(r.result(Duration::from_secs(2)).unwrap().1, Err(Failure::Throttled));
    assert_eq!(r.count("SERVE RESOLVE START"), 1, "no second request during the cooldown");
    let d = r.diag();
    assert!(d.contains("throttled strikes=1 wait_s=30 background_wait_s="), "{d}");
    assert!(d.contains("load held back: cooldown"), "{d}");
}

#[test]
fn a_stream_found_through_a_fallback_client_after_a_429_still_backs_off() {
    let mut r = rig(Opts::new("ok-throttled"));
    r.start(0);
    r.result(WAIT).unwrap().1.expect("the fallback client found a stream");
    assert!(r.rs().cooldown_remaining().is_some(), "yt-dlp met a 429 on the way");
}

#[test]
fn every_request_to_youtube_leaves_one_log_line_with_its_kind_and_video() {
    let mut r = rig(Opts::new("ok"));
    let latency = r.log.with_file_name("latency.log");
    r.rs().set_log_file(latency.clone());
    let ahead = r.coordinator_requests[1].track.clone();
    r.rs().prefetch(&ahead);
    assert!(wait_until(WAIT, || r.rs().cached() == 1));
    r.start(2);
    r.result(WAIT).unwrap().1.expect("lease");
    // Cached: no request, no line.
    r.rs().prefetch(&ahead);
    r.start(1);
    r.result(WAIT).unwrap().1.expect("lease");
    let text = fs::read_to_string(&latency).unwrap();
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("yt_request")).collect();
    let click = r.coordinator_requests[2].track.video_id.as_str().to_owned();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].ends_with(&format!("yt_request kind=resolve_ahead video_id={}", ahead.video_id.as_str())), "{lines:?}");
    assert!(lines[1].ends_with(&format!("yt_request kind=resolve video_id={click}")), "{lines:?}");
    assert!(!r.diag().contains("yt_request"), "kept out of the diagnostics ring");
}

#[test]
fn prefetch_stops_during_the_cooldown() {
    let mut r = rig(Opts::new("fail-429"));
    r.start(0);
    assert_eq!(r.result(WAIT).unwrap().1, Err(Failure::Throttled));
    for i in 1..4 {
        let t = r.coordinator_requests[i].track.clone();
        r.rs().prefetch(&t);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.count("SERVE RESOLVE START"), 1, "speculation asked for nothing");
    assert!(r.diag().contains("prefetch skipped reason=cooldown"), "{}", r.diag());
}

#[test]
fn a_throttle_shared_with_another_caller_holds_back_the_resolver_too() {
    let mut r = rig(Opts::new("ok"));
    let shared = apollo_services_common::Throttle::new();
    r.rs().set_throttle(shared.clone());
    shared.report_throttled(Some(Duration::from_secs(120))); // e.g. InnerTube saw 429 + Retry-After
    r.start(0);
    assert_eq!(r.result(Duration::from_secs(2)).unwrap().1, Err(Failure::Throttled));
    assert_eq!(r.count("SERVE RESOLVE START"), 0);
    assert!(r.rs().cooldown_remaining().unwrap() > Duration::from_secs(100));
}

#[test]
fn a_cached_stream_still_plays_during_the_cooldown() {
    let mut r = rig(Opts::new("ok"));
    let track = r.coordinator_requests[1].track.clone();
    r.rs().prefetch(&track);
    assert!(wait_until(WAIT, || r.rs().cached() == 1));
    r.rs().set_throttle({
        let t = apollo_services_common::Throttle::new();
        t.report_throttled(None);
        t
    });
    r.start(1);
    r.result(WAIT).unwrap().1.expect("served from the cache: no request needed");
}

#[test]
fn speculative_resolves_are_paced_even_without_a_429() {
    let mut o = Opts::new("ok");
    o.warm.max_workers = 4;
    o.warm.background_limit = 4;
    let mut r = rig(o);
    for i in 0..8 {
        let t = r.coordinator_requests[i].track.clone();
        r.rs().prefetch(&t);
        std::thread::sleep(Duration::from_millis(40));
    }
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        r.count("SERVE RESOLVE START"),
        3,
        "a burst of three, then one every six seconds: {}",
        r.diag()
    );
    assert!(r.diag().contains("prefetch skipped reason="), "{}", r.diag());
    // Playback is never rationed: a click on a fourth track still resolves at once.
    r.start(9);
    r.result(WAIT).unwrap().1.expect("lease");
}

#[test]
fn only_two_prefetches_run_at_once() {
    let mut o = Opts::new("ok");
    o.delay_ms = 600;
    o.warm.max_workers = 4;
    o.warm.background_limit = 4;
    let mut r = rig(o);
    for i in 0..3 {
        let t = r.coordinator_requests[i].track.clone();
        r.rs().prefetch(&t);
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.count("SERVE RESOLVE START"), 2, "the third was dropped: {}", r.diag());
}
