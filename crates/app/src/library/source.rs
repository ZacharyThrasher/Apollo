//! The port the library screen reads through. Blocking by design: the screen calls it from
//! GPUI's background executor, never on the UI thread, and nothing here touches GPUI types.

use apollo_domain::library::Collection;
use apollo_domain::listing::{ListItem, LoadFailure, Page};
use apollo_domain::model::Track;

use crate::art::DecodedArt;

/// One row of a collection's track list. `occurrence` is the row's own token: the same video
/// may sit in a playlist twice, so paging dedups on it and never on the video id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackRow {
    pub track: Track,
    pub occurrence: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
}

impl ListItem for TrackRow {
    fn dedup_key(&self) -> Option<String> {
        self.occurrence.clone()
    }
}

pub trait LibrarySource: Send + Sync {
    /// Every library playlist, then every saved album, each in the service's order. Liked Music
    /// may or may not be among the playlists; the screen guarantees it appears exactly once.
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure>;

    /// First page of a collection's tracks.
    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure>;

    /// A following page, by the continuation token the previous page carried.
    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure>;

    /// Cover art for a thumbnail URL a collection or row carried.
    fn artwork(&self, url: &str) -> Option<DecodedArt>;
}

/// What the screen says for a failed load: what happened in six words or fewer.
pub fn failure_text(failure: LoadFailure) -> &'static str {
    match failure {
        LoadFailure::Session => "Sign-in expired",
        LoadFailure::Network => "Connection problem",
        LoadFailure::Throttled => "Too many requests",
        LoadFailure::Parse | LoadFailure::Other => "Couldn't load this",
    }
}
