//! Milestone 0 login spike (ADR-0001 candidate): the user signs in normally inside a WebView2
//! window; the app then reads the YouTube cookies from the WebView2 cookie manager and
//! validates them with an authenticated InnerTube call.
//!
//! Safety properties:
//!  * the user types credentials into Google's own page; this program never sees them;
//!  * cookie VALUES are never printed, logged, put in argv, or written anywhere unless
//!    --export-cookiefile is passed explicitly (for the yt-dlp probe, step 6);
//!  * logged URLs are reduced to host + path (no query/fragment);
//!  * output is a redacted report only.
//!
//! Usage: apollo-m0-login [--wipe] [--export-cookiefile <path>]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use sha1::{Digest, Sha1};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::{PageLoadEvent, WebContext, WebViewBuilder};

// Sign-in entry point used by YouTube Music's own "Sign in" link (ltmpl=music), returning to YTM.
const SIGNIN_URL: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&passive=true&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26next%3Dhttps%253A%252F%252Fmusic.youtube.com%252F";
const ORIGIN: &str = "https://music.youtube.com";

#[derive(Debug)]
enum UserEvent {
    Nav(String),
    Poll,
    Validated(bool),
}

#[derive(Clone)]
struct Captured {
    name: String,
    value: String,
    domain: String,
    path: String,
    secure: bool,
    http_only: bool,
    expires_unix: Option<i64>,
}

/// host + path only; never query or fragment (they can carry tokens).
fn redact_url(u: &str) -> String {
    let no_scheme = u.split("://").nth(1).unwrap_or(u);
    no_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_string()
}

