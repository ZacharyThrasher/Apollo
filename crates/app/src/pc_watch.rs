//! What the PC does around the player: the sound output changing or going away, the PC sleeping
//! and waking, and another app starting to make sound. This is the decision half, pure so it is
//! testable anywhere; `pc_watch_win.rs` takes the snapshots on Windows (Core Audio) and feeds
//! them in.
//!
//! * **Output lost.** The default playback device's id changed or there is none now (headphones
//!   unplugged, Bluetooth dropped, a dock removed), or the wall clock jumped between two polls
//!   (the PC slept; the suspend notice usually arrives first, this is the backstop). The player
//!   pauses in place and reopens the track on the next Play.
//! * **Other audio.** Another process (not Apollo, not the system-sounds session) has been
//!   audible for [`SUSTAINED`]: a movie, a call. Short sounds never reach that, so a chat
//!   notification or an error ding does not pause the music. Each stretch of sound pauses at most
//!   once, so pressing Play while the movie is still going is not fought, and nothing resumes by
//!   itself.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

/// How long another app must keep playing before it counts (a notification is shorter).
pub const SUSTAINED: Duration = Duration::from_secs(2);
/// Silence shorter than this does not end a stretch of sound (a pause between sentences).
pub const QUIET_GAP: Duration = Duration::from_secs(3);
/// How often the snapshots are taken.
pub const POLL: Duration = Duration::from_millis(250);
/// A wall clock that moved this much more than the poll interval means the PC was asleep.
pub const WAKE_JUMP: Duration = Duration::from_secs(4);

/// What the player should be told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcEvent {
    /// The output changed or went away, or the PC slept: pause in place.
    OutputLost,
    /// Another app has been playing for a while.
    OtherAudio,
}

/// One other process's sound at this instant. Apollo's own process and the system-sounds session
/// are left out by the platform end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sound {
    pub process: u32,
    /// Its session is active and the level is above silence.
    pub audible: bool,
}

/// One look at the machine.
#[derive(Clone, Debug)]
pub struct Snapshot<'a> {
    /// The default playback device's id; `None` when there is none.
    pub device: Option<&'a str>,
    pub sounds: &'a [Sound],
    pub wall: SystemTime,
}

struct Stretch {
    began: Instant,
    last_audible: Instant,
    /// Already reported: this stretch is over before it can pause anything again.
    reported: bool,
}

#[derive(Default)]
pub struct PcWatch {
    device: Option<Option<String>>,
    wall: Option<SystemTime>,
    stretches: HashMap<u32, Stretch>,
}

