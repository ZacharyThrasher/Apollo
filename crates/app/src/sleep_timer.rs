//! The sleep timer: stop the music after 15, 30, 45 or 60 minutes, or at the end of the track
//! that is playing, with a fade out over about five seconds first.
//!
//! Pure state, so it is testable without a clock or a window: time is passed in, and
//! [`SleepTimer::tick`] answers with what the player should do right now ([`SleepStep`]). The
//! player turns `Fade` into a volume factor on top of the user's volume (never changing the
//! volume setting itself) and `Done` into a pause.
//!
//! * **Minutes.** The countdown runs on the wall clock whether or not anything is playing. At
//!   zero the volume falls to silence over [`FADE`] and the player pauses. If nothing is playing
//!   at zero the timer just ends.
//! * **End of track.** The fade covers the last [`FADE`] of the track and the pause lands
//!   [`END_MARGIN`] before the end, so the next track (gapless or not) never starts. It follows
//!   the playhead, so a seek back restores the level and a seek forward shortens the fade; it
//!   carries over a skip to another track and ends at the end of that one.

use std::time::{Duration, Instant};

/// How long the volume takes to fall to silence.
pub const FADE: Duration = Duration::from_secs(5);
/// End of track: the pause lands this long before the end, so the next track cannot begin.
pub const END_MARGIN: Duration = Duration::from_millis(250);
/// The playhead is only reported a few times a second; between reports it is extrapolated, but
/// never further than this (a stalled stream must not look like it is still advancing).
const MAX_EXTRAPOLATION: Duration = Duration::from_millis(600);

/// What the owner picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SleepChoice {
    Off,
    Minutes(u32),
    EndOfTrack,
}

impl SleepChoice {
    /// The popover's rows, in order.
    pub const MENU: [SleepChoice; 6] = [
        SleepChoice::Off,
        SleepChoice::Minutes(15),
        SleepChoice::Minutes(30),
        SleepChoice::Minutes(45),
        SleepChoice::Minutes(60),
        SleepChoice::EndOfTrack,
    ];

    pub fn label(self) -> String {
        match self {
            SleepChoice::Off => "Off".to_owned(),
            SleepChoice::Minutes(m) => format!("{m} min"),
            SleepChoice::EndOfTrack => "End of track".to_owned(),
        }
    }

    pub fn is_on(self) -> bool {
        self != SleepChoice::Off
    }
}

/// Where the music is, as far as the timer cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Playhead {
    /// The user wants audio right now.
    pub playing: bool,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
}

/// What the player should do now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SleepStep {
    /// Nothing: not time yet, or no timer.
    Idle,
    /// Play at this fraction of the user's volume (1.0 = untouched, 0.0 = silent).
    Fade(f32),
    /// Silent now: pause. The timer is over.
    Done,
    /// A fade was under way and the user paused (or the track ended some other way): bring the
    /// volume back. The timer is over.
    Cancelled,
}

#[derive(Debug)]
pub struct SleepTimer {
    choice: SleepChoice,
    started: Instant,
    /// Minutes: when the fade began.
    fade_from: Option<Instant>,
    /// End of track: the playhead as last reported, and when it was first seen at that value.
    seen: Option<(u64, Instant)>,
    /// A fade is under way (so a pause by the user cancels it instead of being ignored).
    fading: bool,
}

impl Default for SleepTimer {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl SleepTimer {
    pub fn new(now: Instant) -> Self {
        Self {
            choice: SleepChoice::Off,
            started: now,
            fade_from: None,
            seen: None,
            fading: false,
        }
    }

    pub fn choice(&self) -> SleepChoice {
        self.choice
    }

    /// Whether a fade is under way (the volume is below what the user set).
    pub fn is_fading(&self) -> bool {
        self.fading
    }

    /// Start (or restart) the timer with `choice`; `Off` turns it off.
    pub fn set(&mut self, choice: SleepChoice, now: Instant) {
        self.choice = choice;
        self.started = now;
        self.fade_from = None;
        self.seen = None;
        self.fading = false;
    }

    /// The time left on a countdown (not for End of track).
    pub fn time_left(&self, now: Instant) -> Option<Duration> {
        let SleepChoice::Minutes(m) = self.choice else {
            return None;
        };
        Some(
            Duration::from_secs(u64::from(m) * 60)
                .saturating_sub(now.saturating_duration_since(self.started)),
        )
    }

