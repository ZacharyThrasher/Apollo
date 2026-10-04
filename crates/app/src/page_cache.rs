//! The page cache: the last answer for Home, the library lists and the album, artist and
//! playlist pages, so reopening one draws at once and refreshes behind it ("instant pages").
//!
//! * **Layers.** Answers live in memory (read on the UI thread, no I/O) and in the `page_cache`
//!   table of the app's SQLite file (written by one worker thread, in order, off the UI thread).
//!   When the account is chosen the table's rows for it are read into memory in the background.
//! * **Per account.** Everything is keyed by the account the answer was fetched for. Choosing a
//!   different account purges the old one's rows; [`PageCache::sign_out`] purges everything.
//!   While no account is chosen the cache answers nothing and stores nothing.
//! * **Invalidation.** A local write (a like, an add to a playlist, later playlist edits) drops
//!   the pages it changed with [`PageCache::invalidate_collection`] and friends. A fetch that was
//!   started before the drop must not put its (older) answer back, so every fetch takes a
//!   [`Ticket`] first and [`PageCache::put`] refuses a ticket that an invalidation, an account
//!   change or a sign-out has overtaken.
//! * **Safe by default.** An answer that no longer parses (the types changed between versions)
//!   is a miss, never an error. Nothing here is a credential: pages hold what the screens show.
//!
//! The cache is a GPUI global ([`PageCache::install`]); a model without one behaves exactly as it
//! did before the cache existed, which is what the tests that do not care about it get.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use apollo_domain::model::BrowseId;
use apollo_services_common::unix_now;
use apollo_store::Store;
use gpui::{App, Global};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// The page keys the screens use.
pub mod keys {
    use apollo_domain::model::BrowseId;

    /// Home's first batch of shelves.
    pub const HOME: &str = "home";
    /// Every library playlist and saved album.
    pub const COLLECTIONS: &str = "lib:collections";

    /// The Library's Artists tab.
    pub const ARTISTS: &str = "lib:artists";
    /// The Library's History tab.
    pub const HISTORY: &str = "lib:history";

    /// An album or playlist page: its whole track list.
    pub fn collection(id: &BrowseId) -> String {
        format!("coll:{}", id.as_str())
    }

    /// An artist page.
    pub fn artist(id: &BrowseId) -> String {
        format!("artist:{}", id.as_str())
    }
}

/// A cached answer and when it was fetched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cached<T> {
    pub value: T,
    /// Unix seconds.
    pub fetched_at: i64,
}

impl<T> Cached<T> {
    /// How old the answer is now.
    pub fn age(&self) -> Duration {
        Duration::from_secs(u64::try_from(unix_now() as i64 - self.fetched_at).unwrap_or(0))
    }
}

/// Taken before a fetch starts and handed to [`PageCache::put`] with its answer, so an answer
/// that arrives after the page was invalidated (or the account changed) is dropped.
#[derive(Clone, Debug)]
pub struct Ticket {
    account: Option<String>,
    account_gen: u64,
    seq: u64,
}

#[derive(Clone)]
struct Slot {
    body: Arc<str>,
    fetched_at: i64,
}

#[derive(Default)]
struct State {
    account: Option<String>,
    /// Bumped by every account change and sign-out.
    account_gen: u64,
    /// Orders fetches against invalidations.
    seq: u64,
    mem: HashMap<String, Slot>,
    /// Key -> the `seq` at which it was last invalidated.
    invalidated: HashMap<String, u64>,
    /// [`PageCache::sign_out`] has run and no account has been chosen since.
    signed_out: bool,
}

enum Job {
    /// Serialize an answer, publish it to memory (unless it was overtaken) and save it.
    Put(Box<dyn FnOnce(&PageCache) -> Option<Job> + Send>),
    /// Save an answer already published to memory.
    Save {
        account: String,
        key: String,
        body: Arc<str>,
        fetched_at: i64,
    },
    Delete {
        account: String,
        key: String,
    },
    /// Every account's rows (sign-out).
    ClearAll,
    /// Every row that is not this account's.
    ClearExcept(String),
    /// Everything before this is written; then the sender is told.
    Barrier(Sender<()>),
}

