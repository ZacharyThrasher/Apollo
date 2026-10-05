//! One shared answer to "is YouTube telling us to slow down?", and a pacer for background work.
//!
//! Everything that talks to YouTube as the account (the yt-dlp resolver, the loudness fetcher,
//! InnerTube requests) shares one [`Throttle`] because the limit is per account and per address,
//! not per feature. Any 429 puts the throttle into a cooldown that grows with every consecutive
//! 429 (30 s, 1 min, 2 min, ... up to 10 min) and honours a `Retry-After` when the response had
//! one. Two lanes keep playback ahead of speculation:
//!
//! * [`Lane::Foreground`]: something the user is waiting for (a click, Retry). Blocked for the
//!   cooldown itself, so a doomed request does not extend the ban.
//! * [`Lane::Background`]: prefetch, loudness, lookahead. Blocked for twice as long, and only
//!   released after the cooldown ran out, so the first request after a ban is the user's.
//!
//! [`Pacer`] is a token bucket for work that has no 429 yet but must not run flat out.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The first cooldown after a 429.
const BASE: Duration = Duration::from_secs(30);
/// The longest cooldown the exponential growth reaches on its own.
const MAX_BACKOFF: Duration = Duration::from_secs(10 * 60);
/// A `Retry-After` longer than this is not believed (it would lock the app out for hours).
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60 * 60);
/// Background work stays blocked this many times as long as the foreground.
const BACKGROUND_FACTOR: u32 = 2;
/// After this long without a 429 the next one starts again from the base wait.
const QUIET_RESET: Duration = Duration::from_secs(20 * 60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Someone is waiting for the answer.
    Foreground,
    /// Speculative: prefetch, lookahead, loudness.
    Background,
}

#[derive(Default)]
struct State {
    strikes: u32,
    last_strike: Option<Instant>,
    foreground_until: Option<Instant>,
    background_until: Option<Instant>,
}

/// Cheap to clone; clones share the state.
#[derive(Clone, Default)]
pub struct Throttle {
    state: Arc<Mutex<State>>,
}

impl std::fmt::Debug for Throttle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Throttle(strikes={})", self.strikes())
    }
}

impl Throttle {
    pub fn new() -> Self {
        Self::default()
    }

    /// The throttle of this process: the one account and address everything shares.
    pub fn global() -> Throttle {
        static GLOBAL: OnceLock<Throttle> = OnceLock::new();
        GLOBAL.get_or_init(Throttle::new).clone()
    }

    /// How long until `lane` may talk to YouTube again, if it may not now.
    pub fn blocked(&self, lane: Lane) -> Option<Duration> {
        self.blocked_at(lane, Instant::now())
    }

    pub fn blocked_at(&self, lane: Lane, now: Instant) -> Option<Duration> {
        let s = self.state.lock().unwrap();
        let until = match lane {
            Lane::Foreground => s.foreground_until,
            Lane::Background => s.background_until,
        }?;
        until.checked_duration_since(now).filter(|d| !d.is_zero())
    }

    /// A request came back 429. `retry_after` is the server's own `Retry-After`, when it sent
    /// one. Returns how long the foreground waits.
    pub fn report_throttled(&self, retry_after: Option<Duration>) -> Duration {
        self.report_throttled_at(Instant::now(), retry_after)
    }

