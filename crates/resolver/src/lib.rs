//! yt-dlp stream resolver (BUILD_SPEC section 9). Produces validated, memory-only stream leases
//! for the media engine. The pure parts (argument building, JSON parsing, failure classes, the
//! URL cache, the warm worker pool) build everywhere; the contained per-track runner is
//! Windows-only.

pub mod argv;
pub mod cache;
pub mod config;
pub mod parse;
pub mod warm;
#[cfg(windows)]
pub mod diag;
mod resolver;
#[cfg(windows)]
mod runner;

pub use config::{CacheConfig, PinnedTools, ResolverConfig, WarmConfig};
pub use resolver::{ResolverError, ResolverHandle, YtDlpResolver};
