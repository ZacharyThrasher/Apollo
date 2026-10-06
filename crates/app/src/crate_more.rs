//! More like the Crate: songs like the ones set aside, shown under the Crate's list on the page
//! (ten rows) and in the docked panel (five), each with a + that sets it aside too.
//!
//! The songs are the radio of one of the Crate's own songs (the same feed Start radio and More like
//! this use), and the seed rotates through the Crate so each set comes from somewhere else in it.
//! What is on offer is cleaned when it is drawn: songs already in the Crate, already liked, or
//! already in the target playlist (as far as this session knows) are left out, so a song you add
//! or like leaves the rail at once. Blocked artists' songs never come in.
//!
//! YouTube is asked as little as can be, because the Crate is where long sessions pile up
//! requests (see ADR 0005):
//!
//! * One radio request per fetch. Nothing is fetched before the Crate page or the docked panel has
//!   been on screen (never at launch), never on a timer and never on hover.
//! * A new set is fetched on its own only when the Crate has changed by [`REFRESH_AFTER`] songs
//!   (added or removed) since the last fetch. The first fetch happens as soon as the rail is
//!   open and the Crate has a song.
//! * Refresh shows the next songs of what was fetched, and asks for another radio only when that
//!   runs out: one request per click at most.
//! * While the process's shared [`Throttle`] says YouTube is busy nothing is asked (the background
//!   lane for the automatic fetches, the foreground lane for a click) and the rail says nothing
//!   about it. A 429 answer is quiet too.
//! * Rail rows are never resolved or prefetched; a song is looked up only when played.

use std::collections::HashSet;

use apollo_domain::blocklist::retain_allowed;
use apollo_domain::library::{bare_playlist_id, CollectionKind, PlaylistTarget};
use apollo_domain::listing::LoadFailure;
use apollo_services_common::{Lane, Throttle};
use gpui::{App, Context, Entity, Global};

use crate::crate_list::CrateModel;
use crate::library::source::TrackRow;
use crate::library::LibraryWrites;
use crate::more_like::seed_index;
use crate::player::PlayerModel;

/// Songs the page's rail shows at once.
pub const PAGE_ROWS: usize = 10;
/// Songs the docked panel's rail shows at once.
pub const DOCK_ROWS: usize = 5;
/// How many songs the Crate must have gained or lost since the last fetch for the rail to fetch
/// a new set without being asked.
pub const REFRESH_AFTER: usize = 3;
/// Songs of one radio kept (a radio answers with fewer than this).
const POOL_MAX: usize = 60;

/// What the rail shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing: switched off, nothing in the Crate, or no fetch has been possible yet.
    Hidden,
    /// The first set is on its way: the skeleton.
    Loading,
    Ready,
    /// A title and Refresh only: the last set is used up, or YouTube is busy (said nowhere).
    Idle,
    Failed(LoadFailure),
}

pub struct CrateMoreModel {
    player: Entity<PlayerModel>,
    crate_: Entity<CrateModel>,
    writes: Entity<LibraryWrites>,
    /// Settings > Playlists > More like the Crate.
    enabled: bool,
    /// The Crate page or the docked panel has been drawn: fetching may start.
    opened: bool,
    /// The last radio, cleaned of blocked artists and of songs the user passed.
    pool: Vec<TrackRow>,
    /// Where the next set starts in `pool`.
    offset: usize,
    /// Songs Refresh has shown and moved past: not offered again.
    passed: HashSet<String>,
    /// The Crate's songs as of the last fetch (`None`: nothing fetched yet).
    baseline: Option<HashSet<String>>,
    /// Radios asked for: which song seeds the next.
    asked: usize,
    loading: bool,
    failure: Option<LoadFailure>,
    /// YouTube said it is busy (or the shared throttle did): the rail stays quiet about it.
    quiet: bool,
    /// Bumped by every reset; an answer for an older one is dropped.
    rev: u64,
    /// The source has been asked this many times (tests and diagnostics).
    requests: u32,
    /// The process's shared answer to "is YouTube busy" (tests use their own).
    throttle: Throttle,
}

/// The shell's rail model, reachable from the Crate page and the dock.
#[derive(Clone)]
pub struct CrateMoreHandle(pub Entity<CrateMoreModel>);

impl Global for CrateMoreHandle {}

pub fn handle(cx: &App) -> Option<Entity<CrateMoreModel>> {
    cx.try_global::<CrateMoreHandle>().map(|h| h.0.clone())
}

/// How many songs differ between two sets of the Crate's songs (added plus removed).
pub fn changed_by(before: &HashSet<String>, now: &HashSet<String>) -> usize {
    before.difference(now).count() + now.difference(before).count()
}

