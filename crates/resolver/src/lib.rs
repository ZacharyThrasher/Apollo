//! yt-dlp stream resolver (BUILD_SPEC section 9). Produces validated, memory-only stream leases
//! for the media engine. The pure parts (argument building, JSON parsing, failure classes) build
//! everywhere; the contained process runner and the resolver itself are Windows-only.

pub mod argv;
pub mod config;
pub mod parse;
#[cfg(windows)]
pub mod diag;
#[cfg(windows)]
mod resolver;
#[cfg(windows)]
mod runner;

pub use config::{PinnedTools, ResolverConfig};
#[cfg(windows)]
pub use resolver::{ResolverError, YtDlpResolver};