struct Inner {
    path: Option<PathBuf>,
    state: Mutex<State>,
    writer: Option<Mutex<Sender<Job>>>,
}

/// See the module docs. Cheap to clone; clones share one cache.
#[derive(Clone)]
pub struct PageCache {
    inner: Arc<Inner>,
}

impl Global for PageCache {}

impl PageCache {
    /// A cache persisted in the SQLite file at `path`, or memory only for `None`.
    pub fn new(path: Option<PathBuf>) -> Self {
        let (writer, rx) = match &path {
            Some(_) => {
                let (tx, rx) = channel::<Job>();
                (Some(Mutex::new(tx)), Some(rx))
            }
            None => (None, None),
        };
        let cache = Self {
            inner: Arc::new(Inner {
                path: path.clone(),
                state: Mutex::new(State::default()),
                writer,
            }),
        };
        if let (Some(path), Some(rx)) = (path, rx) {
            // Holds the cache weakly: the worker ends when the last handle is dropped.
            let weak = Arc::downgrade(&cache.inner);
            let _ = std::thread::Builder::new()
                .name("apollo-page-cache".into())
                .spawn(move || write_loop(path, rx, weak));
        }
        cache
    }

    /// Memory only: nothing survives the process.
    pub fn in_memory() -> Self {
        Self::new(None)
    }

    /// Makes this the app's cache.
    pub fn install(self, cx: &mut App) {
        cx.set_global(self);
    }

    /// The app's cache, when one is installed.
    pub fn try_global(cx: &App) -> Option<PageCache> {
        cx.try_global::<PageCache>().cloned()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send(&self, job: Job) {
        if let Some(writer) = &self.inner.writer {
            let _ = writer.lock().unwrap_or_else(|e| e.into_inner()).send(job);
        }
    }

    // ---- the account ----------------------------------------------------------------------

    /// The account the cache serves, if one is chosen.
    pub fn account(&self) -> Option<String> {
        self.state().account.clone()
    }

    /// Chooses the account the cache serves. A different account than before starts empty and
    /// its predecessor's rows are purged; the account's own saved rows are read into memory in
    /// the background. `None` (nobody chosen yet) stops the cache answering and storing, and
    /// keeps what is saved.
    pub fn set_account(&self, account: Option<String>) {
        let preload_gen = {
            let mut state = self.state();
            if state.account == account {
                return;
            }
            state.account = account.clone();
            state.signed_out = false;
            state.account_gen += 1;
            state.seq += 1;
            state.mem.clear();
            state.invalidated.clear();
            state.account_gen
        };
        let Some(account) = account else {
            return;
        };
        self.send(Job::ClearExcept(account.clone()));
        let Some(path) = self.inner.path.clone() else {
            return;
        };
        let cache = self.clone();
        let started = self.state().seq;
        let _ = std::thread::Builder::new()
            .name("apollo-page-preload".into())
            .spawn(move || {
                let Ok(store) = Store::open(&path) else {
                    return;
                };
                let Ok(pages) = store.pages_for(&account) else {
                    return;
                };
                let mut state = cache.state();
                if state.account_gen != preload_gen {
                    return;
                }
                for page in pages {
                    // What was put or dropped since the read began is newer than the row.
                    let dropped = state.invalidated.get(&page.key).is_some_and(|s| *s > started);
                    if dropped || state.mem.contains_key(&page.key) {
                        continue;
                    }
                    state.mem.insert(
                        page.key,
                        Slot {
                            body: page.body.into(),
                            fetched_at: page.fetched_at,
                        },
                    );
                }
            });
    }

    /// The account signed out: nothing cached for anyone stays, in memory or on disk. Saying it
    /// again changes nothing.
    pub fn sign_out(&self) {
        {
            let mut state = self.state();
            // Idempotent: the shell reports the state on every account change.
            if state.signed_out {
                return;
            }
            state.signed_out = true;
            state.account = None;
            state.account_gen += 1;
            state.seq += 1;
            state.mem.clear();
            state.invalidated.clear();
        }
        self.send(Job::ClearAll);
    }

    // ---- reading and writing ------------------------------------------------------------

    /// The cached answer for `key`, from memory (no I/O: safe on the UI thread). Anything that
    /// does not parse is a miss.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<Cached<T>> {
        let slot = self.state().mem.get(key).cloned()?;
        parse(&slot)
    }

