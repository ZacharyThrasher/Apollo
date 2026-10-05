//! The Crate's add box: "Add by search" and "Paste to add". The state and the lookups, no views;
//! `ui::crate_add` draws the field and the popover under it.
//!
//! **Typing** searches songs after a pause of [`DEBOUNCE`] (a word typed again costs nothing: the
//! answers of the last queries are remembered). The first [`MAX_RESULTS`] songs show; Up and Down
//! move through them and Enter adds the one the keyboard is on. Nothing is prefetched for them.
//!
//! **Pasting** ([`CrateAddModel::paste`]) reads the text line by line (`crate_paste`): a song
//! link adds that song, a playlist or album link adds its songs, and a plain line ("Artist -
//! Title") is one search for the best song hit. One line pasted onto the Crate is added at once;
//! more than one opens a review list first: every line with what it matched, a checkbox (on) and
//! "Add 14". Lines that match nothing say so and are skipped.
//!
//! **Rate limits.** Everything asks YouTube through the same shared cooldown as the rest of the
//! app: lookups run [`CONCURRENCY`] at a time with [`PAUSE`] between, a "too many requests" stops
//! them all for [`COOLDOWN`] ("Paused: YouTube is busy"), and no lookup starts while the app's
//! shared throttle says to wait. One paste adds at most [`crate_paste::MAX_LINES`] lines and
//! [`MAX_SONGS_PER_PASTE`] songs.
//!
//! Whatever is added goes in at the bottom as one step for Ctrl+Z.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::links::ExternalLink;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::{BrowseId, Track};
use apollo_provider::{LinkTarget, ListedTrack, SearchFilter, SearchResults};
use apollo_services_common::Lane;
use gpui::{Context, Entity, EventEmitter};

use crate::confirm;
use crate::crate_list::{CrateModel, NewSong};
use crate::crate_paste::{self, Kind};
use crate::crate_swap;
use crate::library::LibrarySource;
use crate::player::PlayerModel;
use crate::search::{as_link, normalise, SearchSource};

/// Typing pauses this long before a search goes out.
pub const DEBOUNCE: Duration = Duration::from_millis(300);
/// Typing shorter than this never searches.
pub const MIN_CHARS: usize = 2;
/// Songs the popover lists.
pub const MAX_RESULTS: usize = 6;
/// Lookups of a paste that run at once.
pub const CONCURRENCY: usize = 2;
/// The wait between one pair of lookups and the next (and between a playlist's pages).
pub const PAUSE: Duration = Duration::from_millis(400);
/// How long every lookup stays stopped once YouTube says it is busy.
pub const COOLDOWN: Duration = Duration::from_secs(120);
/// Songs one paste adds at most (a long playlist is cut there).
pub const MAX_SONGS_PER_PASTE: usize = 500;
/// Pages of one playlist or album a paste follows.
const MAX_PAGES: usize = 10;
/// Search answers remembered.
const CACHE: usize = 32;
/// Songs of a search a pasted line is matched among.
const MATCH_AMONG: usize = 10;

/// What the popover has to say when it has no songs to list.
pub const BUSY_TEXT: &str = "Paused: YouTube is busy";
pub const NO_RESULTS_TEXT: &str = "No songs found";

/// Where the typed search stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Idle,
    Searching,
    NoResults,
    Failed(LoadFailure),
    /// YouTube said it is busy (or the shared cooldown runs): nothing is asked.
    Paused,
}

/// A song a lookup found, with what the list it came from knew.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub track: Track,
    pub thumbnail: Option<String>,
    pub explicit: bool,
}

impl From<&ListedTrack> for Found {
    fn from(listed: &ListedTrack) -> Self {
        crate::explicit_marks::note(listed.track.video_id.as_str(), listed.explicit);
        Self {
            track: listed.track.clone(),
            thumbnail: listed.thumbnail.clone(),
            explicit: listed.explicit,
        }
    }
}

