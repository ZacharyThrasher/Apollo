//! The Crate: a scratch list of songs to remember. One key (C) or "Add to Crate" in a song's menu
//! sets a song aside at the bottom without touching the queue; the Crate page plays it, arranges
//! it (drag, or Alt+Up/Down), moves it to the queue or a playlist, or empties it.
//!
//! It is local, kept in the app's SQLite store per account (see `apollo_store::CrateRow`), never
//! written to YouTube. Reads come from memory; every change is written by one background thread
//! in the order it was made. Signing out deletes it.
//!
//! Every change is a step for Ctrl+Z (see [`CrateModel::undo`]): the list as it was before, kept
//! in memory for this session. The steps are one line in time with the queue's and the
//! playlists' (`apollo_domain::undo::newest`).
//!
//! "File and forget": with Settings > Playlists > "Take songs out of the Crate once filed" on, a
//! song the service confirms is in a playlist leaves the Crate ([`CrateModel::file`]), as one
//! step for Ctrl+Z.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_domain::undo::Side;
use apollo_provider::{ListedTrack, SearchFilter};
use apollo_services_common::{Lane, Throttle};
use apollo_store::CrateRow;
use gpui::{App, Context, Entity, EventEmitter, Global};

use crate::crate_arrange::Arrange;
use crate::crate_swap::{self, Pick, Want};
use crate::library::writes::{LibraryWrites, WritesEvent};
use crate::player::PlayerModel;
use crate::search::SearchSource;
use crate::settings::{SettingsModel, SqliteSettings, CRATE_FILE_AND_FORGET};

/// The name a Crate skim gives the queue as its source: while it is the queue's source, a skim is
/// "of the Crate" and Delete tosses the song playing, K keeps it.
pub const SKIM_SOURCE: &str = "Crate";

/// How many changes to the Crate Ctrl+Z can take back.
pub const UNDO_LIMIT: usize = 30;

/// The Crate as it was before one change.
struct Step {
    before: Vec<CrateRow>,
    /// What the change did, in words for "Undid: …".
    words: String,
    /// The queue history's depth when the change was made.
    note: i64,
    /// Where the change sits in time among the playlist edits.
    seq: u64,
}

/// "a song" or "3 songs".
fn songs(count: usize) -> String {
    match count {
        1 => "a song".to_owned(),
        n => format!("{n} songs"),
    }
}

/// Where the crate is kept. Blocking: call off the UI thread.
pub trait CrateStore: Send + Sync {
    /// The crate saved for `account_key`, top to bottom as arranged. Another account's is
    /// forgotten.
    fn load(&self, account_key: &str) -> Vec<CrateRow>;
    /// Add a song below the last one.
    fn add(&self, account_key: &str, row: &CrateRow);
    /// The whole crate as arranged now, top to bottom.
    fn reorder(&self, account_key: &str, video_ids: &[String]);
    fn remove(&self, account_key: &str, video_id: &str);
    /// Put `row` where the song `old_video_id` stands (the same song, to correct what is known
    /// of it).
    fn swap(&self, account_key: &str, old_video_id: &str, row: &CrateRow);
    fn clear(&self, account_key: &str);
    /// Delete every account's crate (sign-out).
    fn purge(&self);
}

impl CrateStore for SqliteSettings {
    fn load(&self, account_key: &str) -> Vec<CrateRow> {
        let Some(mut store) = self.open() else {
            return Vec::new();
        };
        store.crate_load(account_key).unwrap_or_else(|err| {
            eprintln!("could not read the crate: {err}");
            Vec::new()
        })
    }

    fn add(&self, account_key: &str, row: &CrateRow) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.crate_add(account_key, row) {
                eprintln!("could not save to the crate: {err}");
            }
        }
    }

    fn reorder(&self, account_key: &str, video_ids: &[String]) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.crate_reorder(account_key, video_ids) {
                eprintln!("could not save the crate's order: {err}");
            }
        }
    }

    fn remove(&self, account_key: &str, video_id: &str) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.crate_remove(account_key, video_id) {
                eprintln!("could not remove from the crate: {err}");
            }
        }
    }

    fn swap(&self, account_key: &str, old_video_id: &str, row: &CrateRow) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.crate_swap(account_key, old_video_id, row) {
                eprintln!("could not swap a crate song: {err}");
            }
        }
    }

    fn clear(&self, account_key: &str) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.crate_clear(account_key) {
                eprintln!("could not clear the crate: {err}");
            }
        }
    }

    fn purge(&self) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.crate_purge() {
                eprintln!("could not forget the crate: {err}");
            }
        }
    }
}

enum Write {
    Add(String, Box<CrateRow>),
    Order(String, Vec<String>),
    Remove(String, String),
    Swap(String, String, Box<CrateRow>),
    Clear(String),
    Purge,
}

/// A song to put in the Crate with what the list it came from knew of it.
#[derive(Clone, Debug)]
pub struct NewSong {
    pub track: Track,
    pub thumbnail: Option<String>,
    /// The "E" badge, when the list showed one.
    pub explicit: bool,
}

/// The crate as the UI sees it.
pub struct CrateModel {
    store: Option<Arc<dyn CrateStore>>,
    writer: Option<Sender<Write>>,
    /// The account the list belongs to; nothing is stored without one.
    key: Option<String>,
    /// Top to bottom, as arranged; a new song goes last.
    items: Vec<CrateRow>,
    /// Bumped whenever the list is replaced, so a load that finishes late is dropped.
    epoch: u64,
    /// What Ctrl+Z can take back, oldest first.
    history: VecDeque<Step>,
    /// Read for the queue history's depth, which steps note (see `apollo_domain::undo`).
    player: Option<Entity<PlayerModel>>,
    /// Read for "Take songs out of the Crate once filed".
    settings: Option<Entity<SettingsModel>>,
    /// Songs a picker is open for: filing them waits until it closes (see [`Self::hold`]).
    held: HashSet<String>,
    /// Songs filed while held, to leave the Crate when the picker closes.
    waiting: Vec<String>,
    /// "Hide filed": the lists leave out the songs the target playlist already holds. A view
    /// only, shared by the page and the dock; it is not kept between runs.
    hide_filed: bool,
    /// Where a swap looks for the other version of a song (see [`crate_swap`]).
    search: Option<Arc<dyn SearchSource>>,
    /// Settings > Playlists > Prefer explicit versions: a clean song added is swapped at once.
    prefer_explicit: bool,
    /// Songs waiting for their search, oldest first.
    jobs: VecDeque<Job>,
    /// The batch the owner started (a header action), if one is running.
    batch: Option<Batch>,
    /// A task is working through `jobs`.
    runner: bool,
    /// Numbers batches.
    serial: u64,
    /// How long the runner waits between one pair of searches and the next.
    pause: Duration,
    /// How long searches stop after YouTube says it is busy.
    cooldown: Duration,
    /// Searches are stopped until then (YouTube said it is busy).
    cooling_until: Option<Instant>,
    /// What the rest of the app has been told about YouTube being busy (the process's own).
    throttle: Throttle,
}

/// One song waiting to be looked up.
#[derive(Clone, Debug)]
struct Job {
    video_id: String,
    want: Want,
    /// The batch it counts for; `None` for a song swapped on its own as it was added.
    batch: Option<u64>,
    /// The crate it was queued for; a job of an older one is dropped.
    epoch: u64,
}

/// A batch the owner started. Its swaps are held until every song has been looked up, then
/// made together as one step for Ctrl+Z.
#[derive(Debug)]
struct Batch {
    id: u64,
    want: Want,
    total: usize,
    done: usize,
    /// Songs the search showed to be that version already.
    already: usize,
    /// Searches that failed (the service could not be reached).
    errors: usize,
    /// (video id it replaces, the row to put there), in the order they were found.
    found: Vec<(String, CrateRow)>,
}

enum Outcome {
    /// A version was found and will take the song's place.
    Found,
    /// The song is that version already (the crate had not known).
    Already,
    /// Nothing changed: no match, or the song left the crate.
    Unchanged,
    Failed,
    /// YouTube said it is busy.
    Throttled,
}

/// What starting a batch did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwapStart {
    /// This many songs are being looked up.
    Started(usize),
    /// None of the songs needs it (all are that version already, or none are picked).
    Nothing,
    /// Another batch is still running.
    Busy,
    /// There is no search to ask yet (the backend is not ready).
    NoSearch,
    /// YouTube said it was busy a moment ago: searches wait.
    Paused,
}

/// What the crate tells the window about swaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrateEvent {
    /// Songs were replaced in place by another version (or put back): (old id, new id).
    Swapped(Vec<(String, String)>),
    /// A batch is under way.
    Progress { want: Want, done: usize, total: usize },
    /// A batch is over.
    Finished {
        want: Want,
        swapped: usize,
        total: usize,
        already: usize,
        unreachable: bool,
        /// Stopped by the service's "too many requests".
        paused: bool,
    },
}

impl EventEmitter<CrateEvent> for CrateModel {}

