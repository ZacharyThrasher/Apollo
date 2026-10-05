//! The Crate: a scratch list of songs to remember. One key (C) or "Add to Crate" in a song's menu
//! sets a song aside at the bottom without touching the queue; the Crate page plays it, arranges
//! it (drag, or Alt+Up/Down), moves it to the queue or a playlist, or empties it.
//!
//! It is local, kept in the app's SQLite store per account (see `apollo_store::CrateRow`), never
//! written to YouTube. Reads come from memory; every change is written by one background thread
//! in the order it was made. Signing out deletes it.

use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ListedTrack, SearchFilter};
use apollo_store::CrateRow;
use gpui::{App, Context, Entity, EventEmitter, Global};

use crate::crate_swap::{self, Pick, Want};
use crate::player::PlayerModel;
use crate::search::SearchSource;
use crate::settings::SqliteSettings;

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
    /// Put `row` in the place of the song `old_video_id` (another version of it, or the same
    /// song with a corrected flag).
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
    /// What Ctrl+Z can take back, oldest first.
    steps: VecDeque<SwapStep>,
    /// Numbers batches and steps.
    serial: u64,
}

/// How many batches Ctrl+Z can take back.
const STEP_LIMIT: usize = 20;

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

/// A batch the owner started: how far it is, and the undo step its swaps go into.
#[derive(Debug)]
struct Batch {
    id: u64,
    want: Want,
    total: usize,
    done: usize,
    swapped: usize,
    /// Songs the search showed to be that version already.
    already: usize,
    /// Searches that failed (the service could not be reached).
    errors: usize,
    /// The queue history's depth when it started (see `domain::undo::newest`).
    note: i64,
    /// Its step in `steps`, made by the first swap.
    step: Option<u64>,
}

/// What one batch changed: the song each swap put in, with the row it replaced.
#[derive(Debug)]
struct SwapStep {
    id: u64,
    /// Where the step sits in time among the other histories ([`apollo_domain::undo::next_seq`]).
    seq: u64,
    note: i64,
    want: Want,
    /// (video id now in the crate, the row it replaced), oldest swap first.
    changes: Vec<(String, CrateRow)>,
}

enum Outcome {
    Swapped,
    /// The song is that version already (the crate had not known).
    Already,
    /// Nothing changed: no match, already that version, or the song left the crate.
    Unchanged,
    Failed,
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
}

