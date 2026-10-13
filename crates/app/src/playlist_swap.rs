//! Swapping a playlist's songs for their explicit (or clean) versions: the matching half. The
//! page asks for it (More > Swap to explicit versions on a playlist of your own); this finds the
//! other version of each song with the rules the Crate uses ([`crate::crate_swap`]), at the
//! Crate's pace and stopping at the shared "too many requests" cooldown, and only once every song
//! has been looked up hands the result to the library, which changes the rows and writes the
//! playlist (see `LibraryModel::apply_swap`) in as few requests as the service allows. A song with
//! no match stays as it is, and every song keeps its place. The whole swap is one step for Ctrl+Z.
//!
//! The tip pill follows along: "Finding explicit versions… 3/40", then, once the service has the
//! swap, "Swapped 12 of 40 to explicit".

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_domain::library::Collection;
use apollo_domain::listing::LoadFailure;
use apollo_provider::{ListedTrack, SearchFilter};
use apollo_services_common::{Lane, Throttle};
use gpui::{App, Context, Entity, EventEmitter, Global};

use crate::crate_swap::{self, Pick, Want};
use crate::library::model::{LibraryHandle, SwapFound};
use crate::library::source::TrackRow;
use crate::library::writes::{LibraryWrites, WritesEvent};
use crate::search::SearchSource;

/// What a swap says to the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaylistSwapEvent {
    /// Looking up the other versions: `done` of `total` songs.
    Progress { want: Want, done: usize, total: usize },
    /// The swap is over. `swapped` rows took the place of another version; `total` rows were not
    /// the wanted version when it began.
    Finished {
        want: Want,
        swapped: usize,
        total: usize,
        already: usize,
        unreachable: bool,
        /// Stopped by the service's "too many requests".
        paused: bool,
    },
    /// The service did not take the swap.
    Failed,
    /// Asked to start while another swap was running.
    Busy,
    /// The playlist is longer than its page has read: reading the rest before looking.
    Reading,
}

impl EventEmitter<PlaylistSwapEvent> for PlaylistSwapModel {}

/// What starting a swap did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// This many songs (distinct) are being looked up.
    Started(usize),
    /// No row needs it, or the page cannot be swapped now (still loading, not editable).
    Nothing,
    Busy,
    /// The whole playlist is being read first.
    Reading,
    /// There is no search to ask yet (the backend is not ready).
    NoSearch,
    /// YouTube said it was busy a moment ago: searches wait.
    Paused,
}

/// The text of the pill when a swap is over. Like the Crate's, but a swap that landed says which
/// version: "Swapped 12 of 40 to explicit".
pub fn finished_text(
    want: Want,
    swapped: usize,
    total: usize,
    already: usize,
    unreachable: bool,
    paused: bool,
) -> String {
    if swapped > 0 && !paused {
        return format!("Swapped {swapped} of {total} to {}", want.word());
    }
    if swapped > 0 {
        return format!("Paused: YouTube is busy. Swapped {swapped} of {total} to {}", want.word());
    }
    crate_swap::finished_text(want, 0, total, already, unreachable, paused)
}

/// The rows of a playlist a swap to `want` looks at: the ones that are not that version yet, as
/// positions in `rows`.
pub fn target_rows(rows: &[TrackRow], want: Want) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.explicit != want.explicit())
        .map(|(at, _)| at)
        .collect()
}

/// A swap under way.
struct Run {
    id: u64,
    want: Want,
    collection: Collection,
    account: Option<String>,
    /// The playlist as it was when the swap began.
    rows: Vec<TrackRow>,
    /// Positions of the rows to swap.
    targets: Vec<usize>,
    /// Every video the playlist holds: a swap never makes it hold a song twice.
    held: HashSet<String>,
    /// Distinct songs still to look up, in the playlist's order.
    waiting: VecDeque<String>,
    total_videos: usize,
    done_videos: usize,
    /// Songs the search showed to be that version already.
    already: HashSet<String>,
    /// Searches that failed (the service could not be reached).
    errors: usize,
    /// (video it replaces, the version found).
    picks: HashMap<String, ListedTrack>,
    /// A task is working through `waiting`.
    runner: bool,
    /// The library has the result and the service has not answered yet: Ctrl+Z step to wait for.
    awaiting: Option<u64>,
    paused: bool,
}

pub struct PlaylistSwapModel {
    search: Option<Arc<dyn SearchSource>>,
    run: Option<Run>,
    serial: u64,
    /// How long to wait between one pair of searches and the next.
    pause: Duration,
    /// How long searches stop after YouTube says it is busy.
    cooldown: Duration,
    cooling_until: Option<Instant>,
    throttle: Throttle,
    /// The whole playlist is being read before the lookups begin.
    reading: bool,
}

