//! Search state: the source port and the query state machine (normalise, debounce, fetch, keep
//! only the answer to the newest query), the typeahead suggestions, the recent searches and the
//! pasted-link path. No views here; `ui::search_page` draws it.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

use apollo_domain::links::{parse_external_link, ExternalLink};
use apollo_domain::listing::LoadFailure;
use apollo_provider::{LinkTarget, SearchFilter, SearchPage, SearchResults};
use gpui::{Context, EventEmitter, Task};

use crate::settings::{BoolSetting, SettingsBackend};

pub mod local;
use local::{LocalIndex, LocalResults};

/// Settings > Search > Save recent searches.
pub const SAVE_RECENT_SEARCHES: BoolSetting = BoolSetting::new("save_recent_searches", true);
/// Settings > Search > Hide music videos: results keep the audio tracks only.
pub const HIDE_MUSIC_VIDEOS: BoolSetting = BoolSetting::new("hide_music_videos", false);

/// Where results come from. Blocking: the model runs it on a background thread, so an
/// implementation may do network I/O or wait on its own runtime.
pub trait SearchSource: Send + Sync {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure>;

    /// First page of one kind of result (a chip). Signed in like `search`.
    fn search_filtered(
        &self,
        _query: &str,
        _filter: SearchFilter,
    ) -> Result<SearchPage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// The page after one `search_filtered` answered, by its token.
    fn search_more(&self, _token: &str) -> Result<SearchPage, LoadFailure> {
        Err(LoadFailure::Other)
    }

    /// Typeahead completions for a partly typed query, best first. Never written to the
    /// account's search history.
    fn suggest(&self, _query: &str) -> Result<Vec<String>, LoadFailure> {
        Ok(Vec::new())
    }

    /// What a pasted link opens.
    fn resolve_link(&self, _link: &ExternalLink) -> Result<LinkTarget, LoadFailure> {
        Err(LoadFailure::Other)
    }
}

/// Stands in when the backend could not start: every search fails with a short message.
pub struct Unavailable;

impl SearchSource for Unavailable {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Err(LoadFailure::Other)
    }
}

/// Typing pauses this long before a query goes out. The service writes every signed-in search
/// to the account's history, so this is also what keeps half-typed words out of it.
pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// Typing pauses this long before suggestions are asked for (they are not recorded, so the
/// pause only spares the service a request per keystroke).
pub const SUGGEST_DEBOUNCE: Duration = Duration::from_millis(150);

/// Typing shorter than this never searches by itself; Enter still does.
pub const MIN_PREVIEW_CHARS: usize = 2;

/// Rows the suggestions popover shows at most.
pub const MAX_SUGGESTIONS: usize = 8;
/// Recent searches the popover shows at most.
pub const SHOWN_RECENTS: usize = 10;
/// Recent searches kept; the oldest beyond this are forgotten.
const KEPT_RECENTS: usize = 20;
/// Where the recents live in the settings table: one query per line.
const RECENTS_KEY: &str = "recent_searches";
/// Suggestion answers remembered, so backspacing over a word costs no request.
const SUGGEST_CACHE: usize = 64;

/// The row of kinds under the field: "All" is the mixed answer, the rest are the service's own
/// filtered searches, paged, and "Library" is the owner's own saved things (never asked of the
/// service).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SearchChip {
    All,
    Songs,
    Albums,
    Artists,
    Playlists,
    CommunityPlaylists,
    Library,
}

impl SearchChip {
    pub const ROW: [SearchChip; 7] = [
        SearchChip::All,
        SearchChip::Songs,
        SearchChip::Albums,
        SearchChip::Artists,
        SearchChip::Playlists,
        SearchChip::CommunityPlaylists,
        SearchChip::Library,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SearchChip::All => "All",
            SearchChip::Songs => "Songs",
            SearchChip::Albums => "Albums",
            SearchChip::Artists => "Artists",
            SearchChip::Playlists => "Playlists",
            SearchChip::CommunityPlaylists => "Community playlists",
            SearchChip::Library => "Library",
        }
    }

    /// Whether the chip's rows come from the service (all but Library).
    pub fn is_remote(self) -> bool {
        self != SearchChip::Library
    }

    /// The service's filter, `None` for the mixed answer (and for Library, which asks nothing).
    pub fn filter(self) -> Option<SearchFilter> {
        match self {
            SearchChip::All | SearchChip::Library => None,
            SearchChip::Songs => Some(SearchFilter::Songs),
            SearchChip::Albums => Some(SearchFilter::Albums),
            SearchChip::Artists => Some(SearchFilter::Artists),
            SearchChip::Playlists => Some(SearchFilter::Playlists),
            SearchChip::CommunityPlaylists => Some(SearchFilter::CommunityPlaylists),
        }
    }
}

