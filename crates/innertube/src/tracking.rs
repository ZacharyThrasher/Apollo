//! Play reporting: the pings the web client sends so a play shows up in YouTube Music's history
//! and keeps feeding recommendations.
//!
//! A `player` response carries `playbackTracking`: a `videostatsPlaybackUrl` the client calls
//! once when playback starts and a `videostatsWatchtimeUrl` it calls on a schedule while the
//! track plays (and when it pauses or ends). Each URL already holds the video's own tokens
//! (`docid`, `ei`, `vm`, ...); the client adds its own per-playback parameters (`cpn`, `cmt`,
//! `st`/`et`, `state`, ...). This module reads the two URLs out of a response and builds the
//! final URLs. It is pure: no network, no clock, no randomness beyond [`new_nonce`].
//!
//! The URLs are never logged and never leave the process except to a YouTube host
//! ([`checked_url`]), because the request that follows carries the account's cookie.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};

use reqwest::Url;
use serde_json::Value;

use crate::transport::Error;

/// What YouTube's web client reports itself as in these pings.
const CLIENT_NAME: &str = "WEB_REMIX";
/// The pings' own protocol version (`ver=2`, as the web client sends).
const PROTOCOL_VERSION: &str = "2";
/// The first watch-time pings when the response does not say (wall-clock seconds since the
/// playback started), as the web client does.
const DEFAULT_SCHEDULE_SECS: [u32; 3] = [10, 20, 30];
/// The gap between watch-time pings once the schedule is used up.
const DEFAULT_INTERVAL_SECS: u32 = 40;
/// Characters a client playback nonce is drawn from (the web client's alphabet).
const NONCE_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// The tracking half of a `player` response.
#[derive(Clone, PartialEq, Eq)]
pub struct PlaybackTracking {
    /// `videostatsPlaybackUrl.baseUrl`: the "playback started" ping.
    pub playback_url: String,
    /// `videostatsWatchtimeUrl.baseUrl`: the watch-time ping.
    pub watchtime_url: String,
    /// `videostatsScheduledFlushWalltimeSeconds`: when the first watch-time pings go out.
    pub schedule_secs: Vec<u32>,
    /// `videostatsDefaultFlushIntervalSeconds`: the gap between pings after the schedule.
    pub interval_secs: u32,
}

// The URLs carry per-video tokens that are tied to the account's session.
impl std::fmt::Debug for PlaybackTracking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackTracking")
            .field("schedule_secs", &self.schedule_secs)
            .field("interval_secs", &self.interval_secs)
            .finish_non_exhaustive()
    }
}

/// Reads the tracking URLs out of a raw `player` response. `None` when the response carries no
/// usable pair (unplayable, or a client that is not served tracking): nothing to report.
pub fn parse_playback_tracking(value: &Value) -> Option<PlaybackTracking> {
    let tracking = value.get("playbackTracking")?;
    let url = |key: &str| {
        tracking
            .get(key)?
            .get("baseUrl")?
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
    };
    let mut schedule_secs: Vec<u32> = tracking
        .get("videostatsScheduledFlushWalltimeSeconds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_u64)
                .filter_map(|s| u32::try_from(s).ok())
                .filter(|s| *s > 0)
                .collect()
        })
        .unwrap_or_default();
    // The schedule is a list of increasing times; anything else is not believed.
    if schedule_secs.is_empty() || schedule_secs.windows(2).any(|w| w[0] >= w[1]) {
        schedule_secs = DEFAULT_SCHEDULE_SECS.to_vec();
    }
    let interval_secs = tracking
        .get("videostatsDefaultFlushIntervalSeconds")
        .and_then(Value::as_u64)
        .and_then(|s| u32::try_from(s).ok())
        .filter(|s| *s > 0)
        .unwrap_or(DEFAULT_INTERVAL_SECS);
    Some(PlaybackTracking {
        playback_url: url("videostatsPlaybackUrl")?,
        watchtime_url: url("videostatsWatchtimeUrl")?,
        schedule_secs,
        interval_secs,
    })
}

