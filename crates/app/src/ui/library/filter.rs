//! The filter box of a playlist or album page: which loaded rows it keeps. A row is kept when
//! every word typed appears in its title, artists or album, in any case.

use crate::library::source::TrackRow;

/// The rows of a loaded list that are shown, as positions in that list.
pub(super) enum Visible {
    /// No filter: the first `n` rows, in place.
    All(usize),
    /// The kept rows' positions, in list order.
    Some(Vec<usize>),
}

impl Visible {
    /// What `needle` (lowercased and trimmed; empty keeps everything) leaves of `rows`.
    pub fn of(rows: &[TrackRow], needle: &str) -> Self {
        if needle.is_empty() {
            return Self::All(rows.len());
        }
        let words: Vec<&str> = needle.split_whitespace().collect();
        Self::Some(
            rows.iter()
                .enumerate()
                .filter(|(_, row)| keeps(row, &words))
                .map(|(i, _)| i)
                .collect(),
        )
    }

    pub fn len(&self) -> usize {
        match self {
            Self::All(n) => *n,
            Self::Some(kept) => kept.len(),
        }
    }

    /// The position in the loaded list of the `n`th shown row.
    pub fn get(&self, n: usize) -> Option<usize> {
        match self {
            Self::All(len) => (n < *len).then_some(n),
            Self::Some(kept) => kept.get(n).copied(),
        }
    }
}

/// `words` are lowercase.
fn keeps(row: &TrackRow, words: &[&str]) -> bool {
    let mut haystack = row.track.title.to_lowercase();
    for artist in &row.track.artists {
        haystack.push(' ');
        haystack.push_str(&artist.name.to_lowercase());
    }
    if let Some(album) = &row.track.album {
        haystack.push(' ');
        haystack.push_str(&album.title.to_lowercase());
    }
    words.iter().all(|w| haystack.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::model::{AlbumRef, ArtistRef, Track};

    fn row(title: &str, artist: &str, album: &str) -> TrackRow {
        let mut track = Track::new(format!("v{title}").replace(' ', "x"), title);
        track.artists = vec![ArtistRef {
            name: artist.into(),
            id: None,
        }];
        track.album = Some(AlbumRef {
            title: album.into(),
            id: None,
        });
        TrackRow {
            track,
            occurrence: None,
            thumbnail: None,
            explicit: false,
            liked: None,
        }
    }

    fn rows() -> Vec<TrackRow> {
        vec![
            row("Midnight Drive", "Neon Arcade", "Night Shift"),
            row("Glass Garden", "Soft Lantern", "Greenhouse"),
            row("Ash", "Monochrome", "Greys"),
        ]
    }

    fn kept(needle: &str) -> Vec<usize> {
        let rows = rows();
        let v = Visible::of(&rows, needle);
        (0..v.len()).filter_map(|n| v.get(n)).collect()
    }

    #[test]
    fn no_filter_keeps_every_row_in_place() {
        assert_eq!(kept(""), [0, 1, 2]);
    }

    #[test]
    fn title_artist_and_album_all_match_in_any_case() {
        assert_eq!(kept("glass"), [1]);
        assert_eq!(kept("neon"), [0]);
        assert_eq!(kept("greys"), [2]);
        assert_eq!(kept("soft lan"), [1]);
    }

    #[test]
    fn every_word_has_to_match_but_not_in_one_field() {
        assert_eq!(kept("midnight arcade"), [0]);
        assert!(kept("midnight lantern").is_empty());
    }

    #[test]
    fn positions_in_the_loaded_list_survive_the_filter() {
        let rows = rows();
        let v = Visible::of(&rows, "ash");
        assert_eq!(v.len(), 1);
        assert_eq!(v.get(0), Some(2));
        assert_eq!(v.get(1), None);
    }
}