/// How a pasted line stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineState {
    /// Not looked up yet.
    Waiting,
    Found,
    /// Looked up and nothing fits (or a link that is not a song, playlist or album).
    NoMatch,
    /// The lookup could not be made (no connection, signed out).
    Failed,
    /// Stopped by "too many requests" before it was looked up.
    Paused,
}

/// One line of a paste and what it came to.
#[derive(Clone, Debug)]
pub struct ReviewLine {
    /// What was pasted, as the review shows it.
    pub raw: String,
    kind: Kind,
    pub state: LineState,
    /// One song for a song link or a text line; many for a playlist or album.
    pub songs: Vec<Found>,
    /// "Playlist" or "Album" for a link to one.
    pub group: Option<&'static str>,
    /// The checkbox.
    pub picked: bool,
}

impl ReviewLine {
    fn new(line: crate_paste::Line) -> Self {
        let group = match &line.kind {
            Kind::Link(ExternalLink::Playlist(_)) => Some("Playlist"),
            Kind::Link(ExternalLink::Album(_)) => Some("Album"),
            _ => None,
        };
        Self {
            raw: line.raw,
            kind: line.kind,
            state: LineState::Waiting,
            songs: Vec::new(),
            group,
            picked: true,
        }
    }

    /// The line's answer as the review words it: the song's title, "Album · 12 songs", or why
    /// there is none.
    pub fn title(&self) -> String {
        match (self.state, self.songs.as_slice()) {
            (LineState::Found, [one]) if self.group.is_none() => one.track.title.clone(),
            (LineState::Found, many) => format!(
                "{} \u{b7} {}",
                self.group.unwrap_or("Songs"),
                match many.len() {
                    1 => "1 song".to_owned(),
                    n => format!("{n} songs"),
                }
            ),
            (LineState::NoMatch, _) => "No match".to_owned(),
            (LineState::Failed, _) => "Couldn't look it up".to_owned(),
            (LineState::Paused, _) => "Not looked up".to_owned(),
            (LineState::Waiting, _) => "Looking it up\u{2026}".to_owned(),
        }
    }

    /// The artists of a single song, for the line under its title.
    pub fn artists(&self) -> String {
        match (self.state, self.songs.as_slice()) {
            (LineState::Found, [one]) if self.group.is_none() => one
                .track
                .artists
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        }
    }

    /// Whether the checkbox means anything: only a line that found songs can be added.
    pub fn can_pick(&self) -> bool {
        self.state == LineState::Found && !self.songs.is_empty()
    }
}

/// A paste being looked up, and the review list for it when it has several lines.
#[derive(Debug)]
pub struct Review {
    pub lines: Vec<ReviewLine>,
    /// Lines past the 50 that were left out.
    pub left_out: usize,
    /// The row the keyboard is on.
    pub cursor: usize,
    /// More than one line: nothing is added until the list is confirmed.
    pub confirm: bool,
    /// Stopped by "too many requests".
    pub paused: bool,
    id: u64,
    pending: VecDeque<usize>,
    runner: bool,
}

impl Review {
    pub fn total(&self) -> usize {
        self.lines.len()
    }

    /// Lines that have their answer (or will not get one).
    pub fn settled(&self) -> usize {
        self.lines.iter().filter(|l| l.state != LineState::Waiting).count()
    }

    /// Lookups are still going.
    pub fn working(&self) -> bool {
        !self.paused && self.lines.iter().any(|l| l.state == LineState::Waiting)
    }

    /// Lines that found songs.
    pub fn found(&self) -> usize {
        self.lines.iter().filter(|l| l.can_pick()).count()
    }
}

/// What the add box tells its view.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CrateAddEvent {
    /// Songs went into the Crate (video ids, in order).
    Added(Vec<String>),
}

/// What a paste did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteStart {
    /// Nothing in it that is a line.
    Nothing,
    /// One plain line pasted into the field: it is just typing (the caller puts the text in).
    Typed,
    /// Lookups are under way (the review list, or an add that follows by itself).
    Started,
    /// YouTube said it was busy a moment ago: nothing was asked.
    Paused,
    /// The backend is not ready.
    NoSearch,
}

