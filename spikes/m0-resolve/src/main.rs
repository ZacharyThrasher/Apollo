//! Milestone 0 step 6 probe (owner-run): authenticated yt-dlp resolution + libmpv playback.
//!
//! Output is REDACTED by construction: no cookies, no signed URLs, no titles, no video IDs.
//! The signed media URL lives only in memory and goes to libmpv through the API (never argv).
//! yt-dlp is launched with an argument ARRAY, no user config, no plugins, no cache, no remote
//! components, and an explicit Deno path. There is NO anonymous fallback: without an
//! explicit credential source the program refuses to run, except `--anonymous-control`.
//!
//! Usage:
//!   apollo-m0-resolve (--cookies <netscape-file> | --cookies-from-browser <name>)
//!       [--from-liked N | --ids <file>] [--full-realtime 1,5] [--anonymous-control]
//!
//! `--from-liked N` picks N varied tracks from the account's Liked Music playlist (itself an
//! authenticated check: that playlist is empty/unavailable for a logged-out session).

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use libmpv2::{Mpv, events::Event, mpv_end_file_reason};

const YTDLP: &str = "tools/yt-dlp/yt-dlp.exe";
const DENO: &str = "tools/deno/deno.exe";
const MAX_STDOUT: usize = 40 * 1024 * 1024;

enum Auth {
    CookieFile(PathBuf),
    Browser(String),
    None,
}

struct Run {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
    timed_out: bool,
    secs: f64,
}

/// Runs yt-dlp with a bounded lifetime. On timeout the whole process tree is killed.
fn run_ytdlp(root: &Path, auth: &Auth, extra: &[&str], timeout: Duration) -> Run {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(root.join(YTDLP));
    cmd.args([
        "-v",
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-cache-dir",
        "--no-remote-components",
        "--no-update",
        "--no-warnings",
        "--skip-download",
        "--no-playlist",
    ]);
    cmd.arg("--js-runtimes").arg(format!("deno:{}", root.join(DENO).display()));
    match auth {
        Auth::CookieFile(p) => {
            cmd.arg("--cookies").arg(p);
        }
        Auth::Browser(b) => {
            cmd.arg("--cookies-from-browser").arg(b);
        }
        Auth::None => {}
    }
    cmd.args(extra);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let started = Instant::now();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Run { code: None, stdout: vec![], stderr: format!("spawn failed: {e}"), timed_out: false, secs: 0.0 };
        }
    };
    let pid = child.id();
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    let (otx, orx) = mpsc::channel();
    let (etx, erx) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(MAX_STDOUT as u64).read_to_end(&mut buf);
        let _ = otx.send(buf);
    });
    thread::spawn(move || {
        let mut s = String::new();
        let _ = (&mut err).take(4 * 1024 * 1024).read_to_string(&mut s);
        let _ = etx.send(s);
    });
    let mut timed_out = false;
    let code = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.code(),
            Ok(None) => {
                if started.elapsed() > timeout {
                    timed_out = true;
                    // Kill the whole tree (the one-file exe spawns a child).
                    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(0x0800_0000).output();
                    let _ = child.wait();
                    break None;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };
    let stdout = orx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let stderr = erx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    Run { code, stdout, stderr, timed_out, secs: started.elapsed().as_secs_f64() }
}

/// Coarse, redacted classification of yt-dlp's diagnostic text. Raw text is never printed.
fn classify(stderr: &str) -> Vec<&'static str> {
    let table = [
        ("Sign in to confirm", "bot-check"),
        ("PO Token", "po-token-mentioned"),
        ("po_token", "po-token-mentioned"),
        ("SABR", "sabr-mentioned"),
        ("DRM", "drm-mentioned"),
        ("Video unavailable", "video-unavailable"),
        ("members-only", "members-only"),
        ("Premium", "premium-mentioned"),
        ("HTTP Error 429", "http-429"),
        ("HTTP Error 403", "http-403"),
        ("Requested format is not available", "format-unavailable"),
        ("Private video", "private"),
        ("n challenge", "n-challenge-mentioned"),
        ("Signature solving failed", "sig-solve-failed"),
        ("no longer valid", "cookies-no-longer-valid"),
        ("cookies", "cookies-mentioned"),
    ];
    let mut v: Vec<&'static str> = Vec::new();
    for (needle, label) in table {
        if stderr.contains(needle) && !v.contains(&label) {
            v.push(label);
        }
    }
    v
}

