//! Opt-in: the real pinned-style yt-dlp with the real worker plugin and Deno, against real
//! YouTube, as a guest (no account). Checks the Rust <-> plugin protocol end to end and prints
//! cold/warm/cached timings.
//!
//!   APOLLO_LIVE_YTDLP=/path/to/yt-dlp APOLLO_LIVE_DENO=/path/to/deno \
//!     cargo test -p apollo-resolver --features test-support --test warm_live -- --ignored --nocapture
//!
//! Proxy/CA variables (HTTPS_PROXY, SSL_CERT_FILE, REQUESTS_CA_BUNDLE) are passed through when set.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_domain::lease::UrlPolicy;
use apollo_domain::model::Track;
use apollo_domain::playback::{Coordinator, Effect, Event};
use apollo_resolver::{CacheConfig, ResolverConfig, WarmConfig, YtDlpResolver};
use apollo_runtime::{ResolveRequest, Resolver};
use apollo_services_common::{
    random_hex, EventSink, LeaseStore, SecretString, SessionProvider, SessionSnapshot,
};

struct Guest;

impl SessionProvider for Guest {
    fn snapshot(&self) -> Option<SessionSnapshot> {
        Some(SessionSnapshot {
            epoch: 1,
            cookies_netscape: SecretString::new("# Netscape HTTP Cookie File\n".to_owned()),
        })
    }
    fn offer_rotation(&self, _epoch: u64, _cookies: SecretString) -> bool {
        false
    }
}

fn requests(ids: &[&str]) -> Vec<ResolveRequest> {
    let mut c = Coordinator::default();
    let tracks: Vec<Track> = ids.iter().map(|i| Track::new(*i, "t")).collect();
    let mut out = Vec::new();
    let mut effects = c.handle(Event::PlayContext { tracks, start: 0 });
    while out.len() < ids.len() {
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
        effects = c.handle(Event::Next);
    }
    out
}

#[test]
#[ignore = "needs yt-dlp, deno and network"]
fn real_worker_end_to_end() {
    let (Some(ytdlp), Some(deno)) = (
        std::env::var_os("APOLLO_LIVE_YTDLP"),
        std::env::var_os("APOLLO_LIVE_DENO"),
    ) else {
        eprintln!("set APOLLO_LIVE_YTDLP and APOLLO_LIVE_DENO");
        return;
    };
    let root = std::env::temp_dir().join(format!("apollo-live-{}", random_hex(4)));
    let mut extra_env = Vec::new();
    for k in ["HTTPS_PROXY", "https_proxy", "SSL_CERT_FILE", "REQUESTS_CA_BUNDLE", "NO_PROXY"] {
        if let Ok(v) = std::env::var(k) {
            extra_env.push((k.to_owned(), v));
        }
    }
    let cfg = ResolverConfig {
        ytdlp: PathBuf::from(ytdlp),
        deno: PathBuf::from(deno),
        run_dir: root.join("run"),
        timeout: Duration::from_secs(60),
        max_concurrent: 2,
        max_stdout_bytes: 8 << 20,
        url_policy: UrlPolicy::production(),
        warm: Some(WarmConfig {
            request_timeout: Duration::from_secs(60),
            prewarm: 1,
            ..WarmConfig::default()
        }),
        cache: Some(CacheConfig::default()),
        pinned: None,
        extra_env,
    };
    let (tx, rx) = mpsc::channel();
    let tx = Mutex::new(tx);
    let sink: EventSink = Arc::new(move |ev| {
        if let Event::Resolved { load, result } = ev {
            let _ = tx.lock().unwrap().send((load, result));
        }
    });
    let mut rs = YtDlpResolver::new(cfg, LeaseStore::new(), sink, Arc::new(Guest)).expect("resolver");
    let reqs = requests(&["9bZkp7q19f0", "RgKAFK5djSk", "60ItHLz5WEA"]);
    let mut ok = 0;
    for r in reqs.iter().chain(reqs.iter().take(1)) {
        let t0 = Instant::now();
        let mut r = r.clone();
        r.load = apollo_domain::playback::LoadId::for_tests(100 + ok);
        rs.start(r);
        let (_, res) = rx.recv_timeout(Duration::from_secs(90)).expect("result");
        println!("resolve {} -> {:?} in {:.2}s", ok, res.as_ref().map(|_| "lease"), t0.elapsed().as_secs_f32());
        if res.is_ok() {
            ok += 1;
        }
    }
    for line in rs.diagnostics() {
        println!("diag: {line}");
    }
    assert!(ok >= 3, "at least three resolutions must succeed (YouTube may rate-limit a shared IP)");
    let _ = std::fs::remove_dir_all(&root);
}