/// A client playback nonce (`cpn`): 16 characters, fresh for each playback and shared by every
/// ping about it.
pub fn new_nonce() -> String {
    // `RandomState` is seeded from the OS per instance; one hasher per 8 characters is plenty
    // for an identifier that only has to differ between playbacks.
    let mut out = String::with_capacity(16);
    for _ in 0..2 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u8(0);
        let mut bits = hasher.finish();
        for _ in 0..8 {
            out.push(NONCE_ALPHABET[(bits & 63) as usize] as char);
            bits >>= 6;
        }
    }
    out
}

/// Whether the player was playing or paused when a watch-time ping was taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PingState {
    Playing,
    Paused,
}

/// One watch-time ping: what was played since the last one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchtimePing {
    /// Where the player is now.
    pub position_ms: u64,
    /// The stretches of the track played since the previous ping (start, end), oldest first.
    /// More than one when the listener sought.
    pub ranges: Vec<(u64, u64)>,
    /// Wall-clock time since the playback started, paused time included.
    pub elapsed_ms: u64,
    /// The track's length, when known.
    pub duration_ms: Option<u64>,
    pub state: PingState,
    /// The last ping of this playback (the track ended or was left).
    pub is_final: bool,
    /// The player volume, 0 to 100.
    pub volume: u8,
    pub muted: bool,
}

