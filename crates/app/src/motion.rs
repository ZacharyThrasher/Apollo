//! Interaction motion that is not a one-shot GPUI animation.

use std::time::Instant;

use apollo_theme::tokens::motion;

/// 0-1 hover tint level that follows the pointer over 120 ms (ease-out cubic), restarting from
/// wherever it currently is when the pointer enters or leaves mid-fade.
#[derive(Clone, Copy, Debug, Default)]
pub struct HoverFade {
    hovered: bool,
    from: f32,
    changed_at: Option<Instant>,
    instant: bool,
}

impl HoverFade {
    pub fn set(&mut self, hovered: bool, now: Instant, animations_enabled: bool) {
        if hovered == self.hovered {
            return;
        }
        self.from = self.level(now).0;
        self.hovered = hovered;
        self.changed_at = Some(now);
        self.instant = !animations_enabled;
    }

    pub fn is_hovered(&self) -> bool {
        self.hovered
    }

    /// Current level and whether it is still moving.
    pub fn level(&self, now: Instant) -> (f32, bool) {
        let target = if self.hovered { 1.0 } else { 0.0 };
        let Some(at) = self.changed_at else {
            return (target, false);
        };
        if self.instant {
            return (target, false);
        }
        let elapsed = now.saturating_duration_since(at);
        if elapsed >= motion::HOVER_PRESS {
            return (target, false);
        }
        let t = motion::ease_out_cubic(elapsed.as_secs_f32() / motion::HOVER_PRESS.as_secs_f32());
        (self.from + (target - self.from) * t, true)
    }
}

/// 0-1 "how open" a panel is, following open/close over the panel duration (200 ms, ease-out
/// cubic) and restarting from wherever it currently is when toggled mid-slide.
#[derive(Clone, Copy, Debug, Default)]
pub struct PanelSlide {
    open: bool,
    from: f32,
    changed_at: Option<Instant>,
    instant: bool,
}

impl PanelSlide {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set(&mut self, open: bool, now: Instant, animations_enabled: bool) {
        if open == self.open {
            return;
        }
        self.from = self.level(now).0;
        self.open = open;
        self.changed_at = Some(now);
        self.instant = !animations_enabled;
    }

    /// Current openness and whether it is still moving.
    pub fn level(&self, now: Instant) -> (f32, bool) {
        let target = if self.open { 1.0 } else { 0.0 };
        let Some(at) = self.changed_at else {
            return (target, false);
        };
        if self.instant {
            return (target, false);
        }
        let elapsed = now.saturating_duration_since(at);
        if elapsed >= motion::PANEL {
            return (target, false);
        }
        let t = motion::ease_out_cubic(elapsed.as_secs_f32() / motion::PANEL.as_secs_f32());
        (self.from + (target - self.from) * t, true)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn rises_over_120ms_and_settles() {
        let t0 = Instant::now();
        let mut f = HoverFade::default();
        assert_eq!(f.level(t0), (0.0, false));
        f.set(true, t0, true);
        let (mid, moving) = f.level(t0 + Duration::from_millis(30));
        assert!(moving && mid > 0.0 && mid < 1.0);
        assert_eq!(f.level(t0 + Duration::from_millis(120)), (1.0, false));
    }

    #[test]
    fn leaving_mid_fade_starts_from_the_current_level() {
        let t0 = Instant::now();
        let mut f = HoverFade::default();
        f.set(true, t0, true);
        let t1 = t0 + Duration::from_millis(40);
        let (seen, _) = f.level(t1);
        f.set(false, t1, true);
        let (after, moving) = f.level(t1);
        assert!(moving);
        assert!((after - seen).abs() < 1e-5);
        assert_eq!(f.level(t1 + Duration::from_millis(200)), (0.0, false));
    }

    #[test]
    fn a_panel_slides_open_over_200ms_and_closed_again() {
        let t0 = Instant::now();
        let mut p = PanelSlide::default();
        assert_eq!(p.level(t0), (0.0, false));
        p.set(true, t0, true);
        let (mid, moving) = p.level(t0 + Duration::from_millis(60));
        assert!(moving && mid > 0.0 && mid < 1.0);
        assert_eq!(p.level(t0 + Duration::from_millis(200)), (1.0, false));
        let t1 = t0 + Duration::from_secs(1);
        p.set(false, t1, true);
        assert!(p.level(t1 + Duration::from_millis(60)).0 < 1.0);
        assert_eq!(p.level(t1 + Duration::from_millis(200)), (0.0, false));
    }

    #[test]
    fn closing_mid_slide_starts_from_where_it_is() {
        let t0 = Instant::now();
        let mut p = PanelSlide::default();
        p.set(true, t0, true);
        let t1 = t0 + Duration::from_millis(80);
        let (seen, _) = p.level(t1);
        p.set(false, t1, true);
        assert!((p.level(t1).0 - seen).abs() < 1e-5);
    }

    #[test]
    fn a_panel_is_instant_without_animations() {
        let t0 = Instant::now();
        let mut p = PanelSlide::default();
        p.set(true, t0, false);
        assert_eq!(p.level(t0), (1.0, false));
        p.set(false, t0, false);
        assert_eq!(p.level(t0), (0.0, false));
    }

    #[test]
    fn instant_without_animations() {
        let t0 = Instant::now();
        let mut f = HoverFade::default();
        f.set(true, t0, false);
        assert_eq!(f.level(t0), (1.0, false));
    }
}
