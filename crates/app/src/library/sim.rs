//! Simulated library for the default (no account, no network) backend and the tests. It covers
//! the shapes the screen has to survive: Liked Music, several playlists and albums, a long
//! paged playlist, an empty one and one that always fails to load.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apollo_domain::library::{
    bare_playlist_id, Collection, CollectionKind, PageMeta, PlaylistEdit, PlaylistTarget, Privacy,
    WriteFailure, LIKED_MUSIC_BROWSE_ID,
};
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::{AlbumRef, ArtistRef, BrowseId, Track};
use apollo_provider::{
    ArtistSection, ExplorePage, HistoryDay, Hit, HomeChip, HomeItem, HomePage, HomeShelf,
    ListedTrack, MoodTile, RelatedArtist,
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
/// The simulated Library > Songs list, paged like the long playlist when it is long.
pub const SONGS_ID: &str = "FEmusic_liked_videos";
/// How many songs `preset_long_library_songs` puts in the list: three pages.
pub const LONG_SONGS: usize = 250;
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
/// The simulated mood chips of the home feed, `(title, params)`.
pub const HOME_CHIPS: [(&str, &str); 3] = [
    ("Relax", "sim-chip-relax"),
    ("Workout", "sim-chip-workout"),
    ("Focus", "sim-chip-focus"),
];
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

/// How the simulated service answers a Related tab request (tests drive the states with this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RelatedBehaviour {
    #[default]
    Shelves,
    /// Every request fails.
    Fail(LoadFailure),
    /// The service has no related tab for the track.
    Empty,
}

/// Songs on the simulated chart: more than Explore shows, so "Show all" has somewhere to go.
pub const CHART_SONGS: usize = 24;
/// The chart's own page, where "Show all" leads.
pub const CHART_ID: &str = "VLPLsimchart";
/// Mood tiles for the states: one with no playlists, one that fails to load.
pub const MOOD_EMPTY: &str = "sim-mood:Empty";
pub const MOOD_BROKEN: &str = "sim-mood:Broken";

/// How the simulated service answers Explore (tests drive the states with this).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExploreBehaviour {
    #[default]
    Feed,
    /// Every request fails.
    Fail(LoadFailure),
    /// The page reads cleanly and holds nothing.
    Empty,
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
    /// Video ids the account thumbs-downed ("Not interested").
    disliked: Vec<String>,
    /// `(bare playlist id, video id)` added through `add_to_playlist`, oldest first.
    added: Vec<(String, String)>,
    writes: WriteBehaviour,
    /// Makes `playlists_for` fail (the picker's error state).
    playlists: Option<LoadFailure>,
    /// Writes the simulation received, `(what, video id)`, for tests.
    log: Vec<(String, String)>,
    home: HomeBehaviour,
    /// Home requests the simulation received: `""` for the first batch, else the token, and
    /// `chip:` and the chip's params for a mood chip.
    home_log: Vec<String>,
    /// Makes mood chip requests fail.
    chip_failure: Option<LoadFailure>,
    explore: ExploreBehaviour,
    /// Explore requests received.
    explore_requests: u32,
    /// The mood tokens `mood_playlists` was asked for, in order.
    mood_log: Vec<String>,
    /// Makes the Artists and History tabs fail to load.
    tabs: Option<LoadFailure>,
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
    /// How `related` answers (tests drive the states with this).
    related: RelatedBehaviour,
    /// The videos `related` was asked about, in order.
    related_log: Vec<String>,
    /// Channel ids the account is subscribed to.
    subscribed: Vec<String>,
    /// Mix playlist ids `mix` was asked for, in order.
    mixes: Vec<String>,
    /// Makes `artist` fail once the page has been seen (the read-back after an unknown write).
    artist_fails: Option<LoadFailure>,
    /// Video ids in Library > Songs, most recently added first.
    songs: Vec<String>,
    /// Songs whose row offered no library toggle, so nothing can be done about them from here.
    no_toggle: HashSet<String>,
    /// Songs the model told the source are in the library because Library > Songs listed them.
    listed_saved: HashSet<String>,
    /// What playlists were last given as a description and a visibility, by bare id.
    descriptions: HashMap<String, String>,
    privacies: HashMap<String, Privacy>,
    /// The videos a playlist was created with, by bare id.
    created_with: HashMap<String, Vec<String>>,
}

