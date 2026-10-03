//! What a library lists: the account's playlists and saved albums as openable collections.
//!
//! Pure types only. The provider maps service cards into [`Collection`]; the UI lists them and
//! opens one to show its tracks (see [`crate::listing`] for the paginated track list).

use crate::model::BrowseId;

/// Browse id of the account's Liked Music playlist (`LM` with the `VL` browse prefix).
pub const LIKED_MUSIC_BROWSE_ID: &str = "VLLM";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CollectionKind {
    Playlist,
    Album,
}

/// One entry of the library: enough to draw a row and to ask the provider for its tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collection {
    pub kind: CollectionKind,
    /// Browse id: `VL…` for a playlist, `MPRE…` for an album.
    pub id: BrowseId,
    pub title: String,
    /// The service's own second line (owner, track count, artist and year); shown as given.
    pub subtitle: Option<String>,
    /// Cover art URL, when the card carries one.
    pub thumbnail: Option<String>,
}

impl Collection {
    pub fn is_liked_music(&self) -> bool {
        self.kind == CollectionKind::Playlist && self.id.as_str() == LIKED_MUSIC_BROWSE_ID
    }
}
