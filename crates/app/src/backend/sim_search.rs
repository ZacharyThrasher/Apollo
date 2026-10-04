//! Simulated search: the playback fixtures as a tiny catalogue, so the search screen runs with
//! no account and no network. A few special queries exercise the states a real service
//! produces: `error` fails, `many` returns a long list, `nothing` finds nothing.

use std::time::Duration;

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::links::ExternalLink;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{Hit, LinkTarget, ListedTrack, SearchResults};

use super::sim::{track_of, Fixture, FIXTURES};
use crate::art::{DecodedArt, SyntheticArt};
use crate::library::sim::{SEARCH_ALBUM_PREFIX, SEARCH_ARTIST_PREFIX};
use crate::search::SearchSource;
use crate::url_thumbs::ThumbSource;

const LATENCY: Duration = Duration::from_millis(200);

pub struct SimSearch;

/// Thumbnail "URLs" the simulated store understands: `sim://hue/<degrees>` and `sim://gray`.
fn thumb_url(art: Option<SyntheticArt>) -> Option<String> {
    match art? {
        SyntheticArt::Hue(h) => Some(format!("sim://hue/{h}")),
        SyntheticArt::Gray => Some("sim://gray".into()),
    }
}

fn song(f: &Fixture) -> ListedTrack {
    ListedTrack {
        track: track_of(f),
        occurrence: None,
        thumbnail: thumb_url(f.art),
        explicit: false,
        liked: None,
    }
}

fn hit(id: &str, title: &str, subtitle: Option<&str>, art: Option<SyntheticArt>) -> Hit {
    Hit {
        id: BrowseId::new(id),
        title: title.to_owned(),
        subtitle: subtitle.map(str::to_owned),
        thumbnail: thumb_url(art),
        explicit: false,
    }
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(needle)
}

impl SearchSource for SimSearch {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        std::thread::sleep(LATENCY);
        let q = query.to_lowercase();
        if q.contains("error") {
            return Err(LoadFailure::Network);
        }
        if q.starts_with("nothing") {
            return Ok(SearchResults::default());
        }
        if q.starts_with("many") {
            let songs = (1..=40)
                .map(|i| ListedTrack {
                    track: Track::new(format!("simMany{i:04}"), format!("Many Songs {i}")),
                    occurrence: None,
                    thumbnail: None,
                    explicit: false,
                    liked: None,
                })
                .collect();
            return Ok(SearchResults {
                songs,
                ..SearchResults::default()
            });
        }
        let matching: Vec<&Fixture> = FIXTURES
            .iter()
            .filter(|f| contains(f.title, &q) || contains(f.artist, &q) || contains(f.album, &q))
            .collect();
        let mut out = SearchResults {
            songs: matching.iter().map(|f| song(f)).collect(),
            ..SearchResults::default()
        };
        for f in &matching {
            let album_id = format!("{}{}", SEARCH_ALBUM_PREFIX, f.album);
            if !out.albums.iter().any(|a| a.id.as_str() == album_id) {
                out.albums
                    .push(hit(&album_id, f.album, Some(f.artist), f.art));
            }
            let artist_id = format!("{}{}", SEARCH_ARTIST_PREFIX, f.artist);
            if !out.artists.iter().any(|a| a.id.as_str() == artist_id) {
                out.artists.push(hit(&artist_id, f.artist, None, f.art));
            }
        }
        for (id, title, owner, art) in PLAYLISTS {
            if contains(title, &q) {
                out.playlists.push(hit(id, title, Some(owner), Some(*art)));
            }
        }
        Ok(out)
    }

    fn suggest(&self, query: &str) -> Result<Vec<String>, LoadFailure> {
        std::thread::sleep(LATENCY / 4);
        let q = query.to_lowercase();
        if q.contains("error") {
            return Err(LoadFailure::Network);
        }
        let mut out: Vec<String> = Vec::new();
        for f in FIXTURES {
            for text in [f.title, f.artist, f.album] {
                if contains(text, &q) && !out.iter().any(|o| o == text) {
                    out.push(text.to_owned());
                }
            }
        }
        out.truncate(8);
        Ok(out)
    }

    fn resolve_link(&self, link: &ExternalLink) -> Result<LinkTarget, LoadFailure> {
        std::thread::sleep(LATENCY);
        match link {
            ExternalLink::Track { video, .. } => FIXTURES
                .iter()
                .find(|f| track_of(f).video_id == *video)
                .map(|f| LinkTarget::Track(song(f)))
                .ok_or(LoadFailure::Other),
            ExternalLink::Playlist(id) => PLAYLISTS
                .iter()
                .find(|(pid, ..)| pid.strip_prefix("VL") == Some(id.as_str()))
                .map(|(pid, title, owner, art)| {
                    LinkTarget::Collection(Collection {
                        kind: CollectionKind::Playlist,
                        id: BrowseId::new(*pid),
                        title: (*title).to_owned(),
                        subtitle: Some((*owner).to_owned()),
                        thumbnail: thumb_url(Some(*art)),
                    })
                })
                .ok_or(LoadFailure::Other),
            ExternalLink::Album(_) | ExternalLink::Artist(_) => Err(LoadFailure::Other),
        }
    }
}

const PLAYLISTS: &[(&str, &str, &str, SyntheticArt)] = &[
    (
        "VLPLsim1",
        "Night Drive Mix",
        "Apollo",
        SyntheticArt::Hue(35.0),
    ),
    ("VLPLsim2", "Greys", "Apollo", SyntheticArt::Gray),
    (
        "VLPLsim3",
        "Green Rooms",
        "Apollo",
        SyntheticArt::Hue(140.0),
    ),
];

pub struct SimThumbs;

impl ThumbSource for SimThumbs {
    fn fetch(&self, url: &str) -> Option<DecodedArt> {
        let kind = match url.strip_prefix("sim://")? {
            "gray" => SyntheticArt::Gray,
            rest => SyntheticArt::Hue(rest.strip_prefix("hue/")?.parse().ok()?),
        };
        Some(DecodedArt::from_rgba(kind.render(96)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_finds_songs_and_derives_their_albums_and_artists() {
        let r = SimSearch.search("moss").unwrap();
        assert_eq!(r.songs.len(), 1);
        assert_eq!(r.albums[0].title, "Moss");
        assert_eq!(r.artists[0].title, "Verdant");
    }

    #[test]
    fn special_queries_reach_every_state() {
        assert_eq!(
            SimSearch.search("an error").unwrap_err(),
            LoadFailure::Network
        );
        assert!(SimSearch.search("nothing at all").unwrap().is_empty());
        assert_eq!(SimSearch.search("many").unwrap().songs.len(), 40);
        assert!(SimSearch.search("zzzz").unwrap().is_empty());
    }

    #[test]
    fn thumbnail_urls_round_trip() {
        let hue = thumb_url(Some(SyntheticArt::Hue(35.0))).unwrap();
        assert!(SimThumbs.fetch(&hue).is_some());
        assert!(SimThumbs.fetch("sim://gray").is_some());
        assert!(SimThumbs.fetch("https://example.test/x.jpg").is_none());
    }
}
