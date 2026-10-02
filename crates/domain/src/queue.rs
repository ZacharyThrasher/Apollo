//! Queue semantics (BUILD_SPEC QUEUE-01).
//!
//! Segments, in playback order: `manual` -> `context` -> `radio`.
//! Every occurrence has its own [`QueueEntryId`], even for the same video.

use crate::model::{QueueEntryId, Track};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Came from the source context (album, playlist, search result list).
    Context,
    /// Added by the user via Play next / Add to queue / explicit drag.
    Manual,
    /// Generated continuation.
    Radio,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub track: Track,
    pub origin: Origin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Off,
    One,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdvanceKind {
    /// End of media reached.
    Automatic,
    /// User pressed Next. Overrides repeat-one.
    Explicit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// A different entry is now current.
    Started(QueueEntryId),
    /// Repeat-one: the same entry plays again (automatic EOF only).
    RepeatedCurrent(QueueEntryId),
    /// Nothing to advance to; queue state is unchanged.
    EndOfQueue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Previous {
    SeekToStart,
    Started(QueueEntryId),
}

/// Previous seeks to the start when past this position (QUEUE-01).
pub const PREVIOUS_RESTART_THRESHOLD_MS: u64 = 3_000;

struct Current {
    entry: QueueEntry,
    /// Row removed by the user: keeps playing but is excluded from future cycles.
    removed: bool,
}

pub struct Queue {
    next_id: u64,
    history: Vec<QueueEntry>,
    current: Option<Current>,
    manual: Vec<QueueEntry>,
    context: Vec<QueueEntry>,
    radio: Vec<QueueEntry>,
    /// Canonical (unshuffled) order of context entries, including played ones.
    /// Only `Origin::Context` entries appear here.
    canonical: Vec<QueueEntryId>,
    shuffle: bool,
    seed: u64,
    repeat: Repeat,
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

impl Queue {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            history: Vec::new(),
            current: None,
            manual: Vec::new(),
            context: Vec::new(),
            radio: Vec::new(),
            canonical: Vec::new(),
            shuffle: false,
            seed: 0,
            repeat: Repeat::Off,
        }
    }

    fn mint(&mut self, track: Track, origin: Origin) -> QueueEntry {
        let id = QueueEntryId(self.next_id);
        self.next_id += 1;
        QueueEntry { id, track, origin }
    }

    // ---- read access -------------------------------------------------

    pub fn current(&self) -> Option<&QueueEntry> {
        self.current.as_ref().map(|c| &c.entry)
    }
    pub fn history(&self) -> &[QueueEntry] {
        &self.history
    }
    pub fn manual(&self) -> &[QueueEntry] {
        &self.manual
    }
    pub fn context(&self) -> &[QueueEntry] {
        &self.context
    }
    pub fn radio(&self) -> &[QueueEntry] {
        &self.radio
    }
    pub fn shuffle(&self) -> bool {
        self.shuffle
    }
    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    /// Upcoming entries in the order they will play.
    pub fn upcoming(&self) -> impl Iterator<Item = &QueueEntry> {
        self.manual.iter().chain(&self.context).chain(&self.radio)
    }

    pub fn upcoming_ids(&self) -> Vec<QueueEntryId> {
        self.upcoming().map(|e| e.id).collect()
    }

    pub fn contains(&self, id: QueueEntryId) -> bool {
        self.current().is_some_and(|c| c.id == id)
            || self.history.iter().any(|e| e.id == id)
            || self.upcoming().any(|e| e.id == id)
    }

    // ---- building ----------------------------------------------------

    /// Replace the whole queue with `tracks`, starting at `start`.
    /// Caller is responsible for any "replace queue?" confirmation.
    /// Returns the new current entry, or `None` if `tracks` is empty.
    pub fn play_context(&mut self, tracks: Vec<Track>, start: usize) -> Option<QueueEntryId> {
        self.history.clear();
        self.current = None;
        self.manual.clear();
        self.context.clear();
        self.radio.clear();
        self.canonical.clear();
        if tracks.is_empty() {
            return None;
        }
        let start = start.min(tracks.len() - 1);
        let mut entries: Vec<QueueEntry> = tracks
            .into_iter()
            .map(|t| self.mint(t, Origin::Context))
            .collect();
        self.canonical = entries.iter().map(|e| e.id).collect();
        let rest = entries.split_off(start + 1);
        let current = entries.pop().expect("start entry exists");
        if self.shuffle {
            // Shuffled playback has no meaningful "before start"; shuffle everything else.
            let mut upcoming = entries;
            upcoming.extend(rest);
            self.context = upcoming;
            self.reshuffle_context();
        } else {
            self.history = entries;
            self.context = rest;
        }
        let id = current.id;
        self.current = Some(Current {
            entry: current,
            removed: false,
        });
        Some(id)
    }

    /// Insert `tracks` immediately after the current entry. A later batch
    /// precedes an earlier one; order within a batch is preserved.
    pub fn play_next(&mut self, tracks: Vec<Track>) -> Vec<QueueEntryId> {
        let entries: Vec<QueueEntry> = tracks
            .into_iter()
            .map(|t| self.mint(t, Origin::Manual))
            .collect();
        let ids = entries.iter().map(|e| e.id).collect();
        self.manual.splice(0..0, entries);
        ids
    }

    /// Append to the manual segment, ahead of untouched context/radio items.
    pub fn add_to_queue(&mut self, tracks: Vec<Track>) -> Vec<QueueEntryId> {
        let entries: Vec<QueueEntry> = tracks
            .into_iter()
            .map(|t| self.mint(t, Origin::Manual))
            .collect();
        let ids = entries.iter().map(|e| e.id).collect();
        self.manual.extend(entries);
        ids
    }

    /// Append generated continuation entries. They never precede manual/context items.
    pub fn append_radio(&mut self, tracks: Vec<Track>) -> Vec<QueueEntryId> {
        let entries: Vec<QueueEntry> = tracks
            .into_iter()
            .map(|t| self.mint(t, Origin::Radio))
            .collect();
        let ids = entries.iter().map(|e| e.id).collect();
        self.radio.extend(entries);
        ids
    }

    // ---- editing -----------------------------------------------------

    /// Remove one occurrence. Removing the current entry does not interrupt
    /// playback; it only excludes the entry from future repeat-all cycles.
    pub fn remove(&mut self, id: QueueEntryId) -> bool {
        if let Some(c) = &mut self.current {
            if c.entry.id == id {
                c.removed = true;
                return true;
            }
        }
        for seg in [
            &mut self.history,
            &mut self.manual,
            &mut self.context,
            &mut self.radio,
        ] {
            if let Some(pos) = seg.iter().position(|e| e.id == id) {
                seg.remove(pos);
                self.canonical.retain(|c| *c != id);
                return true;
            }
        }
        false
    }

    /// Move an upcoming entry to `index` within the combined upcoming view
    /// (manual ++ context ++ radio). Explicit placement is authoritative:
    /// an entry dropped into the manual segment leaves the shuffle pool.
    /// The current entry and history entries cannot be moved.
    pub fn move_upcoming(&mut self, id: QueueEntryId, index: usize) -> bool {
        let Some(entry) = self.take_upcoming(id) else {
            return false;
        };
        let manual_len = self.manual.len();
        let index = index.min(manual_len + self.context.len() + self.radio.len());
        let mut entry = entry;
        if index <= manual_len {
            entry.origin = Origin::Manual;
            self.canonical.retain(|c| *c != id);
            self.manual.insert(index, entry);
        } else if index <= manual_len + self.context.len() {
            entry.origin = Origin::Context;
            self.context.insert(index - manual_len, entry);
            self.sync_canonical_after_move(id);
        } else {
            // Past the context: keep it in the context tail rather than radio,
            // so a user-placed item is never treated as generated.
            entry.origin = Origin::Context;
            self.context.push(entry);
            self.sync_canonical_after_move(id);
        }
        true
    }

    fn take_upcoming(&mut self, id: QueueEntryId) -> Option<QueueEntry> {
        for seg in [&mut self.manual, &mut self.context, &mut self.radio] {
            if let Some(pos) = seg.iter().position(|e| e.id == id) {
                return Some(seg.remove(pos));
            }
        }
        None
    }

    fn sync_canonical_after_move(&mut self, id: QueueEntryId) {
        if self.shuffle {
            // Canonical order is the unshuffled original; make sure the id is
            // known to it (e.g. it came from manual/radio) without reordering others.
            if !self.canonical.contains(&id) {
                self.canonical.push(id);
            }
        } else {
            // Unshuffled: the visible order *is* the canonical order.
            let upcoming: Vec<QueueEntryId> = self.context.iter().map(|e| e.id).collect();
            self.canonical.retain(|c| !upcoming.contains(c));
            self.canonical.extend(upcoming);
        }
    }

    // ---- shuffle / repeat --------------------------------------------

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    /// Shuffle reorders only the unplayed context segment. Turning it off
    /// restores canonical order for whatever is still upcoming.
    pub fn set_shuffle(&mut self, enabled: bool, seed: u64) {
        if enabled == self.shuffle {
            return;
        }
        self.shuffle = enabled;
        self.seed = seed;
        if enabled {
            self.reshuffle_context();
        } else {
            let mut pool: Vec<QueueEntry> = std::mem::take(&mut self.context);
            let mut ordered = Vec::with_capacity(pool.len());
            for cid in &self.canonical {
                if let Some(pos) = pool.iter().position(|e| e.id == *cid) {
                    ordered.push(pool.remove(pos));
                }
            }
            ordered.extend(pool); // anything not in canonical keeps relative order at the tail
            self.context = ordered;
        }
    }

    fn reshuffle_context(&mut self) {
        let mut rng = SplitMix64(self.seed);
        let n = self.context.len();
        for i in (1..n).rev() {
            let j = (rng.next() % (i as u64 + 1)) as usize;
            self.context.swap(i, j);
        }
    }

    // ---- advancing ---------------------------------------------------

    pub fn advance(&mut self, kind: AdvanceKind) -> Advance {
        if kind == AdvanceKind::Automatic && self.repeat == Repeat::One {
            if let Some(c) = self.current() {
                return Advance::RepeatedCurrent(c.id);
            }
        }
        // Repeat-all cycles explicit context + manual additions, never radio.
        let next = if !self.manual.is_empty() {
            Some(self.manual.remove(0))
        } else if !self.context.is_empty() {
            Some(self.context.remove(0))
        } else if self.repeat != Repeat::All && !self.radio.is_empty() {
            Some(self.radio.remove(0))
        } else {
            None
        };
        match next {
            Some(entry) => Advance::Started(self.start(entry)),
            None if self.repeat == Repeat::All => self.wrap(),
            None => Advance::EndOfQueue,
        }
    }

    fn start(&mut self, entry: QueueEntry) -> QueueEntryId {
        if let Some(prev) = self.current.take() {
            if !prev.removed {
                self.history.push(prev.entry);
            }
        }
        let id = entry.id;
        self.current = Some(Current {
            entry,
            removed: false,
        });
        id
    }

    /// Repeat-all wrap: replay history + current (minus removed rows, minus
    /// radio) as a fresh cycle with new occurrence IDs.
    fn wrap(&mut self) -> Advance {
        let mut cycle: Vec<(Track, Origin)> = self
            .history
            .iter()
            .filter(|e| e.origin != Origin::Radio)
            .map(|e| (e.track.clone(), e.origin))
            .collect();
        if let Some(c) = &self.current {
            if !c.removed && c.entry.origin != Origin::Radio {
                cycle.push((c.entry.track.clone(), c.entry.origin));
            }
        }
        if cycle.is_empty() {
            return Advance::EndOfQueue;
        }
        self.history.clear();
        self.current = None;
        self.radio.clear();
        self.canonical.clear();
        let mut entries: Vec<QueueEntry> =
            cycle.into_iter().map(|(t, o)| self.mint(t, o)).collect();
        self.canonical = entries
            .iter()
            .filter(|e| e.origin == Origin::Context)
            .map(|e| e.id)
            .collect();
        let first = entries.remove(0);
        self.context = entries;
        self.manual.clear();
        if self.shuffle {
            self.seed ^= self.next_id; // vary order between cycles, stay deterministic
            self.reshuffle_context();
        }
        Advance::Started(self.start(first))
    }

    /// `position_ms` is the decoder-confirmed position of the current entry.
    pub fn previous(&mut self, position_ms: u64) -> Previous {
        if position_ms > PREVIOUS_RESTART_THRESHOLD_MS {
            return Previous::SeekToStart;
        }
        let Some(prev) = self.history.pop() else {
            return Previous::SeekToStart;
        };
        if let Some(cur) = self.current.take() {
            if !cur.removed {
                // Put the current entry back at the head of its own segment.
                match cur.entry.origin {
                    Origin::Manual => self.manual.insert(0, cur.entry),
                    Origin::Context => self.context.insert(0, cur.entry),
                    Origin::Radio => self.radio.insert(0, cur.entry),
                }
            }
        }
        let id = prev.id;
        self.current = Some(Current {
            entry: prev,
            removed: false,
        });
        Previous::Started(id)
    }

    /// Remove and return the next upcoming entry without making it current
    /// (used for next-track lookahead decisions); does not mutate.
    pub fn peek_next(&self) -> Option<&QueueEntry> {
        self.upcoming().next()
    }
}