fn sha1_hex(s: &str) -> String {
    let mut h = Sha1::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn authorization_header(cookies: &[Captured]) -> Option<String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let get = |n: &str| cookies.iter().find(|c| c.name == n).map(|c| c.value.clone());
    let mut parts = Vec::new();
    for (scheme, cookie_name) in [
        ("SAPISIDHASH", "SAPISID"),
        ("SAPISID1PHASH", "__Secure-1PAPISID"),
        ("SAPISID3PHASH", "__Secure-3PAPISID"),
    ] {
        if let Some(v) = get(cookie_name) {
            parts.push(format!("{scheme} {ts}_{}", sha1_hex(&format!("{ts} {v} {ORIGIN}"))));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn netscape_line(c: &Captured) -> String {
    let include_sub = if c.domain.starts_with('.') { "TRUE" } else { "FALSE" };
    format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{}",
        c.domain,
        include_sub,
        c.path,
        if c.secure { "TRUE" } else { "FALSE" },
        c.expires_unix.unwrap_or(0),
        c.name,
        c.value
    )
}

/// Redacted summary of what was captured: names and attributes only.
fn print_cookie_report(cookies: &[Captured]) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    println!("captured {} cookies for {ORIGIN}", cookies.len());
    let mut sorted: Vec<&Captured> = cookies.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for c in sorted {
        let life = match c.expires_unix {
            Some(e) => format!("{}d", (e - now) / 86_400),
            None => "session".into(),
        };
        println!(
            "  {:<28} domain={:<22} secure={} httponly={} life={}",
            c.name, c.domain, c.secure, c.http_only, life
        );
    }
    for needed in ["SAPISID", "__Secure-1PAPISID", "__Secure-3PAPISID", "SID", "__Secure-1PSID", "__Secure-3PSID", "LOGIN_INFO"] {
        println!("  has {needed}: {}", cookies.iter().any(|c| c.name == needed));
    }
}

fn validate(cookies: &[Captured], user_agent: &str) -> bool {
    let Some(auth) = authorization_header(cookies) else {
        println!("validate: no SAPISID-family cookie, cannot build Authorization");
        return false;
    };
    let cookie_header = cookies
        .iter()
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ");
    let body = serde_json::json!({
        "context": { "client": {
            "clientName": "WEB_REMIX", "clientVersion": "1.20260901.01.00", "hl": "en", "gl": "US"
        }}
    })
    .to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let resp = agent
        .post("https://music.youtube.com/youtubei/v1/account/account_menu?prettyPrint=false")
        .header("Cookie", &cookie_header)
        .header("Authorization", &auth)
        .header("Origin", ORIGIN)
        .header("X-Origin", ORIGIN)
        .header("X-Goog-AuthUser", "0")
        .header("User-Agent", user_agent)
        .header("Content-Type", "application/json")
        .send(body.as_bytes());
    let mut resp = match resp {
        Ok(r) => r,
        Err(e) => {
            println!("validate: request failed: {e}");
            return false;
        }
    };
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
    // Redacted facts only.
    let logged_in = json
        .pointer("/responseContext/serviceTrackingParams")
        .and_then(|v| v.as_array())
        .map(|svcs| {
            svcs.iter().any(|s| {
                s["params"].as_array().is_some_and(|ps| {
                    ps.iter().any(|p| p["key"] == "logged_in" && p["value"] == "1")
                })
            })
        })
        .unwrap_or(false);
    let has_header = text.contains("activeAccountHeaderRenderer");
    let has_name = json
        .pointer("/actions/0/openPopupAction/popup/multiPageMenuRenderer/header/activeAccountHeaderRenderer/accountName/runs/0/text")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    println!(
        "validate: account_menu http={status} logged_in_flag={logged_in} account_header={has_header} account_name_present={has_name} body_bytes={}",
        text.len()
    );
    status == 200 && logged_in && has_header
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wipe = args.iter().any(|a| a == "--wipe");
    let export_path: Option<PathBuf> = args
        .iter()
        .position(|a| a == "--export-cookiefile")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    let base = PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("Apollo")
        .join("m0-login-webview");
    if wipe {
        let _ = std::fs::remove_dir_all(&base);
        println!("wiped WebView2 profile");
    }
    std::fs::create_dir_all(&base).expect("profile dir");
    println!("webview2 profile dir: {}", base.display());

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = WindowBuilder::new()
        .with_title("Sign in to YouTube Music")
        .with_inner_size(LogicalSize::new(520.0, 780.0))
        .build(&event_loop)
        .expect("window");

    let user_agent = Arc::new(Mutex::new(String::from("Mozilla/5.0")));
    let mut ctx = WebContext::new(Some(base));
    let nav_proxy = proxy.clone();
    let webview = WebViewBuilder::new_with_web_context(&mut ctx)
        .with_url(SIGNIN_URL)
        .with_on_page_load_handler(move |ev, url| {
            if matches!(ev, PageLoadEvent::Finished) {
                let _ = nav_proxy.send_event(UserEvent::Nav(redact_url(&url)));
            }
        })
        .build(&window)
        .expect("webview2");
    let ua_slot = user_agent.clone();
    let _ = webview.evaluate_script_with_callback("navigator.userAgent", move |s| {
        *ua_slot.lock().unwrap() = s.trim_matches('"').to_string();
    });

    // Poll timer: capture is attempted only when we are on music.youtube.com.
    let poll_proxy = proxy.clone();
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(3));
        if poll_proxy.send_event(UserEvent::Poll).is_err() {
            break;
        }
    });

    let started = Instant::now();
    let mut on_ytm = false;
    let mut captured_once = false;
    let mut result_ok: Option<bool> = None;
    let mut seen: Vec<String> = Vec::new();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Nav(u)) => {
                if seen.last() != Some(&u) {
                    println!("nav: {u}");
                    seen.push(u.clone());
                }
                on_ytm = u.starts_with("music.youtube.com");
                if u.contains("signin/rejected") || u.contains("deniedsigninrejected") {
                    println!("NOTE: Google rejected this sign-in attempt (embedded-browser block page).");
                }
            }
            Event::UserEvent(UserEvent::Poll) => {
                if started.elapsed() > Duration::from_secs(900) {
                    println!("RESULT webview2_signin: timeout (no sign-in within 15 min)");
                    std::process::exit(3);
                }
                if on_ytm && !captured_once {
                    let cookies = match webview.cookies_for_url(&format!("{ORIGIN}/")) {
                        Ok(c) => c,
                        Err(e) => {
                            println!("cookie read failed: {e}");
                            return;
                        }
                    };
                    let captured: Vec<Captured> = cookies
                        .iter()
                        .map(|c| Captured {
                            name: c.name().to_string(),
                            value: c.value().to_string(),
                            domain: c.domain().unwrap_or("").to_string(),
                            path: c.path().unwrap_or("/").to_string(),
                            secure: c.secure().unwrap_or(false),
                            http_only: c.http_only().unwrap_or(false),
                            expires_unix: c.expires_datetime().map(|d| d.unix_timestamp()),
                        })
                        .collect();
                    let signed_in = captured.iter().any(|c| c.name == "SAPISID" || c.name == "__Secure-3PAPISID");
                    if !signed_in {
                        println!("on music.youtube.com but no account cookies yet (not signed in)");
                        return;
                    }
                    captured_once = true;
                    print_cookie_report(&captured);
                    if let Some(p) = &export_path {
                        let mut out = String::from("# Netscape HTTP Cookie File\n");
                        for c in &captured {
                            out.push_str(&netscape_line(c));
                            out.push('\n');
                        }
                        match std::fs::write(p, out) {
                            Ok(()) => println!("exported cookie file (private; delete after use): {}", p.display()),
                            Err(e) => println!("export failed: {e}"),
                        }
                    }
                    let ua = user_agent.lock().unwrap().clone();
                    println!("webview user agent: {ua}");
                    let proxy2 = proxy.clone();
                    thread::spawn(move || {
                        let ok = validate(&captured, &ua);
                        let _ = proxy2.send_event(UserEvent::Validated(ok));
                    });
                }
            }
            Event::UserEvent(UserEvent::Validated(ok)) => {
                result_ok = Some(ok);
                println!(
                    "RESULT webview2_signin: signed_in=true cookies_captured=true authenticated_innertube_call={}",
                    if ok { "PASS" } else { "FAIL" }
                );
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if result_ok.is_none() {
                    println!("RESULT webview2_signin: window closed before sign-in completed");
                }
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