/// Seconds as the web client writes them: three decimals.
fn secs(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

fn with_query(base: &str, params: &[(&str, String)]) -> Option<String> {
    let mut url = Url::parse(base).ok()?;
    {
        let mut query = url.query_pairs_mut();
        for (k, v) in params {
            query.append_pair(k, v);
        }
    }
    Some(url.into())
}

/// The "playback started" URL: the response's own URL plus this playback's nonce and position.
pub fn playback_url(
    base: &str,
    nonce: &str,
    client_version: &str,
    position_ms: u64,
) -> Option<String> {
    with_query(
        base,
        &[
            ("ver", PROTOCOL_VERSION.into()),
            ("c", CLIENT_NAME.into()),
            ("cver", client_version.into()),
            ("cplayer", "UNIPLAYER".into()),
            ("cpn", nonce.into()),
            ("cmt", secs(position_ms)),
        ],
    )
}

/// A watch-time URL for one ping of the playback `nonce`.
pub fn watchtime_url(
    base: &str,
    nonce: &str,
    client_version: &str,
    ping: &WatchtimePing,
) -> Option<String> {
    let list = |pick: fn(&(u64, u64)) -> u64| {
        ping.ranges.iter().map(|r| secs(pick(r))).collect::<Vec<_>>().join(",")
    };
    let mut params = vec![
        ("ver", PROTOCOL_VERSION.into()),
        ("c", CLIENT_NAME.into()),
        ("cver", client_version.into()),
        ("cplayer", "UNIPLAYER".into()),
        ("cpn", nonce.into()),
        ("cmt", secs(ping.position_ms)),
        ("rt", secs(ping.elapsed_ms)),
        ("st", list(|r| r.0)),
        ("et", list(|r| r.1)),
        (
            "state",
            match ping.state {
                PingState::Playing => "playing",
                PingState::Paused => "paused",
            }
            .into(),
        ),
        ("volume", ping.volume.min(100).to_string()),
        ("muted", u8::from(ping.muted).to_string()),
    ];
    if let Some(len) = ping.duration_ms {
        params.push(("len", secs(len)));
    }
    if ping.is_final {
        params.push(("final", "1".into()));
    }
    with_query(base, &params)
}

/// Parses `raw` and accepts it only if it is an https URL on a YouTube host. The pings go out
/// with the account's cookie, so a URL pointing anywhere else is refused, whatever a response
/// said.
pub fn checked_url(raw: &str) -> Result<Url, Error> {
    let url = Url::parse(raw).map_err(|_| Error::Other("tracking url is not a url".into()))?;
    let host_ok = url.host_str().is_some_and(|h| h == "youtube.com" || h.ends_with(".youtube.com"));
    let plain = url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none();
    if host_ok && plain {
        Ok(url)
    } else {
        Err(Error::Other("tracking url is not a YouTube https url".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PLAYBACK: &str = "https://s.youtube.com/api/stats/playback?cl=1&docid=abc&ei=E1&of=O1&vm=V1";
    const WATCHTIME: &str =
        "https://s.youtube.com/api/stats/watchtime?cl=1&docid=abc&ei=E1&of=O1&vm=V1";

    fn response() -> Value {
        json!({
            "playabilityStatus": {"status": "OK"},
            "playbackTracking": {
                "videostatsPlaybackUrl": {"baseUrl": PLAYBACK},
                "videostatsDelayplayUrl": {"baseUrl": "https://s.youtube.com/api/stats/delayplay?x=1"},
                "videostatsWatchtimeUrl": {"baseUrl": WATCHTIME},
                "ptrackingUrl": {"baseUrl": "https://www.youtube.com/ptracking?x=1"},
                "videostatsScheduledFlushWalltimeSeconds": [10, 20, 30],
                "videostatsDefaultFlushIntervalSeconds": 40
            }
        })
    }

    fn query(url: &str) -> Vec<(String, String)> {
        Url::parse(url)
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    }

    fn get<'a>(q: &'a [(String, String)], key: &str) -> Option<&'a str> {
        q.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn ping() -> WatchtimePing {
        WatchtimePing {
            position_ms: 10_020,
            ranges: vec![(0, 10_020)],
            elapsed_ms: 10_100,
            duration_ms: Some(215_000),
            state: PingState::Playing,
            is_final: false,
            volume: 70,
            muted: false,
        }
    }

    #[test]
    fn reads_both_urls_and_the_schedule() {
        let t = parse_playback_tracking(&response()).unwrap();
        assert_eq!(t.playback_url, PLAYBACK);
        assert_eq!(t.watchtime_url, WATCHTIME);
        assert_eq!(t.schedule_secs, [10, 20, 30]);
        assert_eq!(t.interval_secs, 40);
    }

    #[test]
    fn a_response_without_the_pair_has_nothing_to_report() {
        assert!(parse_playback_tracking(&json!({})).is_none());
        assert!(parse_playback_tracking(&json!({"playbackTracking": {}})).is_none());
        let only_playback = json!({"playbackTracking": {"videostatsPlaybackUrl": {"baseUrl": PLAYBACK}}});
        assert!(parse_playback_tracking(&only_playback).is_none(), "both are needed");
        let empty = json!({"playbackTracking": {
            "videostatsPlaybackUrl": {"baseUrl": ""},
            "videostatsWatchtimeUrl": {"baseUrl": WATCHTIME}
        }});
        assert!(parse_playback_tracking(&empty).is_none());
    }

    #[test]
    fn a_missing_or_odd_schedule_falls_back_to_the_web_clients() {
        let mut v = response();
        v["playbackTracking"]["videostatsScheduledFlushWalltimeSeconds"] = json!([30, 10]);
        v["playbackTracking"]["videostatsDefaultFlushIntervalSeconds"] = json!(0);
        let t = parse_playback_tracking(&v).unwrap();
        assert_eq!(t.schedule_secs, [10, 20, 30]);
        assert_eq!(t.interval_secs, 40);
        v["playbackTracking"].as_object_mut().unwrap().remove("videostatsScheduledFlushWalltimeSeconds");
        assert_eq!(parse_playback_tracking(&v).unwrap().schedule_secs, [10, 20, 30]);
    }

    #[test]
    fn debug_shows_no_url() {
        let t = parse_playback_tracking(&response()).unwrap();
        let shown = format!("{t:?}");
        assert!(!shown.contains("youtube.com") && !shown.contains("docid"), "{shown}");
    }

    #[test]
    fn the_start_url_keeps_the_responses_tokens_and_adds_the_playbacks_own() {
        let url = playback_url(PLAYBACK, "NONCE0123456789a", "1.20260101.01.00", 1_250).unwrap();
        assert!(url.starts_with("https://s.youtube.com/api/stats/playback?cl=1&docid=abc&ei=E1&of=O1&vm=V1&"));
        let q = query(&url);
        assert_eq!(get(&q, "docid"), Some("abc"));
        assert_eq!(get(&q, "ver"), Some("2"));
        assert_eq!(get(&q, "c"), Some("WEB_REMIX"));
        assert_eq!(get(&q, "cver"), Some("1.20260101.01.00"));
        assert_eq!(get(&q, "cpn"), Some("NONCE0123456789a"));
        assert_eq!(get(&q, "cmt"), Some("1.250"));
    }

    #[test]
    fn the_watchtime_url_carries_the_played_ranges() {
        let mut p = ping();
        p.ranges = vec![(0, 10_020), (95_000, 99_500)];
        let url = watchtime_url(WATCHTIME, "NONCE0123456789a", "1.2", &p).unwrap();
        let q = query(&url);
        assert_eq!(get(&q, "cpn"), Some("NONCE0123456789a"));
        assert_eq!(get(&q, "cmt"), Some("10.020"));
        assert_eq!(get(&q, "rt"), Some("10.100"));
        assert_eq!(get(&q, "st"), Some("0.000,95.000"));
        assert_eq!(get(&q, "et"), Some("10.020,99.500"));
        assert_eq!(get(&q, "state"), Some("playing"));
        assert_eq!(get(&q, "len"), Some("215.000"));
        assert_eq!(get(&q, "volume"), Some("70"));
        assert_eq!(get(&q, "muted"), Some("0"));
        assert_eq!(get(&q, "final"), None, "only the last ping says final");
    }

    #[test]
    fn a_final_paused_ping_says_so_and_a_missing_length_is_left_out() {
        let mut p = ping();
        p.state = PingState::Paused;
        p.is_final = true;
        p.duration_ms = None;
        p.muted = true;
        let q = query(&watchtime_url(WATCHTIME, "n", "1", &p).unwrap());
        assert_eq!(get(&q, "state"), Some("paused"));
        assert_eq!(get(&q, "final"), Some("1"));
        assert_eq!(get(&q, "len"), None);
        assert_eq!(get(&q, "muted"), Some("1"));
    }

    #[test]
    fn values_are_percent_encoded() {
        let url = playback_url(PLAYBACK, "a&b=c d", "1 2", 0).unwrap();
        let q = query(&url);
        assert_eq!(get(&q, "cpn"), Some("a&b=c d"));
        assert_eq!(get(&q, "cver"), Some("1 2"));
        assert!(!url.contains(' '), "{url}");
    }

    #[test]
    fn a_base_that_is_not_a_url_builds_nothing() {
        assert!(playback_url("not a url", "n", "1", 0).is_none());
        assert!(watchtime_url("", "n", "1", &ping()).is_none());
    }

    #[test]
    fn nonces_are_sixteen_url_safe_characters_and_differ() {
        let a = new_nonce();
        let b = new_nonce();
        assert_eq!(a.len(), 16);
        assert!(a.bytes().all(|c| NONCE_ALPHABET.contains(&c)), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn only_https_youtube_hosts_are_pinged() {
        for ok in [
            "https://s.youtube.com/api/stats/playback?a=1",
            "https://www.youtube.com/api/stats/watchtime",
            "https://music.youtube.com/x",
            "https://youtube.com/x",
        ] {
            assert!(checked_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "http://s.youtube.com/api/stats/playback",
            "https://evil.example/api/stats/playback",
            "https://youtube.com.evil.example/x",
            "https://notyoutube.com/x",
            "https://user:pw@s.youtube.com/x",
            "https://s.youtube.com:8443/x",
            "ftp://s.youtube.com/x",
            "s.youtube.com/x",
            "",
        ] {
            assert!(checked_url(bad).is_err(), "{bad}");
        }
    }
}