impl Default for CrateModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CrateModel {
    /// An empty crate that lasts only for this run (tests, and until a store is installed).
    pub fn new() -> Self {
        Self {
            store: None,
            writer: None,
            key: None,
            items: Vec::new(),
            epoch: 0,
            history: VecDeque::new(),
            player: None,
            settings: None,
            held: HashSet::new(),
            waiting: Vec::new(),
            hide_filed: false,
            search: None,
            prefer_explicit: false,
            jobs: VecDeque::new(),
            batch: None,
            runner: false,
            serial: 0,
            pause: crate_swap::PAUSE,
            cooldown: crate_swap::COOLDOWN,
            cooling_until: None,
            throttle: Throttle::global(),
        }
    }

    /// Put the Crate in the app's line of undo steps and let it follow what is filed: `player`
    /// says where the queue's history is, `writes` says when a song is in a playlist, and
    /// `settings` whether that takes it out of the Crate.
    pub fn follow(
        &mut self,
        player: Entity<PlayerModel>,
        writes: &Entity<LibraryWrites>,
        settings: Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) {
        self.player = Some(player);
        self.settings = Some(settings);
        cx.subscribe(writes, |m, _, event, cx| match event {
            WritesEvent::PlaylistAddSettled { video_id, added: true, .. } => {
                m.filed(std::slice::from_ref(video_id), cx)
            }
            WritesEvent::PlaylistAddedMany { video_ids, .. } => m.filed(video_ids, cx),
            _ => {}
        })
        .detach();
    }

    /// Where the crate is kept between runs. An account already chosen has its crate read now.
    pub fn install(&mut self, store: Option<Arc<dyn CrateStore>>, cx: &mut Context<Self>) {
        self.writer = store.clone().map(|store| {
            let (tx, rx) = channel::<Write>();
            std::thread::Builder::new()
                .name("apollo-crate-save".into())
                .spawn(move || {
                    for write in rx {
                        match write {
                            Write::Add(key, row) => store.add(&key, &row),
                            Write::Order(key, ids) => store.reorder(&key, &ids),
                            Write::Remove(key, video) => store.remove(&key, &video),
                            Write::Swap(key, old, row) => store.swap(&key, &old, &row),
                            Write::Clear(key) => store.clear(&key),
                            Write::Purge => store.purge(),
                        }
                    }
                })
                .ok();
            tx
        });
        self.store = store;
        self.epoch += 1;
        self.load(cx);
    }

    /// The signed-in account (its opaque key) changed. Another account's crate never shows: the
    /// list empties, and the new account's own is read from the store.
    pub fn set_account(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if self.key == key {
            return;
        }
        self.key = key;
        self.epoch += 1;
        self.items.clear();
        self.history.clear();
        self.waiting.clear();
        self.reset_swaps();
        cx.notify();
        self.load(cx);
    }

