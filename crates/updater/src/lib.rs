//! Apollo's self-update. A build is published to GitHub Releases on every change to `main`
//! (`.github/workflows/release.yml`); the running app finds a newer one ([`release`]), stages it
//! beside the install without touching it ([`stage`]), and the swap happens only while Apollo
//! is not running ([`helper`], [`apply`]), so playback is never interrupted.

pub mod apply;
pub mod helper;
pub mod release;
pub mod stage;
pub mod version;

pub use release::{fetch_latest, Release};
pub use stage::{discard_staged, failed_version, stage, staged, Staged};
pub use version::Version;

/// Looks for a build newer than `installed` and, if there is one, stages it. Returns the version
/// now staged (found just now or on an earlier check). A version a swap already failed on is
/// skipped.
pub fn check_and_stage(
    installed: &Version,
    updates: &std::path::Path,
) -> Result<Option<Version>, String> {
    let waiting = staged(updates).map(|s| s.version);
    let Some(latest) = fetch_latest()? else {
        return Ok(waiting);
    };
    if latest.version <= *installed {
        return Ok(waiting);
    }
    if waiting.as_ref().is_some_and(|w| *w >= latest.version) {
        return Ok(waiting);
    }
    if failed_version(updates).is_some_and(|f| f >= latest.version) {
        return Ok(waiting);
    }
    stage(&latest, updates)?;
    Ok(Some(latest.version))
}
