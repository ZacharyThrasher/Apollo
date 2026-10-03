use std::time::{Duration, Instant};

/// Limits position events to one per `interval` and drops repeats (about 4 Hz by default).
/// Time is passed in, so tests need no sleeping.
pub struct PositionCoalescer {
    interval: Duration,
    last_emit: Option<Instant>,
    last_ms: Option<u64>,
}

impl PositionCoalescer {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit: None,
            last_ms: None,
        }
    }

    /// `Some(ms)` when an event should be sent now.
    pub fn offer(&mut self, now: Instant, position_ms: u64) -> Option<u64> {
        if self.last_ms == Some(position_ms) {
            return None;
        }
        if let Some(t) = self.last_emit {
            if now.duration_since(t) < self.interval {
                return None;
            }
        }
        self.last_emit = Some(now);
        self.last_ms = Some(position_ms);
        Some(position_ms)
    }

    /// Forget history (new load or after a seek) so the next sample is sent immediately.
    pub fn reset(&mut self) {
        self.last_emit = None;
        self.last_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_four_per_second_and_no_repeats() {
        let mut c = PositionCoalescer::new(Duration::from_millis(250));
        let t0 = Instant::now();
        let mut sent = Vec::new();
        // mpv-ish: a sample every 10 ms for one second.
        for i in 0..100u64 {
            let now = t0 + Duration::from_millis(i * 10);
            if let Some(ms) = c.offer(now, i * 10) {
                sent.push(ms);
            }
        }
        assert!(sent.len() <= 5, "{sent:?}");
        assert!(sent.len() >= 4, "{sent:?}");
        assert_eq!(sent[0], 0);
        // A repeated value is never re-sent, even after the interval.
        assert_eq!(
            c.offer(t0 + Duration::from_secs(5), *sent.last().unwrap()),
            None
        );
    }

    #[test]
    fn reset_sends_the_next_sample_immediately() {
        let mut c = PositionCoalescer::new(Duration::from_secs(10));
        let t = Instant::now();
        assert_eq!(c.offer(t, 1000), Some(1000));
        assert_eq!(c.offer(t, 2000), None);
        c.reset();
        assert_eq!(c.offer(t, 2000), Some(2000));
    }
}
