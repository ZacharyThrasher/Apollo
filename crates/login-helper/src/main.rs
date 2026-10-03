//! Sign-in helper (ADR 0002). Runs as its own process so the GPUI app never gets a second event
//! loop. The owner signs in on Google's own page inside WebView2 (passkeys and 2FA work as in a
//! browser); this process never sees the password. When the YouTube account cookies appear it
//! prints them as ONE JSON line on stdout, which is the parent's anonymous pipe and therefore the
//! only channel they travel on. They are never written to a file, put in argv, or logged.
//!
//! Usage: apollo-login-helper --profile-dir <dir> [--timeout-secs N]
//! Exit codes: 0 cookies delivered, 2 window closed before sign-in, 3 timeout, 4 setup error.
//! stderr carries only `host/path` navigation notes (no query strings).

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("the sign-in helper is Windows-only");
    std::process::exit(4);
}

#[cfg(windows)]
fn main() {
    imp::run();
}

#[cfg(windows)]
mod imp {
    use std::io::Write;
    use std::path::PathBuf;
    use std::thread;
    use std::time::{Duration, Instant};

    use tao::dpi::LogicalSize;
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::window::WindowBuilder;
    use wry::{PageLoadEvent, WebContext, WebViewBuilder};

    // YouTube Music's own "Sign in" entry point (ltmpl=music), returning to YTM.
    const SIGNIN_URL: &str = "https://accounts.google.com/ServiceLogin?ltmpl=music&service=youtube&passive=true&continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26next%3Dhttps%253A%252F%252Fmusic.youtube.com%252F";

    #[derive(Debug)]
    enum UserEvent {
        Nav(String),
        Poll,
    }

    /// host + path only; a query or fragment can carry tokens.
    fn redact_url(u: &str) -> String {
        let no_scheme = u.split("://").nth(1).unwrap_or(u);
        no_scheme.split(['?', '#']).next().unwrap_or("").to_string()
    }

    pub fn run() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1));
        let Some(profile) = arg("--profile-dir").map(PathBuf::from) else {
            eprintln!("missing --profile-dir");
            std::process::exit(4);
        };
        let timeout = Duration::from_secs(
            arg("--timeout-secs")
                .and_then(|s| s.parse().ok())
                .unwrap_or(900),
        );

        let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
        let proxy = event_loop.create_proxy();
        let window = match WindowBuilder::new()
            .with_title("Sign in to YouTube Music")
            .with_inner_size(LogicalSize::new(520.0, 780.0))
            .build(&event_loop)
        {
            Ok(w) => w,
            Err(_) => std::process::exit(4),
        };
        let mut ctx = WebContext::new(Some(profile));
        let nav_proxy = proxy.clone();
        let webview = match WebViewBuilder::new_with_web_context(&mut ctx)
            .with_url(SIGNIN_URL)
            .with_on_page_load_handler(move |ev, url| {
                if matches!(ev, PageLoadEvent::Finished) {
                    let _ = nav_proxy.send_event(UserEvent::Nav(redact_url(&url)));
                }
            })
            .build(&window)
        {
            Ok(w) => w,
            Err(_) => {
                eprintln!("webview2 unavailable");
                std::process::exit(4);
            }
        };

        let poll_proxy = proxy.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(2));
            if poll_proxy.send_event(UserEvent::Poll).is_err() {
                break;
            }
        });

        let started = Instant::now();
        let mut on_ytm = false;
        let mut last_nav = String::new();
        event_loop.run(move |event, _, control_flow| {
            *control_flow = ControlFlow::Wait;
            match event {
                Event::UserEvent(UserEvent::Nav(u)) => {
                    if u != last_nav {
                        eprintln!("nav: {u}");
                        last_nav = u.clone();
                    }
                    on_ytm = u.starts_with("music.youtube.com");
                    if u.contains("signin/rejected") || u.contains("deniedsignin") {
                        eprintln!("note: Google rejected this sign-in attempt");
                    }
                }
                Event::UserEvent(UserEvent::Poll) => {
                    if started.elapsed() > timeout {
                        std::process::exit(3);
                    }
                    if !on_ytm {
                        return;
                    }
                    let Ok(cookies) = webview.cookies_for_url("https://music.youtube.com/") else {
                        return;
                    };
                    let signed_in = cookies
                        .iter()
                        .any(|c| c.name() == "SAPISID" || c.name() == "__Secure-3PAPISID");
                    if !signed_in {
                        return; // on YTM but not signed in yet
                    }
                    let list: Vec<serde_json::Value> = cookies
                        .iter()
                        .map(|c| {
                            serde_json::json!({
                                "name": c.name(),
                                "value": c.value(),
                                "domain": c.domain().unwrap_or(""),
                                "path": c.path().unwrap_or("/"),
                                "secure": c.secure().unwrap_or(false),
                                "http_only": c.http_only().unwrap_or(false),
                                "expires_unix": c.expires_datetime().map(|d| d.unix_timestamp()),
                            })
                        })
                        .collect();
                    let line = serde_json::json!({ "cookies": list }).to_string();
                    let mut out = std::io::stdout().lock();
                    let ok = out.write_all(line.as_bytes()).is_ok()
                        && out.write_all(b"\n").is_ok()
                        && out.flush().is_ok();
                    std::process::exit(if ok { 0 } else { 4 });
                }
                Event::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                } => std::process::exit(2),
                _ => {}
            }
        });
    }
}
