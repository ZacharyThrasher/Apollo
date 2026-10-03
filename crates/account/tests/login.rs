#![cfg(windows)]
//! Login handoff tests with a fake helper: the real process containment, anonymous-pipe
//! transport, bounded output, cancellation and profile cleanup are exercised; only the
//! WebView2/Google part is faked (that part is owner-run).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apollo_account::identity::{AccountValidator, Identity, Validation};
use apollo_account::jar::CookieJar;
use apollo_account::login::{run_login, LoginCancel, LoginConfig, LoginError};
use apollo_account::service::{AccountService, ImportOutcome, SessionState};
use apollo_account::store::EncryptedStore;
use apollo_account::vault::MemoryVault;
use apollo_services_common::{random_hex, SessionProvider};

const FAKE: &str = env!("CARGO_BIN_EXE_apollo-fake-login-helper");
const CANARY: &str = "LOGINCANARY";

struct Rig {
    root: PathBuf,
    log: PathBuf,
    pidfile: PathBuf,
}

impl Rig {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("apollo-login-test-{}", random_hex(6)));
        fs::create_dir_all(&root).unwrap();
        Self {
            log: root.join("helper.log"),
            pidfile: root.join("pids.txt"),
            root,
        }
    }
    fn profiles(&self) -> PathBuf {
        self.root.join("profiles")
    }
    fn cfg(&self, mode: &str) -> LoginConfig {
        let mut c = LoginConfig::new(PathBuf::from(FAKE), self.profiles());
        c.timeout = Duration::from_secs(30);
        c.grace = Duration::from_secs(1);
        c.extra_env = vec![
            ("APOLLO_FAKE_LOGIN_MODE".into(), mode.into()),
            ("APOLLO_FAKE_LOGIN_LOG".into(), self.log.display().to_string()),
            ("APOLLO_FAKE_LOGIN_PIDFILE".into(), self.pidfile.display().to_string()),
        ];
        c
    }
    fn profile_entries(&self) -> usize {
        fs::read_dir(self.profiles()).map(|r| r.count()).unwrap_or(0)
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
        thread::sleep(Duration::from_millis(40));
    }
    f()
}

struct AlwaysValid;
impl AccountValidator for AlwaysValid {
    fn validate(&self, _: &CookieJar) -> Validation {
        Validation::Valid {
            identities: vec![Identity {
                name: "Owner".into(),
                handle: None,
                page_id: Some("PAGE0".into()),
                selected_by_service: true,
            }],
        }
    }
}

// The whole handoff: helper -> anonymous pipe -> scoped jar -> validated by the service.
#[test]
fn cookies_travel_only_on_the_pipe_and_feed_the_account_service() {
    let r = Rig::new();
    let jar = run_login(&r.cfg("ok"), &LoginCancel::new()).expect("login");
    assert_eq!(jar.len(), 4, "the out-of-scope tracker cookie was dropped: {:?}", jar.names());
    assert!(jar.has_auth_cookies());
    assert_eq!(jar.get("SAPISID").unwrap().value, "LOGINCANARY-sapisid");
    assert!(jar.get("__Secure-3PSID").unwrap().expires_unix.is_none(), "session cookie stays session");

    // The helper saw a private profile dir that is gone afterwards, nested files included.
    let log = r.log_text();
    assert!(log.contains("PROFILE path="), "{log}");
    assert_eq!(r.profile_entries(), 0, "throwaway WebView2 profile must be deleted");
    // Nothing secret in argv or environment (the helper emits cookies on stdout only).
    for l in log.lines().filter(|l| l.starts_with("ARGS ") || l.starts_with("ENV ")) {
        assert!(!l.contains(CANARY), "cookie value found in argv/env");
    }
    assert!(log.lines().next().unwrap().contains("--profile-dir"));

    // Hand it to the service: validated, stored encrypted, usable by the resolver.
    let dir = r.root.join("store");
    let store = EncryptedStore::new(Arc::new(MemoryVault::new()), &dir).unwrap();
    let svc = AccountService::new(Arc::new(AlwaysValid), Some(store));
    assert_eq!(svc.import(jar, true), ImportOutcome::Valid);
    assert_eq!(svc.view().session, SessionState::Valid);
    assert!(svc.snapshot().is_some());
}

// TEST-16: helper crash -> clean error, no leaked stderr, no leftovers.
#[test]
fn a_crashing_helper_is_reported_cleanly_without_leaking_or_leaving_files() {
    let r = Rig::new();
    let err = run_login(&r.cfg("crash"), &LoginCancel::new()).unwrap_err();
    assert_eq!(err, LoginError::HelperFailed(101));
    let shown = format!("{err} {err:?}");
    assert!(!shown.contains(CANARY), "helper stderr leaked into the error: {shown}");
    assert_eq!(r.profile_entries(), 0);
}

#[test]
fn user_close_helper_timeout_and_bad_output_have_distinct_errors() {
    for (mode, want) in [
        ("closed", LoginError::ClosedByUser),
        ("helper-timeout", LoginError::TimedOut),
        ("garbage", LoginError::BadOutput),
        ("empty", LoginError::BadOutput),
    ] {
        let r = Rig::new();
        assert_eq!(run_login(&r.cfg(mode), &LoginCancel::new()).unwrap_err(), want, "{mode}");
        assert_eq!(r.profile_entries(), 0, "{mode}");
    }
}

