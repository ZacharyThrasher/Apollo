//! Explore: new releases, the song chart and the mood and genre tiles, loaded once per session
//! and kept in memory. No views here; `ui::explore_page` draws it.
//!
//! * The page loads when it is first opened and the account can serve it ([`Access::Open`], the
//!   same word Home gets from the shell): four requests are not spent at startup on a page that
//!   may not be visited. After that it is kept for the session.
//! * Reopening Explore after [`STALE_AFTER`] fetches it again in the background; what is on
//!   screen stays until the new answer arrives, and a failed refresh leaves it alone.
//! * A mood tile opens a page of playlists, held here so Back from one of them (a playlist
//!   page opens over Explore) finds the mood page as it was left. Answers are kept per tile
//!   for the session.
//! * A different account (or a sign-out) drops everything, and answers still in flight for the
//!   old one are discarded (`rev`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use apollo_domain::library::Collection;
use apollo_domain::listing::LoadFailure;
use apollo_provider::{ExplorePage, MoodTile};
use gpui::{Context, Entity};

pub use crate::home::{Access, Phase, STALE_AFTER};
use crate::library::LibrarySource;
use crate::player::{PlayerModel, Status};

/// What a mood page is showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoodPhase {
    Loading,
    Ready(Arc<Vec<Collection>>),
    Failed(LoadFailure),
}

/// The mood tile whose playlists are open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoodOpen {
    pub title: String,
    pub params: String,
    pub phase: MoodPhase,
}

pub struct ExploreModel {
    player: Entity<PlayerModel>,
    source: Option<Arc<dyn LibrarySource>>,
    access: Access,
    /// The account epoch the page belongs to.
    epoch: u64,
    /// The page has been opened at least once: it loads (again) whenever it can.
    wanted: bool,
    /// `true` once the page has been asked for under the current account.
    asked: bool,
    loading: bool,
    failure: Option<LoadFailure>,
    page: Option<Arc<ExplorePage>>,
    loaded_at: Option<Instant>,
    refreshing: bool,
    mood: Option<MoodOpen>,
    /// Mood pages already fetched this session, by tile token.
    moods: HashMap<String, Arc<Vec<Collection>>>,
    /// Bumped by every reset, load and refresh; an answer for an older one is dropped.
    rev: u64,
    /// Bumped by every mood open; an answer for an older one is dropped.
    mood_rev: u64,
    /// Explore asks the source this many times (tests and diagnostics).
    requests: u32,
}

