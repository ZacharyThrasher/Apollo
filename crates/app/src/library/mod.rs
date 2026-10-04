//! The library: what the account owns (Liked Music, playlists, saved albums) and the screen
//! state for browsing it. [`source::LibrarySource`] is the port the screen reads through; the
//! simulated backend and the InnerTube provider implement it.

pub mod browse;
pub mod edits;
pub mod filter;
pub mod model;
pub mod sim;
pub mod source;
pub mod writes;

#[cfg(feature = "provider")]
pub mod live;

pub use model::{ArtistDetail, ArtistTarget, Detail, Load, PlayRequest, Tab, Target};
pub use source::{ArtistContent, LibrarySource, TrackRow};
pub use edits::{EditEvent, LibraryEdits};
pub use writes::{LibraryWrites, Notice, WritesEvent};