/// The Crate's add box.
pub struct CrateAddModel {
    crate_: Entity<CrateModel>,
    player: Entity<PlayerModel>,
    /// What is typed, normalised.
    query: String,
    /// The songs the popover lists, for `shown` (which is `query` once the answer is in).
    results: Vec<ListedTrack>,
    shown: String,
    status: Status,
    active: Option<usize>,
    /// Numbers typed queries, so only the newest answer shows.
    generation: u64,
    /// Enter was pressed before the answer: add the first song when it comes.
    add_when_ready: bool,
    cache: HashMap<String, Vec<ListedTrack>>,
    review: Option<Review>,
    /// Numbers pastes.
    serial: u64,
    debounce: Duration,
    pause: Duration,
    cooldown: Duration,
    cooling_until: Option<Instant>,
}

impl EventEmitter<CrateAddEvent> for CrateAddModel {}

impl CrateAddModel {
    pub fn new(crate_: Entity<CrateModel>, player: Entity<PlayerModel>) -> Self {
        Self {
            crate_,
            player,
            query: String::new(),
            results: Vec::new(),
            shown: String::new(),
            status: Status::Idle,
            active: None,
            generation: 0,
            add_when_ready: false,
            cache: HashMap::new(),
            review: None,
            serial: 0,
            debounce: DEBOUNCE,
            pause: PAUSE,
            cooldown: COOLDOWN,
            cooling_until: None,
        }
    }

    /// Waits and cooldown, shortened for tests (the executor's clock does not run by itself).
    pub fn set_pacing(&mut self, debounce: Duration, pause: Duration, cooldown: Duration) {
        self.debounce = debounce;
        self.pause = pause;
        self.cooldown = cooldown;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn results(&self) -> &[ListedTrack] {
        &self.results
    }

    pub fn status(&self) -> Status {
        self.status
    }

    /// The song the keyboard is on.
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    pub fn review(&self) -> Option<&Review> {
        self.review.as_ref()
    }

    /// Whether the Crate holds the song already (the popover marks it).
    pub fn holds(&self, video_id: &str, cx: &gpui::App) -> bool {
        self.crate_.read(cx).contains(video_id)
    }

    fn source(&self, cx: &gpui::App) -> Option<Arc<dyn SearchSource>> {
        self.crate_.read(cx).search_source()
    }

    /// YouTube said it is busy, or something else of the app did.
    fn cooling(&self, cx: &gpui::App) -> bool {
        self.cooling_until.is_some_and(|t| Instant::now() < t)
            || self.crate_.read(cx).throttle().blocked(Lane::Foreground).is_some()
    }

    /// The service said "too many requests": every lookup stops for a while.
    fn back_off(&mut self, cx: &mut Context<Self>) {
        self.cooling_until = Some(Instant::now() + self.cooldown);
        self.generation += 1;
        self.add_when_ready = false;
        self.status = Status::Paused;
        self.results.clear();
        self.active = None;
        let mut direct: Option<Review> = None;
        if let Some(review) = self.review.as_mut() {
            review.paused = true;
            review.pending.clear();
            for line in review.lines.iter_mut().filter(|l| l.state == LineState::Waiting) {
                line.state = LineState::Paused;
            }
            if !review.confirm {
                direct = self.review.take();
            }
        }
        if let Some(review) = direct {
            // A single link that was being followed: what came in before the stop is kept.
            self.add_lines(&review, true, cx);
        }
        cx.notify();
    }

    // ---- typing ---------------------------------------------------------------------------

    /// The field's text changed. Search follows after a pause.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = normalise(text);
        if text == self.query {
            return;
        }
        self.query = text.clone();
        self.generation += 1;
        self.add_when_ready = false;
        let gen = self.generation;
        if text.chars().count() < MIN_CHARS || as_link(&text).is_some() {
            self.results.clear();
            self.shown.clear();
            self.active = None;
            self.status = Status::Idle;
            cx.notify();
            return;
        }
        // An answer already known costs no request.
        if let Some(found) = self.cache.get(&text.to_lowercase()).cloned() {
            self.show(text, found);
            cx.notify();
            return;
        }
        self.status = Status::Searching;
        cx.notify();
        let Some(source) = self.source(cx) else {
            self.status = Status::Failed(LoadFailure::Other);
            return;
        };
        let wait = self.debounce;
        cx.spawn(async move |this, cx| {
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            let Ok(go) = this.update(cx, |m, cx| m.ready_to_ask(gen, cx)) else {
                return;
            };
            if !go {
                return;
            }
            let ask = text.clone();
            let found = cx
                .background_executor()
                .spawn(async move {
                    source
                        .search_filtered(&ask, SearchFilter::Songs)
                        .map(|page| song_hits(&page.results, MAX_RESULTS))
                })
                .await;
            let _ = this.update(cx, |m, cx| m.searched(gen, text, found, cx));
        })
        .detach();
    }

