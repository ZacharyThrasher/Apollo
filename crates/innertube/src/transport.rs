//! HTTP transport. Pure: no UI, webview, mpv or OS access.
//!
//! Vendored from Limusic and trimmed (see NOTICE.md). Differences that matter:
//! no session "healer" (a 401/403 is surfaced as [`Error::SessionExpired`] and the account layer
//! decides what it means), no blocklist or hide-video filters, no upload calls, play reporting only through
//! [`InnerTube::send_tracking_ping`] (https YouTube hosts only, one attempt), and `Session` never
//! prints its cookie.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use serde::Serialize;
use sha1::{Digest, Sha1};
use tokio::sync::Notify;

use crate::clients::YouTubeClient;
use crate::models::context::Locale;

pub const BASE_URL: &str = "https://music.youtube.com/youtubei/v1/";
pub const ORIGIN: &str = "https://music.youtube.com";
pub const REFERER: &str = "https://music.youtube.com/";
pub const SW_JS_DATA_URL: &str = "https://music.youtube.com/sw.js_data";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("visitorData not found in sw.js_data")]
    VisitorDataNotFound,
    /// YouTube refused the stored credentials (401/403, or the logged-out browse state).
    #[error("session rejected by YouTube Music")]
    SessionExpired,
    /// The service answered normally and said no (an edit it did not accept). The request was
    /// understood and did not take effect, unlike a timeout, whose outcome is unknown.
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Other(String),
}

/// Session state, set once at startup / login.
#[derive(Clone, Default)]
pub struct Session {
    pub locale: Locale,
    pub visitor_data: Option<String>,
    pub data_sync_id: Option<String>,
    /// Full cookie string. Present means authenticated requests are possible.
    pub cookie: Option<String>,
}

// Never derive: the cookie is a credential and `{:?}` ends up in logs and panics.
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("locale", &self.locale)
            .field("visitor_data", &self.visitor_data.as_ref().map(|_| "<set>"))
            .field("data_sync_id", &self.data_sync_id.as_ref().map(|_| "<set>"))
            .field("cookie", &self.cookie.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Session {
    /// Pull the `SAPISID` value out of the cookie string, if present.
    fn sapisid(&self) -> Option<String> {
        self.cookie.as_deref().and_then(cookie_sapisid).map(str::to_owned)
    }
}

/// Extract the `SAPISID` (or its modern `__Secure-3PAPISID` alias) value from a Cookie header
/// string. Public so the login flow (context/15) can validate a pasted cookie before setting it.
pub fn cookie_sapisid(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        matches!(k.trim(), "SAPISID" | "__Secure-3PAPISID").then(|| v.trim())
    })
}

