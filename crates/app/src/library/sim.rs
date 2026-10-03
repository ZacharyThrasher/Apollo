//! Simulated library for the default (no account, no network) backend and the tests. It covers
//! the shapes the screen has to survive: Liked Music, several playlists and albums, a long
//! paged playlist, an empty one and one that always fails to load.

use std::time::Duration;

use apollo_domain::library::{Collection, CollectionKind, LIKED_MUSIC_BROWSE_ID};
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};

use super::source::{LibrarySource, TrackRow};
use crate::art::{DecodedArt, SyntheticArt};
use crate::backend::sim::{Fixture, FIXTURES};

const LATENCY: Duration = Duration::from_millis(250);
const PAGE_LATENCY: Duration = Duration::from_millis(120);
/// Rows per page of the long playlist.
pub const PAGE_SIZE: usize = 100;
/// Length of the long playlist: enough that drawing every row would be visibly slow.
pub const LONG_LEN: usize = 3_000;

pub const LONG_ID: &str = "VLPLsimlong";
pub const EMPTY_ID: &str = "VLPLsimempty";
pub const BROKEN_ID: &str = "VLPLsimbroken";
const ROAD_TRIP_ID: &str = "VLPLsimroad";

/// Prefix of the generated track ids; the remaining five characters are the position.
pub const GENERATED_PREFIX: &str = "simLib";

#[derive(Clone, Debug, Default)]
pub struct SimLibrary {
    /// Skips the artificial latency (tests).
    instant: bool,
}

impl SimLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn instant() -> Self {
        Self { instant: true }
    }

    fn wait(&self, d: Duration) {
        if !self.instant {
            std::thread::sleep(d);
        }
    }
}

fn playlist(id: &str, title: &str, subtitle: &str, hue: Option<f32>) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: Some(subtitle.into()),
        thumbnail: hue.map(|h| format!("sim://art/{h}")),
    }
}

fn album(id: &str, title: &str, subtitle: &str, hue: Option<f32>) -> Collection {
    Collection {
        kind: CollectionKind::Album,
        id: BrowseId::new(id),
        title: title.into(),
        subtitle: Some(subtitle.into()),
        thumbnail: hue.map(|h| format!("sim://art/{h}")),
    }
}

fn fixture_row(f: &Fixture, n: usize) -> TrackRow {
    let mut t = Track::new(f.id, f.title);
    t.artists = vec![ArtistRef {
        name: f.artist.into(),
        id: None,
    }];
    t.album = Some(AlbumRef {
        title: f.album.into(),
        id: None,
    });
    t.duration = Some(Duration::from_secs(f.secs));
    TrackRow {
        track: t,
        occurrence: Some(format!("occ-{}-{n}", f.id)),
        thumbnail: match f.art {
            Some(SyntheticArt::Hue(h)) => Some(format!("sim://art/{h}")),
            Some(SyntheticArt::Gray) => Some("sim://art/gray".into()),
            None => None,
        },
        explicit: n % 4 == 3,
    }
}

fn generated_row(n: usize) -> TrackRow {
    let id = format!("{GENERATED_PREFIX}{n:05}");
    let mut t = Track::new(id.clone(), format!("Generated Track {}", n + 1));
    t.artists = vec![ArtistRef {
        name: format!("Artist {}", n % 17 + 1),
        id: None,
    }];
    t.album = Some(AlbumRef {
        title: format!("Album {}", n % 40 + 1),
        id: None,
    });
    t.duration = Some(Duration::from_secs(120 + (n as u64 * 7) % 240));
    TrackRow {
        track: t,
        occurrence: Some(format!("occ-{id}")),
        thumbnail: Some(format!("sim://art/{}", (n % 12) * 30)),
        explicit: false,
    }
}

fn fixture_rows(ids: &[usize]) -> Vec<TrackRow> {
    ids.iter()
        .enumerate()
        .map(|(n, &i)| fixture_row(&FIXTURES[i], n))
        .collect()
}

/// The collections the simulation lists, in service order (Liked Music first, as the service
/// puts it).
pub fn all_collections() -> Vec<Collection> {
    let mut list = vec![
        playlist(LIKED_MUSIC_BROWSE_ID, "Liked Music", "Auto playlist", None),
        playlist(ROAD_TRIP_ID, "Road Trip", "Playlist • 3 songs", Some(35.0)),
        playlist(LONG_ID, "Long Haul", "Playlist • 3,000 songs", Some(215.0)),
        playlist(EMPTY_ID, "Nothing Yet", "Playlist • 0 songs", None),
        playlist(BROKEN_ID, "Won't Load", "Playlist", Some(10.0)),
        playlist(
            "VLPLsimlongtitle",
            "A Playlist With A Remarkably Long Title That Will Not Fit On One Line Of The List",
            "Playlist • An owner with a long name • 12 songs",
            Some(320.0),
        ),
    ];
    for i in 1..=40 {
        list.push(playlist(
            &format!("VLPLsimmix{i:03}"),
            &format!("Mix {i:03}"),
            &format!("Playlist • {} songs", i % 7 + 1),
            (i % 3 != 0).then_some((i * 37 % 360) as f32),
        ));
    }
    list.extend([
        album(
            "MPREbsimnight",
            "Night Shift",
            "Album • Neon Arcade • 2024",
            Some(35.0),
        ),
        album(
            "MPREbsimgreen",
            "Greenhouse",
            "Album • Soft Lantern • 2023",
            Some(215.0),
        ),
        album(
            "MPREbsimlong",
            "Long Names",
            "Album • An Artist With A Similarly Long Name • 2022",
            Some(320.0),
        ),
        album("MPREbsimgrey", "Greys", "Album • Monochrome • 2021", None),
    ]);
    list
}

