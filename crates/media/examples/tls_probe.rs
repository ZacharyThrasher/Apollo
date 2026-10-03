//! Diagnostic (needs internet): does this mpv build verify TLS with the hardened options?
//! A valid certificate must get past the handshake (failing later as "not media"), an expired
//! one must fail the handshake. Prints only coarse outcomes, never URLs.
//!   cargo run -p apollo-media --example tls_probe

#[cfg(windows)]
mod imp {
    use std::time::{Duration, Instant};
    
    use apollo_media::{core_options, MediaConfig};
    use libmpv2::Mpv;
    
    fn probe(label: &str, url: &str) {
        let cfg = MediaConfig {
            audio_output: Some("null".into()),
            ..Default::default()
        };
        let mpv = Mpv::with_initializer(|init| {
            for (k, v) in core_options(&cfg) {
                init.set_property(k, v.as_str())?;
            }
            Ok(())
        })
        .expect("mpv");
        let level = std::ffi::CString::new("warn").unwrap();
        unsafe { libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), level.as_ptr()) };
        let _ = mpv.command("loadfile", &[url, "replace"]);
        let t0 = Instant::now();
        let mut tls_problem = false;
        let mut handshake_ok_then_not_media = false;
        let mut other: Vec<String> = Vec::new();
        while t0.elapsed() < Duration::from_secs(20) {
            unsafe {
                let ev = &*libmpv2_sys::mpv_wait_event(mpv.ctx.as_ptr(), 0.2);
                if ev.event_id == libmpv2_sys::mpv_event_id_MPV_EVENT_LOG_MESSAGE {
                    let m = &*(ev.data as *const libmpv2_sys::mpv_event_log_message);
                    let text = std::ffi::CStr::from_ptr(m.text).to_string_lossy().to_lowercase();
                    if text.contains("certificate") || text.contains("ssl") || text.contains("tls") {
                        tls_problem = true;
                    }
                    if text.contains("unrecognized file format") || text.contains("failed to recognize") {
                        handshake_ok_then_not_media = true;
                    }
                    if other.len() < 3 && !text.contains("http://") && !text.contains("https://") {
                        other.push(text.trim().to_string());
                    }
                }
                if ev.event_id == libmpv2_sys::mpv_event_id_MPV_EVENT_END_FILE {
                    break;
                }
            }
        }
        println!(
            "{label}: tls_problem={tls_problem} reached_content_stage={handshake_ok_then_not_media} notes={other:?}"
        );
    }
    
    pub fn run() {
        probe("valid certificate (example.com)", "https://example.com/");
        probe("expired certificate (expired.badssl.com)", "https://expired.badssl.com/");
        probe("self-signed (self-signed.badssl.com)", "https://self-signed.badssl.com/");
    }
    
}

#[cfg(windows)]
fn main() {
    imp::run();
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
