//! What the stats screens show, worked out from the play log's per-track totals. Pure: no I/O,
//! no clock (the caller says what time it is).

use std::collections::HashMap;

use apollo_domain::model::{ArtistRef, BrowseId, Track};
use apollo_provider::{HomeItem, HomeShelf, ListedTrack};
use apollo_store::PlayTotal;

use super::track_of;

/// Rows in each of the stats lists.
pub const LIST_LEN: usize = 10;
/// A personal shelf stays hidden until it has this many tracks.
pub const SHELF_MIN: usize = 6;
/// Tracks on a personal shelf at most.
pub const SHELF_MAX: usize = 20;
/// "On repeat" looks at this long.
pub const ON_REPEAT_DAYS: i64 = 30;
/// A track is on repeat after this many plays in that time (once is not a repeat).
pub const ON_REPEAT_MIN_PLAYS: u64 = 2;
/// A favourite is forgotten after this long unplayed.
pub const FORGOTTEN_DAYS: i64 = 60;
/// A track is a favourite after this many plays.
pub const FAVOURITE_MIN_PLAYS: u64 = 5;

const DAY_SECS: i64 = 24 * 60 * 60;

/// The period the Stats tab shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Period {
    #[default]
    Weeks4,
    Months6,
    AllTime,
}

impl Period {
    pub const ALL: [Period; 3] = [Period::Weeks4, Period::Months6, Period::AllTime];

    pub fn label(self) -> &'static str {
        match self {
            Period::Weeks4 => "Last 4 weeks",
            Period::Months6 => "Last 6 months",
            Period::AllTime => "All time",
        }
    }

    /// Unix seconds the period starts at when it ends at `now`; `None` is "from the start".
    pub fn since(self, now: i64) -> Option<i64> {
        match self {
            Period::Weeks4 => Some(now - 28 * DAY_SECS),
            Period::Months6 => Some(now - 182 * DAY_SECS),
            Period::AllTime => None,
        }
    }
}

/// A track on the Top tracks list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopTrack {
    pub track: Track,
    pub art: Option<String>,
    pub plays: u64,
}

/// An artist on the Top artists list. `art` is the cover of their most played track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopArtist {
    pub name: String,
    pub id: Option<BrowseId>,
    pub art: Option<String>,
    pub plays: u64,
}

/// An album on the Top albums list. `artist` is the first artist credited on its tracks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopAlbum {
    pub title: String,
    pub id: Option<BrowseId>,
    pub artist: Option<String>,
    pub art: Option<String>,
    pub plays: u64,
}

/// Everything the Stats tab draws for one period.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatsReport {
    /// Plays that counted.
    pub plays: u64,
    pub listened_ms: u64,
    pub tracks: Vec<TopTrack>,
    pub artists: Vec<TopArtist>,
    pub albums: Vec<TopAlbum>,
}

impl StatsReport {
    /// No plays in the period.
    pub fn is_empty(&self) -> bool {
        self.plays == 0
    }

    pub fn top_artist(&self) -> Option<&TopArtist> {
        self.artists.first()
    }
}

/// The report for `totals` (most played first, as the log gives them).
pub fn build(totals: &[PlayTotal]) -> StatsReport {
    let mut report = StatsReport::default();
    // Artists and albums in the order they first appear, which is by their best track.
    let mut artist_ix: HashMap<String, usize> = HashMap::new();
    let mut album_ix: HashMap<String, usize> = HashMap::new();
    for total in totals {
        report.plays += total.plays;
        report.listened_ms += total.listened_ms;
        let track = track_of(total);
        for artist in &track.artists {
            let key = match &artist.id {
                Some(id) => id.as_str().to_owned(),
                None => format!("name:{}", artist.name.to_lowercase()),
            };
            let ix = *artist_ix.entry(key).or_insert_with(|| {
                report.artists.push(TopArtist {
                    name: artist.name.clone(),
                    id: artist.id.clone(),
                    art: total.art.clone(),
                    plays: 0,
                });
                report.artists.len() - 1
            });
            report.artists[ix].plays += total.plays;
        }
        if let Some(album) = &track.album {
            let first_artist = track.artists.first().map(|a| a.name.clone());
            let key = match &album.id {
                Some(id) => id.as_str().to_owned(),
                None => format!(
                    "name:{}|{}",
                    album.title.to_lowercase(),
                    first_artist.as_deref().unwrap_or("").to_lowercase()
                ),
            };
            let ix = *album_ix.entry(key).or_insert_with(|| {
                report.albums.push(TopAlbum {
                    title: album.title.clone(),
                    id: album.id.clone(),
                    artist: first_artist,
                    art: total.art.clone(),
                    plays: 0,
                });
                report.albums.len() - 1
            });
            report.albums[ix].plays += total.plays;
        }
        report.tracks.push(TopTrack {
            track,
            art: total.art.clone(),
            plays: total.plays,
        });
    }
    // Stable: equal plays keep the order they came in (the most recent first).
    report.artists.sort_by(|a, b| b.plays.cmp(&a.plays));
    report.albums.sort_by(|a, b| b.plays.cmp(&a.plays));
    report.tracks.truncate(LIST_LEN);
    report.artists.truncate(LIST_LEN);
    report.albums.truncate(LIST_LEN);
    report
}