/// Which InnerTube player clients yt-dlp tried / skipped (names only).
fn clients_used(stderr: &str) -> String {
    let mut tried: BTreeSet<String> = BTreeSet::new();
    let mut skipped: BTreeSet<String> = BTreeSet::new();
    for line in stderr.lines().filter(|l| l.contains("[youtube]")) {
        if let Some(i) = line.find("Downloading ") {
            let rest = &line[i + 12..];
            if let Some(j) = rest.find(" player API JSON") {
                tried.insert(rest[..j].to_string());
            }
        }
        if let Some(i) = line.find("Skipping client \"") {
            let rest = &line[i + 17..];
            if let Some(j) = rest.find('"') {
                skipped.insert(rest[..j].to_string());
            }
        }
    }
    format!("tried={:?} skipped={:?}", tried, skipped)
}

fn host_ok(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', ':']).next().unwrap_or("");
    host == "googlevideo.com" || host.ends_with(".googlevideo.com")
}

struct Resolved {
    url: String,
    user_agent: Option<String>,
    duration: f64,
    summary: String,
}

fn resolve(root: &Path, auth: &Auth, id: &str) -> Result<Resolved, String> {
    let watch = format!("https://music.youtube.com/watch?v={id}");
    let r = run_ytdlp(root, auth, &["-f", "bestaudio", "-J", &watch], Duration::from_secs(120));
    if r.timed_out {
        return Err("resolver timed out (process tree killed)".into());
    }
    let cats = classify(&r.stderr);
    let json: serde_json::Value = match serde_json::from_slice(&r.stdout) {
        Ok(j) => j,
        Err(_) => return Err(format!("no JSON (exit {:?}); categories={cats:?}; {}", r.code, clients_used(&r.stderr))),
    };
    let id_ok = json["id"].as_str() == Some(id);
    let url = json["url"].as_str().unwrap_or("").to_string();
    let proto = json["protocol"].as_str().unwrap_or("?").to_string();
    let acodec = json["acodec"].as_str().unwrap_or("?").to_string();
    let abr = json["abr"].as_f64().unwrap_or(0.0);
    let ext = json["ext"].as_str().unwrap_or("?").to_string();
    let fid = json["format_id"].as_str().unwrap_or("?").to_string();
    let drm = json["_has_drm"].as_bool().unwrap_or(false);
    let duration = json["duration"].as_f64().unwrap_or(0.0);
    // All audio formats offered, summarized without URLs.
    let mut offered: BTreeSet<String> = BTreeSet::new();
    let mut premium_note = false;
    if let Some(fmts) = json["formats"].as_array() {
        for f in fmts {
            if f["acodec"].as_str().is_some_and(|a| a != "none") && f["vcodec"].as_str() == Some("none") {
                let note = f["format_note"].as_str().unwrap_or("");
                if note.contains("Premium") {
                    premium_note = true;
                }
                offered.insert(format!(
                    "{}:{}@{}k/{}",
                    f["format_id"].as_str().unwrap_or("?"),
                    f["acodec"].as_str().unwrap_or("?").split('.').next().unwrap_or("?"),
                    f["abr"].as_f64().unwrap_or(0.0).round(),
                    f["protocol"].as_str().unwrap_or("?")
                ));
            }
        }
    }
    let problems: Vec<&str> = [
        (!id_ok, "id-mismatch"),
        (!host_ok(&url), "url-not-https-googlevideo"),
        (proto != "https", "protocol-not-plain-https"),
        (acodec == "none" || acodec == "?", "no-audio"),
        (drm, "drm"),
    ]
    .into_iter()
    .filter_map(|(bad, l)| bad.then_some(l))
    .collect();
    let summary = format!(
        "selected={fid} {acodec} {abr:.0}k {ext} proto={proto} duration={duration:.0}s premium_format_flag={premium_note} offered_audio={} {} cats={cats:?}",
        offered.len(),
        clients_used(&r.stderr)
    );
    if !problems.is_empty() {
        return Err(format!("rejected: {problems:?}; {summary}"));
    }
    let user_agent = json["http_headers"]["User-Agent"].as_str().map(str::to_string);
    println!("    resolve {:.1}s: {summary}", r.secs);
    println!("    offered audio formats: {:?}", offered);
    Ok(Resolved { url, user_agent, duration, summary })
}

