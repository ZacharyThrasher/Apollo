//! What Ctrl+Z can take back of the edits made to playlists: songs added to one, songs taken out
//! of one, songs moved within one, and a playlist made with its songs.
//!
//! The log is short (the last [`LIMIT`] edits) and lives in memory only: it goes with the
//! session, and with the account (`LibraryWrites::reset`). The queue keeps its own history of
//! snapshots (`apollo_domain::undo`); the two are one line in time through [`Step::note`], the
//! queue history's depth when the edit was made, which is how `domain::undo::newest` tells
//! which of them Ctrl+Z takes from.
//!
//! A step holds what is needed to put the playlist back exactly: for an add, the tokens of the
//! rows the add made (so another copy of the same song is never touched); for a removal, the
//! rows with the place each had.

use std::collections::VecDeque;

use apollo_domain::library::AddedRow;

use super::source::TrackRow;

/// How many playlist edits are kept.
pub const LIMIT: usize = 20;

/// The playlist an edit was made to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistRef {
    /// The bare id (`PL…`).
    pub id: String,
    pub title: String,
}

/// A row a removal took out, with where it sat.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedRow {
    /// The row as the page showed it (its token is the old one, gone with the row).
    pub row: TrackRow,
    /// Its place in the playlist, from 0.
    pub index: usize,
    /// The token of the first row after it that the same removal did not take out: what it goes
    /// back in front of. `None` when nothing stayed after it (it goes back at the end).
    pub anchor: Option<String>,
}

/// What an add made, as far as the service has said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Made {
    /// The write is out.
    Pending,
    /// The service named the rows it made.
    Rows(Vec<AddedRow>),
    /// The add went through, but there is no telling which rows it made (the reply was lost and
    /// the playlist read back, or the reply did not name them). It cannot be undone exactly, so
    /// it is not undone at all.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Songs added to the playlist.
    Added { videos: Vec<String>, made: Made },
    /// The playlist was made holding these songs.
    Created { videos: Vec<String> },
    /// Rows taken out of the playlist.
    Removed { rows: Vec<RemovedRow> },
    /// Rows moved: the tokens of all of its rows in the order they had before.
    Reordered { order: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub id: u64,
    pub playlist: PlaylistRef,
    pub action: Action,
    /// The queue history's depth when the edit was made.
    pub note: i64,
    /// Where the edit sits in time among the other non-queue histories
    /// ([`apollo_domain::undo::next_seq`]).
    pub seq: u64,
}

impl Step {
    /// What the step did, in words for "Undid: …".
    pub fn describe(&self) -> String {
        let name = &self.playlist.title;
        match &self.action {
            Action::Added { videos, made } => {
                let count = match made {
                    Made::Rows(rows) => rows.len(),
                    _ => videos.len(),
                };
                format!("added {} to {name}", songs(count))
            }
            Action::Created { .. } => format!("created {name}"),
            Action::Removed { rows } => format!("removed {} from {name}", songs(rows.len())),
            Action::Reordered { .. } => format!("reordered {name}"),
        }
    }

    /// An add whose answer has not come yet.
    pub fn is_pending(&self) -> bool {
        matches!(self.action, Action::Added { made: Made::Pending, .. })
    }
}

fn songs(count: usize) -> String {
    match count {
        1 => "a song".to_owned(),
        n => format!("{n} songs"),
    }
}

#[derive(Debug, Default)]
pub struct PlaylistLog {
    steps: VecDeque<Step>,
    next_id: u64,
}

