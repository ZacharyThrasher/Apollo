//! The mini player's HUD mode: how visible it is. Pure state and arithmetic, no window, so it
//! is tested without one; the window end is `ui/mini_player.rs`.
//!
//! * **Hovered or just woken: solid.** The pointer over the HUD, or a track change within the
//!   last [`HOLD`], keeps it fully visible.
//! * **Otherwise (auto-hide on): dim** to [`DIM`], so it stays readable but out of the way.
//! * **Auto-hide off: always solid.**
//!
//! The level moves toward its target at a fixed speed ([`FADE`] for the whole range) instead
//! of jumping. With Windows' animation effects off (`set_animated(false)`) every change is
//! instant.
//!
//! [`IdleFade`] is the plain mini player's version of the same idea (Settings > Mini player):
//! after [`IDLE_AFTER`] without the pointer the controls fade out, then the whole window dims.
//!
//! Two smaller motions live here too: the panel going fully opaque under the pointer
//! ([`SOLIDIFY`]) and the swap of title, artist and art on a track change ([`Swap`]).

use std::time::{Duration, Instant};

/// How long the HUD stays solid after a track change (or when it first appears).
pub const HOLD: Duration = Duration::from_secs(4);
/// The level it dims to.
pub const DIM: f32 = 0.30;
/// Time to travel the whole way between dim and solid.
pub const FADE: Duration = Duration::from_millis(300);

/// Time for the panel to go from its resting fill to fully opaque under the pointer.
pub const SOLIDIFY: Duration = Duration::from_millis(120);
/// Time for the title, artist and art to change over on a new track.
pub const SWAP: Duration = Duration::from_millis(160);
/// How far the text slides on a change, in pixels.
pub const SWAP_SLIDE: f32 = 6.0;

/// A snap of the HUD to a screen edge happens once it has sat still this long.
pub const SETTLE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug)]
pub struct HudFade {
    level: f32,
    auto_hide: bool,
    hovered: bool,
    /// 0 at the panel's resting fill, 1 fully opaque (the pointer is over it).
    solid: f32,
    animated: bool,
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
            solid: 0.0,
            animated: true,
            woke_at: now,
            stepped_at: now,
        }
    }

    /// The current level, 1.0 solid to [`DIM`].
    pub fn level(&self) -> f32 {
        self.level
    }

    /// How opaque the panel is under the pointer, 0 resting to 1 solid.
    pub fn solid(&self) -> f32 {
        self.solid
    }

    /// Whether changes are eased (false: they are instant).
    pub fn set_animated(&mut self, animated: bool) {
        self.animated = animated;
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
        let solid_target = if self.hovered { 1.0 } else { 0.0 };
        let (before, solid_before) = (self.level, self.solid);
        if self.animated {
            let reach = dt / FADE.as_secs_f32() * (1.0 - DIM);
            self.level = approach(self.level, target, reach);
            self.solid = approach(self.solid, solid_target, dt / SOLIDIFY.as_secs_f32());
        } else {
            self.level = target;
            self.solid = solid_target;
        }
        (self.level - before).abs() > f32::EPSILON
            || (self.solid - solid_before).abs() > f32::EPSILON
    }
}

/// How long the plain mini player waits without the pointer (or a track change) before it
/// starts to fade.
pub const IDLE_AFTER: Duration = Duration::from_secs(4);
/// Time for the controls to fade out.
pub const CONTROLS_FADE: Duration = Duration::from_millis(150);
/// The window's level once dimmed.
pub const IDLE_DIM: f32 = 0.60;
/// Time to travel the whole way between solid and [`IDLE_DIM`].
pub const DIM_FADE: Duration = Duration::from_millis(300);

/// How visible the plain mini player is: its controls (previous, play/pause, next, volume) and
/// the window as a whole. The pointer coming back restores both at once; leaving, or a quiet
/// [`IDLE_AFTER`], fades them: the controls first, then the window.
#[derive(Clone, Copy, Debug)]
pub struct IdleFade {
    controls: f32,
    window: f32,
    fade_controls: bool,
    dim: bool,
    hovered: bool,
    animated: bool,
    woke_at: Instant,
    stepped_at: Instant,
}

impl IdleFade {
    /// Everything showing, as if just woken.
    pub fn new(now: Instant, fade_controls: bool, dim: bool) -> Self {
        Self {
            controls: 1.0,
            window: 1.0,
            fade_controls,
            dim,
            hovered: false,
            animated: true,
            woke_at: now,
            stepped_at: now,
        }
    }

