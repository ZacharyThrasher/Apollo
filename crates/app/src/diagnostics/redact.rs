//! Scrubs text before it can leave the machine in a diagnostics report.
//!
//! The report is meant to be pasted into a chat or an issue, so it must never carry anything that
//! acts as a credential or names the person: cookies, auth headers, signed or stream URLs, IP
//! addresses, e-mail addresses, the Windows user name, and anything shaped like an opaque token.
//! [`Redactor::redact`] runs over the *finished* text, so a secret that reaches the report by a
//! route nobody thought of (a log line, a tool's output) is still caught. It errs on the side of
//! removing too much: a diagnostic that lost a path component is a nuisance, a leaked cookie is
//! an account.
//!
//! Replacements are fixed markers (`[redacted]`, `[ip redacted]`, ...) that contain nothing
//! secret-shaped, so redacting twice changes nothing.

use std::net::Ipv6Addr;
use std::str::FromStr;
use std::sync::OnceLock;

use regex::{Captures, Regex};

/// Cookie names that identify or authenticate a Google / YouTube session (and the visitor).
const COOKIE_NAMES: &str = "SAPISID|APISID|SID|HSID|SSID|SIDCC|LOGIN_INFO|VISITOR_INFO1_LIVE|\
    VISITOR_PRIVACY_METADATA|YSC|PREF|CONSENT|SOCS|NID|DEVICE_INFO";

/// Hosts whose URL paths are kept (the query and fragment are always dropped). Everything else
/// keeps only its host.
const PATH_KEPT_HOSTS: [&str; 5] = [
    "youtube.com",
    "music.youtube.com",
    "lrclib.net",
    "github.com",
    "ytimg.com",
];

