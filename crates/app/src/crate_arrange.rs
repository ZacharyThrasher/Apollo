//! Arranging the Crate: the order changes behind the Arrange menu (sort, shuffle, reverse, spread
//! artists) and the Crate's filter box, as plain functions on a list of rows.
//!
//! Everything here is local and free: no YouTube call, and the Crate's own order is rewritten,
//! not a hidden view of it, so Save as playlist keeps what was arranged. [`crate::crate_list`]
//! applies a result as one step for Ctrl+Z.

use std::collections::HashMap;

use apollo_store::CrateRow;

/// What a sort puts in order. Every sort is stable: songs that tie keep the order they had.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Title,
    Artist,
    Album,
    /// Oldest first: the order the songs were set aside in.
    DateAdded,
    /// Shortest first.
    Length,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [
        SortKey::Title,
        SortKey::Artist,
        SortKey::Album,
        SortKey::DateAdded,
        SortKey::Length,
    ];

    /// The menu's label, and the end of "Sorted by artist".
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Title => "Title",
            SortKey::Artist => "Artist",
            SortKey::Album => "Album",
            SortKey::DateAdded => "Date added",
            SortKey::Length => "Length",
        }
    }

    /// The label as it reads inside a sentence ("Sorted by date added").
    pub fn lower(self) -> &'static str {
        match self {
            SortKey::Title => "title",
            SortKey::Artist => "artist",
            SortKey::Album => "album",
            SortKey::DateAdded => "date added",
            SortKey::Length => "length",
        }
    }
}

/// One of the Arrange menu's rewrites of the whole Crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    Sort(SortKey),
    Shuffle,
    Reverse,
    /// No two songs in a row by the same primary artist, where that can be done.
    SpreadArtists,
}

impl Arrange {
    /// Every choice in menu order: the sorts, then the three others.
    pub const ALL: [Arrange; 8] = [
        Arrange::Sort(SortKey::Title),
        Arrange::Sort(SortKey::Artist),
        Arrange::Sort(SortKey::Album),
        Arrange::Sort(SortKey::DateAdded),
        Arrange::Sort(SortKey::Length),
        Arrange::Shuffle,
        Arrange::Reverse,
        Arrange::SpreadArtists,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Arrange::Sort(key) => key.label(),
            Arrange::Shuffle => "Shuffle",
            Arrange::Reverse => "Reverse",
            Arrange::SpreadArtists => "Spread artists",
        }
    }

    /// What the pill says once it is done.
    pub fn done(self) -> String {
        match self {
            Arrange::Sort(key) => format!("Sorted by {}", key.lower()),
            Arrange::Shuffle => "Shuffled the Crate".to_owned(),
            Arrange::Reverse => "Reversed the Crate".to_owned(),
            Arrange::SpreadArtists => "Spread the artists out".to_owned(),
        }
    }

    /// What the pill says when the Crate was in that order already.
    pub fn already(self) -> String {
        match self {
            Arrange::Sort(key) => format!("Already sorted by {}", key.lower()),
            Arrange::SpreadArtists => "Artists are already spread out".to_owned(),
            Arrange::Shuffle | Arrange::Reverse => "Nothing to change".to_owned(),
        }
    }

    /// What "Undid: …" says.
    pub fn words(self) -> String {
        match self {
            Arrange::Sort(key) => format!("sorted the Crate by {}", key.lower()),
            Arrange::Shuffle => "shuffled the Crate".to_owned(),
            Arrange::Reverse => "reversed the Crate".to_owned(),
            Arrange::SpreadArtists => "spread the artists out in the Crate".to_owned(),
        }
    }

    /// The Crate after this rewrite. `seed` only matters to a shuffle.
    pub fn apply(self, rows: &[CrateRow], seed: u64) -> Vec<CrateRow> {
        match self {
            Arrange::Sort(key) => sorted(rows, key),
            Arrange::Shuffle => shuffled(rows, seed),
            Arrange::Reverse => reversed(rows),
            Arrange::SpreadArtists => spread_artists(rows),
        }
    }
}

/// A value that sorts after every present one when it is missing.
fn last_if_none<T: Ord>(value: Option<T>) -> (bool, Option<T>) {
    (value.is_none(), value)
}

