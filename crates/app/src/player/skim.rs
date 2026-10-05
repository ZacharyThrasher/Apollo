//! Skim: audition a list. Each song plays from about a third of the way in for a few seconds,
//! then the next one starts. K sets a song aside (the keys are the shell's), Enter keeps the one
//! playing and goes on normally, Esc puts back what was playing before.
//!
//! Skim is a layer over the one playback authority, not a second player: the list becomes the
//! queue, the coordinator opens each song at its start point ([`Event::NextAt`]) and a ticker
//! moves on when the playhead has covered the window. Skimmed songs never reach YouTube's
//! history or the local stats (the report and tally feeds see nothing loaded), and the queue is
//! not saved while it lasts, so a restart brings back what was there before. It never starts
//! by itself.

use std::collections::HashSet;
use std::time::Duration;

use apollo_domain::model::{QueueEntryId, Track, VideoId};
use apollo_domain::playback::{Event, Intent, Phase};
use apollo_domain::queue::{Queue, QueueSnapshot, QueueSource, Repeat};
use gpui::Context;

use super::PlayerModel;

/// How often the ticker looks at the playhead.
const TICK: Duration = Duration::from_millis(100);

/// A song this close to its end counts as finished even when the window is not covered yet.
const END_SLACK_MS: u64 = 400;

/// Seconds per song when nothing was chosen.
pub const DEFAULT_SECONDS: u32 = 20;

/// The lengths Settings offers.
pub const SECONDS_CHOICES: [u32; 3] = [10, 20, 30];

/// Where a song is first played from: a third of the way in, pulled back so the whole window
/// fits when the song is short, and the start when its length is not known.
pub fn start_ms(duration_ms: Option<u64>, window_ms: u64) -> u64 {
    match duration_ms {
        Some(d) => (d / 3).min(d.saturating_sub(window_ms)),
        None => 0,
    }
}

/// What the views draw while skimming. Positions are the song's own: the window runs from
/// `start_ms` for `window_ms`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SkimView {
    /// 1-based place in the list.
    pub index: usize,
    pub total: usize,
    pub start_ms: u64,
    pub window_ms: u64,
}

impl SkimView {
    /// The playhead inside the window, and the window's length: what the progress bar shows.
    pub fn in_window(&self, position_ms: u64) -> (u64, u64) {
        (
            position_ms.saturating_sub(self.start_ms).min(self.window_ms),
            self.window_ms,
        )
    }

    /// A point of the window (0 to `window_ms`) as a position in the song.
    pub fn to_song(&self, in_window_ms: u64) -> u64 {
        self.start_ms + in_window_ms.min(self.window_ms)
    }

    /// The pill's text.
    pub fn label(&self) -> String {
        format!("Skimming {} of {}", self.index, self.total)
    }
}

pub(super) struct Skim {
    /// How long each song plays, in milliseconds.
    window_ms: u64,
    total: usize,
    /// The list being skimmed, in order: a song picked from the same list moves the skim there
    /// instead of ending it.
    list: Vec<VideoId>,
    /// Every entry the skim made. A current entry outside the set means the user started
    /// something else and Skim steps aside.
    entries: HashSet<QueueEntryId>,
    /// The entry whose window is being played, and where it started.
    entry: Option<QueueEntryId>,
    start_ms: u64,
    /// The queue and position from before, for Esc (and for undo once Enter keeps the queue).
    before: Option<(QueueSnapshot, u64)>,
    /// Music was playing when Skim began: Esc starts it again.
    resume: bool,
}

impl PlayerModel {
    /// Whether the Skim buttons show (Settings > Playback > Skim).
    pub fn skim_enabled(&self) -> bool {
        self.skim_enabled
    }

    pub fn set_skim_enabled(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.skim_enabled == on {
            return;
        }
        self.skim_enabled = on;
        if !on {
            // Switching it off while skimming ends it where it is, as Enter would not: back to
            // what was playing.
            self.skim_cancel(cx);
        }
        cx.notify();
    }

    /// Settings > Playback > Seconds per song. Applies from the next song.
    pub fn set_skim_seconds(&mut self, seconds: u32) {
        self.skim_seconds = seconds.clamp(5, 120);
    }

    pub fn skimming(&self) -> bool {
        self.skim.is_some()
    }

