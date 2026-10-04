//! Play reporting, app side: tell YouTube Music what was played, the way its web client does, so
//! Apollo listening shows in History and keeps shaping Home and mixes.
//!
//! Three layers, from pure to impure:
//!
//! * [`PlaySession`] decides *when* to report. It is fed what the player is doing
//!   ([`Snapshot`]s) and returns [`Report`]s: one `Begin` when audio is really playing, then
//!   watch-time pings on the web client's schedule, one when playback pauses, and a final one
//!   when the track is left. No I/O, no clock of its own.
//! * [`Engine`] turns reports into pings through a [`Pinger`], and holds the rules that keep
//!   reporting harmless: pings go to the account that was signed in when the play began and no
//!   other, a play that was skipped before its tracking arrived is never reported, a failure is
//!   logged and never retried, and a run of failures makes it stand down for a while.
//! * [`ThreadReporter`] runs an engine on its own thread behind [`PlayReporter`], so the player
//!   only ever hands over a snapshot. Nothing here is on the path of playback.
//!
//! Nothing here logs a cookie, a tracking URL or a video id: a failure is "play report: <step>
//! failed".

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How far the media position may drift from the wall clock between two observations and still
/// count as playing straight through (anything more is a seek).
const SEEK_TOLERANCE_MS: u64 = 1_500;
/// Played stretches this close together are one stretch.
const MERGE_GAP_MS: u64 = 1_000;
/// A track counts as restarted (repeat one, replay) when it was within this of its end and the
/// position is back under it.
const RESTART_WINDOW_MS: u64 = 5_000;
/// Consecutive failed pings before reporting stands down.
const BREAKER_FAILURES: u32 = 5;
/// How long it stands down.
const BREAKER_PAUSE_MS: u64 = 10 * 60 * 1000;
/// The player's snapshots are handed over at most this often while nothing changes.
const FEED_EVERY: Duration = Duration::from_secs(1);

/// What the player is doing with the current track.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Audio is flowing.
    Playing,
    /// Paused (or nothing asked to play).
    Paused,
    /// Asked to play but not flowing: resolving, opening, buffering, seeking, failed.
    Stalled,
}

/// The player at one moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The queue entry (not the video: the same song twice in a row is two plays).
    pub entry: u64,
    pub video_id: String,
    pub activity: Activity,
    pub position_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume: u8,
    pub muted: bool,
}

/// Whether a watch-time ping was taken while playing or paused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchState {
    Playing,
    Paused,
}

/// One watch-time ping: what was played since the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchReport {
    pub position_ms: u64,
    pub ranges: Vec<(u64, u64)>,
    pub elapsed_ms: u64,
    pub duration_ms: Option<u64>,
    pub state: WatchState,
    pub is_final: bool,
    pub volume: u8,
    pub muted: bool,
}

/// What to tell YouTube Music.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// Playback of `video_id` has really started.
    Begin {
        entry: u64,
        video_id: String,
        position_ms: u64,
    },
    Watch {
        entry: u64,
        ping: WatchReport,
    },
}

/// When the watch-time pings go out: wall-clock seconds since the playback began for the first
/// few, then every `interval_secs`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub first_secs: Vec<u32>,
    pub interval_secs: u32,
}

impl Default for Schedule {
    /// The web client's: 10, 20 and 30 seconds in, then every 40.
    fn default() -> Self {
        Self {
            first_secs: vec![10, 20, 30],
            interval_secs: 40,
        }
    }
}

impl Schedule {
    /// Milliseconds since the playback began at which ping number `index` (0-based) is due.
    pub fn due_ms(&self, index: usize) -> u64 {
        let secs = match self.first_secs.get(index) {
            Some(s) => u64::from(*s),
            None => {
                let last = self.first_secs.last().copied().map_or(0, u64::from);
                let extra = (index + 1 - self.first_secs.len()) as u64;
                last + extra * u64::from(self.interval_secs)
            }
        };
        secs * 1000
    }
}

struct Cur {
    entry: u64,
    began_at: u64,
    /// Stretches played since the last ping, oldest first. While `cursor` is set the last one is
    /// still open (its end follows the position).
    ranges: Vec<(u64, u64)>,
    /// The last observation while playing: position and when. `None` while paused or stalled, so
    /// the gap is not counted as play time.
    cursor: Option<(u64, u64)>,
    last_pos: u64,
    flushes: usize,
    paused: bool,
    schedule: Schedule,
    duration_ms: Option<u64>,
    volume: u8,
    muted: bool,
}

