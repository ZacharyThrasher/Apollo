//! A short memory of what the app printed to stderr, for the diagnostics report.
//!
//! A packaged Windows build has no console, so everything the app says through `eprintln!` is
//! lost the moment it is printed. The crate shadows `eprintln!` (see `lib.rs`) with a macro that
//! prints as before and also keeps the last [`CAPACITY`] lines here. Lines are kept as printed:
//! nothing is scrubbed on the way in, because the report scrubs the whole text on the way out,
//! and nothing here is ever written to disk.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Mutex;

use apollo_services_common::unix_now;

/// How many lines are kept; the oldest go first.
pub const CAPACITY: usize = 400;
/// A longer line is cut here, so one runaway message cannot push everything else out.
pub const MAX_LINE_CHARS: usize = 300;

/// One printed line and when it was printed (seconds since the Unix epoch).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub at_unix: u64,
    pub text: String,
}

/// The remembered lines. A type of its own so tests can use a private one: the real log is
/// shared by every thread of the process, tests included.
pub struct Ring {
    lines: Mutex<VecDeque<Entry>>,
}

impl Default for Ring {
    fn default() -> Self {
        Self::new()
    }
}

impl Ring {
    pub const fn new() -> Self {
        Self {
            lines: Mutex::new(VecDeque::new()),
        }
    }

    /// Remembers `text`, one entry per line.
    pub fn record_at(&self, text: &str, at_unix: u64) {
        let mut log = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            if log.len() == CAPACITY {
                log.pop_front();
            }
            log.push_back(Entry {
                at_unix,
                text: line.chars().take(MAX_LINE_CHARS).collect(),
            });
        }
    }

    /// The remembered lines, oldest first.
    pub fn snapshot(&self) -> Vec<Entry> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }
}

static LOG: Ring = Ring::new();

/// What the crate's `eprintln!` calls: prints to stderr as usual and remembers the line.
pub fn print(args: fmt::Arguments<'_>) {
    let text = args.to_string();
    std::eprintln!("{text}");
    record(&text);
}

/// Remembers `text` in the process-wide log.
pub fn record(text: &str) {
    LOG.record_at(text, unix_now());
}

/// The process-wide log, oldest first.
pub fn snapshot() -> Vec<Entry> {
    LOG.snapshot()
}

/// Whether a line reads like something going wrong. The app has no log levels, only messages
/// ("could not save setting", "sign-in failed"), so the report's "recent errors" are the lines
/// that say so in words.
pub fn looks_like_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "could not",
        "couldn't",
        "cannot",
        "can't",
        "failed",
        "failure",
        "error",
        "unavailable",
        "rejected",
        "unable",
        "expired",
        "denied",
        "panicked",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_lines_in_order_and_splits_on_newlines() {
        let ring = Ring::new();
        ring.record_at("first\nsecond\n\nthird", 10);
        let all = ring.snapshot();
        assert_eq!(
            all.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["first", "second", "third"]
        );
        assert!(all.iter().all(|e| e.at_unix == 10));
    }

    #[test]
    fn drops_the_oldest_beyond_capacity_and_cuts_long_lines() {
        let ring = Ring::new();
        for n in 0..CAPACITY + 5 {
            ring.record_at(&format!("line {n}"), 0);
        }
        let all = ring.snapshot();
        assert_eq!(all.len(), CAPACITY);
        assert_eq!(all[0].text, "line 5");
        let ring = Ring::new();
        ring.record_at(&"x".repeat(MAX_LINE_CHARS * 3), 0);
        assert_eq!(ring.snapshot()[0].text.chars().count(), MAX_LINE_CHARS);
    }

    #[test]
    fn error_lines_are_recognised_by_their_words() {
        assert!(looks_like_error("could not save setting volume: disk full"));
        assert!(looks_like_error("sign-in failed: network"));
        assert!(!looks_like_error("latency resolve=210 open=90"));
    }

    /// The crate's own `eprintln!` (see lib.rs) keeps what it prints.
    #[test]
    fn the_crates_eprintln_lands_in_the_log() {
        eprintln!("diagnostics test marker {}", 41 + 1);
        assert!(snapshot()
            .iter()
            .any(|e| e.text == "diagnostics test marker 42"));
    }
}
