//! LRCLIB (https://lrclib.net): free, no key. It asks for a User-Agent that says who is calling.
//!
//! Lookup order: `/api/get` with the track's artist, title, album and duration (an exact
//! match); then `/api/search` keeping only results within [`DURATION_SLACK_SECS`] of the
//! track's length. Synced lyrics are preferred over plain at every step.

use std::time::Duration;

use serde::Deserialize;

use crate::{parse_lrc, FetchError, Lyrics, LyricsQuery, LyricsSource};

pub const USER_AGENT: &str = "Apollo/0.1 (https://github.com/ZacharyThrasher/Apollo)";
const BASE: &str = "https://lrclib.net";
/// A search result's length may differ from the track's by this much (different masters).
pub const DURATION_SLACK_SECS: f64 = 2.0;
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

    fn exact(&self, q: &LyricsQuery) -> Result<Option<Record>, FetchError> {
        let Some(duration) = q.duration_secs else {
            return Ok(None);
        };
        let reply = self.call(
            "/api/get",
            &[
                ("artist_name", q.artist.clone()),
                ("track_name", q.title.clone()),
                ("album_name", q.album.clone().unwrap_or_default()),
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

    /// Search results for the track, best candidate first: nearest length among those within
    /// the slack, synced ahead of plain at equal distance.
    fn search(&self, q: &LyricsQuery, with_artist: bool) -> Result<Vec<Record>, FetchError> {
        let mut params = vec![("track_name", q.title.clone())];
        if with_artist && !q.first_artist.is_empty() {
            params.push(("artist_name", q.first_artist.clone()));
        }
        let reply = self.call("/api/search", &params)?;
        if reply.status == 404 {
            return Ok(Vec::new());
        }
        let mut found: Vec<Record> =
            serde_json::from_str(&reply.body).map_err(|e| FetchError(e.to_string()))?;
        found.retain(|r| {
            r.lyrics().is_some()
                && match (q.duration_secs, r.duration) {
                    (Some(want), Some(have)) => {
                        (have - f64::from(want)).abs() <= DURATION_SLACK_SECS
                    }
                    // Nothing to compare: only a search with the artist is trusted.
                    _ => with_artist,
                }
        });
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

impl<H: Http> LyricsSource for Lrclib<H> {
    fn fetch(&self, q: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        let mut fallback = None;
        if let Some(lyrics) = self.exact(q)?.and_then(|r| r.lyrics()) {
            if lyrics.is_synced() {
                return Ok(Some(lyrics));
            }
            fallback = Some(lyrics);
        }
        // Not an exact match, or only plain text: look for a synced version of a near match.
        for with_artist in [true, false] {
            if let Some(best) = self
                .search(q, with_artist)?
                .first()
                .and_then(|r| r.lyrics())
            {
                if best.is_synced() || fallback.is_none() {
                    return Ok(Some(best));
                }
            }
            if fallback.is_some() && with_artist {
                break;
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
        assert_eq!(asked.len(), 2);
        assert!(
            asked[1].contains("/api/search?track_name=Blinding%20Lights&artist_name=The%20Weeknd")
        );
        assert!(
            !asked[1].contains("Someone"),
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
        // Both searches (with and without the artist) were tried before giving up.
        assert_eq!(script.asked.lock().unwrap().len(), 3);
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
