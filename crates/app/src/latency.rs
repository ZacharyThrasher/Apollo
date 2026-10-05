//! "How long until I hear it?" Measures the gap between a play request and the first sound, split
//! into the stages the user waits through, and appends one line per start to a small log
//! (`%LOCALAPPDATA%\Apollo\app\latency.log`). Only durations and a request kind are recorded:
//! no ids, titles, URLs or account data. (The `yt_request kind=.. video_id=..` lines the resolver
//! writes to the same file do carry a video id; see `docs/resolver-speed.md`.)
//!
//! * `resolve_ms`: request to the stream URL being ready (0 when a prefetched URL was used, `-`
//!   when the preloaded next track was adopted without a resolution).
//! * `open_ms`: URL ready to the audio engine reporting the file open.
//! * `audio_ms`: engine open to the first position update, i.e. audio actually moving (granular
//!   to the 250 ms position interval).
//! * `total_ms`: request to first audio, the number that matters.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Past this the log is moved aside at startup, so it cannot grow without bound.
const MAX_LOG_BYTES: u64 = 256 * 1024;

#[derive(Debug, Default)]
pub struct Latency {
    started: Option<Started>,
    path: Option<PathBuf>,
}

#[derive(Debug)]
struct Started {
    kind: &'static str,
    at: Instant,
    resolved: Option<Instant>,
    opened: Option<Instant>,
}

impl Latency {
    pub fn set_log(&mut self, path: Option<PathBuf>) {
        if let Some(p) = &path {
            rotate(p);
        }
        self.path = path;
    }

    /// The user asked for something to start playing.
    pub fn begin(&mut self, kind: &'static str, now: Instant) {
        self.started = Some(Started {
            kind,
            at: now,
            resolved: None,
            opened: None,
        });
    }

    /// The user stopped waiting (pause, stop, a newer request replaces it via `begin`).
    pub fn cancel(&mut self) {
        self.started = None;
    }

    pub fn resolved(&mut self, now: Instant) {
        if let Some(s) = &mut self.started {
            s.resolved.get_or_insert(now);
        }
    }

    pub fn opened(&mut self, now: Instant) {
        if let Some(s) = &mut self.started {
            s.opened.get_or_insert(now);
        }
    }

    /// First sign of audio moving: completes the measurement and writes the line.
    pub fn audio(&mut self, now: Instant) -> Option<String> {
        let s = self.started.take()?;
        let line = format_line(&s, now);
        eprintln!("latency {line}");
        if let Some(path) = &self.path {
            append(path, &line);
        }
        Some(line)
    }
}

fn ms(d: Duration) -> u128 {
    d.as_millis()
}

fn format_line(s: &Started, now: Instant) -> String {
    let resolve = s
        .resolved
        .map_or("-".to_owned(), |r| ms(r.saturating_duration_since(s.at)).to_string());
    let open_from = s.resolved.unwrap_or(s.at);
    let (open, audio) = match s.opened {
        Some(o) => (
            ms(o.saturating_duration_since(open_from)).to_string(),
            ms(now.saturating_duration_since(o)).to_string(),
        ),
        None => ("-".to_owned(), "-".to_owned()),
    };
    format!(
        "kind={} resolve_ms={resolve} open_ms={open} audio_ms={audio} total_ms={}",
        s.kind,
        ms(now.saturating_duration_since(s.at))
    )
}

fn append(path: &Path, line: &str) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(format!("t={stamp} {line}\n").as_bytes());
    }
}

fn rotate(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(path, path.with_extension("old"));
    }
}

/// `%LOCALAPPDATA%\Apollo\app\latency.log` (none where the variable is unset).
pub fn default_log() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("Apollo").join("app").join("latency.log"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn splits_the_wait_into_stages() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.begin("play_tracks", t0);
        l.resolved(at(t0, 1800));
        l.opened(at(t0, 1850));
        let line = l.audio(at(t0, 2100)).unwrap();
        assert_eq!(
            line,
            "kind=play_tracks resolve_ms=1800 open_ms=50 audio_ms=250 total_ms=2100"
        );
        assert!(l.audio(at(t0, 3000)).is_none(), "one line per start");
    }

    #[test]
    fn an_adopted_preload_has_no_resolve_stage() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.begin("next", t0);
        l.opened(at(t0, 40));
        assert_eq!(
            l.audio(at(t0, 290)).unwrap(),
            "kind=next resolve_ms=- open_ms=40 audio_ms=250 total_ms=290"
        );
    }

    #[test]
    fn only_the_first_event_of_a_stage_counts_and_cancel_discards() {
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.begin("play_now", t0);
        l.resolved(at(t0, 10));
        l.resolved(at(t0, 999));
        l.cancel();
        assert!(l.audio(at(t0, 500)).is_none());
        l.begin("play_now", t0);
        l.resolved(at(t0, 10));
        l.resolved(at(t0, 999));
        l.opened(at(t0, 60));
        assert!(l.audio(at(t0, 100)).unwrap().contains("resolve_ms=10 "));
    }

    #[test]
    fn lines_land_in_the_log_with_a_timestamp_and_no_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app").join("latency.log");
        let t0 = Instant::now();
        let mut l = Latency::default();
        l.set_log(Some(path.clone()));
        l.begin("next", t0);
        l.opened(at(t0, 10));
        l.audio(at(t0, 20));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("t="), "{text}");
        assert!(text.contains("kind=next"), "{text}");
    }

    #[test]
    fn an_oversized_log_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("latency.log");
        std::fs::write(&path, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        let mut l = Latency::default();
        l.set_log(Some(path.clone()));
        assert!(!path.exists());
        assert!(dir.path().join("latency.old").exists());
    }
}