/// What the crate tells the window about swaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrateEvent {
    /// Songs were replaced in place by another version (or put back): (old id, new id).
    Swapped(Vec<(String, String)>),
    /// A batch is under way.
    Progress { want: Want, done: usize, total: usize },
    /// A batch is over.
    Finished { want: Want, swapped: usize, total: usize, already: usize, unreachable: bool },
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
            search: None,
            prefer_explicit: false,
            jobs: VecDeque::new(),
            batch: None,
            runner: false,
            steps: VecDeque::new(),
            serial: 0,
        }
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
        self.reset_swaps();
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
        self.reset_swaps();
        self.items.clear();
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
                cx.notify();
            });
        })
        .detach();
    }

    /// Sign-out: the crate goes with the session, here and in the store.
    pub fn forget(&mut self, cx: &mut Context<Self>) {
        self.key = None;
        self.epoch += 1;
        self.reset_swaps();
        self.items.clear();
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
        cx.notify();
        self.auto_swap(vec![video], cx);
        true
    }

    /// Drop songs into the crate before what is at slot `slot` now (`len` or more means the
    /// bottom). A song already in the crate moves there instead of being added twice; the songs
    /// keep the order they are given in. Returns whether anything changed.
    pub fn insert_at(
        &mut self,
        songs: Vec<(Track, Option<String>)>,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = self
            .items
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect::<Vec<_>>();
        let mut slot = slot.min(self.items.len());
        let mut fresh = Vec::new();
        for (track, thumbnail) in songs {
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
                fresh.push(video.to_owned());
                let row = CrateRow {
                    explicit: crate::explicit_marks::is_explicit(video),
                    track,
                    thumbnail,
                    added_ms: now_ms(),
                };
                if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
                    let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
                }
                self.items.insert(slot, row);
            }
            slot += 1;
        }
        let after = self
            .items
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect::<Vec<_>>();
        if before == after {
            return false;
        }
        self.save_order();
        cx.notify();
        self.auto_swap(fresh, cx);
        // Songs that were not in the crate before are the ones added (the rest only moved).
        let added: Vec<String> = after.into_iter().filter(|v| !before.contains(v)).collect();
        crate::confirm::crated(added, cx);
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
        let row = self.items.remove(from);
        self.items.insert(to, row);
        self.save_order();
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
        let Some(at) = self
            .items
            .iter()
            .position(|r| r.track.video_id.as_str() == video_id)
        else {
            return false;
        };
        self.items.remove(at);
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Remove(key.clone(), video_id.to_owned()));
        }
        self.prune_steps();
        cx.notify();
        true
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.items.clear();
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Clear(key.clone()));
        }
        self.prune_steps();
        cx.notify();
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

    fn index_of(&self, video_id: &str) -> Option<usize> {
        self.items
            .iter()
            .position(|r| r.track.video_id.as_str() == video_id)
    }

    /// Look for the other version of the songs that are not `want` yet: every song of the crate,
    /// or only those of `only` (a selection). Each swap replaces the song where it stands, and
    /// the whole batch is one step for Ctrl+Z (`note` is the queue history's depth now). The
    /// window hears of it through [`CrateEvent`].
    pub fn start_swap(
        &mut self,
        want: Want,
        only: Option<&[String]>,
        note: i64,
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
            cx.emit(CrateEvent::Finished { want, swapped: 0, total: 0, already: 0, unreachable: false });
            return SwapStart::Nothing;
        }
        self.serial += 1;
        let id = self.serial;
        let total = ids.len();
        self.batch = Some(Batch {
            id,
            want,
            total,
            done: 0,
            swapped: 0,
            already: 0,
            errors: 0,
            note,
            step: None,
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

    /// A song just added: with Prefer explicit versions on, look for its explicit version. Quiet,
    /// and not a step for Ctrl+Z.
    fn auto_swap(&mut self, video_ids: Vec<String>, cx: &mut Context<Self>) {
        if !self.prefer_explicit || self.search.is_none() {
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
    /// at a time, if one is not running.
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
        })
        .detach();
    }

    /// The next songs to look up. Ones that left the crate meanwhile count as done.
    fn take_jobs(&mut self, cx: &mut Context<Self>) -> Vec<(Job, Track)> {
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

    /// A search came back: swap the song if it found its other version.
    fn finish_job(
        &mut self,
        job: Job,
        found: Result<Vec<ListedTrack>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if job.epoch != self.epoch {
            return;
        }
        // A batch that was undone while this was in flight changes nothing more.
        if job.batch.is_some() && self.batch.as_ref().map(|b| b.id) != job.batch {
            return;
        }
        let outcome = match found {
            Err(_) => Outcome::Failed,
            Ok(songs) => self.apply(&job, &songs, cx),
        };
        self.settle(&job, outcome, cx);
    }

    /// Put the picked version in the song's place.
    fn apply(&mut self, job: &Job, songs: &[ListedTrack], cx: &mut Context<Self>) -> Outcome {
        let Some(at) = self.index_of(&job.video_id) else {
            return Outcome::Unchanged;
        };
        let old = self.items[at].clone();
        let picked = crate_swap::pick(&old.track, job.want, songs, |id| self.contains(id));
        match picked {
            Pick::None => Outcome::Unchanged,
            Pick::Already => {
                // The song is that version already; the crate only had not known.
                if old.explicit != job.want.explicit() {
                    let mut row = old.clone();
                    row.explicit = job.want.explicit();
                    self.write_swap(&job.video_id, &row);
                    self.items[at] = row;
                    cx.notify();
                }
                Outcome::Already
            }
            Pick::Swap(found_at) => {
                let found = &songs[found_at];
                let mut track = found.track.clone();
                if track.artists.is_empty() {
                    track.artists = old.track.artists.clone();
                }
                let row = CrateRow {
                    track,
                    thumbnail: found.thumbnail.clone().or_else(|| old.thumbnail.clone()),
                    explicit: job.want.explicit(),
                    added_ms: old.added_ms,
                };
                let new_id = row.track.video_id.as_str().to_owned();
                self.write_swap(&job.video_id, &row);
                self.items[at] = row;
                if let Some(batch) = job.batch {
                    self.note_change(batch, new_id.clone(), old);
                }
                cx.emit(CrateEvent::Swapped(vec![(job.video_id.clone(), new_id)]));
                cx.notify();
                Outcome::Swapped
            }
        }
    }

    fn write_swap(&self, old_video_id: &str, row: &CrateRow) {
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Swap(key.clone(), old_video_id.to_owned(), Box::new(row.clone())));
        }
    }

    /// Note a swap of batch `batch` for Ctrl+Z, in the batch's step (made by its first swap).
    fn note_change(&mut self, batch: u64, now: String, replaced: CrateRow) {
        let Some(b) = self.batch.as_mut().filter(|b| b.id == batch) else {
            return;
        };
        let live = b
            .step
            .filter(|id| self.steps.iter().any(|s| s.id == *id));
        let step = match live {
            Some(id) => id,
            None => {
                self.serial += 1;
                let id = self.serial;
                b.step = Some(id);
                self.steps.push_back(SwapStep {
                    id,
                    seq: apollo_domain::undo::next_seq(),
                    note: b.note,
                    want: b.want,
                    changes: Vec::new(),
                });
                while self.steps.len() > STEP_LIMIT {
                    self.steps.pop_front();
                }
                id
            }
        };
        if let Some(step) = self.steps.iter_mut().find(|s| s.id == step) {
            step.changes.push((now, replaced));
        }
    }

    /// One song of a job is settled; the batch it counts for moves on and may be over.
    fn settle(&mut self, job: &Job, outcome: Outcome, cx: &mut Context<Self>) {
        let Some(batch) = self.batch.as_mut().filter(|b| Some(b.id) == job.batch) else {
            return;
        };
        batch.done += 1;
        match outcome {
            Outcome::Swapped => batch.swapped += 1,
            Outcome::Already => batch.already += 1,
            Outcome::Failed => batch.errors += 1,
            Outcome::Unchanged => {}
        }
        let (want, done, total) = (batch.want, batch.done, batch.total);
        if done < total {
            cx.emit(CrateEvent::Progress { want, done, total });
            return;
        }
        let (swapped, already, unreachable) = (batch.swapped, batch.already, batch.errors == total);
        self.batch = None;
        cx.emit(CrateEvent::Finished { want, swapped, total, already, unreachable });
    }

    /// A new crate (another account, sign-out): nothing waits and nothing can be taken back.
    fn reset_swaps(&mut self) {
        self.jobs.clear();
        self.batch = None;
        self.steps.clear();
    }

    /// Steps forget swaps whose song left the crate; a step with none left is gone.
    fn prune_steps(&mut self) {
        let present: HashSet<&str> = self
            .items
            .iter()
            .map(|r| r.track.video_id.as_str())
            .collect();
        for step in &mut self.steps {
            step.changes.retain(|(id, _)| present.contains(id.as_str()));
        }
        self.steps.retain(|s| !s.changes.is_empty());
    }

    /// The newest swap Ctrl+Z can take back: the queue history's depth it was made at, and its
    /// place in time among the playlist edits (`apollo_domain::undo::next_seq`).
    pub fn undo_stamp(&self) -> Option<(i64, u64)> {
        self.steps.back().map(|s| (s.note, s.seq))
    }

    /// Ctrl+Z: put back the songs the newest batch swapped, where they are. Returns what was
    /// undone, in words for "Undid: …"; `None` when nothing could be put back.
    pub fn undo_swap(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let step = self.steps.pop_back()?;
        // A batch still running stops: what it had not reached stays as it was.
        if let Some(batch) = self.batch.as_ref().filter(|b| b.step == Some(step.id)) {
            let id = Some(batch.id);
            self.jobs.retain(|j| j.batch != id);
            self.batch = None;
        }
        let mut put_back = Vec::new();
        for (now, old) in step.changes.iter().rev() {
            let Some(at) = self.index_of(now) else {
                continue;
            };
            // The old song came back to the crate by another way meanwhile: it cannot be twice.
            if old.track.video_id.as_str() != now && self.contains(old.track.video_id.as_str()) {
                continue;
            }
            self.write_swap(now, old);
            self.items[at] = old.clone();
            put_back.push((now.clone(), old.track.video_id.as_str().to_owned()));
        }
        if put_back.is_empty() {
            return None;
        }
        let words = match put_back.len() {
            1 => format!("swapped a song to its {} version", step.want.word()),
            n => format!("swapped {n} songs to {} versions", step.want.word()),
        };
        cx.emit(CrateEvent::Swapped(put_back));
        cx.notify();
        Some(words)
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
    let Some(model) = handle(cx) else { return };
    let mut added = Vec::new();
    for (track, thumbnail) in songs {
        let thumbnail = thumbnail.or_else(|| player.read(cx).art_url(track.video_id.as_str()));
        let video = track.video_id.as_str().to_owned();
        if model.update(cx, |m, cx| m.add(track, thumbnail, cx)) {
            added.push(video);
        }
    }
    if added.is_empty() {
        crate::confirm::already_crated(cx);
    } else {
        crate::confirm::crated(added, cx);
    }
}

