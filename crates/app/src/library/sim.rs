//! Simulated library for the default (no account, no network) backend and the tests. It covers
//! the shapes the screen has to survive: Liked Music, several playlists and albums, a long
//! paged playlist, an empty one and one that always fails to load.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_domain::library::{
    Collection, CollectionKind, PlaylistTarget, WriteFailure, LIKED_MUSIC_BROWSE_ID,
};
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_provider::{
    ArtistSection, HistoryDay, Hit, HomeItem, HomePage, HomeShelf, ListedTrack, RelatedArtist,
};

use super::source::{ArtistContent, LibrarySource, TrackRow};
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

/// Browse ids search results carry in the simulation: an album is every fixture of that title,
/// an artist is every fixture by that name, a single is one fixture's track.
pub const SEARCH_ALBUM_PREFIX: &str = "MPREb_sim_";
pub const SEARCH_ARTIST_PREFIX: &str = "UCsim_";
const SINGLE_PREFIX: &str = "MPREb_sim_single_";
/// Artist pages for the states: one that fails, one with nothing on it, one that answers slowly.
pub const ARTIST_BROKEN_ID: &str = "UCsimbroken";
pub const ARTIST_EMPTY_ID: &str = "UCsimempty";
pub const ARTIST_SLOW_ID: &str = "UCsimslow";
const SLOW_LATENCY: Duration = Duration::from_millis(1_500);

/// Prefix of the generated track ids; the remaining five characters are the position.
pub const GENERATED_PREFIX: &str = "simLib";

/// Where the simulated radio's tracks start numbering, and how many one answer holds.
const RADIO_BASE: usize = 5_000;
const RADIO_BATCH: usize = 10;

/// Token of the home feed's second and third batches.
pub const HOME_SECOND: &str = "sim-home@2";
pub const HOME_THIRD: &str = "sim-home@3";
/// Songs on the simulated "Quick picks" shelf: enough for several pages of columns.
pub const QUICK_PICKS: usize = 20;

/// How the simulated service answers the home feed (tests drive the states with this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HomeBehaviour {
    #[default]
    Feed,
    /// Every request fails.
    Fail(LoadFailure),
    /// The feed reads cleanly and holds nothing.
    Empty,
    /// The first batch is fine and every later one fails.
    FailMore(LoadFailure),
}

/// How the simulated service answers a write (tests drive the failure paths with this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum WriteBehaviour {
    #[default]
    Succeed,
    /// The service refuses (or, for `Unknown`, the reply is lost): the write does not land.
    Fail(WriteFailure),
    /// The write lands, but the reply is lost, so the caller sees an unknown outcome.
    LandsReplyLost,
}

/// Playlists the simulated account can add to, in the order the picker lists them. A playlist
/// id here is the bare one (no `VL`).
const OWNED: &[(&str, &str)] = &[
    ("PLsimroad", "Road Trip"),
    ("PLsimlong", "Long Haul"),
    ("PLsimempty", "Nothing Yet"),
    (
        "PLsimlongtitle",
        "A Playlist With A Remarkably Long Title That Will Not Fit On One Line Of The List",
    ),
    ("PLsimmix001", "Mix 001"),
    ("PLsimmix002", "Mix 002"),
    ("PLsimmix003", "Mix 003"),
    ("PLsimmix004", "Mix 004"),
    ("PLsimmix005", "Mix 005"),
    ("PLsimmix006", "Mix 006"),
    ("PLsimmix007", "Mix 007"),
    ("PLsimmix008", "Mix 008"),
];
/// The fixture positions Road Trip starts with (see `rows_for`).
const ROAD_TRIP_FIXTURES: [usize; 3] = [0, 1, 5];

#[derive(Debug)]
struct SimState {
    /// Video ids in Liked Music, most recently liked first.
    liked: Vec<String>,
    /// `(bare playlist id, video id)` added through `add_to_playlist`, oldest first.
    added: Vec<(String, String)>,
    writes: WriteBehaviour,
    /// Makes `playlists_for` fail (the picker's error state).
    playlists: Option<LoadFailure>,
    /// Writes the simulation received, `(what, video id)`, for tests.
    log: Vec<(String, String)>,
    home: HomeBehaviour,
    /// Home requests the simulation received: `""` for the first batch, else the token.
    home_log: Vec<String>,
    /// Makes the Artists and History tabs fail to load.
    tabs: Option<LoadFailure>,
    /// Makes `radio` fail (autoplay's error path).
    radio: Option<LoadFailure>,
    /// The seeds `radio` was asked about, in order.
    radio_seeds: Vec<String>,
    /// Channel ids the account is subscribed to.
    subscribed: Vec<String>,
    /// Mix playlist ids `mix` was asked for, in order.
    mixes: Vec<String>,
    /// Makes `artist` fail once the page has been seen (the read-back after an unknown write).
    artist_fails: Option<LoadFailure>,
}

