//! Pure-Rust InnerTube metadata client: transport, client identity, models and endpoints.
//!
//! Vendored from Limusic (GPL-3.0-or-later) and trimmed; see NOTICE.md. The crate knows nothing
//! about UI, webviews, mpv or the OS and is testable against JSON fixtures with no network.
//! Streaming is deliberately absent: audio is resolved by yt-dlp. The `player` endpoint is here
//! for two things only: the track's loudness (`InnerTube::player_loudness`) and the URLs a play is
//! reported to (`InnerTube::player_tracking`, the pings in [`tracking`]).

pub mod clients;
pub mod endpoints;
pub mod models;
pub mod tracking;
pub mod transport;

pub use clients::{Clients, YouTubeClient, METADATA_CLIENT};
pub use models::browse::{
    AlbumPage, ArtistCarousel, ArtistPage, BrowseItem, HistoryGroup, HomePage, Mood, MoodSection,
    PlaylistContinuation, PlaylistOption, PlaylistPage, PlaylistSort, QuerySuggestion, SearchPage,
    SearchResults,
    SearchSuggestions, Section, SortMenu,
};
pub use models::context::Locale;
pub use models::player::PlayerLoudness;
pub use tracking::{new_nonce, PingState, PlaybackTracking, WatchtimePing};
pub use models::metadata::{
    AccountIdentity, AccountInfo, NextResult, Rating, SearchResult, SongItem,
};
pub use transport::{cookie_sapisid, Error, InnerTube, Session};
