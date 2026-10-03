//! yt-dlp output -> `StreamCandidate`, and yt-dlp failure text -> `Failure`.
//! Pure functions; nothing here logs or returns raw text, URLs or titles.

use std::time::Duration;

use apollo_domain::lease::{AudioCodec, LeaseError, StreamCandidate, Transport};
use apollo_domain::model::VideoId;
use apollo_domain::playback::Failure;
use url::Url;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    NotJson,
    MissingField,
}

/// Parse the `-J` document. The candidate is untrusted until `UrlPolicy::validate` accepts it.
pub fn parse_ytdlp_json(bytes: &[u8]) -> Result<StreamCandidate, ParseError> {
    let j: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ParseError::NotJson)?;
    let id = j["id"].as_str().ok_or(ParseError::MissingField)?.to_owned();
    let url = j["url"].as_str().ok_or(ParseError::MissingField)?.to_owned();

    let protocol = j["protocol"].as_str().unwrap_or("").to_ascii_lowercase();
    let note = j["format_note"].as_str().unwrap_or("").to_ascii_lowercase();
    let transport = if protocol.contains("sabr") || note.contains("sabr") || url.contains("/sabr") {
        Transport::Sabr
    } else if protocol == "https" {
        Transport::Https
    } else if protocol.contains("m3u8") || protocol.contains("dash") || protocol.contains("ism") {
        Transport::Manifest
    } else {
        Transport::Other(protocol)
    };

    let acodec = j["acodec"].as_str().unwrap_or("none").to_ascii_lowercase();
    let has_audio = !acodec.is_empty() && acodec != "none";
    let codec = if !has_audio {
        None
    } else if acodec.starts_with("opus") {
        Some(AudioCodec::Opus)
    } else if acodec.starts_with("mp4a") || acodec.starts_with("aac") {
        Some(AudioCodec::Aac)
    } else {
        Some(AudioCodec::Other)
    };

    let headers = j["http_headers"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default();

    Ok(StreamCandidate {
        video_id: id,
        expires_at_unix: expiry_from_url(&url),
        url,
        transport,
        has_audio,
        drm: j["_has_drm"].as_bool().unwrap_or(false),
        codec,
        bitrate_kbps: j["abr"].as_f64().filter(|b| *b > 0.0).map(|b| b.round() as u32),
        sample_rate_hz: j["asr"].as_u64().map(|v| v as u32),
        duration: j["duration"]
            .as_f64()
            .filter(|d| d.is_finite() && *d > 0.0)
            .map(Duration::from_secs_f64),
        headers,
    })
}

/// googlevideo URLs carry `expire=<unix seconds>`. Read it without keeping the URL around.
fn expiry_from_url(url: &str) -> Option<u64> {
    let u = Url::parse(url).ok()?;
    u.query_pairs()
        .find(|(k, _)| k == "expire")
        .and_then(|(_, v)| v.parse().ok())
}

/// Internal outcome code for diagnostics (never contains data from the stream).
pub fn lease_error_code(e: &LeaseError) -> &'static str {
    match e {
        LeaseError::IdMismatch => "id-mismatch",
        LeaseError::InvalidUrl => "invalid-url",
        LeaseError::Scheme => "bad-scheme",
        LeaseError::UserInfo => "url-userinfo",
        LeaseError::Host => "untrusted-host",
        LeaseError::Port => "bad-port",
        LeaseError::Drm => "drm",
        LeaseError::Sabr => "sabr",
        LeaseError::UnsupportedTransport => "unsupported-transport",
        LeaseError::NoAudio => "no-audio",
    }
}

/// Every refused candidate is a specific, user-visible "unsupported" (TEST-19): never a
/// fabricated URL, never an alternate-policy retry.
pub fn lease_error_failure(_e: &LeaseError) -> Failure {
    Failure::Unsupported
}

