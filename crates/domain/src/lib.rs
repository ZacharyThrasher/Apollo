//! Pure domain types and state rules. No GPUI, InnerTube JSON, yt-dlp or mpv
//! types may appear here (BUILD_SPEC ARCH-01).

pub mod account;
pub mod audio;
pub mod lease;
pub mod library;
pub mod links;
pub mod listing;
pub mod loudness;
pub mod model;
pub mod playback;
pub mod queue;
pub mod undo;

#[cfg(test)]
mod fuzz_tests;
