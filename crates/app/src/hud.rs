//! The mini player's HUD mode: how visible it is. Pure state and arithmetic, no window, so it
//! is tested without one; the window end is `ui/mini_player.rs`.
//!
//! * **Hovered or just woken: solid.** The pointer over the HUD, or a track change within the
//!   last [`HOLD`], keeps it fully visible.
//! * **Otherwise (auto-hide on): dim** to [`DIM`], so it stays readable but out of the way.
//! * **Auto-hide off: always solid.**
//!
//! The level moves toward its target at a fixed speed ([`FADE`] for the whole range) instead
//! of jumping.

use std::time::{Duration, Instant};

/// How long the HUD stays solid after a track change (or when it first appears).
pub const HOLD: Duration = Duration::from_secs(4);
/// The level it dims to.
pub const DIM: f32 = 0.30;
/// Time to travel the whole way between dim and solid.
pub const FADE: Duration = Duration::from_millis(300);

/// A snap of the HUD to a screen edge happens once it has sat still this long.
pub const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug)]
pub struct HudFade {
    level: f32,
    auto_hide: bool,
    hovered: bool,
    woke_at: Instant,
    stepped_at: Instant,
}

impl HudFade {
    /// Solid, as if just woken.
    pub fn new(now: Instant, auto_hide: bool) -> Self {
        Self {
            level: 1.0,
            auto_hide,
            hovered: false,
            woke_at: now,
            stepped_at: now,
        }
    }

    /// The current level, 1.0 solid to [`DIM`].
    pub fn level(&self) -> f32 {
        self.level
    }

    /// A new track started: solid again for [`HOLD`].
    pub fn wake(&mut self, now: Instant) {
        self.woke_at = now;
    }

    pub fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }

    pub fn set_auto_hide(&mut self, on: bool, now: Instant) {
        if on && !self.auto_hide {
            // Switching it on starts a fresh hold rather than dimming at once.
            self.woke_at = now;
        }
        self.auto_hide = on;
    }

    /// The level it is heading for at `now`.
    pub fn target(&self, now: Instant) -> f32 {
        let holding = now.saturating_duration_since(self.woke_at) < HOLD;
        if self.hovered || !self.auto_hide || holding {
            1.0
        } else {
            DIM
        }
    }

    /// Moves the level toward its target for the time since the last call. True while it is
    /// still moving (the caller keeps redrawing) or just moved.
    pub fn step(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.stepped_at).as_secs_f32();
        self.stepped_at = now;
        let target = self.target(now);
        if (self.level - target).abs() < f32::EPSILON {
            return false;
        }
        let reach = dt / FADE.as_secs_f32() * (1.0 - DIM);
        self.level = if self.level < target {
            (self.level + reach).min(target)
        } else {
            (self.level - reach).max(target)
        };
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    /// Steps in 50 ms ticks from offset `from` to `to` seconds after `t0`; returns the level.
    fn run(f: &mut HudFade, t0: Instant, from: f32, to: f32) -> f32 {
        let mut t = from;
        while t < to - 1e-4 {
            t += 0.05;
            f.step(t0 + secs(t));
        }
        f.level()
    }

    #[test]
    fn it_starts_solid_and_stays_solid_for_the_hold() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        assert_eq!(f.level(), 1.0);
        assert_eq!(run(&mut f, t0, 0.0, 3.9), 1.0);
    }

    #[test]
    fn it_dims_after_the_hold_and_not_before_the_fade_has_run() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        run(&mut f, t0, 0.0, 4.0);
        let mid = run(&mut f, t0, 4.0, 4.15);
        assert!(mid < 1.0 && mid > DIM, "{mid}");
        assert!((run(&mut f, t0, 4.15, 5.0) - DIM).abs() < 1e-4);
    }

    #[test]
    fn a_track_change_brings_it_back_for_another_hold() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        run(&mut f, t0, 0.0, 6.0);
        assert!((f.level() - DIM).abs() < 1e-4);
        f.wake(t0 + secs(6.0));
        assert!(
            (run(&mut f, t0, 6.0, 6.4) - 1.0).abs() < 1e-4,
            "solid again"
        );
        assert!(
            (run(&mut f, t0, 6.4, 9.9) - 1.0).abs() < 1e-4,
            "still within the hold"
        );
        assert!(
            (run(&mut f, t0, 9.9, 11.0) - DIM).abs() < 1e-4,
            "dim after it"
        );
    }

    #[test]
    fn the_pointer_keeps_it_solid_and_leaving_starts_the_dimming_at_once() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        f.set_hovered(true);
        assert_eq!(run(&mut f, t0, 0.0, 10.0), 1.0);
        f.set_hovered(false);
        // Long past the hold: it starts dimming on the next tick.
        let after = run(&mut f, t0, 10.0, 10.1);
        assert!(after < 1.0, "{after}");
        assert!((run(&mut f, t0, 10.1, 11.0) - DIM).abs() < 1e-4);
    }

    #[test]
    fn with_auto_hide_off_it_never_dims_and_turning_it_on_waits_a_hold() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, false);
        assert_eq!(run(&mut f, t0, 0.0, 30.0), 1.0);
        f.set_auto_hide(true, t0 + secs(30.0));
        assert_eq!(run(&mut f, t0, 30.0, 33.9), 1.0);
        assert!((run(&mut f, t0, 33.9, 35.0) - DIM).abs() < 1e-4);
        f.set_auto_hide(false, t0 + secs(35.0));
        assert!((run(&mut f, t0, 35.0, 35.4) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn step_reports_whether_it_moved() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        assert!(!f.step(t0 + secs(1.0)), "already at its target");
        assert!(f.step(t0 + secs(4.1)), "dimming");
    }
}
