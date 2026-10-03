//! Diagnostics helper for owner-run probes: list some tracks of the account's Liked Music
//! playlist through the same contained, pinned yt-dlp. The real metadata adapter (provider crate)
//! replaces this for the app; here it only lets a probe pick varied tracks without handling any
//! secret. Ids and artist names stay in memory in the returned struct; nothing is logged.

use std::collections::HashSet;
use std::fs;
use std::process::Command;
use std::time::Duration;

use apollo_domain::playback::Failure;
use apollo_services_common::win::PrivateDir;
use apollo_services_common::SessionProvider;

use crate::argv::valid_video_id;
use crate::config::ResolverConfig;
use crate::parse::classify_failure;
use crate::runner::{run_contained, KillSwitch, RunStatus};

#[derive(Clone)]
pub struct ListedTrack {
    pub id: String,
    pub duration_secs: Option<u64>,
    /// Channel/artist label, used only to pick varied tracks.
    pub artist: Option<String>,
    /// Display title and the listing's own thumbnail URL, for the app's quick test queue.
    pub title: Option<String>,
    pub thumbnail: Option<String>,
}

impl std::fmt::Debug for ListedTrack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ListedTrack(<redacted>, {:?}s)", self.duration_secs)
    }
}

/// Up to `max` entries of Liked Music. Requires a valid account session (no guest listing).
pub fn list_liked_music(
    cfg: &ResolverConfig,
    session: &dyn SessionProvider,
    max: usize,
) -> Result<Vec<ListedTrack>, Failure> {
    let snapshot = session.snapshot().ok_or(Failure::SessionExpired)?;
    let cookies_base = cfg.run_dir.join("cookies");
    let scratch = cfg.run_dir.join("scratch");
    fs::create_dir_all(&scratch).map_err(|_| Failure::Unsupported)?;
    let dir = PrivateDir::create(&cookies_base, "ytdlp").map_err(|_| Failure::Unsupported)?;
    let cookie_path = dir
        .create_file("cookies.txt", snapshot.cookies_netscape.expose().as_bytes())
        .map_err(|_| Failure::Unsupported)?;

    let mut cmd = Command::new(&cfg.ytdlp);
    cmd.args([
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-cache-dir",
        "--no-remote-components",
        "--no-update",
        "--no-warnings",
        "--skip-download",
        "--flat-playlist",
        "--yes-playlist",
        "-J",
    ]);
    cmd.arg("--playlist-end").arg(max.to_string());
    let mut js = std::ffi::OsString::from("deno:");
    js.push(cfg.deno.as_os_str());
    cmd.arg("--js-runtimes").arg(js);
    cmd.arg("--cookies").arg(&cookie_path);
    cmd.arg("https://music.youtube.com/playlist?list=LM");
    cmd.env_clear().current_dir(&scratch);
    let sys_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    cmd.env("SystemRoot", &sys_root)
        .env("TEMP", &scratch)
        .env("TMP", &scratch)
        .env("USERPROFILE", &scratch)
        .env("APPDATA", &scratch)
        .env("LOCALAPPDATA", &scratch)
        .env("PYTHONIOENCODING", "utf-8");

    let switch = KillSwitch::default();
    let run = run_contained(cmd, Duration::from_secs(120), cfg.max_stdout_bytes.max(16 * 1024 * 1024), &switch);
    drop(dir); // cookie file zeroed and removed before anything else
    match run.status {
        RunStatus::Exited(0) => {}
        RunStatus::Exited(_) => return Err(classify_failure(&run.stderr).0),
        RunStatus::TimedOut => return Err(Failure::Network),
        _ => return Err(Failure::Unsupported),
    }
    let j: serde_json::Value = serde_json::from_slice(&run.stdout).map_err(|_| Failure::Unsupported)?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in j["entries"].as_array().into_iter().flatten() {
        let Some(id) = e["id"].as_str().filter(|i| valid_video_id(i)) else { continue };
        if !seen.insert(id.to_owned()) {
            continue;
        }
        out.push(ListedTrack {
            id: id.to_owned(),
            duration_secs: e["duration"].as_f64().map(|d| d as u64),
            artist: e["channel"].as_str().or(e["uploader"].as_str()).map(str::to_owned),
            title: e["title"].as_str().map(str::to_owned),
            // Largest first; prefer a JPEG/PNG over WebP only if one is listed.
            thumbnail: e["thumbnails"].as_array().and_then(|t| {
                let urls: Vec<&str> = t.iter().rev().filter_map(|t| t["url"].as_str()).collect();
                urls.iter()
                    .find(|u| !u.split('?').next().unwrap_or(u).ends_with(".webp"))
                    .or(urls.first())
                    .map(|u| (*u).to_owned())
            }),
        });
    }
    Ok(out)
}

/// Choose `n` tracks with distinct artists, spread across durations.
pub fn pick_varied(mut tracks: Vec<ListedTrack>, n: usize) -> Vec<ListedTrack> {
    tracks.retain(|t| t.duration_secs.is_some_and(|d| (45..=900).contains(&d)));
    let mut artists = HashSet::new();
    tracks.retain(|t| artists.insert(t.artist.clone().unwrap_or_else(|| t.id.clone())));
    tracks.sort_by_key(|t| t.duration_secs);
    if tracks.len() <= n {
        return tracks;
    }
    (0..n)
        .map(|k| tracks[k * (tracks.len() - 1) / (n - 1).max(1)].clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, d: u64, a: &str) -> ListedTrack {
        ListedTrack { id: id.into(), duration_secs: Some(d), artist: Some(a.into()), title: None, thumbnail: None }
    }

    #[test]
    fn picks_distinct_artists_spread_over_durations() {
        let all = vec![
            t("a", 100, "x"), t("b", 120, "x"), t("c", 200, "y"), t("d", 300, "z"),
            t("e", 30, "w"), t("f", 1200, "v"), t("g", 400, "u"),
        ];
        let p = pick_varied(all, 3);
        assert_eq!(p.len(), 3);
        let artists: HashSet<_> = p.iter().map(|x| x.artist.clone()).collect();
        assert_eq!(artists.len(), 3, "distinct artists");
        assert!(p.iter().all(|x| (45..=900).contains(&x.duration_secs.unwrap())), "too short/long dropped");
        assert!(p.first().unwrap().duration_secs <= p.last().unwrap().duration_secs);
    }

    #[test]
    fn debug_hides_ids_and_artists() {
        assert!(!format!("{:?}", t("SECRETID001", 100, "Artist Name")).contains("SECRETID"));
        assert!(!format!("{:?}", t("SECRETID001", 100, "Artist Name")).contains("Artist"));
    }
}