impl CrateMoreModel {
    pub fn new(
        player: Entity<PlayerModel>,
        crate_: Entity<CrateModel>,
        writes: Entity<LibraryWrites>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&crate_, |m, _, cx| m.crate_changed(cx)).detach();
        // A like takes a song off the rail.
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        // The backend may come up after the page: the first fetch waits for it, and only until
        // it has happened.
        cx.observe(&player, |m, _, cx| {
            if m.baseline.is_none() {
                m.evaluate(cx);
            }
        })
        .detach();
        Self {
            player,
            crate_,
            writes,
            enabled: true,
            opened: false,
            pool: Vec::new(),
            offset: 0,
            passed: HashSet::new(),
            baseline: None,
            asked: 0,
            loading: false,
            failure: None,
            quiet: false,
            rev: 0,
            requests: 0,
            throttle: Throttle::global(),
        }
    }

    // ---- reading -----------------------------------------------------------------------

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_open(&self) -> bool {
        self.opened
    }

    /// The source has been asked this many times.
    pub fn requests(&self) -> u32 {
        self.requests
    }

    /// A newer set is on its way while the previous one stays on screen.
    pub fn is_refreshing(&self) -> bool {
        self.loading && !self.pool.is_empty()
    }

    /// The songs on offer, at most `limit`: what was fetched from `offset` on, less the songs the
    /// Crate holds, the account has liked, or the target playlist is known to hold.
    pub fn visible(&self, limit: usize, cx: &App) -> Vec<TrackRow> {
        let crate_ = self.crate_.read(cx);
        let writes = self.writes.read(cx);
        let target = known_target(cx);
        self.pool[self.offset.min(self.pool.len())..]
            .iter()
            .filter(|r| {
                let id = r.track.video_id.as_str();
                !crate_.contains(id)
                    && !writes.is_liked(id)
                    && !target.as_ref().is_some_and(|t| t.holds(id, writes, cx))
            })
            .take(limit)
            .cloned()
            .collect()
    }

    /// What the rail shows.
    pub fn phase(&self, cx: &App) -> Phase {
        if !self.enabled || self.crate_.read(cx).is_empty() {
            return Phase::Hidden;
        }
        if !self.visible(1, cx).is_empty() {
            return Phase::Ready;
        }
        if self.loading {
            return Phase::Loading;
        }
        if self.quiet {
            return Phase::Idle;
        }
        if let Some(failure) = self.failure {
            return Phase::Failed(failure);
        }
        if self.baseline.is_some() {
            return Phase::Idle;
        }
        Phase::Hidden
    }

    // ---- loading -----------------------------------------------------------------------

    /// Settings > Playlists > More like the Crate.
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if enabled {
            self.evaluate(cx);
        }
        cx.notify();
    }

    /// Use `throttle` in place of the process's own (tests).
    pub fn set_throttle(&mut self, throttle: Throttle) {
        self.throttle = throttle;
    }

    /// The Crate page or the docked panel is on screen: from now on the rail may fetch. Safe to
    /// call on every draw.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        if self.opened {
            return;
        }
        self.opened = true;
        self.evaluate(cx);
        cx.notify();
    }

    /// The Crate changed: forget everything when it is empty, else fetch a new set once enough
    /// of it has changed.
    fn crate_changed(&mut self, cx: &mut Context<Self>) {
        if self.crate_.read(cx).is_empty() {
            self.reset();
        } else {
            self.evaluate(cx);
        }
        cx.notify();
    }

    /// Another Crate (or none): nothing fetched for the old one is worth showing.
    fn reset(&mut self) {
        self.rev += 1;
        self.pool.clear();
        self.offset = 0;
        self.passed.clear();
        self.baseline = None;
        self.asked = 0;
        self.loading = false;
        self.failure = None;
        self.quiet = false;
    }

    /// Fetches a new set on its own when the rail is open, there is a song to seed it, and
    /// either nothing has been fetched yet or the Crate has changed enough since.
    fn evaluate(&mut self, cx: &mut Context<Self>) {
        if !self.enabled || !self.opened || self.loading {
            return;
        }
        let now = self.crate_ids(cx);
        if now.is_empty() {
            return;
        }
        let due = match &self.baseline {
            None => true,
            Some(before) => changed_by(before, &now) >= REFRESH_AFTER,
        };
        if due {
            self.fetch(false, cx);
        }
    }

    fn crate_ids(&self, cx: &App) -> HashSet<String> {
        self.crate_
            .read(cx)
            .items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    }

    /// Show another set: the next songs of what was fetched, else the radio of another song.
    /// `shown` is how many rows the caller draws.
    pub fn refresh(&mut self, shown: usize, cx: &mut Context<Self>) {
        if !self.enabled || self.loading {
            return;
        }
        let rest = self.visible(usize::MAX, cx);
        if rest.len() > shown {
            // Move past the songs on screen: they were seen, so they are not offered again.
            let last = rest[shown - 1].track.video_id.as_str().to_owned();
            for row in &rest[..shown] {
                self.passed.insert(row.track.video_id.as_str().to_owned());
            }
            if let Some(at) = self.pool.iter().position(|r| r.track.video_id.as_str() == last) {
                self.offset = at + 1;
            }
            cx.notify();
            return;
        }
        self.fetch(true, cx);
    }

    /// Try again after a failure.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if !self.loading {
            self.failure = None;
            self.fetch(true, cx);
        }
    }

    /// Asks for the radio of the next seed song of the Crate. `manual`: a click, which waits for
    /// the foreground lane only; the automatic fetches wait for the background lane.
    fn fetch(&mut self, manual: bool, cx: &mut Context<Self>) {
        if !self.enabled || !self.opened || self.loading {
            return;
        }
        let ids: Vec<String> = self
            .crate_
            .read(cx)
            .items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect();
        if ids.is_empty() {
            return;
        }
        let lane = if manual { Lane::Foreground } else { Lane::Background };
        if self.throttle.blocked(lane).is_some() {
            // YouTube asked us to slow down: nothing is asked and nothing is said. The next
            // change of the Crate, the next open or the next click looks again.
            self.quiet = true;
            cx.notify();
            return;
        }
        // The backend may not be up yet; the player's own notifications look again.
        let Some(source) = self.player.read(cx).library_source() else {
            return;
        };
        let seed = ids[seed_index(self.asked, ids.len())].clone();
        self.asked += 1;
        self.requests += 1;
        self.loading = true;
        self.quiet = false;
        self.failure = None;
        self.baseline = Some(ids.into_iter().collect());
        let rev = self.rev;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.radio(&seed) })
                .await;
            let _ = this.update(cx, |m, cx| m.arrived(rev, result, cx));
        })
        .detach();
    }

    fn arrived(
        &mut self,
        rev: u64,
        result: Result<Vec<TrackRow>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.loading = false;
        match result {
            Ok(rows) => {
                // A blocked artist's songs are not offered, as in a radio.
                let blocked = crate::blocked_artists::snapshot(cx);
                let rows = retain_allowed(rows, &blocked, |r| &r.track);
                let mut seen = HashSet::new();
                let fresh: Vec<TrackRow> = rows
                    .into_iter()
                    .filter(|r| {
                        let id = r.track.video_id.as_str();
                        !self.passed.contains(id) && seen.insert(id.to_owned())
                    })
                    .take(POOL_MAX)
                    .collect();
                // An answer with nothing new leaves what is shown alone.
                if !fresh.is_empty() {
                    self.pool = fresh;
                    self.offset = 0;
                }
                self.failure = None;
                self.quiet = false;
            }
            Err(LoadFailure::Throttled) => self.quiet = true,
            Err(failure) => self.failure = Some(failure),
        }
        cx.notify();
    }

    // ---- adding ------------------------------------------------------------------------

    /// Set a suggested song aside, with the usual pill. It leaves the rail by itself: it is in
    /// the Crate now. Whether it went in.
    pub fn add(&mut self, video_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(row) = self
            .pool
            .iter()
            .find(|r| r.track.video_id.as_str() == video_id)
            .cloned()
        else {
            return false;
        };
        crate::crate_list::add_rows(vec![(row.track, row.thumbnail)], false, cx)
            .is_some_and(|added| added.new > 0)
    }
}