    /// Read the account's crate from the store, off the UI thread.
    fn load(&mut self, cx: &mut Context<Self>) {
        let (Some(store), Some(key)) = (self.store.clone(), self.key.clone()) else {
            return;
        };
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let rows = cx
                .background_executor()
                .spawn(async move { store.load(&key) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.epoch != epoch {
                    return;
                }
                // Anything set aside while this was loading goes below what was stored.
                let added: Vec<CrateRow> = m.items.drain(..).collect();
                let stored: HashSet<String> = rows
                    .iter()
                    .map(|r| r.track.video_id.as_str().to_owned())
                    .collect();
                m.items = rows;
                m.items.extend(
                    added
                        .into_iter()
                        .filter(|r| !stored.contains(r.track.video_id.as_str())),
                );
                // Steps made while this was loading hold a list that was missing what was stored.
                m.history.clear();
                cx.notify();
            });
        })
        .detach();
    }

    /// Sign-out: the crate goes with the session, here and in the store.
    pub fn forget(&mut self, cx: &mut Context<Self>) {
        self.key = None;
        self.epoch += 1;
        self.items.clear();
        self.history.clear();
        self.waiting.clear();
        self.reset_swaps();
        if let Some(writer) = &self.writer {
            let _ = writer.send(Write::Purge);
        }
        cx.notify();
    }

    /// The account the list belongs to.
    pub fn account(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn items(&self) -> &[CrateRow] {
        &self.items
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the lists leave out songs the target playlist already holds.
    pub fn hide_filed(&self) -> bool {
        self.hide_filed
    }

    pub fn set_hide_filed(&mut self, hide: bool, cx: &mut Context<Self>) {
        if self.hide_filed != hide {
            self.hide_filed = hide;
            cx.notify();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn contains(&self, video_id: &str) -> bool {
        self.items
            .iter()
            .any(|r| r.track.video_id.as_str() == video_id)
    }

    /// The tracks in list order, for playing or queueing.
    pub fn tracks(&self) -> Vec<Track> {
        self.items.iter().map(|r| r.track.clone()).collect()
    }

    /// Set a song aside, at the bottom. One already in the crate stays where it is. Returns
    /// whether it was new.
    pub fn add(&mut self, track: Track, thumbnail: Option<String>, cx: &mut Context<Self>) -> bool {
        self.add_marked(track, thumbnail, false, cx)
    }

    /// [`add`](Self::add) for a song whose row knew whether the service marks it explicit.
    pub fn add_marked(
        &mut self,
        track: Track,
        thumbnail: Option<String>,
        explicit: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.contains(track.video_id.as_str()) {
            return false;
        }
        let before = self.items.clone();
        let video = track.video_id.as_str().to_owned();
        let row = CrateRow {
            explicit: explicit || crate::explicit_marks::is_explicit(&video),
            track,
            thumbnail,
            added_ms: now_ms(),
        };
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
        }
        self.items.push(row);
        self.record(before, "added a song to the Crate".to_owned(), cx);
        cx.notify();
        self.auto_swap(vec![video], cx);
        true
    }

    /// Set several songs aside at the bottom, in the order given, as one step for Ctrl+Z. A song
    /// the crate already holds (or that comes twice in `songs_in`) is skipped. Returns the ids
    /// that went in; nothing goes in, and nothing is recorded, when they were all held.
    pub fn add_many(
        &mut self,
        songs_in: Vec<(Track, Option<String>)>,
        cx: &mut Context<Self>,
    ) -> Vec<String> {
        let mut held: HashSet<String> = self
            .items
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect();
        let before = self.items.clone();
        let mut added = Vec::new();
        for (track, thumbnail) in songs_in {
            let video = track.video_id.as_str().to_owned();
            if !held.insert(video.clone()) {
                continue;
            }
            let row = CrateRow {
                explicit: crate::explicit_marks::is_explicit(&video),
                track,
                thumbnail,
                added_ms: now_ms(),
            };
            if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
                let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
            }
            self.items.push(row);
            added.push(video);
        }
        if added.is_empty() {
            return added;
        }
        let words = format!("added {} to the Crate", songs(added.len()));
        self.record(before, words, cx);
        cx.notify();
        // "Prefer explicit versions" (off unless chosen) treats these like any other add.
        self.auto_swap(added.clone(), cx);
        added
    }

    /// [`add_many`](Self::add_many) for songs a search or a paste found, which know whether the
    /// service marks them explicit. One step for Ctrl+Z; the caller says what happened in the tip
    /// pill (this says nothing).
    pub fn add_found(&mut self, songs_in: Vec<NewSong>, cx: &mut Context<Self>) -> Vec<String> {
        let before = self.items.clone();
        let mut added: Vec<String> = Vec::new();
        for song in songs_in {
            let video = song.track.video_id.as_str().to_owned();
            if self.contains(&video) {
                continue;
            }
            let row = CrateRow {
                explicit: song.explicit || crate::explicit_marks::is_explicit(&video),
                track: song.track,
                thumbnail: song.thumbnail,
                added_ms: now_ms(),
            };
            if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
                let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
            }
            self.items.push(row);
            added.push(video);
        }
        if added.is_empty() {
            return added;
        }
        self.record(before, format!("added {} to the Crate", songs(added.len())), cx);
        cx.notify();
        self.auto_swap(added.clone(), cx);
        added
    }

    /// Drop songs into the crate before what is at slot `slot` now (`len` or more means the
    /// bottom). A song already in the crate moves there instead of being added twice; the songs
    /// keep the order they are given in. Returns whether anything changed. One step for Ctrl+Z.
    pub fn insert_at(
        &mut self,
        songs_in: Vec<(Track, Option<String>)>,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = self.items.clone();
        let asked = songs_in.len();
        let mut added_ids: Vec<String> = Vec::new();
        let mut slot = slot.min(self.items.len());
        for (track, thumbnail) in songs_in {
            let video = track.video_id.as_str();
            if let Some(at) = self.items.iter().position(|r| r.track.video_id.as_str() == video) {
                // Already here: it moves to the drop point (the slot is counted in the list as it
                // is, so what sat above the point shifts the point up by one).
                let row = self.items.remove(at);
                if at < slot {
                    slot -= 1;
                }
                self.items.insert(slot, row);
            } else {
                let row = CrateRow {
                    explicit: crate::explicit_marks::is_explicit(video),
                    track,
                    thumbnail,
                    added_ms: now_ms(),
                };
                if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
                    let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
                }
                added_ids.push(row.track.video_id.as_str().to_owned());
                self.items.insert(slot, row);
            }
            slot += 1;
        }
        let ids = |rows: &[CrateRow]| {
            rows.iter()
                .map(|r| r.track.video_id.as_str().to_owned())
                .collect::<Vec<_>>()
        };
        if ids(&before) == ids(&self.items) {
            return false;
        }
        self.save_order();
        let words = if !added_ids.is_empty() {
            format!("added {} to the Crate", songs(added_ids.len()))
        } else {
            format!("moved {} in the Crate", songs(asked))
        };
        self.record(before, words, cx);
        cx.notify();
        // Songs that were not in the crate before are the ones added (the rest only moved).
        crate::confirm::crated(added_ids.clone(), cx);
        self.auto_swap(added_ids, cx);
        true
    }

    /// Move the song at `from` so it sits before what is at slot `slot` now (`slot` counts the
    /// list as it is, so `len` means the very end). Returns whether anything moved: a song
    /// dropped where it already is changes nothing.
    pub fn move_row(&mut self, from: usize, slot: usize, cx: &mut Context<Self>) -> bool {
        let len = self.items.len();
        if from >= len || slot > len {
            return false;
        }
        let to = if slot > from { slot - 1 } else { slot };
        if to == from {
            return false;
        }
        let before = self.items.clone();
        let row = self.items.remove(from);
        self.items.insert(to, row);
        self.save_order();
        self.record(before, "moved a song in the Crate".to_owned(), cx);
        cx.notify();
        true
    }

    /// Move a song one place up (`-1`) or down (`1`). Returns whether it moved: the top song
    /// cannot go up, nor the bottom one down.
    pub fn nudge(&mut self, video_id: &str, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(at) = self
            .items
            .iter()
            .position(|r| r.track.video_id.as_str() == video_id)
        else {
            return false;
        };
        let to = at as isize + delta;
        if to < 0 || to >= self.items.len() as isize {
            return false;
        }
        // Moving before the next song's slot is moving to just after it.
        let slot = if delta > 0 { to as usize + 1 } else { to as usize };
        self.move_row(at, slot, cx)
    }

    /// Write the arranged order, after a move.
    fn save_order(&self) {
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let ids = self
                .items
                .iter()
                .map(|r| r.track.video_id.as_str().to_owned())
                .collect();
            let _ = writer.send(Write::Order(key.clone(), ids));
        }
    }

    pub fn remove(&mut self, video_id: &str, cx: &mut Context<Self>) -> bool {
        self.remove_many(&[video_id.to_owned()], cx) > 0
    }

    /// Take several songs out as one step for Ctrl+Z. Returns how many were in the crate.
    pub fn remove_many(&mut self, video_ids: &[String], cx: &mut Context<Self>) -> usize {
        let words = |n| format!("removed {} from the Crate", songs(n));
        self.take_out(video_ids, words, cx)
    }

    fn take_out(
        &mut self,
        video_ids: &[String],
        words: impl FnOnce(usize) -> String,
        cx: &mut Context<Self>,
    ) -> usize {
        let before = self.items.clone();
        let gone: HashSet<&str> = video_ids.iter().map(String::as_str).collect();
        self.items.retain(|r| !gone.contains(r.track.video_id.as_str()));
        let count = before.len() - self.items.len();
        if count == 0 {
            return 0;
        }
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            for row in &before {
                let video = row.track.video_id.as_str();
                if gone.contains(video) {
                    let _ = writer.send(Write::Remove(key.clone(), video.to_owned()));
                }
            }
        }
        self.record(before, words(count), cx);
        cx.notify();
        count
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let before = std::mem::take(&mut self.items);
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Clear(key.clone()));
        }
        self.record(before, "cleared the Crate".to_owned(), cx);
        cx.notify();
    }

    /// Put the crate in exactly this state (a rearrangement made elsewhere, such as a swap in
    /// place) as one step for Ctrl+Z; `words` say what it did ("swapped 3 songs").
    pub fn replace_all(
        &mut self,
        rows: Vec<CrateRow>,
        words: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        let before = self.items.clone();
        if before == rows {
            return;
        }
        self.write_state(&rows);
        self.items = rows;
        self.record(before, words.into(), cx);
        cx.notify();
    }

    /// One of the Arrange menu's rewrites of the whole Crate (a sort, shuffle, reverse or spread
    /// of the artists), as one step for Ctrl+Z. Returns whether the order changed; the pill says
    /// what was done, or that the Crate was in that order already.
    pub fn arrange(&mut self, how: Arrange, cx: &mut Context<Self>) -> bool {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        self.arrange_seeded(how, seed, cx)
    }

    /// [`Self::arrange`] with the shuffle's seed given (tests).
    pub fn arrange_seeded(&mut self, how: Arrange, seed: u64, cx: &mut Context<Self>) -> bool {
        if self.items.len() < 2 {
            return false;
        }
        let rows = how.apply(&self.items, seed);
        if rows == self.items {
            crate::confirm::arranged(how.already(), false, cx);
            return false;
        }
        self.replace_all(rows, how.words(), cx);
        crate::confirm::arranged(how.done(), true, cx);
        true
    }

    /// Send songs to the top or the bottom, keeping the order they have among themselves. One
    /// step for Ctrl+Z. Returns whether anything moved: songs already there stay put.
    pub fn move_to_edge(&mut self, video_ids: &[String], top: bool, cx: &mut Context<Self>) -> bool {
        let want: HashSet<&str> = video_ids.iter().map(String::as_str).collect();
        let (moved, rest): (Vec<CrateRow>, Vec<CrateRow>) = self
            .items
            .iter()
            .cloned()
            .partition(|r| want.contains(r.track.video_id.as_str()));
        if moved.is_empty() {
            return false;
        }
        let count = moved.len();
        let rows: Vec<CrateRow> = if top {
            moved.into_iter().chain(rest).collect()
        } else {
            rest.into_iter().chain(moved).collect()
        };
        if rows == self.items {
            return false;
        }
        let end = if top { "top" } else { "bottom" };
        self.replace_all(rows, format!("moved {} to the {end} of the Crate", songs(count)), cx);
        let said = match count {
            1 => format!("Moved to {end}"),
            n => format!("Moved {n} songs to {end}"),
        };
        crate::confirm::arranged(said, true, cx);
        true
    }

    // ---- undo ----------------------------------------------------------------------------

    /// Note that a change turned `before` into the list now.
    fn record(&mut self, before: Vec<CrateRow>, words: String, cx: &App) {
        let note = self
            .player
            .as_ref()
            .map_or(0, |p| p.read(cx).queue_undo_depth());
        self.history.push_back(Step {
            before,
            words,
            note,
            seq: apollo_domain::undo::tick(),
        });
        while self.history.len() > UNDO_LIMIT {
            self.history.pop_front();
        }
    }

    /// Where the newest change sits in time, for Ctrl+Z to choose between histories.
    pub fn undo_side(&self) -> Option<Side> {
        self.history.back().map(|s| Side { note: s.note, seq: s.seq })
    }

    pub fn can_undo(&self) -> bool {
        !self.history.is_empty()
    }

    /// Ctrl+Z: take back the newest change. Returns what it was, in words for "Undid: …".
    pub fn undo(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let step = self.history.pop_back()?;
        self.write_state(&step.before);
        self.items = step.before;
        cx.notify();
        Some(step.words)
    }

    /// Make the store hold `rows`, in order, given what the list holds now.
    fn write_state(&self, rows: &[CrateRow]) {
        let (Some(writer), Some(key)) = (&self.writer, &self.key) else {
            return;
        };
        let want: HashSet<&str> = rows.iter().map(|r| r.track.video_id.as_str()).collect();
        let have: HashSet<&str> = self.items.iter().map(|r| r.track.video_id.as_str()).collect();
        for row in &self.items {
            let video = row.track.video_id.as_str();
            if !want.contains(video) {
                let _ = writer.send(Write::Remove(key.clone(), video.to_owned()));
            }
        }
        for row in rows {
            if !have.contains(row.track.video_id.as_str()) {
                let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
            }
        }
        let ids = rows
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect();
        let _ = writer.send(Write::Order(key.clone(), ids));
    }

    // ---- file and forget -----------------------------------------------------------------

    /// Songs are being filed through a picker: they stay in the crate until [`Self::release`],
    /// so a song marked for several playlists is not gone after the first.
    pub fn hold(&mut self, video_ids: &[String]) {
        self.held = video_ids.iter().cloned().collect();
    }

    /// The picker closed: songs filed while it was open leave the crate now.
    pub fn release(&mut self, cx: &mut Context<Self>) {
        self.held.clear();
        let waiting = std::mem::take(&mut self.waiting);
        self.take_out(&waiting, |n| format!("took {} out of the Crate", songs(n)), cx);
    }

    /// The service holds these songs in a playlist: with "Take songs out of the Crate once
    /// filed" on, the ones in the crate leave it as one step for Ctrl+Z.
    pub fn filed(&mut self, video_ids: &[String], cx: &mut Context<Self>) {
        let on = self
            .settings
            .as_ref()
            .is_some_and(|s| s.read(cx).get(&CRATE_FILE_AND_FORGET));
        if on {
            self.file(video_ids, cx);
        }
    }

    /// Take filed songs out of the crate (a held song waits for the picker to close). Returns
    /// how many left now.
    pub fn file(&mut self, video_ids: &[String], cx: &mut Context<Self>) -> usize {
        let (held, free): (Vec<&String>, Vec<&String>) = video_ids
            .iter()
            .filter(|v| self.contains(v))
            .partition(|v| self.held.contains(*v));
        self.waiting.extend(held.into_iter().cloned());
        let free: Vec<String> = free.into_iter().cloned().collect();
        self.take_out(&free, |n| format!("took {} out of the Crate", songs(n)), cx)
    }
}

