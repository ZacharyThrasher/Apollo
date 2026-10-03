//! Owner-run resolve benchmark: how long from "play" to a playable stream, per strategy, with the
//! REAL account session, yt-dlp and Deno (and libmpv for the last step). Prints timings only: no
//! cookies, URLs, titles, ids or account names. Run it with the Apollo app closed.
//!
//!   apollo-resolve-bench [--tracks N] [--tools DIR] [--out FILE]
//!
//! It signs in with the app's own stored session (the Windows credential vault entry and
//! encrypted store the app uses), picks N tracks from Liked Music, then measures, on distinct
//! tracks each time:
//!   1. legacy      a fresh yt-dlp process per track (what the app did before): the BEFORE number
//!   2. warm        the warm worker pool: first click straight after launch, then steady state
//!   3. cached      the same tracks again (a URL the app already holds)
//!   4. prefetched  a track resolved ahead of the click (hover / page open), then clicked
//!   5. engine      cached URL -> libmpv reports the file open and audio moving (silent output)
//! The summary at the end is the before/after to copy back.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(windows)]
mod imp {
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use apollo_account::identity::{InnerTubeValidator, UreqHttp};
    use apollo_account::service::AccountService;
    use apollo_account::store::EncryptedStore;
    use apollo_account::vault::{Vault, WindowsCredentialVault};
    use apollo_domain::model::Track;
    use apollo_domain::playback::{Coordinator, EngineEvent, Effect, Event, Failure, LeaseId};
    use apollo_media::{MediaConfig, MpvEngine};
    use apollo_resolver::diag::{list_liked_music, pick_varied};
    use apollo_resolver::{ResolverConfig, YtDlpResolver};
    use apollo_runtime::{MediaEngine, ResolveRequest, Resolver};
    use apollo_services_common::{EventSink, LeaseStore};

    struct Out {
        lines: Vec<String>,
    }

    impl Out {
        fn say(&mut self, s: impl Into<String>) {
            let s = s.into();
            println!("{s}");
            self.lines.push(s);
        }
    }

    fn median(v: &[u64]) -> u64 {
        let mut s = v.to_vec();
        s.sort_unstable();
        s.get(s.len() / 2).copied().unwrap_or(0)
    }

    fn secs(ms: u64) -> String {
        format!("{:.2}s", ms as f64 / 1000.0)
    }

    fn stats(v: &[u64]) -> String {
        if v.is_empty() {
            return "no data".into();
        }
        let list: Vec<String> = v.iter().map(|m| secs(*m)).collect();
        format!(
            "median {}  min {}  max {}   [{}]",
            secs(median(v)),
            secs(*v.iter().min().unwrap()),
            secs(*v.iter().max().unwrap()),
            list.join(", ")
        )
    }

    /// Resolve requests for `ids` in order, with fresh load ids each time `ids` repeats.
    fn mint(ids: &[String], repeats: usize) -> Vec<ResolveRequest> {
        let mut c = Coordinator::default();
        let mut tracks = Vec::new();
        for _ in 0..repeats {
            tracks.extend(ids.iter().map(|i| Track::new(i.clone(), "")));
        }
        let want = tracks.len();
        let mut out = Vec::new();
        let mut effects = c.handle(Event::PlayContext { tracks, start: 0 });
        while out.len() < want {
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
            if out.len() < want {
                effects = c.handle(Event::Next);
            }
        }
        out
    }

    struct Rig {
        resolver: YtDlpResolver,
        leases: LeaseStore,
        rx: Receiver<Event>,
        sink: EventSink,
    }

    fn rig(cfg: ResolverConfig, account: &AccountService) -> Rig {
        let (tx, rx) = mpsc::channel::<Event>();
        let tx = Mutex::new(tx);
        let sink: EventSink = Arc::new(move |ev| {
            let _ = tx.lock().unwrap().send(ev);
        });
        let leases = LeaseStore::new();
        let resolver = YtDlpResolver::new(cfg, leases.clone(), sink.clone(), Arc::new(account.clone()))
            .unwrap_or_else(|e| {
                eprintln!("resolver: {e}");
                std::process::exit(1);
            });
        Rig { resolver, leases, rx, sink }
    }

