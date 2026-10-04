//! Settings > Data > Diagnostics report: a plain-text file with what a bug report needs and
//! nothing a stranger could use. [`log`] keeps the app's last stderr lines, [`redact`] scrubs,
//! [`report`] lays the text out, and [`gather`] collects the facts (blocking: it asks the helper
//! tools for their versions, so call it off the UI thread).

pub mod log;
pub mod redact;
pub mod report;
pub mod time;

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use apollo_services_common::unix_now;

use crate::settings::{SettingsBackend, REPORTED_KEYS};

pub use report::{build, ReportInput};

/// What the backend said about itself when it was installed.
struct BackendNote {
    label: &'static str,
    tools_dir: Option<PathBuf>,
}

static BACKEND: OnceLock<BackendNote> = OnceLock::new();

/// Records the backend's short label and where its helper tools live (`None` in the
/// simulation), once, when the backend is installed.
pub fn note_backend(label: &'static str, tools_dir: Option<PathBuf>) {
    let _ = BACKEND.set(BackendNote { label, tools_dir });
}

/// What the UI thread knows, handed to [`gather`] along with the clock.
pub struct Snapshot {
    /// "signed in, session valid, Premium assumed": never who.
    pub account: String,
    /// The account's name, handle and channel id: scrubbed wherever they show up.
    pub known_secrets: Vec<String>,
    pub settings: Option<Arc<dyn SettingsBackend>>,
    /// The app's database file, when it has one.
    pub db_path: Option<PathBuf>,
}

/// Collects the report's facts. Blocking: runs the helper tools with `--version`.
pub fn gather(snapshot: Snapshot) -> ReportInput {
    let backend = BACKEND.get();
    let settings = snapshot
        .settings
        .as_ref()
        .map(|backend| {
            REPORTED_KEYS
                .iter()
                .map(|key| {
                    let value = backend.get(key).unwrap_or_else(|| "(default)".to_owned());
                    ((*key).to_owned(), value)
                })
                .collect()
        })
        .unwrap_or_default();
    let mut known = snapshot.known_secrets;
    known.extend(ambient_secrets(|name| std::env::var(name).ok()));
    ReportInput {
        created_unix: unix_now(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        backend: backend.map_or("not started", |b| b.label).to_owned(),
        os: os_description(),
        arch: std::env::consts::ARCH.to_owned(),
        tools: backend
            .and_then(|b| b.tools_dir.as_deref())
            .map(tool_versions)
            .unwrap_or_default(),
        account: snapshot.account,
        settings,
        store: snapshot.db_path.as_deref().and_then(store_summary),
        log: log::snapshot(),
        known_secrets: known,
    }
}

/// The Windows account and machine names, which appear in paths and in some tool output.
fn ambient_secrets(var: impl Fn(&str) -> Option<String>) -> Vec<String> {
    ["USERNAME", "USER", "COMPUTERNAME", "USERDOMAIN"]
        .iter()
        .filter_map(|name| var(name))
        .collect()
}

/// "schema 7, 120 KB". Nothing from inside the database.
fn store_summary(path: &Path) -> Option<String> {
    let bytes = std::fs::metadata(path).ok()?.len();
    let schema = apollo_store::Store::open(path)
        .ok()
        .and_then(|s| s.schema_version().ok())
        .map_or("unknown".to_owned(), |v| v.to_string());
    Some(format!("schema {schema}, {} KB", bytes.div_ceil(1024)))
}

#[cfg(windows)]
fn os_description() -> String {
    // `ver` prints "Microsoft Windows [Version 10.0.26100.4652]".
    run_version("cmd", &["/C", "ver"]).unwrap_or_else(|| "Windows".to_owned())
}

#[cfg(not(windows))]
fn os_description() -> String {
    std::env::consts::OS.to_owned()
}

/// The helper tools' versions, each asked for once with a short timeout. A tool that does not
/// answer is reported as such: a missing or broken tool is exactly what a report is for.
#[cfg(all(windows, feature = "real-services"))]
fn tool_versions(dir: &Path) -> Vec<(String, String)> {
    let ask = |name: &str, exe: PathBuf| {
        let version =
            run_version(&exe, &["--version"]).unwrap_or_else(|| "did not answer".to_owned());
        (name.to_owned(), version)
    };
    vec![
        ask("yt-dlp", dir.join("yt-dlp").join("yt-dlp.exe")),
        ask("Deno", dir.join("deno").join("deno.exe")),
        ("libmpv".to_owned(), apollo_media::client_api_version()),
    ]
}

#[cfg(not(all(windows, feature = "real-services")))]
fn tool_versions(_: &Path) -> Vec<(String, String)> {
    Vec::new()
}

/// Runs `exe args` without a console window and returns the first line it prints, giving up
/// after a few seconds.
#[cfg(windows)]
fn run_version(exe: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);

    impl SettingsBackend for Memory {
        fn get(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: &str, value: &str) {
            self.0.lock().unwrap().insert(key.into(), value.into());
        }
    }

    /// Only known keys are listed: the table also holds recent searches, the saved queue and
    /// window positions, none of which belong in a report.
    #[test]
    fn the_report_lists_known_settings_only() {
        let memory = Memory::default();
        memory.set("volume_percent", "70");
        memory.set("recent_searches", "my embarrassing search");
        memory.set("window_main", "x=10,y=20");
        let input = gather(Snapshot {
            account: "signed in".into(),
            known_secrets: Vec::new(),
            settings: Some(Arc::new(memory)),
            db_path: None,
        });
        let text = build(&input);
        assert!(text.contains("volume_percent: 70"));
        assert!(
            text.contains("close_to_tray: (default)"),
            "an unset setting says so"
        );
        assert!(!text.contains("embarrassing") && !text.contains("\nrecent_searches:"));
        assert!(!text.contains("window_main"));
    }

    #[test]
    fn the_windows_user_and_machine_names_are_scrubbed_wherever_they_appear() {
        let env: HashMap<&str, &str> =
            HashMap::from([("USERNAME", "zthrash"), ("COMPUTERNAME", "ZACH-DESKTOP")]);
        let known = ambient_secrets(|name| env.get(name).map(|v| (*v).to_owned()));
        assert_eq!(known, ["zthrash", "ZACH-DESKTOP"]);
        let input = ReportInput {
            known_secrets: known,
            log: vec![log::Entry {
                at_unix: 0,
                text: "opened D:\\sync\\zthrash-profile\\file on zach-desktop".into(),
            }],
            ..ReportInput::default()
        };
        let text = build(&input).to_lowercase();
        assert!(
            !text.contains("zthrash") && !text.contains("zach-desktop"),
            "{text}"
        );
    }
}