    /// The pause is over: ask, unless the text moved on or YouTube is busy.
    fn ready_to_ask(&mut self, gen: u64, cx: &mut Context<Self>) -> bool {
        if gen != self.generation {
            return false;
        }
        if self.cooling(cx) {
            self.status = Status::Paused;
            self.results.clear();
            self.active = None;
            self.add_when_ready = false;
            cx.notify();
            return false;
        }
        true
    }

    fn show(&mut self, text: String, found: Vec<ListedTrack>) {
        self.status = if found.is_empty() { Status::NoResults } else { Status::Idle };
        self.active = (!found.is_empty()).then_some(0);
        self.results = found;
        self.shown = text;
    }

    fn searched(
        &mut self,
        gen: u64,
        text: String,
        found: Result<Vec<ListedTrack>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        match found {
            Err(LoadFailure::Throttled) => self.back_off(cx),
            Err(f) => {
                if gen == self.generation {
                    self.results.clear();
                    self.shown.clear();
                    self.active = None;
                    self.add_when_ready = false;
                    self.status = Status::Failed(f);
                    cx.notify();
                }
            }
            Ok(found) => {
                if self.cache.len() >= CACHE {
                    self.cache.clear();
                }
                self.cache.insert(text.to_lowercase(), found.clone());
                if gen == self.generation {
                    self.show(text, found);
                    cx.notify();
                    if std::mem::take(&mut self.add_when_ready) {
                        self.add_result(0, cx);
                    }
                }
            }
        }
    }

