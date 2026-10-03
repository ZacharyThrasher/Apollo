//! Fallback ways to get a session when the WebView2 sign-in is not available (AUTH-03):
//! a Netscape cookie file, a bare `Cookie:` header (an ephemeral seed), and Firefox, for a
//! profile the owner explicitly chooses (no scanning of other browsers or profiles).
//! Every result is scoped to the YouTube/Google account cookies and is untrusted until
//! `AccountService::import` validates it with the service.

use std::fs;
use std::path::{Path, PathBuf};

use crate::jar::{Cookie, CookieJar};

const MAX_COOKIE_FILE: u64 = 2 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    NotFound,
    TooLarge,
    Unreadable,
    /// Parsed fine but nothing in YouTube/Google scope (wrong file, wrong profile, signed out).
    NoRelevantCookies,
    /// The browser database could not be read.
    Database,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImportError::NotFound => "file not found",
            ImportError::TooLarge => "file is too large to be a cookie file",
            ImportError::Unreadable => "file could not be read",
            ImportError::NoRelevantCookies => "no YouTube/Google cookies found",
            ImportError::Database => "browser cookie database could not be read",
        })
    }
}

impl std::error::Error for ImportError {}

fn non_empty(jar: CookieJar) -> Result<CookieJar, ImportError> {
    if jar.is_empty() {
        Err(ImportError::NoRelevantCookies)
    } else {
        Ok(jar)
    }
}

/// Import a Netscape-format cookie file (browser extension export, yt-dlp, curl).
pub fn netscape_file(path: &Path) -> Result<CookieJar, ImportError> {
    let meta = fs::metadata(path).map_err(|_| ImportError::NotFound)?;
    if !meta.is_file() {
        return Err(ImportError::NotFound);
    }
    if meta.len() > MAX_COOKIE_FILE {
        return Err(ImportError::TooLarge);
    }
    let text = fs::read_to_string(path).map_err(|_| ImportError::Unreadable)?;
    non_empty(CookieJar::from_netscape(&text))
}

/// Import a bare `Cookie:` header. No attributes survive, so this is a session-only seed and the
/// jar says so (`ephemeral_seed`). No lifetime is invented.
pub fn cookie_header(text: &str) -> Result<CookieJar, ImportError> {
    if text.len() as u64 > MAX_COOKIE_FILE {
        return Err(ImportError::TooLarge);
    }
    non_empty(CookieJar::from_cookie_header(text).scoped())
}

// ------------------------------------------------------------------------- Firefox

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirefoxProfile {
    pub name: String,
    pub path: PathBuf,
    pub is_default: bool,
}

