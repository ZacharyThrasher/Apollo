//! Explore: new releases, charts and the mood and genre tiles, mapped to the domain's types.
//!
//! The page is assembled from four YouTube Music answers (`FEmusic_explore`, `FEmusic_charts`,
//! `FEmusic_new_releases_albums`, `FEmusic_moods_and_genres`), each of which may be missing: which
//! shelves exist depends on the account and the country, and some answers are refused to a guest.
//! So nothing here expects a particular shelf. A shelf is placed by what it holds and what it is
//! called, one that cannot be placed is a plain card shelf, and what could not be read is counted
//! in `skipped`, never silently dropped.
//!
//! The mapping is pure and tested against fixtures.

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::BrowseId;
use innertube::{BrowseItem, MoodSection, Section};

use crate::home::{item_from_card, HomeItem, HomeShelf};
use crate::map::{collection_from_card, ListedTrack};

/// A mood or genre tile: opens a page of playlists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoodTile {
    pub title: String,
    /// The token that browses the tile's playlists (see `InnerTubeSource::mood_playlists`).
    pub params: String,
}

/// Everything Explore shows, in the order it is shown.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExplorePage {
    /// New albums and singles, as cards.
    pub new_releases: Vec<Collection>,
    /// The song chart, best first.
    pub top_songs: Vec<ListedTrack>,
    /// The chart as a page of its own, when the service names one: where "Show all" goes.
    pub top_songs_all: Option<Collection>,
    /// Every other shelf of cards, in the service's order.
    pub shelves: Vec<HomeShelf>,
    pub moods: Vec<MoodTile>,
    /// Shelves and items the answers carried that could not be shown.
    pub skipped: u32,
}

impl ExplorePage {
    pub fn is_empty(&self) -> bool {
        self.new_releases.is_empty()
            && self.top_songs.is_empty()
            && self.shelves.is_empty()
            && self.moods.is_empty()
    }
}

/// A shelf with the page it opens ("Show all"), when the service names one.
struct Placed {
    shelf: HomeShelf,
    all: Option<Collection>,
    from_charts: bool,
}

/// Assemble the page from the answers that came back. `None` is an answer that did not.
pub fn explore_page(
    explore: Option<&innertube::HomePage>,
    charts: Option<&innertube::HomePage>,
    new_releases: Option<&[BrowseItem]>,
    moods: Option<&[MoodSection]>,
) -> ExplorePage {
    let mut skipped = 0u32;
    let mut placed: Vec<Placed> = Vec::new();
    for (parsed, from_charts) in [(explore, false), (charts, true)] {
        let Some(parsed) = parsed else {
            continue;
        };
        skipped = skipped.saturating_add(parsed.skipped_shelves);
        for section in &parsed.sections {
            skipped = skipped.saturating_add(section.skipped);
            match place(section, from_charts) {
                Some(p) => placed.push(p),
                None => skipped = skipped.saturating_add(1),
            }
        }
    }

    // The song chart: the best-named shelf of nothing but songs; the charts page wins a tie.
    let chart = placed
        .iter()
        .enumerate()
        .filter(|(_, p)| p.shelf.is_songs())
        .filter_map(|(i, p)| song_chart_rank(&p.shelf.title).map(|r| (r, !p.from_charts, i)))
        .min()
        .map(|(_, _, i)| i);
    let (mut top_songs, mut top_songs_all) = (Vec::new(), None);
    if let Some(i) = chart {
        let p = placed.remove(i);
        top_songs = p.shelf.songs().into_iter().cloned().collect();
        top_songs_all = p.all;
        // Another shelf of chart songs is the same list again.
        placed.retain(|p| !(p.shelf.is_songs() && song_chart_rank(&p.shelf.title).is_some()));
    }

    // New releases: the album endpoint's cards, else the explore shelf that carries them.
    let mut releases: Vec<Collection> = new_releases
        .unwrap_or_default()
        .iter()
        .filter_map(collection_of_card)
        .collect();
    let named = placed
        .iter()
        .position(|p| is_new_releases_title(&p.shelf.title));
    if let Some(i) = named {
        let p = placed.remove(i);
        if releases.is_empty() {
            releases = p
                .shelf
                .items
                .iter()
                .filter_map(|item| match item {
                    HomeItem::Collection(c) => Some(c.clone()),
                    _ => None,
                })
                .collect();
        }
    }
    dedupe(&mut releases, |c| c.id.as_str().to_owned());

    let mut shelves: Vec<HomeShelf> = Vec::new();
    for p in placed {
        if !shelves.iter().any(|seen| seen.title == p.shelf.title) {
            shelves.push(p.shelf);
        }
    }

    let mut tiles: Vec<MoodTile> = moods
        .unwrap_or_default()
        .iter()
        .flat_map(|section| &section.items)
        .filter(|m| !m.title.trim().is_empty() && !m.params.is_empty())
        .map(|m| MoodTile {
            title: m.title.trim().to_owned(),
            params: m.params.clone(),
        })
        .collect();
    dedupe(&mut tiles, |t| t.title.clone());

    ExplorePage {
        new_releases: releases,
        top_songs,
        top_songs_all,
        shelves,
        moods: tiles,
        skipped,
    }
}

