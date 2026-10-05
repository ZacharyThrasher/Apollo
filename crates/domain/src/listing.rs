//! Paginated, revision-guarded result lists (BUILD_SPEC ARCH-03, DATA-02).
//!
//! Search, library and playlist views all use [`Listing`]. A new query (or
//! account change) bumps the revision; completions for an older revision are
//! rejected, so abandoned work can never update the view. Continuations are
//! scoped to a revision, repeated tokens stop pagination, and an empty result
//! is kept distinct from a failed or partially parsed one.

use std::collections::HashSet;

/// Items that can be deduplicated across overlapping pages.
pub trait ListItem {
    /// Stable identity of this *occurrence*, or `None` to never deduplicate it.
    /// Playlist rows must key on the occurrence token, not the video ID, so
    /// intentional duplicates survive.
    fn dedup_key(&self) -> Option<String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Revision(u64);

#[derive(Clone, Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub continuation: Option<String>,
    /// Sanitized count of unsupported renderers skipped while parsing this page.
    pub skipped: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadFailure {
    /// Response arrived but could not be understood. Not the same as "empty".
    Parse,
    Network,
    Session,
    Throttled,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadState {
    Idle,
    Loading,
    /// At least one page applied and there is no continuation left.
    Complete,
    /// Pages applied, more available.
    HasMore,
    /// Pagination stopped on a repeated continuation token.
    Truncated,
    Failed(LoadFailure),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Yes,
    /// Revision no longer current; the result was discarded.
    Stale,
    /// Page repeated an already-seen continuation; its items were dropped and paging stopped.
    RepeatedContinuation,
}

/// Hard cap so a misbehaving service cannot grow a list forever.
pub const MAX_PAGES: usize = 500;

pub struct Listing<T> {
    revision: Revision,
    next_revision: u64,
    items: Vec<T>,
    keys: HashSet<String>,
    seen_tokens: HashSet<String>,
    continuation: Option<String>,
    pages: usize,
    skipped: u32,
    state: LoadState,
}

impl<T: ListItem> Default for Listing<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: ListItem> Listing<T> {
    pub fn new() -> Self {
        Self {
            revision: Revision(0),
            next_revision: 1,
            items: Vec::new(),
            keys: HashSet::new(),
            seen_tokens: HashSet::new(),
            continuation: None,
            pages: 0,
            skipped: 0,
            state: LoadState::Idle,
        }
    }

    /// Start a fresh listing. Everything in flight for earlier revisions becomes stale.
    pub fn begin(&mut self) -> Revision {
        let rev = Revision(self.next_revision);
        self.next_revision += 1;
        self.revision = rev;
        self.items.clear();
        self.keys.clear();
        self.seen_tokens.clear();
        self.continuation = None;
        self.pages = 0;
        self.skipped = 0;
        self.state = LoadState::Loading;
        rev
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }
    pub fn items(&self) -> &[T] {
        &self.items
    }
    pub fn state(&self) -> LoadState {
        self.state
    }
    /// Token for the next page request, tied to the current revision.
    pub fn continuation(&self) -> Option<(Revision, &str)> {
        self.continuation.as_deref().map(|c| (self.revision, c))
    }
    pub fn skipped(&self) -> u32 {
        self.skipped
    }
    /// True only for a successfully completed load with no items. A failed or
    /// still-loading list is never "empty", and neither is one whose rows were all
    /// unreadable: that is a parse problem, not an empty library.
    pub fn is_confirmed_empty(&self) -> bool {
        self.state == LoadState::Complete && self.items.is_empty() && self.skipped == 0
    }

    /// Drop the items `keep` rejects, without touching paging state. Returns how many went. The
    /// keys of dropped items are forgotten, so a later page can bring one back.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) -> usize {
        let before = self.items.len();
        let keys = &mut self.keys;
        self.items.retain(|item| {
            let kept = keep(item);
            if !kept {
                if let Some(k) = item.dedup_key() {
                    keys.remove(&k);
                }
            }
            kept
        });
        before - self.items.len()
    }

    /// Put one item at the end (a row shown before the service has confirmed it). Paging state
    /// is untouched. An item whose key is already held is not added twice; returns whether it
    /// was added.
    pub fn push(&mut self, item: T) -> bool {
        if let Some(k) = item.dedup_key() {
            if !self.keys.insert(k) {
                return false;
            }
        }
        self.items.push(item);
        true
    }

    /// Put one item at place `at` (the end when that is past it): a row shown before the service
    /// has confirmed it. Paging state is untouched. An item whose key is already held is not
    /// added twice; returns whether it was added.
    pub fn insert(&mut self, at: usize, item: T) -> bool {
        if let Some(k) = item.dedup_key() {
            if !self.keys.insert(k) {
                return false;
            }
        }
        self.items.insert(at.min(self.items.len()), item);
        true
    }

    /// Take the row at `from` and put it before the insertion slot `slot` (0 above the first row,
    /// `len` below the last). Returns whether anything moved. Paging state is untouched.
    pub fn move_item(&mut self, from: usize, slot: usize) -> bool {
        if crate::library::plan_move(self.items.len(), from, slot).is_none() {
            return false;
        }
        let item = self.items.remove(from);
        let at = if slot > from { slot - 1 } else { slot };
        self.items.insert(at, item);
        true
    }

    /// Put the items in a new order: place `n` takes the item that was at `order[n]`. Returns
    /// whether it did; `order` that is not a permutation of the list changes nothing. Paging state
    /// is untouched.
    pub fn reorder(&mut self, order: &[usize]) -> bool {
        let len = self.items.len();
        let mut seen = vec![false; len];
        if order.len() != len
            || order
                .iter()
                .any(|&i| i >= len || std::mem::replace(&mut seen[i], true))
        {
            return false;
        }
        let mut taken: Vec<Option<T>> = std::mem::take(&mut self.items)
            .into_iter()
            .map(Some)
            .collect();
        self.items = order.iter().filter_map(|&i| taken[i].take()).collect();
        true
    }

    /// A request for the next page is marked in flight.
    pub fn mark_loading_more(&mut self, rev: Revision) -> bool {
        if rev != self.revision || self.continuation.is_none() {
            return false;
        }
        self.state = LoadState::Loading;
        true
    }

    pub fn apply(&mut self, rev: Revision, page: Page<T>) -> Applied {
        if rev != self.revision {
            return Applied::Stale;
        }
        // Stop on a repeated continuation: the page that *carries* a token we have
        // already followed is a loop, not new data.
        if let Some(token) = &page.continuation {
            if self.seen_tokens.contains(token) {
                self.continuation = None;
                self.state = LoadState::Truncated;
                return Applied::RepeatedContinuation;
            }
        }
        for item in page.items {
            match item.dedup_key() {
                Some(k) => {
                    if self.keys.insert(k) {
                        self.items.push(item);
                    }
                }
                None => self.items.push(item),
            }
        }
        self.skipped += page.skipped;
        self.pages += 1;
        match page.continuation {
            Some(token) if self.pages < MAX_PAGES => {
                self.seen_tokens.insert(token.clone());
                self.continuation = Some(token);
                self.state = LoadState::HasMore;
            }
            Some(_) => {
                self.continuation = None;
                self.state = LoadState::Truncated;
            }
            None => {
                self.continuation = None;
                self.state = LoadState::Complete;
            }
        }
        Applied::Yes
    }

    /// Record a failed request. Items already shown are kept.
    pub fn fail(&mut self, rev: Revision, failure: LoadFailure) -> Applied {
        if rev != self.revision {
            return Applied::Stale;
        }
        self.state = LoadState::Failed(failure);
        Applied::Yes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Row(&'static str);
    impl ListItem for Row {
        fn dedup_key(&self) -> Option<String> {
            Some(self.0.to_owned())
        }
    }
    /// Playlist occurrence: never deduplicated.
    #[derive(Clone, Debug, PartialEq)]
    struct Occ(&'static str);
    impl ListItem for Occ {
        fn dedup_key(&self) -> Option<String> {
            None
        }
    }

    fn page<T>(items: Vec<T>, cont: Option<&str>) -> Page<T> {
        Page {
            items,
            continuation: cont.map(str::to_owned),
            skipped: 0,
        }
    }

    #[test]
    fn move_item_reorders_without_touching_paging() {
        let mut l = Listing::new();
        let rev = l.begin();
        l.apply(rev, page(vec![Occ("a"), Occ("b"), Occ("c")], Some("t1")));
        assert!(l.move_item(2, 0));
        assert_eq!(l.items(), [Occ("c"), Occ("a"), Occ("b")]);
        assert!(!l.move_item(1, 1), "onto itself");
        assert!(!l.move_item(1, 2), "just below itself");
        assert!(l.move_item(0, 3), "to the end");
        assert_eq!(l.items(), [Occ("a"), Occ("b"), Occ("c")]);
        assert_eq!(l.state(), LoadState::HasMore);
        assert_eq!(l.continuation(), Some((rev, "t1")));
    }

    #[test]
    fn insert_puts_a_row_at_a_place_or_at_the_end_without_touching_paging() {
        let mut l = Listing::new();
        let rev = l.begin();
        l.apply(rev, page(vec![Occ("a"), Occ("b")], Some("t1")));
        assert!(l.insert(1, Occ("x")));
        assert_eq!(l.items(), [Occ("a"), Occ("x"), Occ("b")]);
        assert!(l.insert(99, Occ("z")), "past the end is the end");
        assert_eq!(l.items(), [Occ("a"), Occ("x"), Occ("b"), Occ("z")]);
        assert_eq!(l.continuation(), Some((rev, "t1")));
        let mut keyed = Listing::new();
        let rev = keyed.begin();
        keyed.apply(rev, page(vec![Row("a")], None));
        assert!(!keyed.insert(0, Row("a")), "a held key is not added twice");
    }

    #[test]
    fn reorder_takes_a_permutation_and_nothing_else() {
        let mut l = Listing::new();
        let rev = l.begin();
        l.apply(rev, page(vec![Occ("a"), Occ("b"), Occ("c")], Some("t1")));
        assert!(l.reorder(&[2, 0, 1]));
        assert_eq!(l.items(), [Occ("c"), Occ("a"), Occ("b")]);
        assert!(!l.reorder(&[0, 0, 1]), "a row twice");
        assert!(!l.reorder(&[0, 1]), "a row short");
        assert!(!l.reorder(&[0, 1, 3]), "no such row");
        assert_eq!(l.items(), [Occ("c"), Occ("a"), Occ("b")]);
        assert_eq!(l.continuation(), Some((rev, "t1")));
    }

    #[test]
    fn retain_drops_rows_and_lets_a_later_page_bring_one_back() {
        let mut l = Listing::new();
        let rev = l.begin();
        l.apply(rev, page(vec![Row("a"), Row("b"), Row("c")], Some("t1")));
        assert_eq!(l.retain(|r| r.0 != "b"), 1);
        assert_eq!(l.items(), [Row("a"), Row("c")]);
        assert_eq!(l.state(), LoadState::HasMore, "paging is untouched");
        // "a" is still known, so a repeat is dropped; "b" was forgotten, so it can return.
        l.apply(rev, page(vec![Row("a"), Row("b")], None));
        assert_eq!(l.items(), [Row("a"), Row("c"), Row("b")]);
    }

    // TEST-17
    #[test]
    fn results_for_an_abandoned_query_never_update_the_view() {
        let mut l = Listing::new();
        let first = l.begin();
        let second = l.begin(); // user typed again
                                // The slow first query completes after the second began.
        assert_eq!(l.apply(first, page(vec![Row("old")], None)), Applied::Stale);
        assert!(l.items().is_empty());
        assert_eq!(l.apply(second, page(vec![Row("new")], None)), Applied::Yes);
        assert_eq!(l.items(), [Row("new")]);
        // A late failure of the old query cannot mark the new one failed either.
        assert_eq!(l.fail(first, LoadFailure::Network), Applied::Stale);
        assert_eq!(l.state(), LoadState::Complete);
    }

    #[test]
    fn continuation_is_tied_to_its_revision() {
        let mut l = Listing::new();
        let r1 = l.begin();
        l.apply(r1, page(vec![Row("a")], Some("tok1")));
        assert_eq!(l.continuation(), Some((r1, "tok1")));
        let r2 = l.begin();
        assert_eq!(
            l.continuation(),
            None,
            "a new revision never inherits a cursor"
        );
        assert!(!l.mark_loading_more(r1));
        assert_eq!(l.apply(r1, page(vec![Row("b")], None)), Applied::Stale);
        assert!(l.items().is_empty());
        let _ = r2;
    }

    // TEST-24
    #[test]
    fn repeated_continuation_stops_pagination() {
        let mut l = Listing::new();
        let r = l.begin();
        l.apply(r, page(vec![Row("a")], Some("t1")));
        l.apply(r, page(vec![Row("b")], Some("t2")));
        // The service loops back to a token we already followed.
        assert_eq!(
            l.apply(r, page(vec![Row("c")], Some("t1"))),
            Applied::RepeatedContinuation
        );
        assert_eq!(l.state(), LoadState::Truncated);
        assert_eq!(l.continuation(), None);
        assert_eq!(l.items(), [Row("a"), Row("b")]);
    }

    #[test]
    fn page_cap_prevents_runaway_lists() {
        let mut l = Listing::new();
        let r = l.begin();
        for i in 0..MAX_PAGES {
            let tok = format!("t{i}");
            l.apply(
                r,
                Page {
                    items: vec![Occ("x")],
                    continuation: Some(tok),
                    skipped: 0,
                },
            );
        }
        assert_eq!(l.state(), LoadState::Truncated);
        assert_eq!(l.continuation(), None);
    }

    #[test]
    fn overlapping_pages_dedupe_but_intentional_duplicates_survive() {
        let mut search = Listing::new();
        let r = search.begin();
        search.apply(r, page(vec![Row("a"), Row("b")], Some("t")));
        search.apply(r, page(vec![Row("b"), Row("c")], None));
        assert_eq!(search.items(), [Row("a"), Row("b"), Row("c")]);

        let mut playlist = Listing::new();
        let r = playlist.begin();
        playlist.apply(r, page(vec![Occ("song"), Occ("song")], Some("t")));
        playlist.apply(r, page(vec![Occ("song")], None));
        assert_eq!(
            playlist.items().len(),
            3,
            "same track three times stays three rows"
        );
    }

    #[test]
    fn empty_is_distinct_from_failed_and_from_loading() {
        let mut l: Listing<Row> = Listing::new();
        assert!(!l.is_confirmed_empty(), "idle is not empty");
        let r = l.begin();
        assert!(!l.is_confirmed_empty(), "loading is not empty");
        l.fail(r, LoadFailure::Parse);
        assert_eq!(l.state(), LoadState::Failed(LoadFailure::Parse));
        assert!(
            !l.is_confirmed_empty(),
            "a parse failure must not look like an empty library"
        );
        let r = l.begin();
        l.apply(r, page(vec![], None));
        assert!(l.is_confirmed_empty());
    }

    #[test]
    fn partial_parse_keeps_good_content_and_counts_skips() {
        let mut l = Listing::new();
        let r = l.begin();
        let applied = l.apply(
            r,
            Page {
                items: vec![Row("a")],
                continuation: None,
                skipped: 2,
            },
        );
        assert_eq!(applied, Applied::Yes);
        assert_eq!(l.items(), [Row("a")]);
        assert_eq!(l.skipped(), 2);
    }

    #[test]
    fn failure_after_some_pages_keeps_what_was_shown() {
        let mut l = Listing::new();
        let r = l.begin();
        l.apply(r, page(vec![Row("a")], Some("t")));
        assert!(l.mark_loading_more(r));
        l.fail(r, LoadFailure::Throttled);
        assert_eq!(l.items(), [Row("a")]);
        assert_eq!(l.state(), LoadState::Failed(LoadFailure::Throttled));
    }

    #[test]
    fn a_page_of_only_unreadable_rows_is_not_an_empty_list() {
        let mut l = Listing::<Row>::new();
        let rev = l.begin();
        l.apply(
            rev,
            Page {
                items: vec![],
                continuation: None,
                skipped: 3,
            },
        );
        assert_eq!(l.state(), LoadState::Complete);
        assert_eq!(l.skipped(), 3);
        assert!(!l.is_confirmed_empty());
    }
}