    /// Whether the skim running is of the list that named itself `name` when it started (the
    /// queue's source), so a list can give the keys its own meaning while it is the one skimmed.
    pub fn skimming_from(&self, name: &str) -> bool {
        self.skim.is_some()
            && self
                .driver
                .as_ref()
                .and_then(|d| d.coordinator().queue().source().map(|s| s.name == name))
                .unwrap_or(false)
    }

    /// The song being skimmed is dealt with (kept or tossed): on to the next one at once, or
    /// back to what was playing when it was the last.
    pub fn skim_pass(&mut self, cx: &mut Context<Self>) {
        if self.skim.is_some() {
            self.skim_next(cx);
        }
    }

    /// A song left the list being skimmed (the Crate's Delete): a song picked from that list
    /// later still counts as the same list. The skim's length and place are not changed.
    pub fn skim_forget(&mut self, video: &str) {
        if let Some(skim) = self.skim.as_mut() {
            skim.list.retain(|id| id.as_str() != video);
        }
    }

    /// Where Skim stands, for the views; none while not skimming.
    pub fn skim_view(&self) -> Option<SkimView> {
        let skim = self.skim.as_ref()?;
        let queue = self.driver.as_ref()?.coordinator().queue();
        Some(SkimView {
            index: queue.history().len() + 1,
            total: skim.total,
            start_ms: skim.start_ms,
            window_ms: self.skim_window_ms(skim),
        })
    }

    /// The window as played: shortened when the song ends before it would.
    fn skim_window_ms(&self, skim: &Skim) -> u64 {
        let duration = self.driver.as_ref().and_then(|d| {
            let c = d.coordinator();
            c.duration_ms()
                .or_else(|| c.queue().current()?.track.duration.map(|t| t.as_millis() as u64))
        });
        match duration {
            Some(d) => skim.window_ms.min(d.saturating_sub(skim.start_ms)).max(1),
            None => skim.window_ms,
        }
    }

    /// Start skimming `tracks`, the list a page shows, from the first. What was playing is kept
    /// for Esc. The list is played in order, with shuffle and repeat off for as long as it lasts.
    pub fn start_skim(
        &mut self,
        tracks: Vec<Track>,
        source: Option<QueueSource>,
        cx: &mut Context<Self>,
    ) {
        self.start_skim_at(tracks, 0, source, cx);
    }

    /// [`PlayerModel::start_skim`] from song `start` of the list (K on a row: skim from here).
    pub fn start_skim_at(
        &mut self,
        tracks: Vec<Track>,
        start: usize,
        source: Option<QueueSource>,
        cx: &mut Context<Self>,
    ) {
        if !self.skim_enabled || tracks.is_empty() || self.driver.is_none() {
            return;
        }
        let start = start.min(tracks.len() - 1);
        // Skimming again from inside a skim keeps the original "before".
        let (before, resume) = match self.skim.take() {
            Some(old) => (old.before, old.resume),
            None => (
                self.undo_capture(),
                self.driver
                    .as_ref()
                    .is_some_and(|d| d.coordinator().intent() == Intent::Playing),
            ),
        };
        let window_ms = u64::from(self.skim_seconds) * 1_000;
        let first = start_ms(
            tracks[start].duration.map(|d| d.as_millis() as u64),
            window_ms,
        );
        let total = tracks.len();
        let list: Vec<VideoId> = tracks.iter().map(|t| t.video_id.clone()).collect();
        let seed = self.next_seed();
        if let Some(d) = self.driver.as_mut() {
            d.edit_queue(|q| {
                q.set_shuffle(false, seed);
                q.set_repeat(Repeat::Off);
            });
        }
        // Set before the first song starts, so nothing of it is reported.
        self.skim = Some(Skim {
            window_ms,
            total,
            list,
            entries: HashSet::new(),
            entry: None,
            start_ms: first,
            before,
            resume,
        });
        self.handle(
            Event::PlayContextAt {
                tracks,
                start,
                position_ms: first,
            },
            cx,
        );
        self.edit_queue_untracked(cx, |q| q.set_source(source));
        let (entries, current) = match self.driver.as_ref() {
            Some(d) => {
                let q = d.coordinator().queue();
                let snapshot = q.snapshot();
                let ids: HashSet<QueueEntryId> = snapshot
                    .history
                    .iter()
                    .chain(&snapshot.context)
                    .map(|e| e.id)
                    .chain(snapshot.current.iter().map(|(e, _)| e.id))
                    .collect();
                (ids, q.current().map(|e| e.id))
            }
            None => (HashSet::new(), None),
        };
        if let Some(skim) = self.skim.as_mut() {
            skim.entries = entries;
            skim.entry = current;
        }
        self.prefetch_upcoming();
        self.ensure_skim_ticker(cx);
        cx.notify();
    }