    /// The minutes left, rounded up, for the label on the moon: only while a countdown runs.
    pub fn minutes_left(&self, now: Instant) -> Option<u32> {
        let left = self.time_left(now)?;
        Some(left.as_secs().div_ceil(60) as u32)
    }

    pub fn tick(&mut self, now: Instant, head: Playhead) -> SleepStep {
        match self.choice {
            SleepChoice::Off => SleepStep::Idle,
            SleepChoice::Minutes(m) => self.tick_minutes(now, head, m),
            SleepChoice::EndOfTrack => self.tick_end_of_track(now, head),
        }
    }

    fn finish(&mut self, step: SleepStep) -> SleepStep {
        self.choice = SleepChoice::Off;
        self.fade_from = None;
        self.seen = None;
        self.fading = false;
        step
    }

    fn tick_minutes(&mut self, now: Instant, head: Playhead, minutes: u32) -> SleepStep {
        let deadline = self.started + Duration::from_secs(u64::from(minutes) * 60);
        if self.fade_from.is_none() {
            if now < deadline {
                return SleepStep::Idle;
            }
            if !head.playing {
                // Nothing to fade: the countdown simply ends.
                return self.finish(SleepStep::Idle);
            }
            // The fade is anchored to the deadline, not to whenever the timer was looked at, so a
            // late look joins it where it is (or finds it over) instead of starting it again.
            self.fade_from = Some(deadline);
            self.fading = true;
        }
        if !head.playing {
            // Paused mid-fade: leave it paused, at the user's volume again.
            return self.finish(SleepStep::Cancelled);
        }
        let from = self.fade_from.expect("set above");
        let t = now.saturating_duration_since(from).as_secs_f32() / FADE.as_secs_f32();
        if t >= 1.0 {
            self.finish(SleepStep::Done)
        } else {
            SleepStep::Fade(1.0 - t)
        }
    }

    fn tick_end_of_track(&mut self, now: Instant, head: Playhead) -> SleepStep {
        let (Some(duration), true) = (head.duration_ms, head.playing) else {
            // Nothing playing, or the length is not known yet: nothing to time against. A fade
            // that was under way was cut short by a pause.
            self.seen = None;
            return if std::mem::take(&mut self.fading) {
                SleepStep::Cancelled
            } else {
                SleepStep::Idle
            };
        };
        // Extrapolate between position reports so the fade is smooth rather than stepping at
        // the report rate.
        let at = match self.seen {
            Some((pos, since)) if pos == head.position_ms => {
                now.saturating_duration_since(since).min(MAX_EXTRAPOLATION)
            }
            _ => {
                self.seen = Some((head.position_ms, now));
                Duration::ZERO
            }
        };
        let position = Duration::from_millis(head.position_ms) + at;
        let remaining = Duration::from_millis(duration).saturating_sub(position);
        let usable = remaining.saturating_sub(END_MARGIN);
        if remaining > FADE {
            // Early in the track (or sought back): full volume.
            return if std::mem::take(&mut self.fading) {
                SleepStep::Cancelled
            } else {
                SleepStep::Idle
            };
        }
        if usable.is_zero() {
            return self.finish(SleepStep::Done);
        }
        self.fading = true;
        SleepStep::Fade((usable.as_secs_f32() / (FADE - END_MARGIN).as_secs_f32()).min(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn playing(position_ms: u64, duration_ms: u64) -> Playhead {
        Playhead {
            playing: true,
            position_ms,
            duration_ms: Some(duration_ms),
        }
    }

    const PAUSED: Playhead = Playhead {
        playing: false,
        position_ms: 0,
        duration_ms: Some(200_000),
    };

    #[test]
    fn the_menu_is_off_four_lengths_and_end_of_track() {
        let labels: Vec<_> = SleepChoice::MENU.iter().map(|c| c.label()).collect();
        assert_eq!(
            labels,
            ["Off", "15 min", "30 min", "45 min", "60 min", "End of track"]
        );
        assert!(!SleepChoice::Off.is_on());
        assert!(SleepChoice::EndOfTrack.is_on());
    }

    #[test]
    fn off_never_does_anything() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        assert_eq!(timer.tick(t0 + secs(9999), playing(0, 1000)), SleepStep::Idle);
        assert_eq!(timer.minutes_left(t0), None);
    }

    #[test]
    fn minutes_count_down_rounded_up_and_only_for_a_countdown() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(30), t0);
        assert_eq!(timer.minutes_left(t0), Some(30));
        assert_eq!(timer.minutes_left(t0 + secs(1)), Some(30));
        assert_eq!(timer.minutes_left(t0 + secs(60)), Some(29));
        assert_eq!(timer.minutes_left(t0 + secs(29 * 60 + 1)), Some(1));
        assert_eq!(timer.minutes_left(t0 + secs(31 * 60)), Some(0));
        timer.set(SleepChoice::EndOfTrack, t0);
        assert_eq!(timer.minutes_left(t0), None);
    }

