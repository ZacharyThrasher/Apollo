use std::time::Duration;

/// Opaque source video ID. Never a display name or URL.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VideoId(String);

impl VideoId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Opaque browse (album/artist) ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BrowseId(String);

impl BrowseId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

/// Identifies one *occurrence* of a track in the queue. The same video queued
/// three times yields three distinct IDs. IDs are minted by [`crate::queue::Queue`]
/// and never reused within a queue's lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QueueEntryId(pub(crate) u64);

impl QueueEntryId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Artist and album names are not identifiers; the ID is optional because
/// not every source reports one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistRef {
    pub name: String,
    pub id: Option<BrowseId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlbumRef {
    pub title: String,
    pub id: Option<BrowseId>,
}

/// Unknown values stay `None` rather than becoming zero/empty placeholders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub video_id: VideoId,
    pub title: String,
    pub artists: Vec<ArtistRef>,
    pub album: Option<AlbumRef>,
    pub duration: Option<Duration>,
}

impl Track {
    pub fn new(video_id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            video_id: VideoId::new(video_id),
            title: title.into(),
            artists: Vec::new(),
            album: None,
            duration: None,
        }
    }
}
