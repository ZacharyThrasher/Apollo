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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub id: BrowseId,
    pub title: String,
    /// The service's second line without its redundant kind label ("Album • Artist • 2013"
    /// becomes "Artist • 2013"), `None` when nothing is left.
    pub subtitle: Option<String>,
    pub thumbnail: Option<String>,
    pub explicit: bool,
}

/// The best match for a query: the card the service leads its unfiltered answer with. Its row
/// is also first in its own group, so `songs`, `albums`, `artists` and `playlists` stay complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TopResult {
    Song(ListedTrack),
    Album(Hit),
    Artist(Hit),
    Playlist(Hit),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchResults {
    /// `None` on a filtered page and when the service named no best match.
    pub top: Option<TopResult>,
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

    /// The same results without the songs that are music videos (a top result that is one
    /// goes too).
    pub fn without_videos(&self) -> SearchResults {
        let mut out = self.clone();
        out.songs
            .retain(|s| !self.video_ids.contains(s.track.video_id.as_str()));
        if matches!(&out.top, Some(TopResult::Song(s)) if self.video_ids.contains(s.track.video_id.as_str()))
        {
            out.top = None;
        }
        out
    }

    /// Add what a later page of the same list found, keeping the first copy of any repeat
    /// (the service sometimes carries a row over a page boundary).
    pub fn append(&mut self, more: SearchResults) {
        for song in more.songs {
            if !self
                .songs
                .iter()
                .any(|s| s.track.video_id == song.track.video_id)
            {
                self.songs.push(song);
            }
        }
        for (list, extra) in [
            (&mut self.albums, more.albums),
            (&mut self.artists, more.artists),
            (&mut self.playlists, more.playlists),
        ] {
            for hit in extra {
                if !list.iter().any(|h| h.id == hit.id) {
                    list.push(hit);
                }
            }
        }
        self.skipped = self.skipped.saturating_add(more.skipped);
        self.video_ids.extend(more.video_ids);
    }
}

/// What a filtered search lists: one kind of result, paged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchFilter {
    Songs,
    Albums,
    Artists,
    Playlists,
    CommunityPlaylists,
}

impl SearchFilter {
    pub(crate) fn params(self) -> &'static str {
        match self {
            SearchFilter::Songs => innertube::endpoints::FILTER_SONG,
            SearchFilter::Albums => innertube::endpoints::FILTER_ALBUM,
            SearchFilter::Artists => innertube::endpoints::FILTER_ARTIST,
            SearchFilter::Playlists => innertube::endpoints::FILTER_FEATURED_PLAYLIST,
            SearchFilter::CommunityPlaylists => innertube::endpoints::FILTER_COMMUNITY_PLAYLIST,
        }
    }
}

/// One page of a filtered search and the token for the next, `None` on the last page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchPage {
    pub results: SearchResults,
    pub continuation: Option<String>,
}

/// Map a filtered page (or a continuation of one).
pub fn search_page(parsed: &innertube::SearchPage) -> SearchPage {
    SearchPage {
        results: group(parsed.items.iter()),
        continuation: parsed.continuation.clone(),
    }
}

/// Group a parsed search response. The "top result" card and its related rows come first in
/// their own kind's group; a row that appears twice (card and list) is kept once.
pub fn search_results(parsed: &innertube::SearchResults) -> SearchResults {
    let rows = [
        &parsed.top,
        &parsed.songs,
        &parsed.albums,
        &parsed.artists,
        &parsed.playlists,
    ];
    let mut out = group(rows.into_iter().flatten());
    // The service's first card row is the best match; a card that could not be read is no match.
    out.top = parsed.top.first().and_then(|item| match item.kind {
        "song" => track_from_item(item).map(TopResult::Song),
        "album" => hit_from_item(item).map(TopResult::Album),
        "artist" => hit_from_item(item).map(TopResult::Artist),
        "playlist" => hit_from_item(item).map(TopResult::Playlist),
        _ => None,
    });
    out
}

fn group<'a>(items: impl Iterator<Item = &'a BrowseItem>) -> SearchResults {
    let mut out = SearchResults::default();
    // `seen` makes a later flat row that repeats an earlier one a no-op.
    let mut seen: Vec<(&'static str, &str)> = Vec::new();
    for item in items {
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
