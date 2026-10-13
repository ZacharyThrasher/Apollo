//! Finding the newest published build: GitHub's "latest release" for the repository, its zip and
//! the zip's SHA-256.

use std::time::Duration;

use crate::version::Version;

/// The repository releases are published to.
pub const REPO: &str = "ZacharyThrasher/Apollo";

const API_TIMEOUT: Duration = Duration::from_secs(20);
const USER_AGENT: &str = "Apollo-updater";

/// A published build that can be installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub zip_url: String,
    pub sha256_url: String,
}

/// Name of the zip a release `version` carries (`Apollo-0.1.17-win-x64.zip`).
pub fn zip_name(version: &str) -> String {
    format!("Apollo-{version}-win-x64.zip")
}

/// Reads GitHub's `releases/latest` answer. `None` when it is a draft or prerelease, its tag is
/// not a version, or it lacks the zip or the checksum; an asset URL must be on `repo`'s own
/// release download path.
pub fn parse_latest(json: &str, repo: &str) -> Option<Release> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value["draft"].as_bool() == Some(true) || value["prerelease"].as_bool() == Some(true) {
        return None;
    }
    let version = Version::parse(value["tag_name"].as_str()?)?;
    let want_zip = zip_name(&version.to_string());
    let want_sum = format!("{want_zip}.sha256");
    let prefix = format!("https://github.com/{repo}/releases/download/");
    let find = |name: &str| -> Option<String> {
        value["assets"].as_array()?.iter().find_map(|asset| {
            let url = asset["browser_download_url"].as_str()?;
            (asset["name"].as_str()? == name && url.starts_with(&prefix)).then(|| url.to_string())
        })
    };
    Some(Release {
        zip_url: find(&want_zip)?,
        sha256_url: find(&want_sum)?,
        version,
    })
}

/// The first hex token of a `.sha256` file (`<hash>  <name>` or just `<hash>`), lower-cased.
pub fn parse_checksum(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?;
    (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

pub(crate) fn agent(timeout: Option<Duration>) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(timeout)
        .user_agent(USER_AGENT)
        .build()
        .into()
}

/// Asks GitHub for the latest release of [`REPO`]. `Ok(None)`: it has nothing installable.
pub fn fetch_latest() -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut response = agent(Some(API_TIMEOUT))
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|err| format!("release check failed: {err}"))?;
    let body = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|err| format!("release check unreadable: {err}"))?;
    Ok(parse_latest(&body, REPO))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(tag: &str, extra: &str) -> String {
        format!(
            r#"{{"tag_name":"{tag}","draft":false,"prerelease":false,"assets":[
              {{"name":"Apollo-0.1.17-win-x64.zip","browser_download_url":"https://github.com/ZacharyThrasher/Apollo/releases/download/v0.1.17/Apollo-0.1.17-win-x64.zip"}},
              {{"name":"Apollo-0.1.17-win-x64.zip.sha256","browser_download_url":"https://github.com/ZacharyThrasher/Apollo/releases/download/v0.1.17/Apollo-0.1.17-win-x64.zip.sha256"}}
              {extra}]}}"#
        )
    }

    #[test]
    fn reads_a_release() {
        let release = parse_latest(&answer("v0.1.17", ""), REPO).unwrap();
        assert_eq!(release.version.to_string(), "0.1.17");
        assert!(release.zip_url.ends_with("Apollo-0.1.17-win-x64.zip"));
        assert!(release.sha256_url.ends_with(".zip.sha256"));
    }

    #[test]
    fn ignores_releases_it_cannot_install() {
        assert!(parse_latest(&answer("nightly", ""), REPO).is_none());
        // The tag names a version the assets do not belong to.
        assert!(parse_latest(&answer("v0.1.18", ""), REPO).is_none());
        let draft = answer("v0.1.17", "").replace(r#""draft":false"#, r#""draft":true"#);
        assert!(parse_latest(&draft, REPO).is_none());
        let pre = answer("v0.1.17", "").replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert!(parse_latest(&pre, REPO).is_none());
        assert!(parse_latest("not json", REPO).is_none());
    }

    #[test]
    fn only_trusts_downloads_from_its_own_repository() {
        let foreign = answer("v0.1.17", "").replace("ZacharyThrasher/Apollo", "someone/else");
        assert!(parse_latest(&foreign, REPO).is_none());
    }

    #[test]
    fn reads_checksums() {
        let hash = "A".repeat(64);
        assert_eq!(
            parse_checksum(&format!("{hash}  Apollo.zip\n")),
            Some("a".repeat(64))
        );
        assert_eq!(parse_checksum("short"), None);
        assert_eq!(parse_checksum(&"z".repeat(64)), None);
        assert_eq!(parse_checksum(""), None);
    }
}
