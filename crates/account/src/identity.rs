//! Session and identity validation through an authenticated, account-specific InnerTube call
//! (AUTH-04). A public search is not validation, and neither is the resolver succeeding: M0
//! showed yt-dlp resolves with no or fake cookies. This module is the only thing allowed to say
//! "this session is valid".
//!
//! The JSON shapes below are what the service returned in the M0 owner-run test
//! (`logged_in` flag, `activeAccountHeaderRenderer`) plus a tolerant search for the multi-channel
//! list (`accountItem`). The multi-channel parser is verified only against synthetic fixtures;
//! the owner-run account test must confirm it against a real brand-account login.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sha1::{Digest, Sha1};

use crate::jar::CookieJar;

const ORIGIN: &str = "https://music.youtube.com";
/// A client version that worked in M0. Not a constant to rely on: the service may reject old ones.
const CLIENT_VERSION: &str = "1.20260901.01.00";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36";

#[derive(Clone, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    pub handle: Option<String>,
    /// Channel picture URL when the service gave one (https, Google-hosted images only).
    pub avatar_url: Option<String>,
    /// Opaque id the service uses to address this channel (page id), when it gives one.
    pub page_id: Option<String>,
    pub selected_by_service: bool,
}

impl Identity {
    /// Stable key for "which identity did the owner choose".
    pub fn key(&self) -> String {
        self.page_id.clone().unwrap_or_else(|| self.name.clone())
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Names and handles are personal data: never in logs.
        f.write_str("Identity(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validation {
    /// The service accepted the session. `identities` lists every channel/account the login can
    /// act as; more than one means the owner must choose (never inferred).
    Valid { identities: Vec<Identity> },
    /// The service says this session is not logged in (or there is nothing to authenticate with).
    Expired,
    /// Could not reach the service, or it answered with an error that says nothing about the
    /// session (5xx, 429, timeout). Not a logout.
    Unreachable,
    /// The service answered 200 but not in a shape we understand. Reported as a compatibility
    /// problem, never as valid.
    Unrecognized,
}

#[derive(Debug)]
pub enum NetError {
    Timeout,
    Unreachable(String),
}

pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

pub trait HttpPost: Send + Sync {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse, NetError>;
}

pub trait AccountValidator: Send + Sync {
    fn validate(&self, jar: &CookieJar) -> Validation;
}

/// Real HTTP via ureq with a hard timeout. Response bodies are capped.
pub struct UreqHttp {
    agent: ureq::Agent,
}

impl Default for UreqHttp {
    fn default() -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(20)))
            .build()
            .into();
        Self { agent }
    }
}

impl HttpPost for UreqHttp {
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<HttpResponse, NetError> {
        let mut req = self.agent.post(url);
        for (n, v) in headers {
            req = req.header(n, v);
        }
        match req.send(body.as_bytes()) {
            Ok(mut resp) => {
                let status = resp.status().as_u16();
                let body = resp
                    .body_mut()
                    .with_config()
                    .limit(4 * 1024 * 1024)
                    .read_to_string()
                    .unwrap_or_default();
                Ok(HttpResponse { status, body })
            }
            Err(ureq::Error::Timeout(_)) => Err(NetError::Timeout),
            // Error text can contain the URL; keep only a coarse label.
            Err(_) => Err(NetError::Unreachable("transport error".into())),
        }
    }
}