/// The target playlist and what is known of its songs.
struct Target {
    playlist: PlaylistTarget,
    /// Its songs, when the Library has its page open with every row loaded.
    open_rows: Option<HashSet<String>>,
}

impl Target {
    fn holds(&self, video_id: &str, writes: &LibraryWrites, _cx: &App) -> bool {
        writes.added_here(&self.playlist, video_id)
            || self.open_rows.as_ref().is_some_and(|rows| rows.contains(video_id))
    }
}

/// The target playlist (A adds to it), if one is chosen. What it holds is known from what was
/// added to it this session and from its page, while that is open in the Library.
fn known_target(cx: &App) -> Option<Target> {
    let target = cx
        .try_global::<crate::target_playlist::TargetHandle>()?
        .0
        .read(cx)
        .target(cx)?;
    let playlist = target.as_playlist();
    let open_rows = cx
        .try_global::<crate::library::model::LibraryHandle>()
        .and_then(|h| {
            let model = h.0.read(cx);
            let d = model.detail()?;
            let same = d.collection.kind == CollectionKind::Playlist
                && bare_playlist_id(d.collection.library_key()) == playlist.id;
            same.then(|| {
                d.listing
                    .items()
                    .iter()
                    .map(|r| r.track.video_id.as_str().to_owned())
                    .collect()
            })
        });
    Some(Target { playlist, open_rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn added_and_removed_songs_both_count_as_changes() {
        let before = set(&["a", "b", "c"]);
        assert_eq!(changed_by(&before, &before), 0);
        assert_eq!(changed_by(&before, &set(&["a", "b", "c", "d"])), 1);
        assert_eq!(changed_by(&before, &set(&["a", "b"])), 1);
        assert_eq!(changed_by(&before, &set(&["a", "x", "y"])), 4);
        // A move is not a change.
        assert_eq!(changed_by(&before, &set(&["c", "b", "a"])), 0);
    }

    #[test]
    fn three_changes_are_enough() {
        let before = set(&["a", "b"]);
        assert!(changed_by(&before, &set(&["a", "b", "c", "d"])) < REFRESH_AFTER);
        assert!(changed_by(&before, &set(&["a", "b", "c", "d", "e"])) >= REFRESH_AFTER);
    }
}