/// Small deterministic PRNG so shuffle order is reproducible from a stored seed.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str) -> Track {
        Track::new(id, id)
    }
    fn tracks(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|i| t(i)).collect()
    }
    fn vids<'a>(it: impl Iterator<Item = &'a QueueEntry>) -> Vec<String> {
        it.map(|e| e.track.video_id.as_str().to_owned()).collect()
    }
    fn cur(q: &Queue) -> String {
        q.current().unwrap().track.video_id.as_str().to_owned()
    }

    // TEST-07
    #[test]
    fn same_video_three_times_gives_distinct_occurrences() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "x", "x", "x", "b"]), 0);
        let xs: Vec<_> = q
            .upcoming()
            .filter(|e| e.track.video_id.as_str() == "x")
            .map(|e| e.id)
            .collect();
        assert_eq!(xs.len(), 3);
        assert!(xs[0] != xs[1] && xs[1] != xs[2] && xs[0] != xs[2]);

        // Removing the middle occurrence leaves the other two.
        assert!(q.remove(xs[1]));
        assert_eq!(vids(q.upcoming()), ["x", "x", "b"]);
        assert!(q.contains(xs[0]) && q.contains(xs[2]) && !q.contains(xs[1]));

        // Reordering one occurrence moves exactly that one.
        assert!(q.move_upcoming(xs[2], 0));
        assert_eq!(q.upcoming_ids()[0], xs[2]);
        assert_eq!(q.upcoming_ids()[1], xs[0]);
    }

    #[test]
    fn ids_are_never_reused_across_replace() {
        let mut q = Queue::new();
        let a = q.play_context(tracks(&["a", "b"]), 0).unwrap();
        let b = q.play_context(tracks(&["a", "b"]), 0).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn play_context_start_middle_fills_history() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c", "d"]), 2);
        assert_eq!(cur(&q), "c");
        assert_eq!(vids(q.history().iter()), ["a", "b"]);
        assert_eq!(vids(q.upcoming()), ["d"]);
    }

    #[test]
    fn play_next_batches_stack_and_keep_inner_order() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        q.play_next(tracks(&["p1", "p2"]));
        q.play_next(tracks(&["q1", "q2"]));
        assert_eq!(vids(q.upcoming()), ["q1", "q2", "p1", "p2", "b"]);
    }

    #[test]
    fn add_to_queue_goes_after_manual_before_context() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 0);
        q.play_next(tracks(&["n"]));
        q.add_to_queue(tracks(&["m1"]));
        q.add_to_queue(tracks(&["m2"]));
        q.append_radio(tracks(&["r"]));
        assert_eq!(vids(q.upcoming()), ["n", "m1", "m2", "b", "c", "r"]);
    }

    // TEST-08
    #[test]
    fn shuffle_keeps_current_history_and_manual_and_is_reversible() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c", "d", "e", "f", "g", "h"]), 2);
        q.add_to_queue(tracks(&["m1", "m2"]));
        let history_before = vids(q.history().iter());
        let canonical_upcoming = vids(q.context().iter());

        q.set_shuffle(true, 42);
        assert_eq!(cur(&q), "c");
        assert_eq!(vids(q.history().iter()), history_before);
        assert_eq!(vids(q.manual().iter()), ["m1", "m2"]);
        // Same set, (almost certainly) different order, deterministic for the seed.
        let mut shuffled = vids(q.context().iter());
        let first = shuffled.clone();
        shuffled.sort();
        let mut want = canonical_upcoming.clone();
        want.sort();
        assert_eq!(shuffled, want);
        let mut q2 = Queue::new();
        q2.play_context(tracks(&["a", "b", "c", "d", "e", "f", "g", "h"]), 2);
        q2.add_to_queue(tracks(&["m1", "m2"]));
        q2.set_shuffle(true, 42);
        assert_eq!(vids(q2.context().iter()), first);

        // Manual priority preserved while advancing.
        assert!(matches!(
            q.advance(AdvanceKind::Explicit),
            Advance::Started(_)
        ));
        assert_eq!(cur(&q), "m1");

        q.set_shuffle(false, 0);
        assert_eq!(vids(q.context().iter()), canonical_upcoming);
    }

    #[test]
    fn explicit_move_into_manual_leaves_shuffle_pool() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c", "d", "e"]), 0);
        q.set_shuffle(true, 7);
        let target = q
            .context()
            .iter()
            .find(|e| e.track.video_id.as_str() == "e")
            .unwrap()
            .id;
        assert!(q.move_upcoming(target, 0));
        assert_eq!(q.manual()[0].id, target);
        q.set_shuffle(false, 0);
        assert_eq!(vids(q.context().iter()), ["b", "c", "d"]);
        assert_eq!(vids(q.manual().iter()), ["e"]);
    }

    // TEST-09
    #[test]
    fn repeat_one_repeats_on_eof_but_explicit_next_advances_once() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 0);
        q.set_repeat(Repeat::One);
        let a = q.current().unwrap().id;
        assert_eq!(
            q.advance(AdvanceKind::Automatic),
            Advance::RepeatedCurrent(a)
        );
        assert_eq!(cur(&q), "a");
        assert!(matches!(
            q.advance(AdvanceKind::Explicit),
            Advance::Started(_)
        ));
        assert_eq!(cur(&q), "b");
        assert_eq!(vids(q.history().iter()), ["a"]);
    }

    #[test]
    fn end_of_queue_without_repeat_leaves_state_untouched() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b"]), 0);
        assert!(matches!(
            q.advance(AdvanceKind::Automatic),
            Advance::Started(_)
        ));
        assert_eq!(q.advance(AdvanceKind::Automatic), Advance::EndOfQueue);
        assert_eq!(cur(&q), "b");
        assert_eq!(vids(q.history().iter()), ["a"]);
    }

    #[test]
    fn repeat_all_wraps_with_fresh_ids_and_skips_radio_and_removed() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 0);
        q.append_radio(tracks(&["r"]));
        q.set_repeat(Repeat::All);
        let old_a = q.current().unwrap().id;
        q.advance(AdvanceKind::Automatic); // b
        q.advance(AdvanceKind::Automatic); // c; radio must NOT be entered under repeat-all
        assert_eq!(cur(&q), "c");
        let c_id = q.current().unwrap().id;
        q.remove(c_id); // removed while playing: no interruption, excluded from next cycle
        assert_eq!(cur(&q), "c");
        let Advance::Started(new_id) = q.advance(AdvanceKind::Automatic) else {
            panic!("repeat-all must wrap")
        };
        assert_ne!(new_id, old_a);
        assert_eq!(cur(&q), "a");
        assert_eq!(vids(q.upcoming()), ["b"]);
        assert!(q.radio().is_empty());
    }

    #[test]
    fn radio_plays_after_context_when_not_repeat_all() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a"]), 0);
        q.append_radio(tracks(&["r1"]));
        q.advance(AdvanceKind::Automatic);
        assert_eq!(cur(&q), "r1");
    }

    #[test]
    fn previous_semantics() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 0);
        // At the beginning of history it seeks to zero.
        assert_eq!(q.previous(500), Previous::SeekToStart);
        q.advance(AdvanceKind::Explicit); // b
                                          // Past 3 s: restart, do not leave the track.
        assert_eq!(q.previous(3_001), Previous::SeekToStart);
        assert_eq!(cur(&q), "b");
        // Within 3 s: go to the previous entry; current returns to the front of upcoming.
        assert!(matches!(q.previous(2_000), Previous::Started(_)));
        assert_eq!(cur(&q), "a");
        assert_eq!(vids(q.upcoming()), ["b", "c"]);
    }

    #[test]
    fn removing_current_does_not_stop_it_and_previous_drops_it() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 0);
        q.advance(AdvanceKind::Explicit);
        let b = q.current().unwrap().id;
        assert!(q.remove(b));
        assert_eq!(cur(&q), "b");
        assert!(matches!(q.previous(0), Previous::Started(_)));
        assert_eq!(cur(&q), "a");
        assert_eq!(vids(q.upcoming()), ["c"]);
    }

    #[test]
    fn current_and_history_cannot_be_moved() {
        let mut q = Queue::new();
        q.play_context(tracks(&["a", "b", "c"]), 1);
        let cur_id = q.current().unwrap().id;
        let hist_id = q.history()[0].id;
        assert!(!q.move_upcoming(cur_id, 0));
        assert!(!q.move_upcoming(hist_id, 0));
    }

    #[test]
    fn empty_context_is_none_and_clamps_start() {
        let mut q = Queue::new();
        assert_eq!(q.play_context(vec![], 0), None);
        assert!(q.current().is_none());
        q.play_context(tracks(&["a", "b"]), 99);
        assert_eq!(cur(&q), "b");
    }
}