#[test]
fn missing_helper_is_reported() {
    let r = Rig::new();
    let mut c = r.cfg("ok");
    c.helper_exe = r.root.join("nope.exe");
    assert_eq!(run_login(&c, &LoginCancel::new()).unwrap_err(), LoginError::HelperMissing);
}

// Output is bounded: a helper flooding stdout is killed, not buffered without limit.
#[test]
fn flooding_output_is_cut_off() {
    let r = Rig::new();
    let t0 = Instant::now();
    assert_eq!(run_login(&r.cfg("big"), &LoginCancel::new()).unwrap_err(), LoginError::BadOutput);
    assert!(t0.elapsed() < Duration::from_secs(15), "{:?}", t0.elapsed());
    assert_eq!(r.profile_entries(), 0);
}

// TEST-16 / TEST-28: cancelling a sign-in kills the helper and its children and cleans up.
#[test]
fn cancel_kills_the_helper_tree_and_removes_the_profile() {
    let r = Rig::new();
    let cancel = LoginCancel::new();
    let (cfg, c2) = (r.cfg("hang-tree"), cancel.clone());
    let t = thread::spawn(move || run_login(&cfg, &c2));
    assert!(wait_until(Duration::from_secs(10), || r.pids().len() == 2), "helper never started");
    let pids = r.pids();
    let t0 = Instant::now();
    cancel.cancel();
    assert_eq!(t.join().unwrap().unwrap_err(), LoginError::Cancelled);
    assert!(t0.elapsed() < Duration::from_secs(5));
    assert!(wait_until(Duration::from_secs(5), || pids.iter().all(|p| !alive(*p))), "orphan survived");
    assert_eq!(r.profile_entries(), 0);
}

// An overlong sign-in is abandoned by the parent even if the helper never exits.
#[test]
fn parent_enforces_the_timeout_on_a_stuck_helper() {
    let r = Rig::new();
    let mut c = r.cfg("hang-tree");
    c.timeout = Duration::from_secs(1);
    c.grace = Duration::from_secs(1);
    let t0 = Instant::now();
    assert_eq!(run_login(&c, &LoginCancel::new()).unwrap_err(), LoginError::TimedOut);
    assert!(t0.elapsed() < Duration::from_secs(10));
    let pids = r.pids();
    assert!(wait_until(Duration::from_secs(5), || pids.iter().all(|p| !alive(*p))));
    assert_eq!(r.profile_entries(), 0);
}

// TEST-23 (login half): canary scan over everything the caller can observe.
#[test]
fn canaries_do_not_leak_through_debug_display_or_files() {
    let r = Rig::new();
    let jar = run_login(&r.cfg("ok"), &LoginCancel::new()).unwrap();
    let shown = format!("{jar:?} {:?}", jar.cookies);
    assert!(!shown.contains(CANARY), "{shown}");
    for mode in ["crash", "garbage", "closed"] {
        let e = run_login(&Rig::new().cfg(mode), &LoginCancel::new()).unwrap_err();
        assert!(!format!("{e} {e:?}").contains(CANARY));
    }
    // No file anywhere under the rig holds a cookie value (profile dir was the only candidate).
    let mut stack = vec![r.root.clone()];
    let seen = Mutex::new(0usize);
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).into_iter().flatten().flatten() {
            let p: &Path = &e.path();
            if p.is_dir() {
                stack.push(p.to_path_buf());
            } else if p != r.log && p != r.pidfile {
                *seen.lock().unwrap() += 1;
                let text = fs::read_to_string(p).unwrap_or_default();
                assert!(!text.contains("LOGINCANARY-"), "cookie value on disk in {}", p.display());
            }
        }
    }
}

/// Opt-in: the REAL WebView2 helper (a window opens for a few seconds on this PC). Cancelled
/// before any sign-in; proves the throwaway profile is removed even though WebView2's child
/// processes still hold files for a moment after the helper dies (found in the first real run:
/// the profile was left behind).
///   cargo test -p apollo-account --test login -- --ignored --nocapture
#[test]
#[ignore = "opens the real sign-in window briefly"]
fn real_helper_cancelled_leaves_no_profile_behind() {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug/apollo-login-helper.exe");
    if !helper.exists() {
        eprintln!("NOT RUN: build apollo-login-helper first");
        return;
    }
    let r = Rig::new();
    let cfg = LoginConfig::new(helper, r.profiles());
    let cancel = LoginCancel::new();
    let c2 = cancel.clone();
    let t = thread::spawn(move || {
        thread::sleep(Duration::from_secs(6)); // let WebView2 create its profile and processes
        c2.cancel();
    });
    let err = run_login(&cfg, &cancel).unwrap_err();
    t.join().unwrap();
    assert_eq!(err, LoginError::Cancelled);
    assert_eq!(r.profile_entries(), 0, "WebView2 profile left behind after cancel");
}