/// Coarse failure class from yt-dlp's stderr and exit state. The text is only inspected,
/// never stored or surfaced (it can contain URLs and account hints).
pub fn classify_failure(stderr: &str) -> (Failure, &'static str) {
    let t = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| t.contains(n));
    if has(&[
        "sign in to confirm",
        "cookies are no longer valid",
        "login required",
        "please sign in",
    ]) {
        return (Failure::SessionExpired, "session-expired");
    }
    if has(&["http error 429", "too many requests", "rate-limited"]) {
        return (Failure::Throttled, "throttled");
    }
    if has(&[
        "members-only",
        "join this channel",
        "premium",
        "not available in your country",
        "blocked it in your country",
        "video unavailable",
        "private video",
        "this video is not available",
        "age-restricted",
        "confirm your age",
    ]) {
        return (Failure::Restricted, "restricted");
    }
    if has(&[
        "requested format is not available",
        "drm",
        "sabr",
        "no video formats found",
        "signature solving failed",
        "n challenge",
    ]) {
        return (Failure::Unsupported, "unsupported-format");
    }
    if has(&[
        "timed out",
        "temporary failure in name resolution",
        "getaddrinfo failed",
        "network is unreachable",
        "connection reset",
        "connection refused",
        "unable to download",
        "http error 5",
        "remote end closed",
    ]) {
        return (Failure::Network, "network");
    }
    (Failure::Unsupported, "unknown-resolver-failure")
}

/// Which pinned-resolution outcome we publish for diagnostics.
pub fn video_id_of(v: &VideoId) -> &str {
    v.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"{
      "id": "dQw4w9WgXcQ",
      "url": "https://rr1---sn-x.googlevideo.com/videoplayback?expire=1999999999&sig=URLCANARY",
      "protocol": "https", "acodec": "opus", "abr": 265.4, "asr": 48000, "duration": 212.5,
      "ext": "webm", "format_id": "774",
      "http_headers": {"User-Agent": "UA", "Accept": "*/*", "Cookie": "SID=HEADERCANARY"}
    }"#;

    #[test]
    fn parses_a_typical_document() {
        let c = parse_ytdlp_json(DOC.as_bytes()).unwrap();
        assert_eq!(c.video_id, "dQw4w9WgXcQ");
        assert_eq!(c.transport, Transport::Https);
        assert!(c.has_audio && !c.drm);
        assert_eq!(c.codec, Some(AudioCodec::Opus));
        assert_eq!(c.bitrate_kbps, Some(265));
        assert_eq!(c.sample_rate_hz, Some(48_000));
        assert_eq!(c.expires_at_unix, Some(1_999_999_999));
        assert_eq!(c.duration.unwrap().as_secs(), 212);
        assert_eq!(c.headers.len(), 3, "filtering is UrlPolicy's job, not the parser's");
        // Debug is redacted.
        assert!(!format!("{c:?}").contains("URLCANARY"));
    }

    #[test]
    fn transports_are_mapped_conservatively() {
        let mk = |proto: &str| {
            let d = DOC.replace("\"protocol\": \"https\"", &format!("\"protocol\": \"{proto}\""));
            parse_ytdlp_json(d.as_bytes()).unwrap().transport
        };
        assert_eq!(mk("m3u8_native"), Transport::Manifest);
        assert_eq!(mk("http_dash_segments"), Transport::Manifest);
        assert!(matches!(mk("http"), Transport::Other(_)));
        assert_eq!(mk("sabr"), Transport::Sabr);
    }

    #[test]
    fn no_audio_and_drm_are_flagged_not_hidden() {
        let d = DOC.replace("\"acodec\": \"opus\"", "\"acodec\": \"none\"");
        assert!(!parse_ytdlp_json(d.as_bytes()).unwrap().has_audio);
        let d = DOC.replace("\"ext\"", "\"_has_drm\": true, \"ext\"");
        assert!(parse_ytdlp_json(d.as_bytes()).unwrap().drm);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert_eq!(parse_ytdlp_json(b"not json").unwrap_err(), ParseError::NotJson);
        assert_eq!(parse_ytdlp_json(b"{}").unwrap_err(), ParseError::MissingField);
        assert_eq!(parse_ytdlp_json(b"").unwrap_err(), ParseError::NotJson);
        assert_eq!(parse_ytdlp_json(b"[1,2]").unwrap_err(), ParseError::MissingField);
    }

    #[test]
    fn failure_classification() {
        use Failure::*;
        for (text, want) in [
            ("ERROR: [youtube] x: Sign in to confirm you're not a bot", SessionExpired),
            ("The provided YouTube account cookies are no longer valid", SessionExpired),
            ("HTTP Error 429: Too Many Requests", Throttled),
            ("ERROR: This video is available to Music Premium members only", Restricted),
            ("Video unavailable. The uploader has not made this video available in your country", Restricted),
            ("ERROR: Requested format is not available", Unsupported),
            ("ERROR: unable to download video data: timed out", Network),
            ("something nobody has seen before", Unsupported),
        ] {
            assert_eq!(classify_failure(text).0, want, "{text}");
        }
    }
}
