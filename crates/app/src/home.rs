//! Home: the account's YouTube Music feed as titled shelves, loaded once per session and kept in
//! memory. No views here; `ui::home_page` draws it.
//!
//! * The first batch loads as soon as the backend is installed and the account can serve it
//!   ([`Access::Open`]). Shelves after it come in on demand ([`HomeModel::load_more`]).
//! * Reopening Home after [`STALE_AFTER`] fetches the first batch again in the background; what
//!   is on screen stays until the new one arrives, and a failed refresh leaves it alone.
//! * A different account (or a sign-out) drops everything, and answers still in flight for the
//!   old one are discarded (`rev`).

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_domain::listing::LoadFailure;
use apollo_provider::{HomeItem, HomePage, HomeShelf};
use gpui::{Context, Entity};

use crate::library::LibrarySource;
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
        };
        model.try_start(cx);
        model
    }

    // ---- reading -----------------------------------------------------------------------

    pub fn phase(&self) -> Phase {
        match self.access {
            Access::SignedOut => Phase::SignedOut,
            Access::Expired => Phase::Failed(LoadFailure::Session),
            Access::Waiting => Phase::Loading,
            Access::Open => match (self.failure, self.asked && !self.loading) {
                (Some(f), _) => Phase::Failed(f),
                (None, true) => Phase::Ready,
                (None, false) => Phase::Loading,
            },
        }
    }

    pub fn shelves(&self) -> &[Arc<HomeShelf>] {
        &self.shelves
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
        if access != Access::Open || changed_account || !was_open {
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
    }

    /// The first batch, with the skeleton showing meanwhile.
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        if self.access != Access::Open {
            return;
        }
        self.clear();
        self.asked = true;
        self.loading = true;
        self.requests += 1;
        let rev = self.rev;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.home() })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_first(rev, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_first(
        &mut self,
        rev: u64,
        result: Result<HomePage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.loading = false;
        match result {
            Ok(page) => {
                self.failure = None;
                self.shelves = page.shelves.into_iter().map(Arc::new).collect();
                self.continuation = page.continuation;
                self.loaded_at = Some(Instant::now());
            }
            Err(failure) => self.failure = Some(failure),
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
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.home() })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_refresh(rev, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_refresh(
        &mut self,
        rev: u64,
        result: Result<HomePage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.refreshing = false;
        // Failed or empty: keep the feed that is shown, and try again at the next reopening.
        if let Ok(page) = result {
            if !page.shelves.is_empty() {
                self.shelves = page.shelves.into_iter().map(Arc::new).collect();
                self.continuation = page.continuation;
                self.loaded_at = Some(Instant::now());
            }
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