/// The shelf song for a track the log remembers.
fn song(total: &PlayTotal) -> HomeItem {
    HomeItem::Song(ListedTrack {
        track: track_of(total),
        occurrence: None,
        thumbnail: total.art.clone(),
        explicit: false,
        liked: None,
    })
}

/// Home's personal shelves at `now`: "On repeat" from what was played in the last
/// [`ON_REPEAT_DAYS`] (`recent`), "Forgotten favourites" from all time (`all`). A shelf with
/// fewer than [`SHELF_MIN`] tracks is left out. Both lists are most played first.
pub fn personal_shelves(recent: &[PlayTotal], all: &[PlayTotal], now: i64) -> Vec<HomeShelf> {
    let mut shelves = Vec::new();
    let on_repeat: Vec<HomeItem> = recent
        .iter()
        .filter(|t| t.plays >= ON_REPEAT_MIN_PLAYS)
        .take(SHELF_MAX)
        .map(song)
        .collect();
    if on_repeat.len() >= SHELF_MIN {
        shelves.push(HomeShelf {
            title: "On repeat".into(),
            items: on_repeat,
        });
    }
    let forgotten: Vec<HomeItem> = all
        .iter()
        .filter(|t| t.plays >= FAVOURITE_MIN_PLAYS && t.last_played <= now - FORGOTTEN_DAYS * DAY_SECS)
        .take(SHELF_MAX)
        .map(song)
        .collect();
    if forgotten.len() >= SHELF_MIN {
        shelves.push(HomeShelf {
            title: "Forgotten favourites".into(),
            items: forgotten,
        });
    }
    shelves
}

