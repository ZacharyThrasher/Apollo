//! Owner-run Milestone 2 probe. Drives the REAL account session, resolver and libmpv engine
//! through the runtime `Driver` + coordinator, the way the app will, and reports redacted
//! PASS/FAIL lines. It never prints cookies, URLs, titles, ids or account names.
//!
//!   apollo-m2-probe [--source stored|firefox|login|file:<path>] [--tracks N] [--silent]
//!                   [--select-identity K] [--keep-session]
//!
//! Run from the repository root (it looks for `tools/` there). By default it LOGS OUT at the end
//! (that is part of the test); pass --keep-session to keep the stored session instead.

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
    use std::process::Command;
    use std::sync::mpsc::{self, Receiver};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use apollo_account::identity::{InnerTubeValidator, UreqHttp};
    use apollo_account::import;
    use apollo_account::jar::CookieJar;
    use apollo_account::login::{run_login, LoginCancel, LoginConfig};
    use apollo_account::service::{AccountService, IdentityState, ImportOutcome, RestoreOutcome, SessionState, StorageMode};
    use apollo_account::store::EncryptedStore;
    use apollo_account::vault::{Vault, WindowsCredentialVault};
    use apollo_domain::model::Track;
    use apollo_domain::playback::{Event, Failure, Phase};
    use apollo_domain::queue::Repeat;
    use apollo_media::{MediaConfig, MpvEngine};
    use apollo_resolver::diag::{list_liked_music, pick_varied};
    use apollo_resolver::{ResolverConfig, YtDlpResolver};
    use apollo_runtime::Driver;
    use apollo_services_common::{unix_now, EventSink, LeaseStore, SessionProvider};

    type D = Driver<YtDlpResolver, MpvEngine>;

    struct Report {
        lines: Vec<(bool, String)>,
    }

    impl Report {
        fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
            let detail = detail.into();
            println!("  [{}] {name}: {detail}", if ok { "PASS" } else { "FAIL" });
            self.lines.push((ok, name.to_string()));
        }
    }

    enum Source {
        Stored,
        Firefox,
        Login,
        File(PathBuf),
    }

    /// Process every queued event for up to `dur`, returning early when `until` holds.
    fn pump(d: &mut D, rx: &Receiver<Event>, dur: Duration, mut until: impl FnMut(&D) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < dur {
            if until(d) {
                return true;
            }
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(50)) {
                d.handle(ev);
                while let Ok(ev) = rx.try_recv() {
                    d.handle(ev);
                }
            }
        }
        until(d)
    }

    fn count_processes(image: &str) -> usize {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("IMAGENAME eq {image}"), "/FO", "CSV", "/NH"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_lowercase())
            .unwrap_or_default();
        out.matches(&image.to_lowercase()).count()
    }

    fn current_video(d: &D) -> Option<String> {
        d.coordinator().queue().current().map(|e| e.track.video_id.as_str().to_owned())
    }

    pub fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let arg = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
        let flag = |n: &str| args.iter().any(|a| a == n);
        let source = match arg("--source").as_deref() {
            None | Some("stored") => Source::Stored,
            Some("firefox") => Source::Firefox,
            Some("login") => Source::Login,
            Some(s) if s.starts_with("file:") => Source::File(PathBuf::from(&s[5..])),
            Some(other) => {
                eprintln!("unknown --source {other}");
                std::process::exit(2);
            }
        };
        let n_tracks: usize = arg("--tracks").and_then(|s| s.parse().ok()).unwrap_or(4).max(3);
        let silent = flag("--silent");
        let mut rep = Report { lines: Vec::new() };

        let root = std::env::current_dir().unwrap();
        let tools = root.join("tools");
        let app = PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Apollo").join("m2-probe");
        println!("Milestone 2 probe (redacted output)");

        // ---------------------------------------------------------------- account
        println!("\n[account]");
        let vault = Arc::new(WindowsCredentialVault::new("Apollo-m2-probe"));
        let store = EncryptedStore::new(vault.clone() as Arc<dyn Vault>, &app.join("store")).expect("store");
        let ciphertext = store.ciphertext_path().to_path_buf();
        let account = AccountService::new(
            Arc::new(InnerTubeValidator::new(Arc::new(UreqHttp::default()))),
            Some(store),
        );

        let restored = account.restore();
        println!("  restore: {restored:?}");
        let mut need_import = !matches!(restored, RestoreOutcome::Restored)
            || account.view().session != SessionState::Valid;
        if matches!(source, Source::Stored) && need_import {
            println!("  no valid stored session; re-run with --source firefox | login | file:<path>");
            std::process::exit(2);
        }
        if !matches!(source, Source::Stored) {
            need_import = true;
        }
        if need_import {
            let jar: Result<CookieJar, String> = match &source {
                Source::Firefox => {
                    let ff = PathBuf::from(std::env::var("APPDATA").unwrap()).join("Mozilla").join("Firefox");
                    let profiles = import::firefox_profiles(&ff);
                    match profiles.iter().find(|p| p.is_default).or(profiles.first()) {
                        Some(p) => import::firefox_profile(&p.path).map_err(|e| e.to_string()),
                        None => Err("no Firefox profile".into()),
                    }
                }
                Source::Login => {
                    println!("  opening the sign-in window: sign in there (the app never sees your password)");
                    let helper = root.join("target").join("debug").join("apollo-login-helper.exe");
                    let cfg = LoginConfig::new(helper, app.join("login"));
                    run_login(&cfg, &LoginCancel::new()).map_err(|e| e.to_string())
                }
                Source::File(p) => import::netscape_file(p).map_err(|e| e.to_string()),
                Source::Stored => unreachable!(),
            };
            let jar = match jar {
                Ok(j) => {
                    rep.check("obtain cookies", true, format!("{} in-scope cookies, auth cookies present: {}", j.len(), j.has_auth_cookies()));
                    j
                }
                Err(e) => {
                    rep.check("obtain cookies", false, e);
                    std::process::exit(1);
                }
            };
            let outcome = account.import(jar, true);
            let ok = matches!(outcome, ImportOutcome::Valid | ImportOutcome::NeedsIdentityChoice);
            rep.check("service validates the session (account_menu)", ok, format!("{outcome:?}"));
            if outcome == ImportOutcome::NeedsIdentityChoice {
                if let IdentityState::Ambiguous(list) = account.view().identity {
                    println!("  the login can act as {} channels (explicit choice required)", list.len());
                    let k: usize = arg("--select-identity").and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
                    match list.get(k) {
                        Some(i) => {
                            account.select_identity(&i.key());
                        }
                        None => {
                            println!("  re-run with --select-identity <0..{}>", list.len() - 1);
                            std::process::exit(2);
                        }
                    }
                }
            }
        }
        let view = account.view();
        rep.check(
            "session valid + identity selected + owner declaration",
            view.can_use_content(),
            format!("session={:?} identity={} premium=\"{}\" storage={}",
                view.session,
                match &view.identity { IdentityState::Selected(_) => "selected".to_string(), other => format!("{other:?}") },
                view.premium_label(),
                match &view.storage { StorageMode::Durable => "durable (vault key + encrypted file)".to_string(), o => format!("{o:?}") }),
        );
        if !view.can_use_content() {
            std::process::exit(1);
        }
        rep.check("ciphertext on disk is encrypted", {
            let raw = std::fs::read(&ciphertext).unwrap_or_default();
            !raw.is_empty() && !String::from_utf8_lossy(&raw).contains("SAPISID")
        }, "no cookie names in the stored file");

        // ------------------------------------------------------------ wire the engine
        println!("\n[playback]");
        let (tx, rx) = mpsc::channel::<Event>();
        let tx = Mutex::new(tx);
        let sink: EventSink = Arc::new(move |ev| {
            let _ = tx.lock().unwrap().send(ev);
        });
        let leases = LeaseStore::new();
        let run_dir = app.join("run");
        let rcfg = ResolverConfig::production(&tools, run_dir);
        let resolver = match YtDlpResolver::new(rcfg.clone(), leases.clone(), sink.clone(), Arc::new(account.clone())) {
            Ok(r) => {
                rep.check("pinned yt-dlp and Deno verified", true, "SHA-256 match (docs/build-baseline.md)");
                r
            }
            Err(e) => {
                rep.check("pinned yt-dlp and Deno verified", false, e.to_string());
                std::process::exit(1);
            }
        };
        let mcfg = MediaConfig {
            audio_output: silent.then(|| "null".to_string()),
            volume: if silent { 100 } else { 30 },
            ..Default::default()
        };
        let engine = MpvEngine::new(mcfg, leases.clone(), sink.clone()).expect("engine");
        let mut d: D = Driver::new(resolver, engine);

        // ---------------------------------------------------------- pick the tracks
        let listed = list_liked_music(&rcfg, &account, 300);
        let tracks = match listed {
            Ok(all) => {
                let n_all = all.len();
                let picked = pick_varied(all, n_tracks);
                rep.check("authenticated Liked Music listing", n_all > 0, format!("{n_all} entries visible, {} picked (distinct artists, varied durations)", picked.len()));
                picked
            }
            Err(f) => {
                rep.check("authenticated Liked Music listing", false, format!("{f:?}"));
                std::process::exit(1);
            }
        };
        if tracks.len() < 3 {
            println!("  need at least 3 usable liked tracks");
            std::process::exit(1);
        }
        let ids: Vec<String> = tracks.iter().map(|t| t.id.clone()).collect();
        let q_tracks: Vec<Track> = ids.iter().map(|i| Track::new(i.clone(), "")).collect();
        let yt_before = count_processes("yt-dlp.exe");

        // ------------------------------------------------------- sequential playback
        let t0 = Instant::now();
        d.handle(Event::PlayContext { tracks: q_tracks, start: 0 });
        let playing = pump(&mut d, &rx, Duration::from_secs(60), |d| d.coordinator().phase() == Phase::Playing);
        rep.check(
            "first track resolves and plays (authenticated, no PO-token service)",
            playing,
            format!("phase={:?} after {:.1}s (resolve + connect + first audio)", d.coordinator().phase(), t0.elapsed().as_secs_f64()),
        );
        if !playing {
            rep.check("failure class reported", false, format!("{:?}", d.coordinator().error()));
            finish(&mut rep, &account, &ciphertext, &vault, args.iter().any(|a| a == "--keep-session"), &mut d, &leases, &rx);
            return;
        }
        let first = current_video(&d);
        let p0 = d.coordinator().position_ms();
        pump(&mut d, &rx, Duration::from_secs(3), |_| false);
        let p1 = d.coordinator().position_ms();
        rep.check("position advances while playing (~4 Hz updates)", p1 >= p0 + 1500, format!("{} ms -> {} ms in 3 s", p0, p1));
        let dur = d.coordinator().duration_ms().unwrap_or(0);
        rep.check("duration reported", dur > 10_000, format!("{:.0} s", dur as f64 / 1000.0));

        // Seek to the middle.
        d.handle(Event::Seek { position_ms: dur / 2 });
        let settled = pump(&mut d, &rx, Duration::from_secs(20), |d| {
            d.coordinator().phase() == Phase::Playing && d.coordinator().position_ms().abs_diff(dur / 2) < 2500
        });
        rep.check("seek to middle (generation-matched Seeked)", settled, format!("target {} ms, now {} ms, phase {:?}", dur / 2, d.coordinator().position_ms(), d.coordinator().phase()));

        // Seek near the end: real EOF -> automatic advance to the next queue entry.
        d.handle(Event::Seek { position_ms: dur.saturating_sub(4000) });
        let advanced = pump(&mut d, &rx, Duration::from_secs(60), |d| {
            d.coordinator().phase() == Phase::Playing && current_video(d) != first && current_video(d).is_some()
        });
        rep.check("end of file advances to the next track automatically", advanced, format!("phase {:?}", d.coordinator().phase()));

        // Explicit Next.
        let before = current_video(&d);
        d.handle(Event::Next);
        let nexted = pump(&mut d, &rx, Duration::from_secs(60), |d| {
            d.coordinator().phase() == Phase::Playing && current_video(d) != before
        });
        rep.check("explicit Next plays the following track", nexted, format!("phase {:?}", d.coordinator().phase()));

        // Pause / resume.
        d.handle(Event::Pause);
        pump(&mut d, &rx, Duration::from_secs(5), |d| d.coordinator().phase() == Phase::Paused);
        let paused_at = d.coordinator().position_ms();
        pump(&mut d, &rx, Duration::from_secs(2), |_| false);
        let still = d.coordinator().position_ms();
        rep.check("pause holds position", d.coordinator().phase() == Phase::Paused && still.abs_diff(paused_at) < 400, format!("{} ms -> {} ms over 2 s", paused_at, still));
        d.handle(Event::Play);
        let resumed = pump(&mut d, &rx, Duration::from_secs(10), |d| d.coordinator().phase() == Phase::Playing && d.coordinator().position_ms() > still + 1000);
        rep.check("resume continues from where it stopped", resumed, format!("phase {:?}", d.coordinator().phase()));

        // TEST-06: twenty rapid Next commands.
        d.edit_queue(|q| q.set_repeat(Repeat::All));
        for _ in 0..20 {
            d.handle(Event::Next);
        }
        let settled = pump(&mut d, &rx, Duration::from_secs(90), |d| d.coordinator().phase() == Phase::Playing);
        pump(&mut d, &rx, Duration::from_secs(3), |_| false);
        rep.check(
            "twenty rapid Next commands: one owner, no stale audio, no crash",
            settled && d.coordinator().phase() == Phase::Playing && d.coordinator().active_load().is_some(),
            format!("phase {:?}", d.coordinator().phase()),
        );
        // Helper processes are reaped.
        let yt_during = count_processes("yt-dlp.exe");
        pump(&mut d, &rx, Duration::from_secs(8), |_| false);
        let yt_after = count_processes("yt-dlp.exe");
        rep.check("no leaked yt-dlp helper processes", yt_after <= yt_before, format!("before {yt_before}, after {yt_after} (peak seen {yt_during})"));

        finish(&mut rep, &account, &ciphertext, &vault, flag("--keep-session"), &mut d, &leases, &rx);
    }

    /// Logout during playback, then verify nothing survives and nothing plays anonymously.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        rep: &mut Report,
        account: &AccountService,
        ciphertext: &std::path::Path,
        vault: &Arc<WindowsCredentialVault>,
        keep: bool,
        d: &mut D,
        leases: &LeaseStore,
        rx: &Receiver<Event>,
    ) {
        println!("\n[logout]");
        if keep {
            println!("  --keep-session: leaving the stored session in place");
        } else {
            let was_playing = d.coordinator().phase() == Phase::Playing;
            // The app's logout sequence (SEC-03): stop playback, cancel resolver work, drop
            // leases, then drop cookies and delete the vault entry.
            d.handle(Event::Logout);
            d.resolver.cancel_all();
            leases.clear();
            let report = account.logout();
            pump(d, rx, Duration::from_secs(2), |_| false);
            rep.check("logout while playing stops playback", was_playing && d.coordinator().phase() == Phase::Idle, format!("phase {:?}", d.coordinator().phase()));
            rep.check("leases dropped, no resolver job in flight", leases.is_empty() && d.resolver.in_flight() == 0, "memory-only leases cleared");
            rep.check("session ended and secrets deleted", report.stored.key_deleted && report.stored.ciphertext_deleted && !ciphertext.exists() && vault.get("jar-key-v1").map(|v| v.is_none()).unwrap_or(false), "vault key and encrypted file removed");
            rep.check("resolver snapshot is gone", account.snapshot().is_none() && account.view().session == SessionState::Absent, "no session to resolve with");

            // No silent guest playback: a play attempt now must fail as SessionExpired.
            d.handle(Event::PlayContext { tracks: vec![Track::new("dQw4w9WgXcQ", "")], start: 0 });
            pump(d, rx, Duration::from_secs(15), |d| d.coordinator().phase() == Phase::Failed);
            rep.check(
                "after logout nothing plays anonymously",
                d.coordinator().phase() == Phase::Failed && d.coordinator().error() == Some(Failure::SessionExpired),
                format!("phase {:?}, error {:?}", d.coordinator().phase(), d.coordinator().error()),
            );
            let diag = d.resolver.diagnostics().join("\n");
            rep.check("resolver refused without starting a process", diag.contains("no-session"), "outcome code no-session recorded");
        }
        let failed = rep.lines.iter().filter(|(ok, _)| !ok).count();
        println!("\nRESULT: {} checks, {} failed ({})", rep.lines.len(), failed, unix_now());
        std::process::exit(if failed == 0 { 0 } else { 1 });
    }
}