impl ExploreModel {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |m, _, cx| m.try_start(cx)).detach();
        let mut model = Self {
            player,
            source: None,
            access: Access::Waiting,
            epoch: 0,
            wanted: false,
            asked: false,
            loading: false,
            failure: None,
            page: None,
            loaded_at: None,
            refreshing: false,
            mood: None,
            moods: HashMap::new(),
            rev: 0,
            mood_rev: 0,
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

    pub fn page(&self) -> Option<&Arc<ExplorePage>> {
        self.page.as_ref()
    }

    /// The mood page that is open, if one is.
    pub fn mood(&self) -> Option<&MoodOpen> {
        self.mood.as_ref()
    }

    pub fn is_refreshing(&self) -> bool {
        self.refreshing
    }

    pub fn loaded_at(&self) -> Option<Instant> {
        self.loaded_at
    }

    /// How many times the source has been asked for the page (first loads and refreshes).
    pub fn requests(&self) -> u32 {
        self.requests
    }

    /// Whether what is on screen is older than [`STALE_AFTER`] at `now`.
    pub fn is_stale(&self, now: Instant) -> bool {
        self.loaded_at
            .is_some_and(|at| now.saturating_duration_since(at) >= STALE_AFTER)
    }

    // ---- loading -----------------------------------------------------------------------

    /// Picks up the backend's source once the player has one, and loads the page. Called on
    /// every player change; does nothing once a source is held.
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
                if self.wanted {
                    self.load(cx);
                }
            }
            None if unavailable && self.wanted && self.failure.is_none() => {
                self.failure = Some(LoadFailure::Other);
                self.asked = true;
                cx.notify();
            }
            None => {}
        }
    }

    /// The shell's word on whether the account can serve the page, with the epoch of the
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
        if access == Access::Open && self.wanted {
            self.load(cx);
        }
        cx.notify();
    }

    /// Explore was opened: the first time loads the page, a failure is tried again, and a page
    /// older than [`STALE_AFTER`] is fetched again in the background.
    pub fn show(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.wanted = true;
        if self.source.is_none() || self.access != Access::Open || self.loading {
            // Not yet: `try_start` or `set_access` loads it once it can.
            self.try_start(cx);
            return;
        }
        if !self.asked || self.failure.is_some() {
            self.load(cx);
        } else if self.is_stale(now) && !self.refreshing {
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
        self.mood_rev += 1;
        self.asked = false;
        self.loading = false;
        self.refreshing = false;
        self.failure = None;
        self.page = None;
        self.loaded_at = None;
        self.mood = None;
        self.moods.clear();
    }

    /// The page, with the skeleton showing meanwhile.
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
                .spawn(async move { source.explore() })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_first(rev, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_first(
        &mut self,
        rev: u64,
        result: Result<ExplorePage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.loading = false;
        match result {
            Ok(page) => {
                self.failure = None;
                self.page = Some(Arc::new(page));
                self.loaded_at = Some(Instant::now());
            }
            Err(failure) => self.failure = Some(failure),
        }
        cx.notify();
    }

    /// Fetches the page again without disturbing what is shown: the new one replaces it when it
    /// arrives, and a failure leaves it as it was.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.rev += 1;
        let rev = self.rev;
        self.refreshing = true;
        self.requests += 1;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.explore() })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_refresh(rev, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_refresh(
        &mut self,
        rev: u64,
        result: Result<ExplorePage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.refreshing = false;
        // Failed or empty: keep the page that is shown, and try again at the next reopening.
        if let Ok(page) = result {
            if !page.is_empty() {
                self.page = Some(Arc::new(page));
                self.loaded_at = Some(Instant::now());
            }
        }
        cx.notify();
    }

    // ---- moods -------------------------------------------------------------------------

    /// Opens the page of playlists for a mood or genre tile.
    pub fn open_mood(&mut self, tile: &MoodTile, cx: &mut Context<Self>) {
        self.mood_rev += 1;
        let phase = match self.moods.get(&tile.params) {
            Some(found) => MoodPhase::Ready(found.clone()),
            None => MoodPhase::Loading,
        };
        let loading = phase == MoodPhase::Loading;
        self.mood = Some(MoodOpen {
            title: tile.title.clone(),
            params: tile.params.clone(),
            phase,
        });
        if loading {
            self.fetch_mood(tile.params.clone(), cx);
        }
        cx.notify();
    }

    /// Asks for the open mood page again after a failure.
    pub fn retry_mood(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.mood.as_mut() else {
            return;
        };
        if !matches!(open.phase, MoodPhase::Failed(_)) {
            return;
        }
        open.phase = MoodPhase::Loading;
        let params = open.params.clone();
        self.mood_rev += 1;
        self.fetch_mood(params, cx);
        cx.notify();
    }

    /// Back from a mood page to the tiles.
    pub fn close_mood(&mut self, cx: &mut Context<Self>) {
        if self.mood.take().is_some() {
            self.mood_rev += 1;
            cx.notify();
        }
    }

    fn fetch_mood(&mut self, params: String, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            if let Some(open) = self.mood.as_mut() {
                open.phase = MoodPhase::Failed(LoadFailure::Other);
            }
            return;
        };
        let rev = self.mood_rev;
        cx.spawn(async move |this, cx| {
            let asked = params.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.mood_playlists(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_mood(rev, params, result, cx));
        })
        .detach();
    }

    fn finish_mood(
        &mut self,
        rev: u64,
        params: String,
        result: Result<Vec<Collection>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.mood_rev {
            return;
        }
        let Some(open) = self.mood.as_mut().filter(|o| o.params == params) else {
            return;
        };
        open.phase = match result {
            Ok(found) => {
                let found = Arc::new(found);
                self.moods.insert(params, found.clone());
                MoodPhase::Ready(found)
            }
            Err(failure) => MoodPhase::Failed(failure),
        };
        cx.notify();
    }
}
