//! Test double for yt-dlp. Behaviour comes from APOLLO_FAKE_MODE; it logs its argv/env/cookie
//! file facts (never the cookie content) to APOLLO_FAKE_LOG so tests can scan for leaks.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, thread};

fn append(path: &str, line: String) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        // One write call, so lines from concurrent processes never interleave.
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

fn doc(id: &str, url: &str, extra: &str) -> String {
    format!(
        r#"{{"id":"{id}","url":"{url}","protocol":"https","acodec":"opus","abr":265.0,"asr":48000,"duration":200.0,"format_id":"774","ext":"webm","http_headers":{{"User-Agent":"FakeUA/1.0","Accept":"*/*","Accept-Language":"en-us","Cookie":"SID=HEADERCANARY"}}{extra}}}"#
    )
}

fn good_url() -> String {
    format!(
        "https://rr1---sn-test.googlevideo.com/videoplayback?expire={}&sig=URLCANARY",
        now() + 3600
    )
}

fn millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()
}

/// The warm-worker mode (`apollo-serve:`): the same one-JSON-object-per-line protocol as the
/// real plugin (`plugin/apollo_serve.py`), with behaviour from the environment:
/// APOLLO_FAKE_MODE (ok, rotate, wrong-id, drm, fail-bot, crash-once, crash-always, hang,
/// startup-fail),
/// APOLLO_FAKE_DELAY_MS (per resolve), APOLLO_FAKE_STARTUP_MS, APOLLO_FAKE_LOG.
fn serve(args: &[String]) {
    use std::io::BufRead;
    let mode = env::var("APOLLO_FAKE_MODE").unwrap_or_else(|_| "ok".into());
    let log = env::var("APOLLO_FAKE_LOG").ok();
    let num = |k: &str| env::var(k).ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    let (delay, startup) = (num("APOLLO_FAKE_DELAY_MS"), num("APOLLO_FAKE_STARTUP_MS"));
    let log_line = |l: String| {
        if let Some(p) = &log {
            append(p, l);
        }
    };
    log_line(format!("SERVE START pid={} args={args:?}", std::process::id()));
    if mode == "startup-fail" {
        // What yt-dlp prints when the worker plugin was not loaded.
        eprintln!("ERROR: Unsupported URL: apollo-serve:");
        std::process::exit(1);
    }
    log_line(format!("SERVE ENV {:?}", env::vars().collect::<Vec<_>>()));
    let out = std::io::stdout();
    let mut out = out.lock();
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let id = req["id"].clone();
        let reply = |out: &mut std::io::StdoutLock, v: serde_json::Value| {
            let _ = writeln!(out, "{v}");
            let _ = out.flush();
        };
        match req["op"].as_str().unwrap_or("") {
            "init" => {
                thread::sleep(Duration::from_millis(startup));
                // Noise before the reply must not break the channel.
                let _ = writeln!(out, "[apollo-serve] Extracting URL: apollo-serve:");
                reply(&mut out, serde_json::json!({"event":"ready","startup_ms":startup}));
            }
            "cookies" => {
                let n = req["netscape"].as_str().map_or(0, |t| t.lines().count());
                log_line(format!("SERVE COOKIES lines={n}"));
                reply(&mut out, serde_json::json!({"id": id, "ok": true, "n": n}));
            }
            "resolve" => {
                let vid = req["video_id"].as_str().unwrap_or("").to_owned();
                log_line(format!("SERVE RESOLVE START {vid} t={}", millis()));
                if mode == "crash-always" {
                    std::process::exit(9);
                }
                if mode == "crash-once" {
                    if let Ok(flag) = env::var("APOLLO_FAKE_FLAG") {
                        if fs::metadata(&flag).is_err() {
                            let _ = fs::write(&flag, "x");
                            std::process::exit(9);
                        }
                    }
                }
                if mode == "hang" {
                    thread::sleep(Duration::from_secs(300));
                }
                thread::sleep(Duration::from_millis(delay));
                let phases = serde_json::json!([["Downloading webpage", 0], ["Downloading player API JSON", delay / 2]]);
                let doc_for = |id: &str, extra: &str| {
                    serde_json::from_str::<serde_json::Value>(&doc(id, &good_url(), extra)).unwrap()
                };
                let mut v = match mode.as_str() {
                    "wrong-id" => serde_json::json!({"id": id, "ok": true, "info": doc_for("zzzzzzzzzzz", "")}),
                    "drm" => serde_json::json!({"id": id, "ok": true, "info": doc_for(&vid, r#","_has_drm":true"#)}),
                    "fail-bot" => serde_json::json!({"id": id, "ok": false,
                        "error": format!("ERROR: [youtube] {vid}: Sign in to confirm you're not a bot")}),
                    _ => serde_json::json!({"id": id, "ok": true, "info": doc_for(&vid, "")}),
                };
                v["phases"] = phases;
                v["ms"] = serde_json::json!(delay);
                if mode == "rotate" {
                    v["jar"] = serde_json::json!("# Netscape HTTP Cookie File\n# rotated-by-worker\n");
                }
                log_line(format!("SERVE RESOLVE END {vid} t={}", millis()));
                reply(&mut out, v);
            }
            "quit" => {
                log_line("SERVE QUIT".into());
                break;
            }
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "apollo-serve:") {
        serve(&args);
        return;
    }
    let mode = env::var("APOLLO_FAKE_MODE").unwrap_or_else(|_| "ok".into());
    let log = env::var("APOLLO_FAKE_LOG").ok();
    let cookie_path = args
        .iter()
        .position(|a| a == "--cookies")
        .and_then(|i| args.get(i + 1))
        .cloned();
    let vid = args
        .last()
        .and_then(|u| u.split("v=").nth(1))
        .unwrap_or("")
        .to_string();

    if let Some(l) = &log {
        append(l, format!("ARGS {args:?}"));
        append(l, format!("ENV {:?}", env::vars().collect::<Vec<_>>()));
        if let Some(p) = &cookie_path {
            let meta = fs::metadata(p);
            append(
                l,
                format!(
                    "COOKIEFILE path={p} exists={} len={}",
                    meta.is_ok(),
                    meta.map(|m| m.len()).unwrap_or(0)
                ),
            );
            // The cookie jar is readable by us (same user) while we run.
            append(l, format!("COOKIEFILE readable={}", fs::read(p).is_ok()));
        }
    }

    match mode.as_str() {
        "ok" => println!("{}", doc(&vid, &good_url(), "")),
        "rotate" => {
            if let Some(p) = &cookie_path {
                let mut s = fs::read_to_string(p).unwrap_or_default();
                s.push_str("# rotated-by-helper\n");
                let _ = fs::write(p, s);
            }
            println!("{}", doc(&vid, &good_url(), ""));
        }
        "wrong-id" => println!("{}", doc("zzzzzzzzzzz", &good_url(), "")),
        "drm" => println!("{}", doc(&vid, &good_url(), r#","_has_drm":true"#)),
        "sabr" => println!(
            "{}",
            doc(&vid, &good_url(), "").replace("\"protocol\":\"https\"", "\"protocol\":\"sabr\"")
        ),
        "bad-host" => println!("{}", doc(&vid, "https://evil.example/videoplayback?expire=1", "")),
        "http-url" => println!(
            "{}",
            doc(&vid, "http://rr1---sn-test.googlevideo.com/videoplayback?expire=1", "")
        ),
        "no-audio" => println!(
            "{}",
            doc(&vid, &good_url(), "").replace("\"acodec\":\"opus\"", "\"acodec\":\"none\"")
        ),
        "garbage" => println!("this is not json"),
        "fail-bot" => {
            eprintln!("ERROR: [youtube] {vid}: Sign in to confirm you're not a bot");
            std::process::exit(1);
        }
        "fail-premium" => {
            eprintln!("ERROR: [youtube] {vid}: This video is available to Music Premium members only");
            std::process::exit(1);
        }
        "fail-429" => {
            eprintln!("ERROR: unable to download: HTTP Error 429: Too Many Requests");
            std::process::exit(1);
        }
        "fail-format" => {
            eprintln!("ERROR: [youtube] {vid}: Requested format is not available");
            std::process::exit(1);
        }
        "fail-net" => {
            eprintln!("ERROR: unable to download video data: <urlopen error timed out>");
            std::process::exit(1);
        }
        "fail-unknown" => {
            eprintln!("ERROR: something nobody has seen before");
            std::process::exit(1);
        }
        "big-stdout" => {
            let out = std::io::stdout();
            let mut out = out.lock();
            let chunk = vec![b'x'; 1 << 16];
            for _ in 0..512 {
                if out.write_all(&chunk).is_err() {
                    break;
                }
            }
            let _ = out.flush();
            thread::sleep(Duration::from_secs(60));
        }
        "hang-tree" => {
            // A grandchild that would outlive a plain kill of this process.
            let me = env::current_exe().unwrap();
            let child = Command::new(me)
                .env("APOLLO_FAKE_MODE", "sleep")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            if let Ok(p) = env::var("APOLLO_FAKE_PIDFILE") {
                append(&p, format!("{} {}", std::process::id(), child.id()));
            }
            thread::sleep(Duration::from_secs(120));
        }
        "sleep" => thread::sleep(Duration::from_secs(120)),
        other => {
            eprintln!("fake-ytdlp: unknown mode {other}");
            std::process::exit(3);
        }
    }
}