fn sha1_hex(s: &str) -> String {
    let mut h = Sha1::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// `SAPISIDHASH`-family `Authorization` value. `None` when the jar has no `*APISID` cookie.
pub fn authorization_header(jar: &CookieJar, now_unix: u64) -> Option<String> {
    let mut parts = Vec::new();
    for (scheme, cookie) in [
        ("SAPISIDHASH", "SAPISID"),
        ("SAPISID1PHASH", "__Secure-1PAPISID"),
        ("SAPISID3PHASH", "__Secure-3PAPISID"),
    ] {
        if let Some(c) = jar.get(cookie) {
            parts.push(format!(
                "{scheme} {now_unix}_{}",
                sha1_hex(&format!("{now_unix} {} {ORIGIN}", c.value))
            ));
        }
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Does a cookie with this `domain` attribute get sent to `host`? Standard domain matching: the
/// host equals the domain or is a subdomain of it. A `.google.com` cookie is NOT sent to
/// music.youtube.com.
pub(crate) fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    let d = cookie_domain.trim_start_matches('.').to_ascii_lowercase();
    let h = host.to_ascii_lowercase();
    h == d || h.ends_with(&format!(".{d}"))
}

/// The `Cookie:` header a browser would send to `https://{host}/`: only matching, unexpired
/// cookies. The jar legitimately holds the same name under several domains (for example
/// SAPISID under both .google.com and .youtube.com); sending them all makes the service pick
/// the wrong ones and report a logged-out session.
pub fn cookie_header_for(jar: &CookieJar, host: &str, now_unix: u64) -> String {
    jar.cookies
        .iter()
        .filter(|c| domain_matches(&c.domain, host))
        .filter(|c| c.path == "/" || c.path.is_empty() || "/".starts_with(&c.path))
        .filter(|c| c.expires_unix.is_none_or(|e| e as u64 > now_unix))
        .map(|c| format!("{}={}", c.name, c.value))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The `SAPISID` family must also come from the cookies that match the host.
fn matching_jar(jar: &CookieJar, host: &str) -> CookieJar {
    CookieJar {
        cookies: jar
            .cookies
            .iter()
            .filter(|c| domain_matches(&c.domain, host))
            .cloned()
            .collect(),
        ephemeral_seed: jar.ephemeral_seed,
    }
}

pub struct InnerTubeValidator {
    http: Arc<dyn HttpPost>,
    base: String,
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl InnerTubeValidator {
    pub fn new(http: Arc<dyn HttpPost>) -> Self {
        Self::with_base(http, ORIGIN.to_string())
    }

    pub fn with_base(http: Arc<dyn HttpPost>, base: String) -> Self {
        Self {
            http,
            base,
            clock: Box::new(apollo_services_common::unix_now),
        }
    }

    fn call(&self, jar: &CookieJar, endpoint: &str) -> Result<HttpResponse, NetError> {
        let now = (self.clock)();
        let host = self
            .base
            .split("://")
            .nth(1)
            .unwrap_or(&self.base)
            .split('/')
            .next()
            .unwrap_or("music.youtube.com")
            .to_string();
        let auth = authorization_header(&matching_jar(jar, &host), now).unwrap_or_default();
        let headers = vec![
            ("Cookie".to_string(), cookie_header_for(jar, &host, now)),
            ("Authorization".to_string(), auth),
            ("Origin".to_string(), ORIGIN.to_string()),
            ("X-Origin".to_string(), ORIGIN.to_string()),
            ("X-Goog-AuthUser".to_string(), "0".to_string()),
            ("User-Agent".to_string(), USER_AGENT.to_string()),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];
        let body = serde_json::json!({
            "context": { "client": {
                "clientName": "WEB_REMIX", "clientVersion": CLIENT_VERSION, "hl": "en", "gl": "US"
            }}
        })
        .to_string();
        self.http.post(
            &format!("{}/youtubei/v1/{endpoint}?prettyPrint=false", self.base),
            &headers,
            &body,
        )
    }
}

impl AccountValidator for InnerTubeValidator {
    fn validate(&self, jar: &CookieJar) -> Validation {
        if authorization_header(jar, 0).is_none() {
            return Validation::Expired; // nothing to authenticate with
        }
        let menu = match self.call(jar, "account/account_menu") {
            Ok(r) => r,
            Err(_) => return Validation::Unreachable,
        };
        match menu.status {
            200 => {}
            401 | 403 => return Validation::Expired,
            _ => return Validation::Unreachable, // 429/5xx/other: says nothing about the session
        }
        let Ok(menu_json) = serde_json::from_str::<Value>(&menu.body) else {
            return Validation::Unrecognized;
        };
        match logged_in_flag(&menu_json) {
            Some(true) => {}
            Some(false) => return Validation::Expired,
            None => return Validation::Unrecognized,
        }
        let active = parse_active_account(&menu_json);

        // Which channels can this login act as? A failure here must not invalidate the session;
        // we then know only the active identity.
        let listed = self
            .call(jar, "account/accounts_list")
            .ok()
            .filter(|r| r.status == 200)
            .and_then(|r| serde_json::from_str::<Value>(&r.body).ok())
            .map(|j| parse_account_items(&j))
            .unwrap_or_default();

        let identities = if listed.is_empty() {
            match active {
                Some(a) => vec![a],
                None => return Validation::Unrecognized, // logged in, but no identity to bind
            }
        } else {
            listed
        };
        Validation::Valid { identities }
    }
}

// ---------------------------------------------------------------------------- parsing

fn logged_in_flag(j: &Value) -> Option<bool> {
    let svcs = j.pointer("/responseContext/serviceTrackingParams")?.as_array()?;
    for s in svcs {
        for p in s["params"].as_array().into_iter().flatten() {
            if p["key"] == "logged_in" {
                return Some(p["value"] == "1");
            }
        }
    }
    None
}

fn text_of(v: &Value) -> Option<String> {
    v.get("simpleText")
        .and_then(|t| t.as_str())
        .map(str::to_owned)
        .or_else(|| v.pointer("/runs/0/text").and_then(|t| t.as_str()).map(str::to_owned))
        .filter(|s| !s.is_empty())
}

/// Depth-first search for every value stored under `key` (the response nests deeply and the
/// exact path changes between client versions).
fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                if k == key {
                    out.push(val);
                }
                find_all(val, key, out);
            }
        }
        Value::Array(a) => a.iter().for_each(|x| find_all(x, key, out)),
        _ => {}
    }
}

