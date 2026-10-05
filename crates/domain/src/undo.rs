//! Undo and redo for queue edits.
//!
//! The history keeps whole [`QueueSnapshot`]s: an entry's id is stable and a snapshot is only
//! track ids and ordering, so putting one back is a plain swap. What makes it more than that is
//! the track that is playing: undoing must not interrupt it. [`put_back`] rebuilds the old
//! queue *around* what plays now, and only when the undone action itself changed the playing
//! track (a new list replaced the queue, a radio started) does the old track come back, as the
//! caller's job to hold paused at its old position.

use std::collections::VecDeque;

use crate::model::QueueEntryId;
use crate::queue::{Queue, QueueSnapshot};

/// How many steps are kept. Well past the 20 the owner asked for; a snapshot is small.
pub const DEPTH: usize = 50;

/// The queue as it was before one action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub snapshot: QueueSnapshot,
    /// Where the current track was when the snapshot was taken (used when it comes back).
    pub position_ms: u64,
    /// The action this step undoes changed which track is playing.
    pub changed_current: bool,
}

fn current_id(snapshot: &QueueSnapshot) -> Option<QueueEntryId> {
    snapshot.current.as_ref().map(|(entry, _)| entry.id)
}

#[derive(Debug)]
pub struct UndoHistory {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    limit: usize,
    /// Bumped by every change to the stacks, so a caller can tell nothing else was recorded
    /// between two moments.
    generation: u64,
    /// How many steps are undoable, counted past the ones the limit dropped: a record or redo
    /// adds one, an undo takes one off. Other histories (playlist edits) note it when they
    /// record, which is how they tell what happened before and after them.
    depth: i64,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoHistory {
    pub fn new() -> Self {
        Self::with_limit(DEPTH)
    }

    pub fn with_limit(limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            limit: limit.max(1),
            generation: 0,
            depth: 0,
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// How deep the history is now; see the field.
    pub fn depth(&self) -> i64 {
        self.depth
    }

    /// Forget everything (sign-out, a different account).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.generation += 1;
        self.depth = 0;
    }

    /// An action turned `before` (with the playing track at `position_ms`) into `after`.
    /// Returns whether it was recorded: an action that changed nothing is not a step. A new
    /// step ends the redo line.
    pub fn record(
        &mut self,
        before: QueueSnapshot,
        position_ms: u64,
        after: &QueueSnapshot,
    ) -> bool {
        if &before == after {
            return false;
        }
        let changed_current = current_id(&before) != current_id(after);
        self.undo.push_back(Step {
            snapshot: before,
            position_ms,
            changed_current,
        });
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.generation += 1;
        self.depth += 1;
        true
    }

    /// Take the latest step to put back. `live` (the queue now, with the playing track at
    /// `position_ms`) becomes what a redo returns to.
    pub fn undo(&mut self, live: QueueSnapshot, position_ms: u64) -> Option<Step> {
        let step = self.undo.pop_back()?;
        self.redo.push(Step {
            snapshot: live,
            position_ms,
            changed_current: step.changed_current,
        });
        self.generation += 1;
        self.depth -= 1;
        Some(step)
    }

    /// The reverse of [`UndoHistory::undo`].
    pub fn redo(&mut self, live: QueueSnapshot, position_ms: u64) -> Option<Step> {
        let step = self.redo.pop()?;
        self.undo.push_back(Step {
            snapshot: live,
            position_ms,
            changed_current: step.changed_current,
        });
        while self.undo.len() > self.limit {
            self.undo.pop_front();
        }
        self.generation += 1;
        self.depth += 1;
        Some(step)
    }
}

/// A number that only grows, from one call to the next anywhere in the process. The histories
/// that are not the queue's (playlist edits, Crate swaps) stamp each step with one when they
/// make it, which is how Ctrl+Z tells which of them is newest.
pub fn next_seq() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
}

/// Which history Ctrl+Z takes from. The queue's steps and the playlist edits' steps are kept
/// apart (different things to put back), but they are one line in time: the newest of either
/// goes first. A playlist edit notes the queue history's [`UndoHistory::depth`] when it is
/// made; queue steps recorded since then leave the depth above that note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Newest {
    Queue,
    Playlist,
    Nothing,
}

/// The newest step of the two histories. `playlist_note` is the queue depth noted by the newest
/// playlist step (`None`: there is none); `queue_depth` the queue history's depth now, and
/// `queue_undoable` whether it has a step to take.
pub fn newest(queue_depth: i64, queue_undoable: bool, playlist_note: Option<i64>) -> Newest {
    match playlist_note {
        Some(note) if queue_undoable && queue_depth > note => Newest::Queue,
        Some(_) => Newest::Playlist,
        None if queue_undoable => Newest::Queue,
        None => Newest::Nothing,
    }
}