fn play(res: &Resolved, full_realtime: bool) -> bool {
    let mpv = match Mpv::with_initializer(|init| {
        init.set_property("config", false)?;
        init.set_property("load-scripts", false)?;
        init.set_property("ytdl", false)?;
        init.set_property("osc", false)?;
        init.set_property("input-default-bindings", false)?;
        init.set_property("input-vo-keyboard", false)?;
        init.set_property("terminal", false)?;
        init.set_property("vo", "null")?;
        init.set_property("vid", "no")?;
        init.set_property("idle", "yes")?;
        init.set_property("volume", 25)?;
        init.set_property("network-timeout", 20)?;
        init.set_property("cache", "yes")?;
        init.set_property("cache-on-disk", false)?;
        init.set_property("demuxer-max-bytes", "32MiB")?;
        Ok(())
    }) {
        Ok(m) => m,
        Err(e) => {
            println!("    play: mpv init failed: {e:?}");
            return false;
        }
    };
    // Only a User-Agent is forwarded; never Cookie/Authorization headers.
    if let Some(ua) = &res.user_agent {
        let _ = mpv.set_property("user-agent", ua.as_str());
    }
    let t0 = Instant::now();
    if let Err(e) = mpv.command("loadfile", &[res.url.as_str(), "replace"]) {
        println!("    play: loadfile rejected: {e:?}");
        return false;
    }
    let dur = res.duration;
    #[derive(PartialEq, Debug, Clone, Copy)]
    enum P { Loading, P1, SeekStart, P2, SeekMid, P3, SeekEnd, Eof }
    let mut p = P::Loading;
    let mut since = Instant::now();
    let mut first_audio: Option<f64> = None;
    let mut restart_after_seek = false;
    let mut target = 0.0;
    let mut seek_at = Instant::now();
    let mut ok = true;
    let mut results: Vec<String> = Vec::new();
    let mut last_pos = -1.0f64;
    let mut last_adv = Instant::now();
    let mut eof = String::from("none");
    let hard_limit = if full_realtime { Duration::from_secs_f64(dur + 120.0) } else { Duration::from_secs(150) };
    let loop_start = Instant::now();
    let mut seek_pending_label = "";
    loop {
        if loop_start.elapsed() > hard_limit {
            ok = false;
            results.push(format!("timeout in {p:?}"));
            break;
        }
        match mpv.wait_event(0.05) {
            Some(Ok(Event::PlaybackRestart)) => {
                if matches!(p, P::Loading | P::P1) && first_audio.is_none() {
                    first_audio = Some(t0.elapsed().as_secs_f64());
                    if p == P::Loading {
                        p = P::P1;
                        since = Instant::now();
                    }
                } else if matches!(p, P::SeekStart | P::SeekMid | P::SeekEnd) {
                    restart_after_seek = true;
                }
            }
            Some(Ok(Event::FileLoaded)) => {
                if p == P::Loading {
                    p = P::P1;
                    since = Instant::now();
                }
            }
            Some(Ok(Event::EndFile(reason))) => {
                eof = if reason == mpv_end_file_reason::Eof {
                    "EOF".into()
                } else if reason == mpv_end_file_reason::Error {
                    "ERROR".into()
                } else {
                    "OTHER".into()
                };
                if p != P::Eof {
                    ok = false;
                    results.push(format!("end-file {eof} before eof phase ({p:?})"));
                }
                break;
            }
            _ => {}
        }
        let pos = mpv.get_property::<f64>("time-pos").unwrap_or(-1.0);
        if pos > last_pos + 0.2 {
            last_pos = pos;
            last_adv = Instant::now();
        }
        if matches!(p, P::P1 | P::P2 | P::P3 | P::Eof) && last_adv.elapsed() > Duration::from_secs(20) && !mpv.get_property::<bool>("pause").unwrap_or(false) {
            ok = false;
            results.push(format!("stalled >20s at {pos:.1}s"));
            break;
        }
        match p {
            P::P1 | P::P2 | P::P3 if since.elapsed() >= Duration::from_millis(2000) && pos >= 0.0 => {
                target = match p {
                    P::P1 => 0.5,
                    P::P2 => dur / 2.0,
                    _ => (dur - 3.0).max(0.0),
                };
                let t = format!("{target:.3}");
                let _ = mpv.command("seek", &[&t, "absolute+exact"]);
                seek_at = Instant::now();
                restart_after_seek = false;
                seek_pending_label = match p {
                    P::P1 => "seek-start",
                    P::P2 => "seek-mid",
                    _ => "seek-end",
                };
                p = match p {
                    P::P1 => P::SeekStart,
                    P::P2 => P::SeekMid,
                    _ => P::SeekEnd,
                };
            }
            P::SeekStart | P::SeekMid | P::SeekEnd if restart_after_seek && pos >= 0.0 || (matches!(p, P::SeekStart | P::SeekMid | P::SeekEnd) && seek_at.elapsed() > Duration::from_secs(15)) => {
                let good = restart_after_seek && pos >= target - 0.1 && pos <= target + 3.0;
                if !good {
                    ok = false;
                }
                results.push(format!("{seek_pending_label}={} (target {target:.1}s -> pos {pos:.1}s, {:.1}s)", if good { "ok" } else { "FAIL" }, seek_at.elapsed().as_secs_f64()));
                last_pos = pos;
                last_adv = Instant::now();
                since = Instant::now();
                p = match p {
                    P::SeekStart => P::P2,
                    P::SeekMid => {
                        if full_realtime {
                            // Return to the beginning and play the whole track in real time.
                            let _ = mpv.command("seek", &["0", "absolute+exact"]);
                            last_pos = 0.0;
                            P::Eof
                        } else {
                            P::P3
                        }
                    }
                    _ => P::Eof,
                };
            }
            _ => {}
        }
    }
    println!(
        "    play: first_audio={} {} eof={eof} wall={:.1}s",
        first_audio.map(|s| format!("{s:.2}s")).unwrap_or_else(|| "never".into()),
        results.join(" "),
        loop_start.elapsed().as_secs_f64()
    );
    if first_audio.is_none() {
        ok = false;
    }
    ok && eof == "EOF"
}

