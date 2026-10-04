//! The diagnostics report: one plain-text file the owner can attach to a bug report.
//!
//! [`build`] is pure: it lays out what it is given and then runs the whole text through
//! [`Redactor`], so what leaves the app is scrubbed no matter how a value got in. Gathering the
//! values (the OS, the tools' versions, the settings) is [`super::Environment`]'s job.

use super::log::{looks_like_error, Entry};
use super::redact::Redactor;
use super::time::iso_utc;

/// How many of the most recent error lines get their own section.
pub const RECENT_ERRORS: usize = 20;
/// How many of the most recent log lines the report carries.
pub const RECENT_LOG_LINES: usize = 200;

/// Everything the report says, before it is scrubbed.
#[derive(Clone, Debug, Default)]
pub struct ReportInput {
    pub created_unix: u64,
    pub app_version: String,
    /// "real services" or "simulation".
    pub backend: String,
    pub os: String,
    pub arch: String,
    /// `(name, version)` of the helper tools; a tool that could not be asked says why instead.
    pub tools: Vec<(String, String)>,
    /// Whether anyone is signed in and what the app thinks of the session: never who.
    pub account: String,
    /// Setting key and value, known keys only (see `Environment::SETTINGS_KEYS`).
    pub settings: Vec<(String, String)>,
    /// The app's database: schema version and size.
    pub store: Option<String>,
    pub log: Vec<Entry>,
    /// Strings that must not appear anywhere in the report (see [`Redactor::with_known`]).
    pub known_secrets: Vec<String>,
}

/// The finished, scrubbed text.
pub fn build(input: &ReportInput) -> String {
    let mut out = String::new();
    out.push_str("Apollo diagnostics report\n");
    out.push_str("Cookies, sign-in headers, stream URLs, IP addresses, e-mail addresses and the Windows user name are removed.\n\n");

    section(&mut out, "App");
    line(&mut out, "Created", &iso_utc(input.created_unix));
    line(&mut out, "Version", &input.app_version);
    line(&mut out, "Backend", &input.backend);
    line(&mut out, "OS", &input.os);
    line(&mut out, "Architecture", &input.arch);
    line(&mut out, "Account", &input.account);
    if let Some(store) = &input.store {
        line(&mut out, "Database", store);
    }

    section(&mut out, "Tools");
    if input.tools.is_empty() {
        out.push_str("none (simulation)\n");
    }
    for (name, version) in &input.tools {
        line(&mut out, name, version);
    }

    section(&mut out, "Settings");
    for (key, value) in &input.settings {
        line(&mut out, key, value);
    }

    let errors: Vec<&Entry> = input
        .log
        .iter()
        .filter(|e| looks_like_error(&e.text))
        .collect();
    section(&mut out, "Recent errors");
    if errors.is_empty() {
        out.push_str("none\n");
    }
    for entry in tail(&errors, RECENT_ERRORS) {
        log_line(&mut out, entry);
    }

    section(&mut out, "Recent log");
    if input.log.is_empty() {
        out.push_str("empty\n");
    }
    let all: Vec<&Entry> = input.log.iter().collect();
    for entry in tail(&all, RECENT_LOG_LINES) {
        log_line(&mut out, entry);
    }

    Redactor::new()
        .with_known(input.known_secrets.iter().cloned())
        .redact(&out)
}

fn section(out: &mut String, title: &str) {
    out.push_str(&format!("\n== {title} ==\n"));
}

fn line(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!("{key}: {value}\n"));
}

fn log_line(out: &mut String, entry: &Entry) {
    out.push_str(&format!("{} {}\n", iso_utc(entry.at_unix), entry.text));
}