impl Cur {
    fn extend(&mut self, now: u64, pos: u64) {
        match self.cursor {
            Some((p0, t0)) => {
                let wall = now.saturating_sub(t0);
                let moved = pos.abs_diff(p0);
                let straight = pos >= p0 && moved.abs_diff(wall) <= SEEK_TOLERANCE_MS;
                match self.ranges.last_mut() {
                    Some(r) if straight => r.1 = r.1.max(pos),
                    _ => self.ranges.push((pos, pos)),
                }
            }
            None => match self.ranges.last_mut() {
                // Resuming where it stopped: carry on the same stretch.
                Some(r) if pos.abs_diff(r.1) <= MERGE_GAP_MS => r.1 = r.1.max(pos),
                _ => self.ranges.push((pos, pos)),
            },
        }
        self.cursor = Some((pos, now));
        self.last_pos = pos;
    }

    /// The ping for what was played since the last one. `None` when nothing was and it is not
    /// the final one.
    fn flush(&mut self, now: u64, state: WatchState, is_final: bool) -> Option<Report> {
        let mut ranges: Vec<(u64, u64)> = std::mem::take(&mut self.ranges)
            .into_iter()
            .filter(|(s, e)| e > s)
            .collect();
        if ranges.is_empty() {
            if !is_final {
                if self.cursor.is_some() {
                    self.ranges.push((self.last_pos, self.last_pos));
                }
                return None;
            }
            // The last ping always goes, even with nothing new to say, so the playback is closed.
            ranges.push((self.last_pos, self.last_pos));
        }
        // The next ping starts where this one ended.
        if self.cursor.is_some() {
            self.ranges.push((self.last_pos, self.last_pos));
        }
        Some(Report::Watch {
            entry: self.entry,
            ping: WatchReport {
                position_ms: self.last_pos,
                ranges,
                elapsed_ms: now.saturating_sub(self.began_at),
                duration_ms: self.duration_ms,
                state,
                is_final,
                volume: self.volume,
                muted: self.muted,
            },
        })
    }
}

/// Decides what to report from what the player is doing. Time is the caller's: `now` is
/// milliseconds on any monotonic clock.
#[derive(Default)]
pub struct PlaySession {
    cur: Option<Cur>,
}

impl PlaySession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget the current play without reporting anything (reporting was turned off, or the
    /// account changed).
    pub fn reset(&mut self) {
        self.cur = None;
    }

    /// Use the schedule the response named for `entry`'s watch-time pings.
    pub fn set_schedule(&mut self, entry: u64, schedule: Schedule) {
        if let Some(c) = self.cur.as_mut().filter(|c| c.entry == entry) {
            c.schedule = schedule;
        }
    }

    /// The player is now `snapshot` (`None`: nothing is loaded). Returns what to report, in order.
    pub fn observe(&mut self, now: u64, snapshot: Option<&Snapshot>) -> Vec<Report> {
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
            if let Some(mut c) = self.cur.take() {
                let state = if c.paused {
                    WatchState::Paused
                } else {
                    WatchState::Playing
                };
                out.extend(c.flush(now, state, true));
            }
        }
        let Some(s) = snapshot else { return out };

        if self.cur.is_none() {
            // Nothing is reported until audio is really flowing.
            if s.activity != Activity::Playing || s.position_ms == 0 {
                return out;
            }
            self.cur = Some(Cur {
                entry: s.entry,
                began_at: now,
                ranges: Vec::new(),
                cursor: None,
                last_pos: s.position_ms,
                flushes: 0,
                paused: false,
                schedule: Schedule::default(),
                duration_ms: s.duration_ms,
                volume: s.volume,
                muted: s.muted,
            });
            out.push(Report::Begin {
                entry: s.entry,
                video_id: s.video_id.clone(),
                position_ms: s.position_ms,
            });
        }
        let Some(c) = self.cur.as_mut() else {
            return out;
        };
        c.duration_ms = s.duration_ms.or(c.duration_ms);
        c.volume = s.volume;
        c.muted = s.muted;
        match s.activity {
            Activity::Playing => {
                c.paused = false;
                c.extend(now, s.position_ms);
                let elapsed = now.saturating_sub(c.began_at);
                if elapsed >= c.schedule.due_ms(c.flushes) {
                    // After a long pause every missed time is behind us: one ping, not a burst.
                    while elapsed >= c.schedule.due_ms(c.flushes) {
                        c.flushes += 1;
                    }
                    out.extend(c.flush(now, WatchState::Playing, false));
                }
            }
            Activity::Paused => {
                if !c.paused {
                    c.paused = true;
                    c.cursor = None;
                    out.extend(c.flush(now, WatchState::Paused, false));
                }
            }
            Activity::Stalled => c.cursor = None,
        }
        out
    }
}

