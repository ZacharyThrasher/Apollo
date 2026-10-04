//! What the Library's Artists and History tabs and its sort control need that is not about a
//! single page: sort keys and orderings, and the play history as the screen draws it. Pure; the
//! model owns the state and the view draws it.

use apollo_domain::library::Collection;
use apollo_provider::{HistoryDay, Hit};

use super::model::Tab;
use super::source::TrackRow;

/// How a Library list is ordered. `Recent` is the service's own order, which is the order the
/// account added things in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SortKey {
    #[default]
    Recent,
    Title,
    Artist,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            Self::Recent => "Recently added",
            Self::Title => "A\u{2013}Z",
            Self::Artist => "Artist",
        }
    }

    /// What is saved.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Recent => "recent",
            Self::Title => "title",
            Self::Artist => "artist",
        }
    }

    pub fn from_stored(text: &str) -> Option<Self> {
        match text.trim() {
            "recent" => Some(Self::Recent),
            "title" => Some(Self::Title),
            "artist" => Some(Self::Artist),
            _ => None,
        }
    }
}

impl Tab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Playlists => "Playlists",
            Self::Albums => "Albums",
            Self::Artists => "Artists",
            Self::History => "History",
            Self::Stats => "Stats",
        }
    }

    /// The tab's name in the saved sort choice.
    pub fn stored(self) -> &'static str {
        match self {
            Self::Playlists => "playlists",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::History => "history",
            Self::Stats => "stats",
        }
    }

    /// The orders the tab offers, the default first. Only albums have an artist to sort by (a
    /// playlist's second line names its owner, and an artist has no artist), and History is the
    /// order it was played in.
    pub fn sorts(self) -> &'static [SortKey] {
        match self {
            Self::Playlists | Self::Artists => &[SortKey::Recent, SortKey::Title],
            Self::Albums => &[SortKey::Recent, SortKey::Title, SortKey::Artist],
            // Stats picks a period instead of an order.
            Self::History | Self::Stats => &[],
        }
    }
}

/// The artist an album's second line names: "Album • Neon Arcade • 2024" and "Neon Arcade •
/// 2024" both give "Neon Arcade". Empty when there is none.
pub fn subtitle_artist(subtitle: Option<&str>) -> String {
    const KINDS: [&str; 5] = ["album", "single", "ep", "playlist", "song"];
    subtitle
        .unwrap_or_default()
        .split(" \u{2022} ")
        .map(str::trim)
        .find(|part| !part.is_empty() && !KINDS.contains(&part.to_lowercase().as_str()))
        .unwrap_or_default()
        .to_owned()
}

/// Playlists or albums in `key` order. Liked Music stays first whatever the order. `Recent`
/// leaves the service's order alone.
pub fn sort_collections(list: &mut [Collection], key: SortKey) {
    match key {
        SortKey::Recent => {}
        SortKey::Title => {
            list.sort_by_cached_key(|c| (!c.is_liked_music(), c.title.to_lowercase()))
        }
        SortKey::Artist => list.sort_by_cached_key(|c| {
            let artist = subtitle_artist(c.subtitle.as_deref()).to_lowercase();
            // Albums with no artist go last.
            (
                !c.is_liked_music(),
                artist.is_empty(),
                artist,
                c.title.to_lowercase(),
            )
        }),
    }
}

/// Artists in `key` order (they have no artist of their own: `Artist` reads as `Title`).
pub fn sort_artists(list: &mut [Hit], key: SortKey) {
    if key != SortKey::Recent {
        list.sort_by_cached_key(|h| h.title.to_lowercase());
    }
}

/// One of YouTube Music's date buckets of the history, as rows.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryGroup {
    pub title: String,
    pub rows: Vec<TrackRow>,
}

pub fn history_groups(days: Vec<HistoryDay>) -> Vec<HistoryGroup> {
    days.into_iter()
        .map(|day| HistoryGroup {
            title: day.title,
            rows: day.items.into_iter().map(TrackRow::from).collect(),
        })
        .collect()
}

/// Rows across every bucket.
pub fn history_len(groups: &[HistoryGroup]) -> usize {
    groups.iter().map(|g| g.rows.len()).sum()
}

/// The `n`th row counting across buckets, and the bucket it is in.
pub fn history_row(groups: &[HistoryGroup], n: usize) -> Option<(usize, &TrackRow)> {
    let mut left = n;
    for (g, group) in groups.iter().enumerate() {
        if left < group.rows.len() {
            return Some((g, &group.rows[left]));
        }
        left -= group.rows.len();
    }
    None
}

