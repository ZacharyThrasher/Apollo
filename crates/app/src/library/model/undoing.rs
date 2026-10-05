//! Taking back a playlist edit (Ctrl+Z). The edits themselves are noted in
//! [`super::super::undo`]; this is the side that acts on them. Songs are taken back out or put
//! back through the playlist's edit queue (see `queue_edit`), so they show at once, go to the
//! service one at a time, and a refused one puts the service's own list back with a notice.

use apollo_domain::library::{Collection, CollectionKind, WriteFailure};
use apollo_domain::model::BrowseId;
use gpui::Context;

use super::{Entry, LibraryModel, PageEdit};
use crate::library::edits::{failure_text, Edit};
use crate::library::undo::{Action, Made, PlaylistRef};
use crate::library::writes::BUSY_TEXT;

/// The collection a playlist edit was made to, as the edit queue and the edits want it.
fn collection_of(playlist: &PlaylistRef) -> Collection {
    Collection {
        kind: CollectionKind::Playlist,
        id: BrowseId::new(format!("VL{}", playlist.id)),
        title: playlist.title.clone(),
        subtitle: Some("Playlist".into()),
        thumbnail: None,
    }
}

impl LibraryModel {
    /// Take back the newest playlist edit. Returns what it was, in words for "Undid: …", when
    /// there was one and it is under way; `None` when there was nothing, the newest edit is an
    /// add the service has not answered yet (nothing is taken back, and a notice says so), or
    /// it cannot be taken back exactly (a notice says that).
    pub fn undo_playlist_edit(&mut self, cx: &mut Context<Self>) -> Option<String> {
        if self.writes.read(cx).newest_edit_pending() {
            self.writes.update(cx, |w, cx| w.raise(BUSY_TEXT, cx));
            return None;
        }
        let step = self.writes.update(cx, |w, _| w.take_playlist_step())?;
        let words = step.describe();
        let collection = collection_of(&step.playlist);
        let key = collection.library_key().to_owned();
        match step.action {
            Action::Added { made: Made::Rows(rows), .. } => {
                let tokens: Vec<String> = rows.iter().map(|r| r.set_video_id.clone()).collect();
                let videos: Vec<String> = rows.iter().map(|r| r.video_id.clone()).collect();
                // The pages open on the playlist lose the rows now. A row shown for the add and
                // not yet read back has no token; it goes by its song.
                for entry in &mut self.stack {
                    if let Entry::Collection(d) = entry {
                        if d.collection.kind == CollectionKind::Playlist
                            && d.collection.library_key() == key
                        {
                            d.listing.retain(|r| match &r.occurrence {
                                Some(token) => !tokens.contains(token),
                                None => !videos.contains(&r.track.video_id.as_str().to_owned()),
                            });
                        }
                    }
                }
                self.writes.update(cx, |w, _| w.forget_added(&key, &videos));
                for row in rows {
                    self.queue_edit(
                        collection.clone(),
                        PageEdit::Unadd { video_id: row.video_id, occurrence: row.set_video_id },
                        cx,
                    );
                }
                self.refresh_when_written(&key);
                Some(words)
            }
            Action::Added { .. } => {
                // Which rows the add made is not known, so which to take out is not either.
                let text = failure_text(Edit::Undo, WriteFailure::Rejected);
                self.writes.update(cx, |w, cx| w.raise(text, cx));
                None
            }
            Action::Removed { rows } => {
                // Back on the open pages at once, without tokens until they are read again.
                for entry in &mut self.stack {
                    if let Entry::Collection(d) = entry {
                        if d.collection.kind == CollectionKind::Playlist
                            && d.collection.library_key() == key
                        {
                            for removed in &rows {
                                let mut row = removed.row.clone();
                                row.occurrence = None;
                                d.listing.insert(removed.index, row);
                            }
                        }
                    }
                }
                let items = rows
                    .iter()
                    .map(|r| (r.row.track.video_id.as_str().to_owned(), r.anchor.clone()))
                    .collect();
                self.queue_edit(collection, PageEdit::Restore { items }, cx);
                self.refresh_when_written(&key);
                cx.notify();
                Some(words)
            }
            Action::Reordered { order } => {
                // Open pages show the old order at once, when they hold exactly the rows the
                // order knows (any others, added since, stay after them).
                for entry in &mut self.stack {
                    if let Entry::Collection(d) = entry {
                        if d.collection.kind == CollectionKind::Playlist
                            && d.collection.library_key() == key
                        {
                            let items = d.listing.items();
                            let tokens: Vec<String> =
                                items.iter().filter_map(|r| r.occurrence.clone()).collect();
                            if tokens.len() == items.len() {
                                let back = apollo_domain::arrange::back_to(&tokens, &order);
                                let place: Vec<usize> = back
                                    .iter()
                                    .filter_map(|t| tokens.iter().position(|x| x == t))
                                    .collect();
                                d.listing.reorder(&place);
                            }
                        }
                    }
                }
                self.queue_edit(collection, PageEdit::Reorder { order }, cx);
                self.refresh_when_written(&key);
                cx.notify();
                Some(words)
            }
            Action::Created { videos } => {
                self.edits
                    .update(cx, |e, cx| e.undo_create(step.playlist, videos, cx));
                Some(words)
            }
        }
    }

    /// Note, for Ctrl+Z, the order a playlist's rows had before a move.
    pub(super) fn note_reorder(
        &mut self,
        collection: &Collection,
        order: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let playlist = PlaylistRef {
            id: collection.library_key().to_owned(),
            title: collection.title.clone(),
        };
        self.writes
            .update(cx, |w, cx| w.record_playlist_step(playlist, Action::Reordered { order }, cx));
    }

    /// Once the playlist's queued edits are all written, its open pages are read again.
    fn refresh_when_written(&mut self, key: &str) {
        if let Some(queue) = self.edit_queues.get_mut(key) {
            queue.refresh = true;
        }
    }
}
