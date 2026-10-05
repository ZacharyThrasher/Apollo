//! Library screen state: the collection lists, the stack of open pages (collections and
//! artists) and their paged track lists, the artwork cache, and the playback commands. No
//! rendering; `ui::library` draws it.
//!
//! All service calls go through [`LibrarySource`] on GPUI's background executor. Every
//! completion carries the id of the page it was started for, so an answer for a page that has
//! since been closed is dropped instead of landing in the wrong list.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use apollo_domain::library::{
    is_user_playlist_id, moved, plan_move, Collection, CollectionKind, PageMeta,
    LIKED_MUSIC_BROWSE_ID,
};
use apollo_domain::listing::{Applied, Listing, LoadFailure, LoadState, Page, Revision};
use apollo_domain::model::{BrowseId, Track};
use apollo_domain::queue::QueueSource;
use gpui::{App, Context, Entity, RenderImage};
use image::imageops;

use apollo_provider::{HistoryDay, Hit};

use super::browse::{
    history_groups, history_len, history_row, sort_artists, sort_collections, sort_songs,
    HistoryGroup, SortKey,
};
use super::edits::{self, Edit, EditEvent, LibraryEdits};
use super::filter::Visible;
use super::source::{ArtistContent, LibrarySource, TrackRow};
use crate::search::local::LocalIndex;
use super::writes::{LibraryWrites, WritesEvent};

mod artist;
mod stats;
pub use artist::MixKind;
use crate::art::{sized_thumbnail, DecodedArt};
use crate::page_cache::{keys, PageCache, Ticket};
use crate::player::{PlayerModel, Status};
use crate::queue_source;
use crate::settings::LibrarySortStore;
use crate::stats::report::{Period, StatsReport};
use crate::url_thumbs::Thumb as ThumbView;

/// Cover downloads running at once. Fast scrolling queues many; the newest request goes first.
const MAX_INFLIGHT_ART: usize = 4;
/// Decoded covers kept. Rows are small, so this is a few dozen MB at most.
const ART_CACHE_CAP: usize = 400;
/// Pixel size covers are decoded to (2x the 48px row art for high-DPI displays).
const LIST_ART_PX: u32 = 96;
/// Detail header art, drawn at 96px (2x for high-DPI displays and a little headroom).
const HEADER_ART_PX: u32 = 320;
/// Album and playlist cards on an artist page, drawn at 144px.
const CARD_ART_PX: u32 = 288;
/// An artist banner is kept at its own aspect and shrunk to this width at most.
const BANNER_ART_PX: u32 = 1600;

/// What the Songs tab's stand-in collection is called; not a browse id the service knows.
/// One page of Library > Songs, first (`None`) or by continuation token. Every row on it is in
/// the library, which the source is told so its menus offer "Remove from library" there.
fn read_songs(
    source: &dyn LibrarySource,
    token: Option<&str>,
) -> Result<Page<TrackRow>, LoadFailure> {
    let page = match token {
        Some(token) => source.more_tracks(token),
        None => source.library_songs(),
    }?;
    for row in &page.items {
        source.note_song_saved(row.track.video_id.as_str(), true);
    }
    Ok(page)
}

const SONGS_BROWSE_ID: &str = "apollo:library-songs";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Playlists,
    Albums,
    Artists,
    History,
    /// The songs saved to the library (not the likes: those are Liked Music).
    Songs,
    /// What Apollo itself played: local stats (see `crate::stats`).
    Stats,
}

impl Tab {
    pub const ALL: [Tab; 6] = [
        Tab::Playlists,
        Tab::Albums,
        Tab::Artists,
        Tab::History,
        Tab::Songs,
        Tab::Stats,
    ];
}

#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Idle,
    Loading,
    Ready(T),
    Failed(LoadFailure),
}

/// The library's two lists. Liked Music is always the first playlist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Collections {
    pub playlists: Vec<Collection>,
    pub albums: Vec<Collection>,
}

/// A play command that waits for the track list to finish loading, so the queue it builds is
/// the whole collection and never a silently truncated first page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayRequest {
    /// First track, as an index into the loaded rows. `None`: the first row, or a random one
    /// when shuffling.
    pub start: Option<usize>,
    /// `Some` sets shuffle on or off for the new queue; `None` keeps the player's setting.
    pub shuffle: Option<bool>,
    /// Skim the list instead of playing it (audition each song for a few seconds).
    pub skim: bool,
}

impl PlayRequest {
    pub const PLAY_ALL: PlayRequest = PlayRequest {
        start: None,
        shuffle: Some(false),
        skim: false,
    };
    pub const SHUFFLE: PlayRequest = PlayRequest {
        start: None,
        shuffle: Some(true),
        skim: false,
    };
    /// Skim from the first song: the list in order.
    pub const SKIM: PlayRequest = PlayRequest {
        start: None,
        shuffle: Some(false),
        skim: true,
    };

    pub fn row(index: usize) -> Self {
        Self {
            start: Some(index),
            shuffle: None,
            skim: false,
        }
    }
}

/// Something a card can open: an album or playlist (a collection) or an artist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Collection(Collection),
    Artist(ArtistTarget),
}

/// What is known about an artist before its page arrives (from the card that was tapped), so the
/// header does not shift when the page lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtistTarget {
    pub id: BrowseId,
    pub name: String,
    pub thumbnail: Option<String>,
}

pub struct ArtistDetail {
    id: u64,
    pub target: ArtistTarget,
    pub state: Load<Arc<ArtistContent>>,
    pub pending: Option<PlayRequest>,
    /// The subscription write (or its read-back) in flight, with the state it asks for: what the
    /// button shows until the service has answered.
    pub subscribing: Option<bool>,
    /// Radio or Shuffle is fetching its tracks.
    pub mix: Option<MixKind>,
    /// The bio is open in full (it is clamped to a few lines otherwise).
    pub bio_open: bool,
}

impl ArtistDetail {
    /// The header's name: the page's own once it has arrived.
    pub fn name(&self) -> &str {
        match &self.state {
            Load::Ready(c) => &c.name,
            _ => &self.target.name,
        }
    }

    /// The header's picture: the card's (usually square), else the page's.
    pub fn thumbnail(&self) -> Option<&str> {
        self.target.thumbnail.as_deref().or(match &self.state {
            Load::Ready(c) => c.thumbnail.as_deref(),
            _ => None,
        })
    }

    pub fn content(&self) -> Option<&Arc<ArtistContent>> {
        match &self.state {
            Load::Ready(c) => Some(c),
            _ => None,
        }
    }

    /// What the Subscribe button shows: the request in flight, else what the service holds.
    pub fn subscribed(&self) -> bool {
        self.subscribing
            .unwrap_or_else(|| self.content().is_some_and(|c| c.subscribed))
    }
}

enum Entry {
    Collection(Detail),
    Artist(ArtistDetail),
}

/// The edits of one playlist's rows that are shown but not yet settled, oldest first, each with
/// the account it was asked in. They go out one at a time (each one's order depends on the ones
/// before it) and outlive the page they were made on: leaving the page does not drop them.
struct EditQueue {
    collection: Collection,
    items: VecDeque<(PageEdit, Option<String>)>,
    /// One is out.
    busy: bool,
}

/// An edit of the rows of a playlist page, already shown and waiting to be (or being) written.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PageEdit {
    Remove {
        video_id: String,
        occurrence: String,
    },
    Move {
        occurrence: String,
        /// The row it goes in front of; `None` is the end.
        before: Option<String>,
        /// Every row's token in the order the playlist should have afterwards.
        desired: Vec<String>,
    },
}

pub struct Detail {
    id: u64,
    pub collection: Collection,
    pub listing: Listing<TrackRow>,
    pub pending: Option<PlayRequest>,
    /// What the page says about editing and saving it (known once its first page arrived).
    pub meta: PageMeta,
    /// Bumped by every silent refresh; an older refresh's answer is dropped.
    refresh: u64,
}

impl Detail {
    /// The rows can be removed and reordered: the account owns the playlist (the page said so and
    /// its id is a user playlist's) and every row is here.
    pub fn can_edit(&self) -> bool {
        self.meta.owned
            && is_user_playlist_id(self.collection.library_key())
            && !self.is_loading()
            && self.failure().is_none()
    }

    /// More pages are on the way (or the first one is).
    pub fn is_loading(&self) -> bool {
        matches!(
            self.listing.state(),
            LoadState::Idle | LoadState::Loading | LoadState::HasMore
        )
    }