/// Where row `n` sits among the scrolling column's children, which hold one heading per bucket
/// before that bucket's rows.
pub fn history_child(groups: &[HistoryGroup], n: usize) -> usize {
    n + history_row(groups, n).map_or(groups.len(), |(g, _)| g + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::library::{CollectionKind, LIKED_MUSIC_BROWSE_ID};
    use apollo_domain::model::BrowseId;

    fn c(kind: CollectionKind, id: &str, title: &str, subtitle: Option<&str>) -> Collection {
        Collection {
            kind,
            id: BrowseId::new(id),
            title: title.into(),
            subtitle: subtitle.map(Into::into),
            thumbnail: None,
        }
    }

    fn titles(list: &[Collection]) -> Vec<&str> {
        list.iter().map(|c| c.title.as_str()).collect()
    }

    #[test]
    fn the_artist_is_the_first_part_that_is_not_a_kind_label() {
        assert_eq!(
            subtitle_artist(Some("Album \u{2022} Neon Arcade \u{2022} 2024")),
            "Neon Arcade"
        );
        assert_eq!(
            subtitle_artist(Some("Neon Arcade \u{2022} 2024")),
            "Neon Arcade"
        );
        assert_eq!(subtitle_artist(Some("Single")), "");
        assert_eq!(subtitle_artist(None), "");
    }

    #[test]
    fn a_to_z_ignores_case_and_keeps_liked_music_first() {
        let mut list = vec![
            c(
                CollectionKind::Playlist,
                LIKED_MUSIC_BROWSE_ID,
                "Liked Music",
                None,
            ),
            c(CollectionKind::Playlist, "VLa", "road trip", None),
            c(CollectionKind::Playlist, "VLb", "Anthems", None),
            c(CollectionKind::Playlist, "VLc", "Zen", None),
        ];
        sort_collections(&mut list, SortKey::Title);
        assert_eq!(
            titles(&list),
            ["Liked Music", "Anthems", "road trip", "Zen"]
        );
        // The service's order comes back untouched.
        let mut recent = list.clone();
        recent.reverse();
        let before = titles(&recent).join("|");
        sort_collections(&mut recent, SortKey::Recent);
        assert_eq!(titles(&recent).join("|"), before);
    }

    #[test]
    fn albums_sort_by_artist_then_title_with_unknown_artists_last() {
        let mut list = vec![
            c(
                CollectionKind::Album,
                "1",
                "Zed",
                Some("Album \u{2022} Beta \u{2022} 2020"),
            ),
            c(CollectionKind::Album, "2", "Mystery", None),
            c(
                CollectionKind::Album,
                "3",
                "Able",
                Some("Album \u{2022} Beta \u{2022} 2021"),
            ),
            c(
                CollectionKind::Album,
                "4",
                "Solo",
                Some("Album \u{2022} alpha \u{2022} 2022"),
            ),
        ];
        sort_collections(&mut list, SortKey::Artist);
        assert_eq!(titles(&list), ["Solo", "Able", "Zed", "Mystery"]);
    }

    #[test]
    fn only_albums_offer_artist_and_history_offers_nothing() {
        assert_eq!(Tab::Albums.sorts().len(), 3);
        assert!(!Tab::Playlists.sorts().contains(&SortKey::Artist));
        assert!(!Tab::Artists.sorts().contains(&SortKey::Artist));
        assert!(Tab::History.sorts().is_empty());
        for tab in [Tab::Playlists, Tab::Albums, Tab::Artists] {
            assert_eq!(
                tab.sorts()[0],
                SortKey::Recent,
                "recently added is the default"
            );
        }
    }

    #[test]
    fn a_saved_sort_reads_back_and_junk_does_not() {
        for key in [SortKey::Recent, SortKey::Title, SortKey::Artist] {
            assert_eq!(SortKey::from_stored(key.stored()), Some(key));
        }
        assert_eq!(SortKey::from_stored("sideways"), None);
    }

    fn group(title: &str, n: usize) -> HistoryGroup {
        HistoryGroup {
            title: title.into(),
            rows: (0..n)
                .map(|i| TrackRow {
                    track: apollo_domain::model::Track::new(format!("vid{title}{i:04}"), "T"),
                    occurrence: None,
                    thumbnail: None,
                    explicit: false,
                    liked: None,
                })
                .collect(),
        }
    }

    #[test]
    fn history_rows_count_across_buckets_and_find_their_heading() {
        let groups = vec![group("Today", 2), group("Yesterday", 3)];
        assert_eq!(history_len(&groups), 5);
        assert_eq!(history_row(&groups, 1).map(|(g, _)| g), Some(0));
        assert_eq!(history_row(&groups, 2).map(|(g, _)| g), Some(1));
        assert!(history_row(&groups, 5).is_none());
        // Children: H, r, r, H, r, r, r.
        assert_eq!(
            (0..5)
                .map(|n| history_child(&groups, n))
                .collect::<Vec<_>>(),
            [1, 2, 4, 5, 6]
        );
    }
}