/// The next page of a filtered list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum More {
    /// Nothing in flight (there may or may not be a next page: see [`SearchModel::has_more`]).
    Idle,
    Loading,
    Failed(LoadFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchState {
    /// Nothing to show: no query, or one too short to search for.
    Idle,
    /// A query is waiting out the debounce or is in flight.
    Loading,
    /// The newest query has an answer (possibly an empty one).
    Ready,
    Failed(LoadFailure),
    /// The text is a link, but not one to a track, playlist, album or artist.
    BadLink,
}

/// What the model tells the page to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchEvent {
    /// A pasted link was looked up: play the track or open the page.
    OpenLink(LinkTarget),
}

/// What the page is doing with the text in the field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// No query, or one too short to search for.
    Idle,
    /// The text is a link, but not one to a track, playlist, album or artist.
    BadLink,
    /// A pasted link is being looked up (or failed, for Retry).
    LinkLoading,
    LinkFailed(LoadFailure),
    /// A query: what is shown is its chip's pane.
    Query,
}

/// One chip's list: the service's answer for the query it was asked, and its paging.
#[derive(Default)]
struct Pane {
    /// A request (or the pause before it) is pending.
    loading: bool,
    /// The last request failed.
    failure: Option<LoadFailure>,
    /// The service's answer as it came, so toggling hide-music-videos needs no new request.
    raw: Option<Arc<SearchResults>>,
    /// What the page lists: `raw` with the hide-music-videos choice applied.
    results: Option<Arc<SearchResults>>,
    /// The query `raw` answers, so asking again for it costs nothing.
    answered: Option<String>,
    continuation: Option<String>,
    more: Option<More>,
    /// Debounce plus request. Replacing it drops the older one, so an abandoned query can
    /// never update the view.
    task: Option<Task<()>>,
    more_task: Option<Task<()>>,
}

pub struct SearchModel {
    source: Option<Arc<dyn SearchSource>>,
    /// The query as searched: trimmed, inner whitespace collapsed.
    query: String,
    mode: Mode,
    /// The chip on show; it stays across queries.
    chip: SearchChip,
    panes: HashMap<SearchChip, Pane>,
    /// Completions for the text being typed (cleared when it is empty or a link).
    suggestions: Vec<String>,
    suggest_task: Option<Task<()>>,
    suggest_cache: HashMap<String, Vec<String>>,
    /// Newest first.
    recents: Vec<String>,
    recents_writer: Option<Sender<String>>,
    /// Both switches live in Settings; the page hands them over.
    save_recents: bool,
    hide_videos: bool,
    /// The link being looked up (or that failed, for Retry).
    link: Option<ExternalLink>,
    /// What the library holds, as the Library screen last loaded it.
    index: Arc<LocalIndex>,
    /// What the query found in it, and the query that answered. Local, so it is ready long before
    /// the service answers.
    local: Arc<LocalResults>,
    local_for: String,
    local_task: Option<Task<()>>,
}

impl EventEmitter<SearchEvent> for SearchModel {}

impl SearchModel {
    pub fn new() -> Self {
        Self {
            source: None,
            query: String::new(),
            mode: Mode::Idle,
            chip: SearchChip::All,
            panes: HashMap::new(),
            suggestions: Vec::new(),
            suggest_task: None,
            suggest_cache: HashMap::new(),
            recents: Vec::new(),
            recents_writer: None,
            save_recents: SAVE_RECENT_SEARCHES.default,
            hide_videos: HIDE_MUSIC_VIDEOS.default,
            link: None,
            index: Arc::new(LocalIndex::default()),
            local: Arc::new(LocalResults::default()),
            local_for: String::new(),
            local_task: None,
        }
    }