    pub fn report_throttled_at(&self, now: Instant, retry_after: Option<Duration>) -> Duration {
        let mut s = self.state.lock().unwrap();
        let cooling = s.foreground_until.filter(|u| now < *u);
        // Requests that were already in flight when the first 429 arrived fail together: they
        // neither add a strike nor push the end out. Only a request made after the cooldown ran
        // out is another strike (a server `Retry-After` can still lengthen the current one).
        if let (Some(until), None) = (cooling, retry_after) {
            return until - now;
        }
        if cooling.is_none() {
            if s.last_strike.is_some_and(|at| now.saturating_duration_since(at) > QUIET_RESET) {
                s.strikes = 0;
            }
            s.strikes = s.strikes.saturating_add(1);
            s.last_strike = Some(now);
        }
        let exponent = s.strikes.saturating_sub(1).min(16);
        let backoff = BASE.saturating_mul(1 << exponent).min(MAX_BACKOFF);
        let wait = retry_after.map_or(backoff, |r| backoff.max(r.min(MAX_RETRY_AFTER)));
        let fg = now + wait;
        let bg = now + wait.saturating_mul(BACKGROUND_FACTOR);
        s.foreground_until = Some(s.foreground_until.map_or(fg, |u| u.max(fg)));
        s.background_until = Some(s.background_until.map_or(bg, |u| u.max(bg)));
        wait
    }

    /// A request went through. Ends the cooldown, unless it is a straggler that was already in
    /// flight while one is running.
    pub fn report_ok(&self) {
        self.report_ok_at(Instant::now());
    }

    pub fn report_ok_at(&self, now: Instant) {
        let mut s = self.state.lock().unwrap();
        if s.foreground_until.is_some_and(|u| now < u) {
            return;
        }
        *s = State::default();
    }

    /// Consecutive 429s since the last success.
    pub fn strikes(&self) -> u32 {
        self.state.lock().unwrap().strikes
    }
}

/// A token bucket: up to `capacity` at once, refilled one per `every`.
pub struct Pacer {
    capacity: f64,
    every: Duration,
    state: Mutex<(f64, Instant)>,
}

impl Pacer {
    pub fn new(capacity: u32, every: Duration) -> Self {
        Self {
            capacity: f64::from(capacity.max(1)),
            every,
            state: Mutex::new((f64::from(capacity.max(1)), Instant::now())),
        }
    }

    /// Take a token if there is one.
    pub fn try_take(&self) -> bool {
        self.try_take_at(Instant::now())
    }

    pub fn try_take_at(&self, now: Instant) -> bool {
        let mut s = self.state.lock().unwrap();
        let elapsed = now.saturating_duration_since(s.1);
        s.1 = now.max(s.1);
        if !self.every.is_zero() {
            s.0 = (s.0 + elapsed.as_secs_f64() / self.every.as_secs_f64()).min(self.capacity);
        } else {
            s.0 = self.capacity;
        }
        if s.0 >= 1.0 {
            s.0 -= 1.0;
            true
        } else {
            false
        }
    }
}

