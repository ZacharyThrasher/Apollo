//! Searching the owner's own library: liked songs, saved albums, playlists and subscribed
//! artists that the Library screen has already loaded. Local only (no request is made from here).
//!
//! A row matches when every word typed appears in its title, artist or album (a collection's
//! title and its second line, which names the artist or owner), ignoring case and accents. The
//! library builds a [`LocalIndex`] once when its lists change, so a keystroke only scans
//! pre-folded strings.

use apollo_domain::library::Collection;
use apollo_provider::Hit;

use crate::library::source::TrackRow;

/// Rows the "In your library" group on All shows at most.
pub const LEAD_ROWS: usize = 4;
/// Rows of one kind the Library chip lists at most (a short query can match thousands of liked
/// songs, and the list is not paged).
pub const CHIP_CAP: usize = 200;

/// Lowercase `text` and drop accents, so "Beyoncé" and "beyonce" are the same word.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        match c {
            // Combining marks left over from decomposed text.
            '\u{300}'..='\u{36f}' => {}
            'à'..='å' | 'ā' | 'ă' | 'ą' => out.push('a'),
            'æ' => out.push_str("ae"),
            'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => out.push('c'),
            'ď' | 'đ' => out.push('d'),
            'è'..='ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => out.push('e'),
            'ĝ' | 'ğ' | 'ġ' | 'ģ' => out.push('g'),
            'ĥ' | 'ħ' => out.push('h'),
            'ì'..='ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => out.push('i'),
            'ĵ' => out.push('j'),
            'ķ' => out.push('k'),
            'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => out.push('l'),
            'ñ' | 'ń' | 'ņ' | 'ň' => out.push('n'),
            'ò'..='ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => out.push('o'),
            'œ' => out.push_str("oe"),
            'ŕ' | 'ŗ' | 'ř' => out.push('r'),
            'ś' | 'ŝ' | 'ş' | 'š' => out.push('s'),
            'ß' => out.push_str("ss"),
            'ţ' | 'ť' | 'ŧ' => out.push('t'),
            'ù'..='ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => out.push('u'),
            'ŵ' => out.push('w'),
            'ý' | 'ÿ' | 'ŷ' => out.push('y'),
            'ź' | 'ż' | 'ž' => out.push('z'),
            c => out.push(c),
        }
    }
    out
}

/// One indexed item with its folded text.
struct Entry<T> {
    item: T,
    title: String,
    /// Title and everything else that can match, space separated.
    all: String,
}

impl<T> Entry<T> {
    fn new(item: T, title: &str, rest: impl IntoIterator<Item = String>) -> Self {
        let title = fold(title);
        let mut all = title.clone();
        for part in rest {
            all.push(' ');
            all.push_str(&fold(&part));
        }
        Self { item, title, all }
    }

    /// How well the typed text matches: 0 the title starts with it, 1 the title holds every
    /// word, 2 other fields do. `None` when some word is missing.
    fn rank(&self, query: &str, words: &[&str]) -> Option<u8> {
        if !words.iter().all(|w| self.all.contains(w)) {
            return None;
        }
        Some(if self.title.starts_with(query) {
            0
        } else if words.iter().all(|w| self.title.contains(w)) {
            1
        } else {
            2
        })
    }
}

/// What the library holds, folded for matching. Built by the library, read by the search.
#[derive(Default)]
pub struct LocalIndex {
    songs: Vec<Entry<TrackRow>>,
    albums: Vec<Entry<Collection>>,
    playlists: Vec<Entry<Collection>>,
    artists: Vec<Entry<Hit>>,
}

