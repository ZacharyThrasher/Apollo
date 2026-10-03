//! Accent crossfade (DIRECTION.md: 400 ms, ease-out cubic, interpolated in OKLCH along the
//! shortest hue path). Pure state so it can be tested without a window.

use std::time::{Duration, Instant};

use apollo_theme::tokens::motion;
use apollo_theme::Accent;

#[derive(Clone, Copy, Debug)]
pub struct AccentAnim {
    from: Accent,
    to: Accent,
    started: Option<Instant>,
    duration: Duration,
}

impl AccentAnim {
    pub fn new(accent: Accent) -> Self {
        Self {
            from: accent,
            to: accent,
            started: None,
            duration: motion::ACCENT_CROSSFADE,
        }
    }

    /// The accent to draw at `now`, and whether it is still moving (so the caller keeps
    /// asking for frames).
    pub fn at(&self, now: Instant) -> (Accent, bool) {
        let Some(started) = self.started else {
            return (self.to, false);
        };
        let elapsed = now.saturating_duration_since(started);
        if elapsed >= self.duration {
            return (self.to, false);
        }
        let t = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        (self.from.lerp(self.to, motion::ease_out_cubic(t)), true)
    }

    pub fn target(&self) -> Accent {
        self.to
    }

    /// Start a crossfade from whatever is on screen now. With animations off the change is
    /// instant (`motion::effective`).
    pub fn retarget(&mut self, now: Instant, to: Accent, animations_enabled: bool) {
        if to.oklch == self.to.oklch {
            return;
        }
        let (current, _) = self.at(now);
        self.from = current;
        self.to = to;
        self.duration = motion::effective(motion::ACCENT_CROSSFADE, animations_enabled);
        self.started = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::art::{DecodedArt, SyntheticArt};

    fn accent(h: f32) -> Accent {
        let a = DecodedArt::from_rgba(SyntheticArt::Hue(h).render(32)).accent;
        assert!(!a.is_fallback);
        a
    }

    #[test]
    fn crossfades_over_400ms_then_settles() {
        let t0 = Instant::now();
        let mut a = AccentAnim::new(accent(20.0));
        a.retarget(t0, accent(200.0), true);
        let (start, moving) = a.at(t0);
        assert!(moving);
        assert!((start.oklch.h - accent(20.0).oklch.h).abs() < 0.5);
        let (mid, moving) = a.at(t0 + Duration::from_millis(100));
        assert!(moving);
        let (lo, hi) = (accent(20.0).oklch.h, accent(200.0).oklch.h);
        assert!(mid.oklch.h > lo.min(hi) && mid.oklch.h < lo.max(hi));
        let (end, moving) = a.at(t0 + Duration::from_millis(400));
        assert!(!moving);
        assert_eq!(end.oklch, accent(200.0).oklch);
    }

    #[test]
    fn instant_when_animations_are_off() {
        let t0 = Instant::now();
        let mut a = AccentAnim::new(accent(20.0));
        a.retarget(t0, accent(200.0), false);
        let (now, moving) = a.at(t0);
        assert!(!moving);
        assert_eq!(now.oklch, accent(200.0).oklch);
    }

    #[test]
    fn hue_takes_the_short_way_round() {
        let t0 = Instant::now();
        // Hues 330 and 20 (after the generator's offsets) sit either side of 0 degrees.
        let (from, to) = (accent(310.0), accent(350.0));
        let (hf, ht) = (from.oklch.h, to.oklch.h);
        let short = ((ht - hf + 540.0).rem_euclid(360.0) - 180.0).abs();
        let mut a = AccentAnim::new(from);
        a.retarget(t0, to, true);
        let (mid, _) = a.at(t0 + Duration::from_millis(60));
        let from_mid = ((mid.oklch.h - hf + 540.0).rem_euclid(360.0) - 180.0).abs();
        assert!(
            from_mid <= short + 0.5,
            "went the long way: {hf} -> {} -> {ht}",
            mid.oklch.h
        );
    }

    #[test]
    fn retarget_midway_starts_from_what_is_on_screen() {
        let t0 = Instant::now();
        let mut a = AccentAnim::new(accent(20.0));
        a.retarget(t0, accent(200.0), true);
        let t1 = t0 + Duration::from_millis(120);
        let (seen, _) = a.at(t1);
        a.retarget(t1, accent(300.0), true);
        let (restart, _) = a.at(t1);
        assert!((restart.oklch.h - seen.oklch.h).abs() < 0.5);
    }
}