    /// Like [`Self::get`], but falls back to the saved row for `account` when memory has
    /// nothing (the first read after launch, before the preload has landed). Blocking: call
    /// off the UI thread.
    pub fn read_saved<T: DeserializeOwned>(&self, account: &str, key: &str) -> Option<Cached<T>> {
        {
            let state = self.state();
            if state.account.as_deref() != Some(account) {
                return None;
            }
            if let Some(slot) = state.mem.get(key) {
                return parse(slot);
            }
        }
        let path = self.inner.path.as_ref()?;
        let page = Store::open(path).ok()?.get_page(account, key).ok()??;
        // The account may have changed while the file was read.
        if self.state().account.as_deref() != Some(account) {
            return None;
        }
        parse(&Slot {
            body: page.body.into(),
            fetched_at: page.fetched_at,
        })
    }

    /// Taken before a fetch starts; see [`Ticket`].
    pub fn ticket(&self) -> Ticket {
        let state = self.state();
        Ticket {
            account: state.account.clone(),
            account_gen: state.account_gen,
            seq: state.seq,
        }
    }

    /// Stores `value` as the answer for `key` unless the ticket has been overtaken (the page
    /// was invalidated, or the account changed, since the fetch began). Serializing and writing
    /// happen on the cache's worker thread, so this costs the caller a clone. Memory gets the
    /// answer when the worker reaches it, not before: [`Self::flush`] waits for that.
    pub fn put<T: Serialize + Send + 'static>(&self, ticket: &Ticket, key: String, value: T) {
        if !self.is_current(ticket, &key) {
            return;
        }
        let Some(account) = ticket.account.clone() else {
            return;
        };
        let ticket = ticket.clone();
        let apply = move |cache: &PageCache| -> Option<Job> {
            let body = serde_json::to_string(&value).ok()?;
            let body: Arc<str> = body.into();
            let fetched_at = unix_now() as i64;
            {
                let mut state = cache.state();
                // Re-checked under the lock: the drop may have come while serializing.
                if !is_current_in(&state, &ticket, &key) {
                    return None;
                }
                state.mem.insert(
                    key.clone(),
                    Slot {
                        body: body.clone(),
                        fetched_at,
                    },
                );
            }
            Some(Job::Save {
                account,
                key,
                body,
                fetched_at,
            })
        };
        // A memory-only cache has no worker (and nothing to save): publish here (tests, mostly).
        if self.inner.writer.is_some() {
            self.send(Job::Put(Box::new(apply)));
        } else {
            apply(self);
        }
    }

    fn is_current(&self, ticket: &Ticket, key: &str) -> bool {
        is_current_in(&self.state(), ticket, key)
    }

    // ---- invalidation -----------------------------------------------------------------------

    /// Drops one page and refuses the answer of any fetch of it that began before now.
    pub fn invalidate(&self, key: &str) {
        let account = {
            let mut state = self.state();
            state.seq += 1;
            let seq = state.seq;
            state.invalidated.insert(key.to_owned(), seq);
            state.mem.remove(key);
            state.account.clone()
        };
        if let Some(account) = account {
            self.send(Job::Delete {
                account,
                key: key.to_owned(),
            });
        }
    }

    /// An album or playlist changed (a track was added or removed, it was renamed): its page is
    /// dropped, and so is the library list, whose rows carry its title and track count. `id` is
    /// the browse id (`VL…`, `MPRE…`) or a bare playlist id (`PL…`).
    pub fn invalidate_collection(&self, id: &str) {
        self.invalidate(&keys::collection(&BrowseId::new(browse_id(id))));
        self.invalidate(keys::COLLECTIONS);
    }

    /// Liked Music changed.
    pub fn invalidate_liked_music(&self) {
        self.invalidate_collection(apollo_domain::library::LIKED_MUSIC_BROWSE_ID);
    }

    /// The set of playlists or albums changed (one was created, deleted or saved).
    pub fn invalidate_library(&self) {
        self.invalidate(keys::COLLECTIONS);
    }

    /// An artist page changed.
    pub fn invalidate_artist(&self, id: &BrowseId) {
        self.invalidate(&keys::artist(id));
    }

    /// The account subscribed to or left an artist: the page (its button) and the Library's
    /// Artists list both change.
    pub fn invalidate_subscription(&self, id: &BrowseId) {
        self.invalidate_artist(id);
        self.invalidate(keys::ARTISTS);
    }

    /// Wait until everything handed to the worker so far is written (tests, quitting).
    pub fn flush(&self) {
        let Some(_) = &self.inner.writer else {
            return;
        };
        let (done, wait) = channel();
        self.send(Job::Barrier(done));
        let _ = wait.recv_timeout(Duration::from_secs(5));
    }
}