/// Sends the pings. Blocking; called on the reporter's thread only.
pub trait Pinger: Send + 'static {
    /// One playback's tracking (URLs, nonce).
    type Playback: Send;

    /// Who a ping would be attributed to now, as an opaque key; `None` when nobody is signed in.
    fn account(&mut self) -> Option<String>;
    /// Where to report `video_id`. `Ok(None)`: the service gave nothing to report to.
    fn tracking(&mut self, video_id: &str) -> Result<Option<Self::Playback>, PingFailed>;
    /// The watch-time schedule the response named, if it named one.
    fn schedule(&mut self, playback: &Self::Playback) -> Schedule;
    /// The "playback started" ping.
    fn start(&mut self, playback: &Self::Playback, position_ms: u64) -> Result<(), PingFailed>;
    /// A watch-time ping.
    fn watch(&mut self, playback: &Self::Playback, ping: &WatchReport) -> Result<(), PingFailed>;
}

/// A lookup or ping that did not go through. Carries nothing: the app logs the step, not why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PingFailed;

/// What the reporter's thread and the player share: which "generation" of reporting is current
/// (bumped by every cancel), and which queue entry is current right now.
pub struct Shared {
    generation: AtomicU64,
    entry: AtomicU64,
}

const NO_ENTRY: u64 = u64::MAX;

impl Shared {
    pub fn new() -> Self {
        Self {
            generation: AtomicU64::new(0),
            entry: AtomicU64::new(NO_ENTRY),
        }
    }

    fn observed(&self, snapshot: Option<&Snapshot>) {
        self.entry
            .store(snapshot.map_or(NO_ENTRY, |s| s.entry), Ordering::SeqCst);
    }

    fn cancel(&self) -> u64 {
        self.entry.store(NO_ENTRY, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

/// The checks made right before a ping goes out.
struct Gate<'a> {
    shared: &'a Shared,
    generation: u64,
}

impl Gate<'_> {
    /// Reporting has not been cancelled since this work was handed over.
    fn open(&self) -> bool {
        self.shared.generation() == self.generation
    }

    /// `entry` is still what the player is on.
    fn current(&self, entry: u64) -> bool {
        self.shared.entry.load(Ordering::SeqCst) == entry
    }
}

/// Stands down after a run of failures, so an outage or a refusal costs a few requests, not one
/// per track for the rest of the session.
#[derive(Default)]
struct Breaker {
    failures: u32,
    until: Option<u64>,
}

impl Breaker {
    fn standing_down(&mut self, now: u64) -> bool {
        match self.until {
            Some(t) if now < t => true,
            Some(_) => {
                self.until = None;
                self.failures = 0;
                false
            }
            None => false,
        }
    }

    fn ok(&mut self) {
        self.failures = 0;
    }

    fn failed(&mut self, now: u64) {
        self.failures += 1;
        if self.failures >= BREAKER_FAILURES {
            self.until = Some(now + BREAKER_PAUSE_MS);
        }
    }
}

struct Active<P> {
    entry: u64,
    playback: P,
    account: String,
}

/// A [`PlaySession`] and the [`Pinger`] its reports go to.
pub struct Engine<P: Pinger> {
    pinger: P,
    session: PlaySession,
    active: Option<Active<P::Playback>>,
    breaker: Breaker,
}

impl<P: Pinger> Engine<P> {
    pub fn new(pinger: P) -> Self {
        Self {
            pinger,
            session: PlaySession::new(),
            active: None,
            breaker: Breaker::default(),
        }
    }

    fn cancel(&mut self) {
        self.session.reset();
        self.active = None;
    }

    fn observe(&mut self, now: u64, snapshot: Option<Snapshot>, gate: &Gate<'_>) {
        for report in self.session.observe(now, snapshot.as_ref()) {
            if !gate.open() {
                return;
            }
            self.apply(now, report, gate);
        }
    }