fn video_id(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() == 11 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(s.to_string());
    }
    let i = s.find("v=")?;
    let id: String = s[i + 2..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

/// Picks `n` tracks from Liked Music: distinct channels first, spread across durations.
fn pick_from_liked(root: &Path, auth: &Auth, n: usize) -> Result<Vec<String>, String> {
    let r = run_ytdlp(
        root,
        auth,
        &["--flat-playlist", "--playlist-end", "300", "--yes-playlist", "-J", "https://music.youtube.com/playlist?list=LM"],
        Duration::from_secs(180),
    );
    if r.timed_out {
        return Err("liked-music listing timed out".into());
    }
    let j: serde_json::Value = serde_json::from_slice(&r.stdout)
        .map_err(|_| format!("no JSON from liked-music listing; categories={:?}", classify(&r.stderr)))?;
    let entries = j["entries"].as_array().cloned().unwrap_or_default();
    println!("  liked-music listing: {} entries visible to this session", entries.len());
    if entries.is_empty() {
        return Err(format!("empty liked-music listing (not authenticated, or no likes); categories={:?}", classify(&r.stderr)));
    }
    let mut seen = BTreeSet::new();
    let mut cand: Vec<(f64, String)> = Vec::new();
    for e in &entries {
        let dur = e["duration"].as_f64().unwrap_or(0.0);
        let chan = e["channel"].as_str().or(e["uploader"].as_str()).unwrap_or("").to_string();
        if let Some(id) = e["id"].as_str() && dur > 45.0 && dur < 900.0 && seen.insert(chan) {
            cand.push((dur, id.to_string()));
        }
    }
    cand.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    println!("  distinct-artist candidates: {}", cand.len());
    if cand.is_empty() {
        return Err("no usable candidates".into());
    }
    let mut out = Vec::new();
    for k in 0..n.min(cand.len()) {
        let idx = if n <= 1 { 0 } else { k * (cand.len() - 1) / (n.min(cand.len()) - 1).max(1) };
        let id = cand[idx].1.clone();
        if !out.contains(&id) {
            out.push(id);
        }
    }
    Ok(out)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let root = std::env::current_dir().expect("cwd");
    let auth = if let Some(p) = arg("--cookies") {
        Auth::CookieFile(PathBuf::from(p))
    } else if let Some(b) = arg("--cookies-from-browser") {
        Auth::Browser(b)
    } else {
        Auth::None
    };
    let anonymous_control = args.iter().any(|a| a == "--anonymous-control");
    if matches!(auth, Auth::None) && !anonymous_control {
        eprintln!("refusing to run: no credential source (no guest fallback). Use --cookies or --cookies-from-browser.");
        std::process::exit(2);
    }
    if let Auth::CookieFile(p) = &auth && !p.exists() {
        eprintln!("cookie file not found (this is the missing-credentials negative case)");
        std::process::exit(2);
    }
    let full: Vec<usize> = arg("--full-realtime")
        .map(|s| s.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();

    let ids: Vec<String> = if let Some(n) = arg("--from-liked").and_then(|s| s.parse().ok()) {
        match pick_from_liked(&root, &auth, n) {
            Ok(v) => v,
            Err(e) => {
                println!("RESULT liked-music authenticated listing: FAIL ({e})");
                std::process::exit(1);
            }
        }
    } else if let Some(f) = arg("--ids") {
        std::fs::read_to_string(f).unwrap_or_default().lines().filter_map(video_id).collect()
    } else {
        eprintln!("need --from-liked N or --ids <file>");
        std::process::exit(2);
    };

    println!("tracks to probe: {}", ids.len());
    let mut pass = 0;
    for (i, id) in ids.iter().enumerate() {
        let idx = i + 1;
        println!("  #{idx:02}");
        match resolve(&root, &auth, id) {
            Ok(res) => {
                let _ = &res.summary;
                if play(&res, full.contains(&idx)) {
                    pass += 1;
                    println!("    => PASS");
                } else {
                    println!("    => FAIL (playback)");
                }
            }
            Err(e) => println!("    resolve FAIL: {e}\n    => FAIL (resolution)"),
        }
    }
    println!("RESULT resolution+playback: {pass}/{} tracks passed", ids.len());
    std::process::exit(if pass == ids.len() { 0 } else { 1 });
}
