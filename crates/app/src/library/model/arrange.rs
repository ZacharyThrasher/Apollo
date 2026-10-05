//! Arranging the rows of the playlist page on top: one place up or down, to the top or bottom,
//! and taking out the repeats of a song. Like every row edit of the page they show at once and go
//! to the service one at a time through the page's edit queue (see `queue_edit`): a refused one
//! puts the service's own list back and leaves the usual notice. Lives under the model so it can
//! reach that queue.

use apollo_domain::arrange::{self, Edge};
use apollo_domain::library::Collection;
use gpui::Context;

use super::{Entry, LibraryModel, PageEdit};

/// How a set of rows is arranged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arrange {
    /// One place up (`-1`) or down (`1`).
    Nudge(isize),
    /// To the top or the bottom, keeping their order among themselves.
    To(Edge),
}

/// A row that repeats an earlier one of the same playlist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repeat {
    /// The row's own token: what a removal names.
    pub occurrence: String,
    pub video_id: String,
    pub title: String,
    pub artist: String,
    /// Its place in the playlist, from 1.
    pub number: usize,
    /// The place, from 1, of the row that stays.
    pub kept: usize,
}

impl LibraryModel {
    /// Arrange the rows holding these tokens on the playlist page on top, when it is one the
    /// account owns and every row is here. They move at once; the writes follow. Returns whether
    /// anything moved (a row already at the end it moves towards moves nothing).
    pub fn arrange_rows(
        &mut self,
        playlist_key: &str,
        occurrences: &[String],
        how: Arrange,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(Entry::Collection(d)) = self.stack.last_mut() else {
            return false;
        };
        if d.collection.library_key() != playlist_key || !d.can_edit() {
            return false;
        }
        let items = d.listing.items();
        let tokens: Vec<String> = items.iter().filter_map(|r| r.occurrence.clone()).collect();
        // A row with no token cannot be named to the service, so the page cannot be arranged.
        if tokens.len() != items.len() {
            return false;
        }
        let picked: Vec<usize> = tokens
            .iter()
            .enumerate()
            .filter(|(_, t)| occurrences.contains(t))
            .map(|(at, _)| at)
            .collect();
        let order = match how {
            Arrange::Nudge(delta) => arrange::nudged(tokens.len(), &picked, delta),
            Arrange::To(edge) => arrange::to_edge(tokens.len(), &picked, edge),
        };
        let Some(order) = order else {
            return false;
        };
        let desired: Vec<String> = order.iter().map(|&i| tokens[i].clone()).collect();
        let steps = arrange::steps(&tokens, &desired);
        if steps.is_empty() || !d.listing.reorder(&order) {
            return false;
        }
        let collection = d.collection.clone();
        for step in steps {
            self.queue_edit(
                collection.clone(),
                PageEdit::Move {
                    occurrence: step.token,
                    before: step.before,
                    desired: step.after,
                },
                cx,
            );
        }
        cx.notify();
        true
    }

    /// The rows of the playlist page on top that repeat an earlier row (the same song, by video
    /// id; the first one stays), in list order. Empty for a page that cannot be edited, since
    /// nothing could be done about them.
    pub fn repeats(&self, playlist_key: &str) -> Vec<Repeat> {
        let Some(Entry::Collection(d)) = self.stack.last() else {
            return Vec::new();
        };
        if d.collection.library_key() != playlist_key || !d.can_edit() {
            return Vec::new();
        }
        let rows = d.listing.items();
        arrange::repeats(rows.iter().map(|r| r.track.video_id.as_str()))
            .into_iter()
            .filter_map(|(at, first)| {
                let row = &rows[at];
                Some(Repeat {
                    occurrence: row.occurrence.clone()?,
                    video_id: row.track.video_id.as_str().to_owned(),
                    title: row.track.title.clone(),
                    artist: row
                        .track
                        .artists
                        .iter()
                        .map(|a| a.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    number: at + 1,
                    kept: first + 1,
                })
            })
            .collect()
    }

    /// Take the rows holding these tokens out of the playlist page on top (the repeats the
    /// Remove duplicates dialog listed). Each goes at once; the writes follow one at a time, by
    /// the row's own token, so the copy that was listed is the one that goes. Returns how many.
    pub fn remove_rows(
        &mut self,
        playlist_key: &str,
        occurrences: &[String],
        cx: &mut Context<Self>,
    ) -> usize {
        let held: Vec<&String> = match self.stack.last() {
            Some(Entry::Collection(d)) => occurrences
                .iter()
                .filter(|o| {
                    d.listing
                        .items()
                        .iter()
                        .any(|r| r.occurrence.as_deref() == Some(o.as_str()))
                })
                .collect(),
            _ => return 0,
        };
        let tokens: Vec<String> = held.into_iter().cloned().collect();
        for token in &tokens {
            self.remove_row(playlist_key, token, cx);
        }
        tokens.len()
    }

    /// The collection of the page on top, when it is a playlist the account can arrange.
    pub fn arrangeable(&self) -> Option<Collection> {
        match self.stack.last() {
            Some(Entry::Collection(d)) if d.can_edit() => Some(d.collection.clone()),
            _ => None,
        }
    }
}
