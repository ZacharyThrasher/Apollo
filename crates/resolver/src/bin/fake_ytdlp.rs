//! Test double for yt-dlp. Behaviour comes from APOLLO_FAKE_MODE; it logs its argv/env/cookie
//! file facts (never the cookie content) to APOLLO_FAKE_LOG so tests can scan for leaks.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{env, fs, thread};

fn append(path: &str, line: String) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
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

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
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
