#![cfg(windows)]
//! Firefox import against synthetic profiles (a real `moz_cookies` schema built with rusqlite),
//! plus an opt-in redacted check against the owner's real Firefox profile.

use std::fs;
use std::path::PathBuf;

use apollo_account::import::{firefox_profile, firefox_profile_with_temp, firefox_profiles, ImportError};
use apollo_services_common::random_hex;
use rusqlite::Connection;

fn tmp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("apollo-ffimport-test-{}", random_hex(6)));
    fs::create_dir_all(&p).unwrap();
    p
}

fn make_profile(dir: &PathBuf, wal: bool) -> Connection {
    let conn = Connection::open(dir.join("cookies.sqlite")).unwrap();
    if wal {
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        // Keep rows in the WAL: Firefox is running, so the main file lags behind.
        conn.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
    }
    conn.execute_batch(
        "CREATE TABLE moz_cookies (id INTEGER PRIMARY KEY, originAttributes TEXT NOT NULL DEFAULT '', \
         name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, lastAccessed INTEGER, \
         creationTime INTEGER, isSecure INTEGER, isHttpOnly INTEGER, inBrowserElement INTEGER DEFAULT 0, \
         sameSite INTEGER DEFAULT 0, schemeMap INTEGER DEFAULT 0);",
    )
    .unwrap();
    let rows: &[(&str, &str, &str, i64, i64, i64)] = &[
        ("SAPISID", "VALUECANARY-sapisid", ".youtube.com", 1_900_000_000, 1, 0),
        ("__Secure-3PAPISID", "VALUECANARY-3papisid", ".youtube.com", 1_900_000_000_000, 1, 0), // ms
        ("SID", "VALUECANARY-sid", ".google.com", 1_900_000_000, 0, 1),
        ("__Secure-3PSID", "VALUECANARY-3psid", ".google.com", 1_900_000_000, 1, 1),
        ("LOGIN_INFO", "VALUECANARY-login", "music.youtube.com", 1_900_000_000, 1, 1),
        ("tracker", "no", ".doubleclick.net", 1_900_000_000, 0, 0),
        ("lookalike", "no", ".notyoutube.com", 1_900_000_000, 0, 0),
        ("evil", "no", "youtube.com.evil.net", 1_900_000_000, 0, 0),
    ];
    for (n, v, h, e, s, ho) in rows {
        conn.execute(
            "INSERT INTO moz_cookies (name, value, host, path, expiry, isSecure, isHttpOnly) \
             VALUES (?1, ?2, ?3, '/', ?4, ?5, ?6)",
            rusqlite::params![n, v, h, e, s, ho],
        )
        .unwrap();
    }
    conn
}

#[test]
fn imports_only_youtube_and_google_cookies_with_attributes() {
    let d = tmp();
    let _keep_open = make_profile(&d, false);
    let jar = firefox_profile(&d).unwrap();
    let mut names = jar.names();
    names.sort();
    assert_eq!(
        names,
        vec!["LOGIN_INFO", "SAPISID", "SID", "__Secure-3PAPISID", "__Secure-3PSID"],
        "lookalike and unrelated domains must not be imported"
    );
    assert!(jar.has_auth_cookies());
    // Attributes survive; milliseconds are normalized to seconds.
    assert_eq!(jar.get("SAPISID").unwrap().expires_unix, Some(1_900_000_000));
    assert_eq!(jar.get("__Secure-3PAPISID").unwrap().expires_unix, Some(1_900_000_000));
    assert!(jar.get("SID").unwrap().http_only && !jar.get("SID").unwrap().secure);
    assert!(!format!("{jar:?}").contains("VALUECANARY"));
    let _ = fs::remove_dir_all(d);
}

// Firefox is usually running: its newest rows live in the WAL, which must be included.
#[test]
fn sees_rows_that_only_exist_in_the_wal_and_never_modifies_the_original() {
    let d = tmp();
    let _firefox_still_has_it_open = make_profile(&d, true);
    assert!(d.join("cookies.sqlite-wal").exists(), "test setup: rows must be in the WAL");
    let before = (
        fs::read(d.join("cookies.sqlite")).unwrap(),
        fs::read(d.join("cookies.sqlite-wal")).unwrap(),
    );
    let jar = firefox_profile(&d).unwrap();
    assert!(jar.has_auth_cookies(), "WAL rows were missed: {:?}", jar.names());
    let after = (
        fs::read(d.join("cookies.sqlite")).unwrap(),
        fs::read(d.join("cookies.sqlite-wal")).unwrap(),
    );
    assert!(before == after, "the browser's own files were modified");
    let _ = fs::remove_dir_all(d);
}

#[test]
fn missing_database_and_signed_out_profiles_are_distinct_errors() {
    let d = tmp();
    assert_eq!(firefox_profile(&d).unwrap_err(), ImportError::NotFound);
    let conn = Connection::open(d.join("cookies.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE moz_cookies (name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, \
         isSecure INTEGER, isHttpOnly INTEGER);",
    )
    .unwrap();
    drop(conn);
    assert_eq!(firefox_profile(&d).unwrap_err(), ImportError::NoRelevantCookies);
    fs::write(d.join("cookies.sqlite"), b"this is not a database").unwrap();
    assert_eq!(firefox_profile(&d).unwrap_err(), ImportError::Database);
    let _ = fs::remove_dir_all(d);
}

#[test]
fn the_private_copy_is_removed_after_import() {
    let d = tmp();
    let _k = make_profile(&d, false);
    // A base of our own, so parallel tests cannot disturb the count.
    let base = tmp();
    firefox_profile_with_temp(&d, &base).unwrap();
    assert_eq!(
        fs::read_dir(&base).unwrap().count(),
        0,
        "a temporary copy of the cookie database was left behind"
    );
    assert!(firefox_profile_with_temp(&d.join("missing"), &base).is_err());
    assert_eq!(fs::read_dir(&base).unwrap().count(), 0, "also clean after a failed import");
    let _ = fs::remove_dir_all(base);
    let _ = fs::remove_dir_all(d);
}

/// Opt-in: reads the owner's REAL Firefox profile and prints only counts and presence flags.
///   cargo test -p apollo-account --test import -- --ignored --nocapture
#[test]
#[ignore = "reads the real Firefox profile of the person running the test"]
fn real_firefox_profile_redacted_summary() {
    let Some(appdata) = std::env::var_os("APPDATA") else { return };
    let ff = PathBuf::from(appdata).join("Mozilla").join("Firefox");
    let profiles = firefox_profiles(&ff);
    println!("profiles listed: {}", profiles.len());
    let Some(p) = profiles.iter().find(|p| p.is_default).or(profiles.first()) else {
        println!("NOT RUN: no Firefox profile found");
        return;
    };
    match firefox_profile(&p.path) {
        Ok(jar) => println!(
            "REAL FIREFOX IMPORT: ok, {} in-scope cookies, auth cookies present: {}",
            jar.len(),
            jar.has_auth_cookies()
        ),
        Err(e) => println!("REAL FIREFOX IMPORT: {e}"),
    }
}
