//! The yt-dlp command line. Built as an argument ARRAY (never a shell string). Contains no
//! secrets: the only credential-related argument is the *path* of the private cookie file.

use std::ffi::OsString;
use std::path::Path;

/// A YouTube video id: exactly 11 characters from `[A-Za-z0-9_-]`. Anything else is refused
/// before it can reach a command line (no pasted text, no option injection).
pub fn valid_video_id(id: &str) -> bool {
    id.len() == 11
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn watch_url(id: &str) -> Option<String> {
    valid_video_id(id).then(|| format!("https://music.youtube.com/watch?v={id}"))
}

/// Arguments for a resolution-only run: metadata JSON for the best audio format, no download,
/// no playlist, no config/plugins/cache/update, no remote components, explicit JS runtime.
pub fn build_args(video_id: &str, cookie_file: &Path, deno: &Path) -> Option<Vec<OsString>> {
    let url = watch_url(video_id)?;
    let mut a: Vec<OsString> = Vec::new();
    for flag in [
        "--ignore-config",
        "--no-plugin-dirs",
        "--no-cache-dir",
        "--no-remote-components",
        "--no-update",
        "--no-warnings",
        "--no-playlist",
        "--skip-download",
        "--no-write-comments",
        "-f",
        "bestaudio",
        "-J",
    ] {
        a.push(flag.into());
    }
    let mut js = OsString::from("deno:");
    js.push(deno.as_os_str());
    a.push("--js-runtimes".into());
    a.push(js);
    a.push("--cookies".into());
    a.push(cookie_file.as_os_str().to_owned());
    a.push(url.into());
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn video_ids_are_strict() {
        assert!(valid_video_id("dQw4w9WgXcQ"));
        assert!(valid_video_id("abc123DEF_-"));
        for bad in [
            "",
            "short",
            "dQw4w9WgXcQ1",
            "--exec calc",
            "dQw4w9WgXc&",
            "dQw4w9WgXc ",
            "dQw4w9WgXc/",
            "../etc/pa",
            "dQw4w9WgXcé",
        ] {
            assert!(!valid_video_id(bad), "{bad:?}");
        }
    }

    #[test]
    fn args_are_an_array_with_the_safety_flags_and_no_browser_cookies() {
        let args = build_args(
            "dQw4w9WgXcQ",
            &PathBuf::from("C:\\priv\\c.txt"),
            &PathBuf::from("C:\\tools\\deno.exe"),
        )
        .unwrap();
        let s: Vec<String> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        for must in [
            "--ignore-config",
            "--no-plugin-dirs",
            "--no-cache-dir",
            "--no-remote-components",
            "--no-update",
            "--skip-download",
            "--no-playlist",
            "bestaudio",
            "-J",
        ] {
            assert!(s.iter().any(|x| x == must), "missing {must}: {s:?}");
        }
        assert!(!s.iter().any(|x| x.contains("cookies-from-browser")));
        let i = s.iter().position(|x| x == "--cookies").unwrap();
        assert_eq!(s[i + 1], "C:\\priv\\c.txt");
        let j = s.iter().position(|x| x == "--js-runtimes").unwrap();
        assert_eq!(s[j + 1], "deno:C:\\tools\\deno.exe");
        assert_eq!(s.last().unwrap(), "https://music.youtube.com/watch?v=dQw4w9WgXcQ");
        // Exactly one positional URL and it is last.
        assert_eq!(s.iter().filter(|x| x.starts_with("https://")).count(), 1);
    }

    #[test]
    fn refuses_to_build_for_an_unsafe_id() {
        let p = PathBuf::from("x");
        assert!(build_args("--version  ", &p, &p).is_none());
        assert!(build_args("a b c d e f g", &p, &p).is_none());
    }
}