/// The playlist swap, reachable from the track menu.
#[derive(Clone)]
pub struct PlaylistSwapHandle(pub Entity<PlaylistSwapModel>);

impl Global for PlaylistSwapHandle {}

impl Default for PlaylistSwapModel {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaylistSwapModel {
    pub fn new() -> Self {
        Self {
            search: None,
            run: None,
            serial: 0,
            pause: crate_swap::PAUSE,
            cooldown: crate_swap::COOLDOWN,
            cooling_until: None,
            throttle: Throttle::global(),
            reading: false,
        }
    }

    /// The search a swap asks (the backend is ready).
    pub fn install_search(&mut self, search: Arc<dyn SearchSource>) {
        self.search = Some(search);
    }

    /// How long to wait between one pair of searches and the next, and how long searches stop
    /// after the service says it is busy (tests).
    pub fn set_pacing(&mut self, pause: Duration, cooldown: Duration) {
        self.pause = pause;
        self.cooldown = cooldown;
    }

    /// Use `throttle` in place of the process's own (tests).
    pub fn set_throttle(&mut self, throttle: Throttle) {
        self.throttle = throttle;
    }

    /// A swap is running (looking up, or waiting for the service).
    pub fn running(&self) -> bool {
        self.run.is_some() || self.reading
    }

    /// Listen for the service's answer to the swap's write.
    pub fn follow(&mut self, writes: &Entity<LibraryWrites>, cx: &mut Context<Self>) {
        cx.subscribe(writes, |m, _, event, cx| {
            if let WritesEvent::PlaylistSwapSettled { step: Some(step), swapped, ok, .. } = event {
                m.write_settled(*step, *swapped, *ok, cx);
            }
        })
        .detach();
    }

    fn cooling(&self) -> bool {
        self.cooling_until.is_some_and(|t| Instant::now() < t)
            || self.throttle.blocked(Lane::Background).is_some()
    }

    /// Look for the `want` version of the songs of the playlist `playlist_key`, whose page is on
    /// top in the library, and swap in what is found. A long playlist whose page has not read
    /// every row yet is read whole first ("Reading the playlist…").
    pub fn start(&mut self, want: Want, playlist_key: &str, cx: &mut Context<Self>) -> Start {
        if self.run.is_some() || self.reading {
            cx.emit(PlaylistSwapEvent::Busy);
            return Start::Busy;
        }
        if self.search.is_none() {
            return Start::NoSearch;
        }
        let page = cx
            .try_global::<LibraryHandle>()
            .and_then(|h| h.0.read(cx).swap_page(playlist_key));
        let Some(page) = page else {
            cx.emit(finished(want, 0, 0, 0, false, false));
            return Start::Nothing;
        };
        if let Some(rows) = page.rows {
            return self.begin(want, page.collection, page.account, rows, cx);
        }
        self.reading = true;
        cx.emit(PlaylistSwapEvent::Reading);
        let (collection, account, source) = (page.collection, page.account, page.source);
        cx.spawn(async move |this, cx| {
            let read = {
                let collection = collection.clone();
                cx.background_executor()
                    .spawn(async move { crate::library::edits::all_rows(&*source, &collection) })
                    .await
            };
            let _ = this.update(cx, |m, cx| {
                m.reading = false;
                match read {
                    // A row with no token cannot be named to the service.
                    Ok(rows) if rows.iter().all(|r| r.occurrence.is_some()) => {
                        m.begin(want, collection, account, rows, cx);
                    }
                    Ok(_) | Err(_) => cx.emit(PlaylistSwapEvent::Failed),
                }
            });
        })
        .detach();
        Start::Reading
    }

    /// The playlist is read: look up the songs that are not the wanted version yet.
    fn begin(
        &mut self,
        want: Want,
        collection: Collection,
        account: Option<String>,
        rows: Vec<TrackRow>,
        cx: &mut Context<Self>,
    ) -> Start {
        let targets = target_rows(&rows, want);
        if targets.is_empty() {
            cx.emit(finished(want, 0, 0, 0, false, false));
            return Start::Nothing;
        }
        if self.cooling() {
            cx.emit(finished(want, 0, targets.len(), 0, false, true));
            return Start::Paused;
        }
        let mut waiting = VecDeque::new();
        let mut seen = HashSet::new();
        for &at in &targets {
            let id = rows[at].track.video_id.as_str();
            if seen.insert(id.to_owned()) {
                waiting.push_back(id.to_owned());
            }
        }
        let held = rows.iter().map(|r| r.track.video_id.as_str().to_owned()).collect();
        self.serial += 1;
        let total_videos = waiting.len();
        self.run = Some(Run {
            id: self.serial,
            want,
            collection,
            account,
            rows,
            targets,
            held,
            waiting,
            total_videos,
            done_videos: 0,
            already: HashSet::new(),
            errors: 0,
            picks: HashMap::new(),
            runner: false,
            awaiting: None,
            paused: false,
        });
        cx.emit(PlaylistSwapEvent::Progress { want, done: 0, total: total_videos });
        self.ensure_runner(cx);
        Start::Started(total_videos)
    }

