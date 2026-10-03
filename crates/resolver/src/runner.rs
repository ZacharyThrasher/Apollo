//! Runs one helper process under containment with bounded pipes, a timeout and cancellation.
//! Windows only: the whole process tree lives in a Job Object, so a kill, a timeout, a cancel
//! or even a crash of this process leaves no orphan behind.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apollo_services_common::win::{spawn_in_job, JobObject};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Exited(i32),
    TimedOut,
    Cancelled,
    OutputTooLarge,
    SpawnFailed,
}

pub struct RunOutput {
    pub status: RunStatus,
    pub stdout: Vec<u8>,
    /// Kept only so the caller can classify it; never printed or logged.
    pub stderr: String,
    #[allow(dead_code)]
    pub elapsed: Duration,
}

/// Shared with `cancel()` so a cancel can kill the tree immediately, from another thread.
#[derive(Default)]
pub struct KillSwitch {
    pub cancelled: AtomicBool,
    job: Mutex<Option<Arc<JobObject>>>,
}

impl KillSwitch {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(job) = self.job.lock().unwrap().as_ref() {
            job.terminate();
        }
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

const STDERR_CAP: usize = 64 * 1024;

pub fn run_contained(
    mut cmd: Command,
    timeout: Duration,
    max_stdout: usize,
    switch: &KillSwitch,
) -> RunOutput {
    let started = Instant::now();
    let fail = |status| RunOutput {
        status,
        stdout: Vec::new(),
        stderr: String::new(),
        elapsed: started.elapsed(),
    };
    if switch.is_cancelled() {
        return fail(RunStatus::Cancelled);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let job = match JobObject::new_kill_on_close() {
        Ok(j) => Arc::new(j),
        Err(_) => return fail(RunStatus::SpawnFailed),
    };
    *switch.job.lock().unwrap() = Some(job.clone());
    let mut child = match spawn_in_job(&mut cmd, &job) {
        Ok(c) => c,
        Err(_) => {
            *switch.job.lock().unwrap() = None;
            return fail(RunStatus::SpawnFailed);
        }
    };
    // A cancel that arrived between the check above and the job being published.
    if switch.is_cancelled() {
        job.terminate();
    }

    let overflow = Arc::new(AtomicBool::new(false));
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let (otx, orx) = mpsc::channel();
    let (etx, erx) = mpsc::channel();
    let ov = overflow.clone();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        loop {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() + n > max_stdout {
                        ov.store(true, Ordering::SeqCst);
                        // Keep draining (and discarding) so the child can be killed cleanly.
                    } else {
                        buf.extend_from_slice(&chunk[..n]);
                    }
                }
            }
        }
        let _ = otx.send(buf);
    });
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 8 * 1024];
        loop {
            match err.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if buf.len() < STDERR_CAP {
                        buf.extend_from_slice(&chunk[..n.min(STDERR_CAP - buf.len())]);
                    }
                }
            }
        }
        let _ = etx.send(String::from_utf8_lossy(&buf).into_owned());
    });

    let mut forced: Option<RunStatus> = None;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code().unwrap_or(-1),
            Ok(None) => {
                if forced.is_none() {
                    let why = if switch.is_cancelled() {
                        Some(RunStatus::Cancelled)
                    } else if started.elapsed() > timeout {
                        Some(RunStatus::TimedOut)
                    } else if overflow.load(Ordering::SeqCst) {
                        Some(RunStatus::OutputTooLarge)
                    } else {
                        None
                    };
                    if let Some(w) = why {
                        forced = Some(w);
                        job.terminate(); // the whole tree
                    }
                }
                thread::sleep(Duration::from_millis(15));
            }
            Err(_) => {
                job.terminate();
                break -1;
            }
        }
    };
    // Whatever is left in the job (grandchildren that outlived the parent) dies too.
    job.terminate();
    *switch.job.lock().unwrap() = None;
    let stdout = orx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let stderr = erx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    RunOutput {
        status: forced.unwrap_or(RunStatus::Exited(code)),
        stdout,
        stderr,
        elapsed: started.elapsed(),
    }
}
