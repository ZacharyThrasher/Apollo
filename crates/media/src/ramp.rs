use std::time::{Duration, Instant};

/// The engine's volume-normalization gain in dB, with the smoothing between values.
///
/// A new track's gain is applied at once ([`GainRamp::jump`]): the track before it has just been
/// replaced, so there is nothing to click against. A change while a track is playing (the switch
/// flipped, or its loudness arriving after it started) glides ([`GainRamp::glide`]), linearly in
/// dB, so the level moves instead of stepping. Time is passed in, so tests need no sleeping.
#[derive(Debug, Clone)]
pub struct GainRamp {
    current_db: f32,
    from_db: f32,
    target_db: f32,
    started: Instant,
    duration: Duration,
    /// The value last handed out by [`GainRamp::step`], so an unchanged level is not re-sent.
    sent_db: Option<f32>,
}

/// Steps smaller than this are not worth a property write (0.05 dB is far below audible).
const EPSILON_DB: f32 = 0.05;

impl GainRamp {
    pub fn new(now: Instant) -> Self {
        Self {
            current_db: 0.0,
            from_db: 0.0,
            target_db: 0.0,
            started: now,
            duration: Duration::ZERO,
            sent_db: None,
        }
    }

    /// Go to `db` immediately.
    pub fn jump(&mut self, now: Instant, db: f32) {
        self.current_db = db;
        self.from_db = db;
        self.target_db = db;
        self.started = now;
        self.duration = Duration::ZERO;
    }

    /// Move to `db` over `over`, starting from wherever the level is now.
    pub fn glide(&mut self, now: Instant, db: f32, over: Duration) {
        if (db - self.target_db).abs() < f32::EPSILON {
            return;
        }
        self.from_db = self.current_db;
        self.target_db = db;
        self.started = now;
        self.duration = over;
    }

    pub fn target_db(&self) -> f32 {
        self.target_db
    }

    pub fn is_ramping(&self) -> bool {
        (self.current_db - self.target_db).abs() >= f32::EPSILON
    }

    /// The level to apply now, or `None` when it has not moved since the last call.
    pub fn step(&mut self, now: Instant) -> Option<f32> {
        let elapsed = now.saturating_duration_since(self.started);
        self.current_db = if self.duration.is_zero() || elapsed >= self.duration {
            self.target_db
        } else {
            let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
            self.from_db + (self.target_db - self.from_db) * t
        };
        let settled = (self.current_db - self.target_db).abs() < f32::EPSILON;
        let due = match self.sent_db {
            None => true,
            // Always deliver the final value exactly, even when the last step is tiny.
            Some(sent) => {
                (self.current_db - sent).abs() >= EPSILON_DB || (settled && sent != self.current_db)
            }
        };
        due.then(|| {
            self.sent_db = Some(self.current_db);
            self.current_db
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: fn(u64) -> Duration = Duration::from_millis;

    #[test]
    fn a_jump_is_applied_at_once_and_only_once() {
        let t0 = Instant::now();
        let mut r = GainRamp::new(t0);
        r.jump(t0, -4.0);
        assert!(!r.is_ramping());
        assert_eq!(r.step(t0), Some(-4.0));
        assert_eq!(r.step(t0 + MS(10)), None, "unchanged level is not re-sent");
    }

    #[test]
    fn a_glide_moves_linearly_in_decibels_and_lands_exactly() {
        let t0 = Instant::now();
        let mut r = GainRamp::new(t0);
        r.jump(t0, 0.0);
        r.step(t0);
        r.glide(t0, -8.0, MS(400));
        assert!(r.is_ramping());
        let mid = r.step(t0 + MS(200)).unwrap();
        assert!((mid + 4.0).abs() < 1e-4, "halfway: {mid}");
        assert_eq!(r.step(t0 + MS(400)), Some(-8.0));
        assert!(!r.is_ramping());
        assert_eq!(r.step(t0 + MS(500)), None);
    }

    #[test]
    fn a_glide_that_is_redirected_starts_from_where_it_is() {
        let t0 = Instant::now();
        let mut r = GainRamp::new(t0);
        r.glide(t0, -10.0, MS(100));
        let at = r.step(t0 + MS(50)).unwrap();
        assert!((at + 5.0).abs() < 1e-4);
        r.glide(t0 + MS(50), 0.0, MS(100));
        let back = r.step(t0 + MS(100)).unwrap();
        assert!(
            (back + 2.5).abs() < 1e-3,
            "from -5 toward 0, halfway: {back}"
        );
    }

    #[test]
    fn a_jump_cancels_a_glide() {
        let t0 = Instant::now();
        let mut r = GainRamp::new(t0);
        r.glide(t0, -10.0, MS(1000));
        r.step(t0 + MS(100));
        r.jump(t0 + MS(100), -2.0);
        assert_eq!(r.step(t0 + MS(100)), Some(-2.0));
        assert!(!r.is_ramping());
    }

    #[test]
    fn asking_for_the_level_it_is_heading_to_changes_nothing() {
        let t0 = Instant::now();
        let mut r = GainRamp::new(t0);
        r.glide(t0, -6.0, MS(300));
        let before = r.step(t0 + MS(150)).unwrap();
        r.glide(t0 + MS(150), -6.0, MS(300));
        let after = r.step(t0 + MS(300)).unwrap();
        assert!(
            after < before,
            "kept going down, did not restart: {before} -> {after}"
        );
    }
}