    #[test]
    fn the_volume_is_untouched_until_the_countdown_ends_then_fades_over_five_seconds() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(15), t0);
        let at = |s: f32| t0 + secs(15 * 60) + Duration::from_secs_f32(s);
        let head = playing(1000, 200_000);
        assert_eq!(timer.tick(t0 + secs(15 * 60 - 1), head), SleepStep::Idle);
        assert!(!timer.is_fading());
        assert_eq!(timer.tick(at(0.0), head), SleepStep::Fade(1.0));
        assert!(timer.is_fading());
        let SleepStep::Fade(mid) = timer.tick(at(2.5), head) else {
            panic!("fading");
        };
        assert!((mid - 0.5).abs() < 0.01, "{mid}");
        let SleepStep::Fade(late) = timer.tick(at(4.5), head) else {
            panic!("fading");
        };
        assert!(late < 0.15 && late > 0.0, "{late}");
        assert_eq!(timer.tick(at(5.0), head), SleepStep::Done);
        // Over: the timer is off and says nothing more.
        assert_eq!(timer.choice(), SleepChoice::Off);
        assert_eq!(timer.tick(at(6.0), head), SleepStep::Idle);
        assert!(!timer.is_fading());
    }

    #[test]
    fn a_late_look_joins_the_fade_where_it_is_or_finds_it_over() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(15), t0);
        let end = t0 + secs(15 * 60);
        // First looked at 2 s after the deadline: the fade is 2 s along, not just starting.
        let SleepStep::Fade(f) = timer.tick(end + secs(2), playing(0, 200_000)) else {
            panic!("fading");
        };
        assert!((f - 0.6).abs() < 0.01, "{f}");
        // First looked at well after the fade would have finished: silent and paused at once.
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(15), t0);
        assert_eq!(timer.tick(end + secs(60), playing(0, 200_000)), SleepStep::Done);
    }

    #[test]
    fn a_countdown_that_ends_while_paused_just_ends() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(15), t0);
        assert_eq!(timer.tick(t0 + secs(20 * 60), PAUSED), SleepStep::Idle);
        assert_eq!(timer.choice(), SleepChoice::Off);
    }

    #[test]
    fn pausing_in_the_middle_of_a_fade_brings_the_volume_back() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(15), t0);
        let end = t0 + secs(15 * 60);
        assert!(matches!(
            timer.tick(end, playing(0, 200_000)),
            SleepStep::Fade(_)
        ));
        assert_eq!(
            timer.tick(end + secs(2), PAUSED),
            SleepStep::Cancelled
        );
        assert_eq!(timer.choice(), SleepChoice::Off);
    }

    #[test]
    fn picking_again_restarts_the_countdown_and_clears_a_fade() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::Minutes(15), t0);
        let end = t0 + secs(15 * 60);
        assert!(matches!(
            timer.tick(end, playing(0, 200_000)),
            SleepStep::Fade(_)
        ));
        timer.set(SleepChoice::Minutes(30), end + secs(1));
        assert!(!timer.is_fading());
        assert_eq!(timer.tick(end + secs(2), playing(0, 200_000)), SleepStep::Idle);
        assert_eq!(timer.minutes_left(end + secs(1)), Some(30));
    }

    #[test]
    fn end_of_track_is_silent_until_the_last_five_seconds() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        assert_eq!(timer.tick(t0, playing(60_000, 200_000)), SleepStep::Idle);
        // 6 s left: still untouched.
        assert_eq!(timer.tick(t0, playing(194_000, 200_000)), SleepStep::Idle);
        assert!(!timer.is_fading());
        // 5 s left: the fade begins from (almost) full.
        let SleepStep::Fade(start) = timer.tick(t0, playing(195_000, 200_000)) else {
            panic!("fading");
        };
        assert!(start > 0.95 && start <= 1.0, "{start}");
        let SleepStep::Fade(mid) = timer.tick(t0, playing(197_500, 200_000)) else {
            panic!("fading");
        };
        assert!(mid > 0.4 && mid < 0.6, "{mid}");
    }

    #[test]
    fn end_of_track_pauses_just_before_the_end_so_the_next_track_never_starts() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        assert!(matches!(
            timer.tick(t0, playing(199_000, 200_000)),
            SleepStep::Fade(_)
        ));
        // Within the margin of the end: silent, pause.
        assert_eq!(timer.tick(t0, playing(199_800, 200_000)), SleepStep::Done);
        assert_eq!(timer.choice(), SleepChoice::Off);
    }

    #[test]
    fn end_of_track_extrapolates_between_position_reports() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        let SleepStep::Fade(a) = timer.tick(t0, playing(197_000, 200_000)) else {
            panic!("fading");
        };
        // Same reported position 200 ms later: the level has still moved down.
        let SleepStep::Fade(b) = timer.tick(t0 + Duration::from_millis(200), playing(197_000, 200_000))
        else {
            panic!("fading");
        };
        assert!(b < a, "{b} < {a}");
        // A stalled stream does not keep extrapolating forever.
        let SleepStep::Fade(c) = timer.tick(t0 + secs(30), playing(197_000, 200_000)) else {
            panic!("still fading, not done: the stream stalled");
        };
        assert!(c > 0.0);
    }

    #[test]
    fn seeking_back_in_the_last_seconds_restores_the_volume() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        assert!(matches!(
            timer.tick(t0, playing(197_000, 200_000)),
            SleepStep::Fade(_)
        ));
        assert_eq!(timer.tick(t0, playing(100_000, 200_000)), SleepStep::Cancelled);
        // Still armed for the end of the track.
        assert_eq!(timer.choice(), SleepChoice::EndOfTrack);
        assert_eq!(timer.tick(t0, playing(100_500, 200_000)), SleepStep::Idle);
    }

    #[test]
    fn end_of_track_survives_a_skip_and_waits_for_a_length() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        // A new track whose length is not known yet.
        let unknown = Playhead {
            playing: true,
            position_ms: 0,
            duration_ms: None,
        };
        assert_eq!(timer.tick(t0, unknown), SleepStep::Idle);
        assert_eq!(timer.choice(), SleepChoice::EndOfTrack);
        assert_eq!(timer.tick(t0, playing(0, 180_000)), SleepStep::Idle);
    }

    #[test]
    fn pausing_during_the_end_of_track_fade_restores_the_volume_and_keeps_the_timer() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        assert!(matches!(
            timer.tick(t0, playing(197_000, 200_000)),
            SleepStep::Fade(_)
        ));
        let paused = Playhead {
            playing: false,
            position_ms: 197_000,
            duration_ms: Some(200_000),
        };
        assert_eq!(timer.tick(t0, paused), SleepStep::Cancelled);
        assert_eq!(timer.choice(), SleepChoice::EndOfTrack);
        assert_eq!(timer.tick(t0, paused), SleepStep::Idle);
    }

    #[test]
    fn a_track_shorter_than_the_fade_fades_from_where_it_is() {
        let t0 = Instant::now();
        let mut timer = SleepTimer::new(t0);
        timer.set(SleepChoice::EndOfTrack, t0);
        // 3 s left of a 4 s clip: the fade starts from less than full and still ends silent.
        let SleepStep::Fade(f) = timer.tick(t0, playing(1_000, 4_000)) else {
            panic!("fading");
        };
        assert!(f < 0.7 && f > 0.3, "{f}");
        assert_eq!(timer.tick(t0, playing(3_900, 4_000)), SleepStep::Done);
    }
}
