//! Stream lease validation (BUILD_SPEC RES-03, RES-04).
//!
//! The resolver adapter parses yt-dlp output into a [`StreamCandidate`]. Nothing
//! reaches the media engine unless [`UrlPolicy::validate`] turns it into a
//! [`StreamLease`]. Leases are memory-only and secret-bearing: `Debug` never
//! prints the URL or headers.

use std::fmt;
use std::time::Duration;

use url::{Host, Url};

use crate::model::VideoId;

/// Transport the resolver reports for the chosen audio format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Ordinary progressive HTTPS audio resource. The only transport supported initially.
    Https,
    /// Streaming-server-side adaptive streaming: not a direct audio URL.
    Sabr,
    /// HLS/DASH manifests need their own tests before being allowed.
    Manifest,
    Other(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioCodec {
    Opus,
    Aac,
    Other,
}

/// What the resolver adapter claims. Untrusted until validated.
#[derive(Clone)]
pub struct StreamCandidate {
    pub video_id: String,
    pub url: String,
    pub transport: Transport,
    pub has_audio: bool,
    pub drm: bool,
    pub codec: Option<AudioCodec>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub duration: Option<Duration>,
    /// Unix seconds, when the resolver reports an expiry.
    pub expires_at_unix: Option<u64>,
    pub headers: Vec<(String, String)>,
}

impl fmt::Debug for StreamCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamCandidate")
            .field("video_id", &self.video_id)
            .field("url", &"<redacted>")
            .field("transport", &self.transport)
            .field("headers", &self.headers.len())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseError {
    IdMismatch,
    InvalidUrl,
    Scheme,
    UserInfo,
    Host,
    Port,
    Drm,
    Sabr,
    UnsupportedTransport,
    NoAudio,
}

/// Headers a media request may legitimately need. Everything else (notably
/// Cookie and Authorization) is dropped, never forwarded to a CDN.
const ALLOWED_HEADERS: &[&str] = &[
    "user-agent",
    "accept",
    "accept-language",
    "referer",
    "origin",
];

#[derive(Clone, Debug)]
pub struct UrlPolicy {
    /// Registrable domains whose subdomains may serve media. Matched on label boundaries.
    allowed_domains: Vec<String>,
    #[cfg(any(test, feature = "test-support"))]
    allow_loopback: bool,
}

impl Default for UrlPolicy {
    fn default() -> Self {
        Self::production()
    }
}

impl UrlPolicy {
    pub fn production() -> Self {
        Self {
            allowed_domains: vec!["googlevideo.com".to_owned()],
            #[cfg(any(test, feature = "test-support"))]
            allow_loopback: false,
        }
    }

    /// Explicit, test-only permission for `http(s)://127.0.0.1` / `localhost` servers.
    #[cfg(any(test, feature = "test-support"))]
    pub fn for_tests_allow_loopback() -> Self {
        Self {
            allow_loopback: true,
            ..Self::production()
        }
    }

    /// Check `candidate` against the track the coordinator asked for.
    pub fn validate(
        &self,
        expected: &VideoId,
        candidate: StreamCandidate,
    ) -> Result<StreamLease, LeaseError> {
        if candidate.video_id != expected.as_str() {
            return Err(LeaseError::IdMismatch);
        }
        if candidate.drm {
            return Err(LeaseError::Drm);
        }
        match candidate.transport {
            Transport::Https => {}
            Transport::Sabr => return Err(LeaseError::Sabr),
            Transport::Manifest | Transport::Other(_) => {
                return Err(LeaseError::UnsupportedTransport)
            }
        }
        if !candidate.has_audio {
            return Err(LeaseError::NoAudio);
        }
        let url = self.check_url(&candidate.url)?;
        let headers: Vec<(String, String)> = candidate
            .headers
            .into_iter()
            .filter(|(name, _)| ALLOWED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
            .collect();
        Ok(StreamLease {
            video_id: expected.clone(),
            url,
            headers,
            codec: candidate.codec,
            bitrate_kbps: candidate.bitrate_kbps,
            sample_rate_hz: candidate.sample_rate_hz,
            duration: candidate.duration,
            expires_at_unix: candidate.expires_at_unix,
        })
    }

    /// The same scrutiny applies to every redirect target and manifest resource.
    pub fn check_redirect(&self, target: &str) -> Result<Url, LeaseError> {
        self.check_url(target)
    }

    fn check_url(&self, raw: &str) -> Result<Url, LeaseError> {
        let url = Url::parse(raw).map_err(|_| LeaseError::InvalidUrl)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(LeaseError::UserInfo);
        }
        let host = url.host().ok_or(LeaseError::InvalidUrl)?;

        #[cfg(any(test, feature = "test-support"))]
        if self.allow_loopback {
            let loopback = match &host {
                Host::Domain(d) => *d == "localhost",
                Host::Ipv4(ip) => ip.is_loopback(),
                Host::Ipv6(ip) => ip.is_loopback(),
            };
            if loopback && matches!(url.scheme(), "http" | "https") {
                return Ok(url);
            }
        }

        if url.scheme() != "https" {
            return Err(LeaseError::Scheme);
        }
        if !matches!(url.port(), None | Some(443)) {
            return Err(LeaseError::Port);
        }
        let Host::Domain(domain) = host else {
            return Err(LeaseError::Host); // IP literals are never trusted media hosts
        };
        let allowed = self.allowed_domains.iter().any(|base| {
            domain
                .strip_suffix(base.as_str())
                .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1)
        });
        if allowed {
            Ok(url)
        } else {
            Err(LeaseError::Host)
        }
    }
}

