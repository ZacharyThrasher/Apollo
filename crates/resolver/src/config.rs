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

/// Keep yt-dlp running between tracks (see `warm.rs`). Without it every track pays for a new
/// process, a new YouTube session and a re-parse of the player script.
#[derive(Clone, Debug)]
pub struct WarmConfig {
    /// Most workers alive at once. Each is a resident yt-dlp (~100 MB).
    pub max_workers: usize,
    /// Most workers speculative (prefetch / preload) work may occupy, so a user's click always
    /// finds one free.
    pub background_limit: usize,
    /// Workers started in the background at launch, before anyone clicks play.
    pub prewarm: usize,
    /// A worker that has not announced itself within this long is killed.
    pub startup_timeout: Duration,
    /// A request that takes longer kills its worker (a worker is only ever lost, never stuck).
    pub request_timeout: Duration,
    /// yt-dlp `--extractor-args youtube:<key>=<values>` for the workers, e.g.
    /// `("player_skip", ["webpage", "initial_data"])`. Empty = yt-dlp's own defaults.
    pub youtube_args: Vec<(String, Vec<String>)>,
    /// Arguments for a request somebody is waiting on (a click), where speed beats the very best
    /// stream. `None` = the same as `youtube_args`. A click that fails with these is retried once
    /// with `youtube_args`.
    pub foreground_youtube_args: Option<Vec<(String, Vec<String>)>>,
}

impl WarmConfig {
    /// Production defaults: a click skips yt-dlp's watch-page and initial-data requests (about
    /// 1.5 s faster, measured on the owner's PC) at the price of the plain ~130 kbps stream;
    /// everything resolved ahead of time (queue, hover, page open) keeps yt-dlp's defaults, which
    /// give the Premium ~256 kbps stream. See docs/adr/0004-warm-resolver-workers.md.
    pub fn fast_clicks() -> Self {
        Self {
            foreground_youtube_args: Some(vec![(
                "player_skip".to_owned(),
                vec!["webpage".to_owned(), "initial_data".to_owned()],
            )]),
            ..Self::default()
        }
    }

    pub fn with_youtube_arg(mut self, key: &str, values: &[&str]) -> Self {
        self.youtube_args
            .push((key.to_owned(), values.iter().map(|v| (*v).to_owned()).collect()));
        self
    }
}

impl Default for WarmConfig {
    fn default() -> Self {
        Self {
            max_workers: 3,
            background_limit: 2,
            prewarm: 2,
            startup_timeout: Duration::from_secs(30),
            request_timeout: Duration::from_secs(30),
            youtube_args: Vec::new(),
            foreground_youtube_args: None,
        }
    }
}

/// Memory-only reuse of resolved stream URLs. A signed googlevideo URL stays valid for hours;
/// reusing it skips the whole resolution.
#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub max_entries: usize,
    /// A cached URL must stay valid at least this much longer than "now", so a track that
    /// starts from the cache can also be played through.
    pub min_remaining: Duration,
    /// How long a URL with no reported expiry may be reused.
    pub max_unknown_age: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 64,
            min_remaining: Duration::from_secs(15 * 60),
            max_unknown_age: Duration::from_secs(5 * 60),
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
    /// `Some` = keep warm yt-dlp workers (falls back to a process per track when they cannot
    /// run). `None` = the original behaviour: one process per track.
    pub warm: Option<WarmConfig>,
    /// `Some` = reuse resolved URLs (and share one resolution between callers asking for the
    /// same track). `None` = every request resolves.
    pub cache: Option<CacheConfig>,
    /// `None` skips verification (tests only).
    pub pinned: Option<PinnedTools>,
    /// Extra environment for the helper. Only exists in test builds.
    #[cfg(feature = "test-support")]
    pub extra_env: Vec<(String, String)>,
}

impl ResolverConfig {
    /// Production settings: tools under `tools_dir` (`yt-dlp/yt-dlp.exe`, `deno/deno.exe`),
    /// pinned to the M0 baseline, production URL policy, 30 s timeout, warm workers and a
    /// resolved-URL cache. [`ResolverConfig::legacy`] turns both off.
    pub fn production(tools_dir: &Path, run_dir: PathBuf) -> Self {
        Self {
            ytdlp: tools_dir.join("yt-dlp").join("yt-dlp.exe"),
            deno: tools_dir.join("deno").join("deno.exe"),
            run_dir,
            timeout: Duration::from_secs(30),
            max_concurrent: 2,
            max_stdout_bytes: 8 * 1024 * 1024,
            url_policy: UrlPolicy::production(),
            warm: Some(WarmConfig::fast_clicks()),
            cache: Some(CacheConfig::default()),
            pinned: Some(PinnedTools::m0_baseline()),
            #[cfg(feature = "test-support")]
            extra_env: Vec::new(),
        }
    }

    /// The original behaviour (a fresh yt-dlp per track, nothing reused): the "before" in
    /// timing comparisons and an escape hatch.
    pub fn legacy(mut self) -> Self {
        self.warm = None;
        self.cache = None;
        self
    }
}
