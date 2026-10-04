//! Reads the whole library through [`LibrarySource`], reads only.
//!
//! Every call here is one of the source's *reads* (`collections`, `tracks`, `more_tracks`,
//! `subscribed_artists`); none of its writes is ever called, and the tests prove it with a
//! source that records them. The requests go one at a time with a short pause between them: a
//! large library is a few hundred requests, and an unofficial client has no business bursting.

use std::collections::HashSet;
use std::thread::sleep;
use std::time::Duration;

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::{ListItem, LoadFailure, MAX_PAGES};

use super::{ExportEntry, ExportPlaylist, ExportTrack, LibraryExport, Problem};
use crate::library::model::split_collections;
use crate::library::source::failure_text;
use crate::library::{LibrarySource, TrackRow};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Waited before each request.
    pub pause: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pause: Duration::from_millis(150),
        }
    }
}

/// Why nothing could be exported at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    Failed(LoadFailure),
    /// Somebody signed out or switched channel while the export ran, so what was read is a mix
    /// of two accounts. Nothing is written.
    AccountChanged,
}

impl ExportError {
    /// What the row shows: what happened, in a few words.
    pub fn text(self) -> &'static str {
        match self {
            Self::Failed(failure) => failure_text(failure),
            Self::AccountChanged => "Account changed",
        }
    }
}

impl LibraryExport {
    /// What the row says when the file was written but is missing something; `None` when it is
    /// complete.
    pub fn problem_text(&self) -> Option<String> {
        match self.problems.len() {
            0 => None,
            1 => Some("Saved without 1 item".to_owned()),
            n => Some(format!("Saved without {n} items")),
        }
    }
}

/// Reads the library. `now_unix` stamps the file.
pub fn collect(
    source: &dyn LibrarySource,
    now_unix: u64,
    options: Options,
) -> Result<LibraryExport, ExportError> {
    let account = source.account_token();
    let same_account = || -> Result<(), ExportError> {
        if source.account_token() == account {
            Ok(())
        } else {
            Err(ExportError::AccountChanged)
        }
    };
    let reader = Reader { source, options };

    let all = reader
        .read(|| source.collections())
        .map_err(ExportError::Failed)?;
    same_account()?;
    let lists = split_collections(all);

    let mut export = LibraryExport {
        exported_at: now_unix,
        liked_songs: Vec::new(),
        playlists: Vec::new(),
        saved_albums: Vec::new(),
        subscribed_artists: Vec::new(),
        problems: Vec::new(),
    };

    for collection in &lists.playlists {
        let (rows, failure) = reader.tracks(collection);
        // A signed-out session ends everything: the rest would fail the same way.
        if failure == Some(LoadFailure::Session) {
            return Err(ExportError::Failed(LoadFailure::Session));
        }
        same_account()?;
        if let Some(failure) = failure {
            export.problems.push(Problem {
                what: format!("Playlist \"{}\"", collection.title),
                reason: failure_text(failure).to_owned(),
            });
        }
        let tracks: Vec<ExportTrack> = rows.iter().map(ExportTrack::from).collect();
        if collection.is_liked_music() {
            export.liked_songs = tracks;
        } else {
            export.playlists.push(ExportPlaylist::new(
                collection.id.as_str(),
                &collection.title,
                collection.subtitle.as_deref(),
                tracks,
            ));
        }
    }

    // Saved albums are entries, not their songs: the list is what a library is made of, and an
    // album's songs are one search away.
    export.saved_albums = lists
        .albums
        .iter()
        .filter(|c| c.kind == CollectionKind::Album)
        .map(|c| ExportEntry::album(c.id.as_str(), &c.title, c.subtitle.as_deref()))
        .collect();

    match reader.read(|| source.subscribed_artists()) {
        Ok(hits) => {
            export.subscribed_artists = hits
                .iter()
                .map(|h| ExportEntry::artist(h.id.as_str(), &h.title, h.subtitle.as_deref()))
                .collect();
        }
        Err(LoadFailure::Session) => return Err(ExportError::Failed(LoadFailure::Session)),
        Err(failure) => export.problems.push(Problem {
            what: "Subscribed artists".to_owned(),
            reason: failure_text(failure).to_owned(),
        }),
    }
    same_account()?;
    Ok(export)
}

struct Reader<'a> {
    source: &'a dyn LibrarySource,
    options: Options,
}

impl Reader<'_> {
    /// One read, after the pause, retried once when the connection or the service's rate limit
    /// was the problem (and never when the session is gone).
    fn read<T>(&self, mut call: impl FnMut() -> Result<T, LoadFailure>) -> Result<T, LoadFailure> {
        sleep(self.options.pause);
        match call() {
            Err(LoadFailure::Network | LoadFailure::Throttled) => {
                sleep(self.options.pause * 4);
                call()
            }
            other => other,
        }
    }

    /// Every row of a collection: its first page, then the continuations. The rows read so far
    /// come back with the failure that stopped the reading, if one did.
    fn tracks(&self, collection: &Collection) -> (Vec<TrackRow>, Option<LoadFailure>) {
        let mut rows: Vec<TrackRow> = Vec::new();
        let mut seen_rows = HashSet::new();
        let mut seen_tokens = HashSet::new();
        let mut page = match self.read(|| self.source.tracks(collection)) {
            Ok(page) => page,
            Err(failure) => return (rows, Some(failure)),
        };
        for _ in 0..MAX_PAGES {
            for row in page.items {
                // The same song twice in a playlist is two rows with two tokens; only a row
                // that two overlapping pages both carried is dropped.
                if row.dedup_key().is_none_or(|key| seen_rows.insert(key)) {
                    rows.push(row);
                }
            }
            let Some(token) = page.continuation.take() else {
                return (rows, None);
            };
            if !seen_tokens.insert(token.clone()) {
                // The service handed back a token it already gave: stop instead of looping.
                return (rows, Some(LoadFailure::Parse));
            }
            page = match self.read(|| self.source.more_tracks(&token)) {
                Ok(page) => page,
                Err(failure) => return (rows, Some(failure)),
            };
        }
        (rows, Some(LoadFailure::Other))
    }
}

#[cfg(test)]
mod tests;
