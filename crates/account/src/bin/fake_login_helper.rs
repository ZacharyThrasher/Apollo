//! Test double for the WebView2 sign-in helper. Behaviour from APOLLO_FAKE_LOGIN_MODE; logs its
//! argv/env and profile-dir facts (never the cookie values it emits) to APOLLO_FAKE_LOGIN_LOG.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;
use std::{env, fs, thread};

fn append(path: &str, line: String) {
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mode = env::var("APOLLO_FAKE_LOGIN_MODE").unwrap_or_else(|_| "ok".into());
    let log = env::var("APOLLO_FAKE_LOGIN_LOG").ok();
    let profile = args
        .iter()
        .position(|a| a == "--profile-dir")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    if let Some(l) = &log {
        append(l, format!("ARGS {args:?}"));
        append(l, format!("ENV {:?}", env::vars().collect::<Vec<_>>()));
        append(l, format!("PROFILE path={profile} exists={}", std::path::Path::new(&profile).exists()));
    }
    // Make the profile dir look like a real WebView2 profile: nested directories and files.
    if !profile.is_empty() {
        let nested = std::path::Path::new(&profile).join("EBWebView").join("Default");
        let _ = fs::create_dir_all(&nested);
        let _ = fs::write(nested.join("Cookies"), b"encrypted-profile-data");
    }

    match mode.as_str() {
        "ok" => {
            let cookies = serde_json::json!([
                {"name":"SAPISID","value":"LOGINCANARY-sapisid","domain":".youtube.com","path":"/","secure":true,"http_only":false,"expires_unix":4102444800i64},
                {"name":"__Secure-3PAPISID","value":"LOGINCANARY-3papisid","domain":".youtube.com","path":"/","secure":true,"http_only":false,"expires_unix":4102444800i64},
                {"name":"SID","value":"LOGINCANARY-sid","domain":".google.com","path":"/","secure":false,"http_only":true,"expires_unix":4102444800i64},
                {"name":"__Secure-3PSID","value":"LOGINCANARY-3psid","domain":".google.com","path":"/","secure":true,"http_only":true,"expires_unix":null},
                {"name":"tracker","value":"x","domain":".example.com","path":"/","secure":false,"http_only":false,"expires_unix":null}
            ]);
            println!("{}", serde_json::json!({ "cookies": cookies }));
        }
        "crash" => {
            // A crash that tries to leak: stderr text must never reach the caller.
            eprintln!("panic: LOGINCANARY-in-stderr");
            std::process::exit(101);
        }
        "closed" => std::process::exit(2),
        "helper-timeout" => std::process::exit(3),
        "garbage" => println!("this is not json"),
        "empty" => println!("{}", serde_json::json!({ "cookies": [] })),
        "big" => {
            let out = std::io::stdout();
            let mut out = out.lock();
            let chunk = vec![b'x'; 1 << 16];
            for _ in 0..64 {
                if out.write_all(&chunk).is_err() {
                    break;
                }
            }
            let _ = out.flush();
            thread::sleep(Duration::from_secs(60));
        }
        "hang-tree" => {
            let me = env::current_exe().unwrap();
            let child = Command::new(me)
                .env("APOLLO_FAKE_LOGIN_MODE", "sleep")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            if let Ok(p) = env::var("APOLLO_FAKE_LOGIN_PIDFILE") {
                append(&p, format!("{} {}", std::process::id(), child.id()));
            }
            thread::sleep(Duration::from_secs(120));
        }
        "sleep" => thread::sleep(Duration::from_secs(120)),
        other => {
            eprintln!("unknown mode {other}");
            std::process::exit(3);
        }
    }
}