/// Apply `Set-Cookie` response values to a `Cookie` request header, returning the new header only
/// when something actually changed. Existing names keep their position; new ones go on the end.
///
/// No domain/path matching: every response this is fed comes from music.youtube.com and the jar
/// only ever goes back there, so a `Domain=` a browser would reject cannot reach us.
///
/// ponytail: deletions (`NAME=;`) are ignored rather than applied. A cookie Google wants gone is
/// dead server-side anyway, so carrying it costs nothing, while honouring the deletion would let
/// one odd response drop `SAPISID` and silently sign the user out.
pub(crate) fn merge_set_cookie(cookie: &str, set_cookie: &[&str]) -> Option<String> {
    let mut jar: Vec<(String, String)> = cookie
        .split(';')
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut changed = false;
    for line in set_cookie {
        let pair = line.split(';').next().unwrap_or_default();
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let (name, value) = (name.trim(), value.trim());
        if name.is_empty() || value.is_empty() {
            continue;
        }
        match jar.iter_mut().find(|(n, _)| n == name) {
            Some(entry) if entry.1 == value => {}
            Some(entry) => {
                entry.1 = value.to_owned();
                changed = true;
            }
            None => {
                jar.push((name.to_owned(), value.to_owned()));
                changed = true;
            }
        }
    }
    changed.then(|| jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; "))
}

/// The transport client. One shared `reqwest::Client`; proxy must be set before the first request
/// or reqwest snapshots it as none.
///
/// `session` is behind a shared lock so a runtime login is visible through every clone. Reads and
/// writes are quick and never held across an `.await`, so a std `RwLock` is right.
#[derive(Clone)]
pub struct InnerTube {
    http: reqwest::Client,
    session: Arc<RwLock<Session>>,
    /// Pinged whenever a signed-in request comes back rejected (401/403, or a 200 carrying the
    /// logged-out browse state). `notify_one` stores a permit, so a listener that is busy still
    /// sees the next rejection.
    session_rejected: Arc<Notify>,
    /// Pinged when a response's `Set-Cookie` actually changed the stored jar, so the account layer
    /// can write the rotated cookie back to the vault. See [`InnerTube::absorb_cookies`].
    cookie_changed: Arc<Notify>,
}

impl InnerTube {
    pub fn new(session: Session, proxy: Option<&str>) -> Result<Self, Error> {
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .pool_idle_timeout(Duration::from_secs(300))
            .pool_max_idle_per_host(10);
        if let Some(p) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(p)?);
        }
        Ok(InnerTube {
            http: builder.build()?,
            session: Arc::new(RwLock::new(session)),
            session_rejected: Arc::new(Notify::new()),
            cookie_changed: Arc::new(Notify::new()),
        })
    }

    /// Signal raised when YouTube rejects the signed-in session. See the field.
    pub fn session_rejected(&self) -> Arc<Notify> {
        self.session_rejected.clone()
    }

    /// Signal raised when the stored cookie jar was updated from a response. See the field.
    pub fn cookie_changed(&self) -> Arc<Notify> {
        self.cookie_changed.clone()
    }

    /// The session no longer authenticates. Both callers (transport 401/403 and the logged-out
    /// browse payload) route through here so the listener hears about either one.
    pub(crate) fn reject_session(&self) -> Error {
        self.session_rejected.notify_one();
        Error::SessionExpired
    }

    /// Merge a response's `Set-Cookie` into the stored jar.
    ///
    /// Google rotates `__Secure-1PSIDTS` / `__Secure-3PSIDTS` on the very requests this client
    /// makes, and invalidates the previous value when it does. Dropping the new one (which is
    /// what a `reqwest` client with no cookie store does) is what killed the login a few hours
    /// into every session — issue #165 / KI-2. Note that `cookie_store(true)` would not help:
    /// reqwest skips its own store whenever a `Cookie` header is already set, and `headers()`
    /// always sets one.
    fn absorb_cookies(&self, headers: &HeaderMap) {
        if headers.get(SET_COOKIE).is_none() {
            return;
        }
        let set_cookie: Vec<&str> =
            headers.get_all(SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).collect();
        {
            let mut s = self.session.write().unwrap();
            let Some(merged) = s.cookie.as_deref().and_then(|c| merge_set_cookie(c, &set_cookie))
            else {
                return;
            };
            s.cookie = Some(merged);
        }
        // Names only, never values: this line ends up in the log a user attaches to a bug report,
        // and it is the one piece of evidence that says whether rotation is being kept.
        let names: Vec<&str> = set_cookie
            .iter()
            .filter_map(|line| line.split(';').next()?.split_once('='))
            .map(|(name, _)| name.trim())
            .collect();
        tracing::debug!(cookies = ?names, "kept rotated cookies");
        self.cookie_changed.notify_one();
    }
    // --- session accessors (context/15) -----------------------------------------------------

    /// True when a login cookie is set.
    pub fn is_logged_in(&self) -> bool {
        self.session.read().unwrap().cookie.is_some()
    }

    /// The current visitorData (read fresh per resolve — a login may have refreshed it).
    pub fn visitor_data(&self) -> Option<String> {
        self.session.read().unwrap().visitor_data.clone()
    }

    /// The current cookie header, if logged in (for the stream-validation HEAD request).
    pub fn cookie(&self) -> Option<String> {
        self.session.read().unwrap().cookie.clone()
    }

    pub fn set_cookie(&self, cookie: Option<String>) {
        self.session.write().unwrap().cookie = cookie;
    }

    pub fn set_data_sync_id(&self, id: Option<String>) {
        self.session.write().unwrap().data_sync_id = id;
    }

    pub fn data_sync_id(&self) -> Option<String> {
        self.session.read().unwrap().data_sync_id.clone()
    }

    pub fn set_visitor_data(&self, vd: Option<String>) {
        self.session.write().unwrap().visitor_data = vd;
    }

    /// The language YouTube answers in (`hl`), from the UI's own language setting.
    ///
    /// Home shelf titles, mood chips, playlist subtitles and auto-playlist names are YouTube's
    /// text, not ours, so a Korean UI left on `hl=en` reads half English (#274). `gl` stays where
    /// it is: that picks *which* feed comes back, not the language of its labels, and a user who
    /// switches the interface to Korean has not asked for a different catalogue.
    ///
    /// Ignores anything that is not tag-shaped. The value comes from the webview and ends up in a
    /// request header, and a locale we do not recognize is better dropped than sent.
    pub fn set_locale(&self, hl: &str) {
        if hl.is_empty() || !hl.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            tracing::warn!(hl, "ignoring an unusable locale");
            return;
        }
        self.session.write().unwrap().locale.hl = hl.to_owned();
    }

    /// Build the request `context` for a client from the current session. Crate-internal — the
    /// endpoints facade calls it. Reads and drops the lock synchronously (no `.await` inside).
    pub(crate) fn context_for(&self, client: &YouTubeClient) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        // `onBehalfOfUser` makes Google *require* a credential: with no cookie it turns a request
        // that would have worked anonymously into a hard 401. Only send it when we can authenticate.
        let dsid = s.cookie.as_ref().and(s.data_sync_id.as_deref());
        client.to_context(&s.locale, s.visitor_data.as_deref(), dsid)
    }

    /// A context with no account attached, for requests that must not be attributed to the user:
    /// no `onBehalfOfUser`, and the caller passes `set_login = false` so no cookie goes with it.
    pub(crate) fn context_anonymous(
        &self,
        client: &YouTubeClient,
    ) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        client.to_context(&s.locale, s.visitor_data.as_deref(), None)
    }

    /// Build a one-off authenticated context for identity validation without changing the shared
    /// session seen by concurrent browse/playback requests. The caller commits the id only after
    /// the validation response succeeds.
    pub(crate) fn context_for_identity(
        &self,
        client: &YouTubeClient,
        data_sync_id: &str,
    ) -> crate::models::context::Context {
        let s = self.session.read().unwrap();
        let dsid = s.cookie.as_ref().map(|_| data_sync_id);
        client.to_context(&s.locale, s.visitor_data.as_deref(), dsid)
    }
    /// POST a JSON body to an InnerTube endpoint with this client's headers, retrying transient
    /// network errors (3 attempts, 500ms x 2 backoff).
    pub async fn post<B: Serialize>(
        &self,
        path: &str,
        client: &YouTubeClient,
        body: &B,
        set_login: bool,
    ) -> Result<serde_json::Value, Error> {
        // `path` may already carry query params (e.g. browse continuations); chain accordingly.
        let sep = if path.contains('?') { '&' } else { '?' };
        let url = format!("{BASE_URL}{path}{sep}prettyPrint=false");
        let body = serde_json::to_vec(body)?;

        let mut delay = Duration::from_millis(500);
        let mut attempt = 0;

        loop {
            attempt += 1;
            // Rebuild headers on every iteration so retried requests use the updated cookie.
            let headers = self.headers(client, set_login);
            let res = self
                .http
                .post(&url)
                .headers(headers)
                .body(body.clone())
                .send()
                .await
                .and_then(|r| r.error_for_status());

            match res {
                Ok(resp) => {
                    self.absorb_cookies(resp.headers());
                    return Ok(resp.json().await?);
                }
                // Retry only on connect/timeout (transient).
                Err(e) if attempt < 3 && (e.is_timeout() || e.is_connect() || e.is_request()) => {
                    tracing::warn!(attempt, error = %e.without_url(), "retrying InnerTube POST {path}");
                    tokio::time::sleep(delay).await;
                    delay *= 2;
                }
                // Signed in and Google says "no credential" (401) or "not for you" (403): the
                // stored cookie is dead. Only for a request that actually carried the cookie: a
                // deliberately anonymous one is refused for its own reasons and says nothing
                // about the session. The caller decides what a rejection means (an outage or a
                // 5xx is *not* a rejection and never lands here).
                Err(e)
                    if set_login
                        && client.login_supported
                        && self.is_logged_in()
                        && e.status().is_some_and(|s| s == 401 || s == 403) =>
                {
                    tracing::warn!(status = ?e.status(), "InnerTube {path} rejected the session");
                    return Err(self.reject_session());
                }
                Err(e) => return Err(e.without_url().into()),
            }
        }
    }

    /// Per-request headers. context/01 §ytClient. Note `X-YouTube-Client-Name` carries the
    /// numeric client **id**, not the name string — intentional and required.
    fn headers(&self, client: &YouTubeClient, set_login: bool) -> HeaderMap {
        let mut h = HeaderMap::new();
        let set = |h: &mut HeaderMap, k: &'static str, v: &str| {
            if let Ok(val) = HeaderValue::from_str(v) {
                h.insert(HeaderName::from_static(k), val);
            }
        };
        set(&mut h, "content-type", "application/json");
        set(&mut h, "accept", "application/json");
        set(&mut h, "x-goog-api-format-version", "1");
        set(&mut h, "x-youtube-client-name", &client.client_id);
        set(&mut h, "x-youtube-client-version", &client.client_version);
        set(&mut h, "x-origin", ORIGIN);
        set(&mut h, "referer", REFERER);
        set(&mut h, "user-agent", &client.user_agent);

        let s = self.session.read().unwrap();
        set(&mut h, "accept-language", &s.locale.accept_language());
        if let Some(vd) = &s.visitor_data {
            set(&mut h, "x-goog-visitor-id", vd);
        }

        // SAPISIDHASH cookie auth — only when logged in AND the client supports it (Phase 3).
        if set_login && client.login_supported {
            if let Some(cookie) = &s.cookie {
                set(&mut h, "cookie", cookie);
                if let Some(sapisid) = s.sapisid() {
                    if let Ok(val) = HeaderValue::from_str(&sapisid_hash(&sapisid, ORIGIN)) {
                        h.insert(HeaderName::from_static("authorization"), val);
                    }
                }
            }
        }
        h
    }
    /// Send one play-reporting ping (a GET to a `playbackTracking` URL) as the signed-in account.
    ///
    /// The URL must be an https YouTube URL ([`crate::tracking::checked_url`]) because the
    /// request carries the cookie. Unlike [`post`](Self::post): one attempt (a ping is never
    /// retried), a short timeout so a hung host cannot hold the caller, and the response is
    /// neither a verdict on the session (a refused ping does not sign anyone out) nor a source
    /// of cookies (`s.youtube.com` sets its own, which do not belong in the account's jar).
    pub(crate) async fn send_tracking_ping(
        &self,
        client: &YouTubeClient,
        url: &str,
    ) -> Result<(), Error> {
        let url = crate::tracking::checked_url(url)?;
        let mut headers = self.headers(client, true);
        headers.remove("content-type");
        headers.remove("x-goog-api-format-version");
        headers.insert(HeaderName::from_static("accept"), HeaderValue::from_static("*/*"));
        self.http
            .get(url)
            .headers(headers)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| Error::Http(e.without_url()))?;
        Ok(())
    }

    /// Bootstrap `visitorData` anonymously by scraping `sw.js_data`. context/04 §A.
    pub async fn fetch_visitor_data(&self) -> Result<String, Error> {
        let text = self.http.get(SW_JS_DATA_URL).send().await?.error_for_status()?.text().await?;
        parse_visitor_data(&text)
    }
}