impl PcWatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn poll(&mut self, now: Instant, snap: &Snapshot<'_>) -> Vec<PcEvent> {
        let mut events = Vec::new();

        // The PC slept: the wall clock jumped further than the poll interval explains.
        if let Some(prev) = self.wall {
            if snap
                .wall
                .duration_since(prev)
                .is_ok_and(|gap| gap > POLL + WAKE_JUMP)
            {
                events.push(PcEvent::OutputLost);
            }
        }
        self.wall = Some(snap.wall);

        // The default device is not the one there was. The first look only learns it.
        let device = snap.device.map(str::to_owned);
        match &self.device {
            Some(known) if *known != device => {
                if !events.contains(&PcEvent::OutputLost) {
                    events.push(PcEvent::OutputLost);
                }
            }
            _ => {}
        }
        self.device = Some(device);

        // Other apps' sound.
        for sound in snap.sounds.iter().filter(|s| s.audible) {
            let stretch = self.stretches.entry(sound.process).or_insert(Stretch {
                began: now,
                last_audible: now,
                reported: false,
            });
            if now.saturating_duration_since(stretch.last_audible) > QUIET_GAP {
                // A new stretch after a long quiet.
                *stretch = Stretch {
                    began: now,
                    last_audible: now,
                    reported: false,
                };
            }
            stretch.last_audible = now;
            if !stretch.reported && now.saturating_duration_since(stretch.began) >= SUSTAINED {
                stretch.reported = true;
                events.push(PcEvent::OtherAudio);
            }
        }
        // Forget processes that have been quiet for a good while.
        self.stretches
            .retain(|_, s| now.saturating_duration_since(s.last_audible) <= QUIET_GAP * 4);
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Clock {
        t0: Instant,
        wall0: SystemTime,
        at: Duration,
    }

    impl Clock {
        fn new() -> Self {
            Self {
                t0: Instant::now(),
                wall0: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
                at: Duration::ZERO,
            }
        }

        fn advance(&mut self, by: Duration) {
            self.at += by;
        }

        fn now(&self) -> Instant {
            self.t0 + self.at
        }

        fn wall(&self) -> SystemTime {
            self.wall0 + self.at
        }
    }

    fn look(
        watch: &mut PcWatch,
        clock: &Clock,
        device: Option<&str>,
        sounds: &[Sound],
    ) -> Vec<PcEvent> {
        watch.poll(
            clock.now(),
            &Snapshot {
                device,
                sounds,
                wall: clock.wall(),
            },
        )
    }

    const MOVIE: Sound = Sound {
        process: 42,
        audible: true,
    };

    #[test]
    fn the_first_look_only_learns_the_device() {
        let mut watch = PcWatch::new();
        let clock = Clock::new();
        assert!(look(&mut watch, &clock, Some("speakers"), &[]).is_empty());
        let mut watch = PcWatch::new();
        assert!(look(&mut watch, &clock, None, &[]).is_empty());
    }

    #[test]
    fn a_changed_or_vanished_default_device_loses_the_output_once() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        look(&mut watch, &clock, Some("speakers"), &[]);
        clock.advance(POLL);
        assert!(look(&mut watch, &clock, Some("speakers"), &[]).is_empty());
        // Headphones plugged in and become the default.
        clock.advance(POLL);
        assert_eq!(
            look(&mut watch, &clock, Some("headphones"), &[]),
            [PcEvent::OutputLost]
        );
        clock.advance(POLL);
        assert!(look(&mut watch, &clock, Some("headphones"), &[]).is_empty());
        // Unplugged with nothing to fall back to.
        clock.advance(POLL);
        assert_eq!(look(&mut watch, &clock, None, &[]), [PcEvent::OutputLost]);
        clock.advance(POLL);
        assert!(look(&mut watch, &clock, None, &[]).is_empty());
        // And back.
        clock.advance(POLL);
        assert_eq!(
            look(&mut watch, &clock, Some("speakers"), &[]),
            [PcEvent::OutputLost]
        );
    }

    #[test]
    fn a_long_gap_between_looks_is_the_pc_waking_from_sleep() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        look(&mut watch, &clock, Some("speakers"), &[]);
        clock.advance(POLL);
        assert!(look(&mut watch, &clock, Some("speakers"), &[]).is_empty());
        // A slow poll is not sleep.
        clock.advance(Duration::from_secs(2));
        assert!(look(&mut watch, &clock, Some("speakers"), &[]).is_empty());
        // An hour of it is, and a device change at the same moment is still one event.
        clock.advance(Duration::from_secs(3600));
        assert_eq!(
            look(&mut watch, &clock, Some("headphones"), &[]),
            [PcEvent::OutputLost]
        );
    }

    #[test]
    fn a_clock_set_backwards_is_not_a_wake() {
        let mut watch = PcWatch::new();
        let clock = Clock::new();
        look(&mut watch, &clock, Some("speakers"), &[]);
        let events = watch.poll(
            clock.now() + POLL,
            &Snapshot {
                device: Some("speakers"),
                sounds: &[],
                wall: clock.wall() - Duration::from_secs(3600),
            },
        );
        assert!(events.is_empty());
    }

    #[test]
    fn another_app_must_play_for_two_seconds_before_it_counts() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        let mut events = Vec::new();
        // A one-second ding.
        for _ in 0..4 {
            events.extend(look(&mut watch, &clock, Some("d"), &[MOVIE]));
            clock.advance(POLL);
        }
        for _ in 0..20 {
            events.extend(look(&mut watch, &clock, Some("d"), &[]));
            clock.advance(POLL);
        }
        assert!(events.is_empty(), "{events:?}");
        // A movie.
        let mut at = None;
        for i in 0..40 {
            if look(&mut watch, &clock, Some("d"), &[MOVIE]) == [PcEvent::OtherAudio] {
                at.get_or_insert(i);
            }
            clock.advance(POLL);
        }
        let first = at.expect("reported") as u64;
        // About two seconds in (eight polls), and exactly once.
        assert!((7..=9).contains(&first), "{first}");
    }

    #[test]
    fn a_stretch_of_sound_pauses_at_most_once_even_if_play_is_pressed_again() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        let mut reports = 0;
        for _ in 0..120 {
            reports += look(&mut watch, &clock, Some("d"), &[MOVIE])
                .iter()
                .filter(|e| **e == PcEvent::OtherAudio)
                .count();
            clock.advance(POLL);
        }
        assert_eq!(reports, 1);
    }

    #[test]
    fn short_pauses_in_the_sound_do_not_start_a_new_stretch_but_a_long_quiet_does() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        let feed = |watch: &mut PcWatch, clock: &mut Clock, audible: bool, polls: usize| {
            let sound = [Sound {
                process: 7,
                audible,
            }];
            let mut reports = 0;
            for _ in 0..polls {
                reports += look(watch, clock, Some("d"), &sound)
                    .iter()
                    .filter(|e| **e == PcEvent::OtherAudio)
                    .count();
                clock.advance(POLL);
            }
            reports
        };
        let mut reports = feed(&mut watch, &mut clock, true, 12); // reported at ~2 s
        reports += feed(&mut watch, &mut clock, false, 8); // 2 s of quiet: same stretch
        reports += feed(&mut watch, &mut clock, true, 12); // still the same stretch
        assert_eq!(reports, 1);
        reports += feed(&mut watch, &mut clock, false, 20); // 5 s of quiet: over
        reports += feed(&mut watch, &mut clock, true, 12); // a new stretch
        assert_eq!(reports, 2);
    }

    #[test]
    fn two_apps_are_tracked_apart() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        let a = Sound {
            process: 1,
            audible: true,
        };
        let b = Sound {
            process: 2,
            audible: true,
        };
        let mut reports = 0;
        for i in 0..40 {
            // B joins late.
            let sounds: &[Sound] = if i < 16 { &[a] } else { &[a, b] };
            reports += look(&mut watch, &clock, Some("d"), sounds)
                .iter()
                .filter(|e| **e == PcEvent::OtherAudio)
                .count();
            clock.advance(POLL);
        }
        assert_eq!(reports, 2);
    }

    #[test]
    fn inaudible_sessions_never_count() {
        let mut watch = PcWatch::new();
        let mut clock = Clock::new();
        let quiet = Sound {
            process: 9,
            audible: false,
        };
        for _ in 0..100 {
            assert!(look(&mut watch, &clock, Some("d"), &[quiet]).is_empty());
            clock.advance(POLL);
        }
    }
}
