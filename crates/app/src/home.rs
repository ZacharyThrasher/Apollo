//! Home: the account's YouTube Music feed as titled shelves, loaded once per session and kept in
//! memory. No views here; `ui::home_page` draws it.
//!
//! * The first batch loads as soon as the backend is installed and the account can serve it
//!   ([`Access::Open`]). Shelves after it come in on demand ([`HomeModel::load_more`]).
//! * Reopening Home after [`STALE_AFTER`] fetches the first batch again in the background; what
//!   is on screen stays until the new one arrives, and a failed refresh leaves it alone.
//! * A different account (or a sign-out) drops everything, and answers still in flight for the
//!   old one are discarded (`rev`).
//! * The first batch is also kept in the page cache ([`crate::page_cache`]). At launch it is drawn
//!   as soon as the account is known, before the session is confirmed, and the feed is fetched
//!   behind it and swapped in when it arrives (`from_cache`). A failure leaves the cached shelves
//!   alone, except a dead session, which is always shown.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_domain::listing::LoadFailure;
use apollo_provider::{HomeItem, HomePage, HomeShelf};
use gpui::{Context, Entity};

use crate::library::LibrarySource;
use crate::page_cache::{keys, Cached, PageCache, Ticket};
use crate::player::{PlayerModel, Status};

/// Home reopened after this long refreshes.
pub const STALE_AFTER: Duration = Duration::from_secs(10 * 60);

/// Paging stops here: the feed has no end, and a screen of this many rows is already far more
/// than anyone scrolls through.
pub const MAX_SHELVES: usize = 30;

/// Whether the account can serve the feed, as the shell sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The account is still being checked.
    Waiting,
    /// A confirmed session, or one that could not be checked: the feed is asked for.
    Open,
    /// The service refused the session.
    Expired,
    /// Nobody is signed in.
    SignedOut,
}

/// What the page is showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    SignedOut,
    /// Skeleton: the account or the first batch is on its way.
    Loading,
    Ready,
    Failed(LoadFailure),
}

/// The state of the batch after the last one on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum More {
    /// Nothing is in flight (there may or may not be a next batch).
    Idle,
    Loading,
    Failed(LoadFailure),
}

pub struct HomeModel {
    player: Entity<PlayerModel>,
    source: Option<Arc<dyn LibrarySource>>,
    access: Access,
    /// The account epoch the shelves belong to.
    epoch: u64,
    /// `Some` once the first batch has been asked for under the current account.
    asked: bool,
    loading: bool,
    failure: Option<LoadFailure>,
    shelves: Vec<Arc<HomeShelf>>,
    continuation: Option<String>,
    more: More,
    loaded_at: Option<Instant>,
    refreshing: bool,
    /// Bumped by every reset, first load and refresh; an answer for an older one is dropped.
    rev: u64,
    /// Home asks the source this many times (tests and diagnostics).
    requests: u32,
    /// The shelves on screen came from the page cache and no answer has replaced them yet.
    from_cache: bool,
    /// The account key those cached shelves belong to.
    cached_for: Option<String>,
    /// An answer (first batch or refresh) has landed since the last `clear`: the cache must not
    /// be drawn over it.
    fresh_seen: bool,
    /// The account whose cached feed was asked for since the last `clear`.
    restoring: Option<String>,
    /// Home > Personal shelves: "On repeat" and "Forgotten favourites" are shown.
    personal_on: bool,
    /// The personal shelves worked out from the play log, with the account they are for. They
    /// are local: they come and go with the switch and are not part of the feed or its cache.
    personal: Vec<Arc<HomeShelf>>,
    personal_for: Option<String>,
    /// Bumped by every personal refresh and drop; an older answer is dropped.
    personal_rev: u64,
}

