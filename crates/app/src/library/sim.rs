//! Simulated library for the default (no account, no network) backend and the tests. It covers
//! the shapes the screen has to survive: Liked Music, several playlists and albums, a long
//! paged playlist, an empty one and one that always fails to load.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_domain::library::{
    bare_playlist_id, Collection, CollectionKind, PageMeta, PlaylistTarget, WriteFailure,
    LIKED_MUSIC_BROWSE_ID,
};
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_provider::{ArtistSection, Hit, HomeItem, HomePage, HomeShelf, ListedTrack};

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
    /// Makes `radio` fail (autoplay's error path).
    radio: Option<LoadFailure>,
    /// The seeds `radio` was asked about, in order.
    radio_seeds: Vec<String>,
    /// Playlists made through `create_playlist`, newest first: `(bare id, title)`.
    created: Vec<(String, String)>,
    /// Titles changed through `rename_playlist`, by bare id.
    titles: HashMap<String, String>,
    /// Bare ids of playlists deleted through `delete_playlist`.
    deleted: HashSet<String>,
    /// A playlist's rows once an edit made them differ from the fixed ones (or it was created):
    /// authoritative from then on, by bare id.
    edited_rows: HashMap<String, Vec<TrackRow>>,
    /// Library keys saved through `set_saved` and library keys taken out of it.
    saved: Vec<String>,
    unsaved: HashSet<String>,
    /// The account the simulation says writes go to (see `LibrarySource::account_token`).
    account: Option<String>,
    /// Makes `collections` fail (a read-back that cannot be made).
    collections_failure: Option<LoadFailure>,
    /// Counter for the ids of created playlists.
    next_playlist: usize,
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
            radio: None,
            radio_seeds: Vec::new(),
            created: Vec::new(),
            titles: HashMap::new(),
            deleted: HashSet::new(),
            edited_rows: HashMap::new(),
            saved: Vec::new(),
            unsaved: HashSet::new(),
            account: None,
            collections_failure: None,
            next_playlist: 1,
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
    pub fn set_radio_failure(&self, failure: Option<LoadFailure>) {
        self.state().radio = failure;
    }

    /// The seeds the radio was asked for, in order.
    pub fn radio_seeds(&self) -> Vec<String> {
        self.state().radio_seeds.clone()
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

    /// The account writes are said to go to (tests change it to act as an account switch).
    pub fn set_account(&self, account: Option<&str>) {
        self.state().account = account.map(str::to_owned);
    }

    /// Makes `collections` fail (tests of a read-back that cannot be made).
    pub fn set_collections_failure(&self, failure: Option<LoadFailure>) {
        self.state().collections_failure = failure;
    }

    /// The playlists the account can edit now, bare ids, newest created first.
    pub fn owned_ids(&self) -> Vec<String> {
        let state = self.state();
        owned_ids(&state)
    }

    /// The bare ids of the rows of a playlist the simulation holds now, in order (tokens).
    pub fn occurrences(&self, bare_id: &str) -> Vec<String> {
        self.rows(&format!("VL{bare_id}"))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|r| r.occurrence)
            .collect()
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
        return Some(ArtistContent {
            name: "Quiet Artist".into(),
            thumbnail: None,
            top_songs: Vec::new(),
            sections: Vec::new(),
            skipped: 0,
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
    Some(ArtistContent {
        name: name.into(),
        thumbnail: art_url(&FIXTURES[own[0]]),
        top_songs: fixture_rows(&own),
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
        let bare = id.strip_prefix("VL").unwrap_or(id);
        if let Some(rows) = self.state().edited_rows.get(bare) {
            return Some(rows.clone());
        }
        let mut rows = rows_for(id)?;
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

    /// A write received: logged, and answered as the behaviour says. The first is whether the
    /// write lands (a lost reply lands it too), the second what the caller is told.
    fn begin_write(&self, what: String, subject: &str) -> (bool, Result<(), WriteFailure>) {
        let mut state = self.state();
        state.log.push((what, subject.to_owned()));
        match state.writes {
            WriteBehaviour::Succeed => (true, Ok(())),
            WriteBehaviour::Fail(f) => (false, Err(f)),
            WriteBehaviour::LandsReplyLost => (true, Err(WriteFailure::Unknown)),
        }
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
        let state = self.state();
        if let Some(f) = state.collections_failure {
            return Err(f);
        }
        Ok(library_now(&state))
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

    fn page(&self, collection: &Collection) -> Result<(Page<TrackRow>, PageMeta), LoadFailure> {
        let page = self.tracks(collection)?;
        let key = collection.library_key().to_owned();
        let state = self.state();
        let meta = match collection.kind {
            CollectionKind::Playlist => PageMeta {
                owned: owned_ids(&state).contains(&key),
                save_id: Some(key),
                in_library: None,
            },
            CollectionKind::Album => PageMeta {
                owned: false,
                save_id: Some(album_save_id(&key)),
                in_library: Some(library_now(&state).iter().any(|c| c.library_key() == key)),
            },
        };
        Ok((page, meta))
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
        artist_content(id.as_str()).ok_or(LoadFailure::Other)
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
        Ok(owned_list(&state)
            .into_iter()
            .map(|(id, title)| {
                let road_trip_holds = id == "PLsimroad"
                    && ROAD_TRIP_FIXTURES
                        .iter()
                        .any(|&i| FIXTURES[i].id == video_id);
                let held = state
                    .edited_rows
                    .get(&id)
                    .is_some_and(|rows| rows.iter().any(|r| r.track.video_id.as_str() == video_id));
                PlaylistTarget {
                    holds_track: road_trip_holds
                        || held
                        || state.added.iter().any(|(p, v)| *p == id && v == video_id),
                    id,
                    title,
                    thumbnail: None,
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
                if let Some(rows) = state.edited_rows.get_mut(playlist_id) {
                    let n = rows.len();
                    let mut row = liked_row(video_id, n);
                    row.occurrence = Some(format!("occ-added-{playlist_id}-{video_id}"));
                    row.liked = None;
                    rows.push(row);
                }
            }
        }
        lands.map(|_| ())
    }

    fn account_token(&self) -> Option<String> {
        self.state().account.clone()
    }

    fn editable_playlists(&self) -> Result<Vec<String>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let state = self.state();
        if let Some(f) = state.playlists {
            return Err(f);
        }
        Ok(owned_ids(&state))
    }

    fn save_target(&self, collection: &Collection) -> Result<String, LoadFailure> {
        match collection.kind {
            CollectionKind::Playlist => Ok(collection.library_key().to_owned()),
            CollectionKind::Album => Ok(album_save_id(collection.library_key())),
        }
    }

    fn create_playlist(&self, title: &str) -> Result<String, WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) = self.begin_write("create".to_owned(), title);
        let id = {
            let mut state = self.state();
            let id = format!("PLsimnew{:03}", state.next_playlist);
            if applies {
                state.next_playlist += 1;
                state.created.insert(0, (id.clone(), title.to_owned()));
                state.edited_rows.insert(id.clone(), Vec::new());
            }
            id
        };
        reply.map(|()| id)
    }

    fn delete_playlist(&self, playlist_id: &str) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) = self.begin_write("delete".to_owned(), playlist_id);
        if applies {
            self.state().deleted.insert(playlist_id.to_owned());
        }
        reply
    }

    fn rename_playlist(&self, playlist_id: &str, title: &str) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) = self.begin_write(format!("rename:{title}"), playlist_id);
        if applies {
            self.state()
                .titles
                .insert(playlist_id.to_owned(), title.to_owned());
        }
        reply
    }

    fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        occurrence: &str,
    ) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) = self.begin_write(format!("remove:{playlist_id}"), video_id);
        if applies {
            let mut rows = self.rows(&format!("VL{playlist_id}")).unwrap_or_default();
            rows.retain(|r| r.occurrence.as_deref() != Some(occurrence));
            self.state().edited_rows.insert(playlist_id.to_owned(), rows);
        }
        reply
    }

    fn move_in_playlist(
        &self,
        playlist_id: &str,
        occurrence: &str,
        before: Option<&str>,
    ) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) = self.begin_write(format!("move:{playlist_id}"), occurrence);
        if applies {
            let mut rows = self.rows(&format!("VL{playlist_id}")).unwrap_or_default();
            if let Some(from) = rows
                .iter()
                .position(|r| r.occurrence.as_deref() == Some(occurrence))
            {
                let row = rows.remove(from);
                let at = before
                    .and_then(|b| rows.iter().position(|r| r.occurrence.as_deref() == Some(b)))
                    .unwrap_or(rows.len());
                rows.insert(at, row);
                self.state().edited_rows.insert(playlist_id.to_owned(), rows);
            }
        }
        reply
    }

    fn set_saved(&self, save_id: &str, saved: bool) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) =
            self.begin_write(if saved { "save" } else { "unsave" }.to_owned(), save_id);
        if applies {
            let key = save_id.strip_prefix(ALBUM_SAVE_PREFIX).unwrap_or(save_id).to_owned();
            let mut state = self.state();
            if saved {
                state.unsaved.remove(&key);
                if !state.saved.contains(&key) {
                    state.saved.push(key);
                }
            } else {
                state.saved.retain(|k| *k != key);
                state.unsaved.insert(key);
            }
        }
        reply
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

