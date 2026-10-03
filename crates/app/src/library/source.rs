//! The port the library screen reads through. Blocking by design: the screen calls it from
//! GPUI's background executor, never on the UI thread, and nothing here touches GPUI types.

use apollo_domain::library::Collection;
use apollo_domain::listing::{ListItem, LoadFailure, Page};
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::ArtistSection;

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

/// What an artist page shows: the top songs and the rows of albums, singles and playlists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistContent {
    pub name: String,
    /// The service's header picture (often wide); the card the page was opened from usually
    /// carries a squarer one, which the page prefers.
    pub thumbnail: Option<String>,
    pub top_songs: Vec<TrackRow>,
    pub sections: Vec<ArtistSection>,
    /// Songs or cards the page carried that could not be shown.
    pub skipped: u32,
}

impl ArtistContent {
    pub fn is_empty(&self) -> bool {
        self.top_songs.is_empty() && self.sections.is_empty()
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

    /// An artist page: top songs and album rows, all in one answer.
    fn artist(&self, id: &BrowseId) -> Result<ArtistContent, LoadFailure>;

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