fn first_string(v: &Value, key: &str) -> Option<String> {
    let mut found = Vec::new();
    find_all(v, key, &mut found);
    found.iter().find_map(|f| f.as_str().map(str::to_owned))
}

/// Hosts the account service serves channel pictures from. Anything else is ignored, so a
/// response can never make the app fetch an arbitrary address.
const AVATAR_HOSTS: &[&str] = &[".googleusercontent.com", ".ggpht.com"];

/// The largest `accountPhoto` thumbnail that is an https URL on a known image host.
fn avatar_url(v: &Value) -> Option<String> {
    let thumbs = v.get("accountPhoto")?.get("thumbnails")?.as_array()?;
    thumbs
        .iter()
        .filter_map(|t| {
            let url = t.get("url")?.as_str()?;
            let host = url.strip_prefix("https://")?.split(['/', '?', '#']).next()?;
            let host = host.rsplit('@').next()?; // never trust userinfo tricks
            let allowed = !host.contains(':')
                && AVATAR_HOSTS.iter().any(|h| host.ends_with(h) && host.len() > h.len());
            allowed.then(|| (t.get("width").and_then(Value::as_u64).unwrap_or(0), url))
        })
        .max_by_key(|(w, _)| *w)
        .map(|(_, url)| url.to_owned())
}

fn parse_active_account(j: &Value) -> Option<Identity> {
    let mut hdr = Vec::new();
    find_all(j, "activeAccountHeaderRenderer", &mut hdr);
    let h = hdr.first()?;
    Some(Identity {
        name: h.get("accountName").and_then(text_of)?,
        handle: h.get("channelHandle").and_then(text_of),
        avatar_url: avatar_url(h),
        page_id: None,
        selected_by_service: true,
    })
}