/// Profiles listed in `<firefox_dir>/profiles.ini` (normally `%APPDATA%\Mozilla\Firefox`).
/// Only reads that one ini file; cookie databases are not touched until the owner picks one.
pub fn firefox_profiles(firefox_dir: &Path) -> Vec<FirefoxProfile> {
    let Ok(text) = fs::read_to_string(firefox_dir.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut out: Vec<FirefoxProfile> = Vec::new();
    let mut install_default: Option<String> = None;
    let mut cur: Option<(String, String, bool, bool)> = None; // name, path, relative, default
    let mut in_install = false;
    let flush = |cur: &mut Option<(String, String, bool, bool)>, out: &mut Vec<FirefoxProfile>| {
        if let Some((name, path, rel, def)) = cur.take() {
            if !path.is_empty() {
                let p = PathBuf::from(native_separators(&path));
                out.push(FirefoxProfile {
                    name,
                    path: if rel { firefox_dir.join(p) } else { p },
                    is_default: def,
                });
            }
        }
    };
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            flush(&mut cur, &mut out);
            in_install = line.starts_with("[Install");
            cur = if line.starts_with("[Profile") {
                Some((String::new(), String::new(), true, false))
            } else {
                None
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        if in_install && k == "Default" {
            install_default = Some(native_separators(v));
        }
        if let Some(c) = cur.as_mut() {
            match k {
                "Name" => c.0 = v.to_string(),
                "Path" => c.1 = v.to_string(),
                "IsRelative" => c.2 = v == "1",
                "Default" => c.3 = v == "1",
                _ => {}
            }
        }
    }
    flush(&mut cur, &mut out);
    // Modern Firefox: the [Install...] section is authoritative. A profile-level `Default=1` is a
    // legacy flag that can point at a profile nobody uses, so it only counts when no install
    // section names a default.
    if let Some(d) = install_default {
        for p in out.iter_mut() {
            p.is_default = p.path.to_string_lossy().ends_with(&d);
        }
    }
    out
}

/// Firefox stores `expiry` in seconds; some versions use milliseconds. Anything beyond year
/// ~5138 in seconds must be milliseconds.
fn normalize_expiry(v: i64) -> Option<i64> {
    match v {
        i64::MIN..=0 => None,
        1..=99_999_999_999 => Some(v),
        _ => Some(v / 1000),
    }
}

/// Read the YouTube/Google cookies of one Firefox profile the owner chose. The database is
/// copied (with its WAL) into a private temp directory and read there, so Firefox's own files are
/// never modified or locked, and the copy is wiped afterwards.
#[cfg(windows)]
pub fn firefox_profile(profile_dir: &Path) -> Result<CookieJar, ImportError> {
    firefox_profile_with_temp(profile_dir, &std::env::temp_dir().join("apollo-import"))
}

/// Same, with an explicit base directory for the private working copy.
#[cfg(windows)]
pub fn firefox_profile_with_temp(profile_dir: &Path, tmp_base: &Path) -> Result<CookieJar, ImportError> {
    use apollo_services_common::win::PrivateDir;
    use rusqlite::{Connection, OpenFlags};

    let db = profile_dir.join("cookies.sqlite");
    if !db.is_file() {
        return Err(ImportError::NotFound);
    }
    let tmp = PrivateDir::create(tmp_base, "ff").map_err(|_| ImportError::Unreadable)?;
    let copy = tmp.path().join("cookies.sqlite");
    fs::copy(&db, &copy).map_err(|_| ImportError::Unreadable)?;
    for ext in ["-wal", "-shm"] {
        let side = profile_dir.join(format!("cookies.sqlite{ext}"));
        if side.is_file() {
            let _ = fs::copy(&side, tmp.path().join(format!("cookies.sqlite{ext}")));
        }
    }
    let result = (|| -> Result<CookieJar, ImportError> {
        let conn = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|_| ImportError::Database)?;
        // Read-write only so SQLite can replay the copied WAL; the copy is thrown away.
        let mut stmt = conn
            .prepare(
                "SELECT name, value, host, path, expiry, isSecure, isHttpOnly FROM moz_cookies \
                 WHERE host = 'youtube.com' OR host LIKE '%.youtube.com' \
                    OR host = 'google.com' OR host LIKE '%.google.com'",
            )
            .map_err(|_| ImportError::Database)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Cookie {
                    name: r.get(0)?,
                    value: r.get(1)?,
                    domain: r.get(2)?,
                    path: r.get(3)?,
                    expires_unix: normalize_expiry(r.get::<_, i64>(4)?),
                    secure: r.get::<_, i64>(5)? != 0,
                    http_only: r.get::<_, i64>(6)? != 0,
                })
            })
            .map_err(|_| ImportError::Database)?;
        let cookies: Vec<Cookie> = rows.filter_map(Result::ok).collect();
        non_empty(
            CookieJar {
                cookies,
                ephemeral_seed: false,
            }
            .scoped(),
        )
    })();
    let _ = fs::remove_dir_all(tmp.path());
    result
}