    /// A song picked in a list while skimming: when it is the list being skimmed, the skim moves
    /// to that song and goes on from there (what was playing before is still what Esc brings
    /// back). Whether it was taken; a song of any other list is not, and plays normally.
    pub(super) fn skim_pick(
        &mut self,
        tracks: &[Track],
        start: usize,
        source: Option<&QueueSource>,
        cx: &mut Context<Self>,
    ) -> bool {
        let same = self.skim.as_ref().is_some_and(|skim| {
            skim.list.len() == tracks.len()
                && skim.list.iter().zip(tracks).all(|(id, t)| *id == t.video_id)
        });
        if !same || start >= tracks.len() {
            return false;
        }
        self.start_skim_at(tracks.to_vec(), start, source.cloned(), cx);
        true
    }

    /// Esc: stop skimming and put back what was playing before (nothing, when nothing was).
    pub fn skim_cancel(&mut self, cx: &mut Context<Self>) {
        let Some(skim) = self.skim.take() else {
            return;
        };
        self.restore_before(skim.before, skim.resume, cx);
    }

    /// Enter: stop skimming and play this song normally, from its start. The queue stays as the
    /// list, so the rest plays on after it; Ctrl+Z goes back to what was queued before.
    pub fn skim_keep(&mut self, cx: &mut Context<Self>) {
        let Some(skim) = self.skim.take() else {
            return;
        };
        self.undo_commit(skim.before);
        self.handle(Event::Seek { position_ms: 0 }, cx);
        self.handle(Event::Play, cx);
        self.prefetch_upcoming();
        cx.notify();
    }

    /// Enter on a list row while skimming: that song plays normally, from its start, so the
    /// skim is over (what was playing before is not coming back).
    pub fn skim_leave(&mut self) {
        self.skim_yield();
    }

    /// Something else took over the queue (another list was played): Skim just steps aside.
    pub(super) fn skim_yield(&mut self) {
        self.skim = None;
    }

    /// Next (the button, the key, the media key) while skimming: the next song's window.
    pub(super) fn skim_next(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self
            .driver
            .as_ref()
            .and_then(|d| d.coordinator().queue().peek_next().cloned())
        else {
            // The end of the list.
            self.skim_cancel(cx);
            return;
        };
        let window_ms = match self.skim.as_ref() {
            Some(s) => s.window_ms,
            None => return,
        };
        let from = start_ms(next.track.duration.map(|d| d.as_millis() as u64), window_ms);
        if let Some(skim) = self.skim.as_mut() {
            skim.entry = Some(next.id);
            skim.start_ms = from;
        }
        self.handle(Event::NextAt { position_ms: from }, cx);
        self.prefetch_upcoming();
    }

    /// Previous while skimming: the song before, from its window start; at the first song, its
    /// own window again.
    pub(super) fn skim_previous(&mut self, cx: &mut Context<Self>) {
        let window_ms = match self.skim.as_ref() {
            Some(s) => s.window_ms,
            None => return,
        };
        let before = self
            .driver
            .as_ref()
            .and_then(|d| d.coordinator().queue().history().last().cloned());
        match before {
            Some(prev) => {
                let from = start_ms(prev.track.duration.map(|d| d.as_millis() as u64), window_ms);
                if let Some(skim) = self.skim.as_mut() {
                    skim.entry = Some(prev.id);
                    skim.start_ms = from;
                }
                self.handle(Event::PlayEntry { id: prev.id }, cx);
                self.handle(Event::Seek { position_ms: from }, cx);
            }
            None => {
                let from = self.skim.as_ref().map_or(0, |s| s.start_ms);
                self.handle(Event::Seek { position_ms: from }, cx);
            }
        }
    }