    /// The backend is ready. A query typed before that (state `Loading`) goes out now.
    pub fn install(&mut self, source: Arc<dyn SearchSource>, cx: &mut Context<Self>) {
        self.source = Some(source);
        if self.state() == SearchState::Loading {
            match self.link.clone() {
                Some(link) => self.open_link(link, cx),
                None => self.start(false, cx),
            }
        }
        if !self.query.is_empty() && self.suggestions.is_empty() {
            self.request_suggestions(cx);
        }
    }

    /// Recents live in `backend` (read now, off the UI thread; written in order by one worker).
    pub fn install_recents(&mut self, backend: Arc<dyn SettingsBackend>, cx: &mut Context<Self>) {
        let (tx, rx) = channel::<String>();
        let writer = backend.clone();
        let _ = std::thread::Builder::new()
            .name("apollo-recent-searches".into())
            .spawn(move || {
                for value in rx {
                    writer.set(RECENTS_KEY, &value);
                }
            });
        self.recents_writer = Some(tx);
        cx.spawn(async move |this, cx| {
            let stored = cx
                .background_executor()
                .spawn(async move { backend.get(RECENTS_KEY) })
                .await;
            let _ = this.update(cx, |m, cx| {
                // Anything searched before the read finished is newer than what was stored.
                let mut merged = std::mem::take(&mut m.recents);
                for old in decode_recents(&stored.unwrap_or_default()) {
                    push_recent(&mut merged, &old, true);
                }
                merged.truncate(KEPT_RECENTS);
                m.recents = merged;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn chip(&self) -> SearchChip {
        self.chip
    }

    fn pane(&self, chip: SearchChip) -> Option<&Pane> {
        self.panes.get(&chip)
    }

    fn pane_mut(&mut self, chip: SearchChip) -> &mut Pane {
        self.panes.entry(chip).or_default()
    }

    pub fn state(&self) -> SearchState {
        match self.mode {
            Mode::Idle => SearchState::Idle,
            Mode::BadLink => SearchState::BadLink,
            Mode::LinkLoading => SearchState::Loading,
            Mode::LinkFailed(f) => SearchState::Failed(f),
            // The Library chip asks nothing: its answer is the local one.
            Mode::Query if !self.chip.is_remote() => SearchState::Ready,
            Mode::Query => match self.pane(self.chip) {
                Some(p) if p.loading => SearchState::Loading,
                Some(p) => match p.failure {
                    Some(f) => SearchState::Failed(f),
                    None if p.answered.is_some() => SearchState::Ready,
                    None => SearchState::Loading,
                },
                None => SearchState::Loading,
            },
        }
    }

    /// Results of the chip on show for the last answered query. Kept while a newer one loads,
    /// so the page does not flash to skeletons on every keystroke.
    pub fn results(&self) -> Option<&Arc<SearchResults>> {
        if self.mode != Mode::Query {
            return None;
        }
        self.pane(self.chip).and_then(|p| p.results.as_ref())
    }

    /// A finished search that found nothing and read everything it was given.
    pub fn is_confirmed_empty(&self) -> bool {
        self.state() == SearchState::Ready && self.results().is_some_and(|r| r.is_empty())
    }

    /// What the library holds for the query (see [`Self::local_ready`]). Empty while there is
    /// no query.
    pub fn local(&self) -> &Arc<LocalResults> {
        &self.local
    }

    /// What to list from the library: the answer for the query on show (the previous one while
    /// a newer one is being worked out), `None` when nothing is searched.
    pub fn local_shown(&self) -> Option<&Arc<LocalResults>> {
        (self.mode == Mode::Query).then_some(&self.local)
    }

    /// `local` answers the query now in the field.
    pub fn local_ready(&self) -> bool {
        self.mode == Mode::Query && self.local_for == self.query
    }

    /// The library holds nothing for the query (the Library chip's empty state).
    pub fn is_local_empty(&self) -> bool {
        self.local_ready() && self.local.is_empty()
    }

    /// The Library screen's lists changed: look again for the query in the field.
    pub fn set_library(&mut self, index: Arc<LocalIndex>, cx: &mut Context<Self>) {
        self.index = index;
        self.local_for.clear();
        self.refresh_local(false, cx);
    }

    /// The service has another page of the chip on show.
    pub fn has_more(&self) -> bool {
        self.mode == Mode::Query
            && self
                .pane(self.chip)
                .is_some_and(|p| p.continuation.is_some() && p.answered.as_deref() == Some(&self.query))
    }

    /// The next page of the chip on show: in flight, failed or neither.
    pub fn more(&self) -> More {
        self.pane(self.chip)
            .and_then(|p| p.more)
            .unwrap_or(More::Idle)
    }

    /// Completions for what is being typed, best first, at most [`MAX_SUGGESTIONS`].
    pub fn suggestions(&self) -> &[String] {
        &self.suggestions
    }

    /// The most recent searches first, at most [`SHOWN_RECENTS`]; none while saving is off.
    pub fn recents(&self) -> &[String] {
        if self.save_recents {
            &self.recents[..self.recents.len().min(SHOWN_RECENTS)]
        } else {
            &[]
        }
    }

    /// The Settings > Search switches. Turning "Save recent searches" off forgets the list.
    pub fn set_options(&mut self, save_recents: bool, hide_videos: bool, cx: &mut Context<Self>) {
        let mut changed = false;
        if self.save_recents != save_recents {
            self.save_recents = save_recents;
            if !save_recents {
                self.recents.clear();
                self.persist_recents();
            }
            changed = true;
        }
        if self.hide_videos != hide_videos {
            self.hide_videos = hide_videos;
            for pane in self.panes.values_mut() {
                pane.apply_filter(hide_videos);
            }
            changed = true;
        }
        if changed {
            cx.notify();
        }
    }

    /// Put the current query at the top of the recents (the owner searched for it on purpose).
    pub fn remember(&mut self, cx: &mut Context<Self>) {
        if !self.save_recents || self.query.is_empty() || as_link(&self.query).is_some() {
            return;
        }
        let query = self.query.clone();
        push_recent(&mut self.recents, &query, false);
        self.recents.truncate(KEPT_RECENTS);
        self.persist_recents();
        cx.notify();
    }

    pub fn remove_recent(&mut self, text: &str, cx: &mut Context<Self>) {
        let before = self.recents.len();
        self.recents.retain(|r| !r.eq_ignore_ascii_case(text));
        if self.recents.len() != before {
            self.persist_recents();
            cx.notify();
        }
    }

    pub fn clear_recents(&mut self, cx: &mut Context<Self>) {
        if !self.recents.is_empty() {
            self.recents.clear();
            self.persist_recents();
            cx.notify();
        }
    }

    /// The account signed out: nothing it searched for stays.
    pub fn forget_account(&mut self, cx: &mut Context<Self>) {
        self.recents.clear();
        self.persist_recents();
        self.suggest_cache.clear();
        self.suggestions.clear();
        self.suggest_task = None;
        self.link = None;
        self.index = Arc::new(LocalIndex::default());
        self.clear(cx);
    }

    fn persist_recents(&self) {
        if let Some(writer) = &self.recents_writer {
            let _ = writer.send(encode_recents(&self.recents));
        }
    }

    /// The text field changed. Debounced; below [`MIN_PREVIEW_CHARS`] it clears the page.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let query = normalise(text);
        if query == self.query {
            return;
        }
        self.query = query;
        self.link = None;
        self.request_suggestions(cx);
        if self.query.chars().count() < MIN_PREVIEW_CHARS || as_link(&self.query).is_some() {
            // A pasted link is never searched for: Enter opens it.
            self.clear(cx);
        } else if self.answered_on_show() {
            // Typed back to what is already on screen.
            self.mode = Mode::Query;
            let chip = self.chip;
            self.pane_mut(chip).cancel();
            cx.notify();
        } else {
            self.start(true, cx);
        }
        self.refresh_local(true, cx);
    }

    /// Search for `text` now, as if it had been typed and Enter pressed (a picked suggestion or
    /// recent). The field's own change event that follows finds the query already set.
    pub fn search_for(&mut self, text: &str, cx: &mut Context<Self>) {
        let query = normalise(text);
        if query.is_empty() {
            return;
        }
        if query != self.query {
            self.query = query;
            self.link = None;
            self.suggestions.clear();
            self.suggest_task = None;
            self.clear(cx);
        }
        self.submit(cx);
    }

    /// Enter: search now, with no debounce and with any length. A query already answered is not
    /// asked again. A link opens what it names instead of being searched for.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.query.is_empty() {
            return;
        }
        // Whatever suggestions are pending are no longer wanted.
        self.suggest_task = None;
        if let Some(parsed) = as_link(&self.query) {
            match parsed {
                Ok(link) => self.open_link(link, cx),
                Err(_) => {
                    self.reset_panes();
                    self.link = None;
                    self.mode = Mode::BadLink;
                    cx.notify();
                }
            }
            return;
        }
        self.remember(cx);
        self.refresh_local(false, cx);
        if self.mode == Mode::Query && self.answered_on_show() {
            return;
        }
        self.start(false, cx);
    }

    /// Ask again after a failure.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if let Some(link) = self.link.clone() {
            self.open_link(link, cx);
        } else if !self.query.is_empty() {
            self.start(false, cx);
        }
    }