    fn apply(&mut self, now: u64, report: Report, gate: &Gate<'_>) {
        match report {
            Report::Begin {
                entry,
                video_id,
                position_ms,
            } => {
                self.active = None;
                if self.breaker.standing_down(now) {
                    return;
                }
                let Some(account) = self.pinger.account() else {
                    return;
                };
                let playback = match self.pinger.tracking(&video_id) {
                    Ok(Some(p)) => p,
                    Ok(None) => return,
                    Err(PingFailed) => {
                        eprintln!("play report: tracking lookup failed");
                        self.breaker.failed(now);
                        return;
                    }
                };
                // The lookup takes a moment. A track skipped (or an account left, or reporting
                // turned off) in the meantime was not really played: it is not reported.
                if !gate.open()
                    || !gate.current(entry)
                    || self.pinger.account().as_deref() != Some(account.as_str())
                {
                    return;
                }
                let schedule = self.pinger.schedule(&playback);
                self.session.set_schedule(entry, schedule);
                match self.pinger.start(&playback, position_ms) {
                    Ok(()) => {
                        self.breaker.ok();
                        self.active = Some(Active {
                            entry,
                            playback,
                            account,
                        });
                    }
                    Err(PingFailed) => {
                        eprintln!("play report: start ping failed");
                        self.breaker.failed(now);
                    }
                }
            }
            Report::Watch { entry, ping } => {
                let Some(active) = self.active.as_ref().filter(|a| a.entry == entry) else {
                    return;
                };
                if self.breaker.standing_down(now) {
                    return;
                }
                if self.pinger.account().as_deref() != Some(active.account.as_str()) {
                    // The account changed under this play: nothing more goes out for it.
                    self.active = None;
                    return;
                }
                match self.pinger.watch(&active.playback, &ping) {
                    Ok(()) => self.breaker.ok(),
                    Err(PingFailed) => {
                        eprintln!("play report: watch-time ping failed");
                        self.breaker.failed(now);
                    }
                }
                if ping.is_final {
                    self.active = None;
                }
            }
        }
    }
}

/// What the player talks to. Both calls return at once.
pub trait PlayReporter: Send + Sync {
    /// The player is now `snapshot` (`None`: nothing loaded).
    fn observe(&self, snapshot: Option<Snapshot>);
    /// Stop: drop what is queued and send nothing more for the play in progress (reporting was
    /// turned off, or the account changed).
    fn cancel(&self);
}

enum Msg {
    Observe {
        at_ms: u64,
        generation: u64,
        snapshot: Option<Snapshot>,
    },
    Cancel,
}

/// An [`Engine`] on its own thread. Dropping it ends the thread.
pub struct ThreadReporter {
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    epoch: Instant,
}

impl ThreadReporter {
    /// `None` when the thread cannot start: nothing is reported.
    pub fn spawn<P: Pinger>(pinger: P) -> Option<Self> {
        let (tx, rx) = channel::<Msg>();
        let shared = Arc::new(Shared::new());
        let worker_shared = shared.clone();
        std::thread::Builder::new()
            .name("apollo-play-report".into())
            .spawn(move || {
                let mut engine = Engine::new(pinger);
                for msg in rx {
                    match msg {
                        Msg::Observe {
                            at_ms,
                            generation,
                            snapshot,
                        } => {
                            let gate = Gate {
                                shared: &worker_shared,
                                generation,
                            };
                            if gate.open() {
                                engine.observe(at_ms, snapshot, &gate);
                            }
                        }
                        Msg::Cancel => engine.cancel(),
                    }
                }
            })
            .ok()?;
        Some(Self {
            tx,
            shared,
            epoch: Instant::now(),
        })
    }
}

impl PlayReporter for ThreadReporter {
    fn observe(&self, snapshot: Option<Snapshot>) {
        self.shared.observed(snapshot.as_ref());
        let _ = self.tx.send(Msg::Observe {
            at_ms: self.epoch.elapsed().as_millis() as u64,
            generation: self.shared.generation(),
            snapshot,
        });
    }

    fn cancel(&self) {
        self.shared.cancel();
        let _ = self.tx.send(Msg::Cancel);
    }
}

/// Spaces the player's snapshots out: one when the track, the activity or whether audio has
/// started changed, otherwise at most one a second.
#[derive(Default)]
pub struct Feed {
    last: Option<(Option<(u64, Activity, bool)>, Instant)>,
}

impl Feed {
    /// Whether `snapshot` should be handed over now.
    pub fn due(&mut self, snapshot: Option<&Snapshot>, now: Instant) -> bool {
        // Audio arriving (the position leaving zero) is a change: it is what starts a report.
        let key = snapshot.map(|s| (s.entry, s.activity, s.position_ms > 0));
        let due = match self.last {
            Some((last_key, at)) => last_key != key || now.duration_since(at) >= FEED_EVERY,
            None => true,
        };
        if due {
            self.last = Some((key, now));
        }
        due
    }

    /// Forget what was last handed over (reporting was turned off).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn snap(entry: u64, activity: Activity, position_ms: u64) -> Snapshot {
        Snapshot {
            entry,
            video_id: format!("video{entry}"),
            activity,
            position_ms,
            duration_ms: Some(200_000),
            volume: 80,
            muted: false,
        }
    }

    fn playing(entry: u64, position_ms: u64) -> Snapshot {
        snap(entry, Activity::Playing, position_ms)
    }

    /// Plays `entry` straight through from `from_ms` to `to_ms`, observing once a second;
    /// returns everything reported.
    fn play(
        session: &mut PlaySession,
        entry: u64,
        t0: u64,
        from_ms: u64,
        to_ms: u64,
    ) -> Vec<Report> {
        let mut out = Vec::new();
        let mut pos = from_ms;
        let mut t = t0;
        while pos <= to_ms {
            out.extend(session.observe(t, Some(&playing(entry, pos))));
            pos += 1000;
            t += 1000;
        }
        out
    }