fn rows_for(id: &str) -> Option<Vec<TrackRow>> {
    Some(match id {
        LIKED_MUSIC_BROWSE_ID => fixture_rows(&[0, 1, 2, 3, 4, 5]),
        ROAD_TRIP_ID => fixture_rows(&[0, 1, 5]),
        EMPTY_ID => Vec::new(),
        "MPREbsimnight" => fixture_rows(&[0]),
        "MPREbsimgreen" => fixture_rows(&[1]),
        "MPREbsimlong" => fixture_rows(&[2]),
        "MPREbsimgrey" => fixture_rows(&[3]),
        LONG_ID => (0..LONG_LEN).map(generated_row).collect(),
        id if id.starts_with("VLPLsim") => fixture_rows(&[2, 0, 1]),
        _ => return None,
    })
}

impl LibrarySource for SimLibrary {
    fn collections(&self) -> Result<Vec<Collection>, LoadFailure> {
        self.wait(LATENCY);
        Ok(all_collections())
    }

    fn tracks(&self, collection: &Collection) -> Result<Page<TrackRow>, LoadFailure> {
        self.wait(LATENCY);
        let id = collection.id.as_str();
        if id == BROKEN_ID {
            return Err(LoadFailure::Network);
        }
        let rows = rows_for(id).ok_or(LoadFailure::Other)?;
        Ok(page_of(id, rows, 0))
    }

    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        // Token: "<collection id>@<offset>".
        let (id, offset) = token.rsplit_once('@').ok_or(LoadFailure::Parse)?;
        let offset: usize = offset.parse().map_err(|_| LoadFailure::Parse)?;
        let rows = rows_for(id).ok_or(LoadFailure::Other)?;
        Ok(page_of(id, rows, offset))
    }

    fn artwork(&self, url: &str) -> Option<DecodedArt> {
        let kind = url.strip_prefix("sim://art/")?;
        let art = if kind == "gray" {
            SyntheticArt::Gray
        } else {
            SyntheticArt::Hue(kind.parse().ok()?)
        };
        Some(DecodedArt::from_rgba(art.render(96)))
    }
}

fn page_of(id: &str, rows: Vec<TrackRow>, offset: usize) -> Page<TrackRow> {
    // Only the long playlist is paged; everything else arrives in one page.
    let size = if id == LONG_ID { PAGE_SIZE } else { usize::MAX };
    let end = offset.saturating_add(size).min(rows.len());
    let continuation = (end < rows.len()).then(|| format!("{id}@{end}"));
    Page {
        items: rows[offset.min(end)..end].to_vec(),
        continuation,
        skipped: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_playlist_pages_cover_every_row_once() {
        let lib = SimLibrary::instant();
        let long = all_collections()
            .into_iter()
            .find(|c| c.id.as_str() == LONG_ID)
            .unwrap();
        let mut page = lib.tracks(&long).unwrap();
        let mut all = page.items.clone();
        while let Some(token) = page.continuation.clone() {
            page = lib.more_tracks(&token).unwrap();
            all.extend(page.items.clone());
        }
        assert_eq!(all.len(), LONG_LEN);
        let mut keys: Vec<_> = all.iter().filter_map(|r| r.occurrence.clone()).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), LONG_LEN, "occurrence tokens are unique");
        assert!(all.iter().all(|r| r.track.video_id.as_str().len() == 11));
    }

    #[test]
    fn broken_playlist_fails_and_empty_one_is_empty() {
        let lib = SimLibrary::instant();
        let find = |id: &str| {
            all_collections()
                .into_iter()
                .find(|c| c.id.as_str() == id)
                .unwrap()
        };
        assert_eq!(
            lib.tracks(&find(BROKEN_ID)).err(),
            Some(LoadFailure::Network)
        );
        let empty = lib.tracks(&find(EMPTY_ID)).unwrap();
        assert!(empty.items.is_empty() && empty.continuation.is_none());
    }

    #[test]
    fn artwork_decodes_for_hues_and_gray_and_not_for_other_urls() {
        let lib = SimLibrary::instant();
        assert!(lib.artwork("sim://art/35").is_some());
        assert!(lib.artwork("sim://art/gray").is_some());
        assert!(lib.artwork("https://example.com/a.jpg").is_none());
    }
}