/// The queue `step` describes, rebuilt around what plays in `live`.
///
/// * The playing entry is in the step (anywhere): it stays current and the step's order is
///   put around it, so entries it had played become history and the rest up next.
/// * It is not, and the undone action did not change the playing track (the track only started
///   later, from entries the action added): it keeps playing, as a row taken out of the queue.
/// * It is not, and the action changed the playing track: the step's own current track is
///   current again, and the caller holds it paused at the step's position.
///
/// Ids already used stay used: the rebuilt queue never mints one a live entry still has.
/// `None` when the step does not describe a valid queue.
pub fn put_back(step: &Step, live: &Queue) -> Option<Queue> {
    let mut snapshot = step.snapshot.clone();
    snapshot.next_id = snapshot.next_id.max(live.next_id());
    let mut queue = Queue::restore(snapshot).ok()?;
    if let Some(playing) = live.current() {
        if queue.contains(playing.id) {
            queue.jump_to(playing.id);
        } else if !step.changed_current {
            queue.keep_playing(playing.clone());
        }
    }
    Some(queue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;

    fn t(id: &str) -> Track {
        Track::new(id, id)
    }
    fn tracks(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|i| t(i)).collect()
    }
    fn vids<'a>(it: impl Iterator<Item = &'a crate::queue::QueueEntry>) -> Vec<String> {
        it.map(|e| e.track.video_id.as_str().to_owned()).collect()
    }
    fn cur(q: &Queue) -> String {
        q.current().unwrap().track.video_id.as_str().to_owned()
    }

    /// Run `edit` on `q`, recording it in `h` like the player does.
    fn act(h: &mut UndoHistory, q: &mut Queue, edit: impl FnOnce(&mut Queue)) -> bool {
        let before = q.snapshot();
        edit(q);
        h.record(before, 0, &q.snapshot())
    }

    fn undo(h: &mut UndoHistory, q: &mut Queue) -> bool {
        let Some(step) = h.undo(q.snapshot(), 0) else {
            return false;
        };
        *q = put_back(&step, q).expect("a valid step");
        true
    }

    fn redo(h: &mut UndoHistory, q: &mut Queue) -> bool {
        let Some(step) = h.redo(q.snapshot(), 0) else {
            return false;
        };
        *q = put_back(&step, q).expect("a valid step");
        true
    }

    #[test]
    fn an_edit_that_changes_nothing_is_not_a_step() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        assert!(!act(&mut h, &mut q, |q| q.set_repeat(crate::queue::Repeat::Off)));
        assert!(!act(&mut h, &mut q, |q| {
            q.remove(QueueEntryId::from_raw(999));
        }));
        assert!(!h.can_undo());
    }

    #[test]
    fn undo_and_redo_walk_the_queue_back_and_forth() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 0);
        act(&mut h, &mut q, |q| {
            q.add_to_queue(tracks(&["m"]));
        });
        act(&mut h, &mut q, |q| q.clear_upcoming());
        assert_eq!(vids(q.upcoming()), Vec::<String>::new());

        assert!(undo(&mut h, &mut q));
        assert_eq!(vids(q.upcoming()), ["m", "b", "c"]);
        assert!(undo(&mut h, &mut q));
        assert_eq!(vids(q.upcoming()), ["b", "c"]);
        assert!(!undo(&mut h, &mut q), "nothing left to undo");

        assert!(redo(&mut h, &mut q));
        assert_eq!(vids(q.upcoming()), ["m", "b", "c"]);
        assert!(redo(&mut h, &mut q));
        assert_eq!(vids(q.upcoming()), Vec::<String>::new());
        assert!(!redo(&mut h, &mut q));
    }

    #[test]
    fn a_new_edit_ends_the_redo_line() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        act(&mut h, &mut q, |q| q.clear_upcoming());
        undo(&mut h, &mut q);
        assert!(h.can_redo());
        act(&mut h, &mut q, |q| {
            q.add_to_queue(tracks(&["z"]));
        });
        assert!(!h.can_redo());
    }

    #[test]
    fn the_history_keeps_at_least_twenty_steps_and_drops_the_oldest_past_its_limit() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a"]), 0);
        for i in 0..(DEPTH + 10) {
            act(&mut h, &mut q, |q| {
                q.add_to_queue(vec![t(&format!("m{i}"))]);
            });
        }
        assert_eq!(h.undo_len(), DEPTH);
        assert!(DEPTH >= 20);
        for _ in 0..DEPTH {
            assert!(undo(&mut h, &mut q));
        }
        assert!(!undo(&mut h, &mut q));
        // The ten oldest adds could not be taken back.
        assert_eq!(q.upcoming().count(), 10);
    }

    #[test]
    fn undoing_a_removal_while_something_else_plays_leaves_the_playing_track_alone() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c", "d"]), 0);
        let c = q.upcoming_ids()[1];
        act(&mut h, &mut q, |q| {
            q.remove(c);
        });
        // The listener moves on to b, then undoes the removal of c.
        q.advance(crate::queue::AdvanceKind::Explicit);
        assert_eq!(cur(&q), "b");
        assert!(undo(&mut h, &mut q));
        assert_eq!(cur(&q), "b");
        assert_eq!(vids(q.history().iter()), ["a"]);
        assert_eq!(vids(q.upcoming()), ["c", "d"]);
    }

    #[test]
    fn undoing_a_replace_brings_back_the_old_track_and_its_queue() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 1);
        act(&mut h, &mut q, |q| {
            q.play_context(tracks(&["x", "y"]), 0);
        });
        assert_eq!(cur(&q), "x");
        assert!(undo(&mut h, &mut q));
        assert_eq!(cur(&q), "b");
        assert_eq!(vids(q.history().iter()), ["a"]);
        assert_eq!(vids(q.upcoming()), ["c"]);
        // Redo goes forward again, to the new list's first track.
        assert!(redo(&mut h, &mut q));
        assert_eq!(cur(&q), "x");
        assert_eq!(vids(q.upcoming()), ["y"]);
    }

    #[test]
    fn undoing_a_replace_after_moving_on_in_the_new_list_still_restores_the_old_track() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        act(&mut h, &mut q, |q| {
            q.play_context(tracks(&["x", "y", "z"]), 0);
        });
        q.advance(crate::queue::AdvanceKind::Explicit);
        assert_eq!(cur(&q), "y");
        assert!(undo(&mut h, &mut q));
        assert_eq!(cur(&q), "a");
        assert_eq!(vids(q.upcoming()), ["b"]);
    }

    #[test]
    fn undoing_a_play_next_that_is_now_playing_lets_it_finish() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        act(&mut h, &mut q, |q| {
            q.play_next(tracks(&["n"]));
        });
        q.advance(crate::queue::AdvanceKind::Explicit);
        assert_eq!(cur(&q), "n");
        assert!(undo(&mut h, &mut q));
        // n is no longer in the queue, but it was not stopped.
        assert_eq!(cur(&q), "n");
        assert_eq!(vids(q.history().iter()), ["a"]);
        assert_eq!(vids(q.upcoming()), ["b"]);
        // Once it ends the queue carries on, and it does not come back around.
        q.advance(crate::queue::AdvanceKind::Automatic);
        assert_eq!(cur(&q), "b");
        assert_eq!(vids(q.history().iter()), ["a"]);
    }

    #[test]
    fn undoing_a_shuffle_toggle_restores_the_order() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c", "d", "e", "f"]), 0);
        act(&mut h, &mut q, |q| q.set_shuffle(true, 7));
        assert!(q.shuffle());
        assert!(undo(&mut h, &mut q));
        assert!(!q.shuffle());
        assert_eq!(vids(q.upcoming()), ["b", "c", "d", "e", "f"]);
        assert!(redo(&mut h, &mut q));
        assert!(q.shuffle());
    }

    #[test]
    fn a_put_back_queue_never_reuses_an_id_a_live_entry_has() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        act(&mut h, &mut q, |q| q.clear_upcoming());
        let added = q.add_to_queue(tracks(&["m"]))[0];
        undo(&mut h, &mut q);
        let minted = q.add_to_queue(tracks(&["fresh"]))[0];
        assert!(minted.get() > added.get());
    }

    #[test]
    fn the_depth_follows_records_undos_and_redos_past_the_limit() {
        let mut h = UndoHistory::with_limit(2);
        let mut q = Queue::new();
        q.play_context(tracks(&["a"]), 0);
        assert_eq!(h.depth(), 0);
        for i in 0..4 {
            act(&mut h, &mut q, |q| {
                q.add_to_queue(vec![t(&format!("m{i}"))]);
            });
        }
        assert_eq!((h.depth(), h.undo_len()), (4, 2), "the limit drops steps, not depth");
        undo(&mut h, &mut q);
        assert_eq!(h.depth(), 3);
        redo(&mut h, &mut q);
        assert_eq!(h.depth(), 4);
        h.clear();
        assert_eq!(h.depth(), 0);
    }

    #[test]
    fn the_newest_step_is_found_across_both_histories() {
        // Nothing noted by a playlist step: the queue's, if it has one.
        assert_eq!(newest(0, false, None), Newest::Nothing);
        assert_eq!(newest(2, true, None), Newest::Queue);
        // A playlist edit made at queue depth 2: queue steps since then go first.
        assert_eq!(newest(3, true, Some(2)), Newest::Queue);
        assert_eq!(newest(2, true, Some(2)), Newest::Playlist);
        // Queue steps undone back below the note: the playlist edit is still the newest left.
        assert_eq!(newest(1, true, Some(2)), Newest::Playlist);
        // A queue that cannot undo (empty, or a skim) leaves the playlist step.
        assert_eq!(newest(3, false, Some(2)), Newest::Playlist);
    }

    #[test]
    fn stamps_only_grow() {
        let (a, b, c) = (next_seq(), next_seq(), next_seq());
        assert!(a < b && b < c);
    }

    #[test]
    fn clearing_forgets_both_directions() {
        let mut h = UndoHistory::new();
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        act(&mut h, &mut q, |q| q.clear_upcoming());
        undo(&mut h, &mut q);
        h.clear();
        assert!(!h.can_undo() && !h.can_redo());
    }
}