fn parse_account_items(j: &Value) -> Vec<Identity> {
    let mut items = Vec::new();
    find_all(j, "accountItem", &mut items);
    items
        .into_iter()
        .filter_map(|it| {
            Some(Identity {
                name: it.get("accountName").and_then(text_of)?,
                handle: it.get("accountByline").and_then(text_of),
                avatar_url: avatar_url(it),
                page_id: first_string(it, "pageId"),
                selected_by_service: it["isSelected"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jar::Cookie;
    use std::sync::Mutex;

    fn jar_with(names: &[&str]) -> CookieJar {
        CookieJar {
            cookies: names
                .iter()
                .map(|n| Cookie {
                    name: (*n).into(),
                    value: format!("VALUECANARY-{n}"),
                    domain: ".youtube.com".into(),
                    path: "/".into(),
                    secure: true,
                    http_only: true,
                    expires_unix: Some(4_102_444_800),
                })
                .collect(),
            ephemeral_seed: false,
        }
    }

    const AUTH_NAMES: &[&str] = &["SAPISID", "__Secure-1PAPISID", "__Secure-3PAPISID", "SID"];

    // Synthetic fixtures shaped like the M0 observation (logged_in flag + account header).
    fn menu(logged_in: &str) -> String {
        serde_json::json!({
            "responseContext": {"serviceTrackingParams": [
                {"service": "GFEEDBACK", "params": [{"key": "logged_in", "value": logged_in}]}
            ]},
            "actions": [{"openPopupAction": {"popup": {"multiPageMenuRenderer": {"header": {
                "activeAccountHeaderRenderer": {
                    "accountName": {"runs": [{"text": "Test Person"}]},
                    "channelHandle": {"runs": [{"text": "@testperson"}]}
                }}}}}}]
        })
        .to_string()
    }

    // Synthetic multi-channel list (shape is a best understanding, see module docs).
    fn list(n: usize) -> String {
        let items: Vec<Value> = (0..n)
            .map(|i| {
                serde_json::json!({"accountItem": {
                    "accountName": {"simpleText": format!("Channel {i}")},
                    "accountByline": {"simpleText": format!("@chan{i}")},
                    "isSelected": i == 0,
                    "serviceEndpoint": {"selectActiveIdentityEndpoint": {"supportedTokens": [
                        {"pageIdToken": {"pageId": format!("PAGEID{i}")}}
                    ]}}
                }})
            })
            .collect();
        serde_json::json!({"actions": [{"getMultiPageMenuAction": {"menu": {"multiPageMenuRenderer": {
            "sections": [{"accountSectionListRenderer": {"contents": [
                {"accountItemSectionRenderer": {"contents": items}}
            ]}}]
        }}}}]})
        .to_string()
    }

    struct Mock {
        replies: Mutex<Vec<Result<HttpResponse, NetError>>>,
        seen: Mutex<Vec<(String, Vec<(String, String)>)>>,
    }

    impl Mock {
        fn new(replies: Vec<Result<HttpResponse, NetError>>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into_iter().rev().collect()),
                seen: Mutex::new(vec![]),
            })
        }
    }

    impl HttpPost for Mock {
        fn post(&self, url: &str, headers: &[(String, String)], _body: &str) -> Result<HttpResponse, NetError> {
            self.seen.lock().unwrap().push((url.to_string(), headers.to_vec()));
            self.replies.lock().unwrap().pop().unwrap_or(Err(NetError::Unreachable("no reply".into())))
        }
    }

    fn ok(body: String) -> Result<HttpResponse, NetError> {
        Ok(HttpResponse { status: 200, body })
    }

    fn status(code: u16) -> Result<HttpResponse, NetError> {
        Ok(HttpResponse { status: code, body: String::new() })
    }

    fn validate(replies: Vec<Result<HttpResponse, NetError>>, jar: &CookieJar) -> (Validation, Arc<Mock>) {
        let mock = Mock::new(replies);
        let v = InnerTubeValidator::new(mock.clone()).validate(jar);
        (v, mock)
    }

    #[test]
    fn logged_in_single_identity_is_valid() {
        let (v, mock) = validate(vec![ok(menu("1")), ok(list(1))], &jar_with(AUTH_NAMES));
        let Validation::Valid { identities } = v else { panic!("{v:?}") };
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].name, "Channel 0");
        assert_eq!(identities[0].page_id.as_deref(), Some("PAGEID0"));
        // Authenticated call: cookie + SAPISIDHASH family sent to the right endpoints only.
        let seen = mock.seen.lock().unwrap();
        assert!(seen[0].0.contains("/youtubei/v1/account/account_menu"));
        assert!(seen[1].0.contains("/youtubei/v1/account/accounts_list"));
        let auth = seen[0].1.iter().find(|(n, _)| n == "Authorization").unwrap().1.clone();
        assert!(auth.starts_with("SAPISIDHASH ") && auth.contains(" SAPISID1PHASH ") && auth.contains(" SAPISID3PHASH "));
    }

    #[test]
    fn several_channels_are_returned_for_an_explicit_choice() {
        let (v, _) = validate(vec![ok(menu("1")), ok(list(3))], &jar_with(AUTH_NAMES));
        let Validation::Valid { identities } = v else { panic!() };
        assert_eq!(identities.len(), 3, "never collapsed to one by guessing");
        assert_eq!(
            identities.iter().map(Identity::key).collect::<Vec<_>>(),
            vec!["PAGEID0", "PAGEID1", "PAGEID2"]
        );
    }

    #[test]
    fn a_failing_channel_list_does_not_invalidate_the_session() {
        let (v, _) = validate(vec![ok(menu("1")), status(500)], &jar_with(AUTH_NAMES));
        let Validation::Valid { identities } = v else { panic!("{v:?}") };
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].name, "Test Person");
        assert!(identities[0].page_id.is_none());
    }

    #[test]
    fn logged_out_response_and_auth_errors_are_expired() {
        let jar = jar_with(AUTH_NAMES);
        assert_eq!(validate(vec![ok(menu("0"))], &jar).0, Validation::Expired);
        assert_eq!(validate(vec![status(401)], &jar).0, Validation::Expired);
        assert_eq!(validate(vec![status(403)], &jar).0, Validation::Expired);
    }

    #[test]
    fn missing_auth_cookies_cannot_validate_and_make_no_request() {
        let (v, mock) = validate(vec![], &jar_with(&["SID", "PREF"]));
        assert_eq!(v, Validation::Expired);
        assert!(mock.seen.lock().unwrap().is_empty());
    }

    // NET-02 / AUTH-04: a network or server problem is not a logout.
    #[test]
    fn transport_and_server_errors_are_unreachable_not_expired() {
        let jar = jar_with(AUTH_NAMES);
        assert_eq!(validate(vec![Err(NetError::Timeout)], &jar).0, Validation::Unreachable);
        assert_eq!(validate(vec![status(503)], &jar).0, Validation::Unreachable);
        assert_eq!(validate(vec![status(429)], &jar).0, Validation::Unreachable);
    }

    #[test]
    fn unexpected_shapes_are_unrecognized_never_valid() {
        let jar = jar_with(AUTH_NAMES);
        assert_eq!(validate(vec![ok("not json".into())], &jar).0, Validation::Unrecognized);
        assert_eq!(validate(vec![ok("{}".into())], &jar).0, Validation::Unrecognized);
        // Logged in but nothing to bind an identity to.
        let no_id = serde_json::json!({"responseContext": {"serviceTrackingParams": [
            {"params": [{"key": "logged_in", "value": "1"}]}]}})
        .to_string();
        assert_eq!(validate(vec![ok(no_id), status(500)], &jar).0, Validation::Unrecognized);
    }

    #[test]
    fn debug_output_hides_identity_and_values() {
        let id = Identity { name: "Real Name".into(), handle: Some("@h".into()), avatar_url: None, page_id: Some("P".into()), selected_by_service: true };
        assert!(!format!("{id:?} {:?}", Validation::Valid { identities: vec![id.clone()] }).contains("Real Name"));
    }

    // Found with the real Firefox profile: SAPISID etc. exist under .google.com AND .youtube.com.
    // Sending both (17 KB, duplicate names) made the service answer "logged out".
    #[test]
    fn only_cookies_matching_the_request_host_are_sent() {
        let mut jar = jar_with(&["SAPISID", "SID"]);
        for (name, domain, value) in [
            ("SAPISID", ".google.com", "GOOGLE-DOMAIN-VALUE"),
            ("SID", ".google.com", "GOOGLE-DOMAIN-SID"),
            ("OTHER", ".example.com", "NOT-FOR-YOUTUBE"),
            ("LOOKALIKE", ".notyoutube.com", "NOT-FOR-YOUTUBE"),
        ] {
            jar.cookies.push(Cookie {
                name: name.into(),
                value: value.into(),
                domain: domain.into(),
                path: "/".into(),
                secure: true,
                http_only: true,
                expires_unix: Some(4_102_444_800),
            });
        }
        jar.cookies.push(Cookie {
            name: "EXPIRED".into(),
            value: "OLD".into(),
            domain: ".youtube.com".into(),
            path: "/".into(),
            secure: true,
            http_only: false,
            expires_unix: Some(1_000),
        });
        let header = cookie_header_for(&jar, "music.youtube.com", 1_800_000_000);
        assert!(header.contains("SAPISID=VALUECANARY-SAPISID"), "{header}");
        assert!(header.contains("SID=VALUECANARY-SID"), "{header}");
        for bad in ["GOOGLE-DOMAIN", "NOT-FOR-YOUTUBE", "OLD"] {
            assert!(!header.contains(bad), "{bad} must not be sent: {header}");
        }
        assert_eq!(header.matches("SAPISID=").count(), 1, "no duplicate names: {header}");
        // And the Authorization hash is computed from the youtube.com SAPISID, not google.com's.
        let m = matching_jar(&jar, "music.youtube.com");
        assert!(m.cookies.iter().all(|c| c.domain != ".google.com"));
        assert_eq!(domain_matches(".youtube.com", "music.youtube.com"), true);
        assert_eq!(domain_matches("youtube.com", "music.youtube.com"), true);
        assert_eq!(domain_matches("music.youtube.com", "music.youtube.com"), true);
        assert_eq!(domain_matches(".google.com", "music.youtube.com"), false);
        assert_eq!(domain_matches(".youtube.com", "evilyoutube.com"), false);
    }

    #[test]
    fn authorization_hash_is_deterministic_and_value_dependent() {
        let a = authorization_header(&jar_with(&["SAPISID"]), 1000).unwrap();
        assert!(a.starts_with("SAPISIDHASH 1000_"));
        assert_eq!(a, authorization_header(&jar_with(&["SAPISID"]), 1000).unwrap());
        assert_ne!(a, authorization_header(&jar_with(&["SAPISID"]), 1001).unwrap());
        assert!(authorization_header(&jar_with(&["SID"]), 1000).is_none());
        // No cookie value appears in the header.
        assert!(!a.contains("VALUECANARY"));
    }

    #[test]
    fn avatar_is_the_largest_https_thumbnail_on_a_google_image_host() {
        let item = serde_json::json!({"accountPhoto": {"thumbnails": [
            {"url": "https://lh3.googleusercontent.com/a=s48", "width": 48},
            {"url": "https://yt3.ggpht.com/a=s176", "width": 176},
            {"url": "https://evil.example/huge", "width": 999},
        ]}});
        assert_eq!(avatar_url(&item).as_deref(), Some("https://yt3.ggpht.com/a=s176"));
    }

    #[test]
    fn avatar_urls_that_could_point_elsewhere_are_ignored() {
        for url in [
            "http://lh3.googleusercontent.com/a",
            "https://googleusercontent.com/a",
            "https://lh3.googleusercontent.com@evil.example/a",
            "https://lh3.googleusercontent.com:8443/a",
            "https://evil.example/.googleusercontent.com/a",
            "https://evilgoogleusercontent.com/a",
            "data:image/png;base64,AAAA",
        ] {
            let item = serde_json::json!({"accountPhoto": {"thumbnails": [{"url": url, "width": 10}]}});
            assert_eq!(avatar_url(&item), None, "{url}");
        }
        assert_eq!(avatar_url(&serde_json::json!({})), None);
    }
}