impl LocalIndex {
    pub fn new(
        songs: &[TrackRow],
        albums: &[Collection],
        playlists: &[Collection],
        artists: &[Hit],
    ) -> Self {
        let collection = |c: &Collection| {
            Entry::new(c.clone(), &c.title, c.subtitle.iter().cloned())
        };
        Self {
            songs: songs
                .iter()
                .map(|row| {
                    let rest = row
                        .track
                        .artists
                        .iter()
                        .map(|a| a.name.clone())
                        .chain(row.track.album.iter().map(|a| a.title.clone()));
                    Entry::new(row.clone(), &row.track.title, rest)
                })
                .collect(),
            albums: albums.iter().map(collection).collect(),
            playlists: playlists.iter().map(collection).collect(),
            artists: artists
                .iter()
                .map(|h| Entry::new(h.clone(), &h.title, std::iter::empty()))
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
            && self.albums.is_empty()
            && self.playlists.is_empty()
            && self.artists.is_empty()
    }

    /// Everything that matches `query`, each kind best match first (ties keep the library's
    /// order), and the best few across kinds for the All view.
    pub fn find(&self, query: &str) -> LocalResults {
        let query = fold(query.trim());
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return LocalResults::default();
        }
        fn scan<T: Clone>(
            entries: &[Entry<T>],
            query: &str,
            words: &[&str],
        ) -> Vec<(u8, T)> {
            let mut hits: Vec<(u8, T)> = entries
                .iter()
                .filter_map(|e| e.rank(query, words).map(|r| (r, e.item.clone())))
                .collect();
            // Stable: equal ranks stay in the library's order.
            hits.sort_by_key(|(rank, _)| *rank);
            hits
        }
        let songs = scan(&self.songs, &query, &words);
        let albums = scan(&self.albums, &query, &words);
        let playlists = scan(&self.playlists, &query, &words);
        let artists = scan(&self.artists, &query, &words);

        // The lead rows: best rank first across kinds, songs before albums before playlists
        // before artists among equals.
        let mut lead: Vec<(u8, usize, LocalHit)> = Vec::new();
        let kinds: [Vec<(u8, LocalHit)>; 4] = [
            songs.iter().take(LEAD_ROWS).map(|(r, t)| (*r, LocalHit::Song(t.clone()))).collect(),
            albums.iter().take(LEAD_ROWS).map(|(r, c)| (*r, LocalHit::Album(c.clone()))).collect(),
            playlists
                .iter()
                .take(LEAD_ROWS)
                .map(|(r, c)| (*r, LocalHit::Playlist(c.clone())))
                .collect(),
            artists.iter().take(LEAD_ROWS).map(|(r, h)| (*r, LocalHit::Artist(h.clone()))).collect(),
        ];
        for (order, kind) in kinds.into_iter().enumerate() {
            lead.extend(kind.into_iter().map(|(rank, hit)| (rank, order, hit)));
        }
        lead.sort_by_key(|(rank, order, _)| (*rank, *order));
        lead.truncate(LEAD_ROWS);

        LocalResults {
            total: songs.len() + albums.len() + playlists.len() + artists.len(),
            songs: songs.into_iter().map(|(_, t)| t).collect(),
            albums: albums.into_iter().map(|(_, c)| c).collect(),
            playlists: playlists.into_iter().map(|(_, c)| c).collect(),
            artists: artists.into_iter().map(|(_, h)| h).collect(),
            lead: lead.into_iter().map(|(_, _, hit)| hit).collect(),
        }
    }
}

/// One of the best matches, of any kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalHit {
    Song(TrackRow),
    Album(Collection),
    Playlist(Collection),
    Artist(Hit),
}

/// What a query found in the library.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalResults {
    pub songs: Vec<TrackRow>,
    pub albums: Vec<Collection>,
    pub playlists: Vec<Collection>,
    pub artists: Vec<Hit>,
    /// The best [`LEAD_ROWS`] across kinds, for the All view's "In your library" group.
    pub lead: Vec<LocalHit>,
    /// Matches of every kind, before any cap.
    pub total: usize,
}