// ---- swapping songs for another version ------------------------------------------------------

impl CrateModel {
    /// The search a swap asks (the backend is ready).
    pub fn install_search(&mut self, search: Arc<dyn SearchSource>) {
        self.search = Some(search);
    }

    /// Settings > Playlists > Prefer explicit versions.
    pub fn set_prefer_explicit(&mut self, on: bool) {
        self.prefer_explicit = on;
    }

    /// How long to wait between one pair of searches and the next, and how long searches stop
    /// after the service says it is busy.
    pub fn set_swap_pacing(&mut self, pause: Duration, cooldown: Duration) {
        self.pause = pause;
        self.cooldown = cooldown;
    }

    /// Use `throttle` in place of the process's own (tests).
    pub fn set_throttle(&mut self, throttle: Throttle) {
        self.throttle = throttle;
    }

    /// The search the Crate asks (the add box and a paste ask it too), once the backend is ready.
    pub fn search_source(&self) -> Option<Arc<dyn SearchSource>> {
        self.search.clone()
    }

    /// The process's record of YouTube being busy, as the Crate has it.
    pub fn throttle(&self) -> Throttle {
        self.throttle.clone()
    }

    /// Whether the service marks the song explicit, as far as the crate knows.
    pub fn is_explicit(&self, video_id: &str) -> bool {
        self.items
            .iter()
            .any(|r| r.track.video_id.as_str() == video_id && r.explicit)
    }

    /// Whether a batch the owner started is running.
    pub fn swapping(&self) -> bool {
        self.batch.is_some()
    }

    /// Whether searches are stopped because the service said it is busy: by a swap's own
    /// search, or by anything else of the app (the shared throttle's background lane, which
    /// leaves the first request after a ban to the user).
    fn cooling(&self) -> bool {
        self.cooling_until.is_some_and(|t| Instant::now() < t)
            || self.throttle.blocked(Lane::Background).is_some()
    }

    fn index_of(&self, video_id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|r| r.track.video_id.as_str() == video_id)
    }

    /// Look for the other version of the songs that are not `want` yet: every song of the crate,
    /// or only those of `only` (a selection). The swaps are made together once every song has
    /// been looked up, as one step for Ctrl+Z, each song keeping its place. The window hears of
    /// it through [`CrateEvent`].
    pub fn start_swap(
        &mut self,
        want: Want,
        only: Option<&[String]>,
        cx: &mut Context<Self>,
    ) -> SwapStart {
        if self.batch.is_some() {
            return SwapStart::Busy;
        }
        if self.search.is_none() {
            return SwapStart::NoSearch;
        }
        let ids = crate_swap::targets(
            self.items
                .iter()
                .map(|r| (r.track.video_id.as_str(), r.explicit)),
            want,
            only,
        );
        if ids.is_empty() {
            cx.emit(CrateEvent::Finished {
                want,
                swapped: 0,
                total: 0,
                already: 0,
                unreachable: false,
                paused: false,
            });
            return SwapStart::Nothing;
        }
        let total = ids.len();
        if self.cooling() {
            cx.emit(CrateEvent::Finished {
                want,
                swapped: 0,
                total,
                already: 0,
                unreachable: false,
                paused: true,
            });
            return SwapStart::Paused;
        }
        self.serial += 1;
        let id = self.serial;
        self.batch = Some(Batch {
            id,
            want,
            total,
            done: 0,
            already: 0,
            errors: 0,
            found: Vec::new(),
        });
        let epoch = self.epoch;
        self.jobs.extend(ids.into_iter().map(|video_id| Job {
            video_id,
            want,
            batch: Some(id),
            epoch,
        }));
        cx.emit(CrateEvent::Progress { want, done: 0, total });
        self.ensure_runner(cx);
        SwapStart::Started(total)
    }

    /// Songs just added: with Prefer explicit versions on, look for their explicit versions.
    /// Quiet, and not a step for Ctrl+Z.
    fn auto_swap(&mut self, video_ids: Vec<String>, cx: &mut Context<Self>) {
        if !self.prefer_explicit || self.search.is_none() || self.cooling() {
            return;
        }
        let epoch = self.epoch;
        for video_id in video_ids {
            if self.is_explicit(&video_id) {
                continue;
            }
            self.jobs.push_back(Job { video_id, want: Want::Explicit, batch: None, epoch });
        }
        self.ensure_runner(cx);
    }

    /// Start the task that works through the waiting songs, [`crate_swap::CONCURRENCY`] searches
    /// at a time with a pause between, if one is not running.
    fn ensure_runner(&mut self, cx: &mut Context<Self>) {
        if self.runner || self.jobs.is_empty() {
            return;
        }
        let Some(source) = self.search.clone() else {
            return;
        };
        self.runner = true;
        cx.spawn(async move |this, cx| loop {
            let Ok(taken) = this.update(cx, |m, cx| m.take_jobs(cx)) else {
                return;
            };
            if taken.is_empty() {
                let _ = this.update(cx, |m, _| m.runner = false);
                return;
            }
            let tasks: Vec<_> = taken
                .iter()
                .map(|(_, track)| {
                    let (source, query) = (source.clone(), crate_swap::query(track));
                    cx.background_executor().spawn(async move {
                        source
                            .search_filtered(&query, SearchFilter::Songs)
                            .map(|page| crate_swap::audio_only(&page.results))
                    })
                })
                .collect();
            for ((job, _), task) in taken.into_iter().zip(tasks) {
                let found = task.await;
                let _ = this.update(cx, |m, cx| m.finish_job(job, found, cx));
            }
            // A small gap before the next pair, so a long Crate does not hammer the service.
            let wait = this
                .update(cx, |m, _| if m.jobs.is_empty() { Duration::ZERO } else { m.pause })
                .unwrap_or(Duration::ZERO);
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
        })
        .detach();
    }

    /// The next songs to look up. Ones that left the crate meanwhile count as done.
    fn take_jobs(&mut self, cx: &mut Context<Self>) -> Vec<(Job, Track)> {
        // Something else of the app was told to slow down since the last pair: stop here.
        if !self.jobs.is_empty() && self.cooling() {
            self.back_off(cx);
            return Vec::new();
        }
        let mut taken = Vec::new();
        while taken.len() < crate_swap::CONCURRENCY {
            let Some(job) = self.jobs.pop_front() else {
                break;
            };
            if job.epoch != self.epoch {
                continue;
            }
            match self.index_of(&job.video_id) {
                Some(at) => taken.push((job, self.items[at].track.clone())),
                None => self.settle(&job, Outcome::Unchanged, cx),
            }
        }
        taken
    }

    /// A search came back: note the song's other version if it found one.
    fn finish_job(
        &mut self,
        job: Job,
        found: Result<Vec<ListedTrack>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if job.epoch != self.epoch {
            return;
        }
        // A batch that was stopped while this was in flight changes nothing more.
        if job.batch.is_some() && self.batch.as_ref().map(|b| b.id) != job.batch {
            return;
        }
        let outcome = match found {
            Err(LoadFailure::Throttled) => Outcome::Throttled,
            Err(_) => Outcome::Failed,
            Ok(songs) => self.apply(&job, &songs, cx),
        };
        self.settle(&job, outcome, cx);
    }

    /// Work out what the search found for the song. A song added on its own is swapped at once;
    /// a batch's is held for the end.
    fn apply(&mut self, job: &Job, songs: &[ListedTrack], cx: &mut Context<Self>) -> Outcome {
        let Some(at) = self.index_of(&job.video_id) else {
            return Outcome::Unchanged;
        };
        let old = self.items[at].clone();
        let held: Vec<String> = self
            .batch
            .iter()
            .filter(|b| Some(b.id) == job.batch)
            .flat_map(|b| b.found.iter().map(|(_, row)| row.track.video_id.as_str().to_owned()))
            .collect();
        let picked = crate_swap::pick(&old.track, job.want, songs, |id| {
            self.contains(id) || held.iter().any(|h| h == id)
        });
        match picked {
            Pick::None => Outcome::Unchanged,
            Pick::Already => {
                // The song is that version already; the crate only had not known.
                if old.explicit != job.want.explicit() {
                    let mut row = old;
                    row.explicit = job.want.explicit();
                    self.write_swap(&job.video_id, &row);
                    self.items[at] = row;
                    cx.notify();
                }
                Outcome::Already
            }
            Pick::Swap(found_at) => {
                let row = swapped_row(&old, &songs[found_at], job.want);
                match job.batch {
                    Some(id) => {
                        if let Some(batch) = self.batch.as_mut().filter(|b| b.id == id) {
                            batch.found.push((job.video_id.clone(), row));
                        }
                    }
                    None => {
                        let new_id = row.track.video_id.as_str().to_owned();
                        self.write_swap(&job.video_id, &row);
                        self.items[at] = row;
                        cx.emit(CrateEvent::Swapped(vec![(job.video_id.clone(), new_id)]));
                        cx.notify();
                    }
                }
                Outcome::Found
            }
        }
    }

    fn write_swap(&self, old_video_id: &str, row: &CrateRow) {
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Swap(key.clone(), old_video_id.to_owned(), Box::new(row.clone())));
        }
    }

    /// One song of a job is settled; the batch it counts for moves on and may be over.
    fn settle(&mut self, job: &Job, outcome: Outcome, cx: &mut Context<Self>) {
        if matches!(outcome, Outcome::Throttled) {
            self.back_off(cx);
            return;
        }
        let Some(batch) = self.batch.as_mut().filter(|b| Some(b.id) == job.batch) else {
            return;
        };
        batch.done += 1;
        match outcome {
            Outcome::Already => batch.already += 1,
            Outcome::Failed => batch.errors += 1,
            Outcome::Found | Outcome::Unchanged | Outcome::Throttled => {}
        }
        let (want, done, total) = (batch.want, batch.done, batch.total);
        if done < total {
            cx.emit(CrateEvent::Progress { want, done, total });
            return;
        }
        self.finish_batch(false, cx);
    }

    /// The service said "too many requests": every search stops for a while, and a batch
    /// ends there with what it had found.
    fn back_off(&mut self, cx: &mut Context<Self>) {
        self.cooling_until = Some(Instant::now() + self.cooldown);
        self.jobs.clear();
        self.finish_batch(true, cx);
    }

    /// The batch is over (every song looked up, or stopped): put what it found in the crate, in
    /// place, as one step for Ctrl+Z, and say how it went.
    fn finish_batch(&mut self, paused: bool, cx: &mut Context<Self>) {
        let Some(batch) = self.batch.take() else {
            return;
        };
        let mut rows = self.items.clone();
        let mut made = Vec::new();
        for (old_id, row) in batch.found {
            let Some(at) = rows.iter().position(|r| r.track.video_id.as_str() == old_id) else {
                continue; // left the crate meanwhile
            };
            let new_id = row.track.video_id.as_str();
            // Another song of the crate, or an earlier swap, is that version already.
            if new_id != old_id && rows.iter().any(|r| r.track.video_id.as_str() == new_id) {
                continue;
            }
            made.push((old_id, new_id.to_owned()));
            rows[at] = row;
        }
        let swapped = made.len();
        if swapped > 0 {
            let words = match swapped {
                1 => format!("swapped a song to its {} version", batch.want.word()),
                n => format!("swapped {n} songs to {} versions", batch.want.word()),
            };
            self.replace_all(rows, words, cx);
            cx.emit(CrateEvent::Swapped(made));
        }
        cx.emit(CrateEvent::Finished {
            want: batch.want,
            swapped,
            total: batch.total,
            already: batch.already,
            unreachable: !paused && batch.errors == batch.total,
            paused,
        });
    }

    /// A new crate (another account, sign-out): nothing waits.
    fn reset_swaps(&mut self) {
        self.jobs.clear();
        self.batch = None;
    }
}