    fn watches(reports: &[Report]) -> Vec<&WatchReport> {
        reports
            .iter()
            .filter_map(|r| match r {
                Report::Watch { ping, .. } => Some(ping),
                Report::Begin { .. } => None,
            })
            .collect()
    }

    // ---- the session -------------------------------------------------------------------

    #[test]
    fn nothing_is_reported_until_audio_is_really_playing() {
        let mut s = PlaySession::new();
        assert!(s.observe(0, None).is_empty());
        assert!(s.observe(10, Some(&snap(1, Activity::Stalled, 0))).is_empty());
        assert!(s.observe(20, Some(&snap(1, Activity::Paused, 5_000))).is_empty());
        assert!(
            s.observe(30, Some(&playing(1, 0))).is_empty(),
            "playing but no audio position yet"
        );
        let reports = s.observe(40, Some(&playing(1, 250)));
        assert_eq!(
            reports,
            [Report::Begin {
                entry: 1,
                video_id: "video1".into(),
                position_ms: 250
            }]
        );
    }

    #[test]
    fn the_start_is_reported_once_per_play() {
        let mut s = PlaySession::new();
        let reports = play(&mut s, 1, 0, 500, 9_500);
        let begins = reports
            .iter()
            .filter(|r| matches!(r, Report::Begin { .. }))
            .count();
        assert_eq!(begins, 1);
        assert!(watches(&reports).is_empty(), "no watch-time ping before 10 s");
    }

    #[test]
    fn watch_time_goes_out_at_10_20_30_then_every_40_seconds() {
        let mut s = PlaySession::new();
        let reports = play(&mut s, 1, 0, 500, 160_000);
        let pings = watches(&reports);
        let elapsed: Vec<u64> = pings.iter().map(|p| p.elapsed_ms).collect();
        assert_eq!(elapsed, [10_000, 20_000, 30_000, 70_000, 110_000, 150_000]);
        assert!(pings.iter().all(|p| p.state == WatchState::Playing && !p.is_final));
    }

    #[test]
    fn each_ping_covers_only_what_was_played_since_the_last() {
        let mut s = PlaySession::new();
        let reports = play(&mut s, 1, 0, 500, 31_000);
        let pings = watches(&reports);
        assert_eq!(pings[0].ranges, [(500, 10_500)]);
        assert_eq!(pings[0].position_ms, 10_500);
        assert_eq!(pings[1].ranges, [(10_500, 20_500)]);
        assert_eq!(pings[2].ranges, [(20_500, 30_500)]);
        assert_eq!(pings[0].duration_ms, Some(200_000));
        assert_eq!((pings[0].volume, pings[0].muted), (80, false));
    }

    #[test]
    fn a_seek_splits_the_played_ranges() {
        let mut s = PlaySession::new();
        let mut reports = play(&mut s, 1, 0, 500, 5_500); // t = 0..5000
        // Jump forward 60 s at t = 6000, then keep playing.
        let mut t = 6_000;
        let mut pos = 65_500;
        while t <= 10_000 {
            reports.extend(s.observe(t, Some(&playing(1, pos))));
            t += 1000;
            pos += 1000;
        }
        let ping = &watches(&reports)[0];
        assert_eq!(ping.ranges, [(500, 5_500), (65_500, 69_500)]);
    }

    #[test]
    fn pausing_sends_what_was_played_and_resuming_carries_on_without_a_new_start() {
        let mut s = PlaySession::new();
        let mut reports = play(&mut s, 1, 0, 500, 3_500); // t = 0..3000
        reports.clear();
        let paused = s.observe(4_000, Some(&snap(1, Activity::Paused, 3_500)));
        let ping = &watches(&paused)[0];
        assert_eq!(ping.state, WatchState::Paused);
        assert_eq!(ping.ranges, [(500, 3_500)]);
        assert!(!ping.is_final);
        // More paused snapshots say nothing.
        assert!(s.observe(5_000, Some(&snap(1, Activity::Paused, 3_500))).is_empty());
        // Resume where it stopped, a minute later.
        let resumed = s.observe(65_000, Some(&playing(1, 3_700)));
        assert!(resumed.is_empty(), "no second Begin: {resumed:?}");
        let more = s.observe(66_000, Some(&playing(1, 4_700)));
        assert!(more.is_empty());
        // The next scheduled ping measures from the start of the play, pause included.
        let mut later = Vec::new();
        let mut t = 67_000;
        let mut pos = 5_700;
        while later.is_empty() && t < 100_000 {
            later = s.observe(t, Some(&playing(1, pos)));
            t += 1000;
            pos += 1000;
        }
        let ping = &watches(&later)[0];
        assert_eq!(ping.elapsed_ms, 70_000, "the 70 s ping, not a burst of catch-up pings");
        assert_eq!(ping.state, WatchState::Playing);
        assert_eq!(ping.ranges.first().map(|r| r.0), Some(3_700));
    }