fn primary_artist(row: &CrateRow) -> Option<String> {
    row.track
        .artists
        .first()
        .map(|a| a.name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
}

/// The Crate ordered by `key`; songs that tie (or have no value, which go last) keep their order.
pub fn sorted(rows: &[CrateRow], key: SortKey) -> Vec<CrateRow> {
    let mut out = rows.to_vec();
    match key {
        SortKey::Title => out.sort_by_cached_key(|r| r.track.title.trim().to_lowercase()),
        SortKey::Artist => out.sort_by_cached_key(|r| last_if_none(primary_artist(r))),
        SortKey::Album => out.sort_by_cached_key(|r| {
            last_if_none(
                r.track
                    .album
                    .as_ref()
                    .map(|a| a.title.trim().to_lowercase())
                    .filter(|t| !t.is_empty()),
            )
        }),
        SortKey::DateAdded => out.sort_by_key(|r| r.added_ms),
        SortKey::Length => out.sort_by_key(|r| last_if_none(r.track.duration)),
    }
    out
}

pub fn reversed(rows: &[CrateRow]) -> Vec<CrateRow> {
    rows.iter().rev().cloned().collect()
}

/// The Crate in a random order, the same one for the same `seed`.
pub fn shuffled(rows: &[CrateRow], seed: u64) -> Vec<CrateRow> {
    let mut out = rows.to_vec();
    // splitmix64: small, and good enough to deal a list of songs.
    let mut state = seed;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    for i in (1..out.len()).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        out.swap(i, j);
    }
    out
}

/// The Crate with no two songs in a row by the same primary artist, wherever that can be done.
///
/// It keeps the order as far as it can: each place takes the earliest song, in the order the
/// Crate had, whose artist is not the one just placed and who still leaves the rest spreadable.
/// An artist with too many songs to be kept apart (more than half the list, rounded up) has its
/// songs spread as evenly as the others allow, and the rest stays in order. A Crate that is
/// already spread comes back unchanged. Songs without an artist never clash with anything.
pub fn spread_artists(rows: &[CrateRow]) -> Vec<CrateRow> {
    let n = rows.len();
    if n < 3 {
        return rows.to_vec();
    }
    // Give each artist a small number; a song with no artist gets one of its own.
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut artist_of: Vec<usize> = Vec::with_capacity(n);
    let mut counts: Vec<usize> = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let name = primary_artist(row).unwrap_or_else(|| format!("\u{0}{i}"));
        let next = ids.len();
        let id = *ids.entry(name).or_insert(next);
        if id == counts.len() {
            counts.push(0);
        }
        counts[id] += 1;
        artist_of.push(id);
    }

    let mut remaining: Vec<usize> = (0..n).collect();
    let mut placed: Vec<usize> = Vec::with_capacity(n);
    let mut previous: Option<usize> = None;
    while !remaining.is_empty() {
        let after = remaining.len() - 1;
        // The two largest runs left, to know the largest one that is not the song placed.
        let (mut top_id, mut top, mut second) = (usize::MAX, 0usize, 0usize);
        for (id, &count) in counts.iter().enumerate() {
            if count > top {
                second = top;
                (top_id, top) = (id, count);
            } else if count > second {
                second = count;
            }
        }
        // Placing `id` leaves `after` songs that must start with something else: no artist may
        // have more than half of them (rounded up), and `id` itself no more than half rounded down.
        let leaves_it_spreadable = |id: usize| {
            let others = if id == top_id { second } else { top };
            counts[id] - 1 <= after / 2 && others <= after.div_ceil(2)
        };
        let pick = remaining
            .iter()
            .position(|&i| Some(artist_of[i]) != previous && leaves_it_spreadable(artist_of[i]))
            .or_else(|| {
                // It cannot be done: take, of the songs that do not repeat the last one, the
                // earliest of the artist with the most left, so the big one is spread as thin as
                // can be.
                let mut best: Option<(usize, usize)> = None;
                for (at, &i) in remaining.iter().enumerate() {
                    if Some(artist_of[i]) == previous {
                        continue;
                    }
                    let count = counts[artist_of[i]];
                    if best.is_none_or(|(_, c)| count > c) {
                        best = Some((at, count));
                    }
                }
                best.map(|(at, _)| at)
            })
            // Nothing but the same artist is left.
            .unwrap_or(0);
        let song = remaining.remove(pick);
        counts[artist_of[song]] -= 1;
        previous = Some(artist_of[song]);
        placed.push(song);
    }
    placed.into_iter().map(|i| rows[i].clone()).collect()
}