/// The row that takes a song's place: the version found, keeping when the song was set aside.
fn swapped_row(old: &CrateRow, found: &ListedTrack, want: Want) -> CrateRow {
    let mut track = found.track.clone();
    if track.artists.is_empty() {
        track.artists = old.track.artists.clone();
    }
    CrateRow {
        track,
        thumbnail: found.thumbnail.clone().or_else(|| old.thumbnail.clone()),
        explicit: want.explicit(),
        added_ms: old.added_ms,
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// The shell's crate, reachable from any row (like the track menu).
#[derive(Clone)]
pub struct CrateHandle(pub Entity<CrateModel>);

impl Global for CrateHandle {}

/// The shell's crate, where there is a shell.
pub fn handle(cx: &App) -> Option<Entity<CrateModel>> {
    cx.try_global::<CrateHandle>().map(|h| h.0.clone())
}

/// Delete while the Crate is being skimmed: the song playing leaves the Crate (one step for
/// Ctrl+Z) and the skim goes on to the next. Whether it was a Crate skim (nothing happens
/// otherwise).
pub fn skim_toss(player: &Entity<PlayerModel>, cx: &mut App) -> bool {
    if !player.read(cx).skimming_from(SKIM_SOURCE) {
        return false;
    }
    let Some(video) = player.read(cx).current_video() else {
        return false;
    };
    if let Some(model) = handle(cx) {
        model.update(cx, |m, cx| m.remove(&video, cx));
    }
    player.update(cx, |p, cx| {
        p.skim_forget(&video);
        p.skim_pass(cx);
    });
    true
}

/// Set `track` aside. `thumbnail` is the cover the row knew; the player's own artwork URL stands
/// in when it did not. Does nothing where there is no shell.
pub fn add_track(
    player: &Entity<PlayerModel>,
    track: Track,
    thumbnail: Option<String>,
    cx: &mut App,
) {
    add_tracks(player, vec![(track, thumbnail)], cx);
}

/// [`add_track`] for several songs at once (a picked set): one confirmation says how many went
/// in, or that they were all in already.
pub fn add_tracks(
    player: &Entity<PlayerModel>,
    songs: Vec<(Track, Option<String>)>,
    cx: &mut App,
) {
    let songs = songs
        .into_iter()
        .map(|(track, thumbnail)| {
            let thumbnail = thumbnail.or_else(|| player.read(cx).art_url(track.video_id.as_str()));
            (track, thumbnail)
        })
        .collect();
    add_rows(songs, false, cx);
}

/// Set a song a card showed aside, with the cover the card knew (no player to ask for another).
pub fn add_card_song(track: Track, thumbnail: Option<String>, cx: &mut App) {
    add_rows(vec![(track, thumbnail)], false, cx);
}

/// What an add of many songs did, for the confirmation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Added {
    /// Songs that went in.
    pub new: usize,
    /// Songs the Crate held already (or the list named twice).
    pub held: usize,
}

/// Set the songs aside as one step for Ctrl+Z and say what happened: "Added 24 songs to Crate",
/// "Added 18 · 6 already in Crate", or "Already in Crate". `partial` says the list is still
/// loading, which the confirmation adds. Does nothing where there is no shell.
pub fn add_rows(songs: Vec<(Track, Option<String>)>, partial: bool, cx: &mut App) -> Option<Added> {
    let model = handle(cx)?;
    let asked = songs.len();
    let added = model.update(cx, |m, cx| m.add_many(songs, cx));
    let result = Added {
        new: added.len(),
        held: asked - added.len(),
    };
    crate::confirm::crated_many(added, result.held, partial, cx);
    Some(result)
}

/// Swap the Crate's songs for their `want` version: every song, or only `only`. The window's tip
/// pill follows along (see [`CrateEvent`]). Does nothing where there is no shell.
pub fn swap(want: Want, only: Option<Vec<String>>, cx: &mut App) -> Option<SwapStart> {
    let model = handle(cx)?;
    Some(model.update(cx, |m, cx| m.start_swap(want, only.as_deref(), cx)))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gpui::{AppContext, TestAppContext};

    use super::*;

    /// A store that keeps everything in memory.
    #[derive(Default)]
    struct Memory {
        rows: Mutex<Vec<(String, CrateRow)>>,
    }

    impl CrateStore for Memory {
        fn load(&self, key: &str) -> Vec<CrateRow> {
            let mut rows = self.rows.lock().unwrap();
            rows.retain(|(k, _)| k == key);
            rows.iter().map(|(_, r)| r.clone()).collect()
        }
        fn add(&self, key: &str, row: &CrateRow) {
            self.rows.lock().unwrap().push((key.into(), row.clone()));
        }
        fn reorder(&self, key: &str, video_ids: &[String]) {
            let mut rows = self.rows.lock().unwrap();
            let rank = |r: &CrateRow| {
                video_ids
                    .iter()
                    .position(|id| id == r.track.video_id.as_str())
                    .unwrap_or(usize::MAX)
            };
            // Stable: songs the order forgot keep their relative place.
            rows.sort_by_key(|(k, r)| if k == key { rank(r) } else { 0 });
        }
        fn remove(&self, key: &str, video_id: &str) {
            self.rows
                .lock()
                .unwrap()
                .retain(|(k, r)| !(k == key && r.track.video_id.as_str() == video_id));
        }
        fn swap(&self, key: &str, old_video_id: &str, row: &CrateRow) {
            let mut rows = self.rows.lock().unwrap();
            if let Some((_, r)) = rows
                .iter_mut()
                .find(|(k, r)| k == key && r.track.video_id.as_str() == old_video_id)
            {
                *r = row.clone();
            }
        }
        fn clear(&self, key: &str) {
            self.rows.lock().unwrap().retain(|(k, _)| k != key);
        }
        fn purge(&self) {
            self.rows.lock().unwrap().clear();
        }
    }

    fn track(id: &str) -> Track {
        Track::new(id, id)
    }

    fn ids(model: &CrateModel) -> Vec<String> {
        model
            .items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    }

    #[gpui::test]
    fn add_many_goes_in_below_in_order_skips_what_is_held_and_is_one_step(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.add(track("a"), None, cx);
            let before = m.history.len();
            let added = m.add_many(
                vec![
                    (track("b"), None),
                    (track("a"), None),
                    (track("c"), Some("art".into())),
                    (track("b"), None),
                ],
                cx,
            );
            assert_eq!(added, ["b", "c"], "held and repeated songs are skipped");
            assert_eq!(ids(m), ["a", "b", "c"]);
            assert_eq!(m.history.len(), before + 1, "one step for the whole add");
            assert_eq!(m.undo(cx).as_deref(), Some("added 2 songs to the Crate"));
            assert_eq!(ids(m), ["a"]);
        });
    }

    #[gpui::test]
    fn add_many_of_only_held_songs_records_nothing(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.add(track("a"), None, cx);
            let before = m.history.len();
            assert!(m.add_many(vec![(track("a"), None)], cx).is_empty());
            assert!(m.add_many(Vec::new(), cx).is_empty());
            assert_eq!(m.history.len(), before);
        });
    }

    #[gpui::test]
    fn songs_are_added_at_the_bottom_once_each(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            assert!(m.add(track("a"), None, cx));
            assert!(m.add(track("b"), None, cx));
            assert!(!m.add(track("a"), None, cx), "already there");
            assert_eq!(ids(m), ["a", "b"]);
            assert!(m.contains("a") && !m.contains("z"));
            assert!(m.remove("b", cx));
            assert!(!m.remove("b", cx));
            assert_eq!(ids(m), ["a"]);
            m.clear(cx);
            assert!(m.is_empty());
        });
    }

    #[gpui::test]
    fn the_crate_is_written_and_read_back_per_account(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            m.add(track("a"), Some("https://example.invalid/a.jpg".into()), cx);
            m.add(track("b"), None, cx);
        });
        cx.run_until_parked();
        // The writer thread is separate from the test executor: wait for it.
        for _ in 0..200 {
            if store.rows.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(store.rows.lock().unwrap().len(), 2);

        // A fresh model for the same account reads it back in the same order.
        let again = cx.new(|_| CrateModel::new());
        again.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
        });
        cx.run_until_parked();
        again.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a", "b"]);
            assert_eq!(
                m.items()[0].thumbnail.as_deref(),
                Some("https://example.invalid/a.jpg")
            );
        });

        // Another account sees nothing of it.
        again.update(cx, |m, cx| m.set_account(Some("other".into()), cx));
        cx.run_until_parked();
        again.read_with(cx, |m, _| assert!(m.is_empty()));
    }

    #[gpui::test]
    fn a_song_moves_to_a_slot_or_one_place_and_the_order_is_kept(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            for id in ["a", "b", "c", "d"] {
                m.add(track(id), None, cx);
            }
            // Dropped before slot 0: it goes to the top.
            assert!(m.move_row(2, 0, cx));
            assert_eq!(ids(m), ["c", "a", "b", "d"]);
            // Dropped on the end slot: it goes to the bottom.
            assert!(m.move_row(0, 4, cx));
            assert_eq!(ids(m), ["a", "b", "d", "c"]);
            // Dropped on itself, or just after itself: nothing moves.
            assert!(!m.move_row(1, 1, cx));
            assert!(!m.move_row(1, 2, cx));
            assert!(!m.move_row(9, 0, cx) && !m.move_row(0, 9, cx));
            assert_eq!(ids(m), ["a", "b", "d", "c"]);
            // One place at a time; the ends stay put.
            assert!(m.nudge("d", -1, cx));
            assert_eq!(ids(m), ["a", "d", "b", "c"]);
            assert!(m.nudge("d", 1, cx));
            assert_eq!(ids(m), ["a", "b", "d", "c"]);
            assert!(!m.nudge("a", -1, cx));
            assert!(!m.nudge("c", 1, cx));
            assert!(!m.nudge("zzz", 1, cx));
            m.add(track("e"), None, cx);
            assert_eq!(ids(m), ["a", "b", "d", "c", "e"], "a new song goes below");
        });
        cx.run_until_parked();
        for _ in 0..200 {
            let order: Vec<String> = store
                .rows
                .lock()
                .unwrap()
                .iter()
                .map(|(_, r)| r.track.video_id.as_str().to_owned())
                .collect();
            if order == ["a", "b", "d", "c", "e"] {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        // A fresh model reads the arranged order back.
        let again = cx.new(|_| CrateModel::new());
        again.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
        });
        cx.run_until_parked();
        again.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b", "d", "c", "e"]));
    }

    #[gpui::test]
    fn dropped_songs_land_at_the_drop_point_and_known_ones_move_there(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            for id in ["a", "b", "c"] {
                m.add(track(id), None, cx);
            }
            // A new song before slot 1.
            assert!(m.insert_at(vec![(track("x"), None)], 1, cx));
            assert_eq!(ids(m), ["a", "x", "b", "c"]);
            // Several at once keep their order; a slot past the end is the bottom.
            assert!(m.insert_at(vec![(track("y"), None), (track("z"), None)], 99, cx));
            assert_eq!(ids(m), ["a", "x", "b", "c", "y", "z"]);
            // A song already here moves instead of doubling, from above or from below the point.
            assert!(m.insert_at(vec![(track("a"), None)], 3, cx));
            assert_eq!(ids(m), ["x", "b", "a", "c", "y", "z"]);
            assert!(m.insert_at(vec![(track("z"), None)], 0, cx));
            assert_eq!(ids(m), ["z", "x", "b", "a", "c", "y"]);
            // Dropping a song where it already is changes nothing.
            assert!(!m.insert_at(vec![(track("x"), None)], 1, cx));
            assert!(!m.insert_at(vec![(track("x"), None)], 2, cx));
            assert_eq!(ids(m), ["z", "x", "b", "a", "c", "y"]);
        });
    }

    #[gpui::test]
    fn an_arranged_order_and_a_move_to_an_end_are_what_is_stored_and_each_is_one_step(
        cx: &mut TestAppContext,
    ) {
        use crate::crate_arrange::{Arrange, SortKey};
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            for id in ["c", "a", "d", "b"] {
                m.add(track(id), None, cx);
            }
            assert!(m.arrange_seeded(Arrange::Sort(SortKey::Title), 1, cx));
            assert_eq!(ids(m), ["a", "b", "c", "d"]);
            assert!(!m.arrange_seeded(Arrange::Sort(SortKey::Title), 1, cx), "already sorted");
            assert!(m.arrange_seeded(Arrange::Reverse, 1, cx));
            assert_eq!(ids(m), ["d", "c", "b", "a"]);
            // Two songs to the top keep their order among themselves; ones already there stay.
            assert!(m.move_to_edge(&["a".into(), "c".into()], true, cx));
            assert_eq!(ids(m), ["c", "a", "d", "b"]);
            assert!(!m.move_to_edge(&["c".into()], true, cx), "already on top");
            assert!(m.move_to_edge(&["c".into()], false, cx));
            assert_eq!(ids(m), ["a", "d", "b", "c"]);
            assert!(!m.move_to_edge(&["nope".into()], true, cx));
        });
        let stored = |store: &Memory| -> Vec<String> {
            store.rows.lock().unwrap().iter().map(|(_, r)| r.track.video_id.as_str().to_owned()).collect()
        };
        for _ in 0..400 {
            if stored(&store) == ["a", "d", "b", "c"] {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(stored(&store), ["a", "d", "b", "c"]);
        // Each was one step, newest first.
        model.update(cx, |m, cx| {
            assert_eq!(m.undo(cx).as_deref(), Some("moved a song to the bottom of the Crate"));
            assert_eq!(m.undo(cx).as_deref(), Some("moved 2 songs to the top of the Crate"));
            assert_eq!(m.undo(cx).as_deref(), Some("reversed the Crate"));
            assert_eq!(m.undo(cx).as_deref(), Some("sorted the Crate by title"));
            assert_eq!(ids(m), ["c", "a", "d", "b"]);
        });
    }

    #[gpui::test]
    fn signing_out_empties_it_and_deletes_the_stored_copy(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            m.add(track("a"), None, cx);
            m.forget(cx);
            assert!(m.is_empty());
        });
        for _ in 0..200 {
            if store.rows.lock().unwrap().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(store.rows.lock().unwrap().is_empty());
    }

    // ---- swapping songs for another version ----------------------------------------------

    use std::collections::HashMap;

    use apollo_domain::model::ArtistRef;
    use apollo_provider::{SearchPage, SearchResults};

    fn song(id: &str, title: &str, secs: u64) -> Track {
        Track {
            video_id: apollo_domain::model::VideoId::new(id),
            title: title.into(),
            artists: vec![ArtistRef { name: "Artist".into(), id: None }],
            album: None,
            duration: Some(Duration::from_secs(secs)),
        }
    }

    fn found(id: &str, title: &str, secs: u64, explicit: bool) -> ListedTrack {
        ListedTrack {
            track: song(id, title, secs),
            occurrence: None,
            thumbnail: Some(format!("https://example.invalid/{id}.jpg")),
            explicit,
            liked: None,
        }
    }

    /// Answers a songs search by its query (title and artist), and counts the questions.
    #[derive(Default)]
    struct Script {
        answers: HashMap<String, Vec<ListedTrack>>,
        fail: bool,
        /// A query the service answers with "too many requests".
        throttle: Option<String>,
        asked: Mutex<Vec<String>>,
    }

    impl SearchSource for Script {
        fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
            Err(LoadFailure::Other)
        }

        fn search_filtered(
            &self,
            query: &str,
            filter: SearchFilter,
        ) -> Result<SearchPage, LoadFailure> {
            assert_eq!(filter, SearchFilter::Songs, "a swap asks for songs only");
            self.asked.lock().unwrap().push(query.to_owned());
            if self.throttle.as_deref() == Some(query) {
                return Err(LoadFailure::Throttled);
            }
            if self.fail {
                return Err(LoadFailure::Network);
            }
            let songs = self.answers.get(query).cloned().unwrap_or_default();
            Ok(SearchPage {
                results: SearchResults { songs, ..SearchResults::default() },
                continuation: None,
            })
        }
    }

    /// What the crate said while a test ran.
    fn listen(model: &Entity<CrateModel>, cx: &mut TestAppContext) -> Arc<Mutex<Vec<CrateEvent>>> {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let sink = heard.clone();
        cx.update(|cx| {
            cx.subscribe(model, move |_, event: &CrateEvent, _| {
                sink.lock().unwrap().push(event.clone())
            })
            .detach()
        });
        heard
    }

    /// A script that knows the explicit versions of "a" (a2) and "b" (b2), and none for "c".
    fn script_for_abc() -> Script {
        let mut script = Script::default();
        for (clean, explicit) in [("a", "a2"), ("b", "b2")] {
            script.answers.insert(
                format!("Song {clean} Artist"),
                vec![
                    found(clean, &format!("Song {clean}"), 200, false),
                    found(explicit, &format!("Song {clean}"), 201, true),
                ],
            );
        }
        script
            .answers
            .insert("Song c Artist".into(), vec![found("c", "Song c", 200, false)]);
        script
    }

    /// A crate of clean songs a, b, c with no wait between searches.
    fn swap_setup(
        cx: &mut TestAppContext,
        script: Script,
    ) -> (Entity<CrateModel>, Arc<Script>, Arc<Mutex<Vec<CrateEvent>>>) {
        let script = Arc::new(script);
        let model = cx.new(|_| CrateModel::new());
        let heard = listen(&model, cx);
        model.update(cx, |m, cx| {
            m.install_search(script.clone());
            m.set_swap_pacing(Duration::ZERO, Duration::from_secs(60));
            for id in ["a", "b", "c"] {
                m.add(song(id, &format!("Song {id}"), 200), Some(format!("art-{id}")), cx);
            }
        });
        (model, script, heard)
    }

    fn finished(
        swapped: usize,
        total: usize,
        already: usize,
        unreachable: bool,
        paused: bool,
    ) -> CrateEvent {
        CrateEvent::Finished {
            want: Want::Explicit,
            swapped,
            total,
            already,
            unreachable,
            paused,
        }
    }

    #[gpui::test]
    fn a_batch_swaps_in_place_and_leaves_songs_with_no_match(cx: &mut TestAppContext) {
        let (model, script, heard) = swap_setup(cx, script_for_abc());
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, cx));
        assert_eq!(start, SwapStart::Started(3));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a2", "b2", "c"], "same places");
            assert!(m.items()[0].explicit && m.items()[1].explicit && !m.items()[2].explicit);
            assert_eq!(
                m.items()[0].thumbnail.as_deref(),
                Some("https://example.invalid/a2.jpg"),
                "the explicit version's own cover"
            );
            assert!(!m.swapping());
        });
        assert_eq!(script.asked.lock().unwrap().len(), 3, "one search per song");
        let heard = heard.lock().unwrap();
        let progress: Vec<(usize, usize)> = heard
            .iter()
            .filter_map(|e| match e {
                CrateEvent::Progress { done, total, .. } => Some((*done, *total)),
                _ => None,
            })
            .collect();
        assert_eq!(progress, [(0, 3), (1, 3), (2, 3)], "the pill counts up to the last song");
        assert_eq!(heard.last(), Some(&finished(2, 3, 0, false, false)));
        assert!(heard.contains(&CrateEvent::Swapped(vec![
            ("a".into(), "a2".into()),
            ("b".into(), "b2".into())
        ])));
    }

    #[gpui::test]
    fn only_the_picked_songs_are_looked_up(cx: &mut TestAppContext) {
        let (model, script, _) = swap_setup(cx, script_for_abc());
        let picked = ["b".to_owned()];
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, Some(&picked), cx));
        assert_eq!(start, SwapStart::Started(1));
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b2", "c"]));
        assert_eq!(*script.asked.lock().unwrap(), ["Song b Artist"]);
    }

    #[gpui::test]
    fn one_undo_puts_the_whole_batch_back_where_it_was(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let (model, _, _) = swap_setup(cx, script_for_abc());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
        });
        cx.run_until_parked();
        // install() starts a fresh crate for the account; fill it again.
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script_for_abc()));
            for id in ["a", "b", "c"] {
                m.add(song(id, &format!("Song {id}"), 200), Some(format!("art-{id}")), cx);
            }
            m.start_swap(Want::Explicit, None, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a2", "b2", "c"]));

        // The batch is the newest step; Ctrl+Z takes back all of it in one go.
        let words = model.update(cx, |m, cx| m.undo(cx));
        assert_eq!(words.as_deref(), Some("swapped 2 songs to explicit versions"));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a", "b", "c"], "one step undid both");
            assert!(m.items().iter().all(|r| !r.explicit));
            assert_eq!(m.items()[0].thumbnail.as_deref(), Some("art-a"), "the old row, whole");
        });
        let step_before = model.update(cx, |m, cx| m.undo(cx));
        assert_eq!(
            step_before.as_deref(),
            Some("added a song to the Crate"),
            "the next step is the add before the batch"
        );

        // The store followed both ways: the same songs, in the same order.
        for _ in 0..200 {
            let stored: Vec<String> = store
                .rows
                .lock()
                .unwrap()
                .iter()
                .map(|(_, r)| r.track.video_id.as_str().to_owned())
                .collect();
            if stored == ["a", "b"] {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let rows = store.rows.lock().unwrap();
        let stored: Vec<&str> = rows.iter().map(|(_, r)| r.track.video_id.as_str()).collect();
        assert_eq!(stored, ["a", "b"]);
    }

    #[gpui::test]
    fn a_song_that_left_the_crate_during_the_batch_is_not_brought_back(cx: &mut TestAppContext) {
        let (model, _, heard) = swap_setup(cx, script_for_abc());
        model.update(cx, |m, cx| {
            m.start_swap(Want::Explicit, None, cx);
            m.remove("a", cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["b2", "c"]));
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(1, 3, 0, false, false)));
    }

    #[gpui::test]
    fn the_reverse_swap_finds_clean_versions(cx: &mut TestAppContext) {
        let mut script = Script::default();
        script.answers.insert(
            "Song a Artist".into(),
            vec![found("a", "Song a", 200, true), found("a-clean", "Song a (Clean)", 199, false)],
        );
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script));
            m.add_marked(song("a", "Song a", 200), None, true, cx);
        });
        let start = model.update(cx, |m, cx| m.start_swap(Want::Clean, None, cx));
        assert_eq!(start, SwapStart::Started(1));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a-clean"]);
            assert!(!m.items()[0].explicit);
        });
    }

    #[gpui::test]
    fn nothing_to_do_and_a_busy_batch_are_said_not_started(cx: &mut TestAppContext) {
        let (model, _, heard) = swap_setup(cx, script_for_abc());
        // No source yet is its own answer.
        let bare = cx.new(|_| CrateModel::new());
        assert_eq!(
            bare.update(cx, |m, cx| m.start_swap(Want::Explicit, None, cx)),
            SwapStart::NoSearch
        );
        let none: [String; 0] = [];
        assert_eq!(
            model.update(cx, |m, cx| m.start_swap(Want::Explicit, Some(&none), cx)),
            SwapStart::Nothing
        );
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(0, 0, 0, false, false)));
        model.update(cx, |m, cx| {
            assert_eq!(m.start_swap(Want::Explicit, None, cx), SwapStart::Started(3));
            assert!(m.swapping());
            assert_eq!(m.start_swap(Want::Clean, None, cx), SwapStart::Busy);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_service_that_cannot_be_reached_changes_nothing_and_says_so(cx: &mut TestAppContext) {
        let (model, _, heard) = swap_setup(cx, Script { fail: true, ..Script::default() });
        model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, cx));
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b", "c"]));
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(0, 3, 0, true, false)));
        assert_eq!(
            model.update(cx, |m, cx| m.undo(cx)).as_deref(),
            Some("added a song to the Crate"),
            "no swap step was made"
        );
    }

    #[gpui::test]
    fn the_first_too_many_requests_stops_the_batch_and_keeps_what_was_found(
        cx: &mut TestAppContext,
    ) {
        let mut script = script_for_abc();
        script.throttle = Some("Song c Artist".into());
        let (model, script, heard) = swap_setup(cx, script);
        model.update(cx, |m, cx| {
            m.add(song("d", "Song d", 200), None, cx);
            m.add(song("e", "Song e", 200), None, cx);
            m.start_swap(Want::Explicit, None, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a2", "b2", "c", "d", "e"], "a and b were found before c");
            assert!(!m.swapping());
        });
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(2, 5, 0, false, true)));
        // Song e was never searched for: the batch stopped at c (d was already in flight).
        assert!(!script.asked.lock().unwrap().iter().any(|q| q == "Song e Artist"));

        // The found swaps are one step; Ctrl+Z puts them back together.
        assert_eq!(
            model.update(cx, |m, cx| m.undo(cx)).as_deref(),
            Some("swapped 2 songs to explicit versions")
        );
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b", "c", "d", "e"]));

        // While YouTube is busy another batch says so at once and asks nothing.
        let asked = script.asked.lock().unwrap().len();
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, cx));
        assert_eq!(start, SwapStart::Paused);
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(0, 5, 0, false, true)));
        cx.run_until_parked();
        assert_eq!(script.asked.lock().unwrap().len(), asked);

        // A cooldown that has run out lets searches go on.
        model.update(cx, |m, _| m.cooling_until = Some(Instant::now() - Duration::from_secs(1)));
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, cx));
        assert!(matches!(start, SwapStart::Started(_)));
    }

    #[gpui::test]
    fn a_ban_the_rest_of_the_app_ran_into_stops_a_batch_before_it_asks(cx: &mut TestAppContext) {
        let (model, script, heard) = swap_setup(cx, script_for_abc());
        let throttle = Throttle::new();
        model.update(cx, |m, _| m.set_throttle(throttle.clone()));
        throttle.report_throttled(None);
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, cx));
        assert_eq!(start, SwapStart::Paused);
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(0, 3, 0, false, true)));
        cx.run_until_parked();
        assert!(script.asked.lock().unwrap().is_empty());

        // Banned just after a batch started: it stops before asking anything.
        let throttle = Throttle::new();
        let (model, script, heard) = swap_setup(cx, script_for_abc());
        model.update(cx, |m, cx| {
            m.set_throttle(throttle.clone());
            m.start_swap(Want::Explicit, None, cx);
            throttle.report_throttled(None);
        });
        cx.run_until_parked();
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(0, 3, 0, false, true)));
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b", "c"]));
        assert!(script.asked.lock().unwrap().is_empty());
    }

    #[gpui::test]
    fn searches_wait_between_pairs(cx: &mut TestAppContext) {
        let (model, script, _) = swap_setup(cx, script_for_abc());
        model.update(cx, |m, cx| {
            m.set_swap_pacing(Duration::from_millis(400), Duration::from_secs(60));
            m.start_swap(Want::Explicit, None, cx);
        });
        cx.run_until_parked();
        assert_eq!(script.asked.lock().unwrap().len(), 2, "the first pair, then the wait");
        cx.executor().advance_clock(Duration::from_millis(401));
        cx.run_until_parked();
        assert_eq!(script.asked.lock().unwrap().len(), 3, "the last song after the wait");
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a2", "b2", "c"]));
    }

    #[gpui::test]
    fn a_song_that_is_explicit_already_is_found_out_not_swapped(cx: &mut TestAppContext) {
        let mut script = Script::default();
        // The song itself comes back with the E: the crate had not known it was explicit.
        script.answers.insert("Song a Artist".into(), vec![found("a", "Song a", 200, true)]);
        let model = cx.new(|_| CrateModel::new());
        let heard = listen(&model, cx);
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script));
            m.add(song("a", "Song a", 200), None, cx);
            assert!(!m.is_explicit("a"));
            m.start_swap(Want::Explicit, None, cx);
        });
        cx.run_until_parked();
        assert_eq!(heard.lock().unwrap().last(), Some(&finished(0, 1, 1, false, false)));
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a"]);
            assert!(m.is_explicit("a"), "now it is known");
        });
    }

    #[gpui::test]
    fn a_swap_never_puts_a_song_in_the_crate_twice(cx: &mut TestAppContext) {
        let mut script = Script::default();
        script.answers.insert("Song a Artist".into(), vec![found("b", "Song a", 200, true)]);
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script));
            m.add(song("a", "Song a", 200), None, cx);
            // The explicit version is already in the crate, as its own row.
            m.add_marked(song("b", "Song a", 200), None, true, cx);
            m.start_swap(Want::Explicit, None, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b"]));
    }

    #[gpui::test]
    fn two_songs_with_one_explicit_version_swap_only_once(cx: &mut TestAppContext) {
        let mut script = Script::default();
        for id in ["a", "b"] {
            script
                .answers
                .insert(format!("Song {id} Artist"), vec![found("shared", &format!("Song {id}"), 200, true)]);
        }
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script));
            m.set_swap_pacing(Duration::ZERO, Duration::from_secs(60));
            m.add(song("a", "Song a", 200), None, cx);
            m.add(song("b", "Song b", 200), None, cx);
            m.start_swap(Want::Explicit, None, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["shared", "b"]));
    }

    #[gpui::test]
    fn prefer_explicit_swaps_a_clean_song_as_it_is_added_and_quietly(cx: &mut TestAppContext) {
        let mut script = Script::default();
        script.answers.insert("Song a Artist".into(), vec![found("a2", "Song a", 200, true)]);
        let script = Arc::new(script);
        let model = cx.new(|_| CrateModel::new());
        let heard = listen(&model, cx);
        model.update(cx, |m, cx| {
            m.install_search(script.clone());
            m.set_swap_pacing(Duration::ZERO, Duration::from_secs(60));
            m.add(song("a", "Song a", 200), None, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a"], "off by default"));
        assert!(script.asked.lock().unwrap().is_empty());

        model.update(cx, |m, cx| {
            m.set_prefer_explicit(true);
            m.add(song("x", "Song x", 100), None, cx);
            // One added already marked explicit is left alone, and so is one added before.
            m.add_marked(song("e", "Song e", 100), None, true, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "x", "e"]));
        assert_eq!(*script.asked.lock().unwrap(), ["Song x Artist"]);

        let model2 = cx.new(|_| CrateModel::new());
        model2.update(cx, |m, cx| {
            m.install_search(script.clone());
            m.set_prefer_explicit(true);
            m.add(song("a", "Song a", 200), None, cx);
        });
        cx.run_until_parked();
        model2.read_with(cx, |m, _| assert_eq!(ids(m), ["a2"], "swapped where it was added"));
        assert!(
            !heard
                .lock()
                .unwrap()
                .iter()
                .any(|e| matches!(e, CrateEvent::Progress { .. } | CrateEvent::Finished { .. })),
            "no pill for a song swapped on its own"
        );
    }

    #[gpui::test]
    fn prefer_explicit_asks_nothing_while_youtube_is_busy(cx: &mut TestAppContext) {
        let mut script = script_for_abc();
        script.throttle = Some("Song a Artist".into());
        let script = Arc::new(script);
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install_search(script.clone());
            m.set_swap_pacing(Duration::ZERO, Duration::from_secs(60));
            m.set_prefer_explicit(true);
            m.add(song("a", "Song a", 200), None, cx);
        });
        cx.run_until_parked();
        assert_eq!(script.asked.lock().unwrap().len(), 1);
        model.update(cx, |m, cx| {
            m.add(song("b", "Song b", 200), None, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b"], "b is left as it is"));
        assert_eq!(script.asked.lock().unwrap().len(), 1, "no more searches in the cooldown");
    }

    #[gpui::test]
    fn at_most_two_searches_are_taken_at_a_time(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            for id in ["a", "b", "c", "d", "e"] {
                m.add(song(id, id, 100), None, cx);
                m.jobs.push_back(Job {
                    video_id: id.into(),
                    want: Want::Explicit,
                    batch: None,
                    epoch: m.epoch,
                });
            }
            let mut sizes = Vec::new();
            loop {
                let taken = m.take_jobs(cx);
                if taken.is_empty() {
                    break;
                }
                sizes.push(taken.len());
            }
            assert_eq!(sizes, [2, 2, 1]);
        });
    }

    #[gpui::test]
    fn a_song_a_list_showed_with_the_e_comes_into_the_crate_marked(cx: &mut TestAppContext) {
        crate::explicit_marks::note("crate-mark-1", true);
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.add(song("crate-mark-1", "Marked", 100), None, cx);
            m.add(song("crate-mark-2", "Plain", 100), None, cx);
            assert!(m.is_explicit("crate-mark-1"));
            assert!(!m.is_explicit("crate-mark-2"));
        });
        crate::explicit_marks::note("crate-mark-3", true);
        model.update(cx, |m, cx| {
            m.insert_at(vec![(song("crate-mark-3", "Dragged", 100), None)], 0, cx);
            assert!(m.is_explicit("crate-mark-3"), "also when dragged in at a slot");
        });
    }

    #[gpui::test]
    fn a_batch_does_not_outlive_its_account(cx: &mut TestAppContext) {
        let (model, _, _) = swap_setup(cx, script_for_abc());
        model.update(cx, |m, cx| {
            m.start_swap(Want::Explicit, None, cx);
            m.set_account(Some("other".into()), cx);
            assert!(!m.swapping());
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert!(m.is_empty() && !m.can_undo()));
    }
}