    /// Up / Down in the list of songs.
    pub fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let last = self.results.len() as isize - 1;
        let at = self.active.map_or(if delta > 0 { 0 } else { last }, |a| {
            (a as isize + delta).clamp(0, last)
        });
        self.active = Some(at as usize);
        cx.notify();
    }

    pub fn set_active(&mut self, ix: usize, cx: &mut Context<Self>) {
        if self.active != Some(ix) && ix < self.results.len() {
            self.active = Some(ix);
            cx.notify();
        }
    }

    /// Enter: add the song the keyboard is on. With the search still out, the first song is
    /// added the moment it comes. Returns whether a song was added now.
    pub fn add_active(&mut self, cx: &mut Context<Self>) -> bool {
        if self.shown != self.query || self.results.is_empty() {
            if self.status == Status::Searching {
                self.add_when_ready = true;
            }
            return false;
        }
        match self.active {
            Some(ix) => self.add_result(ix, cx),
            None => false,
        }
    }

    /// Add result `ix` to the Crate. The box is then empty again, ready for the next.
    pub fn add_result(&mut self, ix: usize, cx: &mut Context<Self>) -> bool {
        let Some(listed) = self.results.get(ix) else {
            return false;
        };
        let song = Found::from(listed);
        let added = self.put(vec![song], cx);
        self.clear_typed(cx);
        added > 0
    }

    /// Forget what is typed and found (the field is emptied by the view).
    pub fn clear_typed(&mut self, cx: &mut Context<Self>) {
        self.query.clear();
        self.shown.clear();
        self.results.clear();
        self.active = None;
        self.status = Status::Idle;
        self.generation += 1;
        self.add_when_ready = false;
        cx.notify();
    }

    /// Put songs in the Crate (one step for Ctrl+Z) and say so. Returns how many were new.
    fn put(&mut self, songs: Vec<Found>, cx: &mut Context<Self>) -> usize {
        let asked = songs.len();
        let new: Vec<NewSong> = songs
            .into_iter()
            .map(|s| NewSong { track: s.track, thumbnail: s.thumbnail, explicit: s.explicit })
            .collect();
        let added = self.crate_.update(cx, |m, cx| m.add_many(new, cx));
        if added.is_empty() {
            if asked > 0 {
                confirm::already_crated(cx);
            }
        } else {
            confirm::confirm(
                confirm::added_text(added.len(), "Crate"),
                true,
                Some((confirm::Place::Crate, added.clone())),
                cx,
            );
            cx.emit(CrateAddEvent::Added(added.clone()));
        }
        added.len()
    }

    // ---- pasting --------------------------------------------------------------------------

    /// Ctrl+V. `into_field` says the text was pasted into the add box (a single plain line is
    /// then only typing); onto the Crate it is looked up and added.
    pub fn paste(&mut self, text: &str, into_field: bool, cx: &mut Context<Self>) -> PasteStart {
        let parsed = crate_paste::parse(text);
        if parsed.lines.is_empty() {
            return PasteStart::Nothing;
        }
        let single_plain = !parsed.is_multi() && matches!(parsed.lines[0].kind, Kind::Query(_));
        if into_field && single_plain {
            return PasteStart::Typed;
        }
        if self.source(cx).is_none() {
            confirm::arranged("Couldn't reach YouTube Music".to_owned(), false, cx);
            return PasteStart::NoSearch;
        }
        if self.cooling(cx) {
            confirm::arranged(BUSY_TEXT.to_owned(), false, cx);
            return PasteStart::Paused;
        }
        // What was typed or listed gives way to the paste.
        self.generation += 1;
        self.results.clear();
        self.active = None;
        self.add_when_ready = false;
        self.status = Status::Idle;
        self.serial += 1;
        let confirm_first = parsed.is_multi();
        let lines: Vec<ReviewLine> = parsed.lines.into_iter().map(ReviewLine::new).collect();
        let pending = (0..lines.len()).collect();
        self.review = Some(Review {
            lines,
            left_out: parsed.left_out,
            cursor: 0,
            confirm: confirm_first,
            paused: false,
            id: self.serial,
            pending,
            runner: false,
        });
        self.ensure_runner(cx);
        cx.notify();
        PasteStart::Started
    }

    fn ensure_runner(&mut self, cx: &mut Context<Self>) {
        let Some(review) = self.review.as_mut() else {
            return;
        };
        if review.runner || review.pending.is_empty() {
            return;
        }
        review.runner = true;
        let id = review.id;
        cx.spawn(async move |this, cx| loop {
            let Ok(Some((jobs, source, library, pause))) =
                this.update(cx, |m, cx| m.take_jobs(id, cx))
            else {
                let _ = this.update(cx, |m, _| {
                    if let Some(r) = m.review.as_mut().filter(|r| r.id == id) {
                        r.runner = false;
                    }
                });
                return;
            };
            let tasks: Vec<_> = jobs
                .iter()
                .map(|(_, kind)| {
                    let (source, library, kind) = (source.clone(), library.clone(), kind.clone());
                    cx.background_executor()
                        .spawn(async move { resolve(&kind, &*source, library.as_deref(), pause) })
                })
                .collect();
            for ((index, _), task) in jobs.iter().zip(tasks) {
                let outcome = task.await;
                let _ = this.update(cx, |m, cx| m.finish_line(id, *index, outcome, cx));
            }
            // A small gap before the next pair, so a long paste trickles.
            let wait = this
                .update(cx, |m, _| {
                    let more = m.review.as_ref().is_some_and(|r| r.id == id && !r.pending.is_empty());
                    if more { m.pause } else { Duration::ZERO }
                })
                .unwrap_or(Duration::ZERO);
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
        })
        .detach();
    }

    /// The next lines to look up, with what they need. `None` ends the runner: nothing is left,
    /// the paste is gone, or YouTube is busy.
    #[allow(clippy::type_complexity)]
    fn take_jobs(
        &mut self,
        id: u64,
        cx: &mut Context<Self>,
    ) -> Option<(
        Vec<(usize, Kind)>,
        Arc<dyn SearchSource>,
        Option<Arc<dyn LibrarySource>>,
        Duration,
    )> {
        if !self.review.as_ref().is_some_and(|r| r.id == id && !r.pending.is_empty()) {
            return None;
        }
        let source = self.source(cx)?;
        if self.cooling(cx) {
            self.back_off(cx);
            return None;
        }
        let library = self.player.read(cx).library_source();
        let review = self.review.as_mut()?;
        let mut jobs = Vec::new();
        while jobs.len() < CONCURRENCY {
            let Some(index) = review.pending.pop_front() else { break };
            jobs.push((index, review.lines[index].kind.clone()));
        }
        Some((jobs, source, library, self.pause))
    }

    /// A lookup came back.
    fn finish_line(&mut self, id: u64, index: usize, outcome: Outcome, cx: &mut Context<Self>) {
        let Some(review) = self.review.as_mut().filter(|r| r.id == id) else {
            // A lookup of a paste that is gone still tells of a busy service.
            if matches!(outcome, Outcome::Throttled | Outcome::Songs { throttled: true, .. }) {
                self.back_off(cx);
            }
            return;
        };
        let line = &mut review.lines[index];
        let mut throttled = false;
        match outcome {
            Outcome::Songs { songs, throttled: stopped } => {
                throttled = stopped;
                line.state = if songs.is_empty() { LineState::NoMatch } else { LineState::Found };
                line.songs = songs;
            }
            Outcome::NoMatch => line.state = LineState::NoMatch,
            Outcome::Failed => line.state = LineState::Failed,
            Outcome::Throttled => {
                line.state = LineState::Paused;
                throttled = true;
            }
        }
        if throttled {
            self.back_off(cx);
            return;
        }
        let done = !review.working() && review.pending.is_empty();
        if done && !review.confirm {
            if let Some(review) = self.review.take() {
                self.add_lines(&review, false, cx);
            }
        }
        cx.notify();
    }

    /// A paste that needed no review is over: what it found goes in. `stopped` says YouTube
    /// stopped it, which the pill then says too.
    fn add_lines(&mut self, review: &Review, stopped: bool, cx: &mut Context<Self>) {
        let songs: Vec<Found> = review
            .lines
            .iter()
            .filter(|l| l.can_pick() && l.picked)
            .flat_map(|l| l.songs.iter().cloned())
            .collect();
        let songs = unique(songs);
        if songs.is_empty() {
            let text = if stopped {
                BUSY_TEXT
            } else if review.lines.iter().any(|l| l.state == LineState::Failed) {
                "Couldn't reach YouTube Music"
            } else {
                "No match"
            };
            confirm::arranged(text.to_owned(), false, cx);
            return;
        }
        let added = self.put(songs, cx);
        if stopped && added > 0 {
            confirm::arranged(format!("{BUSY_TEXT}. Added {added}"), true, cx);
        }
    }

    // ---- the review list ------------------------------------------------------------------

    /// Songs "Add 14" would put in the Crate: those of the lines that are ticked, once each, and
    /// not in the Crate already.
    pub fn addable(&self, cx: &gpui::App) -> usize {
        let Some(review) = self.review.as_ref() else {
            return 0;
        };
        let songs: Vec<Found> = review
            .lines
            .iter()
            .filter(|l| l.can_pick() && l.picked)
            .flat_map(|l| l.songs.iter().cloned())
            .collect();
        let crate_ = self.crate_.read(cx);
        unique(songs)
            .iter()
            .filter(|s| !crate_.contains(s.track.video_id.as_str()))
            .count()
    }

    pub fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(line) = self.review.as_mut().and_then(|r| r.lines.get_mut(ix)) {
            if line.can_pick() {
                line.picked = !line.picked;
                cx.notify();
            }
        }
    }

    pub fn set_cursor(&mut self, ix: usize, cx: &mut Context<Self>) {
        if let Some(review) = self.review.as_mut().filter(|r| ix < r.lines.len()) {
            if review.cursor != ix {
                review.cursor = ix;
                cx.notify();
            }
        }
    }

    /// Up / Down in the review list.
    pub fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(review) = self.review.as_mut() {
            let last = review.lines.len() as isize - 1;
            review.cursor = (review.cursor as isize + delta).clamp(0, last.max(0)) as usize;
            cx.notify();
        }
    }

    /// Space: tick or untick the row the keyboard is on.
    pub fn toggle_cursor(&mut self, cx: &mut Context<Self>) {
        let ix = self.review.as_ref().map(|r| r.cursor);
        if let Some(ix) = ix {
            self.toggle(ix, cx);
        }
    }

    /// "Add 14": the ticked songs go in, as one step for Ctrl+Z. Not while lookups are still
    /// going (they finish, or YouTube stops them, in a moment). Returns how many were added.
    pub fn confirm_review(&mut self, cx: &mut Context<Self>) -> usize {
        if !self.review.as_ref().is_some_and(|r| r.confirm && !r.working()) {
            return 0;
        }
        let Some(review) = self.review.take() else {
            return 0;
        };
        let songs = unique(
            review
                .lines
                .iter()
                .filter(|l| l.can_pick() && l.picked)
                .flat_map(|l| l.songs.iter().cloned())
                .collect(),
        );
        cx.notify();
        if songs.is_empty() {
            return 0;
        }
        self.put(songs, cx)
    }

    /// Esc: the review is closed and nothing is added; lookups still going are dropped.
    pub fn cancel_review(&mut self, cx: &mut Context<Self>) {
        if self.review.take().is_some() {
            cx.notify();
        }
    }

    /// The page was left: nothing typed, nothing under review.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.review = None;
        self.clear_typed(cx);
    }
}