    /// Click: start `req`, wait for its Resolved, return (ms, lease).
    fn click(r: &mut Rig, req: ResolveRequest) -> (u64, Result<LeaseId, Failure>) {
        let load = req.load;
        let t0 = Instant::now();
        r.resolver.start(req);
        loop {
            match r.rx.recv_timeout(Duration::from_secs(90)) {
                Ok(Event::Resolved { load: l, result }) if l == load => {
                    return (t0.elapsed().as_millis() as u64, result);
                }
                Ok(_) => {}
                Err(_) => return (t0.elapsed().as_millis() as u64, Err(Failure::Network)),
            }
        }
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let arg = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
        let n_tracks: usize = arg("--tracks").and_then(|s| s.parse().ok()).unwrap_or(5).clamp(3, 12);
        let local = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA")).join("Apollo").join("app");
        let tools = arg("--tools").map(PathBuf::from).unwrap_or_else(|| {
            let beside = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("tools")));
            match beside {
                Some(d) if d.is_dir() => d,
                _ => std::env::current_dir().unwrap().join("tools"),
            }
        });
        let out_path = arg("--out").map(PathBuf::from).unwrap_or_else(|| local.join("bench-resolve.txt"));
        let mut out = Out { lines: Vec::new() };
        out.say("Apollo resolve benchmark (timings only; nothing secret is printed)");

        // ---------------------------------------------------------------- account
        let vault = Arc::new(WindowsCredentialVault::new("Apollo"));
        let store = match EncryptedStore::new(vault as Arc<dyn Vault>, &local.join("store")) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("could not open the app's store: {e:?}");
                std::process::exit(1);
            }
        };
        let account = AccountService::new(Arc::new(InnerTubeValidator::new(Arc::new(UreqHttp::default()))), Some(store));
        let restored = account.restore();
        let state = account.revalidate();
        out.say(format!("account: restore={restored:?} session={state:?}"));
        if !account.view().can_use_content() {
            out.say("no usable signed-in session in the Apollo app: open Apollo, sign in, close it, run this again.");
            finish(&out, &out_path);
            std::process::exit(2);
        }

        // ------------------------------------------------------------- the tracks
        let base = ResolverConfig::production(&tools, local.join("bench-run"));
        let listed = match list_liked_music(&base, &account, 300) {
            Ok(l) if l.len() >= 3 => l,
            other => {
                out.say(format!("Liked Music listing unusable: {:?}", other.map(|l| l.len())));
                finish(&out, &out_path);
                std::process::exit(1);
            }
        };
        let picked = pick_varied(listed, n_tracks + 2);
        let ids: Vec<String> = picked.iter().map(|t| t.id.clone()).collect();
        let k = n_tracks.min(ids.len().saturating_sub(2));
        let (main_ids, spare) = (ids[..k].to_vec(), ids[k..].to_vec());
        out.say(format!("tracks: {k} for the main runs, {} spare for the prefetch run", spare.len()));

        // ------------------------------------------------------------ 1. legacy
        out.say("\n[1] BEFORE: a fresh yt-dlp process per track (legacy)");
        let mut legacy = Vec::new();
        {
            let mut r = rig(base.clone().legacy(), &account);
            for req in mint(&main_ids, 1) {
                let (ms, res) = click(&mut r, req);
                if res.is_ok() {
                    legacy.push(ms);
                } else {
                    out.say(format!("  a request failed: {:?}", res.err()));
                }
            }
            out.say(format!("  click -> stream ready: {}", stats(&legacy)));
        }

        // ------------------------------------------------------------ 2/3/4 warm
        out.say("\n[2] AFTER: warm workers, first click straight after launch, then steady state");
        let mut warm = Vec::new();
        let mut cached = Vec::new();
        let mut prefetch_ms = Vec::new();
        let mut prefetched_click = Vec::new();
        let mut engine_ms: Option<(u64, u64)> = None;
        let mut worker_ready_ms = None;
        {
            let mut r = rig(base.clone(), &account);
            let t_launch = Instant::now();
            let reqs = mint(&main_ids, 2);
            let (first_pass, second_pass) = reqs.split_at(main_ids.len());
            for req in first_pass.iter().cloned() {
                let (ms, res) = click(&mut r, req);
                if res.is_ok() {
                    warm.push(ms);
                }
            }
            if let Some((_, idle, spawned, _)) = r.resolver.warm_status() {
                let _ = (idle, spawned);
                worker_ready_ms = Some(t_launch.elapsed().as_millis() as u64);
            }
            out.say(format!("  click -> stream ready: {}", stats(&warm)));
            out.say(format!("  first click (includes starting the worker): {}", warm.first().map_or("-".into(), |m| secs(*m))));
            if warm.len() > 1 {
                out.say(format!("  steady state (after the first): {}", stats(&warm[1..])));
            }

            out.say("\n[3] AFTER: the same tracks again (URL already held)");
            for req in second_pass.iter().cloned() {
                let (ms, res) = click(&mut r, req);
                if res.is_ok() {
                    cached.push(ms);
                }
            }
            out.say(format!("  click -> stream ready: {}", stats(&cached)));

            out.say("\n[4] AFTER: resolved ahead of the click (hover / page open), then clicked");
            if let Some(extra) = (!spare.is_empty()).then(|| mint(&spare, 2)) {
                let (a, b) = extra.split_at(spare.len());
                for (pre, real) in a.iter().zip(b.iter()) {
                    let t0 = Instant::now();
                    r.resolver.prefetch(&pre.track);
                    while r.resolver.cached() < main_ids.len() + prefetch_ms.len() + 1 && t0.elapsed() < Duration::from_secs(60) {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    prefetch_ms.push(t0.elapsed().as_millis() as u64);
                    let (ms, res) = click(&mut r, real.clone());
                    if res.is_ok() {
                        prefetched_click.push(ms);
                    }
                }
                out.say(format!("  prefetch itself (runs in the background): {}", stats(&prefetch_ms)));
                out.say(format!("  click afterwards -> stream ready: {}", stats(&prefetched_click)));
            }

            // 5. engine leg on a cached URL.
            out.say("\n[5] cached URL -> libmpv open and audio moving (silent output)");
            let mcfg = MediaConfig { audio_output: Some("null".into()), volume: 0, ..Default::default() };
            match MpvEngine::new(mcfg, r.leases.clone(), r.sink.clone()) {
                Ok(mut engine) => {
                    let reqs = mint(&main_ids[..1], 1);
                    let req = reqs[0].clone();
                    let (ms, res) = click(&mut r, req.clone());
                    if let Ok(lease) = res {
                        let t0 = Instant::now();
                        engine.open(req.load, lease, false, 0);
                        let (mut opened, mut audio) = (None, None);
                        while t0.elapsed() < Duration::from_secs(20) && audio.is_none() {
                            if let Ok(Event::Engine { load, event }) = r.rx.recv_timeout(Duration::from_millis(50)) {
                                if load == req.load {
                                    match event {
                                        EngineEvent::Opened { .. } => opened = Some(t0.elapsed().as_millis() as u64),
                                        EngineEvent::Position(p) if p > 0 => audio = Some(t0.elapsed().as_millis() as u64),
                                        _ => {}
                                    }
                                }
                            }
                        }
                        if let (Some(o), Some(a)) = (opened, audio) {
                            engine_ms = Some((o, a));
                            out.say(format!("  resolve (cached) {ms} ms, then file open {o} ms, audio moving {a} ms after open command"));
                        } else {
                            out.say("  the engine did not report open/audio in time");
                        }
                    }
                }
                Err(e) => out.say(format!("  engine unavailable: {e}")),
            }

            out.say("\n[where the time goes] (worker phases: label@ms-since-request-start; first lines are the cold ones)");
            for line in r.resolver.diagnostics().iter().filter(|l| l.contains("phases=")).take(8) {
                out.say(format!("  {line}"));
            }
            if let Some((alive, idle, spawned, failed)) = r.resolver.warm_status() {
                out.say(format!("  workers: alive={alive} idle={idle} started={spawned} failed_starts={failed}"));
                if failed > 0 {
                    out.say(format!("  worker start problem: {}", r.resolver.warm_last_error().unwrap_or_default()));
                }
            }
        }

        // ------------------------------------------------- 6. worker settings compared
        out.say("\n[6] AFTER, tuned: which yt-dlp setting makes a cold click fastest? (a fresh worker per setting, started before the first click; cold clicks on distinct tracks)");
        let skip: (&str, &[&str]) = ("player_skip", &["webpage", "initial_data"]);
        let skip_all: (&str, &[&str]) = ("player_skip", &["webpage", "initial_data", "configs"]);
        let variants: Vec<(&str, Vec<(&str, &[&str])>)> = vec![
            ("yt-dlp defaults", vec![]),
            ("skip page+initial", vec![skip]),
            ("skip page+initial+configs", vec![skip_all]),
            ("skip all + embedded,tv", vec![skip_all, ("player_client", &["web_embedded", "tv_downgraded"])]),
            ("skip page+initial + embedded,tv", vec![skip, ("player_client", &["web_embedded", "tv_downgraded"])]),
            ("skip page+initial + creator,tv,music", vec![skip, ("player_client", &["web_creator", "tv_downgraded", "web_music"])]),
        ];
        let mut tuned: Vec<(String, Vec<u64>, usize)> = Vec::new();
        for (name, args) in variants {
            let mut cfg = base.clone();
            cfg.cache = None;
            if let Some(w) = cfg.warm.as_mut() {
                w.prewarm = 1;
                w.foreground_youtube_args = None; // each setting is measured as-is
                w.youtube_args = args.iter().map(|(k, v)| ((*k).to_owned(), v.iter().map(|x| (*x).to_owned()).collect())).collect();
            }
            let mut r = rig(cfg, &account);
            let t0 = Instant::now();
            while r.resolver.warm_status().map_or(true, |(_, idle, _, _)| idle < 1) && t0.elapsed() < Duration::from_secs(40) {
                std::thread::sleep(Duration::from_millis(50));
            }
            let mut times = Vec::new();
            let mut failed = 0usize;
            for req in mint(&main_ids[..main_ids.len().min(4)], 1) {
                let (ms, res) = click(&mut r, req);
                match res {
                    Ok(_) => times.push(ms),
                    Err(e) => {
                        failed += 1;
                        out.say(format!("  {name}: a request failed: {e:?}"));
                    }
                }
            }
            out.say(format!("  {name}: click -> stream ready: {} (failed: {failed})", stats(&times)));
            for line in r.resolver.diagnostics().iter().filter(|l| l.contains("src=warm")).take(4) {
                out.say(format!("    {line}"));
            }
            tuned.push((name.to_owned(), times, failed));
        }

        // ------------------------------------------------------------- summary
        out.say("\n=== SUMMARY (click -> playable stream URL) ===");
        out.say(format!("BEFORE  one yt-dlp per track        median {}", secs(median(&legacy))));
        if let Some(first) = warm.first() {
            out.say(format!("AFTER   first click after launch    {}   (starts the worker if it was not ready)", secs(*first)));
        }
        if warm.len() > 1 {
            out.say(format!("AFTER   warm worker, new track      median {}", secs(median(&warm[1..]))));
        }
        out.say(format!("AFTER   URL already held            median {}", secs(median(&cached))));
        if !prefetched_click.is_empty() {
            out.say(format!("AFTER   prefetched, then clicked    median {}", secs(median(&prefetched_click))));
        }
        if let Some((o, a)) = engine_ms {
            out.say(format!("then    libmpv open {o} ms, audio moving {a} ms"));
        }
        for (name, times, failed) in &tuned {
            out.say(format!("TUNED   {name:<38} median {} (failed {failed})", secs(median(times))));
        }
        let _ = worker_ready_ms;
        finish(&out, &out_path);
    }

    fn finish(out: &Out, path: &PathBuf) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, out.lines.join("\n") + "\n");
        println!("\n(also saved to {})", path.display());
    }
}