/// Validated, memory-only media lease.
pub struct StreamLease {
    video_id: VideoId,
    url: Url,
    headers: Vec<(String, String)>,
    pub codec: Option<AudioCodec>,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
    pub duration: Option<Duration>,
    pub expires_at_unix: Option<u64>,
}

impl StreamLease {
    pub fn video_id(&self) -> &VideoId {
        &self.video_id
    }

    /// The signed URL. Pass it to the media engine only; never log or persist it.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Headers for a request to `target`. Same-origin only: a redirect to another
    /// origin gets none of them.
    pub fn headers_for(&self, target: &Url) -> &[(String, String)] {
        if target.origin() == self.url.origin() {
            &self.headers
        } else {
            &[]
        }
    }

    /// Whether the lease can still be used. A reported expiry is honored with a
    /// safety margin; an unknown expiry permits immediate use but is never
    /// treated as valid for indefinite reuse (see `max_unknown_age`).
    pub fn is_fresh(
        &self,
        now_unix: u64,
        margin: Duration,
        age: Duration,
        max_unknown_age: Duration,
    ) -> bool {
        match self.expires_at_unix {
            Some(exp) => now_unix.saturating_add(margin.as_secs()) < exp,
            None => age <= max_unknown_age,
        }
    }
}

impl fmt::Debug for StreamLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamLease")
            .field("video_id", &self.video_id)
            .field("url", &"<redacted>")
            .field("headers", &self.headers.len())
            .field("codec", &self.codec)
            .field("bitrate_kbps", &self.bitrate_kbps)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vid() -> VideoId {
        VideoId::new("abc123DEF_-")
    }

    fn cand(url: &str) -> StreamCandidate {
        StreamCandidate {
            video_id: "abc123DEF_-".into(),
            url: url.into(),
            transport: Transport::Https,
            has_audio: true,
            drm: false,
            codec: Some(AudioCodec::Opus),
            bitrate_kbps: Some(270),
            sample_rate_hz: Some(48_000),
            duration: Some(Duration::from_secs(200)),
            expires_at_unix: Some(10_000),
            headers: vec![],
        }
    }

    const GOOD: &str = "https://rr3---sn-abc.googlevideo.com/videoplayback?expire=1&sig=SECRET";

    fn check(url: &str) -> Result<StreamLease, LeaseError> {
        UrlPolicy::production().validate(&vid(), cand(url))
    }

    // TEST-14
    #[test]
    fn accepts_a_googlevideo_https_url() {
        let lease = check(GOOD).expect("valid");
        assert_eq!(lease.video_id(), &vid());
    }

    #[test]
    fn rejects_mismatched_video_id() {
        let mut c = cand(GOOD);
        c.video_id = "someOtherId".into();
        assert_eq!(
            UrlPolicy::production().validate(&vid(), c).unwrap_err(),
            LeaseError::IdMismatch
        );
    }

    #[test]
    fn rejects_unsafe_schemes() {
        for u in [
            "http://rr1.googlevideo.com/videoplayback",
            "file:///etc/passwd",
            "ftp://rr1.googlevideo.com/x",
            "javascript:alert(1)",
            "C:\\Users\\me\\song.m4a",
            "/tmp/song.m4a",
            "",
        ] {
            assert!(check(u).is_err(), "{u} must be rejected");
        }
        assert_eq!(
            check("http://rr1.googlevideo.com/x").unwrap_err(),
            LeaseError::Scheme
        );
    }

    #[test]
    fn host_match_uses_label_boundaries_not_substrings() {
        for u in [
            "https://googlevideo.com.evil.example/x",
            "https://evilgooglevideo.com/x",
            "https://googlevideo.com/x", // bare registrable domain is not a media host
            "https://notgooglevideo.com/x",
            "https://rr1.googlevideo.com.attacker.net/x",
            "https://example.com/googlevideo.com",
            "https://example.com/?u=rr1.googlevideo.com",
        ] {
            assert_eq!(check(u).unwrap_err(), LeaseError::Host, "{u}");
        }
        assert!(check("https://RR1.GoogleVideo.COM/x").is_ok());
    }

    #[test]
    fn rejects_userinfo_ip_literals_and_odd_ports() {
        assert_eq!(
            check("https://rr1.googlevideo.com@evil.example/x").unwrap_err(),
            LeaseError::UserInfo
        );
        assert_eq!(
            check("https://user:pw@rr1.googlevideo.com/x").unwrap_err(),
            LeaseError::UserInfo
        );
        assert_eq!(
            check("https://203.0.113.9/x").unwrap_err(),
            LeaseError::Host
        );
        assert_eq!(check("https://[::1]/x").unwrap_err(), LeaseError::Host);
        assert_eq!(
            check("https://rr1.googlevideo.com:8443/x").unwrap_err(),
            LeaseError::Port
        );
        assert!(check("https://rr1.googlevideo.com:443/x").is_ok());
    }

    #[test]
    fn rejects_unsupported_transports_drm_and_missing_audio() {
        let p = UrlPolicy::production();
        let mut c = cand(GOOD);
        c.transport = Transport::Sabr;
        assert_eq!(p.validate(&vid(), c).unwrap_err(), LeaseError::Sabr);
        let mut c = cand(GOOD);
        c.transport = Transport::Manifest;
        assert_eq!(
            p.validate(&vid(), c).unwrap_err(),
            LeaseError::UnsupportedTransport
        );
        let mut c = cand(GOOD);
        c.drm = true;
        assert_eq!(p.validate(&vid(), c).unwrap_err(), LeaseError::Drm);
        let mut c = cand(GOOD);
        c.has_audio = false;
        assert_eq!(p.validate(&vid(), c).unwrap_err(), LeaseError::NoAudio);
    }

    #[test]
    fn redirects_get_the_same_scrutiny() {
        let p = UrlPolicy::production();
        assert!(p
            .check_redirect("https://rr2.googlevideo.com/videoplayback")
            .is_ok());
        assert!(p.check_redirect("https://evil.example/steal").is_err());
        assert!(p.check_redirect("http://rr2.googlevideo.com/x").is_err());
    }

    #[test]
    fn credential_headers_are_dropped_and_others_are_origin_scoped() {
        let mut c = cand(GOOD);
        c.headers = vec![
            ("Cookie".into(), "SID=secret".into()),
            ("authorization".into(), "Bearer secret".into()),
            ("X-Custom".into(), "x".into()),
            ("User-Agent".into(), "ua".into()),
            ("Referer".into(), "https://music.youtube.com/".into()),
        ];
        let lease = UrlPolicy::production().validate(&vid(), c).unwrap();
        let same = lease.url().clone();
        let names: Vec<_> = lease
            .headers_for(&same)
            .iter()
            .map(|(n, _)| n.to_ascii_lowercase())
            .collect();
        assert_eq!(names, ["user-agent", "referer"]);
        let other = Url::parse("https://rr9.googlevideo.com/videoplayback").unwrap();
        assert!(
            lease.headers_for(&other).is_empty(),
            "cross-origin redirect gets no headers"
        );
    }

    // TEST-23 (unit part)
    #[test]
    fn debug_output_never_contains_the_url_or_header_values() {
        let mut c = cand(GOOD);
        c.headers = vec![("User-Agent".into(), "ua-secret-value".into())];
        let shown_candidate = format!("{c:?}");
        let lease = UrlPolicy::production().validate(&vid(), c).unwrap();
        let shown_lease = format!("{lease:?}");
        for shown in [shown_candidate, shown_lease] {
            assert!(!shown.contains("SECRET"), "{shown}");
            assert!(!shown.contains("googlevideo"), "{shown}");
            assert!(!shown.contains("ua-secret-value"), "{shown}");
        }
    }

    #[test]
    fn expiry_uses_a_safety_margin_and_unknown_expiry_is_not_cached_forever() {
        let lease = check(GOOD).unwrap(); // expires at 10_000
        let m = Duration::from_secs(60);
        let (age, max) = (Duration::ZERO, Duration::from_secs(300));
        assert!(lease.is_fresh(9_000, m, age, max));
        assert!(
            !lease.is_fresh(9_950, m, age, max),
            "inside the margin counts as stale"
        );
        let mut c = cand(GOOD);
        c.expires_at_unix = None;
        let unknown = UrlPolicy::production().validate(&vid(), c).unwrap();
        assert!(unknown.is_fresh(0, m, Duration::from_secs(10), max));
        assert!(!unknown.is_fresh(0, m, Duration::from_secs(301), max));
    }

    #[test]
    fn loopback_is_only_allowed_by_the_explicit_test_policy() {
        let local = "http://127.0.0.1:8080/a.m4a";
        assert!(UrlPolicy::production()
            .validate(&vid(), cand(local))
            .is_err());
        assert!(UrlPolicy::for_tests_allow_loopback()
            .validate(&vid(), cand(local))
            .is_ok());
        // ...and the test policy still rejects non-loopback junk.
        assert!(UrlPolicy::for_tests_allow_loopback()
            .validate(&vid(), cand("http://example.com/x"))
            .is_err());
    }
}
