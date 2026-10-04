//! What a library lists: the account's playlists and saved albums as openable collections.
//!
//! Pure types only. The provider maps service cards into [`Collection`]; the UI lists them and
//! opens one to show its tracks (see [`crate::listing`] for the paginated track list).

use crate::model::BrowseId;

/// Browse id of the account's Liked Music playlist (`LM` with the `VL` browse prefix).
pub const LIKED_MUSIC_BROWSE_ID: &str = "VLLM";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum CollectionKind {
    Playlist,
    Album,
}

/// One entry of the library: enough to draw a row and to ask the provider for its tracks.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Why a library write (a like, an add to a playlist) did not happen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteFailure {
    /// The service no longer accepts the session.
    Session,
    Throttled,
    /// The service answered and said no: the write did not take effect.
    Rejected,
    /// No usable answer (a timeout, a dropped connection, an unreadable reply): the write may
    /// or may not have landed, so it is read back before anything is believed.
    Unknown,
    /// This backend cannot write.
    Unsupported,
}

impl WriteFailure {
    /// True when the write might have taken effect anyway.
    pub fn outcome_unknown(self) -> bool {
        self == WriteFailure::Unknown
    }
}

/// A playlist a track can be added to (one the account may edit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistTarget {
    /// The bare playlist id (`PL…`), as the edit takes it; not the `VL…` browse id.
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    /// The playlist already holds the track the list was asked for.
    pub holds_track: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_unknown_outcome_needs_reading_back() {
        assert!(WriteFailure::Unknown.outcome_unknown());
        for f in [
            WriteFailure::Session,
            WriteFailure::Throttled,
            WriteFailure::Rejected,
            WriteFailure::Unsupported,
        ] {
            assert!(!f.outcome_unknown(), "{f:?}");
        }
    }
}
