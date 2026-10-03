//! An artist page mapped to the domain's types: the name and picture, the top songs, and the
//! album/single/playlist rows beneath them.
//!
//! The mapping is pure and tested against fixtures. Rows that came back but cannot be used are
//! counted in `skipped`, never silently dropped. Carousels the page cannot open (videos, other
//! artists) are left out on purpose: they are not failures.

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::BrowseId;

use crate::map::{collection_from_card, track_from_song, ListedTrack};

/// A titled row of albums, singles or playlists ("Albums", "Singles", "Featured on", ...).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistSection {
    pub title: String,
    pub items: Vec<Collection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistPage {
    pub id: BrowseId,
    pub name: String,
    /// The header picture (the service's own crop, usually wide).
    pub thumbnail: Option<String>,
    pub top_songs: Vec<ListedTrack>,
    pub sections: Vec<ArtistSection>,
    /// Songs or cards the page carried that could not become a row.
    pub skipped: u32,
}

impl ArtistPage {
    /// The page read cleanly and holds nothing to show.
    pub fn is_empty(&self) -> bool {
        self.top_songs.is_empty() && self.sections.is_empty()
    }
}

/// `Parse` when the page has no name (a response that is not an artist page), or when rows
/// came back but none could be read.
pub fn page_from_artist(page: &innertube::ArtistPage, id: &str) -> Result<ArtistPage, LoadFailure> {
    let name = page
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .ok_or(LoadFailure::Parse)?
        .to_owned();
    let mut skipped = 0u32;
    let mut top_songs = Vec::with_capacity(page.top_songs.len());
    for song in &page.top_songs {
        match track_from_song(song) {
            Some(t) => top_songs.push(t),
            None => skipped = skipped.saturating_add(1),
        }
    }
    let mut sections = Vec::new();
    for carousel in &page.sections {
        let mut items = Vec::new();
        for card in &carousel.items {
            let kind = match card.kind {
                "album" => CollectionKind::Album,
                "playlist" => CollectionKind::Playlist,
                // Videos and other artists: not something this page opens.
                _ => continue,
            };
            match collection_from_card(card, kind) {
                Some(c) => items.push(c),
                None => skipped = skipped.saturating_add(1),
            }
        }
        if !items.is_empty() && !carousel.title.trim().is_empty() {
            sections.push(ArtistSection {
                title: carousel.title.clone(),
                items,
            });
        }
    }
    let mapped = ArtistPage {
        id: BrowseId::new(id),
        name,
        thumbnail: page
            .thumbnail
            .as_ref()
            .filter(|t| !t.trim().is_empty())
            .cloned(),
        top_songs,
        sections,
        skipped,
    };
    if mapped.is_empty() && mapped.skipped > 0 {
        return Err(LoadFailure::Parse);
    }
    Ok(mapped)
}

#[cfg(test)]
mod tests;
