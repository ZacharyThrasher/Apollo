//! Getting a build onto disk without touching the installed one: download the zip, check its
//! SHA-256 against the release's checksum file, unpack it under `updates\staged\files` and only
//! then write `READY`. A half-finished download is never mistaken for a staged update.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::release::{agent, parse_checksum, Release};
use crate::version::Version;

/// A zip bigger than this is not Apollo (the real one is a few tens of MB).
const MAX_ZIP_BYTES: u64 = 400 * 1024 * 1024;
/// Everything the zip unpacks to, together.
const MAX_UNPACKED_BYTES: u64 = 1024 * 1024 * 1024;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SUM_TIMEOUT: Duration = Duration::from_secs(30);
/// Files a build must carry to be worth switching to.
const REQUIRED: &[&str] = &["Apollo.exe", "libmpv-2.dll"];

const READY: &str = "READY";
const FAILED: &str = "failed.txt";

/// A build that is unpacked and waiting for the swap.
#[derive(Clone, Debug)]
pub struct Staged {
    pub version: Version,
    pub files: PathBuf,
}

pub(crate) fn staged_dir(updates: &Path) -> PathBuf {
    updates.join("staged")
}

/// The staged build, when one is complete.
pub fn staged(updates: &Path) -> Option<Staged> {
    let dir = staged_dir(updates);
    let version = Version::parse(&fs::read_to_string(dir.join(READY)).ok()?)?;
    let files = dir.join("files");
    files.is_dir().then_some(Staged { version, files })
}

/// Throws the staged build away (after the swap, or when it could not be applied).
pub fn discard_staged(updates: &Path) {
    let _ = fs::remove_dir_all(staged_dir(updates));
}

/// Remembers that `version` could not be installed, so it is not downloaded again and again.
pub fn mark_failed(updates: &Path, version: &Version) {
    let _ = fs::create_dir_all(updates);
    let _ = fs::write(updates.join(FAILED), version.to_string());
}

/// The version a swap failed on, if any.
pub fn failed_version(updates: &Path) -> Option<Version> {
    Version::parse(&fs::read_to_string(updates.join(FAILED)).ok()?)
}

/// Downloads and unpacks `release` into `updates\staged`, replacing any earlier staged build.
pub fn stage(release: &Release, updates: &Path) -> Result<(), String> {
    fs::create_dir_all(updates).map_err(|err| format!("updates folder: {err}"))?;
    discard_staged(updates);
    let result = stage_inner(release, updates);
    if result.is_err() {
        discard_staged(updates);
    }
    let _ = fs::remove_file(updates.join("download.zip.part"));
    result
}

fn stage_inner(release: &Release, updates: &Path) -> Result<(), String> {
    let sum_text = http_text(&release.sha256_url)?;
    let want = parse_checksum(&sum_text).ok_or("checksum file unreadable")?;

    let part = updates.join("download.zip.part");
    let got = download(&release.zip_url, &part)?;
    if got != want {
        return Err("download does not match its checksum".into());
    }

    let files = staged_dir(updates).join("files");
    unpack(&part, &files)?;
    for name in REQUIRED {
        if !files.join(name).is_file() {
            return Err(format!("build is missing {name}"));
        }
    }
    fs::write(staged_dir(updates).join(READY), release.version.to_string())
        .map_err(|err| format!("could not mark build ready: {err}"))
}

fn http_text(url: &str) -> Result<String, String> {
    let mut response = agent(Some(SUM_TIMEOUT))
        .get(url)
        .call()
        .map_err(|err| format!("checksum download failed: {err}"))?;
    response
        .body_mut()
        .with_config()
        .limit(4096)
        .read_to_string()
        .map_err(|err| format!("checksum unreadable: {err}"))
}

/// Streams `url` into `to`, returning the lower-case hex SHA-256 of what arrived.
fn download(url: &str, to: &Path) -> Result<String, String> {
    let mut response = agent(Some(DOWNLOAD_TIMEOUT))
        .get(url)
        .call()
        .map_err(|err| format!("download failed: {err}"))?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_ZIP_BYTES)
        .reader();
    let mut file = fs::File::create(to).map_err(|err| format!("download file: {err}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|err| format!("download interrupted: {err}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|err| format!("download write: {err}"))?;
    }
    file.flush()
        .map_err(|err| format!("download write: {err}"))?;
    Ok(to_hex(&hasher.finalize()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Unpacks `zip_path` into `into`. Entries that would land outside it are an error.
pub(crate) fn unpack(zip_path: &Path, into: &Path) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|err| format!("zip: {err}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|err| format!("zip unreadable: {err}"))?;
    fs::create_dir_all(into).map_err(|err| format!("unpack folder: {err}"))?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|err| format!("zip entry: {err}"))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| format!("zip entry outside the folder: {}", entry.name()))?;
        let dest = into.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&dest).map_err(|err| format!("unpack: {err}"))?;
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_UNPACKED_BYTES {
            return Err("zip unpacks to too much".into());
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("unpack: {err}"))?;
        }
        let mut out = fs::File::create(&dest).map_err(|err| format!("unpack: {err}"))?;
        std::io::copy(&mut entry, &mut out).map_err(|err| format!("unpack: {err}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zip_with(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn unpacks_nested_files() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("a.zip");
        zip_with(
            &zip,
            &[
                ("Apollo.exe", b"exe"),
                ("tools/yt-dlp/yt-dlp.exe", b"ytdlp"),
            ],
        );
        let out = dir.path().join("out");
        unpack(&zip, &out).unwrap();
        assert_eq!(fs::read(out.join("Apollo.exe")).unwrap(), b"exe");
        assert_eq!(
            fs::read(out.join("tools/yt-dlp/yt-dlp.exe")).unwrap(),
            b"ytdlp"
        );
    }

    #[test]
    fn refuses_entries_that_escape() {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("evil.zip");
        zip_with(&zip, &[("../evil.txt", b"x")]);
        let out = dir.path().join("out");
        assert!(unpack(&zip, &out).is_err());
        assert!(!dir.path().join("evil.txt").exists());
    }

    #[test]
    fn staged_needs_the_ready_marker() {
        let dir = tempfile::tempdir().unwrap();
        let staged_files = staged_dir(dir.path()).join("files");
        fs::create_dir_all(&staged_files).unwrap();
        assert!(staged(dir.path()).is_none());
        fs::write(staged_dir(dir.path()).join(READY), "0.1.5").unwrap();
        assert_eq!(staged(dir.path()).unwrap().version.to_string(), "0.1.5");
        discard_staged(dir.path());
        assert!(staged(dir.path()).is_none());
    }

    #[test]
    fn remembers_a_failed_version() {
        let dir = tempfile::tempdir().unwrap();
        assert!(failed_version(dir.path()).is_none());
        mark_failed(dir.path(), &Version::parse("0.1.9").unwrap());
        assert_eq!(failed_version(dir.path()).unwrap().to_string(), "0.1.9");
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(to_hex(&[0, 15, 255]), "000fff");
    }
}
