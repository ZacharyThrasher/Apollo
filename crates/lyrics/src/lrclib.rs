//! LRCLIB (https://lrclib.net): free, no key. It asks for a User-Agent that says who is calling.
//!
//! Lookup order, stopping at the first synced answer (plain text is kept as the fallback):
//!
//! 1. `/api/get` (an exact match on artist, title, album and length) for the title as listed,
//!    then with the decorations cleaned off (`(feat. …)`, `- Remastered`, a trailing `!`), the
//!    first artist only, and with and without the album (a wrong album is a 404).
//! 2. `/api/search` on the cleaned title with the first artist, then as free text
//!    (`artist title`), then on the title alone.
//!
//! A search result is used only when its title and artist are the track's (see
//! [`crate::normalize`]) and its length is within [`DURATION_SLACK_SECS`] of the track's, so a
//! looser search never lands on another song or another cut.

use std::time::Duration;

use serde::Deserialize;

use crate::normalize::{clean_title, key, same_artist, strip_artist_prefix};
use crate::{parse_lrc, FetchError, Lyrics, LyricsQuery, LyricsSource};

pub const USER_AGENT: &str = "Apollo/0.1 (https://github.com/ZacharyThrasher/Apollo)";
const BASE: &str = "https://lrclib.net";
/// A search result's length may differ from the track's by this much (different masters).
pub const DURATION_SLACK_SECS: f64 = 2.0;
/// A result whose artist is spelled too differently to compare (another script) is still taken
/// when its title is the same and its length is this close.
const STRONG_DURATION_SECS: f64 = 1.0;
/// ...and the title is at least this long once reduced to letters and digits.
const STRONG_TITLE_LEN: usize = 5;
const TIMEOUT: Duration = Duration::from_secs(10);

pub struct HttpReply {
    pub status: u16,
    pub body: String,
}

/// A blocking GET. `Err` is a transport failure (offline, timeout); any HTTP status is a reply.
pub trait Http: Send + Sync {
    fn get(&self, url: &str) -> Result<HttpReply, String>;
}

pub struct UreqHttp {
    agent: ureq::Agent,
}

impl UreqHttp {
    pub fn new() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            .user_agent(USER_AGENT)
            // 404 is an answer ("no such track"), not an error.
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl Http for UreqHttp {
    fn get(&self, url: &str) -> Result<HttpReply, String> {
        let mut response = self.agent.get(url).call().map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        let body = response
            .body_mut()
            .with_config()
            .limit(2 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| e.to_string())?;
        Ok(HttpReply { status, body })
    }
}

pub struct Lrclib<H: Http = UreqHttp> {
    http: H,
    base: String,
}

impl Lrclib<UreqHttp> {
    pub fn new() -> Self {
        Self::with_http(UreqHttp::new())
    }
}

impl Default for Lrclib<UreqHttp> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    track_name: String,
    #[serde(default)]
    artist_name: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default)]
    plain_lyrics: Option<String>,
    #[serde(default)]
    synced_lyrics: Option<String>,
}

impl Record {
    fn lyrics(&self) -> Option<Lyrics> {
        if self.instrumental {
            return None;
        }
        if let Some(lrc) = &self.synced_lyrics {
            let lines = parse_lrc(lrc);
            if !lines.is_empty() {
                return Some(Lyrics::Synced(lines));
            }
        }
        let plain = self.plain_lyrics.as_deref()?.trim();
        (!plain.is_empty()).then(|| Lyrics::Plain(plain.to_owned()))
    }
}

impl<H: Http> Lrclib<H> {
    pub fn with_http(http: H) -> Self {
        Self {
            http,
            base: BASE.to_owned(),
        }
    }