/// `Authorization: SAPISIDHASH <epoch>_<sha1(epoch SAPISID origin)>`. context/01.
pub fn sapisid_hash(sapisid: &str, origin: &str) -> String {
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("SAPISIDHASH {epoch}_{}", sha1_hex(&format!("{epoch} {sapisid} {origin}")))
}

fn sha1_hex(input: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

/// The `sw.js_data` body starts with a 4–5 char junk prefix (`)]}'`); strip it, parse JSON,
/// and find the element matching `^Cg[ts]` in `[0][2]`. context/04 §A.
fn parse_visitor_data(body: &str) -> Result<String, Error> {
    // Drop everything up to and including the first newline or the `)]}'` guard.
    let json_start = body.find('[').ok_or(Error::VisitorDataNotFound)?;
    let value: serde_json::Value = serde_json::from_str(&body[json_start..])?;
    let arr = value
        .get(0)
        .and_then(|v| v.get(2))
        .and_then(|v| v.as_array())
        .ok_or(Error::VisitorDataNotFound)?;
    arr.iter()
        .filter_map(|v| v.as_str())
        .find(|s| s.starts_with("Cgt") || s.starts_with("Cgs"))
        .map(str::to_owned)
        .ok_or(Error::VisitorDataNotFound)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_known_vector() {
        // SHA1("abc") = a9993e364706816aba3e25717850c26c9cd0d89d
        assert_eq!(sha1_hex("abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }

    #[test]
    fn sapisid_hash_shape() {
        let h = sapisid_hash("MYSAPISID", ORIGIN);
        assert!(h.starts_with("SAPISIDHASH "));
        let rest = &h["SAPISIDHASH ".len()..];
        let (epoch, hash) = rest.split_once('_').unwrap();
        assert!(epoch.parse::<u64>().is_ok());
        assert_eq!(hash.len(), 40); // sha1 hex
    }

    #[test]
    fn parse_visitor_data_from_blob() {
        // Shape of sw.js_data: outer array; [0][2] holds the visitorData among other strings.
        let blob = r#")]}'
[["wrs","x",["junk","CgtABCDEFG1234567%3D%3D","more"]],null]"#;
        assert_eq!(parse_visitor_data(blob).unwrap(), "CgtABCDEFG1234567%3D%3D");
    }

    #[test]
    fn on_behalf_of_user_needs_a_cookie() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session { data_sync_id: Some("abc123".into()), ..Default::default() };

        let it = InnerTube::new(session, None).unwrap();
        assert_eq!(it.context_for(web).user.on_behalf_of_user, None, "no cookie ⇒ no obo (401)");

        it.set_cookie(Some("SAPISID=secret".into()));
        assert_eq!(it.context_for(web).user.on_behalf_of_user.as_deref(), Some("abc123"));
    }

    #[test]
    fn anonymous_context_carries_no_account() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session {
            cookie: Some("SAPISID=secret".into()),
            data_sync_id: Some("abc123".into()),
            visitor_data: Some("visitor".into()),
            ..Default::default()
        };
        let it = InnerTube::new(session, None).unwrap();

        // Signed in, yet `search` must stay unattributable (#203).
        let ctx = it.context_anonymous(web);
        assert_eq!(ctx.user.on_behalf_of_user, None);
        assert_eq!(ctx.client.visitor_data.as_deref(), Some("visitor"), "still a session, no user");
    }

    #[test]
    fn identity_validation_context_does_not_mutate_the_committed_session() {
        let clients = crate::clients::Clients::bundled();
        let web = clients.get(crate::clients::METADATA_CLIENT).unwrap();
        let session = Session {
            cookie: Some("SAPISID=secret".into()),
            data_sync_id: Some("committed-id".into()),
            ..Default::default()
        };
        let it = InnerTube::new(session, None).unwrap();

        assert_eq!(
            it.context_for_identity(web, "candidate-id").user.on_behalf_of_user.as_deref(),
            Some("candidate-id")
        );
        assert_eq!(it.context_for(web).user.on_behalf_of_user.as_deref(), Some("committed-id"));
    }

    #[test]
    fn sapisid_extracted_from_cookie() {
        let s = Session {
            cookie: Some("FOO=bar; SAPISID=secret123; OTHER=x".into()),
            ..Default::default()
        };
        assert_eq!(s.sapisid().as_deref(), Some("secret123"));
    }

    // The #165 regression: the rotated value has to land in the jar, in place, or the login dies
    // a few hours in.
    #[test]
    fn a_rotated_cookie_replaces_the_stored_one() {
        let merged = merge_set_cookie(
            "SAPISID=keep; __Secure-3PSIDTS=old; PREF=x",
            &["__Secure-3PSIDTS=new; Path=/; Secure; HttpOnly", "YSC=fresh; Path=/"],
        );
        assert_eq!(
            merged.as_deref(),
            Some("SAPISID=keep; __Secure-3PSIDTS=new; PREF=x; YSC=fresh")
        );
    }

    #[test]
    fn nothing_new_means_no_rewrite_and_no_deletions_applied() {
        // Same values back, plus a deletion we deliberately ignore: no change, so no disk write
        // and no chance of dropping the login.
        assert_eq!(
            merge_set_cookie("SAPISID=keep; PREF=x", &["PREF=x; Path=/", "SAPISID=; Max-Age=0"]),
            None
        );
    }
}
