//! Blocked artists: "Don't play this artist" keeps an artist's songs out of what Apollo adds on
//! its own, which is radio, autoplay and Home's personal shelves ("On repeat", "Forgotten
//! favourites"). Songs already in the queue stay, and playing an artist's page or a playlist
//! that holds their songs still works: it is what the owner asked for then.
//!
//! The list is local, kept in the app's SQLite store per account (see `apollo_store`'s
//! `blocked_artists` table) and never sent to YouTube. Reads come from memory; every change is
//! written by one background thread in the order it was made. Signing out deletes it.

use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use apollo_domain::blocklist::{BlockedArtist, Blocklist};
use gpui::{App, Context, Entity, EventEmitter, Global};

use crate::settings::SqliteSettings;

/// Where the list is kept. Blocking: call off the UI thread.
pub trait BlockStore: Send + Sync {
    /// The list saved for `account_key`, newest first. Another account's is forgotten.
    fn load(&self, account_key: &str) -> Vec<BlockedArtist>;
    fn add(&self, account_key: &str, artist: &BlockedArtist, blocked_ms: i64);
    fn remove(&self, account_key: &str, artist: &BlockedArtist);
    /// Delete every account's list (sign-out).
    fn purge(&self);
}

impl BlockStore for SqliteSettings {
    fn load(&self, account_key: &str) -> Vec<BlockedArtist> {
        let Some(mut store) = self.open() else {
            return Vec::new();
        };
        store.blocked_load(account_key).unwrap_or_else(|err| {
            eprintln!("could not read the blocked artists: {err}");
            Vec::new()
        })
    }

    fn add(&self, account_key: &str, artist: &BlockedArtist, blocked_ms: i64) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.blocked_add(account_key, artist, blocked_ms) {
                eprintln!("could not block the artist: {err}");
            }
        }
    }

    fn remove(&self, account_key: &str, artist: &BlockedArtist) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.blocked_remove(account_key, artist) {
                eprintln!("could not unblock the artist: {err}");
            }
        }
    }

    fn purge(&self) {
        if let Some(mut store) = self.open() {
            if let Err(err) = store.blocked_purge() {
                eprintln!("could not forget the blocked artists: {err}");
            }
        }
    }
}

enum Write {
    Add(String, BlockedArtist, i64),
    Remove(String, BlockedArtist),
    Purge,
}

/// The list changed: whatever was built from it (Home's personal shelves) is out of date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changed;

/// The blocked artists as the UI and the filters see them.
pub struct BlockModel {
    writer: Option<Sender<Write>>,
    store: Option<Arc<dyn BlockStore>>,
    /// The account the list belongs to; nothing is stored without one.
    key: Option<String>,
    list: Blocklist,
    /// Bumped whenever the list is replaced, so a load that finishes late is dropped.
    epoch: u64,
}

impl EventEmitter<Changed> for BlockModel {}

impl Default for BlockModel {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockModel {
    /// An empty list that lasts only for this run (tests, and until a store is installed).
    pub fn new() -> Self {
        Self {
            writer: None,
            store: None,
            key: None,
            list: Blocklist::default(),
            epoch: 0,
        }
    }

