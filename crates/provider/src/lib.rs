//! Metadata provider: adapts the vendored InnerTube client to the domain's types.
//!
//! The mapping is pure and tested against fixtures with no network. Nothing here decides
//! playback, touches credentials at rest or owns UI copy.

mod artist;
mod home;
mod map;
mod search;
mod source;

pub use artist::{page_from_artist, ArtistPage, ArtistSection, RelatedArtist};
pub use home::{home_page, HomeItem, HomePage, HomeShelf};
pub use map::{
    account_validation, classify, classify_write, collection_from_card, collections_from_grid,
    history_days, liked_from_rating, HistoryDay, page_from_album, page_from_continuation, page_from_playlist,
    target_from_option, track_from_song, ListedTrack,
};
pub use search::{search_results, track_from_item, Hit, SearchResults};
pub use source::{InnerTubeSource, LinkTarget};