/// The browse id a playlist or album page is keyed by: a bare playlist id (`PL…`) gains the
/// `VL` the service's browse ids carry.
pub fn browse_id(id: &str) -> String {
    if id.starts_with("VL") || id.starts_with("MPRE") {
        id.to_owned()
    } else {
        format!("VL{id}")
    }
}

fn is_current_in(state: &State, ticket: &Ticket, key: &str) -> bool {
    ticket.account.is_some()
        && state.account == ticket.account
        && state.account_gen == ticket.account_gen
        && state
            .invalidated
            .get(key)
            .map_or(true, |dropped| *dropped <= ticket.seq)
}

fn parse<T: DeserializeOwned>(slot: &Slot) -> Option<Cached<T>> {
    Some(Cached {
        value: serde_json::from_str(&slot.body).ok()?,
        fetched_at: slot.fetched_at,
    })
}

/// The worker: applies jobs in the order they were handed over.
fn write_loop(path: PathBuf, rx: std::sync::mpsc::Receiver<Job>, cache: std::sync::Weak<Inner>) {
    let open = || {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        Store::open(&path).ok()
    };
    while let Ok(job) = rx.recv() {
        // A put publishes to memory, then is saved right here, before the next queued job.
        let job = match job {
            Job::Put(apply) => match cache.upgrade() {
                Some(inner) => match apply(&PageCache { inner }) {
                    Some(save) => save,
                    None => continue,
                },
                None => break,
            },
            other => other,
        };
        let result = match job {
            Job::Barrier(done) => {
                let _ = done.send(());
                continue;
            }
            Job::Put(_) => continue,
            Job::Save {
                account,
                key,
                body,
                fetched_at,
            } => open().map(|s| s.put_page(&account, &key, &body, fetched_at)),
            Job::Delete { account, key } => open().map(|s| s.delete_page(&account, &key)),
            Job::ClearAll => open().map(|s| s.clear_pages()),
            Job::ClearExcept(account) => open().map(|s| s.clear_pages_except(&account)),
        };
        if let Some(Err(err)) = result {
            eprintln!("page cache write failed: {err}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> PageCache {
        let cache = PageCache::in_memory();
        cache.set_account(Some("acct".into()));
        cache
    }

    fn put(cache: &PageCache, key: &str, value: &str) {
        cache.put(&cache.ticket(), key.to_owned(), value.to_owned());
    }

    #[test]
    fn nothing_is_answered_or_stored_before_an_account_is_chosen() {
        let cache = PageCache::in_memory();
        put(&cache, "k", "v");
        assert!(cache.get::<String>("k").is_none());
        cache.set_account(Some("acct".into()));
        assert!(cache.get::<String>("k").is_none(), "the early put was refused");
        put(&cache, "k", "v");
        assert_eq!(cache.get::<String>("k").unwrap().value, "v");
    }

    #[test]
    fn a_different_account_starts_empty_and_sign_out_forgets_everything() {
        let cache = cache();
        put(&cache, "k", "v");
        cache.set_account(Some("other".into()));
        assert!(cache.get::<String>("k").is_none());
        put(&cache, "k", "w");
        cache.set_account(None);
        assert!(cache.get::<String>("k").is_none(), "no account, no answers");
        cache.set_account(Some("other".into()));
        cache.sign_out();
        assert!(cache.account().is_none());
        assert!(cache.get::<String>("k").is_none());
    }

    #[test]
    fn an_answer_that_was_overtaken_by_an_invalidation_is_not_stored() {
        let cache = cache();
        put(&cache, "k", "old");
        let ticket = cache.ticket();
        cache.invalidate("k");
        assert!(cache.get::<String>("k").is_none(), "dropped at once");
        cache.put(&ticket, "k".into(), "stale".to_owned());
        assert!(cache.get::<String>("k").is_none(), "a fetch begun before the write loses");
        cache.put(&cache.ticket(), "k".into(), "fresh".to_owned());
        assert_eq!(cache.get::<String>("k").unwrap().value, "fresh");
        // Other keys are unaffected by the drop.
        let ticket = cache.ticket();
        cache.invalidate("elsewhere");
        cache.put(&ticket, "k2".into(), "kept".to_owned());
        assert_eq!(cache.get::<String>("k2").unwrap().value, "kept");
    }

    #[test]
    fn an_answer_for_the_old_account_is_not_stored_for_the_new_one() {
        let cache = cache();
        let ticket = cache.ticket();
        cache.set_account(Some("other".into()));
        cache.put(&ticket, "k".into(), "old account".to_owned());
        assert!(cache.get::<String>("k").is_none());
    }

    #[test]
    fn a_collection_drop_takes_the_library_list_with_it_and_knows_bare_ids() {
        let cache = cache();
        put(&cache, "coll:VLPLroad", "page");
        put(&cache, keys::COLLECTIONS, "list");
        put(&cache, "coll:VLPLother", "other page");
        cache.invalidate_collection("PLroad");
        assert!(cache.get::<String>("coll:VLPLroad").is_none());
        assert!(cache.get::<String>(keys::COLLECTIONS).is_none());
        assert!(cache.get::<String>("coll:VLPLother").is_some());
        assert_eq!(browse_id("VLPLx"), "VLPLx");
        assert_eq!(browse_id("MPREb_x"), "MPREb_x");
        assert_eq!(browse_id("PLx"), "VLPLx");
    }

    #[test]
    fn a_page_that_no_longer_parses_is_a_miss() {
        let cache = cache();
        put(&cache, "k", "text");
        assert!(cache.get::<Vec<u32>>("k").is_none());
        assert!(cache.get::<String>("k").is_some());
    }

    #[test]
    fn saved_pages_come_back_after_a_restart_and_only_for_their_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let first = PageCache::new(Some(path.clone()));
        first.set_account(Some("acct".into()));
        put(&first, "k", "v");
        first.flush();

        let second = PageCache::new(Some(path.clone()));
        second.set_account(Some("acct".into()));
        // Read straight from the file, as Home does at launch.
        let saved = second.read_saved::<String>("acct", "k").expect("saved row");
        assert_eq!(saved.value, "v");
        assert!(second.read_saved::<String>("other", "k").is_none());
        // And the preload lands in memory shortly after.
        let mut warm = false;
        for _ in 0..200 {
            if second.get::<String>("k").is_some() {
                warm = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(warm, "the preload fills memory");

        let third = PageCache::new(Some(path.clone()));
        third.set_account(Some("someone else".into()));
        third.flush();
        let fourth = PageCache::new(Some(path));
        fourth.set_account(Some("acct".into()));
        assert!(
            fourth.read_saved::<String>("acct", "k").is_none(),
            "another account purged the first one's rows"
        );
    }

    #[test]
    fn sign_out_purges_the_file_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let cache = PageCache::new(Some(path.clone()));
        cache.set_account(Some("acct".into()));
        put(&cache, "k", "v");
        cache.flush();
        cache.sign_out();
        cache.flush();
        let store = Store::open(&path).unwrap();
        assert!(store.pages_for("acct").unwrap().is_empty());
    }

    #[test]
    fn an_invalidation_reaches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apollo.db");
        let cache = PageCache::new(Some(path.clone()));
        cache.set_account(Some("acct".into()));
        put(&cache, "k", "v");
        cache.flush();
        cache.invalidate("k");
        cache.flush();
        let store = Store::open(&path).unwrap();
        assert!(store.get_page("acct", "k").unwrap().is_none());
    }
}