    fn restore_before(
        &mut self,
        before: Option<(QueueSnapshot, u64)>,
        resume: bool,
        cx: &mut Context<Self>,
    ) {
        let restored = before.and_then(|(mut snapshot, position_ms)| {
            let live = self.driver.as_ref()?.coordinator().queue().next_id();
            snapshot.next_id = snapshot.next_id.max(live);
            Some((Queue::restore(snapshot).ok()?, position_ms))
        });
        match (restored, self.driver.as_mut()) {
            (Some((queue, position_ms)), Some(d)) => {
                d.replace_queue(queue, position_ms);
                if resume {
                    d.handle(Event::Play);
                }
            }
            (None, Some(d)) => d.handle(Event::Stop),
            _ => {}
        }
        self.sync(cx);
        self.prefetch_upcoming();
    }

    fn ensure_skim_ticker(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.skim_ticking, true) {
            return;
        }
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            match this.update(cx, |m, cx| m.skim_tick(cx)) {
                Ok(true) => {}
                _ => {
                    let _ = this.update(cx, |m, _| m.skim_ticking = false);
                    break;
                }
            }
        })
        .detach();
    }

    /// One look at the playhead: moves on when the window is covered, positions a song that was
    /// reached some other way, and steps aside when the list is over or the user took over.
    /// Returns whether the ticker should keep going. Public for tests, which do not sleep.
    #[doc(hidden)]
    pub fn skim_tick(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((window_ms, start, entry, listed)) = self.skim.as_ref().map(|s| {
            (s.window_ms, s.start_ms, s.entry, s.entries.clone())
        }) else {
            return false;
        };
        let Some(d) = self.driver.as_ref() else {
            self.skim = None;
            return false;
        };
        let c = d.coordinator();
        let Some(current) = c.queue().current() else {
            return self.skim_over(cx);
        };
        if !listed.contains(&current.id) {
            // A list of the user's own now plays.
            self.skim = None;
            cx.notify();
            return false;
        }
        if matches!(c.phase(), Phase::Idle | Phase::Failed) {
            return self.skim_over(cx);
        }
        let id = current.id;
        let track_ms = current.track.duration.map(|t| t.as_millis() as u64);
        let duration = c.duration_ms().or(track_ms);
        let position = c.position_ms();
        // Only a playhead that is really running counts (not one parked while loading).
        let running = c.intent() == Intent::Playing && c.phase() == Phase::Playing;
        let has_next = c.queue().peek_next().is_some();
        if entry != Some(id) {
            // Reached without Skim's own step (a song that ended early, a pick in the queue,
            // a song that failed and was skipped): play it from its window start.
            let from = start_ms(duration, window_ms);
            if let Some(skim) = self.skim.as_mut() {
                skim.entry = Some(id);
                skim.start_ms = from;
            }
            self.handle(Event::Seek { position_ms: from }, cx);
            return true;
        }
        let song_end = duration.is_some_and(|d| position + END_SLACK_MS >= d);
        if running && (position >= start + window_ms || (song_end && position > start)) {
            if !has_next {
                return self.skim_over(cx);
            }
            self.skim_next(cx);
        }
        // Keep the pill and the bar moving.
        cx.notify();
        true
    }

    /// The list has run out (or nothing is left to play): back to what was playing before.
    fn skim_over(&mut self, cx: &mut Context<Self>) -> bool {
        self.skim_cancel(cx);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_starts_a_third_of_the_way_in() {
        assert_eq!(start_ms(Some(180_000), 20_000), 60_000);
    }

    #[test]
    fn a_short_song_starts_early_enough_to_fit_the_window() {
        assert_eq!(start_ms(Some(60_000), 20_000), 20_000);
        assert_eq!(start_ms(Some(25_000), 20_000), 5_000);
        assert_eq!(start_ms(Some(10_000), 20_000), 0);
    }

    #[test]
    fn a_song_of_unknown_length_starts_at_the_beginning() {
        assert_eq!(start_ms(None, 20_000), 0);
    }

    #[test]
    fn the_progress_bar_shows_only_the_window() {
        let v = SkimView {
            index: 3,
            total: 40,
            start_ms: 60_000,
            window_ms: 20_000,
        };
        assert_eq!(v.in_window(60_000), (0, 20_000));
        assert_eq!(v.in_window(70_000), (10_000, 20_000));
        assert_eq!(v.in_window(30_000), (0, 20_000), "before the window");
        assert_eq!(v.in_window(99_000), (20_000, 20_000), "past it");
        assert_eq!(v.to_song(5_000), 65_000);
        assert_eq!(v.to_song(99_000), 80_000);
        assert_eq!(v.label(), "Skimming 3 of 40");
    }
}