impl SimState {
    fn new() -> Self {
        Self {
            liked: FIXTURES.iter().take(6).map(|f| f.id.to_owned()).collect(),
            added: Vec::new(),
            writes: WriteBehaviour::default(),
            playlists: None,
            log: Vec::new(),
            home: HomeBehaviour::default(),
            home_log: Vec::new(),
            tabs: None,
            radio: None,
            radio_seeds: Vec::new(),
            subscribed: Vec::new(),
            mixes: Vec::new(),
            artist_fails: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SimLibrary {
    /// Skips the artificial latency (tests).
    instant: bool,
    state: Arc<Mutex<SimState>>,
}

impl Default for SimLibrary {
    fn default() -> Self {
        Self {
            instant: false,
            state: Arc::new(Mutex::new(SimState::new())),
        }
    }
}

impl SimLibrary {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn instant() -> Self {
        Self {
            instant: true,
            ..Self::default()
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SimState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// How the next writes are answered (tests).
    pub fn set_write_behaviour(&self, behaviour: WriteBehaviour) {
        self.state().writes = behaviour;
    }

    /// How the home feed is answered from now on (tests).
    pub fn set_home_behaviour(&self, behaviour: HomeBehaviour) {
        self.state().home = behaviour;
    }

    /// The home requests received, `""` for a first batch and the token for a following one.
    pub fn home_log(&self) -> Vec<String> {
        self.state().home_log.clone()
    }

    /// Makes the radio fail (tests).
    pub fn set_tabs_failure(&self, failure: Option<LoadFailure>) {
        self.state().tabs = failure;
    }

    pub fn set_radio_failure(&self, failure: Option<LoadFailure>) {
        self.state().radio = failure;
    }

    /// The seeds the radio was asked for, in order.
    pub fn radio_seeds(&self) -> Vec<String> {
        self.state().radio_seeds.clone()
    }

    /// The channels the account is subscribed to, in the order they were subscribed.
    pub fn subscribed_channels(&self) -> Vec<String> {
        self.state().subscribed.clone()
    }

    /// Subscribes (or unsubscribes) the account without going through a write, so a page can be
    /// opened for an account that already holds the subscription (tests).
    pub fn preset_subscription(&self, channel_id: &str, subscribed: bool) {
        let mut state = self.state();
        state.subscribed.retain(|c| c != channel_id);
        if subscribed {
            state.subscribed.push(channel_id.to_owned());
        }
    }

    /// The mix playlists the simulation was asked for, in order.
    pub fn mix_log(&self) -> Vec<String> {
        self.state().mixes.clone()
    }

    /// Makes every artist page request fail from now on (tests: a read-back that cannot be read).
    pub fn set_artist_failure(&self, failure: Option<LoadFailure>) {
        self.state().artist_fails = failure;
    }

    /// Makes the playlist list fail (tests).
    pub fn set_playlists_failure(&self, failure: Option<LoadFailure>) {
        self.state().playlists = failure;
    }

    /// Video ids in Liked Music, most recently liked first.
    pub fn liked_ids(&self) -> Vec<String> {
        self.state().liked.clone()
    }

    /// `(bare playlist id, video id)` the service holds from `add_to_playlist` calls.
    pub fn added(&self) -> Vec<(String, String)> {
        self.state().added.clone()
    }

    /// Every write the service received, `("like" | "unlike" | "add:<playlist>", video id)`, in
    /// order. A write that failed before landing is in here too.
    pub fn write_log(&self) -> Vec<(String, String)> {
        self.state().log.clone()
    }

    /// Liked Music as the service holds it now.
    fn liked_rows(&self) -> Vec<TrackRow> {
        self.state()
            .liked
            .iter()
            .enumerate()
            .map(|(n, id)| liked_row(id, n))
            .collect()
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
        thumbnail: art_url(f),
        explicit: n % 4 == 3,
        liked: None,
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
        liked: None,
    }
}

fn fixture_rows(ids: &[usize]) -> Vec<TrackRow> {
    ids.iter()
        .enumerate()
        .map(|(n, &i)| fixture_row(&FIXTURES[i], n))
        .collect()
}

/// A Liked Music row for a video id: the fixture when it is one, else a plain row named for the
/// id (a track the simulation has no metadata for).
fn liked_row(id: &str, n: usize) -> TrackRow {
    match FIXTURES.iter().find(|f| f.id == id) {
        Some(f) => {
            let mut row = fixture_row(f, n);
            row.occurrence = Some(format!("occ-liked-{id}"));
            row.liked = Some(true);
            row
        }
        None => TrackRow {
            track: Track::new(id, format!("Liked {id}")),
            occurrence: Some(format!("occ-liked-{id}")),
            thumbnail: None,
            explicit: false,
            liked: Some(true),
        },
    }
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

fn matching(f: impl Fn(&Fixture) -> bool) -> Vec<usize> {
    FIXTURES
        .iter()
        .enumerate()
        .filter(|(_, x)| f(x))
        .map(|(i, _)| i)
        .collect()
}

fn art_url(f: &Fixture) -> Option<String> {
    match f.art {
        Some(SyntheticArt::Hue(h)) => Some(format!("sim://art/{h}")),
        Some(SyntheticArt::Gray) => Some("sim://art/gray".into()),
        None => None,
    }
}

/// The page the simulation answers for an artist id (`UCsim_<name>` from search).
fn artist_content(id: &str) -> Option<ArtistContent> {
    if id == ARTIST_EMPTY_ID {
        // No channel either: a page like this has nothing to subscribe to.
        return Some(ArtistContent {
            name: "Quiet Artist".into(),
            ..ArtistContent::default()
        });
    }
    let name = if id == ARTIST_SLOW_ID {
        "Neon Arcade"
    } else {
        id.strip_prefix(SEARCH_ARTIST_PREFIX)?
    };
    let own = matching(|f| f.artist == name);
    if own.is_empty() {
        return None;
    }
    let mut albums: Vec<Collection> = Vec::new();
    for &i in &own {
        let f = &FIXTURES[i];
        if albums.iter().all(|a| a.title != f.album) {
            albums.push(Collection {
                kind: CollectionKind::Album,
                id: BrowseId::new(format!("{SEARCH_ALBUM_PREFIX}{}", f.album)),
                title: f.album.into(),
                subtitle: Some(format!("Album • {} • 2024", f.artist)),
                thumbnail: art_url(f),
            });
        }
    }
    let singles: Vec<Collection> = own
        .iter()
        .map(|&i| {
            let f = &FIXTURES[i];
            Collection {
                kind: CollectionKind::Album,
                id: BrowseId::new(format!("{SINGLE_PREFIX}{}", f.id)),
                title: f.title.into(),
                subtitle: Some(format!("Single • {} • 2025", f.artist)),
                thumbnail: art_url(f),
            }
        })
        .collect();
    let mut related: Vec<RelatedArtist> = Vec::new();
    for f in FIXTURES.iter().filter(|f| f.artist != name) {
        if related.iter().all(|r| r.name != f.artist) {
            related.push(RelatedArtist {
                id: BrowseId::new(format!("{SEARCH_ARTIST_PREFIX}{}", f.artist)),
                name: f.artist.into(),
                thumbnail: art_url(f),
            });
        }
    }
    Some(ArtistContent {
        name: name.into(),
        thumbnail: art_url(&FIXTURES[own[0]]),
        top_songs: fixture_rows(&own),
        top_songs_id: Some(BrowseId::new("VLPLsimtop")),
        related,
        description: Some(format!(
            "{name} make the music Apollo's simulation plays. This bio runs long on purpose, so \
             the page has to clamp it to a few lines and let a click open the rest. It says \
             nothing true about anyone."
        )),
        subscribers: Some("1.2M subscribers".into()),
        monthly_listeners: Some("3.4M monthly audience".into()),
        channel_id: id.into(),
        subscribed: false,
        radio_playlist_id: Some(format!("RDEMsim_{name}")),
        shuffle_playlist_id: Some(format!("RDAOsim_{name}")),
        sections: vec![
            ArtistSection {
                title: "Albums".into(),
                items: albums,
            },
            ArtistSection {
                title: "Singles".into(),
                items: singles,
            },
        ],
        skipped: 0,
    })
}

/// The artists of the simulated library: one per fixture artist, in the order the account added
/// them (not alphabetical, so a sort has something to do).
pub fn library_artist_hits() -> Vec<Hit> {
    let mut hits: Vec<Hit> = Vec::new();
    for f in FIXTURES.iter().rev() {
        let id = format!("{SEARCH_ARTIST_PREFIX}{}", f.artist);
        if hits.iter().any(|h| h.id.as_str() == id) {
            continue;
        }
        hits.push(Hit {
            id: BrowseId::new(id),
            title: f.artist.into(),
            subtitle: Some(format!("{} songs", matching(|x| x.artist == f.artist).len())),
            thumbnail: art_url(f),
            explicit: false,
        });
    }
    hits
}

/// The simulated history, in YouTube Music's buckets. A song can play on several days.
pub fn history_days() -> Vec<HistoryDay> {
    let listed = |ids: &[usize]| -> Vec<ListedTrack> {
        fixture_rows(ids)
            .into_iter()
            .map(|r| ListedTrack {
                track: r.track,
                occurrence: r.occurrence,
                thumbnail: r.thumbnail,
                explicit: r.explicit,
                liked: r.liked,
            })
            .collect()
    };
    vec![
        HistoryDay {
            title: "Today".into(),
            items: listed(&[3, 1, 0]),
        },
        HistoryDay {
            title: "Yesterday".into(),
            items: listed(&[4, 5]),
        },
        HistoryDay {
            title: "This week".into(),
            items: listed(&[0, 2]),
        },
        HistoryDay {
            title: "Older".into(),
            items: listed(&[3]),
        },
    ]
}

/// A thumbnail URL the simulated store (`SimThumbs`) understands.
fn home_art(art: Option<SyntheticArt>) -> Option<String> {
    match art? {
        SyntheticArt::Hue(h) => Some(format!("sim://hue/{h}")),
        SyntheticArt::Gray => Some("sim://gray".into()),
    }
}

fn home_song(row: TrackRow, art: Option<SyntheticArt>) -> HomeItem {
    HomeItem::Song(ListedTrack {
        track: row.track,
        occurrence: None,
        thumbnail: home_art(art),
        explicit: false,
        liked: None,
    })
}

fn home_album(f: &Fixture) -> HomeItem {
    HomeItem::Collection(Collection {
        kind: CollectionKind::Album,
        id: BrowseId::new(format!("{SEARCH_ALBUM_PREFIX}{}", f.album)),
        title: f.album.into(),
        subtitle: Some(format!("{} • 2024", f.artist)),
        thumbnail: home_art(f.art),
    })
}

fn home_artist(f: &Fixture) -> HomeItem {
    HomeItem::Artist(Hit {
        id: BrowseId::new(format!("{SEARCH_ARTIST_PREFIX}{}", f.artist)),
        title: f.artist.into(),
        subtitle: Some("1.2M subscribers".into()),
        thumbnail: home_art(f.art),
        explicit: false,
    })
}

fn home_playlist(c: &Collection) -> HomeItem {
    let hue = c
        .thumbnail
        .as_deref()
        .and_then(|t| t.strip_prefix("sim://art/"))
        .and_then(|h| h.parse::<f32>().ok());
    HomeItem::Collection(Collection {
        thumbnail: hue.map(|h| format!("sim://hue/{h}")),
        ..c.clone()
    })
}

/// The simulated home feed: three batches, a song shelf, shelves of cards of every kind and one
/// that mixes songs with albums.
fn home_batch(token: Option<&str>) -> Option<HomePage> {
    let quick: Vec<HomeItem> = FIXTURES
        .iter()
        .enumerate()
        .map(|(n, f)| home_song(fixture_row(f, n), f.art))
        .chain((0..QUICK_PICKS.saturating_sub(FIXTURES.len())).map(|n| {
            home_song(
                generated_row(n),
                Some(SyntheticArt::Hue((n % 12) as f32 * 30.0)),
            )
        }))
        .collect();
    let albums: Vec<HomeItem> = FIXTURES.iter().map(home_album).collect();
    let artists: Vec<HomeItem> = FIXTURES.iter().map(home_artist).collect();
    let playlists: Vec<HomeItem> = all_collections()
        .iter()
        .filter(|c| c.kind == CollectionKind::Playlist && c.thumbnail.is_some())
        .filter(|c| c.id.as_str() != BROKEN_ID)
        .take(8)
        .map(home_playlist)
        .collect();
    let mixed: Vec<HomeItem> = FIXTURES
        .iter()
        .enumerate()
        .flat_map(|(n, f)| [home_song(fixture_row(f, n), f.art), home_album(f)])
        .collect();
    let shelf = |title: &str, items: Vec<HomeItem>| HomeShelf {
        title: title.into(),
        items,
    };
    Some(match token {
        None => HomePage {
            shelves: vec![
                shelf("Quick picks", quick),
                shelf("Albums for you", albums),
                shelf("Listen again", mixed),
            ],
            continuation: Some(HOME_SECOND.into()),
            skipped: 0,
        },
        Some(HOME_SECOND) => HomePage {
            shelves: vec![
                shelf("Artists you might like", artists),
                shelf("From your library", playlists),
            ],
            continuation: Some(HOME_THIRD.into()),
            skipped: 0,
        },
        Some(HOME_THIRD) => HomePage {
            shelves: vec![shelf(
                "More albums",
                FIXTURES.iter().rev().map(home_album).collect(),
            )],
            continuation: None,
            skipped: 0,
        },
        Some(_) => return None,
    })
}

fn rows_for(id: &str) -> Option<Vec<TrackRow>> {
    if let Some(single) = id.strip_prefix(SINGLE_PREFIX) {
        let at = matching(|f| f.id == single);
        return (!at.is_empty()).then(|| fixture_rows(&at));
    }
    if let Some(album) = id.strip_prefix(SEARCH_ALBUM_PREFIX) {
        let at = matching(|f| f.album == album);
        return (!at.is_empty()).then(|| fixture_rows(&at));
    }
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

impl SimLibrary {
    /// The rows of a collection: Liked Music follows the likes made here, a playlist grows by the
    /// tracks added to it, everything else is fixed.
    fn rows(&self, id: &str) -> Option<Vec<TrackRow>> {
        if id == LIKED_MUSIC_BROWSE_ID {
            return Some(self.liked_rows());
        }
        let mut rows = rows_for(id)?;
        let bare = id.strip_prefix("VL").unwrap_or(id);
        let added: Vec<String> = self
            .state()
            .added
            .iter()
            .filter(|(playlist, _)| playlist == bare)
            .map(|(_, video)| video.clone())
            .collect();
        let at = rows.len();
        rows.extend(added.iter().enumerate().map(|(n, id)| {
            let mut row = liked_row(id, at + n);
            row.occurrence = Some(format!("occ-added-{bare}-{id}"));
            row.liked = None;
            row
        }));
        Some(rows)
    }

    fn write_answer(&self, what: String, video_id: &str) -> Result<bool, WriteFailure> {
        let mut state = self.state();
        state.log.push((what, video_id.to_owned()));
        match state.writes {
            WriteBehaviour::Succeed => Ok(true),
            WriteBehaviour::Fail(f) => Err(f),
            WriteBehaviour::LandsReplyLost => Err(WriteFailure::Unknown),
        }
    }
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
        let rows = self.rows(id).ok_or(LoadFailure::Other)?;
        Ok(page_of(id, rows, 0))
    }

    fn more_tracks(&self, token: &str) -> Result<Page<TrackRow>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        // Token: "<collection id>@<offset>".
        let (id, offset) = token.rsplit_once('@').ok_or(LoadFailure::Parse)?;
        let offset: usize = offset.parse().map_err(|_| LoadFailure::Parse)?;
        let rows = self.rows(id).ok_or(LoadFailure::Other)?;
        Ok(page_of(id, rows, offset))
    }

    fn artist(&self, id: &BrowseId) -> Result<ArtistContent, LoadFailure> {
        self.wait(if id.as_str() == ARTIST_SLOW_ID {
            SLOW_LATENCY
        } else {
            LATENCY
        });
        if id.as_str() == ARTIST_BROKEN_ID {
            return Err(LoadFailure::Network);
        }
        if let Some(f) = self.state().artist_fails {
            return Err(f);
        }
        let mut content = artist_content(id.as_str()).ok_or(LoadFailure::Other)?;
        content.subscribed = self.state().subscribed.iter().any(|c| *c == content.channel_id);
        Ok(content)
    }

    fn mix(&self, playlist_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let mut state = self.state();
        state.mixes.push(playlist_id.to_owned());
        if let Some(f) = state.radio {
            return Err(f);
        }
        let first = RADIO_BASE + 1_000 + (state.mixes.len() - 1) * RADIO_BATCH;
        Ok((first..first + RADIO_BATCH).map(generated_row).collect())
    }

    fn set_subscribed(&self, channel_id: &str, subscribed: bool) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let lands = self.write_answer(
            if subscribed { "subscribe" } else { "unsubscribe" }.to_owned(),
            channel_id,
        );
        let applies = lands.is_ok() || self.state().writes == WriteBehaviour::LandsReplyLost;
        if applies {
            let mut state = self.state();
            state.subscribed.retain(|c| c != channel_id);
            if subscribed {
                state.subscribed.push(channel_id.to_owned());
            }
        }
        lands.map(|_| ())
    }

    fn library_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        self.wait(LATENCY);
        if let Some(f) = self.state().tabs {
            return Err(f);
        }
        Ok(library_artist_hits())
    }

    fn history(&self) -> Result<Vec<HistoryDay>, LoadFailure> {
        self.wait(LATENCY);
        if let Some(f) = self.state().tabs {
            return Err(f);
        }
        Ok(history_days())
    }

    fn home(&self) -> Result<HomePage, LoadFailure> {
        self.wait(LATENCY);
        let behaviour = {
            let mut state = self.state();
            state.home_log.push(String::new());
            state.home
        };
        match behaviour {
            HomeBehaviour::Fail(f) => Err(f),
            HomeBehaviour::Empty => Ok(HomePage::default()),
            HomeBehaviour::Feed | HomeBehaviour::FailMore(_) => {
                home_batch(None).ok_or(LoadFailure::Other)
            }
        }
    }

    fn home_more(&self, token: &str) -> Result<HomePage, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let behaviour = {
            let mut state = self.state();
            state.home_log.push(token.to_owned());
            state.home
        };
        match behaviour {
            HomeBehaviour::Fail(f) | HomeBehaviour::FailMore(f) => Err(f),
            _ => home_batch(Some(token)).ok_or(LoadFailure::Parse),
        }
    }

    fn radio(&self, video_id: &str) -> Result<Vec<TrackRow>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let mut state = self.state();
        state.radio_seeds.push(video_id.to_owned());
        if let Some(f) = state.radio {
            return Err(f);
        }
        // Fresh tracks every call, far past the library's generated ones.
        let first = RADIO_BASE + (state.radio_seeds.len() - 1) * RADIO_BATCH;
        Ok((first..first + RADIO_BATCH).map(generated_row).collect())
    }

    fn set_liked(&self, video_id: &str, liked: bool) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let lands = self.write_answer(if liked { "like" } else { "unlike" }.to_owned(), video_id);
        let applies = lands.is_ok() || self.state().writes == WriteBehaviour::LandsReplyLost;
        if applies {
            let mut state = self.state();
            state.liked.retain(|id| id != video_id);
            if liked {
                state.liked.insert(0, video_id.to_owned());
            }
        }
        lands.map(|_| ())
    }

