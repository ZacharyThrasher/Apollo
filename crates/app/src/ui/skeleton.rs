//! The loading pulse every screen's skeleton shares: a low-amplitude opacity swing, static when
//! the system has animation effects off (DIRECTION.md, Account screen decisions 4).

use std::time::Duration;

use gpui::{prelude::*, Animation, AnimationExt, AnyElement, Styled};

/// One pulse cycle.
pub const PERIOD: Duration = Duration::from_millis(1200);
/// Opacity at the dimmest point; it swings between this and 1.0.
pub const MIN_OPACITY: f32 = 0.85;

/// Opacity at `t` (0 to 1 through a cycle): the minimum at the start, full midway.
fn opacity(t: f32) -> f32 {
    MIN_OPACITY + (1.0 - MIN_OPACITY) * (1.0 - (2.0 * t - 1.0).abs())
}

/// `el` pulsing under `id`, or as it is when `animate` is false.
pub fn pulse<E: IntoElement + Styled + 'static>(
    el: E,
    id: &'static str,
    animate: bool,
) -> AnyElement {
    if animate {
        el.with_animation(id, Animation::new(PERIOD).repeat(), |el, t| {
            el.opacity(opacity(t))
        })
        .into_any_element()
    } else {
        el.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pulse_stays_between_the_minimum_and_full_opacity() {
        for step in 0..=100 {
            let o = opacity(step as f32 / 100.0);
            assert!((MIN_OPACITY..=1.0).contains(&o), "{o}");
        }
        assert_eq!(opacity(0.0), MIN_OPACITY);
        assert_eq!(opacity(0.5), 1.0);
    }
}