    fn call(&self, path: &str, params: &[(&str, String)]) -> Result<HttpReply, FetchError> {
        let query = params
            .iter()
            .map(|(k, v)| format!("{k}={}", percent_encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let reply = self
            .http
            .get(&format!("{}{path}?{query}", self.base))
            .map_err(FetchError)?;
        match reply.status {
            200 | 404 => Ok(reply),
            other => Err(FetchError(format!("HTTP {other}"))),
        }
    }

    /// `/api/get` for one spelling of the track. `None` when LRCLIB has no such entry.
    fn exact(&self, attempt: &GetAttempt, duration: u32) -> Result<Option<Record>, FetchError> {
        let reply = self.call(
            "/api/get",
            &[
                ("artist_name", attempt.artist.clone()),
                ("track_name", attempt.title.clone()),
                ("album_name", attempt.album.clone().unwrap_or_default()),
                ("duration", duration.to_string()),
            ],
        )?;
        if reply.status == 404 {
            return Ok(None);
        }
        serde_json::from_str(&reply.body)
            .map(Some)
            .map_err(|e| FetchError(e.to_string()))
    }

    /// Search results that are the track itself, best first: synced ahead of plain, then the
    /// nearest length.
    fn search(&self, plan: &SearchPlan, q: &LyricsQuery) -> Result<Vec<Record>, FetchError> {
        let reply = self.call("/api/search", &plan.params)?;
        if reply.status == 404 {
            return Ok(Vec::new());
        }
        let mut found: Vec<Record> =
            serde_json::from_str(&reply.body).map_err(|e| FetchError(e.to_string()))?;
        found.retain(|r| r.lyrics().is_some() && is_the_track(r, q));
        let distance = |r: &Record| match (q.duration_secs, r.duration) {
            (Some(want), Some(have)) => (have - f64::from(want)).abs(),
            _ => 0.0,
        };
        found.sort_by(|a, b| {
            let synced = |r: &Record| r.lyrics().is_some_and(|l| l.is_synced());
            synced(b)
                .cmp(&synced(a))
                .then(distance(a).total_cmp(&distance(b)))
        });
        Ok(found)
    }
}

/// One `/api/get` spelling of the track.
#[derive(Clone, Debug, PartialEq, Eq)]
struct GetAttempt {
    title: String,
    artist: String,
    album: Option<String>,
}

/// The `/api/get` requests to try, most specific first, without repeats.
fn get_attempts(q: &LyricsQuery) -> Vec<GetAttempt> {
    let cleaned = clean_title(&q.title);
    let first = if q.first_artist.is_empty() {
        q.artist.clone()
    } else {
        q.first_artist.clone()
    };
    let mut albums = vec![q.album.clone()];
    if q.album.is_some() {
        albums.push(None);
    }
    let mut out: Vec<GetAttempt> = Vec::new();
    for album in albums {
        for (title, artist) in [
            (q.title.clone(), q.artist.clone()),
            (cleaned.clone(), first.clone()),
        ] {
            let attempt = GetAttempt {
                title,
                artist,
                album: album.clone(),
            };
            if !out.contains(&attempt) {
                out.push(attempt);
            }
        }
    }
    out
}

struct SearchPlan {
    params: Vec<(&'static str, String)>,
}

/// The searches to try, in order: the cleaned title with the first artist; free text (finds
/// entries whose artist field is spelled differently, or that are named `Artist - Title`); the
/// title alone.
fn search_plans(q: &LyricsQuery) -> Vec<SearchPlan> {
    let title = clean_title(&q.title);
    let artist = if q.first_artist.is_empty() {
        q.artist.split(',').next().unwrap_or("").trim().to_owned()
    } else {
        q.first_artist.clone()
    };
    let mut plans = Vec::new();
    if !artist.is_empty() {
        plans.push(SearchPlan {
            params: vec![
                ("track_name", title.clone()),
                ("artist_name", artist.clone()),
            ],
        });
        plans.push(SearchPlan {
            params: vec![("q", format!("{artist} {title}"))],
        });
    }
    plans.push(SearchPlan {
        params: vec![("track_name", title)],
    });
    plans
}

/// Whether a search result is this track: the same title and artist, and a length within
/// [`DURATION_SLACK_SECS`] when both are known.
fn is_the_track(r: &Record, q: &LyricsQuery) -> bool {
    let want_title = key(&clean_title(&q.title));
    if want_title.is_empty() {
        return false;
    }
    let candidate = strip_artist_prefix(&r.track_name, &q.artist);
    let title_ok = key(&candidate) == want_title || key(&clean_title(&candidate)) == want_title;
    if !title_ok {
        return false;
    }
    let apart = match (q.duration_secs, r.duration) {
        (Some(want), Some(have)) => Some((have - f64::from(want)).abs()),
        _ => None,
    };
    if apart.is_some_and(|d| d > DURATION_SLACK_SECS) {
        return false;
    }
    let artist_ok = same_artist(&r.artist_name, &q.artist)
        || (!q.first_artist.is_empty() && same_artist(&r.artist_name, &q.first_artist));
    artist_ok
        || (want_title.chars().count() >= STRONG_TITLE_LEN
            && apart.is_some_and(|d| d <= STRONG_DURATION_SECS))
}

impl<H: Http> LyricsSource for Lrclib<H> {
    fn fetch(&self, q: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        let mut fallback = None;
        if let Some(duration) = q.duration_secs {
            for attempt in get_attempts(q) {
                if let Some(lyrics) = self.exact(&attempt, duration)?.and_then(|r| r.lyrics()) {
                    if lyrics.is_synced() {
                        return Ok(Some(lyrics));
                    }
                    fallback.get_or_insert(lyrics);
                }
            }
        }
        // No exact match, or only plain text: look for a synced version among near matches.
        for plan in search_plans(q) {
            if let Some(best) = self.search(&plan, q)?.first().and_then(|r| r.lyrics()) {
                if best.is_synced() {
                    return Ok(Some(best));
                }
                fallback.get_or_insert(best);
            }
        }
        Ok(fallback)
    }
}

/// Encode everything but the unreserved characters, as a query-string value.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const GET_OK: &str = include_str!("../fixtures/get_ok.json");
    const GET_404: &str = include_str!("../fixtures/get_404.json");
    const SEARCH: &str = include_str!("../fixtures/search_results.json");

    /// Answers by matching the request path; remembers what was asked.
    struct Script {
        replies: Vec<(&'static str, Result<(u16, &'static str), &'static str>)>,
        asked: Mutex<Vec<String>>,
    }

    impl Script {
        fn new(replies: Vec<(&'static str, Result<(u16, &'static str), &'static str>)>) -> Self {
            Self {
                replies,
                asked: Mutex::new(Vec::new()),
            }
        }
    }

    impl Http for &Script {
        fn get(&self, url: &str) -> Result<HttpReply, String> {
            self.asked.lock().unwrap().push(url.to_owned());
            for (needle, reply) in &self.replies {
                if url.contains(needle) {
                    return match reply {
                        Ok((status, body)) => Ok(HttpReply {
                            status: *status,
                            body: (*body).to_owned(),
                        }),
                        Err(e) => Err((*e).to_owned()),
                    };
                }
            }
            panic!("unexpected request {url}");
        }
    }

    fn query(duration: Option<u32>) -> LyricsQuery {
        LyricsQuery {
            video_id: "vid".into(),
            title: "Blinding Lights".into(),
            artist: "The Weeknd, Someone".into(),
            first_artist: "The Weeknd".into(),
            album: Some("After Hours".into()),
            duration_secs: duration,
        }
    }

    #[test]
    fn an_exact_match_with_synced_lyrics_is_one_request() {
        let script = Script::new(vec![("/api/get", Ok((200, GET_OK)))]);
        let lyrics = Lrclib::with_http(&script).fetch(&query(Some(200))).unwrap();
        let Some(Lyrics::Synced(lines)) = lyrics else {
            panic!("expected synced lyrics")
        };
        assert_eq!(lines[0].start_ms, 13130);
        assert_eq!(lines[0].text, "Yeah");
        assert_eq!(lines.len(), 6);
        let asked = script.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert!(asked[0].starts_with("https://lrclib.net/api/get?artist_name=The%20Weeknd%2C%20Someone&track_name=Blinding%20Lights&album_name=After%20Hours&duration=200"), "{}", asked[0]);
    }

    #[test]
    fn no_exact_match_falls_back_to_a_search_within_two_seconds() {
        let script = Script::new(vec![
            ("/api/get", Ok((404, GET_404))),
            ("/api/search", Ok((200, SEARCH))),
        ]);
        // 200 s: the 202 s record is within slack; the 248 s radio edit is not.
        let lyrics = Lrclib::with_http(&script).fetch(&query(Some(200))).unwrap();
        assert!(lyrics.unwrap().is_synced());
        let asked = script.asked.lock().unwrap();
        // Four exact lookups (as listed and cleaned, with and without the album), then the search.
        assert_eq!(asked.len(), 5);
        assert!(asked[0].contains("album_name=After%20Hours"));
        assert!(asked[2].contains("album_name=&"), "{}", asked[2]);
        assert!(
            asked[4].contains("/api/search?track_name=Blinding%20Lights&artist_name=The%20Weeknd")
        );
        assert!(
            !asked[4].contains("Someone"),
            "search names the first artist only"
        );
    }

    #[test]
    fn a_search_result_outside_the_slack_is_not_used() {
        let script = Script::new(vec![
            ("/api/get", Ok((404, GET_404))),
            ("/api/search", Ok((200, SEARCH))),
        ]);
        // 230 s is more than 2 s from every record (202, 248, 201).
        let lyrics = Lrclib::with_http(&script).fetch(&query(Some(230))).unwrap();
        assert_eq!(lyrics, None);
        // All four exact lookups and all three searches were tried before giving up.
        assert_eq!(script.asked.lock().unwrap().len(), 7);
    }

    #[test]
    fn the_nearest_synced_result_beats_a_nearer_plain_one() {
        let script = Script::new(vec![
            ("/api/get", Ok((404, GET_404))),
            ("/api/search", Ok((200, SEARCH))),
        ]);
        // 201 s: the plain-only record is exact, the synced one is 1 s away.
        let lyrics = Lrclib::with_http(&script).fetch(&query(Some(201))).unwrap();
        assert!(lyrics.unwrap().is_synced());
    }

    #[test]
    fn plain_only_results_are_the_fallback() {
        let plain_only = r#"{"duration":200.0,"instrumental":false,"plainLyrics":"one\ntwo","syncedLyrics":null}"#;
        let script = Script::new(vec![
            ("/api/get", Ok((200, plain_only))),
            ("/api/search", Ok((200, "[]"))),
        ]);
        let lyrics = Lrclib::with_http(&script).fetch(&query(Some(200))).unwrap();
        assert_eq!(lyrics, Some(Lyrics::Plain("one\ntwo".into())));
    }

    #[test]
    fn without_a_length_the_exact_lookup_is_skipped_and_the_search_is_not_filtered_by_time() {
        let script = Script::new(vec![("/api/search", Ok((200, SEARCH)))]);
        let lyrics = Lrclib::with_http(&script).fetch(&query(None)).unwrap();
        assert!(lyrics.unwrap().is_synced());
        assert_eq!(script.asked.lock().unwrap().len(), 1);
    }

    fn record(title: &str, artist: &str, duration: f64) -> String {
        format!(
            r#"{{"trackName":"{title}","artistName":"{artist}","duration":{duration},"instrumental":false,"plainLyrics":"words","syncedLyrics":"[00:01.00]hello"}}"#
        )
    }

    fn found_for(q: LyricsQuery, search_body: &'static str) -> Option<Lyrics> {
        let script = Script::new(vec![
            ("/api/get", Ok((404, GET_404))),
            ("/api/search", Ok((200, search_body))),
        ]);
        Lrclib::with_http(&script).fetch(&q).unwrap()
    }

    fn track(title: &str, artist: &str, secs: u32) -> LyricsQuery {
        LyricsQuery {
            video_id: "vid".into(),
            title: title.into(),
            artist: artist.into(),
            first_artist: artist.split(", ").next().unwrap().into(),
            album: None,
            duration_secs: Some(secs),
        }
    }

    #[test]
    fn the_exact_lookup_is_retried_with_a_cleaned_title_and_without_the_album() {
        let script = Script::new(vec![
            ("/api/get", Ok((404, GET_404))),
            ("/api/search", Ok((200, "[]"))),
        ]);
        let mut q = track(
            "Halo - Remastered 2009 (feat. Someone)",
            "Beyoncé, Someone",
            261,
        );
        q.album = Some("I AM... SASHA FIERCE (Deluxe)".into());
        let _ = Lrclib::with_http(&script).fetch(&q);
        let asked = script.asked.lock().unwrap();
        let gets: Vec<&String> = asked.iter().filter(|u| u.contains("/api/get")).collect();
        assert_eq!(gets.len(), 4);
        // As listed, with the album; cleaned and first artist, with the album; then both again
        // without it.
        assert!(gets[0]
            .contains("artist_name=Beyonc%C3%A9%2C%20Someone&track_name=Halo%20-%20Remastered"));
        assert!(gets[1].contains("artist_name=Beyonc%C3%A9&track_name=Halo&album_name=I%20AM"));
        assert!(gets[2].contains("Remastered") && gets[2].contains("album_name=&"));
        assert!(gets[3].contains("artist_name=Beyonc%C3%A9&track_name=Halo&album_name=&"));
    }

    #[test]
    fn a_wrong_album_does_not_stop_a_match_without_it() {
        // 404 with the album, found without it.
        struct AlbumStrict;
        impl Http for AlbumStrict {
            fn get(&self, url: &str) -> Result<HttpReply, String> {
                let has_album = !url.contains("album_name=&") && url.contains("album_name=");
                Ok(if url.contains("/api/get") && !has_album {
                    HttpReply {
                        status: 200,
                        body: GET_OK.to_owned(),
                    }
                } else {
                    HttpReply {
                        status: 404,
                        body: GET_404.to_owned(),
                    }
                })
            }
        }
        let mut q = query(Some(200));
        q.album = Some("Some Deluxe Edition".into());
        assert!(Lrclib::with_http(AlbumStrict)
            .fetch(&q)
            .unwrap()
            .unwrap()
            .is_synced());
    }

    #[test]
    fn a_search_hit_is_used_when_the_title_and_artist_are_spelled_differently() {
        let body: &'static str = Box::leak(
            format!("[{}]", record("Worlds Greatest", "BIG KRIT", 200.0)).into_boxed_str(),
        );
        assert!(
            found_for(track("World's Greatest!", "Big K.R.I.T.", 200), body)
                .unwrap()
                .is_synced()
        );
        // `Artist - Title` entries (named like a video) and a feat. credit in the title.
        let body: &'static str = Box::leak(
            format!(
                "[{}]",
                record("Lil Nas X - Montero (feat. Someone)", "Lil Nas X", 137.0)
            )
            .into_boxed_str(),
        );
        assert!(found_for(track("Montero", "Lil Nas X", 137), body)
            .unwrap()
            .is_synced());
    }