    fn liked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        Ok(Some(self.state().liked.iter().any(|id| id == video_id)))
    }

    fn playlists_for(&self, video_id: &str) -> Result<Vec<PlaylistTarget>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let state = self.state();
        if let Some(f) = state.playlists {
            return Err(f);
        }
        Ok(OWNED
            .iter()
            .map(|(id, title)| {
                let road_trip_holds = *id == "PLsimroad"
                    && ROAD_TRIP_FIXTURES
                        .iter()
                        .any(|&i| FIXTURES[i].id == video_id);
                PlaylistTarget {
                    id: (*id).to_owned(),
                    title: (*title).to_owned(),
                    thumbnail: None,
                    holds_track: road_trip_holds
                        || state.added.iter().any(|(p, v)| p == id && v == video_id),
                }
            })
            .collect())
    }

    fn add_to_playlist(&self, playlist_id: &str, video_id: &str) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let lands = self.write_answer(format!("add:{playlist_id}"), video_id);
        let applies = lands.is_ok() || self.state().writes == WriteBehaviour::LandsReplyLost;
        if applies {
            let mut state = self.state();
            let key = (playlist_id.to_owned(), video_id.to_owned());
            if !state.added.contains(&key) {
                state.added.push(key);
            }
        }
        lands.map(|_| ())
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
    fn search_ids_open_the_albums_artists_and_singles_they_name() {
        let lib = SimLibrary::instant();
        let album = Collection {
            kind: CollectionKind::Album,
            id: BrowseId::new(format!("{SEARCH_ALBUM_PREFIX}Night Shift")),
            title: "Night Shift".into(),
            subtitle: None,
            thumbnail: None,
        };
        let rows = lib.tracks(&album).unwrap().items;
        assert_eq!(rows[0].track.title, "Midnight Drive");
        let artist = lib
            .artist(&BrowseId::new(format!("{SEARCH_ARTIST_PREFIX}Neon Arcade")))
            .unwrap();
        assert_eq!(artist.name, "Neon Arcade");
        assert_eq!(artist.top_songs.len(), 1);
        let titles: Vec<_> = artist.sections.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, ["Albums", "Singles"]);
        // Every card the page offers opens something.
        for card in artist.sections.iter().flat_map(|s| &s.items) {
            assert!(
                !lib.tracks(card).unwrap().items.is_empty(),
                "{}",
                card.title
            );
        }
    }

    #[test]
    fn artist_pages_reach_every_state() {
        let lib = SimLibrary::instant();
        let ask = |id: &str| lib.artist(&BrowseId::new(id));
        assert_eq!(ask(ARTIST_BROKEN_ID).unwrap_err(), LoadFailure::Network);
        assert!(ask(ARTIST_EMPTY_ID).unwrap().is_empty());
        assert_eq!(ask("UCsim_Nobody").unwrap_err(), LoadFailure::Other);
        assert!(ask(ARTIST_SLOW_ID).is_ok());
    }

    #[test]
    fn an_artist_page_carries_everything_the_page_draws() {
        let lib = SimLibrary::instant();
        let artist = lib
            .artist(&BrowseId::new(format!("{SEARCH_ARTIST_PREFIX}Neon Arcade")))
            .unwrap();
        assert!(artist.description.is_some() && artist.subscribers.is_some());
        assert!(!artist.related.is_empty());
        assert!(artist.related.iter().all(|r| r.name != "Neon Arcade"));
        assert!(!artist.subscribed);
        assert_eq!(artist.channel_id, format!("{SEARCH_ARTIST_PREFIX}Neon Arcade"));
        let show_all = Collection {
            kind: CollectionKind::Playlist,
            id: artist.top_songs_id.clone().unwrap(),
            title: "Neon Arcade".into(),
            subtitle: None,
            thumbnail: None,
        };
        assert!(!lib.tracks(&show_all).unwrap().items.is_empty());
        assert_eq!(lib.mix(artist.radio_playlist_id.as_deref().unwrap()).unwrap().len(), RADIO_BATCH);
        assert_eq!(lib.mix_log(), [format!("RDEMsim_Neon Arcade")]);
    }

    #[test]
    fn subscribing_changes_what_the_page_reads_back_and_a_lost_reply_still_lands() {
        let lib = SimLibrary::instant();
        let id = BrowseId::new(format!("{SEARCH_ARTIST_PREFIX}Soft Lantern"));
        let channel = id.as_str().to_owned();
        lib.set_subscribed(&channel, true).unwrap();
        assert!(lib.artist(&id).unwrap().subscribed);
        lib.set_subscribed(&channel, false).unwrap();
        assert!(!lib.artist(&id).unwrap().subscribed);
        lib.set_write_behaviour(WriteBehaviour::LandsReplyLost);
        assert_eq!(lib.set_subscribed(&channel, true), Err(WriteFailure::Unknown));
        assert!(lib.artist(&id).unwrap().subscribed, "the write landed anyway");
        lib.set_write_behaviour(WriteBehaviour::Fail(WriteFailure::Rejected));
        assert_eq!(lib.set_subscribed(&channel, false), Err(WriteFailure::Rejected));
        assert!(lib.artist(&id).unwrap().subscribed, "a refusal changes nothing");
        assert_eq!(
            lib.write_log().iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>(),
            ["subscribe", "unsubscribe", "subscribe", "unsubscribe"]
        );
    }

    #[test]
    fn artwork_decodes_for_hues_and_gray_and_not_for_other_urls() {
        let lib = SimLibrary::instant();
        assert!(lib.artwork("sim://art/35").is_some());
        assert!(lib.artwork("sim://art/gray").is_some());
        assert!(lib.artwork("https://example.com/a.jpg").is_none());
    }
}