struct Patterns {
    netscape: Regex,
    header: Regex,
    scheme_token: Regex,
    cookie_secure: Regex,
    cookie_named: Regex,
    json_secret: Regex,
    stream_host: Regex,
    url: Regex,
    query_secret: Regex,
    email: Regex,
    ipv4: Regex,
    ipv6_candidate: Regex,
    user_dir: Regex,
    opaque: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| {
        let re = |s: &str| Regex::new(s).expect("redaction pattern");
        Patterns {
            // A line of a Netscape cookie file (what yt-dlp is given), tab separated.
            netscape: re(r"(?m)^(?:#HttpOnly_)?\.?[A-Za-z0-9.-]+\t(?:TRUE|FALSE)\t[^\t\r\n]*\t(?:TRUE|FALSE)\t\d+\t[^\t\r\n]+\t[^\r\n]*$"),
            // Header lines: the whole value goes.
            header: re(r"(?i)\b(cookie|set-cookie|authorization|proxy-authorization|x-goog-[a-z0-9-]+|x-youtube-[a-z0-9-]+)\s*[:=][ \t]*[^\r\n]*"),
            // Authorization schemes. Bearer and Basic need a token-sized value after them, so the
            // ordinary words "basic" and "bearer" in a message are left alone.
            scheme_token: re(r"(?i)\b(SAPISIDHASH|SAPISID1PHASH|SAPISID3PHASH)\s+[A-Za-z0-9._~+/=_-]+(?:\s+[A-Za-z0-9._~+/=_-]{16,})*|\b(Bearer|Basic)\s+[A-Za-z0-9._~+/=_-]{12,}"),
            cookie_secure: re(r"(?i)\b(__Secure-[A-Za-z0-9_-]+|__Host-[A-Za-z0-9_-]+)\s*=\s*[^;\s,]*"),
            cookie_named: re(&format!(r"\b({COOKIE_NAMES})\s*=\s*[^;\s,&]+")),
            // "access_token": "abc" and friends.
            json_secret: re(r#"(?i)("(?:[a-z_-]*(?:token|secret|password|passwd|cookie|authorization|api_?key|sapisid|visitor_?data|signature)[a-z_-]*)"\s*:\s*)"(?:[^"\\]|\\.)*""#),
            // Stream hosts, with or without a scheme: the whole URL is signed.
            stream_host: re(r"(?i)(?:https?://)?[a-z0-9.-]*(?:googlevideo|googleusercontent|ggpht)\.com\b[^\s<>\x22'\)\]\}]*"),
            url: re(r#"(?i)\b(?:https?|wss?)://[^\s<>"'\)\]\}]+"#),
            // `sig=...&expire=...` pieces that were cut loose from their URL.
            query_secret: re(r"(?i)\b(sig|lsig|signature|sparams|ip|ipbits|pot|potc|access_token|refresh_token|id_token|api_key|apikey|key|token|auth|password|passwd|secret|visitor[_-]?(?:data|id)?)=[^&\s\x22']+"),
            email: re(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+"),
            ipv4: re(r"\b(?:(?:25[0-5]|2[0-4]\d|1?\d?\d)\.){3}(?:25[0-5]|2[0-4]\d|1?\d?\d)\b(?::\d{1,5}\b)?"),
            ipv6_candidate: re(r"[0-9A-Fa-f]*:[0-9A-Fa-f:]*[0-9A-Fa-f:]"),
            user_dir: re(r"(?i)\b([A-Z]:\\Users\\|/Users/|/home/)[^\\/\s\x22']+"),
            // 32 or more characters of a base64 / hex alphabet with a digit and a letter.
            opaque: re(r"[A-Za-z0-9_+=/-]{32,}"),
        }
    })
}

/// A set of strings that are scrubbed wherever they appear (the account's name, the Windows
/// user name, the machine's name), on top of the pattern rules.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    literals: Vec<String>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds strings that must never appear. Anything shorter than three characters is ignored,
    /// since replacing "a" everywhere would wreck the report without protecting anything.
    pub fn with_known<I, S>(mut self, secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for s in secrets {
            let s = s.into();
            let s = s.trim();
            if s.chars().count() >= 3 {
                self.literals.push(s.to_owned());
            }
        }
        // Longest first, so "Alex Smith" goes before "Alex".
        self.literals.sort_by_key(|s| std::cmp::Reverse(s.len()));
        self.literals.dedup();
        self
    }

    pub fn redact(&self, text: &str) -> String {
        let p = patterns();
        let mut out = text.to_owned();
        for lit in &self.literals {
            out = replace_ignore_case(&out, lit, "[redacted]");
        }
        out = p
            .netscape
            .replace_all(&out, "[cookie line redacted]")
            .into_owned();
        out = p.header.replace_all(&out, "$1: [redacted]").into_owned();
        out = p
            .scheme_token
            .replace_all(&out, "$1 [redacted]")
            .into_owned();
        out = p
            .cookie_secure
            .replace_all(&out, "$1=[redacted]")
            .into_owned();
        out = p
            .cookie_named
            .replace_all(&out, "$1=[redacted]")
            .into_owned();
        out = p
            .json_secret
            .replace_all(&out, r#"$1"[redacted]""#)
            .into_owned();
        out = p
            .stream_host
            .replace_all(&out, "[stream URL redacted]")
            .into_owned();
        out = p
            .url
            .replace_all(&out, |c: &Captures| redact_url(&c[0]))
            .into_owned();
        out = p
            .query_secret
            .replace_all(&out, "$1=[redacted]")
            .into_owned();
        out = p.email.replace_all(&out, "[email redacted]").into_owned();
        out = p.ipv4.replace_all(&out, "[ip redacted]").into_owned();
        out = p.ipv6_candidate.replace_all(&out, redact_ipv6).into_owned();
        out = p.user_dir.replace_all(&out, "${1}[user]").into_owned();
        out = p.opaque.replace_all(&out, redact_opaque).into_owned();
        out
    }
}

/// `https://music.youtube.com/youtubei/v1/browse?key=abc#x` keeps its host and path and loses
/// the rest; a host not on the list keeps only itself.
fn redact_url(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    let had_query = url.contains('?') || url.contains('#');
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    // user:password@host
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = host_port
        .split(':')
        .next()
        .unwrap_or(host_port)
        .to_ascii_lowercase();
    let kept = PATH_KEPT_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")));
    let mut out = format!("{scheme}://{host_port}");
    if kept {
        out.push_str(path);
    } else if !path.is_empty() && path != "/" {
        out.push_str("/[path redacted]");
    }
    if had_query {
        out.push_str("?[query redacted]");
    }
    out
}

/// The candidate is replaced only when it really parses as an IPv6 address, so a clock time
/// ("08:12:33") or a MAC-like run is left alone.
fn redact_ipv6(c: &Captures) -> String {
    let text = &c[0];
    if text.matches(':').count() < 2 {
        return text.to_owned();
    }
    if Ipv6Addr::from_str(text).is_ok() {
        return "[ip redacted]".to_owned();
    }
    // A candidate can carry one colon of surrounding prose ("address:2001:db8::1").
    if let Some(stripped) = text.strip_prefix(':').filter(|_| !text.starts_with("::")) {
        if Ipv6Addr::from_str(stripped).is_ok() {
            return ":[ip redacted]".to_owned();
        }
    }
    if let Some(stripped) = text.strip_suffix(':').filter(|_| !text.ends_with("::")) {
        if Ipv6Addr::from_str(stripped).is_ok() {
            return "[ip redacted]:".to_owned();
        }
    }
    text.to_owned()
}

/// A long run of token characters is removed when it mixes letters and digits, which is what
/// keys, cookie values and hashes look like and a long word or a number never does.
fn redact_opaque(c: &Captures) -> String {
    let text = &c[0];
    let digit = text.chars().any(|ch| ch.is_ascii_digit());
    let letter = text.chars().any(|ch| ch.is_ascii_alphabetic());
    if digit && letter {
        "[token redacted]".to_owned()
    } else {
        text.to_owned()
    }
}

/// Replaces every case-insensitive occurrence of `needle`. Done by hand: lower-casing the
/// haystack can change its byte length for some scripts, so matches are found on chars.
fn replace_ignore_case(haystack: &str, needle: &str, with: &str) -> String {
    let needle_lower: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle_lower.is_empty() {
        return haystack.to_owned();
    }
    let chars: Vec<(usize, char)> = haystack.char_indices().collect();
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while i < chars.len() {
        // Does the needle match starting at char i?
        let mut j = i;
        let mut k = 0;
        while j < chars.len() && k < needle_lower.len() {
            let lowered: Vec<char> = chars[j].1.to_lowercase().collect();
            if needle_lower[k..].len() >= lowered.len()
                && needle_lower[k..k + lowered.len()] == lowered[..]
            {
                k += lowered.len();
                j += 1;
            } else {
                break;
            }
        }
        if k == needle_lower.len() {
            out.push_str(with);
            i = j;
        } else {
            out.push(chars[i].1);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrub(text: &str) -> String {
        Redactor::new().redact(text)
    }

    #[test]
    fn cookie_and_auth_headers_lose_their_values() {
        let out = scrub(
            "GET /youtubei/v1/browse\nCookie: SAPISID=AbC123xyz/Q; __Secure-3PSID=g.a000abcdef\n\
             authorization: SAPISIDHASH 1759565553_0f1e2d3c4b5a69788796a5b4c3d2e1f001234567\n\
             Set-Cookie: SID=zzzz; Path=/\nX-Goog-Visitor-Id: CgtWaXNpdG9yMTIz",
        );
        for needle in [
            "AbC123xyz",
            "g.a000abcdef",
            "0f1e2d3c4b5a",
            "zzzz",
            "CgtWaXNpdG9y",
        ] {
            assert!(!out.contains(needle), "{needle} survived: {out}");
        }
        assert!(out.contains("GET /youtubei/v1/browse"));
        assert!(out.contains("Cookie: [redacted]"));
    }

    #[test]
    fn ordinary_words_that_are_also_auth_schemes_are_left_alone() {
        let text = "basic settings loaded; bearer of news";
        assert_eq!(scrub(text), text);
        let out = scrub("sent Bearer ya29.a0AfH6SMBsecret and Basic dXNlcjpwYXNzd29yZA==");
        assert!(!out.contains("ya29") && !out.contains("dXNlcjpw"), "{out}");
    }

    #[test]
    fn visitor_data_is_removed_in_any_spelling() {
        let out = scrub("visitorData=CgtWaXNpdG9yMTIz&visitor_id=abc123def456 visitor=zz9plural");
        assert!(
            !out.contains("CgtWaXNp") && !out.contains("abc123def") && !out.contains("zz9plural"),
            "{out}"
        );
    }

    #[test]
    fn cookie_pairs_outside_a_header_are_caught() {
        let out = scrub(
            "jar had SAPISID=AbC123xyz and __Secure-1PSIDTS=sidts-CjIB.long; also YSC=q1w2e3",
        );
        for needle in ["AbC123xyz", "sidts-CjIB", "q1w2e3"] {
            assert!(!out.contains(needle), "{needle} survived: {out}");
        }
    }

    #[test]
    fn netscape_cookie_lines_are_removed() {
        let line = ".youtube.com\tTRUE\t/\tTRUE\t1790000000\t__Secure-3PSID\tg.a000secretvalue";
        let out = scrub(&format!("# Netscape HTTP Cookie File\n{line}\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc"));
        assert!(
            !out.contains("secretvalue") && !out.contains("\tabc"),
            "{out}"
        );
        assert!(out.contains("# Netscape HTTP Cookie File"));
    }

    #[test]
    fn stream_urls_vanish_entirely() {
        let url = "https://rr3---sn-4g5e6nsz.googlevideo.com/videoplayback?expire=1759570000&ei=abc&ip=203.0.113.9&id=o-AB12&sig=AOq0QJ8wRQIhAK&lsig=AG3C_xAE";
        let out = scrub(&format!("opening {url} now"));
        assert_eq!(out, "opening [stream URL redacted] now");
        let bare = scrub("rr3---sn-4g5e6nsz.googlevideo.com/videoplayback?sig=AOq0QJ8wRQ");
        assert!(!bare.contains("sig=") && !bare.contains("AOq0"), "{bare}");
    }

    #[test]
    fn urls_keep_host_and_at_most_a_known_path() {
        assert_eq!(
            scrub("fetch https://music.youtube.com/youtubei/v1/browse?key=AIzaSyABCDEF&prettyPrint=false failed"),
            "fetch https://music.youtube.com/youtubei/v1/browse?[query redacted] failed"
        );
        assert_eq!(
            scrub("see https://example.com/reset/Zm9vYmFy?code=1#frag"),
            "see https://example.com/[path redacted]?[query redacted]"
        );
        assert_eq!(
            scrub("https://user:hunter2@example.com:8080/x"),
            "https://example.com:8080/[path redacted]"
        );
        assert_eq!(
            scrub("avatar https://lh3.googleusercontent.com/a/ACg8ocLongAvatarToken=s96-c"),
            "avatar [stream URL redacted]"
        );
    }

    #[test]
    fn loose_query_pieces_and_json_secrets_go() {
        let out = scrub(
            r#"retry with sig=AOq0QJ8wRQ&expire=1759570000 and {"access_token": "ya29.a0Af", "name": "ok"}"#,
        );
        assert!(
            !out.contains("AOq0QJ8wRQ") && !out.contains("ya29"),
            "{out}"
        );
        assert!(out.contains(r#""name": "ok""#));
    }

    #[test]
    fn ip_addresses_of_both_families_go_but_clock_times_stay() {
        let out = scrub(
            "peer 203.0.113.9:443 and 10.0.0.12, v6 2001:db8:85a3::8a2e:370:7334, loopback ::1, \
             mapped ::ffff:192.0.2.1, at 08:12:33 on 2026-10-04T08:12:33Z",
        );
        for needle in ["203.0.113.9", "10.0.0.12", "2001:db8", "::1,", "192.0.2.1"] {
            assert!(!out.contains(needle), "{needle} survived: {out}");
        }
        assert!(out.contains("at 08:12:33 on 2026-10-04T08:12:33Z"), "{out}");
    }

    #[test]
    fn version_numbers_are_not_mistaken_for_addresses() {
        let out = scrub("yt-dlp 2026.08.19, Deno 2.9.7, Windows 10.0.26100.4652");
        assert_eq!(
            out,
            "yt-dlp 2026.08.19, Deno 2.9.7, Windows 10.0.26100.4652"
        );
    }

    #[test]
    fn email_addresses_go() {
        let out = scrub("signed in as zachary.example+music@gmail.com (premium)");
        assert_eq!(out, "signed in as [email redacted] (premium)");
    }

    #[test]
    fn the_windows_user_name_goes() {
        let out =
            scrub(r"C:\Users\Zachary\AppData\Local\Apollo\app\apollo.db and c:\users\zachary\x");
        assert!(!out.to_lowercase().contains("zachary"), "{out}");
        assert!(out.contains(r"AppData\Local\Apollo\app\apollo.db"));
    }

    #[test]
    fn opaque_tokens_go_but_long_words_and_numbers_stay() {
        let out = scrub("key AIzaSyA1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6 and sha 66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a");
        assert!(
            !out.contains("AIzaSy") && !out.contains("66674953"),
            "{out}"
        );
        let words =
            "Pneumonoultramicroscopicsilicovolcanoconiosis 12345678901234567890123456789012345";
        assert_eq!(scrub(words), words);
    }

    #[test]
    fn known_strings_are_removed_whatever_their_case() {
        let r = Redactor::new().with_known(["Alex Smith", "alexs", "ab"]);
        let out = r.redact("Welcome ALEX SMITH (@AlexS) - ab stays");
        assert_eq!(out, "Welcome [redacted] (@[redacted]) - ab stays");
    }

    #[test]
    fn redacting_twice_changes_nothing() {
        let once = scrub("Cookie: SAPISID=abc123\nhttps://rr1.googlevideo.com/videoplayback?sig=x\nme@example.com 203.0.113.9");
        assert_eq!(scrub(&once), once);
    }
}
