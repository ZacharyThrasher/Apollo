//! Search state: the source port and the query state machine (normalise, debounce, fetch, keep
//! only the answer to the newest query), the typeahead suggestions, the recent searches and the
//! pasted-link path. No views here; `ui::search_page` draws it.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::Duration;

use apollo_domain::links::{parse_external_link, ExternalLink};
use apollo_domain::listing::LoadFailure;
use apollo_provider::{LinkTarget, SearchResults};
use gpui::{Context, EventEmitter, Task};

use crate::settings::{BoolSetting, SettingsBackend};

/// Settings > Search > Save recent searches.
pub const SAVE_RECENT_SEARCHES: BoolSetting = BoolSetting::new("save_recent_searches", true);
/// Settings > Search > Hide music videos: results keep the audio tracks only.
pub const HIDE_MUSIC_VIDEOS: BoolSetting = BoolSetting::new("hide_music_videos", false);

/// Where results come from. Blocking: the model runs it on a background thread, so an
/// implementation may do network I/O or wait on its own runtime.
pub trait SearchSource: Send + Sync {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure>;

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

pub struct SearchModel {
    source: Option<Arc<dyn SearchSource>>,
    /// The query as searched: trimmed, inner whitespace collapsed.
    query: String,
    state: SearchState,
    /// What the page lists: `raw` with the hide-music-videos choice applied.
    results: Option<Arc<SearchResults>>,
    /// The service's answer as it came, so toggling the filter needs no new request.
    raw: Option<Arc<SearchResults>>,
    /// The query `results` answer, so asking again for it costs nothing.
    answered: Option<String>,
    /// Debounce plus request. Replacing it drops the older one, so an abandoned query can
    /// never update the view.
    task: Option<Task<()>>,
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
}

impl EventEmitter<SearchEvent> for SearchModel {}

impl SearchModel {
    pub fn new() -> Self {
        Self {
            source: None,
            query: String::new(),
            state: SearchState::Idle,
            results: None,
            raw: None,
            answered: None,
            task: None,
            suggestions: Vec::new(),
            suggest_task: None,
            suggest_cache: HashMap::new(),
            recents: Vec::new(),
            recents_writer: None,
            save_recents: SAVE_RECENT_SEARCHES.default,
            hide_videos: HIDE_MUSIC_VIDEOS.default,
            link: None,
        }
    }

    /// The backend is ready. A query typed before that (state `Loading`) goes out now.
    pub fn install(&mut self, source: Arc<dyn SearchSource>, cx: &mut Context<Self>) {
        self.source = Some(source);
        if self.state == SearchState::Loading {
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

    pub fn state(&self) -> SearchState {
        self.state
    }

    /// Results of the last answered query. Kept while a newer one loads, so the page does not
    /// flash to skeletons on every keystroke.
    pub fn results(&self) -> Option<&Arc<SearchResults>> {
        self.results.as_ref()
    }

    /// A finished search that found nothing and read everything it was given.
    pub fn is_confirmed_empty(&self) -> bool {
        self.state == SearchState::Ready && self.results.as_ref().is_some_and(|r| r.is_empty())
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
            self.apply_filter();
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
        } else if self.answered.as_deref() == Some(self.query.as_str()) {
            // Typed back to what is already on screen.
            self.task = None;
            self.state = SearchState::Ready;
            cx.notify();
        } else {
            self.start(true, cx);
        }
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
                    self.task = None;
                    self.link = None;
                    self.results = None;
                    self.raw = None;
                    self.answered = None;
                    self.state = SearchState::BadLink;
                    cx.notify();
                }
            }
            return;
        }
        self.remember(cx);
        if self.answered.as_deref() == Some(self.query.as_str()) {
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

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.state = SearchState::Idle;
        self.results = None;
        self.raw = None;
        self.answered = None;
        cx.notify();
    }

    fn apply_filter(&mut self) {
        self.results = self.raw.as_ref().map(|raw| {
            if self.hide_videos && !raw.video_ids.is_empty() {
                Arc::new(raw.without_videos())
            } else {
                raw.clone()
            }
        });
    }

    fn start(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.state = SearchState::Loading;
        let query = self.query.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
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
                .spawn(async move { source.search(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| m.finish(query, result, cx));
        }));
        cx.notify();
    }

    fn finish(
        &mut self,
        query: String,
        result: Result<SearchResults, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if query != self.query {
            return;
        }
        match result {
            Ok(results) => {
                self.raw = Some(Arc::new(results));
                self.apply_filter();
                self.answered = Some(query);
                self.state = SearchState::Ready;
            }
            Err(failure) => {
                self.results = None;
                self.raw = None;
                self.answered = None;
                self.state = SearchState::Failed(failure);
            }
        }
        cx.notify();
    }

    fn open_link(&mut self, link: ExternalLink, cx: &mut Context<Self>) {
        self.state = SearchState::Loading;
        self.results = None;
        self.raw = None;
        self.answered = None;
        self.link = Some(link.clone());
        self.task = Some(cx.spawn(async move |this, cx| {
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
        }));
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
                self.state = SearchState::Idle;
                cx.emit(SearchEvent::OpenLink(target));
            }
            Err(failure) => self.state = SearchState::Failed(failure),
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
