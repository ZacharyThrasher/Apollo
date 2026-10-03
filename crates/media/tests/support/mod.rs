//! Test rig: a loopback HTTP server that serves the owned fixtures (with Range support) and
//! can misbehave on request, plus helpers to mint load ids and collect engine events.
//! Loopback URLs are only accepted because tests use `UrlPolicy::for_tests_allow_loopback()`.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use apollo_domain::lease::{AudioCodec, StreamCandidate, Transport, UrlPolicy};
use apollo_domain::model::{Track, VideoId};
use apollo_domain::playback::{Coordinator, Effect, EngineEvent, Event, LeaseId, LoadId};
use apollo_media::{MediaConfig, MpvEngine};
use apollo_services_common::{EventSink, LeaseStore};

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio")
}

#[derive(Clone, Debug)]
pub struct Req {
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl Req {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

pub struct TestServer {
    pub addr: SocketAddr,
    pub log: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (l2, s2) = (log.clone(), stop.clone());
        let thread = thread::spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        let (l3, s3) = (l2.clone(), s2.clone());
                        thread::spawn(move || {
                            let _ = handle(conn, l3, s3);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(10)),
                }
            }
        });
        Self {
            addr,
            log,
            stop,
            thread: Some(thread),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    pub fn requests(&self) -> Vec<Req> {
        self.log.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_request(conn: &mut TcpStream) -> Option<(String, Vec<(String, String)>)> {
    conn.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = conn.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 64 * 1024 {
            return None;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    let path = first.split(' ').nth(1)?.to_string();
    let headers = lines
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    Some((path, headers))
}

fn respond(conn: &mut TcpStream, status: &str, extra: &[(&str, String)], body: &[u8]) {
    let mut head = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
    for (n, v) in extra {
        head.push_str(&format!("{n}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = conn.write_all(head.as_bytes());
    let _ = conn.write_all(body);
}

fn handle(
    mut conn: TcpStream,
    log: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
) -> Option<()> {
    let (path, headers) = read_request(&mut conn)?;
    log.lock().unwrap().push(Req {
        path: path.clone(),
        headers: headers.clone(),
    });
    let range = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("range"))
        .and_then(|(_, v)| v.strip_prefix("bytes="))
        .and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((a.parse::<usize>().ok()?, b.parse::<usize>().ok()))
        });

    if let Some(code) = path.strip_prefix("/status/") {
        let code: u16 = code.parse().ok()?;
        respond(&mut conn, &format!("{code} X"), &[], b"nope");
        return Some(());
    }
    if let Some(to) = path.strip_prefix("/redirect?to=") {
        respond(&mut conn, "302 Found", &[("Location", to.to_string())], b"");
        return Some(());
    }
    if path == "/garbage" {
        respond(
            &mut conn,
            "200 OK",
            &[("Content-Length", "4096".into())],
            &vec![0x5au8; 4096],
        );
        return Some(());
    }
    if path == "/truncated" {
        // Promise far more than we send, then hang up.
        let data = std::fs::read(fixtures_dir().join("tone_aac.m4a")).ok()?;
        let promised = data.len();
        respond(
            &mut conn,
            "200 OK",
            &[("Content-Length", promised.to_string())],
            &data[..2000],
        );
        return Some(());
    }
    if path == "/hang" {
        let t0 = Instant::now();
        while !stop.load(Ordering::Relaxed) && t0.elapsed() < Duration::from_secs(60) {
            thread::sleep(Duration::from_millis(50));
        }
        return Some(());
    }
    // /slow/<ms>/<file> delays, then serves the file.
    let (delay_ms, file) = match path.strip_prefix("/slow/") {
        Some(rest) => {
            let (ms, f) = rest.split_once('/')?;
            (ms.parse::<u64>().ok()?, f.to_string())
        }
        None => (0, path.trim_start_matches('/').to_string()),
    };
    if delay_ms > 0 {
        thread::sleep(Duration::from_millis(delay_ms));
    }
    let data = match std::fs::read(fixtures_dir().join(&file)) {
        Ok(d) => d,
        Err(_) => {
            respond(&mut conn, "404 Not Found", &[], b"");
            return Some(());
        }
    };
    let ctype = if file.ends_with(".m4a") {
        "audio/mp4"
    } else if file.ends_with(".webm") {
        "audio/webm"
    } else {
        "audio/ogg"
    };
    match range {
        Some((start, end)) if start < data.len() => {
            let end = end.unwrap_or(data.len() - 1).min(data.len() - 1);
            let body = &data[start..=end];
            respond(
                &mut conn,
                "206 Partial Content",
                &[
                    ("Content-Type", ctype.into()),
                    ("Accept-Ranges", "bytes".into()),
                    ("Content-Length", body.len().to_string()),
                    ("Content-Range", format!("bytes {start}-{end}/{}", data.len())),
                ],
                body,
            );
        }
        _ => respond(
            &mut conn,
            "200 OK",
            &[
                ("Content-Type", ctype.into()),
                ("Accept-Ranges", "bytes".into()),
                ("Content-Length", data.len().to_string()),
            ],
            &data,
        ),
    }
    Some(())
}

// ------------------------------------------------------------------- the rig

pub const VID: &str = "abc123DEF_-";

pub struct Rig {
    pub engine: MpvEngine,
    pub leases: LeaseStore,
    pub server: TestServer,
    pub events: Receiver<(LoadId, EngineEvent)>,
    pub loads: Vec<LoadId>,
}

/// Distinct, real `LoadId`s (their constructor is private to the domain crate, so mint them
/// the way the app does: through the coordinator).
pub fn mint_loads(n: usize) -> Vec<LoadId> {
    let mut c = Coordinator::default();
    let tracks: Vec<Track> = (0..n + 1).map(|i| Track::new(format!("t{i}"), "t")).collect();
    let mut out = Vec::new();
    let mut effects = c.handle(Event::PlayContext { tracks, start: 0 });
    loop {
        for e in &effects {
            if let Effect::Resolve { load, .. } = e {
                out.push(*load);
            }
        }
        if out.len() >= n {
            break;
        }
        effects = c.handle(Event::Next);
    }
    out.truncate(n);
    out
}

pub fn rig() -> Rig {
    rig_with(MediaConfig {
        audio_output: Some("null".into()),
        ..Default::default()
    })
}

pub fn rig_with(cfg: MediaConfig) -> Rig {
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let Event::Engine { load, event } = ev {
            let _ = tx.lock().unwrap().send((load, event));
        }
    });
    let leases = LeaseStore::new();
    let engine = MpvEngine::new(cfg, leases.clone(), sink).expect("engine");
    Rig {
        engine,
        leases,
        server: TestServer::start(),
        events: rx,
        loads: mint_loads(24),
    }
}

pub fn candidate(url: String, headers: Vec<(&str, &str)>, expires: Option<u64>) -> StreamCandidate {
    StreamCandidate {
        video_id: VID.into(),
        url,
        transport: Transport::Https,
        has_audio: true,
        drm: false,
        codec: Some(AudioCodec::Opus),
        bitrate_kbps: Some(128),
        sample_rate_hz: Some(48_000),
        duration: Some(Duration::from_secs(12)),
        expires_at_unix: expires,
        headers: headers
            .into_iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect(),
    }
}

impl Rig {
    pub fn lease_url(&self, url: String) -> LeaseId {
        self.lease_full(url, vec![], None)
    }

    pub fn lease_full(
        &self,
        url: String,
        headers: Vec<(&str, &str)>,
        expires: Option<u64>,
    ) -> LeaseId {
        let lease = UrlPolicy::for_tests_allow_loopback()
            .validate(&VideoId::new(VID), candidate(url, headers, expires))
            .expect("loopback lease");
        self.leases.insert(lease)
    }

    pub fn lease_path(&self, path: &str) -> LeaseId {
        self.lease_url(self.server.url(path))
    }

    /// Collect events for `load` until `done` is true for one of them. Events of other loads
    /// are returned too, tagged, so tests can assert nothing leaked.
    pub fn collect_until(
        &self,
        timeout: Duration,
        done: impl Fn(LoadId, &EngineEvent) -> bool,
    ) -> Result<Vec<(LoadId, EngineEvent)>, Vec<(LoadId, EngineEvent)>> {
        let t0 = Instant::now();
        let mut all = Vec::new();
        while t0.elapsed() < timeout {
            match self.events.recv_timeout(Duration::from_millis(50)) {
                Ok((l, e)) => {
                    let stop = done(l, &e);
                    all.push((l, e));
                    if stop {
                        return Ok(all);
                    }
                }
                Err(_) => {}
            }
        }
        Err(all)
    }

    /// Everything that arrives within `window`.
    pub fn drain_for(&self, window: Duration) -> Vec<(LoadId, EngineEvent)> {
        let t0 = Instant::now();
        let mut all = Vec::new();
        while t0.elapsed() < window {
            if let Ok(x) = self.events.recv_timeout(Duration::from_millis(25)) {
                all.push(x);
            }
        }
        all
    }
}
