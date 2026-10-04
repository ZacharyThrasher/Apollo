//! Simulated search: the playback fixtures as a tiny catalogue, so the search screen runs with
//! no account and no network. A few special queries exercise the states a real service
//! produces: `error` fails, `many` returns a long list (and long, paged filtered lists),
//! `stall` is `many` whose second page fails, `nothing` finds nothing.

use std::time::Duration;

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::links::ExternalLink;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{
    Hit, LinkTarget, ListedTrack, SearchFilter, SearchPage, SearchResults, TopResult,
};

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
        if q.starts_with("many") || q.starts_with("stall") {
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
        // The first artist is the best match when one matched; otherwise the first song.
        out.top = out
            .artists
            .first()
            .cloned()
            .map(TopResult::Artist)
            .or_else(|| out.songs.first().cloned().map(TopResult::Song));
        Ok(out)
    }

    fn search_filtered(
        &self,
        query: &str,
        filter: SearchFilter,
    ) -> Result<SearchPage, LoadFailure> {
        std::thread::sleep(LATENCY);
        if query.to_lowercase().contains("error") {
            return Err(LoadFailure::Network);
        }
        Ok(filtered_page(query, filter, 0))
    }

    fn search_more(&self, token: &str) -> Result<SearchPage, LoadFailure> {
        std::thread::sleep(LATENCY);
        let (filter, offset, query) = decode_token(token).ok_or(LoadFailure::Other)?;
        // "stall" lists fail their second page, so the page's retry row can be reached.
        if query.starts_with("stall") && offset > 0 {
            return Err(LoadFailure::Network);
        }
        Ok(filtered_page(&query, filter, offset))
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

/// Rows a simulated filtered page holds.
const PAGE: usize = 20;
/// Pages "many" and "stall" run to: a list long enough to scroll and page.
const LONG_PAGES: usize = 3;

fn filter_name(filter: SearchFilter) -> &'static str {
    match filter {
        SearchFilter::Songs => "songs",
        SearchFilter::Albums => "albums",
        SearchFilter::Artists => "artists",
        SearchFilter::Playlists => "playlists",
        SearchFilter::CommunityPlaylists => "community",
    }
}

/// `sim:<filter>:<offset>:<query>`.
fn token(filter: SearchFilter, offset: usize, query: &str) -> String {
    format!("sim:{}:{offset}:{query}", filter_name(filter))
}

fn decode_token(token: &str) -> Option<(SearchFilter, usize, String)> {
    let mut parts = token.splitn(4, ':');
    (parts.next()? == "sim").then_some(())?;
    let name = parts.next()?;
    let filter = [
        SearchFilter::Songs,
        SearchFilter::Albums,
        SearchFilter::Artists,
        SearchFilter::Playlists,
        SearchFilter::CommunityPlaylists,
    ]
    .into_iter()
    .find(|f| filter_name(*f) == name)?;
    let offset = parts.next()?.parse().ok()?;
    Some((filter, offset, parts.next()?.to_owned()))
}

/// Page `offset / PAGE` of the simulated list of one kind for `query`.
fn filtered_page(query: &str, filter: SearchFilter, offset: usize) -> SearchPage {
    let q = query.to_lowercase();
    let long = q.starts_with("many") || q.starts_with("stall");
    let total = if long { PAGE * LONG_PAGES } else { 0 };
    let mut out = SearchResults::default();
    let end;
    if long {
        end = (offset + PAGE).min(total);
        for i in offset + 1..=end {
            let art = Some(SyntheticArt::Hue((i * 37 % 360) as f32));
            match filter {
                SearchFilter::Songs => out.songs.push(ListedTrack {
                    track: Track::new(format!("simMany{i:04}"), format!("Many Songs {i}")),
                    occurrence: None,
                    thumbnail: thumb_url(art),
                    explicit: false,
                    liked: None,
                }),
                SearchFilter::Albums => out.albums.push(hit(
                    &format!("{SEARCH_ALBUM_PREFIX}Many Albums {i}"),
                    &format!("Many Albums {i}"),
                    Some("Verdant"),
                    art,
                )),
                SearchFilter::Artists => out.artists.push(hit(
                    &format!("{SEARCH_ARTIST_PREFIX}Many Artists {i}"),
                    &format!("Many Artists {i}"),
                    None,
                    art,
                )),
                SearchFilter::Playlists | SearchFilter::CommunityPlaylists => out
                    .playlists
                    .push(hit("VLPLsim1", &format!("Many Playlists {i}"), Some("Apollo"), art)),
            }
        }
    } else {
        // The fixtures' own matches, on a single page.
        let matched = SimSearch.search(query).unwrap_or_default();
        out = match filter {
            SearchFilter::Songs => SearchResults {
                songs: matched.songs,
                ..SearchResults::default()
            },
            SearchFilter::Albums => SearchResults {
                albums: matched.albums,
                ..SearchResults::default()
            },
            SearchFilter::Artists => SearchResults {
                artists: matched.artists,
                ..SearchResults::default()
            },
            SearchFilter::Playlists => SearchResults {
                playlists: matched.playlists,
                ..SearchResults::default()
            },
            SearchFilter::CommunityPlaylists => SearchResults::default(),
        };
        end = 0;
    }
    let continuation = (long && end < total).then(|| token(filter, end, query));
    SearchPage {
        results: out,
        continuation,
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
    fn a_long_filtered_list_pages_to_its_end() {
        let mut page = SimSearch
            .search_filtered("many", SearchFilter::Albums)
            .unwrap();
        let mut seen = page.results.albums.len();
        while let Some(token) = page.continuation.clone() {
            page = SimSearch.search_more(&token).unwrap();
            seen += page.results.albums.len();
        }
        assert_eq!(seen, PAGE * LONG_PAGES);
    }

    #[test]
    fn a_stalling_list_fails_its_second_page() {
        let page = SimSearch
            .search_filtered("stall", SearchFilter::Songs)
            .unwrap();
        let token = page.continuation.unwrap();
        assert_eq!(
            SimSearch.search_more(&token).unwrap_err(),
            LoadFailure::Network
        );
    }

    #[test]
    fn thumbnail_urls_round_trip() {
        let hue = thumb_url(Some(SyntheticArt::Hue(35.0))).unwrap();
        assert!(SimThumbs.fetch(&hue).is_some());
        assert!(SimThumbs.fetch("sim://gray").is_some());
        assert!(SimThumbs.fetch("https://example.test/x.jpg").is_none());
    }
}
