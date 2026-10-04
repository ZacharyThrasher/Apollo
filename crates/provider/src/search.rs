//! Search: an unfiltered InnerTube search mapped to the domain's types, grouped by kind.
//!
//! The mapping is pure and tested against recorded responses. Unreadable rows are counted in
//! `skipped`, never silently dropped, so "nothing found" stays distinct from "could not read".

use std::collections::BTreeSet;

use apollo_domain::model::{ArtistRef, BrowseId, Track, VideoId};
use innertube::BrowseItem;

use crate::map::{parse_duration, ListedTrack};

/// An album, artist or playlist result. The id is the service's browse id; nothing here can be
/// played directly (the pages that open these come later).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Hit {
    pub id: BrowseId,
    pub title: String,
    /// The service's second line without its redundant kind label ("Album • Artist • 2013"
    /// becomes "Artist • 2013"), `None` when nothing is left.
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub songs: Vec<ListedTrack>,
    pub albums: Vec<Hit>,
    pub artists: Vec<Hit>,
    pub playlists: Vec<Hit>,
    /// Rows that came back but could not become a result (no usable id or title).
    pub skipped: u32,
    /// Video ids of `songs` rows that are music videos rather than the audio track.
    pub video_ids: BTreeSet<String>,
}

impl SearchResults {
    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }

    /// The same results without the songs that are music videos.
    pub fn without_videos(&self) -> SearchResults {
        let mut out = self.clone();
        out.songs
            .retain(|s| !self.video_ids.contains(s.track.video_id.as_str()));
        out
    }
}

/// Group a parsed search response. The "top result" card and its related rows come first in
/// their own kind's group; a row that appears twice (card and list) is kept once.
pub fn search_results(parsed: &innertube::SearchResults) -> SearchResults {
    let mut out = SearchResults::default();
    // `top` is first so the best match leads its group; `seen` makes the later flat row a no-op.
    let mut seen: Vec<(&'static str, &str)> = Vec::new();
    let rows = [
        &parsed.top,
        &parsed.songs,
        &parsed.albums,
        &parsed.artists,
        &parsed.playlists,
    ];
    for item in rows.into_iter().flatten() {
        if seen.contains(&(item.kind, item.id.as_str())) {
            continue;
        }
        seen.push((item.kind, item.id.as_str()));
        match item.kind {
            "song" => match track_from_item(item) {
                Some(track) => {
                    if item.is_video {
                        out.video_ids
                            .insert(track.track.video_id.as_str().to_owned());
                    }
                    out.songs.push(track)
                }
                None => out.skipped = out.skipped.saturating_add(1),
            },
            "album" | "artist" | "playlist" => match hit_from_item(item) {
                Some(hit) => match item.kind {
                    "album" => out.albums.push(hit),
                    "artist" => out.artists.push(hit),
                    _ => out.playlists.push(hit),
                },
                None => out.skipped = out.skipped.saturating_add(1),
            },
            _ => out.skipped = out.skipped.saturating_add(1),
        }
    }
    out
}

/// `None` when the row has no valid video id or no title: it can be neither played nor shown.
pub fn track_from_item(item: &BrowseItem) -> Option<ListedTrack> {
    let video_id = VideoId::parse(&item.id)?;
    if item.title.trim().is_empty() {
        return None;
    }
    let artists: Vec<ArtistRef> = if item.artist_runs.is_empty() {
        item.subtitle
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|name| {
                vec![ArtistRef {
                    name: name.to_owned(),
                    id: None,
                }]
            })
            .unwrap_or_default()
    } else {
        // Separators like " & " carry no id; only linked runs are artists.
        item.artist_runs
            .iter()
            .filter(|r| r.id.is_some())
            .map(|r| ArtistRef {
                name: r.text.clone(),
                id: r.id.clone().map(BrowseId::new),
            })
            .collect()
    };
    Some(ListedTrack {
        track: Track {
            video_id,
            title: item.title.clone(),
            artists,
            // A search row names its album only by id, and a `Track` album needs a title.
            album: None,
            duration: item.duration.as_deref().and_then(parse_duration),
        },
        occurrence: None,
        thumbnail: item.thumbnail.clone(),
        explicit: item.explicit,
        // A search response states no per-account field, `likeStatus` included.
        liked: None,
    })
}

pub(crate) fn hit_from_item(item: &BrowseItem) -> Option<Hit> {
    if item.id.trim().is_empty() || item.title.trim().is_empty() {
        return None;
    }
    Some(Hit {
        id: BrowseId::new(item.id.clone()),
        title: item.title.clone(),
        subtitle: item
            .subtitle
            .as_deref()
            .and_then(|s| strip_kind_label(s, item.kind)),
        thumbnail: item.thumbnail.clone(),
        explicit: item.explicit,
    })
}

/// Drops a leading "Album" / "Artist" / "Playlist" segment: the group heading already says it.
/// Other leading labels ("Single", "EP") carry information and stay.
pub(crate) fn strip_kind_label(subtitle: &str, kind: &str) -> Option<String> {
    let mut parts = subtitle.split(" • ").map(str::trim);
    let first = parts.next()?;
    let rest: Vec<&str> = parts.collect();
    let rest_text = rest.join(" • ");
    let text = if first.eq_ignore_ascii_case(kind) {
        rest_text
    } else {
        subtitle.trim().to_owned()
    };
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests;
