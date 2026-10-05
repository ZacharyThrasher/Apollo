//! The port the library screen reads through. Blocking by design: the screen calls it from
//! GPUI's background executor, never on the UI thread, and nothing here touches GPUI types.

use apollo_domain::library::{
    bare_playlist_id, AddedRow, Collection, CollectionKind, PageMeta, PlaylistEdit, PlaylistTarget, Privacy,
    WriteFailure,
};
use apollo_domain::listing::{ListItem, LoadFailure, Page};
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{
    ArtistSection, ExplorePage, HistoryDay, HomePage, HomeShelf, Hit, ListedTrack, RelatedArtist,
};

use crate::art::DecodedArt;

/// The public video the editable-playlist list is asked about: the sheet is per video, and only
/// its list of playlists is read. Read-only; it adds the video nowhere. The playlist picker asks
/// about it too when it is opened to choose a target playlist with no song at hand.
pub const PROBE_VIDEO: &str = "dQw4w9WgXcQ";

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

    /// First page of a collection's tracks with what the page says about editing and saving it.
    /// A source that has nothing to say about that answers with the default (nothing editable).
    fn page(&self, collection: &Collection) -> Result<(Page<TrackRow>, PageMeta), LoadFailure> {
        Ok((self.tracks(collection)?, PageMeta::default()))
    }

    /// A following page, by the continuation token the previous page carried.
    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure>;

    /// An artist page: top songs and album rows, all in one answer.
    fn artist(&self, id: &BrowseId) -> Result<ArtistContent, LoadFailure>;

    /// First page of the songs saved to the library (the Songs tab), most recently added first;
    /// the next pages come from [`Self::more_tracks`] by the page's continuation token.
    fn library_songs(&self) -> Result<Page<TrackRow>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The artists behind the account's library, in the service's order (the Artists tab).
    fn library_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The artists the account subscribes to, in the service's order (the data export). Not
    /// the Artists tab's list, which is the artists behind the saved songs.
    fn subscribed_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
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

    /// The home feed filtered to a mood chip (`params` is a chip's token from an earlier home
    /// answer).
    fn home_chip(&self, _params: &str) -> Result<HomePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The batch of home shelves after the one a token came from.
    fn home_more(&self, _token: &str) -> Result<HomePage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The Related tab of a track: its related songs first, then album, artist and playlist
    /// shelves, in the service's order. A track the service has no related tab for gives none.
    fn related(&self, _video_id: &str) -> Result<Vec<HomeShelf>, LoadFailure> {
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

    /// Tell the service the account is not interested in a track (the thumbs-down). Sent once,
    /// never retried: when the outcome is unknown the caller reads [`Self::disliked`] back. A
    /// liked track is no longer liked afterwards.
    fn set_disliked(&self, _video_id: &str) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Whether the account has thumbs-downed a track; `None` when the service did not say.
    fn disliked(&self, _video_id: &str) -> Result<Option<bool>, LoadFailure> {
        Ok(None)
    }

    /// Subscribe to or unsubscribe from an artist (`UC…` channel id). Sent once, never retried:
    /// when the outcome is unknown the caller reads the artist page back.
    fn set_subscribed(&self, _channel_id: &str, _subscribed: bool) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Whether the library holds a song, as the last row listing it said. `None` when no row has
    /// offered a way to change that, which is when the menu leaves the item out. Cheap and
    /// local: the menu asks it every time it opens.
    fn song_saved(&self, _video_id: &str) -> Option<bool> {
        None
    }

    /// Add a song to the library (`saved`) or take it out. Sent once, never retried: when the
    /// outcome is unknown the caller reads Library > Songs back.
    fn set_song_saved(&self, _video_id: &str, _saved: bool) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// The library is now known to hold the song (`saved`) or not, because Library > Songs was
    /// read back, not because this source wrote it. Lets the source keep what it offers true.
    fn note_song_saved(&self, _video_id: &str, _saved: bool) {}

    /// The playlists the account can add a track to, each flagged when it already holds it.
    fn playlists_for(&self, _video_id: &str) -> Result<Vec<PlaylistTarget>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// Add a track to a playlist the account owns (the bare id of a [`PlaylistTarget`]). A track
    /// already there is not added twice.
    fn add_to_playlist(&self, _playlist_id: &str, _video_id: &str) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Add several tracks to a playlist the account owns, in order, skipping any it holds. A source
    /// that can send them as one request should; this default adds them one by one.
    fn add_many_to_playlist(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<(), WriteFailure> {
        video_ids
            .iter()
            .try_for_each(|video| self.add_to_playlist(playlist_id, video))
    }

    /// Add tracks to a playlist the account owns, in order, skipping any it holds, and say which
    /// rows that made (a skipped duplicate makes none; an answer that does not name the rows
    /// gives none). One track goes as [`Self::add_to_playlist`], several as
    /// [`Self::add_many_to_playlist`]. What the rows are lets an add be taken back exactly.
    fn add_to_playlist_rows(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        match video_ids {
            [video] => self.add_to_playlist(playlist_id, video)?,
            _ => self.add_many_to_playlist(playlist_id, video_ids)?,
        }
        Ok(Vec::new())
    }

    /// Add tracks to a playlist the account owns even if it holds them (putting back a song a
    /// removal took out of a playlist that has it twice), in order. Sent once, never retried: a
    /// lost reply is settled by reading the playlist. The rows it made come back when the answer
    /// names them.
    fn add_again_to_playlist(
        &self,
        _playlist_id: &str,
        _video_ids: &[String],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    // ---- playlist editing -------------------------------------------------------------------
    // None of these is retried by the source: a timeout is an unknown outcome
    // ([`WriteFailure::Unknown`]) and `library::edits` reads the state back to settle it.

    /// Names the account the next write would go to (the signed-in session and its channel), or
    /// `None` when a source has no accounts. `library::edits` compares it with what it saw when
    /// the write was asked for and sends nothing when they differ, so a write never lands on an
    /// account other than the one the person acted in.
    fn account_token(&self) -> Option<String> {
        None
    }

    /// The bare ids of the playlists the account can edit (the add-to-playlist sheet's list).
    fn editable_playlists(&self) -> Result<Vec<String>, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The id a save to the library of `collection` is made on: a playlist's bare id; an album's
    /// audio playlist, which takes reading its page.
    fn save_target(&self, collection: &Collection) -> Result<String, LoadFailure> {
        match collection.kind {
            CollectionKind::Playlist => Ok(bare_playlist_id(collection.id.as_str()).to_owned()),
            CollectionKind::Album => Err(LoadFailure::Other),
        }
    }

    /// Create an empty private playlist; the new playlist's bare id.
    fn create_playlist(&self, _title: &str) -> Result<String, WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Create a playlist that already holds `video_ids`, in order; the new playlist's bare id.
    /// One write, so the playlist and its songs land together or not at all.
    fn create_playlist_with(
        &self,
        _title: &str,
        _description: &str,
        _privacy: Privacy,
        _video_ids: &[String],
    ) -> Result<String, WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    fn delete_playlist(&self, _playlist_id: &str) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Change a playlist's title, description and visibility: only what `edit` carries.
    fn edit_playlist(&self, _playlist_id: &str, _edit: &PlaylistEdit) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Remove one row of a playlist (the row's own occurrence token names it).
    fn remove_from_playlist(
        &self,
        _playlist_id: &str,
        _video_id: &str,
        _occurrence: &str,
    ) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Move one row in front of another; `before` `None` moves it to the end.
    fn move_in_playlist(
        &self,
        _playlist_id: &str,
        _occurrence: &str,
        _before: Option<&str>,
    ) -> Result<(), WriteFailure> {
        Err(WriteFailure::Unsupported)
    }

    /// Save a playlist or album to the library (`saved`) or take it out, by its save target.
    fn set_saved(&self, _save_id: &str, _saved: bool) -> Result<(), WriteFailure> {
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