/// `profiles.ini` always uses `/`; paths are built with the platform's separator.
fn native_separators(p: &str) -> String {
    p.replace('/', std::path::MAIN_SEPARATOR_STR)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("apollo-import-test-{}", apollo_services_common::random_hex(6)));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn expiry_units() {
        assert_eq!(normalize_expiry(0), None);
        assert_eq!(normalize_expiry(-5), None);
        assert_eq!(normalize_expiry(1_900_000_000), Some(1_900_000_000));
        assert_eq!(normalize_expiry(1_900_000_000_000), Some(1_900_000_000));
    }

    #[test]
    fn netscape_file_import_and_limits() {
        let d = tmp();
        let f = d.join("c.txt");
        fs::write(
            &f,
            "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t4102444800\tSAPISID\tv\n.example.com\tTRUE\t/\tFALSE\t0\tx\ty\n",
        )
        .unwrap();
        let jar = netscape_file(&f).unwrap();
        assert_eq!(jar.names(), vec!["SAPISID"], "out-of-scope cookie dropped");
        assert_eq!(netscape_file(&d.join("missing.txt")).unwrap_err(), ImportError::NotFound);
        fs::write(d.join("other.txt"), "# nothing useful\n.example.com\tTRUE\t/\tFALSE\t0\tx\ty\n").unwrap();
        assert_eq!(netscape_file(&d.join("other.txt")).unwrap_err(), ImportError::NoRelevantCookies);
        fs::write(d.join("big.txt"), vec![b'a'; (MAX_COOKIE_FILE + 1) as usize]).unwrap();
        assert_eq!(netscape_file(&d.join("big.txt")).unwrap_err(), ImportError::TooLarge);
        let _ = fs::remove_dir_all(d);
    }

    #[test]
    fn header_seed_is_flagged_and_scoped() {
        let jar = cookie_header("Cookie: SAPISID=a; SID=b; __Secure-3PSID=c").unwrap();
        assert!(jar.ephemeral_seed);
        assert!(jar.cookies.iter().all(|c| c.expires_unix.is_none()), "no invented lifetime");
        assert_eq!(cookie_header("   ").unwrap_err(), ImportError::NoRelevantCookies);
    }

    #[test]
    fn profiles_ini_is_parsed_with_relative_and_default() {
        let d = tmp();
        fs::write(
            d.join("profiles.ini"),
            "[Install4F96D1932A9F858E]\nDefault=Profiles/abc.default-release\nLocked=1\n\n\
             [Profile1]\nName=default\nIsRelative=1\nPath=Profiles/xyz.default\n\n\
             [Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\nDefault=1\n\n\
             [General]\nStartWithLastProfile=1\n",
        )
        .unwrap();
        let ps = firefox_profiles(&d);
        assert_eq!(ps.len(), 2);
        let rel = ps.iter().find(|p| p.name == "default-release").unwrap();
        assert!(rel.is_default);
        assert_eq!(rel.path, d.join("Profiles").join("abc.default-release"));
        assert!(!ps.iter().find(|p| p.name == "default").unwrap().is_default);
        assert!(firefox_profiles(&d.join("nope")).is_empty());
        let _ = fs::remove_dir_all(d);
    }

    // Seen on a real machine: legacy `Default=1` on an unused profile, [Install] on the real one.
    #[test]
    fn the_install_section_beats_a_stale_legacy_default_flag() {
        let d = tmp();
        fs::write(
            d.join("profiles.ini"),
            "[Install308046B0AF4A39CB]
Default=Profiles/c1xiaa3m.default-release
             [Profile1]
Name=default
IsRelative=1
Path=Profiles/vgtecp58.default
Default=1
             [Profile0]
Name=default-release
IsRelative=1
Path=Profiles/c1xiaa3m.default-release
",
        )
        .unwrap();
        let ps = firefox_profiles(&d);
        let defaults: Vec<_> = ps.iter().filter(|p| p.is_default).map(|p| p.name.as_str()).collect();
        assert_eq!(defaults, vec!["default-release"], "{ps:?}");
        // Without any [Install] section the legacy flag still works.
        fs::write(
            d.join("profiles.ini"),
            "[Profile0]
Name=only
IsRelative=1
Path=Profiles/x.default
Default=1
",
        )
        .unwrap();
        assert!(firefox_profiles(&d)[0].is_default);
        let _ = fs::remove_dir_all(d);
    }
}