    pub fn failure(&self) -> Option<LoadFailure> {
        match self.listing.state() {
            LoadState::Failed(f) => Some(f),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtSize {
    List,
    Header,
    Card,
    /// A wide picture kept whole (an artist's banner), not cropped to a square.
    Banner,
}

enum Thumb {
    Loading,
    Ready(Arc<RenderImage>),
    Missing,
}

#[derive(Default)]
struct Thumbs {
    map: HashMap<String, Thumb>,
    /// Keys of finished covers, oldest first, for eviction.
    order: VecDeque<String>,
    /// Requested but not started; the newest is started first.
    queue: Vec<(String, ArtSize)>,
    inflight: usize,
}

/// What a page of rows says about likes: each row's own `likeStatus`, or liked for every row of
/// Liked Music (being listed there is what liked means).
fn like_seeds(rows: &[TrackRow], liked_music: bool) -> Vec<(String, bool)> {
    rows.iter()
        .filter_map(|r| {
            let liked = if liked_music { Some(true) } else { r.liked }?;
            Some((r.track.video_id.as_str().to_owned(), liked))
        })
        .collect()
}

/// How an artist page's answer relates to what is on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Answer {
    /// From the service, with nothing shown yet (or a retry).
    Fresh,
    /// From the page cache: drawn at once; only teaches like state nothing is known about.
    Cached,
    /// From the service, following cached content already shown: swapped in only when it
    /// differs, and a failure other than a dead session leaves what is shown alone.
    Behind,
}

/// Where an answer being applied came from.
#[derive(Clone, Copy)]
enum Origin<'a> {
    /// The service, with the ticket to cache it under (none without a cache).
    Live(Option<&'a Ticket>),
    /// The page cache: drawn at once, fetched again behind.
    Cached,
}

fn key(url: &str, size: ArtSize) -> String {
    format!("{size:?}|{url}")
}

/// The lists as the screen shows them: the loaded ones in the chosen order.
#[derive(Default)]
struct Shown {
    playlists: Vec<Collection>,
    albums: Vec<Collection>,
    artists: Vec<Hit>,
}

pub struct LibraryModel {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    edits: Entity<LibraryEdits>,
    /// Pending row edits by the playlist's library key.
    edit_queues: HashMap<String, EditQueue>,
    source: Option<Arc<dyn LibrarySource>>,
    tab: Tab,
    collections: Load<Collections>,
    collections_rev: u64,
    /// Bumped when the account changes: answers to row edits sent before are not for this library.
    edit_epoch: u64,
    /// The Artists tab: the artists behind the library, in the service's order.
    artists: Load<Vec<Hit>>,
    artists_rev: u64,
    /// The History tab: the play history in the service's date buckets.
    history: Load<Vec<HistoryGroup>>,
    history_rev: u64,
    /// The Songs tab: the songs saved to the library, as a page of its own (it has no collection
    /// behind it). `songs_shown` is the loaded list in the chosen order, which is what is drawn,
    /// selected and played. `songs_stale`: a song was saved since it was read, so the next time
    /// the tab opens it is read again.
    songs: Detail,
    songs_shown: Vec<TrackRow>,
    songs_rev: u64,
    songs_stale: bool,
    /// What the Songs tab's filter box says (lowercased, trimmed; empty for none): Play and
    /// Shuffle on the tab queue only the rows it keeps.
    songs_query: String,
    /// The Stats tab: the report for the chosen period, worked out from the local play log.
    stats: Load<Arc<StatsReport>>,
    stats_period: Period,
    stats_rev: u64,
    sorts: HashMap<Tab, SortKey>,
    sort_store: Option<Arc<dyn LibrarySortStore>>,
    shown: Shown,
    /// Every liked song, loaded the first time a search needs them (the Liked Music page loads
    /// its own copy). `liked_stale`: a like since then is not in it, so the next search that
    /// starts reads it again.
    liked_songs: Load<Arc<Vec<TrackRow>>>,
    liked_rev: u64,
    liked_stale: bool,
    /// A search wanted them before the source was there: load them as soon as it is.
    search_wanted: bool,
    /// What Search looks through, rebuilt whenever one of the lists above changes; `index_rev`
    /// tells a watcher that it did.
    index: Arc<LocalIndex>,
    index_rev: u64,
    /// Open pages, the one on screen last. Album and playlist pages opened from an artist page
    /// stack on it, and back unwinds one at a time.
    stack: Vec<Entry>,
    next_detail: u64,
    thumbs: Thumbs,
}

impl LibraryModel {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        edits: Entity<LibraryEdits>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |m, _, cx| m.try_start(cx)).detach();
        // The lists follow the playlists made, renamed, deleted and saved anywhere in the app.
        cx.subscribe(&edits, |m, _, event, cx| m.edit_happened(event, cx))
            .detach();
        // Liked Music follows the likes made anywhere in the app.
        cx.subscribe(&writes, |m, _, event, cx| match event {
            WritesEvent::LikeSettled { video_id, liked } => m.liked_changed(video_id, *liked, cx),
        })
        .detach();
        let mut model = Self {
            player,
            writes,
            edits,
            edit_queues: HashMap::new(),
            source: None,
            tab: Tab::Playlists,
            collections: Load::Idle,
            collections_rev: 0,
            edit_epoch: 0,
            artists: Load::Idle,
            artists_rev: 0,
            history: Load::Idle,
            history_rev: 0,
            songs: songs_page(),
            songs_shown: Vec::new(),
            songs_rev: 0,
            songs_stale: false,
            songs_query: String::new(),
            stats: Load::Idle,
            stats_period: Period::default(),
            stats_rev: 0,
            sorts: HashMap::new(),
            sort_store: None,
            shown: Shown::default(),
            liked_songs: Load::Idle,
            liked_rev: 0,
            liked_stale: false,
            search_wanted: false,
            index: Arc::new(LocalIndex::default()),
            index_rev: 0,
            stack: Vec::new(),
            next_detail: 1,
            thumbs: Thumbs::default(),
        };
        model.try_start(cx);
        model
    }

    // ---- reading -----------------------------------------------------------------------

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn collections(&self) -> &Load<Collections> {
        &self.collections
    }

    /// The tab's playlists or albums in the chosen order (nothing for the other tabs).
    pub fn list(&self, tab: Tab) -> &[Collection] {
        match (&self.collections, tab) {
            (Load::Ready(_), Tab::Playlists) => &self.shown.playlists,
            (Load::Ready(_), Tab::Albums) => &self.shown.albums,
            _ => &[],
        }
    }

    pub fn artists(&self) -> &Load<Vec<Hit>> {
        &self.artists
    }

    /// The Artists tab's cards in the chosen order.
    pub fn artist_list(&self) -> &[Hit] {
        match self.artists {
            Load::Ready(_) => &self.shown.artists,
            _ => &[],
        }
    }

    pub fn history(&self) -> &Load<Vec<HistoryGroup>> {
        &self.history
    }

    /// The history's rows across its buckets.
    pub fn history_count(&self) -> usize {
        match &self.history {
            Load::Ready(groups) => history_len(groups),
            _ => 0,
        }
    }

    /// The `n`th history row counting across buckets.
    pub fn history_at(&self, n: usize) -> Option<TrackRow> {
        match &self.history {
            Load::Ready(groups) => history_row(groups, n).map(|(_, row)| row.clone()),
            _ => None,
        }
    }

    /// The Songs tab's page: its load state and the rows as the service listed them.
    pub fn songs(&self) -> &Detail {
        &self.songs
    }

    /// The rows of the track list on screen: an open page's, else the Songs tab's in its chosen
    /// order. Nothing for the other tabs.
    pub fn list_rows(&self) -> &[TrackRow] {
        match self.stack.last() {
            Some(Entry::Collection(d)) => d.listing.items(),
            Some(Entry::Artist(_)) => &[],
            None if self.tab == Tab::Songs => &self.songs_shown,
            None => &[],
        }
    }

    /// Whether the Songs tab is what the overview shows.
    pub fn songs_open(&self) -> bool {
        self.stack.is_empty() && self.tab == Tab::Songs
    }

    /// How the tab is ordered: the saved choice, else the default.
    pub fn sort(&self, tab: Tab) -> SortKey {
        self.sorts.get(&tab).copied().unwrap_or_default()
    }

    /// The collection page on screen, if that is what is on top.
    pub fn detail(&self) -> Option<&Detail> {
        match self.stack.last() {
            Some(Entry::Collection(d)) => Some(d),
            _ => None,
        }
    }

    /// The artist page on screen, if that is what is on top.
    pub fn artist(&self) -> Option<&ArtistDetail> {
        match self.stack.last() {
            Some(Entry::Artist(a)) => Some(a),
            _ => None,
        }
    }

    /// Pages open over the overview (0 when the lists are showing).
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    // ---- collections -------------------------------------------------------------------