    /// Show another kind of result. Its list is asked for unless it already answers this query.
    pub fn set_chip(&mut self, chip: SearchChip, cx: &mut Context<Self>) {
        if chip == self.chip {
            return;
        }
        self.chip = chip;
        if self.mode == Mode::Query {
            if !self.answered_on_show() {
                // Another query's rows must not stand in for this one's.
                let pane = self.pane_mut(chip);
                pane.raw = None;
                pane.results = None;
                pane.answered = None;
                self.fetch(chip, false, cx);
            }
        }
        cx.notify();
    }

    /// Whether the chip on show already holds the answer to the current query.
    fn answered_on_show(&self) -> bool {
        // The Library chip has nothing to wait for.
        !self.chip.is_remote()
            || self
                .pane(self.chip)
                .is_some_and(|p| p.answered.as_deref() == Some(self.query.as_str()))
    }

    fn reset_panes(&mut self) {
        // Dropping the tasks cancels any request in flight.
        self.panes.clear();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.reset_panes();
        self.mode = Mode::Idle;
        self.local_task = None;
        self.local = Arc::new(LocalResults::default());
        self.local_for.clear();
        cx.notify();
    }

    /// Look for the query in the library: after the suggestions' pause while typing, at once
    /// otherwise. Below [`MIN_PREVIEW_CHARS`], and for a pasted link, there is nothing to look
    /// for.
    fn refresh_local(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.local_task = None;
        if self.query.chars().count() < MIN_PREVIEW_CHARS || as_link(&self.query).is_some() {
            if !self.local_for.is_empty() {
                self.local = Arc::new(LocalResults::default());
                self.local_for.clear();
                cx.notify();
            }
            return;
        }
        if self.local_for == self.query {
            return;
        }
        if !debounce {
            self.compute_local(cx);
            return;
        }
        self.local_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SUGGEST_DEBOUNCE).await;
            let _ = this.update(cx, |m, cx| m.compute_local(cx));
        }));
    }

    fn compute_local(&mut self, cx: &mut Context<Self>) {
        self.local = Arc::new(self.index.find(&self.query));
        self.local_for = self.query.clone();
        cx.notify();
    }

    /// Ask for the chip on show.
    fn start(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.mode = Mode::Query;
        let chip = self.chip;
        if chip.is_remote() {
            self.fetch(chip, debounce, cx);
        } else {
            cx.notify();
        }
    }

    fn fetch(&mut self, chip: SearchChip, debounce: bool, cx: &mut Context<Self>) {
        debug_assert!(chip.is_remote(), "the Library chip is never asked of the service");
        let query = self.query.clone();
        // One chip asks at a time: a list left waiting for an older query would otherwise stay
        // "loading" for good, its answer being dropped as stale.
        for (other, pane) in self.panes.iter_mut() {
            if *other != chip {
                pane.cancel();
            }
        }
        let pane = self.pane_mut(chip);
        pane.loading = true;
        pane.failure = None;
        pane.more = None;
        pane.more_task = None;
        pane.task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(DEBOUNCE).await;
            }
            let Some(source) = this.update(cx, |m, _| m.source.clone()).ok().flatten() else {
                // No backend yet: `install` restarts this query.
                return;
            };
            let asked = query.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match chip.filter() {
                        None => source.search(&asked).map(|results| SearchPage {
                            results,
                            continuation: None,
                        }),
                        Some(filter) => source.search_filtered(&asked, filter),
                    }
                })
                .await;
            let _ = this.update(cx, |m, cx| m.finish(chip, query, result, cx));
        }));
        cx.notify();
    }

    fn finish(
        &mut self,
        chip: SearchChip,
        query: String,
        result: Result<SearchPage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if query != self.query {
            return;
        }
        let hide = self.hide_videos;
        let pane = self.pane_mut(chip);
        pane.loading = false;
        match result {
            Ok(page) => {
                pane.raw = Some(Arc::new(page.results));
                pane.continuation = page.continuation;
                pane.answered = Some(query);
                pane.failure = None;
                pane.apply_filter(hide);
            }
            Err(failure) => {
                pane.results = None;
                pane.raw = None;
                pane.answered = None;
                pane.continuation = None;
                pane.failure = Some(failure);
            }
        }
        cx.notify();
    }

    /// The next page of the chip on show. A no-op while one is in flight, after a failure (use
    /// [`Self::retry_more`]) and at the end of the list.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.has_more() || self.more() != More::Idle {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let (chip, query) = (self.chip, self.query.clone());
        let pane = self.pane_mut(chip);
        let Some(token) = pane.continuation.clone() else {
            return;
        };
        pane.more = Some(More::Loading);
        pane.more_task = Some(cx.spawn(async move |this, cx| {
            let asked = token.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.search_more(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_more(chip, query, token, result, cx));
        }));
        cx.notify();
    }

    pub fn retry_more(&mut self, cx: &mut Context<Self>) {
        if matches!(self.more(), More::Failed(_)) {
            let chip = self.chip;
            self.pane_mut(chip).more = None;
            self.load_more(cx);
        }
    }

    fn finish_more(
        &mut self,
        chip: SearchChip,
        query: String,
        token: String,
        result: Result<SearchPage, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        let hide = self.hide_videos;
        if query != self.query {
            return;
        }
        let pane = self.pane_mut(chip);
        // The list moved on (a new answer) while this page was on its way.
        if pane.continuation.as_deref() != Some(token.as_str()) {
            return;
        }
        match result {
            Ok(page) => {
                let mut raw = pane.raw.as_deref().cloned().unwrap_or_default();
                let before = (raw.songs.len(), raw.albums.len(), raw.artists.len(), raw.playlists.len());
                raw.append(page.results);
                let after = (raw.songs.len(), raw.albums.len(), raw.artists.len(), raw.playlists.len());
                pane.raw = Some(Arc::new(raw));
                // A page of nothing new ends the list, whatever token it carries.
                pane.continuation = page
                    .continuation
                    .filter(|next| *next != token && after != before);
                pane.more = None;
                pane.apply_filter(hide);
            }
            Err(failure) => pane.more = Some(More::Failed(failure)),
        }
        cx.notify();
    }

    fn open_link(&mut self, link: ExternalLink, cx: &mut Context<Self>) {
        self.reset_panes();
        self.mode = Mode::LinkLoading;
        self.link = Some(link.clone());
        let task = cx.spawn(async move |this, cx| {
            let Some(source) = this.update(cx, |m, _| m.source.clone()).ok().flatten() else {
                // No backend yet: `install` restarts the lookup.
                return;
            };
            let asked = link.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.resolve_link(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_link(link, result, cx));
        });
        // The lookup rides on the chip's pane so a new query or a clear drops it.
        let chip = self.chip;
        self.pane_mut(chip).task = Some(task);
        cx.notify();
    }

    fn finish_link(
        &mut self,
        link: ExternalLink,
        result: Result<LinkTarget, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if self.link.as_ref() != Some(&link) {
            return;
        }
        match result {
            Ok(target) => {
                self.link = None;
                self.mode = Mode::Idle;
                cx.emit(SearchEvent::OpenLink(target));
            }
            Err(failure) => self.mode = Mode::LinkFailed(failure),
        }
        cx.notify();
    }

    fn request_suggestions(&mut self, cx: &mut Context<Self>) {
        self.suggest_task = None;
        if self.query.is_empty() || as_link(&self.query).is_some() {
            if !self.suggestions.is_empty() {
                self.suggestions.clear();
                cx.notify();
            }
            return;
        }
        let key = self.query.to_lowercase();
        if let Some(cached) = self.suggest_cache.get(&key) {
            self.suggestions = cached.clone();
            cx.notify();
            return;
        }
        let query = self.query.clone();
        self.suggest_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SUGGEST_DEBOUNCE).await;
            let Some(source) = this.update(cx, |m, _| m.source.clone()).ok().flatten() else {
                return;
            };
            let asked = query.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.suggest(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish_suggest(query, result, cx));
        }));
    }

    fn finish_suggest(
        &mut self,
        query: String,
        result: Result<Vec<String>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if query != self.query {
            return;
        }
        // A failure is not worth a message: the popover just has nothing to offer.
        let list = clean_suggestions(result.unwrap_or_default(), &query);
        if self.suggest_cache.len() >= SUGGEST_CACHE {
            self.suggest_cache.clear();
        }
        self.suggest_cache
            .insert(query.to_lowercase(), list.clone());
        self.suggestions = list;
        cx.notify();
    }
}

