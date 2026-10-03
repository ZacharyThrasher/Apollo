//! Metadata provider: adapts the vendored InnerTube client to the domain's types.
//!
//! The mapping is pure and tested against fixtures with no network. Nothing here decides
//! playback, touches credentials at rest or owns UI copy.

mod map;
mod search;
mod source;

pub use map::{
    account_validation, classify, collection_from_card, collections_from_grid, page_from_album,
    page_from_continuation, page_from_playlist, track_from_song, ListedTrack,
};
pub use search::{search_results, track_from_item, Hit, SearchResults};
pub use source::InnerTubeSource;