    /// Start the task that works through the waiting songs, [`crate_swap::CONCURRENCY`] searches
    /// at a time with a pause between, if one is not running.
    fn ensure_runner(&mut self, cx: &mut Context<Self>) {
        let Some(source) = self.search.clone() else {
            return;
        };
        let Some(run) = self.run.as_mut().filter(|r| !r.runner && r.awaiting.is_none()) else {
            return;
        };
        run.runner = true;
        let id = run.id;
        cx.spawn(async move |this, cx| loop {
            let Ok(taken) = this.update(cx, |m, cx| m.take(id, cx)) else {
                return;
            };
            if taken.is_empty() {
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
            for ((video, track), task) in taken.into_iter().zip(tasks) {
                let found = task.await;
                let _ = this.update(cx, |m, cx| m.looked_up(id, &video, &track, found, cx));
            }
            // A small gap before the next pair, so a long playlist does not hammer the service.
            let wait = this
                .update(cx, |m, _| {
                    let more = m.run.as_ref().is_some_and(|r| r.id == id && !r.waiting.is_empty());
                    if more {
                        m.pause
                    } else {
                        Duration::ZERO
                    }
                })
                .unwrap_or(Duration::ZERO);
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
        })
        .detach();
    }

    /// The next songs to look up, or none when the swap is over (or must stop).
    fn take(&mut self, id: u64, cx: &mut Context<Self>) -> Vec<(String, apollo_domain::model::Track)> {
        let stopped = self.run.as_ref().is_none_or(|r| r.id != id);
        if stopped {
            return Vec::new();
        }
        // Something else of the app was told to slow down since the last pair: stop here.
        if self.run.as_ref().is_some_and(|r| !r.waiting.is_empty()) && self.cooling() {
            self.back_off(cx);
            return Vec::new();
        }
        let Some(run) = self.run.as_mut() else {
            return Vec::new();
        };
        let mut taken = Vec::new();
        while taken.len() < crate_swap::CONCURRENCY {
            let Some(video) = run.waiting.pop_front() else {
                break;
            };
            if let Some(row) = run.rows.iter().find(|r| r.track.video_id.as_str() == video) {
                taken.push((video, row.track.clone()));
            }
        }
        if taken.is_empty() {
            run.runner = false;
        }
        taken
    }

    /// A search came back.
    fn looked_up(
        &mut self,
        id: u64,
        video: &str,
        track: &apollo_domain::model::Track,
        found: Result<Vec<ListedTrack>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self
            .run
            .as_mut()
            .filter(|r| r.id == id && r.awaiting.is_none() && !r.paused)
        else {
            return;
        };
        match found {
            Err(LoadFailure::Throttled) => {
                self.back_off(cx);
                return;
            }
            Err(_) => run.errors += 1,
            Ok(songs) => {
                let chosen: HashSet<String> =
                    run.picks.values().map(|l| l.track.video_id.as_str().to_owned()).collect();
                let picked = crate_swap::pick(track, run.want, &songs, |candidate| {
                    run.held.contains(candidate) || chosen.contains(candidate)
                });
                match picked {
                    Pick::Swap(at) => {
                        run.picks.insert(video.to_owned(), songs[at].clone());
                    }
                    Pick::Already => {
                        run.already.insert(video.to_owned());
                    }
                    Pick::None => {}
                }
            }
        }
        run.done_videos += 1;
        let (want, done, total) = (run.want, run.done_videos, run.total_videos);
        if done < total {
            cx.emit(PlaylistSwapEvent::Progress { want, done, total });
            return;
        }
        self.finish_lookups(cx);
    }

    /// The service said "too many requests": every search stops for a while, and the swap goes on
    /// with what it had found.
    fn back_off(&mut self, cx: &mut Context<Self>) {
        self.cooling_until = Some(Instant::now() + self.cooldown);
        if let Some(run) = self.run.as_mut() {
            run.waiting.clear();
            run.paused = true;
            run.runner = false;
        }
        self.finish_lookups(cx);
    }

    /// Every song is looked up (or the lookups stopped): put what was found in the playlist.
    fn finish_lookups(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.run.as_mut().filter(|r| r.awaiting.is_none()) else {
            return;
        };
        let found: Vec<SwapFound> = run
            .targets
            .iter()
            .filter_map(|&at| {
                let old = &run.rows[at];
                let listed = run.picks.get(old.track.video_id.as_str())?;
                Some(SwapFound { old: old.clone(), new: TrackRow::from(listed.clone()) })
            })
            .collect();
        let (collection, account, want) = (run.collection.clone(), run.account.clone(), run.want);
        let applied = if found.is_empty() {
            None
        } else {
            let library = cx.try_global::<LibraryHandle>().map(|h| h.0.clone());
            library.and_then(|library| {
                library.update(cx, |m, cx| m.apply_swap(&collection, &account, found, want.explicit(), cx))
            })
        };
        match applied {
            // The service has to take it before the pill says so.
            Some(step) => {
                if let Some(run) = self.run.as_mut() {
                    run.awaiting = Some(step);
                }
            }
            None => self.finish(0, cx),
        }
    }

    /// The service answered the write of the swap this model is waiting for.
    fn write_settled(&mut self, step: u64, swapped: usize, ok: bool, cx: &mut Context<Self>) {
        if self.run.as_ref().and_then(|r| r.awaiting) != Some(step) {
            return;
        }
        if ok {
            self.finish(swapped, cx);
        } else {
            self.run = None;
            cx.emit(PlaylistSwapEvent::Failed);
        }
    }

    fn finish(&mut self, swapped: usize, cx: &mut Context<Self>) {
        let Some(run) = self.run.take() else {
            return;
        };
        let total = run.targets.len();
        let already = run
            .targets
            .iter()
            .filter(|&&at| run.already.contains(run.rows[at].track.video_id.as_str()))
            .count();
        let unreachable = !run.paused && run.errors == run.total_videos;
        cx.emit(finished(run.want, swapped, total, already, unreachable, run.paused));
    }
}

fn finished(
    want: Want,
    swapped: usize,
    total: usize,
    already: usize,
    unreachable: bool,
    paused: bool,
) -> PlaylistSwapEvent {
    PlaylistSwapEvent::Finished { want, swapped, total, already, unreachable, paused }
}

/// The swap model, once the shell has made it.
pub fn handle(cx: &App) -> Option<Entity<PlaylistSwapModel>> {
    cx.try_global::<PlaylistSwapHandle>().map(|h| h.0.clone())
}

/// Swap the songs of the playlist whose page is on top for their `want` version. The window's tip
/// pill follows along.
pub fn swap(want: Want, playlist_key: &str, cx: &mut App) -> Option<Start> {
    let model = handle(cx)?;
    Some(model.update(cx, |m, cx| m.start(want, playlist_key, cx)))
}

#[cfg(test)]
mod tests {
    use apollo_domain::model::Track;

