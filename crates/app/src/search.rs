//! Search state: the source port and the query state machine (normalise, debounce, fetch, keep
//! only the answer to the newest query). No views here; `ui::search_page` draws it.

use std::sync::Arc;
use std::time::Duration;

use apollo_domain::listing::LoadFailure;
use apollo_provider::SearchResults;
use gpui::{Context, Task};

/// Where results come from. Blocking: the model runs it on a background thread, so an
/// implementation may do network I/O or wait on its own runtime.
pub trait SearchSource: Send + Sync {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure>;
}

/// Typing pauses this long before a query goes out. The service writes every signed-in search
/// to the account's history, so this is also what keeps half-typed words out of it.
/// Stands in when the backend could not start: every search fails with a short message.
pub struct Unavailable;

impl SearchSource for Unavailable {
    fn search(&self, _: &str) -> Result<SearchResults, LoadFailure> {
        Err(LoadFailure::Other)
    }
}

pub const DEBOUNCE: Duration = Duration::from_millis(400);

/// Typing shorter than this never searches by itself; Enter still does.
pub const MIN_PREVIEW_CHARS: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchState {
    /// Nothing to show: no query, or one too short to search for.
    Idle,
    /// A query is waiting out the debounce or is in flight.
    Loading,
    /// The newest query has an answer (possibly an empty one).
    Ready,
    Failed(LoadFailure),
}

pub struct SearchModel {
    source: Option<Arc<dyn SearchSource>>,
    /// The query as searched: trimmed, inner whitespace collapsed.
    query: String,
    state: SearchState,
    results: Option<Arc<SearchResults>>,
    /// The query `results` answer, so asking again for it costs nothing.
    answered: Option<String>,
    /// Debounce plus request. Replacing it drops the older one, so an abandoned query can
    /// never update the view.
    task: Option<Task<()>>,
}

impl SearchModel {
    pub fn new() -> Self {
        Self {
            source: None,
            query: String::new(),
            state: SearchState::Idle,
            results: None,
            answered: None,
            task: None,
        }
    }

    /// The backend is ready. A query typed before that (state `Loading`) goes out now.
    pub fn install(&mut self, source: Arc<dyn SearchSource>, cx: &mut Context<Self>) {
        self.source = Some(source);
        if self.state == SearchState::Loading {
            self.start(false, cx);
        }
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

    /// The text field changed. Debounced; below [`MIN_PREVIEW_CHARS`] it clears the page.
    pub fn set_query(&mut self, text: &str, cx: &mut Context<Self>) {
        let query = normalise(text);
        if query == self.query {
            return;
        }
        self.query = query;
        if self.query.chars().count() < MIN_PREVIEW_CHARS {
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

    /// Enter: search now, with no debounce and with any length. A query already answered is not
    /// asked again.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.query.is_empty() || self.answered.as_deref() == Some(self.query.as_str()) {
            return;
        }
        self.start(false, cx);
    }

    /// Ask again after a failure.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if !self.query.is_empty() {
            self.start(false, cx);
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.task = None;
        self.state = SearchState::Idle;
        self.results = None;
        self.answered = None;
        cx.notify();
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
                self.results = Some(Arc::new(results));
                self.answered = Some(query);
                self.state = SearchState::Ready;
            }
            Err(failure) => {
                self.results = None;
                self.answered = None;
                self.state = SearchState::Failed(failure);
            }
        }
        cx.notify();
    }
}

impl Default for SearchModel {
    fn default() -> Self {
        Self::new()
    }
}

/// Trim and collapse runs of whitespace, so "daft  punk " and "daft punk" are one query.
pub fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

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