    /// How visible the controls are, 0 to 1.
    pub fn controls(&self) -> f32 {
        self.controls
    }

    /// The whole window's level, 1.0 solid to [`IDLE_DIM`].
    pub fn window(&self) -> f32 {
        self.window
    }

    /// Whether changes are eased (false: they are instant).
    pub fn set_animated(&mut self, animated: bool) {
        self.animated = animated;
    }

    /// A new track started: everything shows again for [`IDLE_AFTER`].
    pub fn wake(&mut self, now: Instant) {
        self.woke_at = now;
    }

    /// The pointer arrived or left. Arriving restores everything at once; leaving starts a
    /// fresh wait.
    pub fn set_hovered(&mut self, hovered: bool, now: Instant) {
        if hovered {
            self.controls = 1.0;
            self.window = 1.0;
        } else if self.hovered {
            self.woke_at = now;
        }
        self.hovered = hovered;
    }

    /// The two settings; turning either on starts a fresh wait rather than fading at once.
    pub fn set_options(&mut self, fade_controls: bool, dim: bool, now: Instant) {
        if (fade_controls && !self.fade_controls) || (dim && !self.dim) {
            self.woke_at = now;
        }
        self.fade_controls = fade_controls;
        self.dim = dim;
    }

    fn idle_for(&self, now: Instant) -> Duration {
        if self.hovered {
            Duration::ZERO
        } else {
            now.saturating_duration_since(self.woke_at)
        }
    }

    fn controls_target(&self, now: Instant) -> f32 {
        if self.fade_controls && self.idle_for(now) >= IDLE_AFTER {
            0.0
        } else {
            1.0
        }
    }

    /// The window dims once the controls have faded (at once when they are not fading).
    fn window_target(&self, now: Instant) -> f32 {
        let wait = if self.fade_controls {
            IDLE_AFTER + CONTROLS_FADE
        } else {
            IDLE_AFTER
        };
        if self.dim && self.idle_for(now) >= wait {
            IDLE_DIM
        } else {
            1.0
        }
    }

    /// Moves both toward their targets for the time since the last call. True when either
    /// moved.
    pub fn step(&mut self, now: Instant) -> bool {
        let dt = now.saturating_duration_since(self.stepped_at).as_secs_f32();
        self.stepped_at = now;
        let (controls_to, window_to) = (self.controls_target(now), self.window_target(now));
        let (controls_before, window_before) = (self.controls, self.window);
        if self.animated {
            self.controls = approach(self.controls, controls_to, dt / CONTROLS_FADE.as_secs_f32());
            self.window = approach(
                self.window,
                window_to,
                dt / DIM_FADE.as_secs_f32() * (1.0 - IDLE_DIM),
            );
        } else {
            self.controls = controls_to;
            self.window = window_to;
        }
        (self.controls - controls_before).abs() > f32::EPSILON
            || (self.window - window_before).abs() > f32::EPSILON
    }
}

/// Time for one lyric line to cross-fade into the next in the mini player and HUD.
pub const LYRIC_FADE: Duration = Duration::from_millis(200);

/// The cross-fade between two lyric lines: linear progress while it runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct LineFade {
    started: Option<Instant>,
}

impl LineFade {
    pub fn start(&mut self, now: Instant) {
        self.started = Some(now);
    }

    /// A cross-fade has been started and not yet seen through to its end.
    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }

    /// How far along it is, 0 to 1, or `None` when no change is under way. With animation off
    /// it is over at once.
    pub fn progress(&mut self, now: Instant, animated: bool) -> Option<f32> {
        let started = self.started?;
        let t = now.saturating_duration_since(started).as_secs_f32() / LYRIC_FADE.as_secs_f32();
        if !animated || t >= 1.0 {
            self.started = None;
            return None;
        }
        Some(t)
    }
}

/// Volume steps for a mouse-wheel (or touchpad) scroll, from `acc` pixels still carried over
/// plus this event's vertical `delta`. A wheel notch (`lines`) is one step per event; a touchpad
/// (pixels) adds up to one step per [`WHEEL_PIXELS`]. Positive is up, louder. Returns the
/// steps and the pixels carried to the next event.
pub fn wheel_steps(acc: f32, delta: f32, lines: bool) -> (i32, f32) {
    if lines {
        return (delta.signum() as i32 * i32::from(delta != 0.0), 0.0);
    }
    let total = acc + delta;
    let steps = (total / WHEEL_PIXELS).trunc();
    (steps as i32, total - steps * WHEEL_PIXELS)
}