impl Pane {
    /// Drop whatever request is pending (what was answered stays).
    fn cancel(&mut self) {
        self.loading = false;
        self.task = None;
        self.more_task = None;
        self.more = None;
    }

    /// `results` is `raw` less the music videos while the setting asks for that.
    fn apply_filter(&mut self, hide_videos: bool) {
        self.results = self.raw.as_ref().map(|raw| {
            if hide_videos && !raw.video_ids.is_empty() {
                Arc::new(raw.without_videos())
            } else {
                raw.clone()
            }
        });
    }
}

impl Default for SearchModel {
    fn default() -> Self {
        Self::new()
    }
}

/// What a link in the field is: `None` when the text is not one at all, otherwise what
/// [`parse_external_link`] makes of it. A bare `music.youtube.com/...` counts (the scheme is
/// assumed), so a link copied without it still opens.
pub fn as_link(query: &str) -> Option<Result<ExternalLink, apollo_domain::links::LinkError>> {
    let lower = query.to_ascii_lowercase();
    let with_scheme = if lower.starts_with("http://") || lower.starts_with("https://") {
        query.to_owned()
    } else if [
        "music.youtube.com/",
        "www.youtube.com/",
        "m.youtube.com/",
        "youtube.com/",
        "youtu.be/",
    ]
    .iter()
    .any(|host| lower.starts_with(host))
    {
        format!("https://{query}")
    } else {
        return None;
    };
    Some(parse_external_link(&with_scheme))
}