    /// Where the list is kept between runs. An account already chosen has its list read now.
    pub fn install(&mut self, store: Option<Arc<dyn BlockStore>>, cx: &mut Context<Self>) {
        self.writer = store.clone().map(|store| {
            let (tx, rx) = channel::<Write>();
            std::thread::Builder::new()
                .name("apollo-blocked-save".into())
                .spawn(move || {
                    for write in rx {
                        match write {
                            Write::Add(key, artist, ms) => store.add(&key, &artist, ms),
                            Write::Remove(key, artist) => store.remove(&key, &artist),
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

    /// The signed-in account (its opaque key) changed. Another account's list never applies:
    /// it empties, and the new account's own is read from the store.
    pub fn set_account(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        if self.key == key {
            return;
        }
        self.key = key;
        self.epoch += 1;
        self.replace(Blocklist::default(), cx);
        self.load(cx);
    }

    /// Read the account's list from the store, off the UI thread.
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
                // Anything blocked while this was loading stays on top.
                let mut merged = Blocklist::new(rows);
                for added in m.list.entries().iter().rev() {
                    merged.add(added.clone());
                }
                m.replace(merged, cx);
            });
        })
        .detach();
    }

    fn replace(&mut self, list: Blocklist, cx: &mut Context<Self>) {
        if self.list != list {
            self.list = list;
            cx.emit(Changed);
            cx.notify();
        }
    }

    /// Sign-out: the list goes with the session, here and in the store.
    pub fn forget(&mut self, cx: &mut Context<Self>) {
        self.key = None;
        self.epoch += 1;
        self.replace(Blocklist::default(), cx);
        if let Some(writer) = &self.writer {
            let _ = writer.send(Write::Purge);
        }
    }

    /// The account the list belongs to.
    pub fn account(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn list(&self) -> &Blocklist {
        &self.list
    }

    /// A copy for work that runs off the UI thread.
    pub fn snapshot(&self) -> Blocklist {
        self.list.clone()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Whether this artist is blocked.
    pub fn is_blocked(&self, artist: &BlockedArtist) -> bool {
        self.list.blocks_artist(&apollo_domain::model::ArtistRef {
            name: artist.name.clone(),
            id: artist.id.clone().map(apollo_domain::model::BrowseId::new),
        })
    }

    /// Block an artist. Returns whether it was new.
    pub fn block(&mut self, artist: BlockedArtist, cx: &mut Context<Self>) -> bool {
        let mut next = self.list.clone();
        if !next.add(artist.clone()) {
            return false;
        }
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Add(key.clone(), artist, now_ms()));
        }
        self.replace(next, cx);
        true
    }

    /// Let an artist back in. Returns whether it was blocked.
    pub fn unblock(&mut self, artist: &BlockedArtist, cx: &mut Context<Self>) -> bool {
        let mut next = self.list.clone();
        if !next.remove(artist) {
            return false;
        }
        if let (Some(writer), Some(key)) = (&self.writer, &self.key) {
            let _ = writer.send(Write::Remove(key.clone(), artist.clone()));
        }
        self.replace(next, cx);
        true
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// The shell's list, reachable from the player, the menu and the dialog.
#[derive(Clone)]
pub struct BlockHandle(pub Entity<BlockModel>);

impl Global for BlockHandle {}

/// The shell's list, where there is a shell.
pub fn handle(cx: &App) -> Option<Entity<BlockModel>> {
    cx.try_global::<BlockHandle>().map(|h| h.0.clone())
}

/// A copy of the blocked artists for a filter; empty where there is no shell.
pub fn snapshot(cx: &App) -> Blocklist {
    handle(cx).map(|m| m.read(cx).snapshot()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gpui::{AppContext, TestAppContext};

    use super::*;

    /// A store that keeps everything in memory.
    #[derive(Default)]
    struct Memory {
        rows: Mutex<Vec<(String, BlockedArtist)>>,
    }

    impl BlockStore for Memory {
        fn load(&self, key: &str) -> Vec<BlockedArtist> {
            let mut rows = self.rows.lock().unwrap();
            rows.retain(|(k, _)| k == key);
            rows.iter().rev().map(|(_, a)| a.clone()).collect()
        }
        fn add(&self, key: &str, artist: &BlockedArtist, _: i64) {
            self.rows.lock().unwrap().push((key.into(), artist.clone()));
        }
        fn remove(&self, key: &str, artist: &BlockedArtist) {
            self.rows
                .lock()
                .unwrap()
                .retain(|(k, a)| !(k == key && a.id == artist.id && a.name == artist.name));
        }
        fn purge(&self) {
            self.rows.lock().unwrap().clear();
        }
    }

    fn artist(id: &str) -> BlockedArtist {
        BlockedArtist {
            id: Some(id.into()),
            name: format!("Artist {id}"),
        }
    }

    fn wait_for(store: &Memory, rows: usize) {
        // The writer thread is separate from the test executor: wait for it.
        for _ in 0..200 {
            if store.rows.lock().unwrap().len() == rows {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("the writer did not reach {rows} rows");
    }

    #[gpui::test]
    fn blocking_and_unblocking_change_the_list_once_each(cx: &mut TestAppContext) {
        let model = cx.new(|_| BlockModel::new());
        model.update(cx, |m, cx| {
            assert!(m.block(artist("a"), cx));
            assert!(!m.block(artist("a"), cx), "already blocked");
            assert!(m.block(artist("b"), cx));
            assert_eq!(m.len(), 2);
            assert!(m.is_blocked(&artist("a")));
            assert!(m.unblock(&artist("a"), cx));
            assert!(!m.unblock(&artist("a"), cx));
            assert!(!m.is_blocked(&artist("a")));
            assert_eq!(m.len(), 1);
        });
    }

    #[gpui::test]
    fn the_list_is_written_and_read_back_per_account(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| BlockModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            m.block(artist("a"), cx);
            m.block(artist("b"), cx);
        });
        cx.run_until_parked();
        wait_for(&store, 2);

        // A fresh model for the same account reads it back, newest first.
        let again = cx.new(|_| BlockModel::new());
        again.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
        });
        cx.run_until_parked();
        again.read_with(cx, |m, _| {
            let names: Vec<&str> = m.list().entries().iter().map(|a| a.name.as_str()).collect();
            assert_eq!(names, ["Artist b", "Artist a"]);
        });

        // Another account sees nothing of it.
        again.update(cx, |m, cx| m.set_account(Some("other".into()), cx));
        cx.run_until_parked();
        again.read_with(cx, |m, _| assert!(m.is_empty()));
    }

    #[gpui::test]
    fn an_unblock_is_written_too(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| BlockModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            m.block(artist("a"), cx);
        });
        wait_for(&store, 1);
        model.update(cx, |m, cx| {
            m.unblock(&artist("a"), cx);
        });
        wait_for(&store, 0);
    }

    #[gpui::test]
    fn signing_out_empties_it_and_deletes_the_stored_copy(cx: &mut TestAppContext) {
        let store = Arc::new(Memory::default());
        let model = cx.new(|_| BlockModel::new());
        model.update(cx, |m, cx| {
            m.install(Some(store.clone()), cx);
            m.set_account(Some("acct".into()), cx);
            m.block(artist("a"), cx);
            m.forget(cx);
            assert!(m.is_empty());
        });
        wait_for(&store, 0);
    }
}
