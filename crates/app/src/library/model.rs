//! Library screen state: the collection lists, the open collection and its paged track list,
//! the artwork cache, and the playback commands. No rendering; `ui::library` draws it.
//!
//! All service calls go through [`LibrarySource`] on GPUI's background executor. Every
//! completion carries the id it was started for, so an answer for a collection that has since
//! been closed or replaced is dropped instead of landing in the wrong list.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use apollo_domain::library::{Collection, CollectionKind, LIKED_MUSIC_BROWSE_ID};
use apollo_domain::listing::{Applied, Listing, LoadFailure, LoadState, Page, Revision};
use apollo_domain::model::{BrowseId, Track};
use gpui::{Context, Entity, RenderImage};
use image::imageops;

use super::source::{LibrarySource, TrackRow};
use crate::art::DecodedArt;
use crate::player::{PlayerModel, Status};

/// Cover downloads running at once. Fast scrolling queues many; the newest request goes first.
const MAX_INFLIGHT_ART: usize = 4;
/// Decoded covers kept. Rows are small, so this is a few dozen MB at most.
const ART_CACHE_CAP: usize = 400;
/// Pixel size covers are decoded to (2x the 48px row art for high-DPI displays).
const LIST_ART_PX: u32 = 96;
/// Detail header art, drawn at 160px.
const HEADER_ART_PX: u32 = 320;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Playlists,
    Albums,
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
}

impl PlayRequest {
    pub const PLAY_ALL: PlayRequest = PlayRequest {
        start: None,
        shuffle: Some(false),
    };
    pub const SHUFFLE: PlayRequest = PlayRequest {
        start: None,
        shuffle: Some(true),
    };

    pub fn row(index: usize) -> Self {
        Self {
            start: Some(index),
            shuffle: None,
        }
    }
}

pub struct Detail {
    id: u64,
    pub collection: Collection,
    pub listing: Listing<TrackRow>,
    pub pending: Option<PlayRequest>,
}

impl Detail {
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

fn key(url: &str, size: ArtSize) -> String {
    format!("{size:?}|{url}")
}

pub struct LibraryModel {
    player: Entity<PlayerModel>,
    source: Option<Arc<dyn LibrarySource>>,
    tab: Tab,
    collections: Load<Collections>,
    collections_rev: u64,
    detail: Option<Detail>,
    next_detail: u64,
    thumbs: Thumbs,
}

impl LibraryModel {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |m, _, cx| m.try_start(cx)).detach();
        let mut model = Self {
            player,
            source: None,
            tab: Tab::Playlists,
            collections: Load::Idle,
            collections_rev: 0,
            detail: None,
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

    pub fn list(&self, tab: Tab) -> &[Collection] {
        match (&self.collections, tab) {
            (Load::Ready(c), Tab::Playlists) => &c.playlists,
            (Load::Ready(c), Tab::Albums) => &c.albums,
            _ => &[],
        }
    }

    pub fn detail(&self) -> Option<&Detail> {
        self.detail.as_ref()
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
        self.detail = None;
        // Covers still downloading finish into the new cache harmlessly; keep their count.
        self.thumbs = Thumbs {
            inflight: self.thumbs.inflight,
            ..Thumbs::default()
        };
        self.tab = Tab::Playlists;
        self.collections_rev += 1;
        self.collections = Load::Idle;
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
        self.collections = Load::Loading;
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
                m.collections = match result {
                    Ok(list) => Load::Ready(split_collections(list)),
                    Err(f) => Load::Failed(f),
                };
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.tab = tab;
            cx.notify();
        }
    }

    // ---- one collection ----------------------------------------------------------------

    pub fn open(&mut self, collection: Collection, cx: &mut Context<Self>) {
        let id = self.next_detail;
        self.next_detail += 1;
        self.detail = Some(Detail {
            id,
            collection,
            listing: Listing::new(),
            pending: None,
        });
        self.load_detail(cx);
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.detail.take().is_some() {
            cx.notify();
        }
    }

    /// Start over: used by Retry, and after a failure part-way through the pages.
    pub fn reload_detail(&mut self, cx: &mut Context<Self>) {
        self.load_detail(cx);
    }

    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let (Some(source), Some(detail)) = (self.source.clone(), self.detail.as_mut()) else {
            return;
        };
        let (id, rev, collection) = (detail.id, detail.listing.begin(), detail.collection.clone());
        detail.pending = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let first = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { source.tracks(&collection) })
                    .await
            };
            let mut result = first;
            loop {
                let next = this
                    .update(cx, |m, cx| m.apply_page(id, rev, result, cx))
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
    /// load should go on.
    fn apply_page(
        &mut self,
        detail_id: u64,
        rev: Revision,
        result: Result<Page<TrackRow>, LoadFailure>,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let detail = self.detail.as_mut().filter(|d| d.id == detail_id)?;
        let next = match result {
            Ok(page) => match detail.listing.apply(rev, page) {
                Applied::Yes => detail
                    .listing
                    .continuation()
                    .map(|(_, token)| token.to_owned()),
                Applied::Stale | Applied::RepeatedContinuation => None,
            },
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
        if let Some(request) = pending.filter(|_| !failed) {
            self.play(request, cx);
        }
        cx.notify();
        next
    }

    // ---- playback ----------------------------------------------------------------------

    /// Plays the open collection now, or as soon as every page has arrived.
    pub fn request_play(&mut self, request: PlayRequest, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.as_mut() else {
            return;
        };
        if detail.is_loading() {
            detail.pending = Some(request);
            cx.notify();
        } else {
            self.play(request, cx);
        }
    }

    fn play(&mut self, request: PlayRequest, cx: &mut Context<Self>) {
        let Some(detail) = self.detail.as_ref() else {
            return;
        };
        let tracks: Vec<Track> = detail
            .listing
            .items()
            .iter()
            .map(|row| row.track.clone())
            .collect();
        if tracks.is_empty() {
            return;
        }
        let thumbs: Vec<(String, String)> = detail
            .listing
            .items()
            .iter()
            .filter_map(|row| Some((row.track.video_id.as_str().to_owned(), row.thumbnail.clone()?)))
            .collect();
        self.player.update(cx, |p, cx| {
            for (id, url) in &thumbs {
                p.hint_art(id, Some(url));
            }
            p.play_tracks(tracks, request.start, request.shuffle, cx)
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
            let fetch_url = url.clone();
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

/// Square-resized, BGRA, ready for GPUI.
fn sized_render_image(art: &DecodedArt, size: ArtSize) -> Arc<RenderImage> {
    let side = match size {
        ArtSize::List => LIST_ART_PX,
        ArtSize::Header => HEADER_ART_PX,
    };
    let (w, _) = art.pixels.dimensions();
    if w <= side {
        return art.render_image();
    }
    let resized = imageops::resize(&art.pixels, side, side, imageops::FilterType::Triangle);
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
    fn row_requests_keep_the_players_shuffle_setting() {
        assert_eq!(PlayRequest::row(3).shuffle, None);
        assert_eq!(PlayRequest::row(3).start, Some(3));
        assert_eq!(PlayRequest::PLAY_ALL.shuffle, Some(false));
        assert_eq!(PlayRequest::SHUFFLE.shuffle, Some(true));
    }
}