    /// Picks up the backend's library source once the player has one, and loads the lists. Called
    /// on every player change; does nothing after the first success.
    fn try_start(&mut self, cx: &mut Context<Self>) {
        if self.source.is_some() {
            return;
        }
        let (source, unavailable) = {
            let p = self.player.read(cx);
            (
                p.library_source(),
                matches!(p.status(), Status::Unavailable(_)),
            )
        };
        match source {
            Some(source) => {
                self.source = Some(source);
                self.load_collections(cx);
                self.ensure_tab(self.tab, cx);
                if self.search_wanted {
                    self.ensure_searchable(cx);
                }
            }
            None if unavailable && self.collections == Load::Idle => {
                self.collections = Load::Failed(LoadFailure::Other);
                cx.notify();
            }
            None => {}
        }
    }

    /// Forgets everything loaded (the account changed) and loads the lists again. Answers still
    /// in flight for the old account are dropped.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.stack.clear();
        // Row edits not yet sent belong to the old account and are dropped, not sent to the new.
        self.edit_queues.clear();
        self.edit_epoch += 1;
        // Covers still downloading finish into the new cache harmlessly; keep their count.
        self.thumbs = Thumbs {
            inflight: self.thumbs.inflight,
            ..Thumbs::default()
        };
        self.tab = Tab::Playlists;
        self.collections_rev += 1;
        self.collections = Load::Idle;
        self.artists_rev += 1;
        self.artists = Load::Idle;
        self.history_rev += 1;
        self.history = Load::Idle;
        self.songs_rev += 1;
        self.songs = songs_page();
        self.songs_shown.clear();
        self.songs_stale = false;
        self.stats_rev += 1;
        self.stats = Load::Idle;
        self.stats_period = Period::default();
        self.liked_rev += 1;
        self.liked_songs = Load::Idle;
        self.liked_stale = false;
        self.shown = Shown::default();
        self.rebuild_index();
        if self.source.is_some() {
            self.load_collections(cx);
        } else {
            self.try_start(cx);
        }
        cx.notify();
    }

    pub fn reload_collections(&mut self, cx: &mut Context<Self>) {
        self.load_collections(cx);
    }

    fn load_collections(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        // The last answer is drawn at once and the list is fetched again behind it.
        let cache = PageCache::try_global(cx);
        let cached = cache
            .as_ref()
            .and_then(|c| c.get::<Vec<Collection>>(keys::COLLECTIONS));
        let ticket = cache.as_ref().map(|c| c.ticket());
        let quiet = cached.is_some();
        self.collections = match cached {
            Some(cached) => Load::Ready(split_collections(cached.value)),
            None => Load::Loading,
        };
        self.reshow();
        // What the cache showed is what the menus know about the library until the answer lands.
        if let Load::Ready(lists) = &self.collections {
            self.edits.update(cx, |e, cx| e.set_library(lists, cx));
        }
        self.collections_rev += 1;
        let rev = self.collections_rev;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.collections() })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.collections_rev != rev {
                    return;
                }
                match result {
                    Ok(list) => {
                        if let (Some(cache), Some(ticket)) = (&cache, &ticket) {
                            cache.put(ticket, keys::COLLECTIONS.to_owned(), list.clone());
                        }
                        let fresh = Load::Ready(split_collections(list));
                        // Same lists: nothing is redrawn, so nothing flickers or jumps.
                        if m.collections == fresh {
                            return;
                        }
                        m.collections = fresh;
                        if let Load::Ready(lists) = &m.collections {
                            m.edits.update(cx, |e, cx| e.set_library(lists, cx));
                        }
                    }
                    // A dead session is always said out loud; any other failure leaves the
                    // lists that are already on screen alone.
                    Err(LoadFailure::Session) => m.collections = Load::Failed(LoadFailure::Session),
                    Err(_) if quiet => return,
                    Err(f) => m.collections = Load::Failed(f),
                }
                m.reshow();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            self.ensure_tab(tab, cx);
            cx.notify();
        }
    }

    // ---- artists, history, sort -----------------------------------------------------------

    /// Starts what the tab shows when it is opened: the artists once, the history every time
    /// (it keeps what it has on screen while the fresh one loads).
    fn ensure_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        match tab {
            Tab::Artists if matches!(self.artists, Load::Idle) => self.load_artists(cx),
            Tab::History => self.load_history(cx),
            Tab::Songs if matches!(self.songs.listing.state(), LoadState::Idle) || self.songs_stale => {
                self.load_songs(cx)
            }
            Tab::Stats => self.load_stats(cx),
            _ => {}
        }
    }

    pub fn reload_artists(&mut self, cx: &mut Context<Self>) {
        self.load_artists(cx);
    }

    fn load_artists(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        // The last answer is drawn at once and the list is fetched again behind it.
        let cache = PageCache::try_global(cx);
        let cached = cache
            .as_ref()
            .and_then(|c| c.get::<Vec<Hit>>(keys::ARTISTS));
        let ticket = cache.as_ref().map(|c| c.ticket());
        let quiet = cached.is_some();
        self.artists = match cached {
            Some(cached) => Load::Ready(cached.value),
            None => Load::Loading,
        };
        self.reshow();
        self.artists_rev += 1;
        let rev = self.artists_rev;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.library_artists() })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.artists_rev != rev {
                    return;
                }
                match result {
                    Ok(list) => {
                        if let (Some(cache), Some(ticket)) = (&cache, &ticket) {
                            cache.put(ticket, keys::ARTISTS.to_owned(), list.clone());
                        }
                        let fresh = Load::Ready(list);
                        if m.artists == fresh {
                            return;
                        }
                        m.artists = fresh;
                    }
                    Err(LoadFailure::Session) => m.artists = Load::Failed(LoadFailure::Session),
                    Err(_) if quiet => return,
                    Err(f) => m.artists = Load::Failed(f),
                }
                m.reshow();
                cx.notify();
            });
        })
        .detach();
    }

    pub fn reload_history(&mut self, cx: &mut Context<Self>) {
        self.load_history(cx);
    }

    /// Loads the history. Rows already shown stay until the new ones arrive (and stay if they
    /// never do).
    fn load_history(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        // Rows on screen stay; a launch with nothing on screen draws the last answer first.
        let cache = PageCache::try_global(cx);
        let ticket = cache.as_ref().map(|c| c.ticket());
        let mut refresh = matches!(self.history, Load::Ready(_));
        if !refresh {
            let cached = cache
                .as_ref()
                .and_then(|c| c.get::<Vec<HistoryDay>>(keys::HISTORY));
            match cached {
                Some(cached) => {
                    let groups = history_groups(cached.value);
                    let writes = self.writes.read(cx);
                    let seeds: Vec<(String, bool)> = groups
                        .iter()
                        .flat_map(|g| like_seeds(&g.rows, false))
                        .filter(|(video, _)| writes.liked(video).is_none())
                        .collect();
                    self.writes.update(cx, |w, cx| w.seed(seeds, cx));
                    self.history = Load::Ready(groups);
                    refresh = true;
                }
                None => self.history = Load::Loading,
            }
        }
        self.history_rev += 1;
        let rev = self.history_rev;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.history() })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.history_rev != rev {
                    return;
                }
                match result {
                    Ok(days) => {
                        if let (Some(cache), Some(ticket)) = (&cache, &ticket) {
                            cache.put(ticket, keys::HISTORY.to_owned(), days.clone());
                        }
                        let groups = history_groups(days);
                        let seeds: Vec<(String, bool)> = groups
                            .iter()
                            .flat_map(|g| like_seeds(&g.rows, false))
                            .collect();
                        m.writes.update(cx, |w, cx| w.seed(seeds, cx));
                        m.history = Load::Ready(groups);
                    }
                    Err(f) if !refresh => m.history = Load::Failed(f),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---- songs -----------------------------------------------------------------------------

    pub fn reload_songs(&mut self, cx: &mut Context<Self>) {
        self.load_songs(cx);
    }

    /// Loads the library's songs, page after page. The last whole answer is drawn at once and
    /// the list is read again behind it; without one the rows show as the pages arrive.
    fn load_songs(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.songs_stale = false;
        self.songs.pending = None;
        let cache = PageCache::try_global(cx);
        let ticket = cache.as_ref().map(|c| c.ticket());
        let cached = cache
            .as_ref()
            .and_then(|c| c.get::<Vec<TrackRow>>(keys::SONGS));
        self.songs_rev += 1;
        let rev = self.songs_rev;
        if let Some(cached) = cached {
            // Rows drawn from the cache; the fetch behind swaps in the current list when whole.
            let list_rev = self.songs.listing.begin();
            let seeds = like_seeds(&cached.value, false);
            let page = Page {
                items: cached.value,
                continuation: None,
                skipped: 0,
            };
            self.songs.listing.apply(list_rev, page);
            let writes = self.writes.read(cx);
            let seeds: Vec<(String, bool)> = seeds
                .into_iter()
                .filter(|(video, _)| writes.liked(video).is_none())
                .collect();
            self.writes.update(cx, |w, cx| w.seed(seeds, cx));
            self.reshow();
            cx.notify();
            self.refresh_songs(source, cache, ticket, rev, cx);
            return;
        }
        let list_rev = self.songs.listing.begin();
        self.reshow();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let mut result = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { read_songs(&*source, None) })
                    .await
            };
            loop {
                let next = this
                    .update(cx, |m, cx| {
                        m.apply_songs_page(rev, list_rev, result, ticket.as_ref(), cx)
                    })
                    .ok()
                    .flatten();
                let Some(token) = next else { break };
                let source = source.clone();
                result = cx
                    .background_executor()
                    .spawn(async move { read_songs(&*source, Some(&token)) })
                    .await;
            }
        })
        .detach();
    }

    /// Applies one page (or failure) of the songs and returns the token of the page to fetch
    /// next. The whole list is cached once its last page is in.
    fn apply_songs_page(
        &mut self,
        rev: u64,
        list_rev: Revision,
        result: Result<Page<TrackRow>, LoadFailure>,
        ticket: Option<&Ticket>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        if self.songs_rev != rev {
            return None;
        }
        let mut seeds = Vec::new();
        let next = match result {
            Ok(page) => {
                seeds = like_seeds(&page.items, false);
                match self.songs.listing.apply(list_rev, page) {
                    Applied::Yes => self
                        .songs
                        .listing
                        .continuation()
                        .map(|(_, token)| token.to_owned()),
                    Applied::Stale | Applied::RepeatedContinuation => None,
                }
            }
            Err(failure) => {
                self.songs.listing.fail(list_rev, failure);
                None
            }
        };
        if next.is_some() {
            self.songs.listing.mark_loading_more(list_rev);
        }
        if let (Some(ticket), LoadState::Complete) = (ticket, self.songs.listing.state()) {
            if let Some(cache) = PageCache::try_global(cx) {
                cache.put(ticket, keys::SONGS.to_owned(), self.songs.listing.items().to_vec());
            }
        }
        let pending = if self.songs.is_loading() {
            None
        } else {
            self.songs.pending.take()
        };
        self.reshow();
        self.writes.update(cx, |w, cx| w.seed(seeds, cx));
        if let Some(request) = pending.filter(|_| self.songs.failure().is_none()) {
            self.play_songs(request, cx);
        }
        cx.notify();
        next
    }

    /// Read the songs again without clearing what is shown; swap the list in when it is whole
    /// and differs. Quiet on failure: the rows already shown stay.
    fn refresh_songs(
        &mut self,
        source: Arc<dyn LibrarySource>,
        cache: Option<PageCache>,
        ticket: Option<Ticket>,
        rev: u64,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let mut fresh = Listing::<TrackRow>::new();
            let list_rev = fresh.begin();
            let mut result = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { read_songs(&*source, None) })
                    .await
            };
            let whole = loop {
                let Ok(page) = result else {
                    break false;
                };
                if fresh.apply(list_rev, page) != Applied::Yes {
                    break false;
                }
                let Some((_, token)) = fresh.continuation() else {
                    break true;
                };
                let token = token.to_owned();
                let source = source.clone();
                result = cx
                    .background_executor()
                    .spawn(async move { read_songs(&*source, Some(&token)) })
                    .await;
            };
            if !whole {
                return;
            }
            if let (Some(cache), Some(ticket)) = (&cache, &ticket) {
                cache.put(ticket, keys::SONGS.to_owned(), fresh.items().to_vec());
            }
            let _ = this.update(cx, |m, cx| {
                if m.songs_rev != rev || m.songs.is_loading() || m.songs.listing.items() == fresh.items() {
                    return;
                }
                let seeds = like_seeds(fresh.items(), false);
                m.songs.listing = fresh;
                m.reshow();
                m.writes.update(cx, |w, cx| w.seed(seeds, cx));
                cx.notify();
            });
        })
        .detach();
    }

    /// Plays a history row now, in front of whatever is queued (what a Search result does).
    pub fn play_history(&mut self, n: usize, cx: &mut Context<Self>) {
        let Some(row) = self.history_at(n) else {
            return;
        };
        self.player.update(cx, |p, cx| {
            p.hint_art(row.track.video_id.as_str(), row.thumbnail.as_deref());
            p.play_now(row.track, cx)
        });
    }

    /// Orders the tab's list. The choice is remembered.
    pub fn set_sort(&mut self, tab: Tab, key: SortKey, cx: &mut Context<Self>) {
        if !tab.sorts().contains(&key) || self.sort(tab) == key {
            return;
        }
        self.sorts.insert(tab, key);
        self.reshow();
        if let Some(store) = self.sort_store.clone() {
            cx.background_executor()
                .spawn(async move { store.save(tab.stored(), key.stored()) })
                .detach();
        }
        cx.notify();
    }

    /// Where the sort choices are kept: the saved ones are applied as soon as they are read.
    pub fn install_sort_store(
        &mut self,
        store: Option<Arc<dyn LibrarySortStore>>,
        cx: &mut Context<Self>,
    ) {
        self.sort_store = store.clone();
        let Some(store) = store else { return };
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn(async move {
                    [Tab::Playlists, Tab::Albums, Tab::Artists, Tab::Songs]
                        .into_iter()
                        .filter_map(|tab| {
                            let key = SortKey::from_stored(&store.load(tab.stored())?)?;
                            Some((tab, key))
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                for (tab, key) in saved {
                    // A choice made before the saved one was read wins.
                    if tab.sorts().contains(&key) {
                        m.sorts.entry(tab).or_insert(key);
                    }
                }
                m.reshow();
                cx.notify();
            });
        })
        .detach();
    }

    /// Rebuilds what the lists show from what was loaded and the chosen orders.
    fn reshow(&mut self) {
        let (mut playlists, mut albums) = match &self.collections {
            Load::Ready(c) => (c.playlists.clone(), c.albums.clone()),
            _ => (Vec::new(), Vec::new()),
        };
        sort_collections(&mut playlists, self.sort(Tab::Playlists));
        sort_collections(&mut albums, self.sort(Tab::Albums));
        let mut artists = match &self.artists {
            Load::Ready(list) => list.clone(),
            _ => Vec::new(),
        };
        sort_artists(&mut artists, self.sort(Tab::Artists));
        let mut songs = self.songs.listing.items().to_vec();
        sort_songs(&mut songs, self.sort(Tab::Songs));
        self.songs_shown = songs;
        self.shown = Shown {
            playlists,
            albums,
            artists,
        };
        self.rebuild_index();
    }

    // ---- search ----------------------------------------------------------------------------

    /// What Search looks through, and a counter that changes whenever it does.
    pub fn search_index(&self) -> (u64, &Arc<LocalIndex>) {
        (self.index_rev, &self.index)
    }

    fn rebuild_index(&mut self) {
        let songs: &[TrackRow] = match &self.liked_songs {
            Load::Ready(rows) => rows,
            _ => &[],
        };
        self.index = Arc::new(LocalIndex::new(
            songs,
            &self.shown.albums,
            &self.shown.playlists,
            &self.shown.artists,
        ));
        self.index_rev += 1;
    }

    /// A search is starting: bring in what it looks through that the screens have not loaded
    /// yet, the artists and the liked songs, with the reads those screens make. Playlists and
    /// albums load with the app. A load that is under way, or has answered, is left alone.
    pub fn ensure_searchable(&mut self, cx: &mut Context<Self>) {
        if self.source.is_none() {
            self.search_wanted = true;
            return;
        }
        self.search_wanted = false;
        if matches!(self.artists, Load::Idle | Load::Failed(_)) {
            self.load_artists(cx);
        }
        let stale = self.liked_stale && matches!(self.liked_songs, Load::Ready(_));
        if matches!(self.liked_songs, Load::Idle | Load::Failed(_)) || stale {
            self.load_liked_songs(cx);
        }
    }

    /// Every liked song, page after page. Rows already held stay until a complete new list
    /// replaces them, and stay for good if it fails.
    fn load_liked_songs(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if !matches!(self.liked_songs, Load::Ready(_)) {
            self.liked_songs = Load::Loading;
        }
        self.liked_stale = false;
        self.liked_rev += 1;
        let rev = self.liked_rev;
        cx.spawn(async move |this, cx| {
            let mut rows: Vec<TrackRow> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut listing = Listing::<TrackRow>::new();
            let list_rev = listing.begin();
            let liked = liked_music();
            let mut result = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { source.tracks(&liked) })
                    .await
            };
            let outcome = loop {
                let page = match result {
                    Ok(page) => page,
                    Err(f) => break Err(f),
                };
                if listing.apply(list_rev, page) != Applied::Yes {
                    break Ok(());
                }
                let Some((_, token)) = listing.continuation() else {
                    break Ok(());
                };
                let token = token.to_owned();
                let source = source.clone();
                result = cx
                    .background_executor()
                    .spawn(async move { source.more_tracks(&token) })
                    .await;
            };
            if outcome.is_ok() {
                for row in listing.items() {
                    // The same song twice in the list is one library song.
                    if seen.insert(row.track.video_id.as_str().to_owned()) {
                        rows.push(row.clone());
                    }
                }
            }
            let _ = this.update(cx, |m, cx| {
                if m.liked_rev != rev {
                    return;
                }
                match outcome {
                    Ok(()) => {
                        let seeds = like_seeds(&rows, true);
                        m.liked_songs = Load::Ready(Arc::new(rows));
                        m.writes.update(cx, |w, cx| w.seed(seeds, cx));
                    }
                    Err(f) if !matches!(m.liked_songs, Load::Ready(_)) => {
                        m.liked_songs = Load::Failed(f)
                    }
                    Err(_) => {}
                }
                m.rebuild_index();
                cx.notify();
            });
        })
        .detach();
    }

    // ---- pages --------------------------------------------------------------------------

    fn next_id(&mut self) -> u64 {
        let id = self.next_detail;
        self.next_detail += 1;
        id
    }

    pub fn open_target(&mut self, target: Target, cx: &mut Context<Self>) {
        match target {
            Target::Collection(c) => self.open(c, cx),
            Target::Artist(a) => self.open_artist(a, cx),
        }
    }

    /// Opens an album or playlist over whatever is showing.
    pub fn open(&mut self, collection: Collection, cx: &mut Context<Self>) {
        let id = self.next_id();
        self.stack.push(Entry::Collection(Detail {
            id,
            collection,
            listing: Listing::new(),
            pending: None,
            meta: PageMeta::default(),
            refresh: 0,
        }));
        self.load_detail(cx);
    }

    /// Opens an artist page over whatever is showing.
    pub fn open_artist(&mut self, target: ArtistTarget, cx: &mut Context<Self>) {
        let id = self.next_id();
        self.stack.push(Entry::Artist(ArtistDetail {
            id,
            target,
            state: Load::Idle,
            pending: None,
            subscribing: None,
            mix: None,
            bio_open: false,
        }));
        self.load_artist(cx);
    }

    /// Closes the page on top, back to the one beneath it (or the lists). Returns how many
    /// pages are still open.
    pub fn close(&mut self, cx: &mut Context<Self>) -> usize {
        if self.stack.pop().is_some() {
            cx.notify();
        }
        self.stack.len()
    }

    /// Back to the lists, whatever is open.
    pub fn close_all(&mut self, cx: &mut Context<Self>) {
        if !self.stack.is_empty() {
            self.stack.clear();
            cx.notify();
        }
    }

    /// Start over: used by Retry, and after a failure part-way through the pages.
    pub fn reload_detail(&mut self, cx: &mut Context<Self>) {
        if self.songs_open() {
            self.load_songs(cx);
        } else {
            self.load_detail(cx);
        }
    }

    fn collection_mut(&mut self, id: u64) -> Option<&mut Detail> {
        self.stack.iter_mut().find_map(|e| match e {
            Entry::Collection(d) if d.id == id => Some(d),
            _ => None,
        })
    }

    fn artist_mut(&mut self, id: u64) -> Option<&mut ArtistDetail> {
        self.stack.iter_mut().find_map(|e| match e {
            Entry::Artist(a) if a.id == id => Some(a),
            _ => None,
        })
    }

    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(Entry::Collection(detail)) = self.stack.last_mut() else {
            return;
        };
        let (id, collection) = (detail.id, detail.collection.clone());
        detail.pending = None;
        // The last answer (the whole list) is drawn at once and fetched again behind it.
        let cache = PageCache::try_global(cx);
        let cached = cache
            .as_ref()
            .and_then(|c| c.get::<Vec<TrackRow>>(&keys::collection(&collection.id)));
        if let Some(cached) = cached {
            let rev = detail.listing.begin();
            let page = Page {
                items: cached.value,
                continuation: None,
                skipped: 0,
            };
            self.apply_page(id, rev, Ok(page), Origin::Cached, cx);
            self.refresh_detail(id, cx);
            return;
        }
        let rev = detail.listing.begin();
        let ticket = cache.map(|c| c.ticket());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let first = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { source.page(&collection) })
                    .await
            };
            let mut result = match first {
                Ok((page, meta)) => {
                    let _ = this.update(cx, |m, _| {
                        if let Some(d) = m.collection_mut(id) {
                            if d.listing.revision() == rev {
                                d.meta = meta;
                            }
                        }
                    });
                    Ok(page)
                }
                Err(failure) => Err(failure),
            };
            loop {
                let next = this
                    .update(cx, |m, cx| m.apply_page(id, rev, result, Origin::Live(ticket.as_ref()), cx))
                    .ok()
                    .flatten();
                let Some(token) = next else { break };
                let source = source.clone();
                result = cx
                    .background_executor()
                    .spawn(async move { source.more_tracks(&token) })
                    .await;
            }
        })
        .detach();
    }

    /// Applies one page (or failure) and returns the token of the page to fetch next, if the
    /// load should go on. A live fetch carries its cache ticket: when its last page lands the
    /// whole list is cached. A page drawn from the cache only teaches the like state of tracks
    /// nothing is known about yet; the fetch behind it settles the rest.
    fn apply_page(
        &mut self,
        detail_id: u64,
        rev: Revision,
        result: Result<Page<TrackRow>, LoadFailure>,
        origin: Origin<'_>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let detail = self.collection_mut(detail_id)?;
        let liked_music = detail.collection.is_liked_music();
        let mut seeds = Vec::new();
        let next = match result {
            Ok(page) => {
                seeds = like_seeds(&page.items, liked_music);
                match detail.listing.apply(rev, page) {
                    Applied::Yes => detail
                        .listing
                        .continuation()
                        .map(|(_, token)| token.to_owned()),
                    Applied::Stale | Applied::RepeatedContinuation => None,
                }
            }
            Err(failure) => {
                detail.listing.fail(rev, failure);
                None
            }
        };
        if next.is_some() {
            detail.listing.mark_loading_more(rev);
        }
        let settled = !detail.is_loading();
        let failed = detail.failure().is_some();
        let pending = if settled { detail.pending.take() } else { None };
        let to_cache = match origin {
            Origin::Live(Some(ticket)) if detail.listing.state() == LoadState::Complete => {
                Some((ticket.clone(), detail.collection.id.clone(), detail.listing.items().to_vec()))
            }
            _ => None,
        };
        // Opening a page is the strongest hint that its first track is about to be played.
        let first = detail.listing.items().first().map(|r| r.track.clone());
        if let Some((ticket, id, rows)) = to_cache {
            if let Some(cache) = PageCache::try_global(cx) {
                cache.put(&ticket, keys::collection(&id), rows);
            }
        }
        if let Some(track) = first.filter(|_| pending.is_none() && !failed) {
            self.player.update(cx, |p, _| p.prefetch(&track));
        }
        if matches!(origin, Origin::Cached) {
            let writes = self.writes.read(cx);
            seeds.retain(|(video, _)| writes.liked(video).is_none());
        }
        self.writes.update(cx, |w, cx| w.seed(seeds, cx));
        if let Some(request) = pending.filter(|_| !failed) {
            self.play_collection(detail_id, request, cx);
        }
        cx.notify();
        next
    }

    // ---- likes ---------------------------------------------------------------------------

    /// A like or unlike landed: any open Liked Music page follows it without a reload. An unlike
    /// drops the row on the spot; a like fetches the list again behind the rows already shown
    /// and swaps it in when it is whole (a page that is still loading already brings the
    /// current list). A page that is not open needs nothing: opening one always loads it fresh.
    fn liked_changed(&mut self, video_id: &str, liked: bool, cx: &mut Context<Self>) {
        match &mut self.liked_songs {
            Load::Ready(rows) => {
                let listed = rows.iter().any(|r| r.track.video_id.as_str() == video_id);
                if !liked && listed {
                    Arc::make_mut(rows).retain(|r| r.track.video_id.as_str() != video_id);
                    self.rebuild_index();
                    cx.notify();
                } else if liked && !listed {
                    // Only the id is known here: the next search reads the list again.
                    self.liked_stale = true;
                }
            }
            // A load under way may have read the list before this change.
            Load::Loading => self.liked_stale = true,
            _ => {}
        }
        let mut refresh = Vec::new();
        let mut dropped = false;
        for entry in &mut self.stack {
            let Entry::Collection(d) = entry else {
                continue;
            };
            if !d.collection.is_liked_music() {
                continue;
            }
            if liked {
                let listed = d
                    .listing
                    .items()
                    .iter()
                    .any(|r| r.track.video_id.as_str() == video_id);
                if !listed && !d.is_loading() {
                    refresh.push(d.id);
                }
            } else {
                dropped |= d.listing.retain(|r| r.track.video_id.as_str() != video_id) > 0;
            }
        }
        for id in refresh {
            self.refresh_detail(id, cx);
        }
        if dropped {
            cx.notify();
        }
    }

    /// Load a page's list again without clearing what it shows; swap it in when it is complete
    /// (and only when it differs, so an unchanged page is not redrawn). Quiet on failure: the
    /// rows already shown stay.
    fn refresh_detail(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let cache = PageCache::try_global(cx);
        let ticket = cache.as_ref().map(|c| c.ticket());
        let Some(detail) = self.collection_mut(id) else {
            return;
        };
        detail.refresh += 1;
        let (generation, collection) = (detail.refresh, detail.collection.clone());
        cx.spawn(async move |this, cx| {
            let mut fresh = Listing::<TrackRow>::new();
            let rev = fresh.begin();
            let mut result = {
                let source = source.clone();
                let collection = collection.clone();
                cx.background_executor()
                    .spawn(async move { source.tracks(&collection) })
                    .await
            };
            let whole = loop {
                let Ok(page) = result else {
                    break false;
                };
                if fresh.apply(rev, page) != Applied::Yes {
                    break false;
                }
                let Some((_, token)) = fresh.continuation() else {
                    break true;
                };
                let token = token.to_owned();
                let source = source.clone();
                result = cx
                    .background_executor()
                    .spawn(async move { source.more_tracks(&token) })
                    .await;
            };
            if !whole {
                return;
            }
            if let (Some(cache), Some(ticket)) = (&cache, &ticket) {
                cache.put(ticket, keys::collection(&collection.id), fresh.items().to_vec());
            }
            let _ = this.update(cx, |m, cx| {
                let seeds = like_seeds(fresh.items(), collection.is_liked_music());
                if let Some(d) = m.collection_mut(id) {
                    if d.refresh == generation && !d.is_loading() && d.listing.items() != fresh.items()
                    {
                        d.listing = fresh;
                        m.writes.update(cx, |w, cx| w.seed(seeds, cx));
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    // ---- playlist edits ------------------------------------------------------------------

    /// The lists follow what was made, renamed, deleted or saved. A list that is not loaded yet
    /// needs nothing: its load brings the current library.
    fn edit_happened(&mut self, event: &EditEvent, cx: &mut Context<Self>) {
        match event {
            EditEvent::Created(c) => self.patch_lists(|l| insert_collection(l, c.clone())),
            EditEvent::Renamed(c) => {
                let key = c.library_key().to_owned();
                self.patch_lists(|l| {
                    for entry in l.playlists.iter_mut().filter(|e| e.library_key() == key) {
                        entry.title = c.title.clone();
                    }
                });
                for entry in &mut self.stack {
                    if let Entry::Collection(d) = entry {
                        if d.collection.kind == c.kind && d.collection.library_key() == key {
                            d.collection.title = c.title.clone();
                        }
                    }
                }
            }
            EditEvent::Deleted(c) => {
                let key = c.library_key().to_owned();
                self.patch_lists(|l| l.playlists.retain(|e| e.library_key() != key));
            }
            EditEvent::Saved { collection, saved } => {
                let key = collection.library_key().to_owned();
                if *saved {
                    self.patch_lists(|l| insert_collection(l, collection.clone()));
                } else {
                    self.patch_lists(|l| match collection.kind {
                        CollectionKind::Playlist => l.playlists.retain(|e| e.library_key() != key),
                        CollectionKind::Album => l.albums.retain(|e| e.library_key() != key),
                    });
                }
            }
            // The lists did not change, but an open page shows the description the dialog saved.
            EditEvent::Edited(c) => {
                let saved = self.edits.read(cx).details(c).map(|d| d.description.clone());
                if let Some(description) = saved {
                    let key = c.library_key();
                    for entry in &mut self.stack {
                        if let Entry::Collection(d) = entry {
                            if d.collection.kind == c.kind && d.collection.library_key() == key {
                                d.meta.description =
                                    Some(description.clone()).filter(|t| !t.is_empty());
                            }
                        }
                    }
                }
            }
            EditEvent::SongSaved { video_id, saved } => self.song_saved(video_id, *saved, cx),
            EditEvent::Failed(_) => {}
        }
        cx.notify();
    }

    /// A song was saved to the library or taken out: the Songs tab follows. A removal drops the
    /// row on the spot; a save reads the list again (the row's own details are not known here),
    /// at once when the tab is the one showing, else the next time it opens.
    fn song_saved(&mut self, video_id: &str, saved: bool, cx: &mut Context<Self>) {
        if !saved {
            let dropped = self
                .songs
                .listing
                .retain(|r| r.track.video_id.as_str() != video_id);
            if dropped > 0 {
                self.reshow();
            }
            return;
        }
        let listed = self
            .songs
            .listing
            .items()
            .iter()
            .any(|r| r.track.video_id.as_str() == video_id);
        if listed || matches!(self.songs.listing.state(), LoadState::Idle) {
            return;
        }
        self.songs_stale = true;
        if self.songs_open() && !self.songs.is_loading() {
            self.load_songs(cx);
        }
    }

    fn patch_lists(&mut self, f: impl FnOnce(&mut Collections)) {
        if let Load::Ready(lists) = &mut self.collections {
            f(lists);
            self.reshow();
        }
    }

    /// Remove the row with this occurrence token from the playlist page on top: it is gone at once
    /// and the write follows. Only the page of a playlist the account owns, once all its rows are
    /// loaded; anything else is ignored.
    pub fn remove_row(&mut self, playlist_key: &str, occurrence: &str, cx: &mut Context<Self>) {
        let Some(Entry::Collection(d)) = self.stack.last_mut() else {
            return;
        };
        if d.collection.library_key() != playlist_key || !d.can_edit() {
            return;
        }
        let Some(row) = d
            .listing
            .items()
            .iter()
            .find(|r| r.occurrence.as_deref() == Some(occurrence))
            .cloned()
        else {
            return;
        };
        d.listing.retain(|r| r.occurrence.as_deref() != Some(occurrence));
        let collection = d.collection.clone();
        let edit = PageEdit::Remove {
            video_id: row.track.video_id.as_str().to_owned(),
            occurrence: occurrence.to_owned(),
        };
        self.queue_edit(collection, edit, cx);
        cx.notify();
    }

    /// Move row `from` of the playlist page on top to sit before the insertion slot `slot`: it
    /// moves at once and the write follows. Returns whether anything moved.
    pub fn move_row(&mut self, from: usize, slot: usize, cx: &mut Context<Self>) -> bool {
        let Some(Entry::Collection(d)) = self.stack.last_mut() else {
            return false;
        };
        if !d.can_edit() {
            return false;
        }
        let items = d.listing.items();
        let tokens: Vec<String> = items.iter().filter_map(|r| r.occurrence.clone()).collect();
        // A row with no token cannot be named to the service, so the page cannot be reordered.
        if tokens.len() != items.len() {
            return false;
        }
        let Some(plan) = plan_move(items.len(), from, slot) else {
            return false;
        };
        let edit = PageEdit::Move {
            occurrence: tokens[from].clone(),
            before: plan.before.map(|i| tokens[i].clone()),
            desired: moved(&tokens, from, slot),
        };
        d.listing.move_item(from, slot);
        let collection = d.collection.clone();
        self.queue_edit(collection, edit, cx);
        cx.notify();
        true
    }

    fn queue_edit(&mut self, collection: Collection, edit: PageEdit, cx: &mut Context<Self>) {
        // The account is named now, when the person acted, not when the edit goes out.
        let token = self.source.as_ref().and_then(|s| s.account_token());
        let key = collection.library_key().to_owned();
        edits::Stale::Collection(key.clone()).invalidate(cx);
        self.edit_queues
            .entry(key.clone())
            .or_insert_with(|| EditQueue {
                collection,
                items: VecDeque::new(),
                busy: false,
            })
            .items
            .push_back((edit, token));
        self.pump_edits(&key, cx);
    }

    /// Send the next waiting edit of a playlist, unless one is already out.
    fn pump_edits(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(queue) = self.edit_queues.get_mut(key) else {
            return;
        };
        if queue.busy {
            return;
        }
        let Some((edit, token)) = queue.items.pop_front() else {
            self.edit_queues.remove(key);
            return;
        };
        queue.busy = true;
        let collection = queue.collection.clone();
        let (key, generation) = (key.to_owned(), self.edit_epoch);
        let kind = match &edit {
            PageEdit::Remove { .. } => Edit::RemoveSong,
            PageEdit::Move { .. } => Edit::MoveSong,
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    match edit {
                        PageEdit::Remove {
                            video_id,
                            occurrence,
                        } => edits::remove(&*source, &token, &collection, &video_id, &occurrence),
                        PageEdit::Move {
                            occurrence,
                            before,
                            desired,
                        } => edits::move_row(
                            &*source,
                            &token,
                            &collection,
                            &occurrence,
                            before.as_deref(),
                            &desired,
                        ),
                    }
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                // The account changed while it was out: its answer is not for this library.
                if m.edit_epoch == generation {
                    m.edit_settled(&key, kind, result, cx);
                }
            });
        })
        .detach();
    }

    fn edit_settled(
        &mut self,
        key: &str,
        kind: Edit,
        result: Result<(), apollo_domain::library::WriteFailure>,
        cx: &mut Context<Self>,
    ) {
        // Whatever a read cached while the write was out is not the playlist as it is now.
        edits::Stale::Collection(key.to_owned()).invalidate(cx);
        match result {
            Ok(()) => {
                if let Some(queue) = self.edit_queues.get_mut(key) {
                    queue.busy = false;
                }
                self.pump_edits(key, cx);
            }
            Err(failure) => {
                // What is shown was ahead of the service: put the service's own list back, and
                // drop what was waiting behind the edit that failed (it assumed that one landed).
                self.edit_queues.remove(key);
                let text = edits::failure_text(kind, failure);
                self.writes.update(cx, |w, cx| w.raise(text, cx));
                let ids: Vec<u64> = self
                    .stack
                    .iter()
                    .filter_map(|e| match e {
                        Entry::Collection(d) if d.collection.library_key() == key => Some(d.id),
                        _ => None,
                    })
                    .collect();
                for id in ids {
                    self.refresh_detail(id, cx);
                }
            }
        }
    }

    /// A playlist's edits are still going out (tests).
    pub fn edits_pending(&self, key: &str) -> bool {
        self.edit_queues.contains_key(key)
    }

    // ---- artist --------------------------------------------------------------------------

    pub fn reload_artist(&mut self, cx: &mut Context<Self>) {
        self.load_artist(cx);
    }

    fn load_artist(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(Entry::Artist(detail)) = self.stack.last_mut() else {
            return;
        };
        let (id, browse_id) = (detail.id, detail.target.id.clone());
        detail.pending = None;
        // The last answer is drawn at once and fetched again behind it.
        let cache = PageCache::try_global(cx);
        let cached = cache
            .as_ref()
            .and_then(|c| c.get::<ArtistContent>(&keys::artist(&browse_id)));
        let ticket = cache.as_ref().map(|c| c.ticket());
        let how = if cached.is_some() {
            Answer::Behind
        } else {
            Answer::Fresh
        };
        match cached {
            Some(cached) => self.apply_artist(id, Ok(cached.value), Answer::Cached, cx),
            None => {
                detail.state = Load::Loading;
                cx.notify();
            }
        }
        cx.spawn(async move |this, cx| {
            let asked = browse_id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.artist(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if let (Ok(content), Some(cache), Some(ticket)) = (&result, &cache, &ticket) {
                    cache.put(ticket, keys::artist(&browse_id), content.clone());
                }
                m.apply_artist(id, result, how, cx)
            });
        })
        .detach();
    }

    /// Applies an artist page's answer; see [`Answer`].
    fn apply_artist(
        &mut self,
        id: u64,
        result: Result<ArtistContent, LoadFailure>,
        how: Answer,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        if how == Answer::Behind {
            match &result {
                Ok(content) if detail.content().is_some_and(|c| **c == *content) => return,
                Err(failure) if *failure != LoadFailure::Session && detail.content().is_some() => {
                    return
                }
                _ => {}
            }
        }
        let pending = detail.pending.take();
        let ok = result.is_ok();
        let mut seeds = result
            .as_ref()
            .map(|c| like_seeds(&c.top_songs, false))
            .unwrap_or_default();
        detail.state = match result {
            Ok(content) => Load::Ready(Arc::new(content)),
            Err(failure) => Load::Failed(failure),
        };
        if how == Answer::Cached {
            let writes = self.writes.read(cx);
            seeds.retain(|(video, _)| writes.liked(video).is_none());
        }
        let first = match &self.artist_mut(id).map(|d| &d.state) {
            Some(Load::Ready(c)) => c.top_songs.first().map(|r| r.track.clone()),
            _ => None,
        };
        if let Some(track) = first.filter(|_| pending.is_none()) {
            self.player.update(cx, |p, _| p.prefetch(&track));
        }
        self.writes.update(cx, |w, cx| w.seed(seeds, cx));
        if let Some(request) = pending.filter(|_| ok) {
            self.play_artist_request(id, request, cx);
        }
        cx.notify();
    }

    // ---- playback ----------------------------------------------------------------------

    /// Whether the Skim button shows (Settings > Playback > Skim).
    pub fn skim_enabled(&self, cx: &App) -> bool {
        self.player.read(cx).skim_enabled()
    }

    /// Plays the open page now, or as soon as it has finished loading.
    pub fn request_play(&mut self, request: PlayRequest, cx: &mut Context<Self>) {
        match self.stack.last_mut() {
            Some(Entry::Collection(detail)) => {
                if detail.is_loading() {
                    detail.pending = Some(request);
                    cx.notify();
                } else {
                    let id = detail.id;
                    self.play_collection(id, request, cx);
                }
            }
            Some(Entry::Artist(detail)) => match detail.state {
                Load::Idle | Load::Loading => {
                    detail.pending = Some(request);
                    cx.notify();
                }
                Load::Ready(_) => {
                    let id = detail.id;
                    self.play_artist_request(id, request, cx);
                }
                Load::Failed(_) => {}
            },
            None if self.tab == Tab::Songs => {
                if self.songs.is_loading() {
                    self.songs.pending = Some(request);
                    cx.notify();
                } else if self.songs.failure().is_none() {
                    self.play_songs(request, cx);
                }
            }
            None => {}
        }
    }

    /// The Songs tab's filter text changed (already lowercased and trimmed).
    pub fn set_songs_query(&mut self, text: &str) {
        self.songs_query = text.to_owned();
    }

    /// Plays the Songs tab's rows in the order they are shown, filter included.
    fn play_songs(&mut self, request: PlayRequest, cx: &mut Context<Self>) {
        let kept = Visible::of(&self.songs_shown, &self.songs_query);
        let rows: Vec<(Track, Option<String>)> = (0..kept.len())
            .filter_map(|n| kept.get(n))
            .filter_map(|i| self.songs_shown.get(i))
            .map(|row| (row.track.clone(), row.thumbnail.clone()))
            .collect();
        let source = queue_source::named("Library songs");
        self.play_tracks(rows, request, Some(source), cx);
    }

    fn play_collection(&mut self, id: u64, request: PlayRequest, cx: &mut Context<Self>) {
        let Some(detail) = self.collection_mut(id) else {
            return;
        };
        let rows: Vec<(Track, Option<String>)> = detail
            .listing
            .items()
            .iter()
            .map(|row| (row.track.clone(), row.thumbnail.clone()))
            .collect();
        let source = queue_source::of_collection(&detail.collection);
        self.play_tracks(rows, request, Some(source), cx);
    }

    /// An artist's play commands act on the top songs shown on the page.
    fn play_artist(&mut self, id: u64, request: PlayRequest, cx: &mut Context<Self>) {
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        let rows: Vec<(Track, Option<String>)> = detail
            .content()
            .map(|c| {
                c.top_songs
                    .iter()
                    .map(|r| (r.track.clone(), r.thumbnail.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let source = queue_source::of_artist(&detail.target);
        self.play_tracks(rows, request, Some(source), cx);
    }

    /// `rows` pair each track with its listing thumbnail, which the artwork source is told about
    /// first: it only knows the start-up queue's covers otherwise.
    fn play_tracks(
        &mut self,
        rows: Vec<(Track, Option<String>)>,
        request: PlayRequest,
        source: Option<QueueSource>,
        cx: &mut Context<Self>,
    ) {
        if rows.is_empty() {
            return;
        }
        let (tracks, thumbs): (Vec<Track>, Vec<Option<String>>) = rows.into_iter().unzip();
        self.player.update(cx, |p, cx| {
            for (track, thumb) in tracks.iter().zip(&thumbs) {
                p.hint_art(track.video_id.as_str(), thumb.as_deref());
            }
            if request.skim {
                p.start_skim(tracks, source, cx)
            } else {
                p.play_tracks_from(tracks, request.start, request.shuffle, source, cx)
            }
        });
    }

    // ---- artwork -----------------------------------------------------------------------

    /// The decoded cover, if it has arrived. Pure read, so views can call it while drawing.
    pub fn art(&self, url: &str, size: ArtSize) -> Option<Arc<RenderImage>> {
        match self.thumbs.map.get(&key(url, size)) {
            Some(Thumb::Ready(image)) => Some(image.clone()),
            _ => None,
        }
    }

    /// The cover as the shared card and tile pieces want it.
    pub fn thumb(&self, url: Option<&str>, size: ArtSize) -> ThumbView {
        match url.and_then(|u| self.thumbs.map.get(&key(u, size))) {
            Some(Thumb::Ready(image)) => ThumbView::Image(image.clone()),
            Some(Thumb::Missing) => ThumbView::Missing,
            Some(Thumb::Loading) | None => ThumbView::Pending,
        }
    }

    /// True once a cover was requested (arrived, failed or in flight).
    pub fn art_requested(&self, url: &str, size: ArtSize) -> bool {
        self.thumbs.map.contains_key(&key(url, size))
    }

    pub fn want_art(&mut self, url: &str, size: ArtSize, cx: &mut Context<Self>) {
        let k = key(url, size);
        if self.thumbs.map.contains_key(&k) {
            return;
        }
        self.thumbs.map.insert(k, Thumb::Loading);
        self.thumbs.queue.push((url.to_owned(), size));
        self.pump_art(cx);
    }

    fn pump_art(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        while self.thumbs.inflight < MAX_INFLIGHT_ART {
            let Some((url, size)) = self.thumbs.queue.pop() else {
                break;
            };
            self.thumbs.inflight += 1;
            let source = source.clone();
            // Covers shown large ask for a larger picture than the list thumbnail they came as.
            let fetch_url = match size {
                ArtSize::List => url.clone(),
                ArtSize::Header => sized_thumbnail(&url, HEADER_ART_PX),
                ArtSize::Card => sized_thumbnail(&url, CARD_ART_PX),
                // The service's own crop, as it came.
                ArtSize::Banner => url.clone(),
            };
            cx.spawn(async move |this, cx| {
                let image = cx
                    .background_executor()
                    .spawn(async move {
                        source
                            .artwork(&fetch_url)
                            .map(|art| sized_render_image(&art, size))
                    })
                    .await;
                let _ = this.update(cx, |m, cx| {
                    m.thumbs.inflight -= 1;
                    m.finish_art(key(&url, size), image);
                    m.pump_art(cx);
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn finish_art(&mut self, k: String, image: Option<Arc<RenderImage>>) {
        self.thumbs.map.insert(
            k.clone(),
            match image {
                Some(i) => Thumb::Ready(i),
                None => Thumb::Missing,
            },
        );
        self.thumbs.order.push_back(k);
        while self.thumbs.order.len() > ART_CACHE_CAP {
            if let Some(old) = self.thumbs.order.pop_front() {
                self.thumbs.map.remove(&old);
            }
        }
    }
}

/// The Songs tab's page before anything is read: it has no collection behind it, so the one it
/// carries is only a name (and never opened or edited: it is not a user playlist).
fn songs_page() -> Detail {
    Detail {
        id: 0,
        collection: Collection {
            kind: CollectionKind::Playlist,
            id: BrowseId::new(SONGS_BROWSE_ID),
            title: "Songs".into(),
            subtitle: None,
            thumbnail: None,
        },
        listing: Listing::new(),
        pending: None,
        meta: PageMeta::default(),
        refresh: 0,
    }
}

/// A collection into the lists: a playlist right after Liked Music (where a new or saved one
/// appears in the service's own order), an album first. One already listed is left alone.
fn insert_collection(lists: &mut Collections, collection: Collection) {
    let key = collection.library_key().to_owned();
    match collection.kind {
        CollectionKind::Playlist => {
            if !lists.playlists.iter().any(|c| c.library_key() == key) {
                let at = usize::from(lists.playlists.first().is_some_and(Collection::is_liked_music));
                lists.playlists.insert(at, collection);
            }
        }
        CollectionKind::Album => {
            if !lists.albums.iter().any(|c| c.library_key() == key) {
                lists.albums.insert(0, collection);
            }
        }
    }
}

/// Liked Music first, exactly once, then the service's playlists and albums in their order.
pub fn split_collections(list: Vec<Collection>) -> Collections {
    let mut playlists = vec![liked_music()];
    let mut albums = Vec::new();
    for c in list {
        match c.kind {
            CollectionKind::Playlist if c.is_liked_music() => {
                // Keep the service's own card for it (it may carry art), in the first slot.
                playlists[0] = c;
            }
            CollectionKind::Playlist => playlists.push(c),
            CollectionKind::Album => albums.push(c),
        }
    }
    Collections { playlists, albums }
}

pub fn liked_music() -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: BrowseId::new(LIKED_MUSIC_BROWSE_ID),
        title: "Liked Music".into(),
        subtitle: None,
        thumbnail: None,
    }
}

/// Centre square of an image of `w` x `h`: `(x, y, side)`. A wide artist banner is cropped to
/// its middle, never squeezed.
fn centre_square(w: u32, h: u32) -> (u32, u32, u32) {
    let side = w.min(h);
    ((w - side) / 2, (h - side) / 2, side)
}

/// A banner keeps its whole picture (the page crops it to its frame): only shrunk when it is
/// wider than any window needs.
fn banner_render_image(art: &DecodedArt) -> Arc<RenderImage> {
    let (w, h) = art.pixels.dimensions();
    if w <= BANNER_ART_PX {
        return art.render_image();
    }
    let height = (u64::from(h) * u64::from(BANNER_ART_PX) / u64::from(w)).max(1) as u32;
    let resized = imageops::resize(
        &art.pixels,
        BANNER_ART_PX,
        height,
        imageops::FilterType::Triangle,
    );
    DecodedArt::from_rgba(resized).render_image()
}

/// Centre-cropped to a square, resized, BGRA, ready for GPUI.
fn sized_render_image(art: &DecodedArt, size: ArtSize) -> Arc<RenderImage> {
    let side = match size {
        ArtSize::List => LIST_ART_PX,
        ArtSize::Header => HEADER_ART_PX,
        ArtSize::Card => CARD_ART_PX,
        ArtSize::Banner => return banner_render_image(art),
    };
    let (w, h) = art.pixels.dimensions();
    if w == h && w <= side {
        return art.render_image();
    }
    let (x, y, edge) = centre_square(w, h);
    let square = imageops::crop_imm(&art.pixels, x, y, edge, edge).to_image();
    if edge <= side {
        return DecodedArt::from_rgba(square).render_image();
    }
    let resized = imageops::resize(&square, side, side, imageops::FilterType::Triangle);
    DecodedArt::from_rgba(resized).render_image()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(kind: CollectionKind, id: &str, title: &str) -> Collection {
        Collection {
            kind,
            id: BrowseId::new(id),
            title: title.into(),
            subtitle: None,
            thumbnail: None,
        }
    }

    #[test]
    fn liked_music_is_first_and_never_doubled() {
        let split = split_collections(vec![
            c(CollectionKind::Playlist, "VLPLa", "A"),
            c(
                CollectionKind::Playlist,
                LIKED_MUSIC_BROWSE_ID,
                "Liked Music (service)",
            ),
            c(CollectionKind::Album, "MPREb_x", "Album"),
        ]);
        assert_eq!(split.playlists.len(), 2);
        assert!(split.playlists[0].is_liked_music());
        assert_eq!(split.playlists[0].title, "Liked Music (service)");
        assert_eq!(split.playlists[1].title, "A");
        assert_eq!(split.albums.len(), 1);
    }

    #[test]
    fn liked_music_exists_even_when_the_service_does_not_list_it() {
        let split = split_collections(vec![c(CollectionKind::Playlist, "VLPLa", "A")]);
        assert!(split.playlists[0].is_liked_music());
        assert_eq!(split.playlists[0].title, "Liked Music");
        assert!(split_collections(Vec::new()).albums.is_empty());
    }

    #[test]
    fn a_wide_banner_is_cropped_to_its_middle_not_squeezed() {
        assert_eq!(centre_square(1440, 600), (420, 0, 600));
        assert_eq!(centre_square(600, 600), (0, 0, 600));
        assert_eq!(centre_square(300, 500), (0, 100, 300));
    }

    #[test]
    fn row_requests_keep_the_players_shuffle_setting() {
        assert_eq!(PlayRequest::row(3).shuffle, None);
        assert_eq!(PlayRequest::row(3).start, Some(3));
        assert_eq!(PlayRequest::PLAY_ALL.shuffle, Some(false));
        assert_eq!(PlayRequest::SHUFFLE.shuffle, Some(true));
    }
}