fn tail<T>(items: &[T], n: usize) -> &[T] {
    &items[items.len().saturating_sub(n)..]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything that must never reach a report, planted in every field that takes free text.
    const SECRETS: [&str; 12] = [
        "SAPISID=AbC123xyz-secret",
        "g.a000verysecretcookie",
        "SAPISIDHASH 1759565553_deadbeefdeadbeefdeadbeefdeadbeef01234567",
        "sig=AOq0QJ8wRQIhAKsecret",
        "203.0.113.77",
        "2001:db8:85a3::8a2e:370:7334",
        "zachary.private@gmail.com",
        "Zachary Thrasher",
        "zthrash",
        "Bearer ya29.a0AfH6SMBsecrettoken",
        "ytdlp-9f8e7d6c5b4a39281706f5e4d3c2b1a0",
        "visitor=CgtWaXNpdG9yU2VjcmV0",
    ];

    fn tainted() -> ReportInput {
        let entry = |text: &str| Entry {
            at_unix: 1_759_565_553,
            text: text.to_owned(),
        };
        ReportInput {
            created_unix: 1_759_565_553,
            app_version: "0.0.0".into(),
            backend: "real services".into(),
            os: r"Microsoft Windows [Version 10.0.26100.4652] C:\Users\zthrash".into(),
            arch: "x86_64".into(),
            tools: vec![
                ("yt-dlp".into(), "2026.08.19".into()),
                ("libmpv".into(), "client API 2.3 (sig=AOq0QJ8wRQIhAKsecret)".into()),
            ],
            account: "signed in as Zachary Thrasher <zachary.private@gmail.com>".into(),
            settings: vec![
                ("audio_device".into(), "wasapi/{0.0.0.00000000}.{abc}".into()),
                ("note".into(), "Cookie: SAPISID=AbC123xyz-secret; __Secure-3PSID=g.a000verysecretcookie".into()),
            ],
            store: Some(r"schema 7, 120 KB at C:\Users\zthrash\AppData\Local\Apollo\app\apollo.db".into()),
            log: vec![
                entry("latency resolve=210 open=90"),
                entry("resolve failed for https://rr3---sn-a.googlevideo.com/videoplayback?sig=AOq0QJ8wRQIhAKsecret&ip=203.0.113.77"),
                entry("authorization: SAPISIDHASH 1759565553_deadbeefdeadbeefdeadbeefdeadbeef01234567"),
                entry("could not reach 2001:db8:85a3::8a2e:370:7334 as zachary.private@gmail.com"),
                entry("sign-in failed for Zachary Thrasher (Bearer ya29.a0AfH6SMBsecrettoken)"),
                entry(r"cookie file C:\Users\zthrash\AppData\Local\Apollo\app\run\cookies\ytdlp-9f8e7d6c5b4a39281706f5e4d3c2b1a0\cookies.txt"),
                entry("beacon visitor=CgtWaXNpdG9yU2VjcmV0 ok"),
            ],
            known_secrets: vec!["Zachary Thrasher".into(), "zthrash".into()],
        }
    }

    /// The tests the safety bundle is for: a report built from tainted inputs has none of the
    /// planted secrets, anywhere, in either case.
    #[test]
    fn no_planted_secret_survives_anywhere_in_the_report() {
        let report = build(&tainted());
        let lower = report.to_lowercase();
        for secret in SECRETS {
            assert!(
                !lower.contains(&secret.to_lowercase()),
                "{secret:?} leaked into the report:\n{report}"
            );
        }
        // Not just everything removed: the useful parts are still there.
        assert!(report.contains("yt-dlp: 2026.08.19"));
        assert!(report.contains("Backend: real services"));
        assert!(report.contains("latency resolve=210 open=90"));
        assert!(report.contains(r"AppData\Local\Apollo\app\apollo.db"));
    }

    #[test]
    fn the_report_names_the_sections_in_order() {
        let report = build(&tainted());
        let at = |s: &str| report.find(s).unwrap_or_else(|| panic!("missing {s}"));
        assert!(at("== App ==") < at("== Tools ==") && at("== Tools ==") < at("== Settings =="));
        assert!(
            at("== Settings ==") < at("== Recent errors ==")
                && at("== Recent errors ==") < at("== Recent log ==")
        );
        assert!(report.contains("Created: 2025-10-04T08:12:33Z"));
    }

    #[test]
    fn errors_are_listed_apart_and_the_log_is_capped() {
        let mut input = ReportInput::default();
        for n in 0..RECENT_LOG_LINES + 50 {
            input.log.push(Entry {
                at_unix: 0,
                text: format!("line {n}"),
            });
        }
        input.log.push(Entry {
            at_unix: 0,
            text: "sign-in failed".into(),
        });
        let report = build(&input);
        let errors = report
            .split("== Recent errors ==")
            .nth(1)
            .unwrap()
            .split("== Recent log ==")
            .next()
            .unwrap();
        assert!(errors.contains("sign-in failed") && !errors.contains("line 0"));
        let log = report.split("== Recent log ==").nth(1).unwrap();
        assert_eq!(
            log.lines()
                .filter(|l| l.contains("line ") || l.contains("sign-in"))
                .count(),
            RECENT_LOG_LINES
        );
        assert!(
            !log.contains("line 0\n") && log.contains(&format!("line {}", RECENT_LOG_LINES + 49))
        );
    }

    #[test]
    fn an_empty_input_still_gives_a_readable_report() {
        let report = build(&ReportInput::default());
        assert!(report.contains("== Recent errors ==\nnone"));
        assert!(report.contains("== Recent log ==\nempty"));
    }
}
