//! The home feed (`FEmusic_home`) mapped to the domain's types: titled shelves of songs, albums,
//! playlists and artists, in the service's order, plus the token for the next batch of shelves.
//!
//! The mapping is pure and tested against fixtures. A shelf kind the parser does not read and an
//! item it cannot use are counted in `skipped`, never silently dropped, so an empty feed stays
//! distinct from an unreadable one.

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::LoadFailure;
use innertube::BrowseItem;

use crate::map::{collection_from_card, ListedTrack};
use crate::search::{hit_from_item, track_from_item, Hit};

/// One thing on a shelf.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HomeItem {
    /// Plays.
    Song(ListedTrack),
    /// An album, single or playlist: opens its page.
    Collection(Collection),
    /// Opens the artist's page.
    Artist(Hit),
}

impl HomeItem {
    /// What the item is, with its id: two items of one shelf are the same when this matches.
    pub(crate) fn key(&self) -> (&'static str, String) {
        match self {
            Self::Song(t) => ("song", t.track.video_id.as_str().to_owned()),
            Self::Collection(c) => ("collection", c.id.as_str().to_owned()),
            Self::Artist(h) => ("artist", h.id.as_str().to_owned()),
        }
    }

    /// The artwork the item shows.
    pub fn thumbnail(&self) -> Option<&str> {
        match self {
            Self::Song(t) => t.thumbnail.as_deref(),
            Self::Collection(c) => c.thumbnail.as_deref(),
            Self::Artist(h) => h.thumbnail.as_deref(),
        }
    }
}

/// A titled row ("Quick picks", "Listen again", "Albums for you").
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomeShelf {
    pub title: String,
    pub items: Vec<HomeItem>,
}

impl HomeShelf {
    /// Nothing but songs: the shelf is drawn as rows of songs rather than cards.
    pub fn is_songs(&self) -> bool {
        !self.items.is_empty() && self.items.iter().all(|i| matches!(i, HomeItem::Song(_)))
    }

    /// The shelf's songs in order: what clicking one of them queues.
    pub fn songs(&self) -> Vec<&ListedTrack> {
        self.items
            .iter()
            .filter_map(|i| match i {
                HomeItem::Song(t) => Some(t),
                _ => None,
            })
            .collect()
    }
}

/// A mood chip above the home feed ("Relax", "Workout"): asking the feed again with its `params`
/// gives that mood's feed.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomeChip {
    pub title: String,
    pub params: String,
}

/// One answer of the home feed: a batch of shelves and the way to the next batch.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HomePage {
    /// The mood chips the service offered with this answer, in its order. Saved pages from before
    /// chips existed read as having none.
    #[serde(default)]
    pub chips: Vec<HomeChip>,
    pub shelves: Vec<HomeShelf>,
    pub continuation: Option<String>,
    /// Shelves and items the answer carried that could not be shown.
    pub skipped: u32,
}

/// The id prefix of a podcast show: it opens a show page, not a track list, and is not music.
const PODCAST_SHOW_PREFIX: &str = "MPSP";

/// Map a parsed home response. `Parse` when shelves came back but none could be read.
pub fn home_page(parsed: &innertube::HomePage) -> Result<HomePage, LoadFailure> {
    let mut skipped = parsed.skipped_shelves;
    let mut shelves = Vec::with_capacity(parsed.sections.len());
    for section in &parsed.sections {
        skipped = skipped.saturating_add(section.skipped);
        let mut items: Vec<HomeItem> = Vec::with_capacity(section.items.len());
        for card in &section.items {
            match item_from_card(card) {
                Some(item) => {
                    if !items.iter().any(|seen| seen.key() == item.key()) {
                        items.push(item);
                    }
                }
                None => skipped = skipped.saturating_add(1),
            }
        }
        // A shelf with no title or nothing readable would be a bare heading or an empty row.
        if items.is_empty() || section.title.trim().is_empty() {
            skipped = skipped.saturating_add(1);
            continue;
        }
        shelves.push(HomeShelf {
            title: section.title.trim().to_owned(),
            items,
        });
    }
    if shelves.is_empty() && skipped > 0 {
        return Err(LoadFailure::Parse);
    }
    Ok(HomePage {
        chips: chips_of(parsed),
        shelves,
        continuation: parsed.continuation.clone().filter(|t| !t.trim().is_empty()),
        skipped,
    })
}

/// The chips a response carried that Apollo shows: podcasts are not music here (their shows are
/// dropped from every shelf too), so that chip is left out.
fn chips_of(parsed: &innertube::HomePage) -> Vec<HomeChip> {
    parsed
        .chips
        .iter()
        .filter(|c| !c.title.trim().eq_ignore_ascii_case("podcasts") && !c.title.trim().is_empty())
        .filter(|c| !c.params.trim().is_empty())
        .map(|c| HomeChip {
            title: c.title.trim().to_owned(),
            params: c.params.clone(),
        })
        .collect()
}

pub(crate) fn item_from_card(card: &BrowseItem) -> Option<HomeItem> {
    match card.kind {
        "song" => track_from_item(card).map(HomeItem::Song),
        "album" | "playlist" if card.id.starts_with(PODCAST_SHOW_PREFIX) => None,
        "album" | "playlist" => {
            let kind = if card.kind == "album" {
                CollectionKind::Album
            } else {
                CollectionKind::Playlist
            };
            let mut collection = collection_from_card(card, kind)?;
            collection.subtitle = card
                .subtitle
                .as_deref()
                .and_then(|s| crate::search::strip_kind_label(s, card.kind));
            Some(HomeItem::Collection(collection))
        }
        "artist" => hit_from_item(card).map(HomeItem::Artist),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
