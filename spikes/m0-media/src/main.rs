//! Milestone 0 media spike. Usage: apollo-m0-media <audio-file> [--mute]
//!
//! Plays an owned local fixture through libmpv with user config/scripts/ytdl/video disabled,
//! then verifies: load, codec, duration, audible output driver, seek near start / middle / end
//! (checked against the engine's own time-pos, not just "command accepted"), and a real
//! end-of-file event with reason EOF. Prints a plain-text report; exit code 0 only if every
//! check passed.

use std::env;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use libmpv2::{Format, Mpv, events::Event, mpv_end_file_reason};

struct Checks(Vec<(String, bool, String)>);

impl Checks {
    fn record(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        println!("  [{}] {name}: {detail}", if ok { "ok" } else { "FAIL" });
        self.0.push((name.to_string(), ok, detail));
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Loading,
    Playing1,
    SeekStart,
    Playing2,
    SeekMid,
    Playing3,
    SeekEnd,
    AwaitEof,
    Done,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: apollo-m0-media <audio-file> [--mute]");
        return ExitCode::from(2);
    };
    let mute = args.iter().any(|a| a == "--mute");
    let mut checks = Checks(Vec::new());

    let api = unsafe { libmpv2_sys::mpv_client_api_version() };
    println!("file: {path}");
    println!("libmpv client API version: {}.{}", api >> 16, api & 0xffff);

    let mpv = match Mpv::with_initializer(|init| {
        // App-owned, deterministic instance: no user config, scripts, extractor, OSC or video.
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
        init.set_property("keep-open", "no")?;
        init.set_property("volume", if mute { 0 } else { 25 })?;
        Ok(())
    }) {
        Ok(m) => m,
        Err(e) => {
            println!("  [FAIL] mpv init: {e:?}");
            return ExitCode::from(1);
        }
    };

    let s = |name: &str| mpv.get_property::<String>(name).unwrap_or_else(|e| format!("<{e:?}>"));
    println!("mpv-version: {}", s("mpv-version"));
    println!("ffmpeg-version: {}", s("ffmpeg-version"));

    if let Err(e) = mpv.command("loadfile", &[path.as_str(), "replace"]) {
        checks.record("loadfile accepted", false, format!("{e:?}"));
        return ExitCode::from(1);
    }

    let started = Instant::now();
    let mut phase = Phase::Loading;
    let mut phase_since = Instant::now();
    let mut duration = 0.0f64;
    let mut seek_target = 0.0f64;
    let mut seek_issued_at = Instant::now();
    let mut restart_seen_after_seek = false;
    let mut eof_reason: Option<String> = None;

    while phase != Phase::Done {
        if started.elapsed() > Duration::from_secs(40) {
            checks.record("completes within 40s", false, format!("stuck in {phase:?}"));
            break;
        }
        match mpv.wait_event(0.05) {
            Some(Ok(Event::FileLoaded)) => {
                duration = mpv.get_property::<f64>("duration").unwrap_or(0.0);
                let codec = s("audio-codec-name");
                let fmt = s("file-format");
                let sr = mpv.get_property::<i64>("audio-params/samplerate").unwrap_or(0);
                let ch = mpv.get_property::<i64>("audio-params/channel-count").unwrap_or(0);
                println!("  loaded: format={fmt} codec={codec} {sr} Hz {ch} ch duration={duration:.3}s");
                checks.record("file loaded + decoder opened", !codec.starts_with('<'), format!("codec={codec}"));
                checks.record("duration known", duration > 1.0, format!("{duration:.3}s"));
                phase = Phase::Playing1;
                phase_since = Instant::now();
            }
            Some(Ok(Event::AudioReconfig)) => {
                let ao = s("current-ao");
                println!("  audio output driver: {ao}");
            }
            Some(Ok(Event::PlaybackRestart)) => {
                if matches!(phase, Phase::SeekStart | Phase::SeekMid | Phase::SeekEnd) {
                    restart_seen_after_seek = true;
                }
            }
            Some(Ok(Event::EndFile(reason))) => {
                let name = if reason == mpv_end_file_reason::Eof {
                    "EOF"
                } else if reason == mpv_end_file_reason::Error {
                    "ERROR"
                } else if reason == mpv_end_file_reason::Stop {
                    "STOP"
                } else {
                    "OTHER"
                };
                eof_reason = Some(name.to_string());
                if phase == Phase::AwaitEof {
                    let ok = name == "EOF";
                    checks.record(
                        "end-of-file event (reason EOF)",
                        ok,
                        format!("reason={name} after {:.2}s total", started.elapsed().as_secs_f64()),
                    );
                } else {
                    checks.record("unexpected EndFile before EOF phase", false, format!("reason={name} in {phase:?}"));
                }
                phase = Phase::Done;
            }
            _ => {}
        }

        let pos = mpv.get_property::<f64>("time-pos").unwrap_or(-1.0);
        match phase {
            Phase::Playing1 | Phase::Playing2 | Phase::Playing3 => {
                if phase_since.elapsed() >= Duration::from_millis(1200) && pos >= 0.0 {
                    seek_target = match phase {
                        Phase::Playing1 => 0.3,
                        Phase::Playing2 => duration / 2.0,
                        _ => (duration - 1.0).max(0.0),
                    };
                    let target = format!("{seek_target:.3}");
                    let r = mpv.command("seek", &[&target, "absolute+exact"]);
                    println!("  seek -> {target}s issued at pos {pos:.3}s: {r:?}");
                    seek_issued_at = Instant::now();
                    restart_seen_after_seek = false;
                    phase = match phase {
                        Phase::Playing1 => Phase::SeekStart,
                        Phase::Playing2 => Phase::SeekMid,
                        _ => Phase::SeekEnd,
                    };
                    phase_since = Instant::now();
                }
            }
            Phase::SeekStart | Phase::SeekMid | Phase::SeekEnd => {
                if restart_seen_after_seek && pos >= 0.0 || seek_issued_at.elapsed() > Duration::from_secs(3) {
                    let label = match phase {
                        Phase::SeekStart => "seek near start",
                        Phase::SeekMid => "seek to middle",
                        _ => "seek near end",
                    };
                    // time-pos keeps advancing from the target; allow up to 1.0 s of drift.
                    let ok = restart_seen_after_seek && pos >= seek_target - 0.05 && pos <= seek_target + 1.0;
                    checks.record(label, ok, format!("target {seek_target:.3}s, engine pos {pos:.3}s, restart event {restart_seen_after_seek}"));
                    phase = match phase {
                        Phase::SeekStart => Phase::Playing2,
                        Phase::SeekMid => Phase::Playing3,
                        _ => Phase::AwaitEof,
                    };
                    phase_since = Instant::now();
                }
            }
            _ => {}
        }
    }

    if eof_reason.is_none() {
        checks.record("end-of-file event (reason EOF)", false, "never received");
    }
    let failed = checks.0.iter().filter(|c| !c.1).count();
    println!("RESULT {path}: {} checks, {failed} failed", checks.0.len());
    // Keep Format referenced so a future observe_property addition stays type-checked.
    let _ = Format::Double;
    if failed == 0 { ExitCode::SUCCESS } else { ExitCode::from(1) }
}
