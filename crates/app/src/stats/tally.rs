//! Decides when a play counts for the local stats. Pure: it is fed what the player is doing
//! ([`Snapshot`]s, the same ones play reporting gets) and a clock of the caller's, and says what
//! to write down. No I/O.
//!
//! A play counts once the track has really been listened to for [`COUNT_AFTER_MS`] (or half of
//! it, for a short track): skipping through songs counts nothing, and a seek does not make a
//! play out of nothing (time is counted while audio flows, not by position). A track that is
//! left, or paused, has its listening time settled; repeat-one and replays count again.

use crate::reporting::{Activity, Snapshot};

/// Listening time after which a play counts.
pub const COUNT_AFTER_MS: u64 = 30_000;
/// A gap between two observations longer than this is not listening (the PC slept, the app
/// stalled): only this much of it is counted.
const MAX_STEP_MS: u64 = 5_000;
/// A track within this of its end whose position is back under it has restarted.
const RESTART_WINDOW_MS: u64 = 5_000;

/// What to write down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TallyEvent {
    /// The play of queue entry `entry` counts now. `key` names the play, for [`Settled`].
    ///
    /// [`Settled`]: TallyEvent::Settled
    Counted {
        entry: u64,
        key: u64,
        listened_ms: u64,
    },
    /// The play `key` has been listened to for `listened_ms` in all, so far.
    Settled { key: u64, listened_ms: u64 },
}

struct Cur {
    entry: u64,
    key: u64,
    listened_ms: u64,
    /// The last observation while playing: when and where. `None` while paused or stalled, so the
    /// gap is not counted.
    last: Option<(u64, u64)>,
    last_pos: u64,
    duration_ms: Option<u64>,
    counted: bool,
    /// `listened_ms` as last handed out, so a settle says something new.
    settled_ms: u64,
}

pub struct PlayTally {
    cur: Option<Cur>,
    next_key: u64,
    /// Listening time after which a play counts (a short track needs half its length).
    count_after_ms: u64,
}

impl Default for PlayTally {
    fn default() -> Self {
        Self::with_count_after(COUNT_AFTER_MS)
    }
}

impl PlayTally {
    pub fn new() -> Self {
        Self::default()
    }

    /// A tally where a play counts after `count_after_ms` of listening instead of
    /// [`COUNT_AFTER_MS`] (tests and diagnostics).
    pub fn with_count_after(count_after_ms: u64) -> Self {
        Self {
            cur: None,
            next_key: 0,
            count_after_ms,
        }
    }

    /// Listening time after which `cur`'s play counts: half a short track, else the tally's.
    fn needed_ms(&self, cur: &Cur) -> u64 {
        let floor = self.count_after_ms.min(1_000);
        match cur.duration_ms {
            Some(d) if d > 0 => (d / 2).clamp(floor, self.count_after_ms.max(floor)),
            _ => self.count_after_ms,
        }
    }

    /// Forgets the track in progress without writing anything (the account changed).
    pub fn reset(&mut self) {
        self.cur = None;
    }