/// What one line's lookup came to.
enum Outcome {
    Songs {
        songs: Vec<Found>,
        /// YouTube said it was busy part-way (a playlist's later pages).
        throttled: bool,
    },
    NoMatch,
    Failed,
    Throttled,
}

/// The songs of a search to show or match among: the audio tracks, or every song when the
/// search found no audio track at all.
fn song_hits(results: &SearchResults, limit: usize) -> Vec<ListedTrack> {
    let mut songs = crate_swap::audio_only(results);
    if songs.is_empty() {
        songs = results.songs.clone();
    }
    songs.truncate(limit);
    songs
}

/// Each song once, in order.
fn unique(songs: Vec<Found>) -> Vec<Found> {
    let mut seen = std::collections::HashSet::new();
    songs
        .into_iter()
        .filter(|s| seen.insert(s.track.video_id.as_str().to_owned()))
        .collect()
}

/// Look one line up. Runs on a background thread (it blocks on the network).
fn resolve(
    kind: &Kind,
    source: &dyn SearchSource,
    library: Option<&dyn LibrarySource>,
    pause: Duration,
) -> Outcome {
    let failed = |f: LoadFailure| match f {
        LoadFailure::Throttled => Outcome::Throttled,
        LoadFailure::Parse => Outcome::NoMatch,
        _ => Outcome::Failed,
    };
    let one = |song: &ListedTrack| Outcome::Songs { songs: vec![Found::from(song)], throttled: false };
    match kind {
        Kind::BadLink | Kind::Link(ExternalLink::Artist(_)) => Outcome::NoMatch,
        Kind::Query(query) => match source.search_filtered(&query.text, SearchFilter::Songs) {
            Ok(page) => {
                let hits = song_hits(&page.results, MATCH_AMONG);
                match crate_paste::best_match(query, &hits) {
                    Some(at) => one(&hits[at]),
                    None => Outcome::NoMatch,
                }
            }
            Err(f) => failed(f),
        },
        Kind::Link(link @ ExternalLink::Track { .. }) => match source.resolve_link(link) {
            Ok(LinkTarget::Track(song)) => one(&song),
            Ok(_) => Outcome::NoMatch,
            Err(f) => failed(f),
        },
        Kind::Link(ExternalLink::Playlist(id)) => {
            let raw = id.as_str();
            let browse = if raw.starts_with("VL") { raw.to_owned() } else { format!("VL{raw}") };
            collect(library, CollectionKind::Playlist, BrowseId::new(browse), pause)
        }
        Kind::Link(ExternalLink::Album(id)) => {
            collect(library, CollectionKind::Album, id.clone(), pause)
        }
    }
}

