use std::path::{Path, PathBuf};
use std::time::Duration;

use apollo_domain::lease::UrlPolicy;

/// SHA-256 pins for the resolver helper binaries (BUILD_SPEC RES-01, docs/build-baseline.md).
/// Verified once when the resolver is created, so an updated or swapped executable is never
/// run silently.
#[derive(Clone, Debug)]
pub struct PinnedTools {
    pub ytdlp_sha256: String,
    pub deno_sha256: String,
}

impl PinnedTools {
    /// yt-dlp 2026.08.19 (win_exe, bundles yt_dlp_ejs 0.8.0) and Deno 2.9.7, as recorded in M0.
    pub fn m0_baseline() -> Self {
        Self {
            ytdlp_sha256: "66674953fe251b89f4d08c5f0e35e0728679bd67ab3d7d05c0562af101dd3e7a".into(),
            deno_sha256: "e020f3e232bd16e33768dee528e5983349c962952051ced0a5d58ad42f5d9b33".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolverConfig {
    pub ytdlp: PathBuf,
    pub deno: PathBuf,
    /// Base for the private cookie directories and the helper's scratch directory.
    pub run_dir: PathBuf,
    /// Hard limit for one yt-dlp run (the whole process tree is killed at this point).
    pub timeout: Duration,
    /// At most this many helpers at once (active load plus one speculative preload).
    pub max_concurrent: usize,
    /// yt-dlp's `-J` output is capped; more than this kills the run.
    pub max_stdout_bytes: usize,
    pub url_policy: UrlPolicy,
    /// `None` skips verification (tests only).
    pub pinned: Option<PinnedTools>,
    /// Extra environment for the helper. Only exists in test builds.
    #[cfg(feature = "test-support")]
    pub extra_env: Vec<(String, String)>,
}

impl ResolverConfig {
    /// Production settings: tools under `tools_dir` (`yt-dlp/yt-dlp.exe`, `deno/deno.exe`),
    /// pinned to the M0 baseline, production URL policy, 30 s timeout.
    pub fn production(tools_dir: &Path, run_dir: PathBuf) -> Self {
        Self {
            ytdlp: tools_dir.join("yt-dlp").join("yt-dlp.exe"),
            deno: tools_dir.join("deno").join("deno.exe"),
            run_dir,
            timeout: Duration::from_secs(30),
            max_concurrent: 2,
            max_stdout_bytes: 8 * 1024 * 1024,
            url_policy: UrlPolicy::production(),
            pinned: Some(PinnedTools::m0_baseline()),
            #[cfg(feature = "test-support")]
            extra_env: Vec::new(),
        }
    }
}