/// The positions of the songs a filter keeps: every word typed appears in the title, an artist or
/// the album, in any case. An empty filter keeps them all.
pub fn visible(rows: &[CrateRow], needle: &str) -> Vec<usize> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return (0..rows.len()).collect();
    }
    let words: Vec<&str> = needle.split_whitespace().collect();
    rows.iter()
        .enumerate()
        .filter(|(_, row)| keeps(row, &words))
        .map(|(i, _)| i)
        .collect()
}

/// `words` are lowercase.
fn keeps(row: &CrateRow, words: &[&str]) -> bool {
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
    use std::time::Duration;

    use apollo_domain::model::{AlbumRef, ArtistRef, Track};

    use super::*;

    fn song(id: &str, title: &str, artist: Option<&str>) -> CrateRow {
        let mut track = Track::new(id, title);
        if let Some(name) = artist {
            track.artists = vec![ArtistRef { name: name.into(), id: None }];
        }
        CrateRow { track, thumbnail: None, added_ms: 0 }
    }

    fn by(artist: &str, n: usize) -> CrateRow {
        song(&format!("{artist}{n}"), &format!("{artist} {n}"), Some(artist))
    }

    fn ids(rows: &[CrateRow]) -> Vec<String> {
        rows.iter().map(|r| r.track.video_id.as_str().to_owned()).collect()
    }

    fn artists_of(rows: &[CrateRow]) -> String {
        rows.iter()
            .map(|r| r.track.artists.first().map_or("?", |a| a.name.as_str()))
            .collect::<Vec<_>>()
            .join("")
    }

    fn list(spec: &str) -> Vec<CrateRow> {
        spec.chars()
            .enumerate()
            .map(|(i, c)| song(&format!("s{i}"), &format!("t{i}"), Some(&c.to_string())))
            .collect()
    }

    fn clashes(rows: &[CrateRow]) -> usize {
        rows.windows(2)
            .filter(|w| primary_artist(&w[0]).is_some() && primary_artist(&w[0]) == primary_artist(&w[1]))
            .count()
    }

    #[test]
    fn sorts_are_case_blind_and_stable_and_put_what_is_missing_last() {
        let mut a = song("a", "banana", Some("Zed"));
        a.track.album = Some(AlbumRef { title: "Y".into(), id: None });
        a.added_ms = 30;
        a.track.duration = Some(Duration::from_secs(200));
        let mut b = song("b", "Apple", Some("amy"));
        b.track.album = Some(AlbumRef { title: "x".into(), id: None });
        b.added_ms = 10;
        b.track.duration = Some(Duration::from_secs(100));
        let c = song("c", "cherry", None); // no artist, album or length
        let mut d = song("d", "Cherry", Some("Amy"));
        d.added_ms = 20;
        let rows = vec![a, b, c, d];

        assert_eq!(ids(&sorted(&rows, SortKey::Title)), ["b", "a", "c", "d"], "c before d: a tie keeps order");
        assert_eq!(ids(&sorted(&rows, SortKey::Artist)), ["b", "d", "a", "c"], "Amy = amy, kept in order; none last");
        assert_eq!(ids(&sorted(&rows, SortKey::Album)), ["b", "a", "c", "d"]);
        assert_eq!(ids(&sorted(&rows, SortKey::DateAdded)), ["c", "b", "d", "a"]);
        assert_eq!(ids(&sorted(&rows, SortKey::Length)), ["b", "a", "c", "d"], "no length goes last");
    }

    #[test]
    fn reverse_flips_and_shuffle_deals_every_song_once_the_same_way_for_a_seed() {
        let rows = list("ABCDEFGHIJ");
        assert_eq!(ids(&reversed(&rows)), ids(&rows.iter().rev().cloned().collect::<Vec<_>>()));

        let a = shuffled(&rows, 7);
        assert_eq!(ids(&a), ids(&shuffled(&rows, 7)), "the same seed deals the same order");
        assert_ne!(ids(&a), ids(&shuffled(&rows, 8)));
        let mut sorted_ids = ids(&a);
        sorted_ids.sort();
        let mut want = ids(&rows);
        want.sort();
        assert_eq!(sorted_ids, want, "nothing lost or doubled");
        assert!(shuffled(&[], 1).is_empty());
    }

    #[test]
    fn spreading_separates_artists_and_leaves_a_spread_crate_alone() {
        // Needs moving.
        assert_eq!(artists_of(&spread_artists(&list("AABB"))), "ABAB");
        assert_eq!(artists_of(&spread_artists(&list("AAABCD"))), "ABACAD");
        // The dominant artist is picked ahead of its turn so it never ends in a clump.
        assert_eq!(artists_of(&spread_artists(&list("ABBBA"))), "BABAB");
        // Already spread: unchanged, and so is a list with nothing to separate.
        for spec in ["ABAC", "ABCABC", "ABA", "ABCD", "A", "AB", ""] {
            let rows = list(spec);
            assert_eq!(ids(&spread_artists(&rows)), ids(&rows), "{spec}");
        }
    }

    #[test]
    fn when_artists_cannot_be_kept_apart_the_big_one_is_spread_as_thin_as_it_can_be() {
        // Four of seven is more than half rounded up: some clash is certain.
        let out = spread_artists(&list("AAAABCD"));
        assert_eq!(artists_of(&out), "ABACADA");
        // Too many to separate: every B still ends up as spread as the others allow.
        let out = spread_artists(&list("BBBBAC"));
        assert_eq!(artists_of(&out), "BABCBB");
        assert_eq!(clashes(&out), 1, "one clash is the least four of six can have");
    }

    #[test]
    fn spreading_never_loses_a_song_and_separates_whenever_it_can() {
        // Every list of up to eight songs over three artists, in a fixed pseudo-random order.
        let mut seed = 12345u64;
        let mut rng = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) as usize
        };
        for _ in 0..3000 {
            let n = 2 + rng() % 8;
            let spec: String = (0..n).map(|_| ['A', 'B', 'C'][rng() % 3]).collect();
            let rows = list(&spec);
            let out = spread_artists(&rows);
            let mut a = ids(&out);
            a.sort();
            let mut b = ids(&rows);
            b.sort();
            assert_eq!(a, b, "{spec}: songs kept");
            let most = ['A', 'B', 'C'].iter().map(|c| spec.matches(*c).count()).max().unwrap();
            if most <= n.div_ceil(2) {
                assert_eq!(clashes(&out), 0, "{spec} -> {}", artists_of(&out));
            }
        }
    }

    #[test]
    fn songs_without_an_artist_never_clash_and_artists_match_in_any_case() {
        let rows = vec![
            song("a", "t", None),
            song("b", "t", None),
            song("c", "t", None),
        ];
        assert_eq!(ids(&spread_artists(&rows)), ["a", "b", "c"]);
        let rows = vec![by("Amy", 1), by("AMY", 2), by("amy ", 3), by("Bo", 4)];
        let out = spread_artists(&rows);
        // Three of four cannot be kept apart, but the one that can is: Amy, Bo, Amy, Amy.
        assert_eq!(ids(&out), ["Amy1", "Bo4", "AMY2", "amy 3"]);
        assert_eq!(clashes(&out), 1);
    }

    #[test]
    fn the_filter_wants_every_word_in_the_title_artist_or_album() {
        let mut a = song("a", "Blue Monday", Some("New Order"));
        a.track.album = Some(AlbumRef { title: "Power, Corruption".into(), id: None });
        let b = song("b", "Regret", Some("New Order"));
        let c = song("c", "Monday", Some("Someone"));
        let rows = vec![a, b, c];
        assert_eq!(visible(&rows, ""), [0, 1, 2]);
        assert_eq!(visible(&rows, "   "), [0, 1, 2]);
        assert_eq!(visible(&rows, "MONDAY"), [0, 2]);
        assert_eq!(visible(&rows, "new monday"), [0]);
        assert_eq!(visible(&rows, "corruption"), [0]);
        assert!(visible(&rows, "zzz").is_empty());
    }
}