/// Every song of a playlist or album, paged with a pause between pages: up to
/// [`MAX_PAGES`] pages and [`MAX_SONGS_PER_PASTE`] songs. A later page that fails ends the list
/// there (a busy service stops the paste as well); only a failing first page is a failure.
fn collect(
    library: Option<&dyn LibrarySource>,
    kind: CollectionKind,
    id: BrowseId,
    pause: Duration,
) -> Outcome {
    let Some(library) = library else {
        return Outcome::Failed;
    };
    let collection = Collection { kind, id, title: String::new(), subtitle: None, thumbnail: None };
    let first = match library.tracks(&collection) {
        Ok(page) => page,
        Err(LoadFailure::Throttled) => return Outcome::Throttled,
        Err(LoadFailure::Parse) => return Outcome::NoMatch,
        Err(_) => return Outcome::Failed,
    };
    let mut songs: Vec<Found> = first
        .items
        .iter()
        .map(|r| Found { track: r.track.clone(), thumbnail: r.thumbnail.clone(), explicit: r.explicit })
        .collect();
    let mut next = first.continuation;
    let mut throttled = false;
    let mut pages = 1;
    while let Some(token) = next.take() {
        if pages >= MAX_PAGES || songs.len() >= MAX_SONGS_PER_PASTE {
            break;
        }
        if !pause.is_zero() {
            std::thread::sleep(pause);
        }
        match library.more_tracks(&token) {
            Ok(page) => {
                pages += 1;
                songs.extend(page.items.iter().map(|r| Found {
                    track: r.track.clone(),
                    thumbnail: r.thumbnail.clone(),
                    explicit: r.explicit,
                }));
                next = page.continuation;
            }
            Err(f) => {
                throttled = f == LoadFailure::Throttled;
                break;
            }
        }
    }
    songs.truncate(MAX_SONGS_PER_PASTE);
    Outcome::Songs { songs, throttled }
}

