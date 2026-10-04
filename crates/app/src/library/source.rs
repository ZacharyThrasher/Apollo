//! The port the library screen reads through. Blocking by design: the screen calls it from
//! GPUI's background executor, never on the UI thread, and nothing here touches GPUI types.

use apollo_domain::library::{Collection, PlaylistTarget, WriteFailure};
use apollo_domain::listing::{ListItem, LoadFailure, Page};
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{ArtistSection, ExplorePage, HomePage};

use crate::art::DecodedArt;

/// One row of a collection's track list. `occurrence` is the row's own token: the same video
/// may sit in a playlist twice, so paging dedups on it and never on the video id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackRow {
    pub track: Track,
    pub occurrence: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
    /// Whether the account has liked the track, when the row said so.
    pub liked: Option<bool>,
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

    /// The account's home feed: its first batch of shelves and the token for the next batch.
    fn home(&self) -> Result<HomePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The batch of home shelves after the one a token came from.
    fn home_more(&self, _token: &str) -> Result<HomePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// Explore: new releases, the song chart, chart shelves and the mood tiles. Whatever the
    /// service would not give is left out; the call fails only when nothing at all came back.
    fn explore(&self) -> Result<ExplorePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The playlists of one mood or genre tile (the `params` of an [`ExplorePage`] tile).
    fn mood_playlists(&self, _params: &str) -> Result<Vec<Collection>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// What plays after `video_id` when the queue runs out: the song's radio, without the seed
    /// itself. Autoplay appends these to the queue.
    fn radio(&self, _video_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// Cover art for a thumbnail URL a collection or row carried.
    fn artwork(&self, url: &str) -> Option<DecodedArt>;

    // ---- writes ---------------------------------------------------------------------------
    // Blocking like the reads. Defaults say "not supported", so a source that only lists (a
    // test double) needs none of this.

    /// Like or unlike a track. Idempotent.
    fn set_liked(&self, _video_id: &str, _liked: bool) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Whether the account has liked a track; `None` when the service did not say.
    fn liked(&self, _video_id: &str) -> Result<Option<bool>, LoadFailure> {
        Ok(None)
    }

    /// The playlists the account can add a track to, each flagged when it already holds it.
    fn playlists_for(&self, _video_id: &str) -> Result<Vec<PlaylistTarget>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// Add a track to a playlist the account owns (the bare id of a [`PlaylistTarget`]). A track
    /// already there is not added twice.
    fn add_to_playlist(&self, _playlist_id: &str, _video_id: &str) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }
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
