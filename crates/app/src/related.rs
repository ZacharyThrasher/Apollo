//! Related: what YouTube Music shows on a track's Related tab (related songs, then album and
//! artist shelves), for the track that is playing.
//!
//! * Nothing is fetched until the pane is on screen ([`RelatedModel::set_active`]); after that
//!   every change of the playing track asks again.
//! * What is on screen stays until the new answer arrives. A failed answer replaces it with the
//!   failure, because the old shelves belong to another track.
//! * Answers for a track that is no longer playing are discarded (`rev`).
//! * A request follows another by at least [`SETTLE`] (the first goes out at once), and waits for
//!   YouTube's cooldown when one is running, so skipping through songs (a held Shift+Right, Skim)
//!   asks for the song that was landed on, not once per song passed.

use std::sync::Arc;

use apollo_domain::listing::LoadFailure;
use apollo_provider::HomeShelf;
use std::time::{Duration, Instant};

use apollo_services_common::{Lane, Throttle};
use gpui::{Context, Entity};

use crate::library::LibrarySource;
use crate::player::PlayerModel;

/// How long the playing track must stay the same before its Related tab is asked for.
pub const SETTLE: Duration = Duration::from_millis(600);

/// Spaces out lookups that follow the playing track: the first goes out at once, one that comes
/// within [`SETTLE`] of the last waits that long, and a waiting one is replaced by a newer.
#[derive(Default)]
pub struct Settle {
    last: Option<Instant>,
}

impl Settle {
    /// How long a lookup starting now waits (zero for the first), and starts the clock.
    pub fn delay(&mut self) -> Duration {
        let now = Instant::now();
        let wait = match self.last {
            Some(at) => SETTLE.saturating_sub(now.duration_since(at)),
            None => Duration::ZERO,
        };
        self.last = Some(now + wait);
        wait
    }
}

/// What the pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is playing.
    Idle,
    /// The first answer is on its way: the skeleton.
    Loading,
    Ready,
    Failed(LoadFailure),
}

pub struct RelatedModel {
    player: Entity<PlayerModel>,
    source: Option<Arc<dyn LibrarySource>>,
    /// The Related pane is on screen.
    active: bool,
    shelves: Vec<Arc<HomeShelf>>,
    /// The track the shelves belong to.
    shown_for: Option<String>,
    /// The track last asked about.
    asked: Option<String>,
    loading: bool,
    failure: Option<LoadFailure>,
    /// Bumped by every request and reset; an answer for an older one is dropped.
    rev: u64,
    /// The source has been asked this many times (tests and diagnostics).
    requests: u32,
    settle: Settle,
}

impl RelatedModel {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |m, _, cx| m.sync(cx)).detach();
        Self {
            player,
            source: None,
            active: false,
            shelves: Vec::new(),
            shown_for: None,
            asked: None,
            loading: false,
            failure: None,
            rev: 0,
            requests: 0,
            settle: Settle::default(),
        }
    }

    // ---- reading -----------------------------------------------------------------------

    pub fn phase(&self) -> Phase {
        if let Some(f) = self.failure {
            return Phase::Failed(f);
        }
        if self.shown_for.is_some() {
            return Phase::Ready;
        }
        if self.loading || self.asked.is_some() {
            return Phase::Loading;
        }
        Phase::Idle
    }

    pub fn shelves(&self) -> &[Arc<HomeShelf>] {
        &self.shelves
    }

    /// The track the shelves on screen belong to.
    pub fn shown_for(&self) -> Option<&str> {
        self.shown_for.as_deref()
    }

    /// A newer answer is on its way while the previous one stays on screen.
    pub fn is_refreshing(&self) -> bool {
        self.loading && self.shown_for.is_some()
    }

    pub fn requests(&self) -> u32 {
        self.requests
    }

    // ---- loading -----------------------------------------------------------------------

    /// The pane appeared or went away. Appearing fetches for the playing track if need be.
    pub fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.sync(cx);
    }

    /// Try again after a failure.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.asked = None;
        self.failure = None;
        self.sync(cx);
        cx.notify();
    }

    /// Asks for the playing track's Related tab when the pane is on screen and what is held is
    /// for another track. A cheap no-op otherwise: it runs on every player change.
    fn sync(&mut self, cx: &mut Context<Self>) {
        if !self.active {
            return;
        }
        if self.source.is_none() {
            self.source = self.player.read(cx).library_source();
        }
        let current = self.player.read(cx).current_video();
        if current == self.asked {
            return;
        }
        let Some(video) = current else {
            // Nothing is playing any more: nothing to relate to.
            self.rev += 1;
            self.asked = None;
            self.shown_for = None;
            self.shelves.clear();
            self.loading = false;
            self.failure = None;
            cx.notify();
            return;
        };
        let Some(source) = self.source.clone() else {
            return;
        };
        self.rev += 1;
        let rev = self.rev;
        self.asked = Some(video.clone());
        self.loading = true;
        self.failure = None;
        let spaced = self.settle.delay();
        let wait = Throttle::global()
            .blocked(Lane::Background)
            .map_or(spaced, |cooldown| cooldown.max(spaced));
        cx.spawn(async move |this, cx| {
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            // Another track (or the pane going away) in the meantime: that one asks, not this.
            let current = this
                .update(cx, |m, _| {
                    let current = m.rev == rev;
                    if current {
                        m.requests += 1;
                    }
                    current
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let asked = video.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.related(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish(rev, video, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish(
        &mut self,
        rev: u64,
        video: String,
        result: Result<Vec<HomeShelf>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.loading = false;
        match result {
            Ok(shelves) => {
                self.failure = None;
                self.shelves = shelves
                    .into_iter()
                    .filter(|s| !s.items.is_empty())
                    .map(Arc::new)
                    .collect();
                self.shown_for = Some(video);
            }
            Err(failure) => {
                self.failure = Some(failure);
                self.shelves.clear();
                self.shown_for = None;
            }
        }
        cx.notify();
    }
}
