//! Sign-in handoff (ADR 0002, Windows). Launches the WebView2 helper as a separate process in a
//! Job Object. The cookies come back on the helper's **stdout, an anonymous pipe only this
//! process holds** (no named pipe, nothing for another process to connect to, nothing on disk,
//! nothing in argv). A fresh private profile directory is used per attempt and deleted
//! afterwards, so no second copy of the Google session stays behind in WebView2.
//!
//! This crate never sees the owner's password: they type it on Google's own page in the helper.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apollo_services_common::win::{spawn_in_job, JobObject, PrivateDir};
use serde::Deserialize;
use zeroize::Zeroize;

use crate::jar::{Cookie, CookieJar};

/// A sign-in that takes longer than this is abandoned (slow typing, passkey prompts...).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_OUTPUT: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct LoginConfig {
    pub helper_exe: PathBuf,
    /// Base directory for the throwaway WebView2 profile (created user-only).
    pub profile_base: PathBuf,
    pub timeout: Duration,
    /// Extra time the parent waits after the helper's own timeout before killing it.
    pub grace: Duration,
    #[cfg(feature = "test-support")]
    pub extra_env: Vec<(String, String)>,
}

impl LoginConfig {
    pub fn new(helper_exe: PathBuf, profile_base: PathBuf) -> Self {
        Self {
            helper_exe,
            profile_base,
            timeout: DEFAULT_TIMEOUT,
            grace: Duration::from_secs(20),
            #[cfg(feature = "test-support")]
            extra_env: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LoginError {
    HelperMissing,
    SpawnFailed,
    /// The owner closed the sign-in window.
    ClosedByUser,
    TimedOut,
    Cancelled,
    /// The helper exited abnormally (crash, WebView2 missing...). Carries the exit code only.
    HelperFailed(i32),
    /// The helper's output was not the expected JSON, or exceeded the size limit.
    BadOutput,
    Io,
}

impl std::fmt::Display for LoginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LoginError::HelperMissing => "sign-in helper not found".to_string(),
            LoginError::SpawnFailed => "could not start the sign-in helper".to_string(),
            LoginError::ClosedByUser => "sign-in window closed".to_string(),
            LoginError::TimedOut => "sign-in timed out".to_string(),
            LoginError::Cancelled => "sign-in cancelled".to_string(),
            LoginError::HelperFailed(c) => format!("sign-in helper failed (exit {c})"),
            LoginError::BadOutput => "sign-in helper returned unusable data".to_string(),
            LoginError::Io => "sign-in storage error".to_string(),
        };
        f.write_str(&s)
    }
}

impl std::error::Error for LoginError {}

/// Lets another thread cancel a running sign-in (kills the helper and any children).
#[derive(Default, Clone)]
pub struct LoginCancel {
    cancelled: Arc<AtomicBool>,
    job: Arc<Mutex<Option<Arc<JobObject>>>>,
}

impl LoginCancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(j) = self.job.lock().unwrap().as_ref() {
            j.terminate();
        }
    }
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

#[derive(Deserialize)]
struct Wire {
    cookies: Vec<WireCookie>,
}

#[derive(Deserialize)]
struct WireCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_unix: Option<i64>,
}

/// Run one sign-in attempt and return the scoped jar. Blocks until done, cancelled or timed out.
/// The jar is NOT yet trusted: hand it to `AccountService::import`, which validates it.
pub fn run_login(cfg: &LoginConfig, cancel: &LoginCancel) -> Result<CookieJar, LoginError> {
    if !cfg.helper_exe.exists() {
        return Err(LoginError::HelperMissing);
    }
    if cancel.is_cancelled() {
        return Err(LoginError::Cancelled);
    }
    let profile = PrivateDir::create(&cfg.profile_base, "login").map_err(|_| LoginError::Io)?;
    let result = run_helper(cfg, cancel, &profile);
    // The throwaway WebView2 profile (nested directories) is removed whatever happened.
    let _ = std::fs::remove_dir_all(profile.path());
    result
}

fn run_helper(cfg: &LoginConfig, cancel: &LoginCancel, profile: &PrivateDir) -> Result<CookieJar, LoginError> {
    let mut cmd = Command::new(&cfg.helper_exe);
    cmd.arg("--profile-dir")
        .arg(profile.path())
        .arg("--timeout-secs")
        .arg(cfg.timeout.as_secs().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(feature = "test-support")]
    for (k, v) in &cfg.extra_env {
        cmd.env(k, v);
    }
    let job = Arc::new(JobObject::new_kill_on_close().map_err(|_| LoginError::SpawnFailed)?);
    *cancel.job.lock().unwrap() = Some(job.clone());
    let mut child = match spawn_in_job(&mut cmd, &job) {
        Ok(c) => c,
        Err(_) => {
            *cancel.job.lock().unwrap() = None;
            return Err(LoginError::SpawnFailed);
        }
    };
    if cancel.is_cancelled() {
        job.terminate();
    }

    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let overflow = Arc::new(AtomicBool::new(false));
    let ov = overflow.clone();
    let (otx, orx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() + n > MAX_OUTPUT {
                        ov.store(true, Ordering::SeqCst);
                    } else {
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
        let _ = otx.send(buf);
    });
    // Drain stderr (navigation notes) so the helper can never block on a full pipe; it is not kept.
    thread::spawn(move || {
        let mut sink = [0u8; 4096];
        while matches!(err.read(&mut sink), Ok(n) if n > 0) {}
    });

    let started = Instant::now();
    // Allow the helper's own timeout to fire first, then enforce ours.
    let hard = cfg.timeout + cfg.grace;
    let mut forced: Option<LoginError> = None;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if forced.is_none() {
                    let why = if cancel.is_cancelled() {
                        Some(LoginError::Cancelled)
                    } else if started.elapsed() > hard {
                        Some(LoginError::TimedOut)
                    } else if overflow.load(Ordering::SeqCst) {
                        Some(LoginError::BadOutput)
                    } else {
                        None
                    };
                    if let Some(w) = why {
                        forced = Some(w);
                        job.terminate();
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                job.terminate();
                break -1;
            }
        }
    };
    job.terminate(); // anything the helper left running dies with it
    *cancel.job.lock().unwrap() = None;
    // `cancel()` kills the job itself, so the helper can exit (code != 0) before the loop above
    // noticed the flag. A cancel always wins over whatever exit code that produced.
    if forced.is_none() && cancel.is_cancelled() {
        forced = Some(LoginError::Cancelled);
    }
    let mut raw = orx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();

    if let Some(e) = forced {
        raw.zeroize();
        return Err(e);
    }
    match code {
        0 => {}
        2 => {
            raw.zeroize();
            return Err(LoginError::ClosedByUser);
        }
        3 => {
            raw.zeroize();
            return Err(LoginError::TimedOut);
        }
        other => {
            raw.zeroize();
            return Err(LoginError::HelperFailed(other));
        }
    }
    let parsed: Result<Wire, _> = serde_json::from_slice(&raw);
    raw.zeroize();
    let wire = parsed.map_err(|_| LoginError::BadOutput)?;
    let jar = CookieJar {
        cookies: wire
            .cookies
            .into_iter()
            .map(|c| Cookie {
                name: c.name,
                value: c.value,
                domain: c.domain,
                path: c.path,
                secure: c.secure,
                http_only: c.http_only,
                expires_unix: c.expires_unix,
            })
            .collect(),
        ephemeral_seed: false,
    }
    .scoped();
    if jar.is_empty() {
        return Err(LoginError::BadOutput);
    }
    Ok(jar)
}