impl PlaylistLog {
    /// Note an edit (made with the queue history at depth `note`); returns the step's id.
    pub fn record(&mut self, playlist: PlaylistRef, action: Action, note: i64) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.steps.push_back(Step { id, playlist, action, note, seq: apollo_domain::undo::next_seq() });
        while self.steps.len() > LIMIT {
            self.steps.pop_front();
        }
        id
    }

    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The newest step.
    pub fn last(&self) -> Option<&Step> {
        self.steps.back()
    }

    /// The queue depth noted by the newest step.
    pub fn newest_note(&self) -> Option<i64> {
        self.last().map(|s| s.note)
    }

    /// The place in time of the newest step.
    pub fn newest_seq(&self) -> Option<u64> {
        self.last().map(|s| s.seq)
    }

    /// Take the newest step to undo it.
    pub fn take_last(&mut self) -> Option<Step> {
        self.steps.pop_back()
    }

    /// The service answered an add: `rows` are what it made (none named: unknown).
    pub fn settle_added(&mut self, id: u64, rows: Option<Vec<AddedRow>>) {
        if let Some(Step { action: Action::Added { made, .. }, .. }) = self.step_mut(id) {
            *made = match rows {
                Some(rows) if !rows.is_empty() => Made::Rows(rows),
                _ => Made::Unknown,
            };
        }
    }

    /// The edit did not happen (or no longer needs undoing): forget it.
    pub fn drop_step(&mut self, id: u64) {
        self.steps.retain(|s| s.id != id);
    }

    /// Some of a removal's rows did not go after all (a refused write ended the batch): they are
    /// not the step's to put back. A step left with no rows is forgotten.
    pub fn keep_removed_except(&mut self, id: u64, not_removed: &[String]) {
        let Some(Step { action: Action::Removed { rows }, .. }) = self.step_mut(id) else {
            return;
        };
        rows.retain(|r| !r.row.occurrence.as_ref().is_some_and(|t| not_removed.contains(t)));
        if rows.is_empty() {
            self.drop_step(id);
        }
    }

    pub fn clear(&mut self) {
        self.steps.clear();
    }

    fn step_mut(&mut self, id: u64) -> Option<&mut Step> {
        self.steps.iter_mut().find(|s| s.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apollo_domain::model::Track;

    fn list() -> PlaylistRef {
        PlaylistRef { id: "PLgym".into(), title: "Gym Mix".into() }
    }

    fn row(video: &str, token: &str) -> TrackRow {
        TrackRow {
            track: Track::new(video, video),
            occurrence: Some(token.into()),
            thumbnail: None,
            explicit: false,
            liked: None,
        }
    }

    fn removed(video: &str, token: &str, index: usize) -> RemovedRow {
        RemovedRow { row: row(video, token), index, anchor: None }
    }

    fn added(videos: &[&str]) -> Action {
        Action::Added {
            videos: videos.iter().map(|v| (*v).to_owned()).collect(),
            made: Made::Pending,
        }
    }

    #[test]
    fn the_log_keeps_the_last_twenty_edits() {
        let mut log = PlaylistLog::default();
        for n in 0..(LIMIT + 5) {
            log.record(list(), added(&["a"]), n as i64);
        }
        assert_eq!(log.len(), LIMIT);
        assert_eq!(log.newest_note(), Some((LIMIT + 4) as i64));
        let mut taken = 0;
        while log.take_last().is_some() {
            taken += 1;
        }
        assert_eq!(taken, LIMIT);
        assert_eq!(log.newest_note(), None);
    }

    #[test]
    fn an_add_is_pending_until_the_service_names_its_rows() {
        let mut log = PlaylistLog::default();
        let id = log.record(list(), added(&["a", "b"]), 0);
        assert!(log.last().unwrap().is_pending());
        log.settle_added(
            id,
            Some(vec![AddedRow { video_id: "a".into(), set_video_id: "S1".into() }]),
        );
        let step = log.last().unwrap();
        assert!(!step.is_pending());
        // The words count what was made, not what was asked for.
        assert_eq!(step.describe(), "added a song to Gym Mix");
    }

    #[test]
    fn an_answer_that_names_no_rows_leaves_the_add_unknown() {
        let mut log = PlaylistLog::default();
        let id = log.record(list(), added(&["a"]), 0);
        log.settle_added(id, Some(Vec::new()));
        assert!(matches!(
            log.last().unwrap().action,
            Action::Added { made: Made::Unknown, .. }
        ));
        let id = log.record(list(), added(&["a"]), 0);
        log.settle_added(id, None);
        assert!(matches!(
            log.last().unwrap().action,
            Action::Added { made: Made::Unknown, .. }
        ));
    }

    #[test]
    fn a_failed_edit_is_forgotten_wherever_it_sits() {
        let mut log = PlaylistLog::default();
        let first = log.record(list(), added(&["a"]), 0);
        log.record(list(), added(&["b"]), 0);
        log.drop_step(first);
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn a_removal_that_stopped_part_way_only_keeps_the_rows_that_went() {
        let mut log = PlaylistLog::default();
        let id = log.record(
            list(),
            Action::Removed {
                rows: vec![removed("a", "T1", 0), removed("b", "T2", 1), removed("c", "T3", 2)],
            },
            0,
        );
        log.keep_removed_except(id, &["T2".into(), "T3".into()]);
        let Action::Removed { rows } = &log.last().unwrap().action else {
            panic!("a removal");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(log.last().unwrap().describe(), "removed a song from Gym Mix");
        log.keep_removed_except(id, &["T1".into()]);
        assert!(log.is_empty(), "nothing left to put back");
    }

    #[test]
    fn the_words_for_each_kind_of_edit() {
        let mut log = PlaylistLog::default();
        log.record(list(), Action::Created { videos: vec!["a".into()] }, 0);
        assert_eq!(log.last().unwrap().describe(), "created Gym Mix");
        log.record(
            list(),
            Action::Removed { rows: vec![removed("a", "T1", 0), removed("b", "T2", 1)] },
            0,
        );
        assert_eq!(log.last().unwrap().describe(), "removed 2 songs from Gym Mix");
        log.record(list(), added(&["a", "b", "c"]), 0);
        assert_eq!(log.last().unwrap().describe(), "added 3 songs to Gym Mix");
        log.record(list(), Action::Reordered { order: vec!["T1".into()] }, 0);
        assert_eq!(log.last().unwrap().describe(), "reordered Gym Mix");
    }
}