/// Touchpad scroll, in pixels, that makes one volume step.
pub const WHEEL_PIXELS: f32 = 40.0;
/// A volume step: the mouse wheel changes the volume by this many percent.
pub const WHEEL_VOLUME_STEP: i8 = 5;

/// `value` moved `reach` toward `target`, not past it.
fn approach(value: f32, target: f32, reach: f32) -> f32 {
    if value < target {
        (value + reach).min(target)
    } else {
        (value - reach).max(target)
    }
}

/// The change-over of title, artist and art on a new track: eased progress while it runs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Swap {
    started: Option<Instant>,
}

impl Swap {
    pub fn start(&mut self, now: Instant) {
        self.started = Some(now);
    }

    /// A change-over has been started and not yet seen through to its end.
    pub fn is_running(&self) -> bool {
        self.started.is_some()
    }

    /// How far along it is, 0 to 1 (eased out), or `None` when nothing is changing over. With
    /// animation off it is over at once.
    pub fn progress(&mut self, now: Instant, animated: bool) -> Option<f32> {
        let started = self.started?;
        let t = now.saturating_duration_since(started).as_secs_f32() / SWAP.as_secs_f32();
        if !animated || t >= 1.0 {
            self.started = None;
            return None;
        }
        Some(1.0 - (1.0 - t) * (1.0 - t))
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
    fn the_panel_goes_opaque_under_the_pointer_over_the_solidify_time() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, false);
        f.set_hovered(true);
        let mid = run(&mut f, t0, 0.0, 0.05);
        assert!(
            mid == 1.0 && f.solid() > 0.0 && f.solid() < 1.0,
            "{}",
            f.solid()
        );
        run(&mut f, t0, 0.05, 0.2);
        assert_eq!(f.solid(), 1.0);
        f.set_hovered(false);
        run(&mut f, t0, 0.2, 0.4);
        assert_eq!(f.solid(), 0.0);
    }

    #[test]
    fn with_animation_off_every_change_is_instant() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        f.set_animated(false);
        f.set_hovered(true);
        assert!(f.step(t0 + secs(0.01)));
        assert_eq!(f.solid(), 1.0);
        f.set_hovered(false);
        f.step(t0 + secs(5.0));
        assert_eq!(f.level(), DIM);
        assert_eq!(f.solid(), 0.0);
        let mut swap = Swap::default();
        swap.start(t0);
        assert_eq!(swap.progress(t0, false), None);
    }

    #[test]
    fn a_swap_eases_out_over_its_time_and_then_ends() {
        let t0 = Instant::now();
        let mut swap = Swap::default();
        assert_eq!(swap.progress(t0, true), None, "nothing to change over");
        swap.start(t0);
        let early = swap.progress(t0 + secs(0.02), true).unwrap();
        let mid = swap.progress(t0 + secs(0.08), true).unwrap();
        assert!(early > 0.0 && early < mid && mid < 1.0, "{early} {mid}");
        assert!(
            mid > 0.5,
            "eased out: more than half done by the halfway point"
        );
        assert_eq!(swap.progress(t0 + secs(0.2), true), None);
        assert_eq!(
            swap.progress(t0 + secs(0.21), true),
            None,
            "and stays ended"
        );
    }

    #[test]
    fn step_reports_whether_it_moved() {
        let t0 = Instant::now();
        let mut f = HudFade::new(t0, true);
        assert!(!f.step(t0 + secs(1.0)), "already at its target");
        assert!(f.step(t0 + secs(4.1)), "dimming");
    }

    #[test]
    fn idle_controls_fade_after_the_wait_and_then_the_window_dims() {
        let t0 = Instant::now();
        let mut f = IdleFade::new(t0, true, true);
        let mut at = 0.0f32;
        let mut run = |f: &mut IdleFade, to: f32| {
            while at < to - 1e-4 {
                at += 0.05;
                f.step(t0 + secs(at));
            }
        };
        run(&mut f, 3.9);
        assert_eq!((f.controls(), f.window()), (1.0, 1.0));
        run(&mut f, 4.05);
        assert!(f.controls() < 1.0 && f.controls() > 0.0, "{}", f.controls());
        assert_eq!(f.window(), 1.0, "the window waits for the controls");
        run(&mut f, 4.2);
        assert_eq!(f.controls(), 0.0);
        run(&mut f, 4.35);
        assert!(f.window() < 1.0 && f.window() > IDLE_DIM, "{}", f.window());
        run(&mut f, 5.0);
        assert!((f.window() - IDLE_DIM).abs() < 1e-4);
    }

    #[test]
    fn hovering_restores_everything_at_once_and_leaving_waits_again() {
        let t0 = Instant::now();
        let mut f = IdleFade::new(t0, true, true);
        f.step(t0 + secs(9.0));
        assert_eq!((f.controls(), f.window()), (0.0, 0.6));
        f.set_hovered(true, t0 + secs(9.0));
        assert_eq!((f.controls(), f.window()), (1.0, 1.0), "no easing back up");
        f.step(t0 + secs(30.0));
        assert_eq!(
            (f.controls(), f.window()),
            (1.0, 1.0),
            "stays while hovered"
        );
        f.set_hovered(false, t0 + secs(30.0));
        f.step(t0 + secs(33.0));
        assert_eq!((f.controls(), f.window()), (1.0, 1.0), "a fresh wait");
        f.step(t0 + secs(34.1));
        assert_eq!((f.controls(), f.window()), (0.0, 1.0));
    }

    #[test]
    fn each_idle_option_works_alone() {
        let t0 = Instant::now();
        let mut only_dim = IdleFade::new(t0, false, true);
        only_dim.step(t0 + secs(4.01));
        only_dim.step(t0 + secs(6.0));
        assert_eq!(only_dim.controls(), 1.0);
        assert!(
            (only_dim.window() - IDLE_DIM).abs() < 1e-4,
            "no controls to wait for"
        );
        let mut only_fade = IdleFade::new(t0, true, false);
        only_fade.step(t0 + secs(5.0));
        only_fade.step(t0 + secs(6.0));
        assert_eq!((only_fade.controls(), only_fade.window()), (0.0, 1.0));
        // Turning an option on after a long quiet starts a fresh wait.
        let mut late = IdleFade::new(t0, false, false);
        late.step(t0 + secs(20.0));
        late.set_options(true, true, t0 + secs(20.0));
        late.step(t0 + secs(23.0));
        assert_eq!((late.controls(), late.window()), (1.0, 1.0));
    }

    #[test]
    fn idle_changes_are_instant_with_animation_off_and_a_track_wakes_it() {
        let t0 = Instant::now();
        let mut f = IdleFade::new(t0, true, true);
        f.set_animated(false);
        f.step(t0 + secs(5.0));
        assert_eq!((f.controls(), f.window()), (0.0, IDLE_DIM));
        f.wake(t0 + secs(5.0));
        f.step(t0 + secs(5.1));
        assert_eq!((f.controls(), f.window()), (1.0, 1.0));
    }

    #[test]
    fn wheel_notches_step_once_and_touchpad_scrolls_add_up() {
        assert_eq!(wheel_steps(0.0, 3.0, true), (1, 0.0));
        assert_eq!(wheel_steps(0.0, -3.0, true), (-1, 0.0));
        assert_eq!(wheel_steps(0.0, 0.0, true), (0, 0.0));
        assert_eq!(wheel_steps(0.0, 15.0, false), (0, 15.0));
        let (steps, rest) = wheel_steps(15.0, 30.0, false);
        assert_eq!(steps, 1);
        assert!((rest - 5.0).abs() < 1e-4);
        let (steps, rest) = wheel_steps(-30.0, -20.0, false);
        assert_eq!(steps, -1);
        assert!((rest + 10.0).abs() < 1e-4);
    }

    #[test]
    fn a_lyric_line_fades_over_its_time_and_then_ends() {
        let t0 = Instant::now();
        let mut fade = LineFade::default();
        assert_eq!(fade.progress(t0, true), None);
        fade.start(t0);
        assert!(fade.is_running());
        let mid = fade.progress(t0 + secs(0.1), true).unwrap();
        assert!((mid - 0.5).abs() < 1e-3, "{mid}");
        assert_eq!(fade.progress(t0 + secs(0.25), true), None);
        assert!(!fade.is_running());
        fade.start(t0);
        assert_eq!(fade.progress(t0, false), None, "instant with animation off");
    }
}