    /// The player is now `snapshot` (`None`: nothing is loaded); `now` is milliseconds on any
    /// monotonic clock.
    pub fn observe(&mut self, now: u64, snapshot: Option<&Snapshot>) -> Vec<TallyEvent> {
        let mut out = Vec::new();
        let leaving = match (&self.cur, snapshot) {
            (Some(c), Some(s)) => {
                c.entry != s.entry
                    || c.duration_ms.is_some_and(|d| {
                        c.last_pos + RESTART_WINDOW_MS >= d && s.position_ms < RESTART_WINDOW_MS
                    })
            }
            (Some(_), None) => true,
            _ => false,
        };
        if leaving {
            if let Some(c) = self.cur.take() {
                if c.counted && c.listened_ms != c.settled_ms {
                    out.push(TallyEvent::Settled {
                        key: c.key,
                        listened_ms: c.listened_ms,
                    });
                }
            }
        }
        let Some(s) = snapshot else { return out };

        if self.cur.is_none() {
            // Nothing counts until audio is really flowing.
            if s.activity != Activity::Playing || s.position_ms == 0 {
                return out;
            }
            self.next_key += 1;
            self.cur = Some(Cur {
                entry: s.entry,
                key: self.next_key,
                listened_ms: 0,
                last: None,
                last_pos: s.position_ms,
                duration_ms: s.duration_ms,
                counted: false,
                settled_ms: 0,
            });
        }
        let Some(mut c) = self.cur.take() else {
            return out;
        };
        let needed = self.needed_ms(&c);
        c.duration_ms = s.duration_ms.or(c.duration_ms);
        c.last_pos = s.position_ms;
        match s.activity {
            Activity::Playing => {
                if let Some((then, _)) = c.last {
                    c.listened_ms += now.saturating_sub(then).min(MAX_STEP_MS);
                }
                c.last = Some((now, s.position_ms));
                if !c.counted && c.listened_ms >= needed {
                    c.counted = true;
                    c.settled_ms = c.listened_ms;
                    out.push(TallyEvent::Counted {
                        entry: c.entry,
                        key: c.key,
                        listened_ms: c.listened_ms,
                    });
                }
            }
            Activity::Paused => {
                // A pause settles the time, so quitting while paused loses nothing.
                if c.last.take().is_some() && c.counted && c.listened_ms != c.settled_ms {
                    c.settled_ms = c.listened_ms;
                    out.push(TallyEvent::Settled {
                        key: c.key,
                        listened_ms: c.listened_ms,
                    });
                }
            }
            Activity::Stalled => c.last = None,
        }
        self.cur = Some(c);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(entry: u64, activity: Activity, position_ms: u64, duration_ms: u64) -> Snapshot {
        Snapshot {
            entry,
            video_id: format!("v{entry}"),
            activity,
            position_ms,
            duration_ms: Some(duration_ms),
            volume: 80,
            muted: false,
        }
    }

    /// Plays `entry` for `secs` seconds from `from_ms`, observing once a second from `t0`.
    fn play(
        t: &mut PlayTally,
        entry: u64,
        t0: u64,
        from_ms: u64,
        secs: u64,
        dur: u64,
    ) -> Vec<TallyEvent> {
        let mut out = Vec::new();
        for i in 0..=secs {
            out.extend(t.observe(
                t0 + i * 1000,
                Some(&snap(entry, Activity::Playing, from_ms + i * 1000, dur)),
            ));
        }
        out
    }

    #[test]
    fn a_play_counts_after_thirty_seconds_of_listening_and_only_once() {
        let mut t = PlayTally::new();
        let events = play(&mut t, 1, 0, 1000, 29, 200_000);
        assert!(events.is_empty(), "29 s is not yet a play");
        let events = play(&mut t, 1, 29_000, 30_000, 10, 200_000);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            TallyEvent::Counted { entry: 1, listened_ms, .. } if listened_ms >= 30_000
        ));
    }

    #[test]
    fn a_skipped_song_counts_nothing() {
        let mut t = PlayTally::new();
        let mut events = play(&mut t, 1, 0, 1000, 5, 200_000);
        events.extend(t.observe(6000, Some(&snap(2, Activity::Playing, 1000, 200_000))));
        assert!(events.is_empty());
    }

    #[test]
    fn a_short_track_counts_at_half_its_length() {
        let mut t = PlayTally::new();
        let events = play(&mut t, 1, 0, 1000, 12, 20_000);
        assert_eq!(events.len(), 1, "10 s of a 20 s track");
    }

    #[test]
    fn seeking_does_not_make_a_play() {
        let mut t = PlayTally::new();
        let mut events = Vec::new();
        // Jump through the track every second: lots of position, little listening.
        for i in 0..20u64 {
            events.extend(t.observe(
                i * 1000,
                Some(&snap(1, Activity::Playing, 1000 + i * 30_000, 900_000)),
            ));
        }
        assert!(events.is_empty());
    }

    #[test]
    fn time_paused_or_stalled_is_not_listening() {
        let mut t = PlayTally::new();
        let mut events = play(&mut t, 1, 0, 1000, 20, 200_000);
        for i in 0..60u64 {
            events.extend(t.observe(
                21_000 + i * 1000,
                Some(&snap(1, Activity::Paused, 21_000, 200_000)),
            ));
        }
        assert!(events.is_empty(), "a minute paused adds nothing");
        events.extend(play(&mut t, 1, 90_000, 21_000, 15, 200_000));
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, TallyEvent::Counted { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn leaving_the_track_settles_its_listening_time() {
        let mut t = PlayTally::new();
        let mut events = play(&mut t, 1, 0, 1000, 60, 200_000);
        events.extend(t.observe(61_000, Some(&snap(2, Activity::Playing, 1000, 200_000))));
        let settled = events
            .iter()
            .find_map(|e| match e {
                TallyEvent::Settled { listened_ms, .. } => Some(*listened_ms),
                _ => None,
            })
            .expect("settled");
        assert!((59_000..=61_000).contains(&settled), "{settled}");
    }

    #[test]
    fn a_pause_settles_and_resuming_carries_on_the_same_play() {
        let mut t = PlayTally::new();
        let mut events = play(&mut t, 1, 0, 1000, 40, 200_000);
        events.extend(t.observe(41_000, Some(&snap(1, Activity::Paused, 41_000, 200_000))));
        assert!(matches!(events.last(), Some(TallyEvent::Settled { .. })));
        let key = match &events[0] {
            TallyEvent::Counted { key, .. } => *key,
            other => panic!("{other:?}"),
        };
        let later = play(&mut t, 1, 100_000, 41_000, 5, 200_000);
        assert!(later.is_empty(), "resuming is not a new play");
        let out = t.observe(106_500, None);
        assert!(matches!(out[..], [TallyEvent::Settled { key: k, .. }] if k == key));
    }

    #[test]
    fn repeat_one_counts_each_pass() {
        let mut t = PlayTally::new();
        let mut events = play(&mut t, 1, 0, 1000, 49, 50_000);
        // It restarts from the top (the same queue entry).
        events.extend(play(&mut t, 1, 50_000, 1000, 40, 50_000));
        let counted = events
            .iter()
            .filter(|e| matches!(e, TallyEvent::Counted { .. }))
            .count();
        assert_eq!(counted, 2);
    }

    #[test]
    fn a_long_gap_between_observations_is_not_listening() {
        let mut t = PlayTally::new();
        t.observe(0, Some(&snap(1, Activity::Playing, 1000, 200_000)));
        // The PC slept for ten minutes between two observations.
        let events = t.observe(600_000, Some(&snap(1, Activity::Playing, 2000, 200_000)));
        assert!(events.is_empty());
    }

    #[test]
    fn nothing_counts_before_audio_flows() {
        let mut t = PlayTally::new();
        let mut events = Vec::new();
        for i in 0..60u64 {
            events.extend(t.observe(i * 1000, Some(&snap(1, Activity::Playing, 0, 200_000))));
        }
        assert!(events.is_empty());
    }
}
