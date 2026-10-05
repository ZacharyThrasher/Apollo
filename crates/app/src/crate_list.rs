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
use std::time::{SystemTime, UNIX_EPOCH};

use apollo_domain::model::Track;
use apollo_domain::undo::Side;
use apollo_store::CrateRow;
use gpui::{App, Context, Entity, Global};

use crate::library::writes::{LibraryWrites, WritesEvent};
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, SqliteSettings, CRATE_FILE_AND_FORGET};

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
}

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
        if self.contains(track.video_id.as_str()) {
            return false;
        }
        let before = self.items.clone();
        let row = CrateRow {
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
        true
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
        let mut added = 0;
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
                    track,
                    thumbnail,
                    added_ms: now_ms(),
                };
                if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
                    let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
                }
                self.items.insert(slot, row);
                added += 1;
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
        let words = if added > 0 {
            format!("added {} to the Crate", songs(added))
        } else {
            format!("moved {} in the Crate", songs(asked))
        };
        self.record(before, words, cx);
        cx.notify();
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
    let Some(model) = handle(cx) else { return };
    let thumbnail = thumbnail.or_else(|| player.read(cx).art_url(track.video_id.as_str()));
    model.update(cx, |m, cx| {
        m.add(track, thumbnail, cx);
    });
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
}