/// "45 min", "3 h", "12 h 30 min": a listening time for a card.
pub fn listening_time(ms: u64) -> String {
    let minutes = ms / 60_000;
    match (minutes / 60, minutes % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The artists of a track on one line ("A, B").
pub fn artist_line(artists: &[ArtistRef]) -> String {
    artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total(video: &str, plays: u64, last_played: i64, artists: &str, album: Option<(&str, Option<&str>)>) -> PlayTotal {
        PlayTotal {
            video_id: video.into(),
            title: format!("Song {video}"),
            artists: artists.into(),
            album_title: album.map(|(t, _)| t.into()),
            album_id: album.and_then(|(_, id)| id.map(Into::into)),
            duration_ms: Some(200_000),
            art: Some(format!("https://example.com/{video}.jpg")),
            plays,
            listened_ms: plays * 180_000,
            last_played,
        }
    }

    fn artist(name: &str, id: Option<&str>) -> String {
        match id {
            Some(id) => format!(r#"{{"name":"{name}","id":"{id}"}}"#),
            None => format!(r#"{{"name":"{name}","id":null}}"#),
        }
    }

    #[test]
    fn totals_add_up_and_artists_and_albums_are_credited_by_their_plays() {
        let a1 = format!("[{}]", artist("Ann", Some("UC_ann")));
        let a2 = format!("[{},{}]", artist("Ann", Some("UC_ann")), artist("Bob", None));
        let totals = vec![
            total("a", 5, 100, &a1, Some(("First", Some("MPREb_1")))),
            total("b", 4, 90, &a2, Some(("First", Some("MPREb_1")))),
            total("c", 3, 80, &format!("[{}]", artist("Bob", None)), Some(("Other", None))),
        ];
        let report = build(&totals);
        assert_eq!(report.plays, 12);
        assert_eq!(report.listened_ms, 12 * 180_000);
        assert_eq!(report.tracks.len(), 3);
        assert_eq!(report.tracks[0].plays, 5);
        // Ann: 5 + 4, Bob (no id, matched by name): 4 + 3.
        assert_eq!(
            report.artists.iter().map(|a| (a.name.as_str(), a.plays)).collect::<Vec<_>>(),
            vec![("Ann", 9), ("Bob", 7)]
        );
        assert_eq!(report.top_artist().unwrap().name, "Ann");
        assert_eq!(
            report.albums.iter().map(|a| (a.title.as_str(), a.plays)).collect::<Vec<_>>(),
            vec![("First", 9), ("Other", 3)]
        );
        assert_eq!(report.albums[0].artist.as_deref(), Some("Ann"));
        assert_eq!(report.artists[0].art.as_deref(), Some("https://example.com/a.jpg"));
    }

    #[test]
    fn lists_stop_at_ten() {
        let totals: Vec<PlayTotal> = (0..30)
            .map(|i| total(&format!("v{i}"), 30 - i, 100, &format!("[{}]", artist(&format!("A{i}"), None)), Some((&format!("Al{i}"), None))))
            .collect();
        let report = build(&totals);
        assert_eq!(
            (report.tracks.len(), report.artists.len(), report.albums.len()),
            (LIST_LEN, LIST_LEN, LIST_LEN)
        );
    }

    #[test]
    fn nothing_played_is_an_empty_report() {
        let report = build(&[]);
        assert!(report.is_empty());
        assert!(report.top_artist().is_none());
    }

    #[test]
    fn an_unreadable_artist_list_still_counts_the_play() {
        let report = build(&[total("a", 2, 1, "not json", None)]);
        assert_eq!(report.plays, 2);
        assert!(report.artists.is_empty());
        assert_eq!(report.tracks.len(), 1);
    }

    #[test]
    fn periods_start_where_they_say() {
        let now = 10_000_000;
        assert_eq!(Period::Weeks4.since(now), Some(now - 28 * DAY_SECS));
        assert_eq!(Period::Months6.since(now), Some(now - 182 * DAY_SECS));
        assert_eq!(Period::AllTime.since(now), None);
        assert_eq!(Period::default(), Period::Weeks4);
    }

    fn many(n: usize, plays: u64, last_played: i64) -> Vec<PlayTotal> {
        (0..n)
            .map(|i| total(&format!("t{i}"), plays, last_played, "[]", None))
            .collect()
    }

    #[test]
    fn a_shelf_is_hidden_until_it_has_six_tracks() {
        let now = 100 * DAY_SECS;
        assert!(personal_shelves(&many(5, 3, now), &[], now).is_empty());
        let shelves = personal_shelves(&many(6, 3, now), &[], now);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].title, "On repeat");
        assert_eq!(shelves[0].items.len(), 6);
        assert!(shelves[0].is_songs());
    }

    #[test]
    fn on_repeat_needs_a_repeat() {
        let now = 100 * DAY_SECS;
        // Eight tracks played once each in the last month: nothing is on repeat.
        assert!(personal_shelves(&many(8, 1, now), &[], now).is_empty());
    }

    #[test]
    fn forgotten_favourites_are_played_a_lot_and_not_lately() {
        let now = 200 * DAY_SECS;
        let long_ago = now - 61 * DAY_SECS;
        let lately = now - 10 * DAY_SECS;
        let mut all = many(6, 8, long_ago);
        all.extend(many(3, 8, lately).into_iter().map(|mut t| {
            t.video_id = format!("recent-{}", t.video_id);
            t
        }));
        all.extend(many(4, 2, long_ago).into_iter().map(|mut t| {
            t.video_id = format!("rare-{}", t.video_id);
            t
        }));
        let shelves = personal_shelves(&[], &all, now);
        assert_eq!(shelves.len(), 1);
        assert_eq!(shelves[0].title, "Forgotten favourites");
        assert_eq!(shelves[0].items.len(), 6, "only the well-played, long-unplayed ones");
    }

    #[test]
    fn a_shelf_holds_at_most_twenty() {
        let now = 100 * DAY_SECS;
        let shelves = personal_shelves(&many(40, 3, now), &[], now);
        assert_eq!(shelves[0].items.len(), SHELF_MAX);
    }

    #[test]
    fn listening_times_read_plainly() {
        assert_eq!(listening_time(0), "0 min");
        assert_eq!(listening_time(45 * 60_000), "45 min");
        assert_eq!(listening_time(3 * 3_600_000), "3 h");
        assert_eq!(listening_time(12 * 3_600_000 + 30 * 60_000 + 59_000), "12 h 30 min");
    }
}
