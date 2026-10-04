//! The Crate: a scratch list of songs to remember. One key (C) or "Add to Crate" in a song's menu
//! sets a song aside without touching the queue; the Crate page plays it, moves it to the queue
//! or a playlist, or empties it.
//!
//! It is local, kept in the app's SQLite store per account (see `apollo_store::CrateRow`), never
//! written to YouTube. Reads come from memory; every change is written by one background thread
//! in the order it was made. Signing out deletes it.

use std::collections::HashSet;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use apollo_domain::model::Track;
use apollo_store::CrateRow;
use gpui::{App, Context, Entity, Global};

use crate::player::PlayerModel;
use crate::settings::SqliteSettings;

/// Where the crate is kept. Blocking: call off the UI thread.
pub trait CrateStore: Send + Sync {
    /// The crate saved for `account_key`, newest first. Another account's is forgotten.
    fn load(&self, account_key: &str) -> Vec<CrateRow>;
    fn add(&self, account_key: &str, row: &CrateRow);
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
    /// Newest first.
    items: Vec<CrateRow>,
    /// Bumped whenever the list is replaced, so a load that finishes late is dropped.
    epoch: u64,
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
                // Anything set aside while this was loading stays on top.
                let added: Vec<CrateRow> = m.items.drain(..).collect();
                let known: HashSet<String> = added
                    .iter()
                    .map(|r| r.track.video_id.as_str().to_owned())
                    .collect();
                m.items = added;
                m.items.extend(
                    rows.into_iter()
                        .filter(|r| !known.contains(r.track.video_id.as_str())),
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

    /// Set a song aside. One already in the crate stays where it is. Returns whether it was new.
    pub fn add(&mut self, track: Track, thumbnail: Option<String>, cx: &mut Context<Self>) -> bool {
        if self.contains(track.video_id.as_str()) {
            return false;
        }
        let row = CrateRow {
            track,
            thumbnail,
            added_ms: now_ms(),
        };
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Add(key.clone(), Box::new(row.clone())));
        }
        self.items.insert(0, row);
        cx.notify();
        true
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
        cx.notify();
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
            rows.iter().rev().map(|(_, r)| r.clone()).collect()
        }
        fn add(&self, key: &str, row: &CrateRow) {
            self.rows.lock().unwrap().push((key.into(), row.clone()));
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
    fn songs_are_added_newest_first_once_each(cx: &mut TestAppContext) {
        let model = cx.new(|_| CrateModel::new());
        model.update(cx, |m, cx| {
            assert!(m.add(track("a"), None, cx));
            assert!(m.add(track("b"), None, cx));
            assert!(!m.add(track("a"), None, cx), "already there");
            assert_eq!(ids(m), ["b", "a"]);
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

        // A fresh model for the same account reads it back, newest first.
        let again = cx.new(|_| CrateModel::new());
        again.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
        });
        cx.run_until_parked();
        again.read_with(cx, |m, _| {
            assert_eq!(ids(m), ["b", "a"]);
            assert_eq!(
                m.items()[1].thumbnail.as_deref(),
                Some("https://example.invalid/a.jpg")
            );
        });

        // Another account sees nothing of it.
        again.update(cx, |m, cx| m.set_account(Some("other".into()), cx));
        cx.run_until_parked();
        again.read_with(cx, |m, _| assert!(m.is_empty()));
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