/// What an album is saved on in the simulation: its browse id behind the audio-playlist prefix.
const ALBUM_SAVE_PREFIX: &str = "OLAK5uy_sim_";

fn album_save_id(browse_id: &str) -> String {
    format!("{ALBUM_SAVE_PREFIX}{browse_id}")
}

/// The playlists the simulated account owns now, `(bare id, title)`: the ones it made, newest
/// first, then the fixed ones, minus what was deleted, under the titles they were renamed to.
fn owned_list(state: &SimState) -> Vec<(String, String)> {
    let title = |id: &str, default: &str| {
        state
            .titles
            .get(id)
            .cloned()
            .unwrap_or_else(|| default.to_owned())
    };
    state
        .created
        .iter()
        .map(|(id, t)| (id.clone(), t.clone()))
        .chain(OWNED.iter().map(|(id, t)| ((*id).to_owned(), (*t).to_owned())))
        .filter(|(id, _)| !state.deleted.contains(id))
        .map(|(id, t)| {
            let t = title(&id, &t);
            (id, t)
        })
        .collect()
}

fn owned_ids(state: &SimState) -> Vec<String> {
    owned_list(state).into_iter().map(|(id, _)| id).collect()
}

/// The library as the simulated service lists it now: Liked Music first, then what was created,
/// the fixed collections minus what was deleted or unsaved (renamed ones under their new title),
/// and the albums saved since.
fn library_now(state: &SimState) -> Vec<Collection> {
    let mut list = all_collections();
    for c in &mut list {
        if let Some(title) = state.titles.get(bare_playlist_id(c.id.as_str())) {
            c.title = title.clone();
        }
    }
    list.retain(|c| {
        let key = c.library_key();
        !state.deleted.contains(key) && !state.unsaved.contains(key)
    });
    let at = usize::from(list.first().is_some_and(Collection::is_liked_music));
    for (n, (id, title)) in state.created.iter().enumerate() {
        if state.deleted.contains(id) {
            continue;
        }
        list.insert(
            at + n,
            Collection {
                kind: CollectionKind::Playlist,
                id: BrowseId::new(format!("VL{id}")),
                title: state.titles.get(id).cloned().unwrap_or_else(|| title.clone()),
                subtitle: Some("Playlist".into()),
                thumbnail: None,
            },
        );
    }
    for key in &state.saved {
        if list.iter().any(|c| c.library_key() == key) {
            continue;
        }
        if let Some(name) = key.strip_prefix(SEARCH_ALBUM_PREFIX) {
            list.push(Collection {
                kind: CollectionKind::Album,
                id: BrowseId::new(key.clone()),
                title: name.to_owned(),
                subtitle: Some("Album".into()),
                thumbnail: None,
            });
        }
    }
    list
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
    fn artwork_decodes_for_hues_and_gray_and_not_for_other_urls() {
        let lib = SimLibrary::instant();
        assert!(lib.artwork("sim://art/35").is_some());
        assert!(lib.artwork("sim://art/gray").is_some());
        assert!(lib.artwork("https://example.com/a.jpg").is_none());
    }
}