/// Parse a `Retry-After` header value in its delta-seconds form. (The HTTP-date form is not
/// used by YouTube; it is ignored and the exponential backoff applies.)
pub fn parse_retry_after(value: &str) -> Option<Duration> {
    value.trim().parse::<u64>().ok().map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn nothing_is_blocked_until_a_429() {
        let t = Throttle::new();
        let now = Instant::now();
        assert_eq!(t.blocked_at(Lane::Foreground, now), None);
        assert_eq!(t.blocked_at(Lane::Background, now), None);
    }

    #[test]
    fn a_429_blocks_the_foreground_for_the_base_wait_and_the_background_for_twice_that() {
        let t = Throttle::new();
        let now = Instant::now();
        assert_eq!(t.report_throttled_at(now, None), secs(30));
        assert_eq!(t.blocked_at(Lane::Foreground, now), Some(secs(30)));
        assert_eq!(t.blocked_at(Lane::Background, now), Some(secs(60)));
        assert_eq!(t.blocked_at(Lane::Foreground, now + secs(31)), None);
        assert_eq!(t.blocked_at(Lane::Background, now + secs(31)), Some(secs(29)));
    }

    #[test]
    fn requests_already_in_flight_do_not_count_as_new_strikes() {
        let t = Throttle::new();
        let now = Instant::now();
        t.report_throttled_at(now, None);
        t.report_throttled_at(now + secs(1), None);
        t.report_throttled_at(now + secs(2), None);
        assert_eq!(t.strikes(), 1);
        assert_eq!(t.blocked_at(Lane::Foreground, now), Some(secs(30)));
    }

    #[test]
    fn each_failed_attempt_after_the_wait_doubles_it_up_to_the_cap() {
        let t = Throttle::new();
        let mut now = Instant::now();
        let mut waits = Vec::new();
        for _ in 0..7 {
            let wait = t.report_throttled_at(now, None);
            waits.push(wait.as_secs());
            now += wait + secs(1);
        }
        assert_eq!(waits, [30, 60, 120, 240, 480, 600, 600]);
    }

    #[test]
    fn retry_after_is_honoured_when_longer_than_the_backoff_and_capped_when_absurd() {
        let t = Throttle::new();
        let now = Instant::now();
        assert_eq!(t.report_throttled_at(now, Some(secs(300))), secs(300));
        assert_eq!(t.blocked_at(Lane::Background, now), Some(secs(600)));
        let t = Throttle::new();
        assert_eq!(t.report_throttled_at(now, Some(secs(5))), secs(30), "never shorter than the backoff");
        let t = Throttle::new();
        assert_eq!(t.report_throttled_at(now, Some(secs(86_400))), secs(3600));
    }

    #[test]
    fn a_long_quiet_spell_starts_the_backoff_over() {
        let t = Throttle::new();
        let now = Instant::now();
        t.report_throttled_at(now, None);
        t.report_throttled_at(now + secs(31), None);
        assert_eq!(t.strikes(), 2);
        assert_eq!(t.report_throttled_at(now + secs(31 + 25 * 60), None), secs(30));
        assert_eq!(t.strikes(), 1);
    }

    #[test]
    fn a_success_after_the_wait_clears_everything() {
        let t = Throttle::new();
        let now = Instant::now();
        t.report_throttled_at(now, None);
        t.report_throttled_at(now + secs(31), None);
        assert_eq!(t.strikes(), 2);
        t.report_ok_at(now + secs(200));
        assert_eq!(t.strikes(), 0);
        assert_eq!(t.blocked_at(Lane::Background, now + secs(200)), None);
    }

    #[test]
    fn a_straggler_success_during_the_wait_changes_nothing() {
        let t = Throttle::new();
        let now = Instant::now();
        t.report_throttled_at(now, None);
        t.report_ok_at(now + secs(2));
        assert_eq!(t.strikes(), 1);
        assert_eq!(t.blocked_at(Lane::Foreground, now + secs(2)), Some(secs(28)));
    }

    #[test]
    fn the_background_stays_blocked_after_the_foreground_may_try_again() {
        let t = Throttle::new();
        let now = Instant::now();
        t.report_throttled_at(now, None);
        let later = now + secs(35);
        assert_eq!(t.blocked_at(Lane::Foreground, later), None);
        assert!(t.blocked_at(Lane::Background, later).is_some());
    }

    #[test]
    fn the_pacer_allows_a_burst_then_one_per_interval() {
        let p = Pacer::new(3, secs(6));
        let now = Instant::now();
        assert!(p.try_take_at(now));
        assert!(p.try_take_at(now));
        assert!(p.try_take_at(now));
        assert!(!p.try_take_at(now), "burst spent");
        assert!(!p.try_take_at(now + secs(5)));
        assert!(p.try_take_at(now + secs(6)));
        assert!(!p.try_take_at(now + secs(7)));
        // A long quiet time refills to the capacity and no further.
        let much_later = now + secs(600);
        assert!(p.try_take_at(much_later));
        assert!(p.try_take_at(much_later));
        assert!(p.try_take_at(much_later));
        assert!(!p.try_take_at(much_later));
    }

    #[test]
    fn retry_after_seconds_parse_and_dates_are_ignored() {
        assert_eq!(parse_retry_after("120"), Some(secs(120)));
        assert_eq!(parse_retry_after(" 7 "), Some(secs(7)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after(""), None);
    }
}