    #[test]
    fn another_song_or_artist_or_cut_is_never_used() {
        // Same title, other artist, same length.
        let body: &'static str =
            Box::leak(format!("[{}]", record("Halo", "Texas", 261.0)).into_boxed_str());
        assert_eq!(found_for(track("Halo", "Beyoncé", 261), body), None);
        // Same artist, other title, same length.
        let body: &'static str =
            Box::leak(format!("[{}]", record("Halo 2", "Beyoncé", 261.0)).into_boxed_str());
        assert_eq!(found_for(track("Halo", "Beyoncé", 261), body), None);
        // Right song, a cut 5 s longer.
        let body: &'static str =
            Box::leak(format!("[{}]", record("Halo", "Beyoncé", 266.0)).into_boxed_str());
        assert_eq!(found_for(track("Halo", "Beyoncé", 261), body), None);
    }

    #[test]
    fn a_differently_scripted_artist_matches_only_with_a_near_exact_length() {
        let body: &'static str =
            Box::leak(format!("[{}]", record("Lemon", "Kenshi Yonezu", 255.4)).into_boxed_str());
        assert!(found_for(track("Lemon", "米津玄師", 255), body)
            .unwrap()
            .is_synced());
        // 2 s apart is not enough when the artist cannot be compared.
        let body: &'static str =
            Box::leak(format!("[{}]", record("Lemon", "Kenshi Yonezu", 257.4)).into_boxed_str());
        assert_eq!(found_for(track("Lemon", "米津玄師", 255), body), None);
    }

    #[test]
    fn instrumentals_and_empty_records_are_no_lyrics() {
        let instrumental =
            r#"{"duration":200.0,"instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#;
        let script = Script::new(vec![
            ("/api/get", Ok((200, instrumental))),
            ("/api/search", Ok((200, "[]"))),
        ]);
        assert_eq!(
            Lrclib::with_http(&script).fetch(&query(Some(200))).unwrap(),
            None
        );
    }

    #[test]
    fn transport_and_server_failures_are_errors_not_absence() {
        let offline = Script::new(vec![("/api/get", Err("dns error"))]);
        assert!(Lrclib::with_http(&offline)
            .fetch(&query(Some(200)))
            .is_err());
        let broken = Script::new(vec![("/api/get", Ok((503, "oops")))]);
        assert!(Lrclib::with_http(&broken).fetch(&query(Some(200))).is_err());
        let garbage = Script::new(vec![("/api/get", Ok((200, "not json")))]);
        assert!(Lrclib::with_http(&garbage)
            .fetch(&query(Some(200)))
            .is_err());
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(
            percent_encode("Sigur Rós & co"),
            "Sigur%20R%C3%B3s%20%26%20co"
        );
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
    }
}