    use super::*;

    fn row(id: &str, explicit: bool) -> TrackRow {
        TrackRow {
            track: Track::new(id, id),
            occurrence: Some(format!("T-{id}")),
            thumbnail: None,
            explicit,
            liked: None,
        }
    }

    #[test]
    fn a_swap_looks_at_the_rows_that_are_not_the_wanted_version_yet() {
        let rows = [row("a", false), row("b", true), row("c", false)];
        assert_eq!(target_rows(&rows, Want::Explicit), [0, 2]);
        assert_eq!(target_rows(&rows, Want::Clean), [1]);
        assert!(target_rows(&[], Want::Explicit).is_empty());
    }

    #[test]
    fn the_pill_says_which_version_a_landed_swap_made() {
        assert_eq!(
            finished_text(Want::Explicit, 12, 40, 0, false, false),
            "Swapped 12 of 40 to explicit"
        );
        assert_eq!(
            finished_text(Want::Clean, 3, 3, 0, false, false),
            "Swapped 3 of 3 to clean"
        );
        assert_eq!(
            finished_text(Want::Explicit, 5, 40, 0, false, true),
            "Paused: YouTube is busy. Swapped 5 of 40 to explicit"
        );
        assert_eq!(
            finished_text(Want::Explicit, 0, 40, 0, false, false),
            "No explicit versions found"
        );
        assert_eq!(finished_text(Want::Explicit, 0, 0, 0, false, false), "Nothing to swap");
        assert_eq!(
            finished_text(Want::Explicit, 0, 40, 0, false, true),
            "Paused: YouTube is busy"
        );
        assert_eq!(
            finished_text(Want::Explicit, 0, 4, 4, false, false),
            "Already explicit"
        );
    }
}