impl SimState {
    fn new() -> Self {
        Self {
            liked: FIXTURES.iter().take(6).map(|f| f.id.to_owned()).collect(),
            disliked: Vec::new(),
            added: Vec::new(),
            writes: WriteBehaviour::default(),
            playlists: None,
            log: Vec::new(),
            home: HomeBehaviour::default(),
            home_log: Vec::new(),
            chip_failure: None,
            explore: ExploreBehaviour::default(),
            explore_requests: 0,
            mood_log: Vec::new(),
            tabs: None,
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
            related: RelatedBehaviour::default(),
            related_log: Vec::new(),
            subscribed: Vec::new(),
            mixes: Vec::new(),
            artist_fails: None,
            songs: vec![
                FIXTURES[3].id.to_owned(),
                FIXTURES[1].id.to_owned(),
                format!("{GENERATED_PREFIX}{:05}", 0),
                format!("{GENERATED_PREFIX}{:05}", 1),
                format!("{GENERATED_PREFIX}{:05}", 2),
            ],
            no_toggle: HashSet::new(),
            listed_saved: HashSet::new(),
            descriptions: HashMap::new(),
            privacies: HashMap::new(),
            created_with: HashMap::new(),
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

    /// Makes the home feed's mood chips fail (tests).
    pub fn set_chip_failure(&self, failure: Option<LoadFailure>) {
        self.state().chip_failure = failure;
    }

    /// How the Related tab is answered from now on (tests).
    pub fn set_related_behaviour(&self, behaviour: RelatedBehaviour) {
        self.state().related = behaviour;
    }

    /// The videos the Related tab was asked about, in order.
    pub fn related_log(&self) -> Vec<String> {
        self.state().related_log.clone()
    }

    /// How Explore is answered from now on (tests).
    pub fn set_explore_behaviour(&self, behaviour: ExploreBehaviour) {
        self.state().explore = behaviour;
    }

    /// How many times Explore was asked for.
    pub fn explore_requests(&self) -> u32 {
        self.state().explore_requests
    }

    /// The mood tokens asked for, in order.
    pub fn mood_requests(&self) -> Vec<String> {
        self.state().mood_log.clone()
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

    /// Video ids the account thumbs-downed, most recent first.
    pub fn disliked_ids(&self) -> Vec<String> {
        self.state().disliked.clone()
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
    /// The video ids in Library > Songs, most recently added first.
    pub fn library_song_ids(&self) -> Vec<String> {
        self.state().songs.clone()
    }

    /// Replace Library > Songs with exactly these videos.
    pub fn preset_library_songs(&self, ids: &[&str]) {
        self.state().songs = ids.iter().map(|id| (*id).to_owned()).collect();
    }

    /// Fill Library > Songs with [`LONG_SONGS`] generated songs (paged).
    pub fn preset_long_library_songs(&self) {
        self.state().songs = (0..LONG_SONGS)
            .map(|n| format!("{GENERATED_PREFIX}{:05}", n + 10))
            .collect();
    }

    /// The video ids the Songs tab has told the source it holds, sorted.
    pub fn songs_noted_saved(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.state().listed_saved.iter().cloned().collect();
        ids.sort();
        ids
    }

    /// Make this song's row offer no library toggle (the menu then has no save item for it).
    pub fn hide_library_toggle(&self, video_id: &str) {
        self.state().no_toggle.insert(video_id.to_owned());
    }

    /// The description and visibility a playlist was last given.
    pub fn playlist_details(&self, bare_id: &str) -> (Option<String>, Option<Privacy>) {
        let state = self.state();
        (
            state.descriptions.get(bare_id).cloned(),
            state.privacies.get(bare_id).copied(),
        )
    }

    /// The videos a playlist was created holding.
    pub fn created_with(&self, bare_id: &str) -> Vec<String> {
        self.state().created_with.get(bare_id).cloned().unwrap_or_default()
    }

    fn song_rows(&self) -> Vec<TrackRow> {
        self.state()
            .songs
            .iter()
            .enumerate()
            .map(|(n, id)| song_row(id, n))
            .collect()
    }

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

/// A Library > Songs row for a video id: the fixture or generated track when it is one, else a
/// plain row named for the id. Library rows carry no occurrence token.
fn song_row(id: &str, n: usize) -> TrackRow {
    let mut row = match FIXTURES.iter().find(|f| f.id == id) {
        Some(f) => fixture_row(f, n),
        None => match id.strip_prefix(GENERATED_PREFIX).and_then(|n| n.parse::<usize>().ok()) {
            Some(g) => generated_row(g),
            None => TrackRow {
                track: Track::new(id, format!("Song {id}")),
                occurrence: None,
                thumbnail: None,
                explicit: false,
                liked: None,
            },
        },
    };
    row.occurrence = None;
    row.liked = None;
    row
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
    HomeItem::Collection(home_playlist_collection(c))
}

fn home_playlist_collection(c: &Collection) -> Collection {
    let hue = c
        .thumbnail
        .as_deref()
        .and_then(|t| t.strip_prefix("sim://art/"))
        .and_then(|h| h.parse::<f32>().ok());
    Collection {
        thumbnail: hue.map(|h| format!("sim://hue/{h}")),
        ..c.clone()
    }
}

fn album_collection(f: &Fixture) -> Collection {
    Collection {
        kind: CollectionKind::Album,
        id: BrowseId::new(format!("{SEARCH_ALBUM_PREFIX}{}", f.album)),
        title: f.album.into(),
        subtitle: Some(format!("{} • 2024", f.artist)),
        thumbnail: home_art(f.art),
    }
}

fn listed(row: TrackRow, art: Option<SyntheticArt>) -> ListedTrack {
    ListedTrack {
        track: row.track,
        occurrence: None,
        thumbnail: home_art(art),
        explicit: false,
        liked: None,
    }
}

/// The simulated Explore page: new releases, a chart longer than the page shows, one card shelf
/// and the mood tiles (two of them the failure states).
fn explore_content() -> ExplorePage {
    let top_songs: Vec<ListedTrack> = FIXTURES
        .iter()
        .enumerate()
        .map(|(n, f)| listed(fixture_row(f, n), f.art))
        .chain(
            (0..CHART_SONGS.saturating_sub(FIXTURES.len())).map(|n| {
                listed(
                    generated_row(n),
                    Some(SyntheticArt::Hue((n % 12) as f32 * 30.0)),
                )
            }),
        )
        .take(CHART_SONGS)
        .collect();
    let trending: Vec<HomeItem> = FIXTURES.iter().rev().map(home_album).collect();
    let tile = |title: &str| MoodTile {
        title: title.into(),
        params: format!("sim-mood:{title}"),
    };
    ExplorePage {
        new_releases: FIXTURES.iter().map(album_collection).collect(),
        top_songs,
        top_songs_all: Some(Collection {
            kind: CollectionKind::Playlist,
            id: BrowseId::new(CHART_ID),
            title: "Trending".into(),
            subtitle: None,
            thumbnail: None,
        }),
        shelves: vec![HomeShelf {
            title: "Trending".into(),
            items: trending,
        }],
        moods: [
            "Chill", "Workout", "Focus", "Party", "Sleep", "Commute", "Romance", "Feel good",
            "Energy boosters", "Rock", "Pop", "Hip-hop", "Dance & electronic", "Jazz", "Classical",
            "Country", "Empty", "Broken",
        ]
        .into_iter()
        .map(tile)
        .collect(),
        skipped: 0,
    }
}

/// The simulated Related tab: a song shelf, then albums and artists. The fixtures are rotated by
/// the video id, so two tracks do not read the same.
fn related_shelves(video_id: &str) -> Vec<HomeShelf> {
    let spin = video_id.bytes().map(usize::from).sum::<usize>() % FIXTURES.len();
    let order: Vec<(usize, &Fixture)> = FIXTURES
        .iter()
        .enumerate()
        .cycle()
        .skip(spin)
        .take(FIXTURES.len())
        .collect();
    let shelf = |title: &str, items: Vec<HomeItem>| HomeShelf {
        title: title.into(),
        items,
    };
    vec![
        shelf(
            "You might also like",
            order
                .iter()
                .map(|(n, f)| home_song(fixture_row(f, *n), f.art))
                .collect(),
        ),
        shelf(
            "Albums",
            order.iter().take(6).map(|(_, f)| home_album(f)).collect(),
        ),
        shelf(
            "Similar artists",
            order.iter().take(6).map(|(_, f)| home_artist(f)).collect(),
        ),
    ]
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
            chips: HOME_CHIPS
                .iter()
                .map(|(title, params)| HomeChip {
                    title: (*title).into(),
                    params: (*params).into(),
                })
                .collect(),
            shelves: vec![
                shelf("Quick picks", quick),
                shelf("Albums for you", albums),
                shelf("Listen again", mixed),
            ],
            continuation: Some(HOME_SECOND.into()),
            skipped: 0,
        },
        Some(HOME_SECOND) => HomePage {
            chips: Vec::new(),
            shelves: vec![
                shelf("Artists you might like", artists),
                shelf("From your library", playlists),
            ],
            continuation: Some(HOME_THIRD.into()),
            skipped: 0,
        },
        Some(HOME_THIRD) => HomePage {
            chips: Vec::new(),
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
        if id == SONGS_ID {
            return Some(self.song_rows());
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
            CollectionKind::Playlist => {
                let owned = owned_ids(&state).contains(&key);
                PageMeta {
                    owned,
                    description: state
                        .descriptions
                        .get(&key)
                        .cloned()
                        .or_else(|| (!owned).then(|| foreign_description(&key)).flatten()),
                    privacy: owned
                        .then(|| state.privacies.get(&key).copied().unwrap_or_default()),
                    save_id: Some(key),
                    in_library: None,
                }
            }
            CollectionKind::Album => PageMeta {
                owned: false,
                save_id: Some(album_save_id(&key)),
                in_library: Some(library_now(&state).iter().any(|c| c.library_key() == key)),
                description: None,
                privacy: None,
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

    fn library_songs(&self) -> Result<Page<TrackRow>, LoadFailure> {
        self.wait(LATENCY);
        if let Some(f) = self.state().tabs {
            return Err(f);
        }
        Ok(page_of(SONGS_ID, self.song_rows(), 0))
    }

    fn song_saved(&self, video_id: &str) -> Option<bool> {
        let state = self.state();
        (!state.no_toggle.contains(video_id)).then(|| state.songs.iter().any(|id| id == video_id))
    }

    fn note_song_saved(&self, video_id: &str, saved: bool) {
        let mut state = self.state();
        if saved {
            state.listed_saved.insert(video_id.to_owned());
        } else {
            state.listed_saved.remove(video_id);
        }
    }

    fn set_song_saved(&self, video_id: &str, saved: bool) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        if self.state().no_toggle.contains(video_id) {
            return Err(WriteFailure::Rejected);
        }
        let lands = self.write_answer(
            if saved { "song-save" } else { "song-unsave" }.to_owned(),
            video_id,
        );
        let applies = lands.is_ok() || self.state().writes == WriteBehaviour::LandsReplyLost;
        if applies {
            let mut state = self.state();
            state.songs.retain(|id| id != video_id);
            if saved {
                state.songs.insert(0, video_id.to_owned());
            }
        }
        lands.map(|_| ())
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

    fn subscribed_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        self.wait(LATENCY);
        let (failure, channels) = {
            let state = self.state();
            (state.tabs, state.subscribed.clone())
        };
        if let Some(f) = failure {
            return Err(f);
        }
        Ok(channels
            .iter()
            .filter_map(|channel| artist_content(channel))
            .map(|artist| Hit {
                id: BrowseId::new(artist.channel_id),
                title: artist.name,
                subtitle: artist.subscribers,
                thumbnail: artist.thumbnail,
                explicit: false,
            })
            .collect())
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

    fn home_chip(&self, params: &str) -> Result<HomePage, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let failure = {
            let mut state = self.state();
            state.home_log.push(format!("chip:{params}"));
            state.chip_failure
        };
        if let Some(f) = failure {
            return Err(f);
        }
        let title = HOME_CHIPS
            .iter()
            .find(|(_, p)| *p == params)
            .map(|(t, _)| *t)
            .ok_or(LoadFailure::Other)?;
        // A mood's feed: its own shelves, built from the fixtures in a different order per mood.
        let spin = params.bytes().map(usize::from).sum::<usize>() % FIXTURES.len();
        let rotated = || FIXTURES.iter().enumerate().cycle().skip(spin).take(FIXTURES.len());
        Ok(HomePage {
            // The service sends the chip row with a filtered feed too; the app keeps its own.
            chips: HOME_CHIPS
                .iter()
                .map(|(title, params)| HomeChip {
                    title: (*title).into(),
                    params: (*params).into(),
                })
                .collect(),
            shelves: vec![
                HomeShelf {
                    title: format!("{title} picks"),
                    items: rotated()
                        .map(|(n, f)| home_song(fixture_row(f, n), f.art))
                        .collect(),
                },
                HomeShelf {
                    title: format!("{title} albums"),
                    items: rotated().take(6).map(|(_, f)| home_album(f)).collect(),
                },
            ],
            continuation: None,
            skipped: 0,
        })
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

    fn related(&self, video_id: &str) -> Result<Vec<HomeShelf>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        let behaviour = {
            let mut state = self.state();
            state.related_log.push(video_id.to_owned());
            state.related
        };
        match behaviour {
            RelatedBehaviour::Fail(f) => Err(f),
            RelatedBehaviour::Empty => Ok(Vec::new()),
            RelatedBehaviour::Shelves => Ok(related_shelves(video_id)),
        }
    }

    fn explore(&self) -> Result<ExplorePage, LoadFailure> {
        self.wait(LATENCY);
        let behaviour = {
            let mut state = self.state();
            state.explore_requests += 1;
            state.explore
        };
        match behaviour {
            ExploreBehaviour::Fail(f) => Err(f),
            ExploreBehaviour::Empty => Ok(ExplorePage::default()),
            ExploreBehaviour::Feed => Ok(explore_content()),
        }
    }

    fn mood_playlists(&self, params: &str) -> Result<Vec<Collection>, LoadFailure> {
        self.wait(LATENCY);
        self.state().mood_log.push(params.to_owned());
        match params {
            MOOD_BROKEN => Err(LoadFailure::Network),
            MOOD_EMPTY => Ok(Vec::new()),
            _ => Ok(all_collections()
                .iter()
                .filter(|c| c.kind == CollectionKind::Playlist && c.thumbnail.is_some())
                .filter(|c| c.id.as_str() != BROKEN_ID)
                .take(10)
                .map(home_playlist_collection)
                .collect()),
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

    fn set_disliked(&self, video_id: &str) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let lands = self.write_answer("dislike".to_owned(), video_id);
        let applies = lands.is_ok() || self.state().writes == WriteBehaviour::LandsReplyLost;
        if applies {
            // Like and dislike exclude each other, as on the service.
            let mut state = self.state();
            state.liked.retain(|id| id != video_id);
            if !state.disliked.iter().any(|id| id == video_id) {
                state.disliked.insert(0, video_id.to_owned());
            }
        }
        lands.map(|_| ())
    }

    fn disliked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        self.wait(PAGE_LATENCY);
        Ok(Some(self.state().disliked.iter().any(|id| id == video_id)))
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

    fn create_playlist_with(
        &self,
        title: &str,
        description: &str,
        privacy: Privacy,
        video_ids: &[String],
    ) -> Result<String, WriteFailure> {
        self.wait(PAGE_LATENCY);
        let (applies, reply) = self.begin_write(format!("create-with:{}", video_ids.len()), title);
        let id = {
            let mut state = self.state();
            let id = format!("PLsimnew{:03}", state.next_playlist);
            if applies {
                state.next_playlist += 1;
                state.created.insert(0, (id.clone(), title.to_owned()));
                let rows: Vec<TrackRow> = video_ids
                    .iter()
                    .enumerate()
                    .map(|(n, video)| {
                        let mut row = liked_row(video, n);
                        row.occurrence = Some(format!("occ-created-{id}-{n}"));
                        row.liked = None;
                        row
                    })
                    .collect();
                state.edited_rows.insert(id.clone(), rows);
                state.created_with.insert(id.clone(), video_ids.to_vec());
                state.descriptions.insert(id.clone(), description.to_owned());
                state.privacies.insert(id.clone(), privacy);
            }
            id
        };
        reply.map(|()| id)
    }

    fn edit_playlist(&self, playlist_id: &str, edit: &PlaylistEdit) -> Result<(), WriteFailure> {
        self.wait(PAGE_LATENCY);
        let mut parts = Vec::new();
        if let Some(title) = &edit.title {
            parts.push(format!("title={title}"));
        }
        if let Some(description) = &edit.description {
            parts.push(format!("description={description}"));
        }
        if let Some(privacy) = edit.privacy {
            parts.push(format!("privacy={}", privacy.service_word()));
        }
        let (applies, reply) = self.begin_write(format!("edit:{}", parts.join(";")), playlist_id);
        if applies {
            let mut state = self.state();
            if let Some(title) = &edit.title {
                state.titles.insert(playlist_id.to_owned(), title.clone());
            }
            if let Some(description) = &edit.description {
                state.descriptions.insert(playlist_id.to_owned(), description.clone());
            }
            if let Some(privacy) = edit.privacy {
                state.privacies.insert(playlist_id.to_owned(), privacy);
            }
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

/// The blurb a playlist of somebody else's carries; only one of them has any.
pub const FOREIGN_DESCRIBED_ID: &str = "VLPLsimmix020";

fn foreign_description(key: &str) -> Option<String> {
    (key == FOREIGN_DESCRIBED_ID.trim_start_matches("VL")).then(|| {
        "Slow mornings, long drives and a few songs that never get old. Built over about three \
         years by someone who could not stop adding to it, and updated whenever something fits."
            .to_owned()
    })
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
    let size = if id == LONG_ID || id == SONGS_ID { PAGE_SIZE } else { usize::MAX };
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