    #[test]
    fn buffering_is_not_play_time() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 2_500);
        assert!(s.observe(3_000, Some(&snap(1, Activity::Stalled, 2_500))).is_empty());
        assert!(s.observe(9_000, Some(&snap(1, Activity::Stalled, 2_500))).is_empty());
        let reports = s.observe(10_000, Some(&playing(1, 2_600)));
        let ping = &watches(&reports)[0];
        assert_eq!(
            ping.ranges,
            [(500, 2_600)],
            "the stall is bridged, not counted: {ping:?}"
        );
    }

    #[test]
    fn leaving_a_track_sends_a_final_ping_and_the_next_begins() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 14_500);
        let reports = s.observe(15_000, Some(&playing(2, 300)));
        assert_eq!(reports.len(), 2, "{reports:?}");
        let Report::Watch { entry: 1, ping } = &reports[0] else {
            panic!("final ping for the old track first: {reports:?}");
        };
        assert!(ping.is_final);
        assert_eq!(ping.ranges, [(10_500, 14_500)]);
        assert!(matches!(&reports[1], Report::Begin { entry: 2, .. }));
    }

    #[test]
    fn the_same_song_queued_twice_is_two_plays() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 3_500);
        let mut next = playing(2, 300);
        next.video_id = "video1".into();
        let reports = s.observe(4_000, Some(&next));
        assert!(matches!(&reports[1], Report::Begin { entry: 2, .. }), "{reports:?}");
    }

    #[test]
    fn a_stopped_player_ends_the_play() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 3_500);
        let reports = s.observe(4_000, None);
        assert!(watches(&reports)[0].is_final);
        assert!(s.observe(5_000, None).is_empty());
        // And a leave that never reached audio says nothing.
        assert!(s.observe(6_000, Some(&snap(2, Activity::Stalled, 0))).is_empty());
        assert!(s.observe(7_000, None).is_empty());
    }

    #[test]
    fn a_final_ping_goes_even_when_nothing_new_was_played() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 10_500); // the 10 s ping just went
        let reports = s.observe(10_900, None);
        let ping = &watches(&reports)[0];
        assert!(ping.is_final);
        assert_eq!(ping.ranges.len(), 1);
    }

    #[test]
    fn a_paused_then_left_track_ends_paused() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 3_500);
        let _ = s.observe(4_000, Some(&snap(1, Activity::Paused, 3_500)));
        let reports = s.observe(60_000, None);
        let ping = &watches(&reports)[0];
        assert!(ping.is_final);
        assert_eq!(ping.state, WatchState::Paused);
    }

    #[test]
    fn replaying_a_finished_track_is_a_new_play() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 195_000, 199_000);
        let reports = s.observe(200_000, Some(&playing(1, 300)));
        assert!(watches(&reports)[0].is_final, "{reports:?}");
        assert!(matches!(reports.last(), Some(Report::Begin { entry: 1, .. })));
    }

    #[test]
    fn seeking_back_to_the_start_mid_track_is_not_a_replay() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 100_000, 103_000);
        let reports = s.observe(104_000, Some(&playing(1, 500)));
        assert!(reports.iter().all(|r| !matches!(r, Report::Begin { .. })), "{reports:?}");
    }

    #[test]
    fn the_response_can_set_the_schedule() {
        let mut s = PlaySession::new();
        let _ = s.observe(0, Some(&playing(1, 500)));
        s.set_schedule(
            1,
            Schedule {
                first_secs: vec![2],
                interval_secs: 3,
            },
        );
        let mut elapsed = Vec::new();
        for i in 1..=9u64 {
            let reports = s.observe(i * 1000, Some(&playing(1, 500 + i * 1000)));
            elapsed.extend(watches(&reports).iter().map(|p| p.elapsed_ms));
        }
        assert_eq!(elapsed, [2_000, 5_000, 8_000]);
        // A schedule for some other track changes nothing.
        s.set_schedule(9, Schedule { first_secs: vec![1], interval_secs: 1 });
        assert!(s.observe(10_000, Some(&playing(1, 10_500))).is_empty());
    }

    #[test]
    fn reset_forgets_the_play_and_reports_nothing() {
        let mut s = PlaySession::new();
        let _ = play(&mut s, 1, 0, 500, 3_500);
        s.reset();
        assert!(s.observe(4_000, None).is_empty());
        // Playing again afterwards starts a fresh play.
        let reports = s.observe(5_000, Some(&playing(1, 4_500)));
        assert!(matches!(&reports[..], [Report::Begin { .. }]));
    }

    // ---- the feed ----------------------------------------------------------------------

    #[test]
    fn the_feed_passes_changes_at_once_and_a_steady_state_once_a_second() {
        let mut feed = Feed::default();
        let t0 = Instant::now();
        let a = playing(1, 500);
        assert!(feed.due(Some(&playing(1, 0)), t0));
        assert!(feed.due(Some(&a), t0 + Duration::from_millis(50)), "audio arriving");
        assert!(!feed.due(Some(&playing(1, 600)), t0 + Duration::from_millis(200)));
        assert!(feed.due(Some(&playing(1, 1_600)), t0 + Duration::from_millis(1_050)));
        assert!(feed.due(Some(&snap(1, Activity::Paused, 1_600)), t0 + Duration::from_millis(1_100)));
        assert!(feed.due(Some(&playing(2, 100)), t0 + Duration::from_millis(1_200)));
        assert!(feed.due(None, t0 + Duration::from_millis(1_300)));
        feed.reset();
        assert!(feed.due(None, t0 + Duration::from_millis(1_301)));
    }

    // ---- the engine --------------------------------------------------------------------

    #[derive(Default)]
    struct Log {
        lookups: Vec<String>,
        starts: Vec<(String, u64)>,
        watches: Vec<(String, WatchReport)>,
    }

    struct Fake {
        log: Arc<Mutex<Log>>,
        account: Arc<Mutex<Option<String>>>,
        /// `None`: the service has no tracking.
        has_tracking: bool,
        fail_lookup: bool,
        fail_pings: bool,
        next: u32,
        /// Runs during a tracking lookup (to change the world while it is "in flight").
        during_lookup: Option<Box<dyn FnMut() + Send>>,
    }

    impl Fake {
        fn new() -> (Self, Arc<Mutex<Log>>, Arc<Mutex<Option<String>>>) {
            let log = Arc::new(Mutex::new(Log::default()));
            let account = Arc::new(Mutex::new(Some("alice".to_string())));
            (
                Self {
                    log: log.clone(),
                    account: account.clone(),
                    has_tracking: true,
                    fail_lookup: false,
                    fail_pings: false,
                    next: 0,
                    during_lookup: None,
                },
                log,
                account,
            )
        }
    }

    impl Pinger for Fake {
        type Playback = String;

        fn account(&mut self) -> Option<String> {
            self.account.lock().unwrap().clone()
        }

        fn tracking(&mut self, video_id: &str) -> Result<Option<String>, PingFailed> {
            self.log.lock().unwrap().lookups.push(video_id.into());
            if let Some(f) = self.during_lookup.as_mut() {
                f();
            }
            if self.fail_lookup {
                return Err(PingFailed);
            }
            self.next += 1;
            Ok(self.has_tracking.then(|| format!("playback{}", self.next)))
        }

        fn schedule(&mut self, _: &String) -> Schedule {
            Schedule::default()
        }

        fn start(&mut self, playback: &String, position_ms: u64) -> Result<(), PingFailed> {
            if self.fail_pings {
                return Err(PingFailed);
            }
            self.log.lock().unwrap().starts.push((playback.clone(), position_ms));
            Ok(())
        }

        fn watch(&mut self, playback: &String, ping: &WatchReport) -> Result<(), PingFailed> {
            if self.fail_pings {
                return Err(PingFailed);
            }
            self.log.lock().unwrap().watches.push((playback.clone(), ping.clone()));
            Ok(())
        }
    }

    /// Feeds the engine a snapshot the way the reporter thread would.
    fn feed(engine: &mut Engine<Fake>, shared: &Shared, now: u64, snapshot: Option<Snapshot>) {
        shared.observed(snapshot.as_ref());
        let gate = Gate {
            shared,
            generation: shared.generation(),
        };
        engine.observe(now, snapshot, &gate);
    }

    #[test]
    fn a_play_is_reported_start_to_final_with_one_playback() {
        let (fake, log, _) = Fake::new();
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        for i in 0..=14u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        feed(&mut engine, &shared, 15_000, None);
        let log = log.lock().unwrap();
        assert_eq!(log.lookups, ["video1"], "one tracking lookup per play");
        assert_eq!(log.starts, [("playback1".to_string(), 500)]);
        assert_eq!(log.watches.len(), 2, "the 10 s ping and the final one");
        assert!(log.watches.iter().all(|(p, _)| p == "playback1"));
        assert!(log.watches[1].1.is_final);
    }

    #[test]
    fn a_track_skipped_while_its_tracking_loads_is_never_reported() {
        let (mut fake, log, _) = Fake::new();
        let shared = Arc::new(Shared::new());
        let moved = shared.clone();
        // While the first lookup is in flight the player moves on to entry 2.
        fake.during_lookup = Some(Box::new(move || moved.observed(Some(&playing(2, 100)))));
        let mut engine = Engine::new(fake);
        feed(&mut engine, &shared, 0, Some(playing(1, 500)));
        let log = log.lock().unwrap();
        assert_eq!(log.lookups.len(), 1);
        assert!(log.starts.is_empty());
        assert!(log.watches.is_empty());
    }

    #[test]
    fn cancelling_while_a_lookup_is_in_flight_sends_nothing() {
        let (mut fake, log, _) = Fake::new();
        let shared = Arc::new(Shared::new());
        let cancelled = shared.clone();
        fake.during_lookup = Some(Box::new(move || {
            cancelled.cancel();
        }));
        let mut engine = Engine::new(fake);
        feed(&mut engine, &shared, 0, Some(playing(1, 500)));
        assert!(log.lock().unwrap().starts.is_empty());
    }

    #[test]
    fn a_play_is_never_reported_to_a_different_account() {
        let (fake, log, account) = Fake::new();
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        for i in 0..=5u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        assert_eq!(log.lock().unwrap().starts.len(), 1);
        // Someone else signs in, mid-track.
        *account.lock().unwrap() = Some("bob".into());
        for i in 6..=40u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        feed(&mut engine, &shared, 41_000, None);
        assert!(log.lock().unwrap().watches.is_empty(), "nothing for the old play");
        // Their own plays are reported, to them.
        for i in 0..=2u64 {
            feed(&mut engine, &shared, 50_000 + i * 1000, Some(playing(2, 500 + i * 1000)));
        }
        assert_eq!(log.lock().unwrap().starts.len(), 2);
    }

    #[test]
    fn a_signed_out_player_reports_nothing() {
        let (fake, log, account) = Fake::new();
        *account.lock().unwrap() = None;
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        for i in 0..=12u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        let log = log.lock().unwrap();
        assert!(log.lookups.is_empty(), "not even a lookup");
        assert!(log.starts.is_empty() && log.watches.is_empty());
    }

    #[test]
    fn a_response_with_no_tracking_reports_nothing_and_is_not_retried() {
        let (mut fake, log, _) = Fake::new();
        fake.has_tracking = false;
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        for i in 0..=40u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        let log = log.lock().unwrap();
        assert_eq!(log.lookups.len(), 1);
        assert!(log.starts.is_empty() && log.watches.is_empty());
    }

    #[test]
    fn failures_are_not_retried_and_a_run_of_them_stands_reporting_down() {
        let (mut fake, log, _) = Fake::new();
        fake.fail_lookup = true;
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        // Ten tracks in a row, each played a few seconds.
        let mut t = 0;
        for entry in 1..=10u64 {
            for i in 0..3u64 {
                feed(&mut engine, &shared, t, Some(playing(entry, 500 + i * 1000)));
                t += 1000;
            }
        }
        {
            let log = log.lock().unwrap();
            assert_eq!(
                log.lookups.len(),
                BREAKER_FAILURES as usize,
                "one try per track, then it stood down"
            );
        }
        // After the pause it tries again.
        t += BREAKER_PAUSE_MS;
        feed(&mut engine, &shared, t, Some(playing(11, 500)));
        assert_eq!(log.lock().unwrap().lookups.len(), BREAKER_FAILURES as usize + 1);
    }

    #[test]
    fn a_failed_start_ping_means_nothing_more_for_that_play() {
        let (mut fake, log, _) = Fake::new();
        fake.fail_pings = true;
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        for i in 0..=35u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        feed(&mut engine, &shared, 36_000, None);
        let log = log.lock().unwrap();
        assert_eq!(log.lookups.len(), 1);
        assert!(log.starts.is_empty() && log.watches.is_empty());
    }

    #[test]
    fn cancel_drops_the_play_in_progress() {
        let (fake, log, _) = Fake::new();
        let shared = Shared::new();
        let mut engine = Engine::new(fake);
        for i in 0..=5u64 {
            feed(&mut engine, &shared, i * 1000, Some(playing(1, 500 + i * 1000)));
        }
        shared.cancel();
        engine.cancel();
        feed(&mut engine, &shared, 6_000, None);
        assert!(log.lock().unwrap().watches.is_empty());
    }

    // ---- the thread --------------------------------------------------------------------

    #[test]
    fn the_thread_reporter_reports_and_cancel_stops_it() {
        let (fake, log, _) = Fake::new();
        let reporter = ThreadReporter::spawn(fake).expect("thread starts");
        reporter.observe(Some(playing(1, 500)));
        wait_until(|| log.lock().unwrap().starts.len() == 1);
        reporter.cancel();
        reporter.observe(None);
        // A fresh play afterwards is reported again (a new lookup).
        reporter.observe(Some(playing(2, 500)));
        wait_until(|| log.lock().unwrap().starts.len() == 2);
        let log = log.lock().unwrap();
        assert_eq!(log.lookups, ["video1", "video2"]);
        assert!(log.watches.is_empty(), "the cancelled play got no final ping");
    }

    fn wait_until(mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
