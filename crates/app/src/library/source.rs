//! The port the library screen reads through. Blocking by design: the screen calls it from
//! GPUI's background executor, never on the UI thread, and nothing here touches GPUI types.

use apollo_domain::library::{Collection, PlaylistTarget, WriteFailure};
use apollo_domain::listing::{ListItem, LoadFailure, Page};
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{ArtistSection, HistoryDay, HomePage, Hit, ListedTrack, RelatedArtist};

use crate::art::DecodedArt;

/// One row of a collection's track list. `occurrence` is the row's own token: the same video
/// may sit in a playlist twice, so paging dedups on it and never on the video id.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrackRow {
    pub track: Track,
    pub occurrence: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
    /// Whether the account has liked the track, when the row said so.
    pub liked: Option<bool>,
}

impl From<ListedTrack> for TrackRow {
    fn from(listed: ListedTrack) -> Self {
        Self {
            track: listed.track,
            occurrence: listed.occurrence,
            thumbnail: listed.thumbnail,
            explicit: listed.explicit,
            liked: listed.liked,
        }
    }
}

impl ListItem for TrackRow {
    fn dedup_key(&self) -> Option<String> {
        self.occurrence.clone()
    }
}

/// What an artist page shows: the top songs, the rows of albums, singles and playlists, similar
/// artists, the bio and where the account's subscription stands.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtistContent {
    pub name: String,
    /// The service's header picture: wide, which is what the banner wants.
    pub thumbnail: Option<String>,
    pub top_songs: Vec<TrackRow>,
    /// The Top songs shelf's "Show all": a playlist page of every top song.
    pub top_songs_id: Option<BrowseId>,
    pub sections: Vec<ArtistSection>,
    /// "Fans might also like", in the service's order.
    pub related: Vec<RelatedArtist>,
    pub description: Option<String>,
    /// "32.7M subscribers", as the service words it.
    pub subscribers: Option<String>,
    /// "137M monthly audience", as the service words it.
    pub monthly_listeners: Option<String>,
    /// What Subscribe acts on (`UC…`). Empty on a page that cannot be subscribed to.
    pub channel_id: String,
    /// Whether the account is subscribed, as the service last said (or as a write confirmed).
    pub subscribed: bool,
    /// The header's radio and shuffle mixes: playlists to queue.
    pub radio_playlist_id: Option<String>,
    pub shuffle_playlist_id: Option<String>,
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

    /// The artists behind the account's library, in the service's order (the Artists tab).
    fn library_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The play history in the service's date buckets, most recent first (the History tab).
    fn history(&self) -> Result<Vec<HistoryDay>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The account's home feed: its first batch of shelves and the token for the next batch.
    fn home(&self) -> Result<HomePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The batch of home shelves after the one a token came from.
    fn home_more(&self, _token: &str) -> Result<HomePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// What plays after `video_id` when the queue runs out: the song's radio, without the seed
    /// itself. Autoplay appends these to the queue.
    fn radio(&self, _video_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The tracks of a mix that has no seed song (an artist's radio or shuffle), in play order.
    fn mix(&self, _playlist_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
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

    /// Subscribe to or unsubscribe from an artist (`UC…` channel id). Sent once, never retried:
    /// when the outcome is unknown the caller reads the artist page back.
    fn set_subscribed(&self, _channel_id: &str, _subscribed: bool) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
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