/// Swap the Crate's songs for their `want` version: every song, or only `only`. The window's tip
/// pill follows along (see [`CrateEvent`]). Does nothing where there is no shell.
pub fn swap(
    player: &Entity<PlayerModel>,
    want: Want,
    only: Option<Vec<String>>,
    cx: &mut App,
) -> Option<SwapStart> {
    let model = handle(cx)?;
    let note = player.read(cx).queue_undo_depth();
    Some(model.update(cx, |m, cx| m.start_swap(want, only.as_deref(), note, cx)))
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
            let taken = row.track.video_id.as_str() != old_video_id
                && rows
                    .iter()
                    .any(|(k, r)| k == key && r.track.video_id == row.track.video_id);
            if taken {
                return;
            }
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
    use std::time::Duration;

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
        asked: Mutex<Vec<String>>,
    }

    impl SearchSource for Script {
        fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
            Err(LoadFailure::Other)
        }

        fn search_filtered(&self, query: &str, filter: SearchFilter) -> Result<SearchPage, LoadFailure> {
            assert_eq!(filter, SearchFilter::Songs, "a swap asks for songs only");
            self.asked.lock().unwrap().push(query.to_owned());
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

    /// A crate of clean songs a, b, c whose explicit versions (a2, b2; none for c) the script knows.
    fn swap_setup(cx: &mut TestAppContext) -> (Entity<CrateModel>, Arc<Script>, Arc<Mutex<Vec<CrateEvent>>>) {
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
        let script = Arc::new(script);
        let model = cx.new(|_| CrateModel::new());
        let heard = listen(&model, cx);
        model.update(cx, |m, cx| {
            m.install_search(script.clone());
            for id in ["a", "b", "c"] {
                m.add(song(id, &format!("Song {id}"), 200), None, cx);
            }
        });
        (model, script, heard)
    }

    #[gpui::test]
    fn a_batch_swaps_in_place_and_leaves_songs_with_no_match(cx: &mut TestAppContext) {
        let (model, script, heard) = swap_setup(cx);
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, 0, cx));
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
        assert_eq!(
            heard.last(),
            Some(&CrateEvent::Finished { want: Want::Explicit, swapped: 2, total: 3, already: 0, unreachable: false })
        );
        assert!(heard.contains(&CrateEvent::Swapped(vec![("a".into(), "a2".into())])));
    }

    #[gpui::test]
    fn only_the_picked_songs_are_looked_up(cx: &mut TestAppContext) {
        let (model, script, _) = swap_setup(cx);
        let picked = ["b".to_owned()];
        let start = model.update(cx, |m, cx| m.start_swap(Want::Explicit, Some(&picked), 0, cx));
        assert_eq!(start, SwapStart::Started(1));
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b2", "c"]));
        assert_eq!(*script.asked.lock().unwrap(), ["Song b Artist"]);
    }

    #[gpui::test]
    fn one_undo_puts_the_whole_batch_back_where_it_was(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let (model, _, heard) = swap_setup(cx);
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
        });
        cx.run_until_parked();
        // install() starts a fresh crate for the account; fill it again.
        let mut script = Script::default();
        script.answers.insert(
            "Song a Artist".into(),
            vec![found("a2", "Song a", 200, true)],
        );
        script.answers.insert(
            "Song b Artist".into(),
            vec![found("b2", "Song b", 200, true)],
        );
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script));
            for id in ["a", "b", "c"] {
                m.add(song(id, &format!("Song {id}"), 200), Some(format!("art-{id}")), cx);
            }
            assert_eq!(m.undo_stamp(), None, "nothing to take back yet");
            m.start_swap(Want::Explicit, None, 7, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a2", "b2", "c"]);
            assert_eq!(m.undo_stamp().map(|s| s.0), Some(7), "the queue depth is noted");
        });

        let words = model.update(cx, |m, cx| m.undo_swap(cx));
        assert_eq!(words.as_deref(), Some("swapped 2 songs to explicit versions"));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a", "b", "c"], "one step undid both");
            assert!(m.items().iter().all(|r| !r.explicit));
            assert_eq!(m.items()[0].thumbnail.as_deref(), Some("art-a"), "the old row, whole");
            assert_eq!(m.undo_stamp(), None);
        });
        assert!(heard
            .lock()
            .unwrap()
            .contains(&CrateEvent::Swapped(vec![("b2".into(), "b".into()), ("a2".into(), "a".into())])));
        assert_eq!(model.update(cx, |m, cx| m.undo_swap(cx)), None, "nothing left");

        // The store followed both ways, in the same slots.
        for _ in 0..200 {
            let stored: Vec<String> = store
                .rows
                .lock()
                .unwrap()
                .iter()
                .map(|(_, r)| r.track.video_id.as_str().to_owned())
                .collect();
            if stored == ["a", "b", "c"] {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let rows = store.rows.lock().unwrap();
        let stored: Vec<&str> = rows.iter().map(|(_, r)| r.track.video_id.as_str()).collect();
        assert_eq!(stored, ["a", "b", "c"]);
    }

    #[gpui::test]
    fn a_song_that_left_the_crate_is_not_put_back_by_undo(cx: &mut TestAppContext) {
        let (model, _, _) = swap_setup(cx);
        model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, 0, cx));
        cx.run_until_parked();
        model.update(cx, |m, cx| {
            m.remove("a2", cx);
        });
        let words = model.update(cx, |m, cx| m.undo_swap(cx));
        assert_eq!(words.as_deref(), Some("swapped a song to its explicit version"));
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["b", "c"]));
        // Everything of a step gone: no step left to offer.
        let (model, _, _) = swap_setup(cx);
        model.update(cx, |m, cx| m.start_swap(Want::Explicit, None, 0, cx));
        cx.run_until_parked();
        model.update(cx, |m, cx| m.clear(cx));
        assert_eq!(model.read_with(cx, |m, _| m.undo_stamp()), None);
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
        let start = model.update(cx, |m, cx| m.start_swap(Want::Clean, None, 0, cx));
        assert_eq!(start, SwapStart::Started(1));
        cx.run_until_parked();
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a-clean"]);
            assert!(!m.items()[0].explicit);
        });
    }

    #[gpui::test]
    fn nothing_to_do_and_a_busy_batch_are_said_not_started(cx: &mut TestAppContext) {
        let (model, _, heard) = swap_setup(cx);
        // Everything is explicit already when asked for explicit; no source yet is its own answer.
        let bare = cx.new(|_| CrateModel::new());
        assert_eq!(
            bare.update(cx, |m, cx| m.start_swap(Want::Explicit, None, 0, cx)),
            SwapStart::NoSearch
        );
        let none: [String; 0] = [];
        assert_eq!(
            model.update(cx, |m, cx| m.start_swap(Want::Explicit, Some(&none), 0, cx)),
            SwapStart::Nothing
        );
        assert_eq!(
            heard.lock().unwrap().last(),
            Some(&CrateEvent::Finished { want: Want::Explicit, swapped: 0, total: 0, already: 0, unreachable: false })
        );
        model.update(cx, |m, cx| {
            assert_eq!(m.start_swap(Want::Explicit, None, 0, cx), SwapStart::Started(3));
            assert!(m.swapping());
            assert_eq!(m.start_swap(Want::Clean, None, 0, cx), SwapStart::Busy);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn a_service_that_cannot_be_reached_changes_nothing_and_says_so(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        let heard = listen(&model, cx);
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(Script { fail: true, ..Script::default() }));
            m.add(song("a", "Song a", 200), None, cx);
            m.add(song("b", "Song b", 200), None, cx);
            m.start_swap(Want::Explicit, None, 0, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b"]));
        assert_eq!(
            heard.lock().unwrap().last(),
            Some(&CrateEvent::Finished { want: Want::Explicit, swapped: 0, total: 2, already: 0, unreachable: true })
        );
        assert_eq!(model.read_with(cx, |m, _| m.undo_stamp()), None);
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
            m.start_swap(Want::Explicit, None, 0, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            heard.lock().unwrap().last(),
            Some(&CrateEvent::Finished { want: Want::Explicit, swapped: 0, total: 1, already: 1, unreachable: false })
        );
        model.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a"]);
            assert!(m.is_explicit("a"), "now it is known");
            assert_eq!(m.undo_stamp(), None);
        });
    }

    #[gpui::test]
    fn a_swap_never_puts_a_song_in_the_crate_twice(cx: &mut TestAppContext) {
        let mut script = Script::default();
        script.answers.insert(
            "Song a Artist".into(),
            vec![found("b", "Song a", 200, true)],
        );
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            m.install_search(Arc::new(script));
            m.add(song("a", "Song a", 200), None, cx);
            // The explicit version is already in the crate, as its own row.
            m.add_marked(song("b", "Song a", 200), None, true, cx);
            m.start_swap(Want::Explicit, None, 0, cx);
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert_eq!(ids(m), ["a", "b"]));
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
        model2.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["a2"], "swapped where it was added");
            assert_eq!(m.undo_stamp(), None, "no step for Ctrl+Z: the owner did nothing");
        });
        assert!(
            !heard.lock().unwrap().iter().any(|e| matches!(e, CrateEvent::Progress { .. } | CrateEvent::Finished { .. })),
            "no pill for a song swapped on its own"
        );
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
            m.insert_at(vec![(song("crate-mark-1b", "Dropped", 100), None)], 0, cx);
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
        let (model, _, _) = swap_setup(cx);
        model.update(cx, |m, cx| {
            m.start_swap(Want::Explicit, None, 0, cx);
            m.set_account(Some("other".into()), cx);
            assert!(!m.swapping());
        });
        cx.run_until_parked();
        model.read_with(cx, |m, _| assert!(m.is_empty() && m.undo_stamp().is_none()));
    }
}
