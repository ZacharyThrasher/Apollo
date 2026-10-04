//! Metadata provider: adapts the vendored InnerTube client to the domain's types.
//!
//! The mapping is pure and tested against fixtures with no network. Nothing here decides
//! playback, touches credentials at rest or owns UI copy.

mod artist;
mod explore;
mod home;
mod map;
mod marks;
mod search;
mod source;

pub use artist::{page_from_artist, ArtistPage, ArtistSection, RelatedArtist};
pub use explore::{explore_page, mood_playlists, worst, ExplorePage, MoodTile};
pub use home::{home_page, HomeItem, HomePage, HomeShelf};
pub use map::{
    account_validation, classify, classify_write, collection_from_card, collections_from_grid,
    history_days, liked_from_rating, meta_from_album, meta_from_playlist, page_from_album,
    page_from_continuation, page_from_playlist, HistoryDay,
    target_from_option, track_from_song, ListedTrack,
};
pub use search::{
    search_page, search_results, track_from_item, Hit, SearchFilter, SearchPage, SearchResults,
    TopResult,
};
pub use marks::LibraryMarks;
pub use source::{InnerTubeSource, LinkTarget};

/// The play-reporting types the source speaks, so callers need not depend on the InnerTube crate.
pub use innertube::{new_nonce, PingState, PlaybackTracking, WatchtimePing};
