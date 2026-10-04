//! Blocked artists: a local list of artists whose songs radio, autoplay and Home's personal
//! shelves must not add. Pure; the app keeps the list per account and hands a copy to whatever
//! filters.
//!
//! An artist is blocked by its channel id (`UC…`, what an artist page is opened by). A credit
//! that carries no id (some rows name an artist without linking it) is matched by name, exactly
//! as written but ignoring case, so it cannot be used to slip past. A credit that does carry an
//! id is matched by id alone: two different artists may share a name.

use crate::model::{ArtistRef, Track};

/// One blocked artist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockedArtist {
    /// The artist's channel id; `None` for an artist known only by name.
    pub id: Option<String>,
    pub name: String,
}

/// The blocked artists of one account, most recently blocked first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Blocklist {
    entries: Vec<BlockedArtist>,
}

impl Blocklist {
    pub fn new(entries: Vec<BlockedArtist>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[BlockedArtist] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether `artist` is on the list.
    pub fn blocks_artist(&self, artist: &ArtistRef) -> bool {
        self.entries.iter().any(|b| match (&artist.id, &b.id) {
            (Some(id), Some(blocked)) => id.as_str() == blocked,
            (None, _) => names_match(&artist.name, &b.name),
            // A credit with an id, against an entry known only by name: the name decides.
            (Some(_), None) => names_match(&artist.name, &b.name),
        })
    }

    /// Whether any of the track's credited artists is blocked.
    pub fn blocks(&self, track: &Track) -> bool {
        !self.entries.is_empty() && track.artists.iter().any(|a| self.blocks_artist(a))
    }

    /// Whether any of the artists is blocked (a credit list saved without the rest of the track).
    pub fn blocks_any(&self, artists: &[ArtistRef]) -> bool {
        !self.entries.is_empty() && artists.iter().any(|a| self.blocks_artist(a))
    }

    /// Add an artist at the front. Returns `false` when it was already on the list.
    pub fn add(&mut self, artist: BlockedArtist) -> bool {
        if self.entries.iter().any(|b| same_entry(b, &artist)) {
            return false;
        }
        self.entries.insert(0, artist);
        true
    }

    /// Take an artist off. Returns whether anything was removed.
    pub fn remove(&mut self, artist: &BlockedArtist) -> bool {
        let before = self.entries.len();
        self.entries.retain(|b| !same_entry(b, artist));
        self.entries.len() != before
    }
}

fn same_entry(a: &BlockedArtist, b: &BlockedArtist) -> bool {
    match (&a.id, &b.id) {
        (Some(x), Some(y)) => x == y,
        (None, None) => names_match(&a.name, &b.name),
        _ => false,
    }
}

fn names_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && a.to_lowercase() == b.to_lowercase()
}

/// Drop the tracks that have a blocked artist. Order is kept.
pub fn retain_allowed<T>(items: Vec<T>, blocklist: &Blocklist, track_of: impl Fn(&T) -> &Track) -> Vec<T> {
    if blocklist.is_empty() {
        return items;
    }
    items
        .into_iter()
        .filter(|item| !blocklist.blocks(track_of(item)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BrowseId;

    fn credit(name: &str, id: Option<&str>) -> ArtistRef {
        ArtistRef {
            name: name.into(),
            id: id.map(BrowseId::new),
        }
    }

    fn song(video: &str, artists: Vec<ArtistRef>) -> Track {
        let mut t = Track::new(video, video);
        t.artists = artists;
        t
    }

    fn block(id: Option<&str>, name: &str) -> BlockedArtist {
        BlockedArtist {
            id: id.map(Into::into),
            name: name.into(),
        }
    }

    #[test]
    fn an_artist_is_blocked_by_id_whatever_the_name_says() {
        let list = Blocklist::new(vec![block(Some("UCa"), "Artist A")]);
        assert!(list.blocks(&song("1", vec![credit("Artist A (feat.)", Some("UCa"))])));
        assert!(!list.blocks(&song("2", vec![credit("Artist A", Some("UCb"))])), "same name, other artist");
    }

    #[test]
    fn a_credit_without_an_id_is_matched_by_name_ignoring_case() {
        let list = Blocklist::new(vec![block(Some("UCa"), "Artist A")]);
        assert!(list.blocks(&song("1", vec![credit("artist a", None)])));
        assert!(!list.blocks(&song("2", vec![credit("Someone Else", None)])));
        assert!(!list.blocks(&song("3", vec![credit("", None)])), "an empty name matches nothing");
    }

    #[test]
    fn any_credited_artist_blocks_the_song() {
        let list = Blocklist::new(vec![block(Some("UCa"), "A")]);
        let duet = song("1", vec![credit("B", Some("UCb")), credit("A", Some("UCa"))]);
        assert!(list.blocks(&duet));
        assert!(!list.blocks(&song("2", vec![credit("B", Some("UCb"))])));
        assert!(!list.blocks(&song("3", vec![])), "no credits, nothing to match");
    }

    #[test]
    fn an_empty_list_blocks_nothing() {
        assert!(!Blocklist::default().blocks(&song("1", vec![credit("A", Some("UCa"))])));
    }

    #[test]
    fn adding_twice_keeps_one_and_the_newest_comes_first() {
        let mut list = Blocklist::default();
        assert!(list.add(block(Some("UCa"), "A")));
        assert!(list.add(block(Some("UCb"), "B")));
        assert!(!list.add(block(Some("UCa"), "A renamed")));
        assert_eq!(list.len(), 2);
        assert_eq!(list.entries()[0].name, "B");
    }

    #[test]
    fn removing_takes_exactly_that_artist() {
        let mut list = Blocklist::default();
        list.add(block(Some("UCa"), "A"));
        list.add(block(Some("UCb"), "B"));
        assert!(list.remove(&block(Some("UCa"), "A")));
        assert!(!list.remove(&block(Some("UCa"), "A")));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn retain_allowed_keeps_order_and_drops_only_blocked_songs() {
        let list = Blocklist::new(vec![block(Some("UCa"), "A")]);
        let songs = vec![
            song("1", vec![credit("B", Some("UCb"))]),
            song("2", vec![credit("A", Some("UCa"))]),
            song("3", vec![credit("C", Some("UCc"))]),
        ];
        let kept = retain_allowed(songs, &list, |t| t);
        let ids: Vec<&str> = kept.iter().map(|t| t.video_id.as_str()).collect();
        assert_eq!(ids, ["1", "3"]);
    }
}
