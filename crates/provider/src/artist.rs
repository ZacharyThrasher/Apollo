//! An artist page mapped to the domain's types: the name and picture, the top songs, the
//! album/single/playlist rows beneath them, the similar artists, the bio and the subscription.
//!
//! The mapping is pure and tested against fixtures. Rows that came back but cannot be used are
//! counted in `skipped`, never silently dropped. Carousels the page cannot open (music videos)
//! are left out on purpose: they are not failures.

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

/// Another artist the page links ("Fans might also like").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelatedArtist {
    pub id: BrowseId,
    pub name: String,
    pub thumbnail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistPage {
    pub id: BrowseId,
    pub name: String,
    /// The header picture (the service's own crop, usually wide).
    pub thumbnail: Option<String>,
    pub top_songs: Vec<ListedTrack>,
    /// The Top songs shelf's "Show all": a playlist page of every top song.
    pub top_songs_id: Option<BrowseId>,
    pub sections: Vec<ArtistSection>,
    /// Similar artists, in the service's order.
    pub related: Vec<RelatedArtist>,
    /// The artist's bio.
    pub description: Option<String>,
    /// "32.7M subscribers", as the service words it.
    pub subscribers: Option<String>,
    /// "137M monthly audience", as the service words it. Not every artist has one.
    pub monthly_listeners: Option<String>,
    /// The channel the Subscribe button acts on (`UC…`).
    pub channel_id: String,
    /// Whether the signed-in account is subscribed.
    pub subscribed: bool,
    /// The header's radio (`RDEM…`): a playlist to queue, not an id that can be built.
    pub radio_playlist_id: Option<String>,
    /// The header's Shuffle mix of the artist's songs (`RDAO…`).
    pub shuffle_playlist_id: Option<String>,
    /// Songs or cards the page carried that could not become a row.
    pub skipped: u32,
}

impl ArtistPage {
    /// The page read cleanly and holds nothing to show.
    pub fn is_empty(&self) -> bool {
        self.top_songs.is_empty() && self.sections.is_empty() && self.related.is_empty()
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
    let mut related: Vec<RelatedArtist> = Vec::new();
    for carousel in &page.sections {
        let mut items = Vec::new();
        for card in &carousel.items {
            let kind = match card.kind {
                "album" => CollectionKind::Album,
                "playlist" => CollectionKind::Playlist,
                "artist" => {
                    match related_from_card(card) {
                        Some(a) if related.iter().all(|r| r.id != a.id) => related.push(a),
                        Some(_) => {}
                        None => skipped = skipped.saturating_add(1),
                    }
                    continue;
                }
                // Music videos: not something this page opens.
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
        top_songs_id: page
            .top_songs_id
            .as_deref()
            .filter(|id| !id.trim().is_empty())
            .map(BrowseId::new),
        sections,
        related,
        description: non_empty(page.description.as_deref()),
        subscribers: non_empty(page.subscribers.as_deref()),
        monthly_listeners: non_empty(page.monthly_listeners.as_deref()),
        channel_id: non_empty(Some(&page.channel_id)).unwrap_or_else(|| id.to_owned()),
        subscribed: page.subscribed,
        radio_playlist_id: non_empty(page.radio_playlist_id.as_deref()),
        shuffle_playlist_id: non_empty(page.shuffle_playlist_id.as_deref()),
        skipped,
    };
    if mapped.is_empty() && mapped.skipped > 0 {
        return Err(LoadFailure::Parse);
    }
    Ok(mapped)
}

fn non_empty(text: Option<&str>) -> Option<String> {
    text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_owned)
}

/// An artist card as a link to that artist's page. Needs an id and a name to be told apart.
fn related_from_card(card: &innertube::BrowseItem) -> Option<RelatedArtist> {
    let name = non_empty(Some(&card.title))?;
    let id = non_empty(Some(&card.id))?;
    Some(RelatedArtist {
        id: BrowseId::new(id),
        name,
        thumbnail: non_empty(card.thumbnail.as_deref()),
    })
}

#[cfg(test)]
mod tests;