/// The playlists of one mood tile, as cards.
pub fn mood_playlists(cards: &[BrowseItem]) -> Vec<Collection> {
    let mut out: Vec<Collection> = cards
        .iter()
        .filter_map(|c| collection_from_card(c, CollectionKind::Playlist))
        .collect();
    dedupe(&mut out, |c| c.id.as_str().to_owned());
    out
}

fn dedupe<T>(items: &mut Vec<T>, key: impl Fn(&T) -> String) {
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(key(item)));
}

/// An album or playlist card as a collection; any other kind opens no collection page.
fn collection_of_card(card: &BrowseItem) -> Option<Collection> {
    let kind = match card.kind {
        "album" => CollectionKind::Album,
        "playlist" => CollectionKind::Playlist,
        _ => return None,
    };
    let mut c = collection_from_card(card, kind)?;
    c.subtitle = card
        .subtitle
        .as_deref()
        .and_then(|s| crate::search::strip_kind_label(s, card.kind));
    Some(c)
}

fn place(section: &Section, from_charts: bool) -> Option<Placed> {
    let mut items: Vec<HomeItem> = Vec::with_capacity(section.items.len());
    for card in &section.items {
        if let Some(item) = item_from_card(card) {
            if !items.iter().any(|seen| seen.key() == item.key()) {
                items.push(item);
            }
        }
    }
    let title = section.title.trim();
    if items.is_empty() || title.is_empty() || is_off_topic(title, &items) {
        return None;
    }
    Some(Placed {
        all: section_page(section, title),
        shelf: HomeShelf {
            title: title.to_owned(),
            items,
        },
        from_charts,
    })
}

/// The page a shelf's "More" button opens, when it is a playlist or album page.
fn section_page(section: &Section, title: &str) -> Option<Collection> {
    let id = section.more_browse_id.as_deref()?;
    if section.more_params.is_some() {
        return None;
    }
    let kind = if id.starts_with("MPRE") {
        CollectionKind::Album
    } else if id.starts_with("VL") {
        CollectionKind::Playlist
    } else {
        return None;
    };
    Some(Collection {
        kind,
        id: BrowseId::new(id),
        title: title.to_owned(),
        subtitle: None,
        thumbnail: None,
    })
}

/// Podcasts and music videos are not what Explore is for: Apollo plays audio.
fn is_off_topic(title: &str, items: &[HomeItem]) -> bool {
    let mentions = |text: &str| {
        let t = text.to_lowercase();
        ["podcast", "episode", "video"].iter().any(|w| t.contains(w))
    };
    if mentions(title) {
        return true;
    }
    let named = items
        .iter()
        .filter(|item| match item {
            HomeItem::Collection(c) => mentions(&c.title),
            _ => false,
        })
        .count();
    named * 2 >= items.len()
}

/// How good a name a shelf of songs has for the song chart (lower is better).
fn song_chart_rank(title: &str) -> Option<u8> {
    let t = title.to_lowercase();
    if t.contains("top song") {
        Some(0)
    } else if t.contains("trending") {
        Some(1)
    } else if t.contains("top") || t.contains("chart") {
        Some(2)
    } else {
        None
    }
}

fn is_new_releases_title(title: &str) -> bool {
    let t = title.to_lowercase();
    t.contains("new") && ["release", "album", "single"].iter().any(|w| t.contains(w))
}

/// The worst of the failures: the one the screen should name.
pub fn worst(failures: impl IntoIterator<Item = LoadFailure>) -> Option<LoadFailure> {
    fn weight(f: LoadFailure) -> u8 {
        match f {
            LoadFailure::Session => 5,
            LoadFailure::Throttled => 4,
            LoadFailure::Network => 3,
            LoadFailure::Parse => 2,
            LoadFailure::Other => 1,
        }
    }
    failures.into_iter().max_by_key(|f| weight(*f))
}

#[cfg(test)]
mod tests;