/// What the pill says of a paste that was cut at the line limit (the review list shows it).
pub fn left_out_text(left_out: usize) -> Option<String> {
    (left_out > 0).then(|| format!("Only the first {} lines were read", crate_paste::MAX_LINES))
}

/// "Looking up 6/14" while lookups run, "11 of 14 found" after.
pub fn progress_text(review: &Review) -> String {
    if review.working() {
        format!("Looking up\u{2026} {}/{}", review.settled(), review.total())
    } else if review.paused {
        format!("{BUSY_TEXT}. {} of {} found", review.found(), review.total())
    } else {
        format!("{} of {} found", review.found(), review.total())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_progress_words() {
        let line = |state| ReviewLine {
            raw: "x".into(),
            kind: Kind::BadLink,
            state,
            songs: Vec::new(),
            group: None,
            picked: true,
        };
        let mut review = Review {
            lines: vec![line(LineState::Found), line(LineState::Waiting), line(LineState::NoMatch)],
            left_out: 0,
            cursor: 0,
            confirm: true,
            paused: false,
            id: 1,
            pending: VecDeque::new(),
            runner: false,
        };
        assert_eq!(progress_text(&review), "Looking up\u{2026} 2/3");
        review.lines[1].state = LineState::NoMatch;
        // A "found" line with no songs does not count as found.
        assert_eq!(progress_text(&review), "0 of 3 found");
        review.paused = true;
        assert_eq!(progress_text(&review), "Paused: YouTube is busy. 0 of 3 found");
        assert_eq!(left_out_text(0), None);
        assert_eq!(left_out_text(3).as_deref(), Some("Only the first 50 lines were read"));
    }
}
