//! Swapping the songs of the playlist page on top for other versions of themselves (explicit or
//! clean). The searches that find the other versions run apart from the page (see
//! `crate::playlist_swap`); once they are all done this puts the result in: the rows change at
//! once, and the write goes out through the playlist's edit queue (see `queue_edit`), so it keeps
//! its order among the page's other edits. The whole swap is one step for Ctrl+Z.

use std::sync::Arc;

use gpui::Context;

use apollo_domain::library::{is_user_playlist_id, Collection};
use apollo_domain::listing::LoadState;

use super::{Entry, LibraryModel, PageEdit};
use crate::library::edits::Swap;
use crate::library::source::{LibrarySource, TrackRow};
use crate::library::undo::{Action, PlaylistRef, SwappedRow};

/// A song of a playlist and the version that takes its place.
#[derive(Clone, Debug)]
pub struct SwapFound {
    /// The row as the page showed it (its token names it to the service).
    pub old: TrackRow,
    /// The version found: it is shown as soon as the swap is, without a token.
    pub new: TrackRow,
}

/// A playlist page a swap can start from.
#[derive(Clone)]
pub struct SwapPage {
    pub collection: Collection,
    /// Every row of the playlist, when the page has them all (and each can be named to the
    /// service). `None` while the page has only read part of a long playlist: the swap then reads
    /// the whole playlist itself, with `source`.
    pub rows: Option<Vec<TrackRow>>,
    /// The account the person acted in: a swap finished in another one is dropped.
    pub account: Option<String>,
    pub source: Arc<dyn LibrarySource>,
}

impl LibraryModel {
    /// Whether the page on top is the playlist `playlist_key`, whatever it has loaded so far (or
    /// failed to): where a swap can be offered. That the account owns it is the menu's to know;
    /// a swap that has to read the playlist itself does so with the source, not the page.
    pub fn is_own_page(&self, playlist_key: &str) -> bool {
        matches!(
            self.stack.last(),
            Some(Entry::Collection(d))
                if d.collection.library_key() == playlist_key
                    && is_user_playlist_id(d.collection.library_key())
        )
    }

    /// What a swap starts from, when the page on top is `playlist_key` and one the account owns.
    pub fn swap_page(&self, playlist_key: &str) -> Option<SwapPage> {
        if !self.is_own_page(playlist_key) {
            return None;
        }
        let Some(Entry::Collection(d)) = self.stack.last() else {
            return None;
        };
        let source = self.source.clone()?;
        let rows = d.listing.items();
        let complete = d.can_edit() && rows.iter().all(|r| r.occurrence.is_some());
        Some(SwapPage {
            collection: d.collection.clone(),
            rows: complete.then(|| rows.to_vec()),
            account: source.account_token(),
            source,
        })
    }

    /// Put the versions found into the playlist: each row changes for the song found, in its
    /// place, on every open page of it, and one write goes to the service. Songs whose row left
    /// an open page meanwhile are left out. Returns the Ctrl+Z step, or `None` when nothing is
    /// left to swap.
    pub fn apply_swap(
        &mut self,
        collection: &Collection,
        account: &Option<String>,
        found: Vec<SwapFound>,
        explicit: bool,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        // Signed in elsewhere since the swap began: it is not for this library.
        if self.source.as_ref().and_then(|s| s.account_token()) != *account {
            return None;
        }
        let key = collection.library_key().to_owned();
        let open: Vec<usize> = self
            .stack
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Entry::Collection(d) if d.collection.library_key() == key))
            .map(|(at, _)| at)
            .collect();
        // A page that has read the whole playlist decides which rows are still there; one that
        // has read only part of it knows nothing of the rest.
        let found: Vec<SwapFound> = found
            .into_iter()
            .filter(|f| {
                let token = f.old.occurrence.as_deref();
                open.iter().all(|&at| match &self.stack[at] {
                    Entry::Collection(d) => {
                        d.listing.state() != LoadState::Complete
                            || d.listing.items().iter().any(|r| r.occurrence.as_deref() == token)
                    }
                    Entry::Artist(_) => true,
                })
            })
            .collect();
        if found.is_empty() {
            return None;
        }
        for &at in &open {
            let Entry::Collection(d) = &mut self.stack[at] else {
                continue;
            };
            for f in &found {
                let token = f.old.occurrence.as_deref();
                let Some(place) = d.listing.items().iter().position(|r| r.occurrence.as_deref() == token)
                else {
                    continue;
                };
                d.listing.retain(|r| r.occurrence.as_deref() != token);
                let mut row = f.new.clone();
                row.occurrence = None;
                d.listing.insert(place, row);
            }
        }
        let (pairs, rows): (Vec<Swap>, Vec<SwappedRow>) = found
            .iter()
            .filter_map(|f| {
                let pair = Swap {
                    out_video: f.old.track.video_id.as_str().to_owned(),
                    out_token: f.old.occurrence.clone()?,
                    in_video: f.new.track.video_id.as_str().to_owned(),
                };
                let row = SwappedRow {
                    old: f.old.clone(),
                    new_video: pair.in_video.clone(),
                    new_token: None,
                };
                Some((pair, row))
            })
            .unzip();
        let playlist = PlaylistRef { id: key.clone(), title: collection.title.clone() };
        let step = self.writes.update(cx, |w, cx| {
            w.record_playlist_step(playlist, Action::Swapped { rows, explicit, settled: false }, cx)
        });
        self.queue_edit(collection.clone(), PageEdit::Swap { pairs, step: Some(step) }, cx);
        self.refresh_when_written(&key);
        cx.notify();
        Some(step)
    }
}