/// Completions as the popover shows them: no blanks, none that repeat the typed text or each
/// other (ignoring case), at most [`MAX_SUGGESTIONS`].
fn clean_suggestions(raw: Vec<String>, query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in raw {
        let text = normalise(&text);
        if text.is_empty()
            || text.eq_ignore_ascii_case(query)
            || out.iter().any(|o| o.eq_ignore_ascii_case(&text))
        {
            continue;
        }
        out.push(text);
        if out.len() == MAX_SUGGESTIONS {
            break;
        }
    }
    out
}

/// Put `text` first in `list`, removing an earlier copy (ignoring case). With `last` it goes at
/// the end instead, and only when the list does not hold it yet (merging older entries in).
fn push_recent(list: &mut Vec<String>, text: &str, last: bool) {
    let exists = list.iter().position(|r| r.eq_ignore_ascii_case(text));
    match (exists, last) {
        (Some(_), true) => {}
        (Some(i), false) => {
            list.remove(i);
            list.insert(0, text.to_owned());
        }
        (None, true) => list.push(text.to_owned()),
        (None, false) => list.insert(0, text.to_owned()),
    }
}

fn encode_recents(list: &[String]) -> String {
    list.join("\n")
}

fn decode_recents(stored: &str) -> Vec<String> {
    stored
        .lines()
        .map(normalise)
        .filter(|l| !l.is_empty())
        .collect()
}

/// Trim and collapse runs of whitespace, so "daft  punk " and "daft punk" are one query.
pub fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What a link that cannot be opened says, in six words or fewer.
pub const BAD_LINK_TEXT: &str = "Can't open that link";

/// What went wrong in six words or fewer (DIRECTION.md, States and copy).
pub fn failure_text(f: LoadFailure) -> &'static str {
    match f {
        LoadFailure::Session => "Sign-in expired",
        LoadFailure::Throttled => "Too many requests",
        LoadFailure::Network => "Connection problem",
        LoadFailure::Parse | LoadFailure::Other => "Couldn't load results",
    }
}

#[cfg(test)]
mod tests;