impl HomeModel {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |m, _, cx| m.try_start(cx)).detach();
        let mut model = Self {
            player,
            source: None,
            access: Access::Waiting,
            epoch: 0,
            asked: false,
            loading: false,
            failure: None,
            shelves: Vec::new(),
            continuation: None,
            more: More::Idle,
            loaded_at: None,
            refreshing: false,
            rev: 0,
            requests: 0,
            from_cache: false,
            cached_for: None,
            fresh_seen: false,
            restoring: None,
            personal_on: true,
            personal: Vec::new(),
            personal_for: None,
            personal_rev: 0,
        };
        model.try_start(cx);
        model
    }

    // ---- reading -----------------------------------------------------------------------

    pub fn phase(&self) -> Phase {
        match self.access {
            Access::SignedOut => Phase::SignedOut,
            Access::Expired => Phase::Failed(LoadFailure::Session),
            // Cached shelves are drawn while the account is still being checked.
            Access::Waiting if self.shows_cache() => Phase::Ready,
            Access::Waiting => Phase::Loading,
            Access::Open => match (self.failure, self.asked && !self.loading) {
                (Some(f), _) => Phase::Failed(f),
                (None, _) if self.shows_cache() => Phase::Ready,
                (None, true) => Phase::Ready,
                (None, false) => Phase::Loading,
            },
        }
    }

    pub fn shelves(&self) -> &[Arc<HomeShelf>] {
        &self.shelves
    }

    /// What Home draws: the feed's shelves with the personal shelves right after the first of
    /// them. Without a feed there is nothing to put them in.
    pub fn display_shelves(&self) -> Vec<Arc<HomeShelf>> {
        let mut shown = self.shelves.clone();
        if self.personal_on && !shown.is_empty() {
            let at = 1.min(shown.len());
            shown.splice(at..at, self.personal.iter().cloned());
        }
        shown
    }

    /// The personal shelves worked out so far (tests).
    pub fn personal_shelves(&self) -> &[Arc<HomeShelf>] {
        &self.personal
    }

    /// Home > Personal shelves, on or off. Turning it on works the shelves out again.
    pub fn set_personal_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.personal_on == on {
            return;
        }
        self.personal_on = on;
        if on {
            self.refresh_personal(cx);
        } else {
            self.personal_rev += 1;
        }
        cx.notify();
    }

    /// Works the personal shelves out from the play log, off the UI thread, and swaps them in
    /// when they are ready. They stay as they were until then. Cheap and local: asked for at
    /// launch, whenever the account is chosen and every time Home is opened.
    pub fn refresh_personal(&mut self, cx: &mut Context<Self>) {
        if !self.personal_on {
            return;
        }
        let Some(log) = crate::stats::PlayLog::try_global(cx) else {
            return;
        };
        let Some(account) = PageCache::try_global(cx).and_then(|c| c.account()) else {
            return;
        };
        if self.personal_for.as_deref() != Some(account.as_str()) {
            // Another account's shelves must not show while its own are worked out.
            self.personal.clear();
            self.personal_for = Some(account.clone());
        }
        self.personal_rev += 1;
        let rev = self.personal_rev;
        cx.spawn(async move |this, cx| {
            let asked = account.clone();
            let shelves = cx
                .background_executor()
                .spawn(async move {
                    let now = apollo_services_common::unix_now() as i64;
                    let recent = log.totals(
                        &asked,
                        Some(now - crate::stats::report::ON_REPEAT_DAYS * 24 * 60 * 60),
                    );
                    let all = log.totals(&asked, None);
                    crate::stats::report::personal_shelves(&recent, &all, now)
                })
                .await;
            let _ = this.update(cx, |m, cx| {
                let current = PageCache::try_global(cx).and_then(|c| c.account());
                if m.personal_rev != rev || current.as_deref() != Some(account.as_str()) {
                    return;
                }
                let shelves: Vec<Arc<HomeShelf>> = shelves.into_iter().map(Arc::new).collect();
                if shelves != m.personal {
                    m.personal = shelves;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The shelves on screen are the page cache's, not yet replaced by the service's.
    pub fn shows_cache(&self) -> bool {
        self.from_cache && !self.shelves.is_empty() && self.failure.is_none()
    }

    /// Cached shelves are on screen for the account the cache serves now.
    fn keeps_cache(&self, cx: &Context<Self>) -> bool {
        self.shows_cache()
            && self.cached_for.is_some()
            && PageCache::try_global(cx).and_then(|c| c.account()) == self.cached_for
    }

    pub fn more(&self) -> More {
        self.more
    }

    /// Whether another batch can be asked for.
    pub fn has_more(&self) -> bool {
        self.continuation.is_some() && self.shelves.len() < MAX_SHELVES
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub fn loaded_at(&self) -> Option<Instant> {
        self.loaded_at
    }

    /// How many times the source has been asked for the feed (first batches and refreshes).
    pub fn requests(&self) -> u32 {
        self.requests
    }

    /// Whether what is on screen is older than [`STALE_AFTER`] at `now`.
    pub fn is_stale(&self, now: Instant) -> bool {
        self.loaded_at
            .is_some_and(|at| now.saturating_duration_since(at) >= STALE_AFTER)
    }

    // ---- loading -----------------------------------------------------------------------

    /// Picks up the backend's source once the player has one, and loads the first batch. Called
    /// on every player change; does nothing once a source is held.
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
                self.load(cx);
            }
            None if unavailable && self.failure.is_none() => {
                self.failure = Some(LoadFailure::Other);
                self.asked = true;
                cx.notify();
            }
            None => {}
        }
    }

    /// The shell's word on whether the account can serve the feed, with the epoch of the
    /// account it is about. A change of either starts over.
    pub fn set_access(&mut self, access: Access, epoch: u64, cx: &mut Context<Self>) {
        if access == self.access && epoch == self.epoch {
            return;
        }
        let was_open = self.access == Access::Open;
        let changed_account = epoch != self.epoch;
        self.access = access;
        self.epoch = epoch;
        // Cached shelves of the account that is still the one chosen survive the account being
        // checked again: the feed is fetched behind them.
        let keep = matches!(access, Access::Waiting | Access::Open) && self.keeps_cache(cx);
        if !keep && (access != Access::Open || changed_account || !was_open) {
            self.clear();
        }
        if access == Access::Open {
            self.load(cx);
        }
        cx.notify();
    }

    /// Home was opened (the nav, Back from a page opened over it): a failure is tried again, and
    /// a feed older than [`STALE_AFTER`] is fetched again in the background.
    pub fn show(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.refresh_personal(cx);
        if self.source.is_none() || self.access != Access::Open || self.loading {
            return;
        }
        if self.failure.is_some() {
            self.load(cx);
        } else if self.asked && self.is_stale(now) && !self.refreshing {
            self.refresh(cx);
        }
    }

    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if self.access == Access::Open {
            self.load(cx);
        }
    }

    /// Forgets everything loaded; answers in flight are dropped.
    fn clear(&mut self) {
        self.rev += 1;
        self.asked = false;
        self.loading = false;
        self.refreshing = false;
        self.failure = None;
        self.shelves.clear();
        self.continuation = None;
        self.more = More::Idle;
        self.loaded_at = None;
        self.from_cache = false;
        self.cached_for = None;
        self.fresh_seen = false;
        self.restoring = None;
    }

    // ---- the page cache ---------------------------------------------------------------------

    /// Draws the account's last feed from the page cache, if there is one and nothing newer is
    /// on screen: asked for again whenever the account is (re)chosen, a no-op after the first.
    /// The shell calls it once the cache knows the account, which is before the session has
    /// been confirmed, so launch shows Home at once.
    pub fn restore_cached(&mut self, cx: &mut Context<Self>) {
        self.refresh_personal(cx);
        let Some(cache) = PageCache::try_global(cx) else {
            return;
        };
        let Some(account) = cache.account() else {
            return;
        };
        if self.restoring.as_deref() == Some(account.as_str()) || !self.cache_wanted() {
            return;
        }
        self.restoring = Some(account.clone());
        cx.spawn(async move |this, cx| {
            let asked = account.clone();
            let cached = cx
                .background_executor()
                .spawn(async move { cache.read_saved::<HomePage>(&asked, keys::HOME) })
                .await;
            let _ = this.update(cx, |m, cx| m.apply_cached(account, cached, cx));
        })
        .detach();
    }

    /// Nothing the cache could replace is on screen, and the account may see a feed.
    fn cache_wanted(&self) -> bool {
        !matches!(self.access, Access::SignedOut | Access::Expired)
            && self.shelves.is_empty()
            && self.failure.is_none()
            && !self.fresh_seen
    }

    fn apply_cached(
        &mut self,
        account: String,
        cached: Option<Cached<HomePage>>,
        cx: &mut Context<Self>,
    ) {
        let Some(cached) = cached.filter(|c| !c.value.shelves.is_empty()) else {
            return;
        };
        let still_ours = PageCache::try_global(cx).and_then(|c| c.account()).as_deref()
            == Some(account.as_str())
            && self.restoring.as_deref() == Some(account.as_str());
        if !still_ours || !self.cache_wanted() {
            return;
        }
        let age = cached.age();
        self.shelves = cached.value.shelves.into_iter().map(Arc::new).collect();
        // A saved continuation token is not believed: the next batch comes after the refresh.
        self.continuation = None;
        self.from_cache = true;
        self.cached_for = Some(account);
        // Its age counts, so an old feed is refreshed at the next reopening if the fetch behind
        // it fails.
        self.loaded_at = Instant::now().checked_sub(age);
        cx.notify();
    }

    /// Keeps a fresh first batch for the next launch.
    fn remember(&self, ticket: Option<&Ticket>, shelves: &[HomeShelf], cx: &Context<Self>) {
        if let (Some(cache), Some(ticket)) = (PageCache::try_global(cx), ticket) {
            cache.put(
                ticket,
                keys::HOME.to_owned(),
                HomePage {
                    shelves: shelves.to_vec(),
                    continuation: None,
                    skipped: 0,
                },
            );
        }
    }

    /// The first batch, with the skeleton showing meanwhile (or, when cached shelves are on
    /// screen, behind them).
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if self.access != Access::Open {
            return;
        }
        if self.keeps_cache(cx) {
            self.asked = true;
            self.loading = false;
            self.refresh(cx);
            return;
        }
        self.clear();
        self.asked = true;
        self.loading = true;
        self.requests += 1;
        let rev = self.rev;
        let ticket = PageCache::try_global(cx).map(|c| c.ticket());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.home() })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_first(rev, result, ticket, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_first(
        &mut self,
        rev: u64,
        result: Result<HomePage, LoadFailure>,
        ticket: Option<Ticket>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.loading = false;
        match result {
            Ok(page) => {
                self.remember(ticket.as_ref(), &page.shelves, cx);
                self.failure = None;
                self.shelves = page.shelves.into_iter().map(Arc::new).collect();
                self.continuation = page.continuation;
                self.loaded_at = Some(Instant::now());
                self.from_cache = false;
                self.fresh_seen = true;
            }
            // Cached shelves are already on screen: only a dead session takes them away.
            Err(failure) if self.shows_cache() && failure != LoadFailure::Session => {}
            Err(failure) => {
                self.shelves.clear();
                self.continuation = None;
                self.from_cache = false;
                self.failure = Some(failure);
            }
        }
        cx.notify();
    }

    /// Fetches the first batch again without disturbing what is shown: the new one replaces it
    /// when it arrives, and a failure leaves it as it was.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.rev += 1;
        let rev = self.rev;
        self.refreshing = true;
        // A batch being appended belongs to the feed that is about to be replaced.
        self.more = More::Idle;
        self.requests += 1;
        let ticket = PageCache::try_global(cx).map(|c| c.ticket());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.home() })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_refresh(rev, result, ticket, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_refresh(
        &mut self,
        rev: u64,
        result: Result<HomePage, LoadFailure>,
        ticket: Option<Ticket>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.refreshing = false;
        // Failed or empty: keep the feed that is shown, and try again at the next reopening.
        // The exception is a dead session behind cached shelves: that is said out loud.
        match result {
            Ok(page) if !page.shelves.is_empty() => {
                self.remember(ticket.as_ref(), &page.shelves, cx);
                self.shelves = page.shelves.into_iter().map(Arc::new).collect();
                self.continuation = page.continuation;
                self.loaded_at = Some(Instant::now());
                self.from_cache = false;
                self.fresh_seen = true;
            }
            Err(LoadFailure::Session) if self.shows_cache() => {
                self.shelves.clear();
                self.continuation = None;
                self.from_cache = false;
                self.failure = Some(LoadFailure::Session);
            }
            _ => {}
        }
        cx.notify();
    }

    /// The next batch of shelves, appended below the last. A no-op while one is in flight, after
    /// a failure (use [`Self::retry_more`]), at the end of the feed and during a refresh.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.more != More::Idle || self.refreshing || self.loading || !self.asked {
            return;
        }
        self.start_more(cx);
    }

    pub fn retry_more(&mut self, cx: &mut Context<Self>) {
        if matches!(self.more, More::Failed(_)) {
            self.more = More::Idle;
            self.load_more(cx);
        }
    }

    fn start_more(&mut self, cx: &mut Context<Self>) {
        let (Some(source), Some(token)) = (self.source.clone(), self.continuation.clone()) else {
            return;
        };
        if !self.has_more() || self.failure.is_some() {
            return;
        }
        self.more = More::Loading;
        let rev = self.rev;
        cx.spawn(async move |this, cx| {
            let asked = token.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.home_more(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_more(rev, token, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_more(
        &mut self,
        rev: u64,
        token: String,
        result: Result<HomePage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        match result {
            Ok(page) => {
                let before = self.shelves.len();
                for shelf in page.shelves {
                    if !self.shelves.iter().any(|seen| same_shelf(seen, &shelf)) {
                        self.shelves.push(Arc::new(shelf));
                    }
                }
                // A batch that adds nothing, or that points back at the token it came from, ends
                // the feed: paging never loops.
                let progressed = self.shelves.len() > before;
                self.continuation = page
                    .continuation
                    .filter(|next| *next != token && progressed);
                self.more = More::Idle;
            }
            Err(failure) => self.more = More::Failed(failure),
        }
        cx.notify();
    }
}

/// Two shelves with the same title and the same items are one shelf the service sent twice.
fn same_shelf(a: &HomeShelf, b: &HomeShelf) -> bool {
    a.title == b.title && keys(a) == keys(b)
}

fn keys(shelf: &HomeShelf) -> HashSet<String> {
    shelf
        .items
        .iter()
        .map(|i| match i {
            HomeItem::Song(t) => format!("s:{}", t.track.video_id.as_str()),
            HomeItem::Collection(c) => format!("c:{}", c.id.as_str()),
            HomeItem::Artist(h) => format!("a:{}", h.id.as_str()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::library::{Collection, CollectionKind};
    use apollo_domain::model::BrowseId;

    fn shelf(title: &str, ids: &[&str]) -> HomeShelf {
        HomeShelf {
            title: title.into(),
            items: ids
                .iter()
                .map(|id| {
                    HomeItem::Collection(Collection {
                        kind: CollectionKind::Album,
                        id: BrowseId::new(*id),
                        title: (*id).into(),
                        subtitle: None,
                        thumbnail: None,
                    })
                })
                .collect(),
        }
    }

    #[test]
    fn a_shelf_sent_twice_is_one_shelf_but_a_new_row_with_the_same_title_is_not() {
        let a = shelf("Albums", &["x", "y"]);
        assert!(same_shelf(&a, &shelf("Albums", &["y", "x"])));
        assert!(!same_shelf(&a, &shelf("Albums", &["x", "z"])));
        assert!(!same_shelf(&a, &shelf("Singles", &["x", "y"])));
    }
}
