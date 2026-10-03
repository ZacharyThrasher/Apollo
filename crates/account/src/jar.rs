//! The canonical cookie jar. Values are secrets: `Debug` never prints them, memory is zeroed on
//! drop, and the only way out is [`CookieJar::to_netscape`] (for the contained resolver) and the
//! encrypted store.

use std::fmt;

use apollo_services_common::SecretString;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Domains whose cookies we keep: the YouTube / Google account scope (AUTH-03: only what the
/// session needs, never a whole browser's cookies).
const KEEP_DOMAINS: &[&str] = &["youtube.com", "google.com"];

#[derive(Clone, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// Unix seconds. `None` = session cookie. Never invented: a header seed stays a session seed.
    pub expires_unix: Option<i64>,
}

impl Drop for Cookie {
    fn drop(&mut self) {
        self.value.zeroize();
    }
}

impl fmt::Debug for Cookie {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cookie")
            .field("name", &self.name)
            .field("domain", &self.domain)
            .field("value", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl Cookie {
    fn key(&self) -> (&str, &str, &str) {
        (&self.name, &self.domain, &self.path)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct CookieJar {
    pub cookies: Vec<Cookie>,
    /// Built from a bare `Cookie:` header: no attributes, session-only, a seed rather than a
    /// faithful copy. Shown as such to the owner.
    #[serde(default)]
    pub ephemeral_seed: bool,
}

impl fmt::Debug for CookieJar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CookieJar")
            .field("cookies", &self.cookies.len())
            .field("ephemeral_seed", &self.ephemeral_seed)
            .finish()
    }
}

fn domain_in_scope(domain: &str) -> bool {
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    KEEP_DOMAINS
        .iter()
        .any(|k| d == *k || d.ends_with(&format!(".{k}")))
}

impl CookieJar {
    pub fn len(&self) -> usize {
        self.cookies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    pub fn names(&self) -> Vec<&str> {
        self.cookies.iter().map(|c| c.name.as_str()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&Cookie> {
        self.cookies.iter().find(|c| c.name == name)
    }

    /// Keep only YouTube/Google account scope, drop duplicates (last wins).
    pub fn scoped(mut self) -> Self {
        let taken = std::mem::take(&mut self.cookies);
        let mut kept: Vec<Cookie> = Vec::new();
        for c in taken.iter().filter(|c| domain_in_scope(&c.domain)) {
            if let Some(i) = kept.iter().position(|k| k.key() == c.key()) {
                kept[i] = c.clone();
            } else {
                kept.push(c.clone());
            }
        }
        self.cookies = kept;
        self
    }

    /// Has what an authenticated InnerTube/yt-dlp call needs. Presence only: it says nothing
    /// about whether the session is still valid (only the service can tell).
    pub fn has_auth_cookies(&self) -> bool {
        let any = |names: &[&str]| names.iter().any(|n| self.get(n).is_some());
        any(&["SAPISID", "__Secure-3PAPISID"]) && any(&["SID", "__Secure-3PSID"])
    }

    /// Netscape cookie-file text, as yt-dlp reads it. HttpOnly cookies use the `#HttpOnly_`
    /// prefix; session cookies get expiry 0 (no fabricated lifetime).
    pub fn to_netscape(&self) -> SecretString {
        let mut out = String::from("# Netscape HTTP Cookie File\n");
        for c in &self.cookies {
            let include_sub = if c.domain.starts_with('.') { "TRUE" } else { "FALSE" };
            let prefix = if c.http_only { "#HttpOnly_" } else { "" };
            out.push_str(&format!(
                "{prefix}{}\t{include_sub}\t{}\t{}\t{}\t{}\t{}\n",
                c.domain,
                if c.path.is_empty() { "/" } else { &c.path },
                if c.secure { "TRUE" } else { "FALSE" },
                c.expires_unix.unwrap_or(0).max(0),
                c.name,
                c.value
            ));
        }
        SecretString::new(out)
    }

    /// Parse Netscape text (as exported by browsers/extensions/yt-dlp). Unparseable lines are
    /// skipped, never echoed.
    pub fn from_netscape(text: &str) -> Self {
        let mut cookies = Vec::new();
        for raw in text.lines() {
            let (line, http_only) = match raw.strip_prefix("#HttpOnly_") {
                Some(rest) => (rest, true),
                None => (raw, false),
            };
            if line.trim().is_empty() || line.starts_with('#') {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 7 {
                continue;
            }
            let expires: i64 = f[4].trim().parse().unwrap_or(0);
            cookies.push(Cookie {
                domain: f[0].to_string(),
                path: f[2].to_string(),
                secure: f[3].eq_ignore_ascii_case("TRUE"),
                http_only,
                expires_unix: (expires > 0).then_some(expires),
                name: f[5].to_string(),
                value: f[6].trim_end_matches('\r').to_string(),
            });
        }
        CookieJar {
            cookies,
            ephemeral_seed: false,
        }
        .scoped()
    }

    /// A bare `Cookie:` header (`name=value; name2=value2`). No domain/path/expiry survive, so
    /// every cookie becomes a session cookie for `.youtube.com` and the jar is flagged as a seed.
    pub fn from_cookie_header(header: &str) -> Self {
        let header = header
            .trim()
            .strip_prefix("Cookie:")
            .or_else(|| header.trim().strip_prefix("cookie:"))
            .unwrap_or(header)
            .trim();
        let cookies = header
            .split(';')
            .filter_map(|p| p.split_once('='))
            .filter(|(n, _)| !n.trim().is_empty())
            .map(|(n, v)| Cookie {
                name: n.trim().to_string(),
                value: v.trim().to_string(),
                domain: ".youtube.com".into(),
                path: "/".into(),
                secure: true,
                http_only: false,
                expires_unix: None,
            })
            .collect();
        CookieJar {
            cookies,
            ephemeral_seed: true,
        }
    }

    /// Merge cookies a helper rewrote (rotation). Only in-scope cookies already known by
    /// (name, domain, path) are updated; new in-scope ones are added; nothing is ever removed
    /// (a helper that dropped a cookie must not log us out).
    pub fn merge_rotation(&mut self, updated: &CookieJar) {
        for u in updated.cookies.iter().filter(|c| domain_in_scope(&c.domain)) {
            match self.cookies.iter_mut().find(|c| c.key() == u.key()) {
                Some(existing) => {
                    existing.value = u.value.clone();
                    existing.expires_unix = u.expires_unix.or(existing.expires_unix);
                    existing.secure = u.secure;
                    existing.http_only = u.http_only;
                }
                None => self.cookies.push(u.clone()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, domain: &str, value: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires_unix: Some(4_102_444_800),
        }
    }

    #[test]
    fn debug_never_shows_values() {
        let jar = CookieJar {
            cookies: vec![c("SAPISID", ".youtube.com", "VALUECANARY")],
            ephemeral_seed: false,
        };
        let shown = format!("{jar:?} {:?} {:#?}", jar.cookies[0], jar);
        assert!(!shown.contains("VALUECANARY"), "{shown}");
        assert!(shown.contains("SAPISID"));
    }

    #[test]
    fn scope_keeps_only_youtube_and_google_and_dedupes() {
        let jar = CookieJar {
            cookies: vec![
                c("SID", ".google.com", "a"),
                c("SAPISID", ".youtube.com", "b"),
                c("x", ".evil-youtube.com", "no"),
                c("y", ".example.com", "no"),
                c("z", "youtube.com.evil.net", "no"),
                c("SID", ".google.com", "a2"),
                c("LOGIN_INFO", "music.youtube.com", "c"),
            ],
            ephemeral_seed: false,
        }
        .scoped();
        let names = jar.names();
        assert_eq!(names, vec!["SID", "SAPISID", "LOGIN_INFO"]);
        assert_eq!(jar.get("SID").unwrap().value, "a2", "last duplicate wins");
    }

    #[test]
    fn netscape_roundtrip_preserves_attributes_and_httponly() {
        let mut ck = c("SAPISID", ".youtube.com", "v1");
        ck.http_only = false;
        let jar = CookieJar {
            cookies: vec![ck, c("__Secure-3PSID", ".youtube.com", "v2")],
            ephemeral_seed: false,
        };
        let text = jar.to_netscape();
        assert!(text.expose().contains("#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t4102444800\t__Secure-3PSID\tv2"));
        let back = CookieJar::from_netscape(text.expose());
        assert_eq!(back.len(), 2);
        let s = back.get("__Secure-3PSID").unwrap();
        assert!(s.http_only && s.secure);
        assert_eq!(s.expires_unix, Some(4_102_444_800));
        assert!(!back.get("SAPISID").unwrap().http_only);
    }

    #[test]
    fn session_cookies_get_no_fabricated_expiry() {
        let mut ck = c("SID", ".google.com", "v");
        ck.expires_unix = None;
        let jar = CookieJar { cookies: vec![ck], ephemeral_seed: false };
        assert!(jar.to_netscape().expose().contains("\t0\tSID\t"));
        assert_eq!(CookieJar::from_netscape(jar.to_netscape().expose()).get("SID").unwrap().expires_unix, None);
    }

    #[test]
    fn header_seed_is_flagged_and_session_only() {
        let jar = CookieJar::from_cookie_header("Cookie: SAPISID=abc; __Secure-3PSID=def; SID=ghi; junk");
        assert!(jar.ephemeral_seed);
        assert_eq!(jar.len(), 3);
        assert!(jar.cookies.iter().all(|c| c.expires_unix.is_none()));
        assert!(jar.has_auth_cookies());
    }

    #[test]
    fn auth_cookie_presence_check() {
        assert!(!CookieJar::default().has_auth_cookies());
        let only_sid = CookieJar { cookies: vec![c("SID", ".google.com", "x")], ephemeral_seed: false };
        assert!(!only_sid.has_auth_cookies(), "SID alone cannot build SAPISIDHASH");
    }

    #[test]
    fn rotation_updates_adds_and_never_removes() {
        let mut jar = CookieJar {
            cookies: vec![c("SAPISID", ".youtube.com", "old"), c("SID", ".google.com", "keep")],
            ephemeral_seed: false,
        };
        let updated = CookieJar {
            cookies: vec![
                c("SAPISID", ".youtube.com", "new"),
                c("__Secure-1PSIDTS", ".google.com", "ts"),
                c("evil", ".example.com", "no"),
            ],
            ephemeral_seed: false,
        };
        jar.merge_rotation(&updated);
        assert_eq!(jar.get("SAPISID").unwrap().value, "new");
        assert_eq!(jar.get("SID").unwrap().value, "keep", "a cookie the helper omitted survives");
        assert!(jar.get("__Secure-1PSIDTS").is_some());
        assert!(jar.get("evil").is_none(), "out-of-scope cookies are never imported");
    }
}