impl LocalResults {
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The songs among the lead rows, in order (the rows the keyboard can select).
    pub fn lead_songs(&self) -> impl Iterator<Item = &TrackRow> {
        self.lead.iter().filter_map(|hit| match hit {
            LocalHit::Song(row) => Some(row),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use apollo_domain::library::CollectionKind;
    use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};

    use super::*;

    fn song(id: &str, title: &str, artist: &str, album: &str) -> TrackRow {
        let mut track = Track::new(id, title);
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

    fn collection(kind: CollectionKind, id: &str, title: &str, sub: Option<&str>) -> Collection {
        Collection {
            kind,
            id: BrowseId::new(id),
            title: title.into(),
            subtitle: sub.map(Into::into),
            thumbnail: None,
        }
    }

    fn artist(id: &str, name: &str) -> Hit {
        Hit {
            id: BrowseId::new(id),
            title: name.into(),
            subtitle: None,
            thumbnail: None,
            explicit: false,
        }
    }

    fn index() -> LocalIndex {
        LocalIndex::new(
            &[
                song("s1", "Halo", "Beyoncé", "I Am... Sasha Fierce"),
                song("s2", "Get Lucky", "Daft Punk", "Random Access Memories"),
                song("s3", "Lucky Star", "Madonna", "Madonna"),
            ],
            &[collection(
                CollectionKind::Album,
                "MPREb_1",
                "Random Access Memories",
                Some("Daft Punk \u{2022} 2013"),
            )],
            &[collection(
                CollectionKind::Playlist,
                "VLPL1",
                "Road trip",
                Some("Zach"),
            )],
            &[artist("UC1", "Daft Punk"), artist("UC2", "Björk")],
        )
    }

    fn titles(rows: &[TrackRow]) -> Vec<&str> {
        rows.iter().map(|r| r.track.title.as_str()).collect()
    }

    #[test]
    fn folding_ignores_case_and_accents() {
        assert_eq!(fold("Beyoncé"), "beyonce");
        assert_eq!(fold("BJÖRK"), "bjork");
        assert_eq!(fold("Mötley Crüe"), "motley crue");
        assert_eq!(fold("Straße"), "strasse");
        // Decomposed "e" + combining acute.
        assert_eq!(fold("Beyonce\u{301}"), "beyonce");
    }

    #[test]
    fn an_accentless_query_finds_an_accented_title_and_back() {
        let found = index().find("beyonce");
        assert_eq!(titles(&found.songs), ["Halo"]);
        let found = index().find("BJORK");
        assert_eq!(found.artists.len(), 1);
        assert_eq!(found.artists[0].title, "Björk");
    }

    #[test]
    fn matches_the_artist_and_the_album_not_only_the_title() {
        let found = index().find("daft");
        assert_eq!(titles(&found.songs), ["Get Lucky"]);
        assert_eq!(found.albums.len(), 1, "the album's second line names the artist");
        assert_eq!(found.artists.len(), 1);
        let found = index().find("sasha fierce");
        assert_eq!(titles(&found.songs), ["Halo"]);
    }

    #[test]
    fn every_word_must_appear_in_any_order() {
        assert_eq!(titles(&index().find("punk lucky").songs), ["Get Lucky"]);
        assert!(index().find("lucky beyonce").songs.is_empty());
    }

    #[test]
    fn title_matches_come_first_and_the_rest_keep_library_order() {
        // "lucky" starts "Lucky Star" and is inside "Get Lucky"; the prefix match leads.
        let found = index().find("lucky");
        assert_eq!(titles(&found.songs), ["Lucky Star", "Get Lucky"]);
        // "madonna" is the artist of both rows; neither title starts with it... one does.
        let found = index().find("madonna");
        assert_eq!(titles(&found.songs), ["Lucky Star"]);
    }

    #[test]
    fn nothing_matches_blank_or_unknown_text() {
        assert!(index().find("").is_empty());
        assert!(index().find("   ").is_empty());
        assert!(index().find("zzzz").is_empty());
    }

    #[test]
    fn the_lead_rows_mix_kinds_best_first_and_stop_at_four() {
        let found = index().find("daft");
        // Both artist and album/song match through other fields or the title: at most four.
        assert!(found.lead.len() <= LEAD_ROWS);
        assert!(matches!(found.lead.first(), Some(LocalHit::Artist(_))));
        assert_eq!(found.total, 3);
        let many: Vec<TrackRow> = (0..10)
            .map(|i| song(&format!("v{i}"), &format!("Moon {i}"), "A", "B"))
            .collect();
        let found = LocalIndex::new(&many, &[], &[], &[]).find("moon");
        assert_eq!(found.lead.len(), LEAD_ROWS);
        assert_eq!(found.songs.len(), 10);
        assert_eq!(found.lead_songs().count(), LEAD_ROWS);
    }
}
