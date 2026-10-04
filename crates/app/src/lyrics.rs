//! Lyrics for the playing track: looked up off the UI thread when a track becomes current, and
//! for the track after it, so they are there when it starts. Answers (including "no lyrics")
//! are kept for the session on top of the SQLite cache the service has.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use apollo_domain::model::Track;
use apollo_lyrics::{Line, Lyrics, LyricsQuery, LyricsService, LyricsSource};
use gpui::{Context, Entity};

use crate::player::PlayerModel;

/// Answers kept in memory; the oldest go first.
const MAX_SLOTS: usize = 24;
/// A lookup that could not reach the source is tried again this long after, a few times.
const RETRY_AFTER: Duration = Duration::from_secs(15);
const MAX_ATTEMPTS: u32 = 3;

/// What the lyrics pane shows for the current track.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LyricsState {
    /// Nothing is queued, or nothing is known about the track yet.
    Idle,
    Loading,
    /// Looked up; there are none (or the source could not be reached).
    Missing,
    Ready(Arc<Lyrics>),
}

enum Slot {
    Loading,
    Ready(Option<Arc<Lyrics>>),
    /// The source could not be reached (lookups are retried a few times).
    Failed,
}

pub struct LyricsModel {
    player: Entity<PlayerModel>,
    service: Option<Arc<LyricsService>>,
    slots: HashMap<String, Slot>,
    /// Slot keys, oldest first, for eviction.
    order: VecDeque<String>,
    /// The current and next track as last looked at, so a position tick costs one comparison.
    seen: (Option<String>, Option<String>),
}

impl LyricsModel {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |this, _, cx| this.refresh(cx)).detach();
        Self {
            player,
            service: None,
            slots: HashMap::new(),
            order: VecDeque::new(),
            seen: (None, None),
        }
    }

    /// The lookup is ready: lyrics start loading for what is queued.
    pub fn install(&mut self, service: Arc<LyricsService>, cx: &mut Context<Self>) {
        self.service = Some(service);
        self.seen = (None, None);
        self.refresh(cx);
        cx.notify();
    }

    pub fn is_installed(&self) -> bool {
        self.service.is_some()
    }

    /// What to show for `video_id`.
    pub fn state(&self, video_id: Option<&str>) -> LyricsState {
        let Some(id) = video_id else {
            return LyricsState::Idle;
        };
        // Until the lookup is installed (a moment at startup) nothing can be said either way.
        if self.service.is_none() {
            return LyricsState::Loading;
        }
        match self.slots.get(id) {
            None | Some(Slot::Loading) => LyricsState::Loading,
            Some(Slot::Ready(Some(lyrics))) => LyricsState::Ready(lyrics.clone()),
            Some(Slot::Ready(None)) | Some(Slot::Failed) => LyricsState::Missing,
        }
    }

    /// Look up the current track and the one after it, once each.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (current, next) = {
            let player = self.player.read(cx);
            let Some(c) = player.coordinator() else {
                return;
            };
            let queue = c.queue();
            (
                queue.current().map(|e| e.track.clone()),
                queue.upcoming().next().map(|e| e.track.clone()),
            )
        };
        let ids = (
            current.as_ref().map(|t| t.video_id.as_str().to_owned()),
            next.as_ref().map(|t| t.video_id.as_str().to_owned()),
        );
        if ids == self.seen {
            return;
        }
        self.seen = ids;
        for track in current.iter().chain(next.iter()) {
            self.ensure(track, cx);
        }
    }

    fn ensure(&mut self, track: &Track, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            return;
        };
        let id = track.video_id.as_str().to_owned();
        if self.slots.contains_key(&id) {
            return;
        }
        self.begin(id, query_for(track), service, 0, cx);
    }

    fn begin(
        &mut self,
        id: String,
        query: LyricsQuery,
        service: Arc<LyricsService>,
        attempts: u32,
        cx: &mut Context<Self>,
    ) {
        if self.slots.insert(id.clone(), Slot::Loading).is_none() {
            self.order.push_back(id.clone());
            while self.order.len() > MAX_SLOTS {
                if let Some(old) = self.order.pop_front() {
                    self.slots.remove(&old);
                }
            }
        }
        cx.spawn(async move |this, cx| {
            let looked_up = cx
                .background_executor()
                .spawn({
                    let (service, query) = (service.clone(), query.clone());
                    async move { service.lookup(&query) }
                })
                .await;
            let retry = this
                .update(cx, |this, cx| this.finish(&id, looked_up, attempts, cx))
                .ok()
                .flatten();
            if let Some(attempts) = retry {
                cx.background_executor().timer(RETRY_AFTER).await;
                let _ = this.update(cx, |this, cx| {
                    // Only while it is still one of the tracks being looked at.
                    let wanted = this.seen.0.as_deref() == Some(id.as_str())
                        || this.seen.1.as_deref() == Some(id.as_str());
                    if wanted && matches!(this.slots.get(&id), Some(Slot::Failed)) {
                        this.begin(id, query, service, attempts, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// A lookup finished. Returns the attempt count to retry with, if it should be tried again.
    fn finish(
        &mut self,
        id: &str,
        result: Result<Option<Lyrics>, apollo_lyrics::FetchError>,
        attempts: u32,
        cx: &mut Context<Self>,
    ) -> Option<u32> {
        let mut retry = None;
        // A slot evicted while the lookup ran stays gone.
        if self.slots.contains_key(id) {
            let slot = match result {
                Ok(found) => Slot::Ready(found.map(Arc::new)),
                Err(err) => {
                    eprintln!("lyrics for {id}: {err}");
                    let attempts = attempts + 1;
                    retry = (attempts < MAX_ATTEMPTS).then_some(attempts);
                    Slot::Failed
                }
            };
            self.slots.insert(id.to_owned(), slot);
            cx.notify();
        }
        retry
    }
}

/// How the lookup knows a track.
pub fn query_for(track: &Track) -> LyricsQuery {
    let names: Vec<&str> = track.artists.iter().map(|a| a.name.as_str()).collect();
    LyricsQuery {
        video_id: track.video_id.as_str().to_owned(),
        title: track.title.clone(),
        artist: names.join(", "),
        first_artist: names.first().map(|n| (*n).to_owned()).unwrap_or_default(),
        album: track
            .album
            .as_ref()
            .map(|a| a.title.clone())
            .filter(|t| !t.is_empty()),
        duration_secs: track.duration.map(|d| d.as_secs_f64().round() as u32),
    }
}

/// Index of the line being sung at `position_ms`: the last one that has started. `None` before
/// the first line.
pub fn current_line(lines: &[Line], position_ms: u64) -> Option<usize> {
    lines
        .partition_point(|l| l.start_ms <= position_ms)
        .checked_sub(1)
}

/// How long the last line of a song is given when there is no line after it to end it.
pub const LAST_LINE_MS: u64 = 5_000;
/// A line that does not fit is shown in pages over the time it is sung, but never slower than
/// this per character (a long instrumental gap after it does not stretch the pages) ...
pub const PAGE_MS_PER_CHAR: u64 = 100;
/// ... and never given less than this to be spread over.
pub const PAGE_MIN_WINDOW_MS: u64 = 3_000;

/// Splits `text` at word boundaries into the fewest chunks that each `fits` on one line. Words
/// are packed greedily, which is the fewest possible for a run of words in order. A single word
/// that does not fit is a chunk of its own (the view's ellipsis is the last resort for it).
pub fn page_chunks(text: &str, fits: impl Fn(&str) -> bool) -> Vec<String> {
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        let candidate_len = current.len() + 1 + word.len();
        let mut candidate = String::with_capacity(candidate_len);
        candidate.push_str(&current);
        candidate.push(' ');
        candidate.push_str(word);
        if fits(&candidate) {
            current = candidate;
        } else {
            chunks.push(std::mem::replace(&mut current, word.to_owned()));
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// The time the pages of a line are spread over: until the next line starts (or
/// [`LAST_LINE_MS`] for the last), but not stretched past what the line's length is worth.
pub fn page_window_ms(chars: usize, start_ms: u64, next_start_ms: Option<u64>) -> u64 {
    let natural = next_start_ms
        .map(|next| next.saturating_sub(start_ms))
        .unwrap_or(LAST_LINE_MS);
    let cap = (chars as u64 * PAGE_MS_PER_CHAR).max(PAGE_MIN_WINDOW_MS);
    natural.min(cap).max(1)
}

/// Which page shows `elapsed_ms` into a line whose pages share `window_ms` in proportion to
/// their length in characters. Past the window it stays on the last page.
pub fn page_index(chunks: &[String], window_ms: u64, elapsed_ms: u64) -> usize {
    let total: u64 = chunks.iter().map(|c| c.chars().count() as u64).sum();
    if chunks.len() < 2 || total == 0 {
        return 0;
    }
    let mut seen = 0u64;
    for (i, chunk) in chunks.iter().enumerate() {
        seen += chunk.chars().count() as u64;
        // This page ends when its cumulative share of the window has passed.
        if elapsed_ms * total < window_ms * seen {
            return i;
        }
    }
    chunks.len() - 1
}

/// Lyrics for the simulated backend's tracks, so the pane can be seen and tested without a
/// network: the first fixture has synced lines, the second plain text, the rest none.
pub struct SimLyrics;

impl LyricsSource for SimLyrics {
    fn fetch(&self, query: &LyricsQuery) -> Result<Option<Lyrics>, apollo_lyrics::FetchError> {
        Ok(match query.video_id.as_str() {
            "simTrack001" => Some(Lyrics::Synced(
                SIM_SYNCED
                    .iter()
                    .enumerate()
                    .map(|(i, text)| Line {
                        start_ms: 4_000 + i as u64 * 6_000,
                        text: (*text).to_owned(),
                    })
                    .collect(),
            )),
            "simTrack002" => Some(Lyrics::Plain(SIM_SYNCED.join("\n"))),
            _ => None,
        })
    }
}

const SIM_SYNCED: &[&str] = &[
    "Headlights pull the dark apart",
    "Every mile a quiet start",
    "Radio is humming low",
    "Nowhere that we need to go",
    "Streetlights blur to amber seas",
    "Window down and let it breeze",
    "Hold the wheel and hold the night",
    "Everything is going right",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start_ms: u64) -> Line {
        Line {
            start_ms,
            text: String::new(),
        }
    }

    #[test]
    fn the_current_line_is_the_last_one_that_has_started() {
        let lines = [line(1000), line(5000), line(9000)];
        assert_eq!(current_line(&lines, 0), None);
        assert_eq!(current_line(&lines, 999), None);
        assert_eq!(current_line(&lines, 1000), Some(0));
        assert_eq!(current_line(&lines, 4999), Some(0));
        assert_eq!(current_line(&lines, 5000), Some(1));
        assert_eq!(current_line(&lines, 600_000), Some(2));
        assert_eq!(current_line(&[], 5000), None);
    }

    #[test]
    fn the_query_carries_artists_album_and_whole_seconds() {
        use apollo_domain::model::{AlbumRef, ArtistRef};
        let mut t = Track::new("abcdefghijk", "Title");
        t.artists = vec![
            ArtistRef {
                name: "A".into(),
                id: None,
            },
            ArtistRef {
                name: "B".into(),
                id: None,
            },
        ];
        t.album = Some(AlbumRef {
            title: "Album".into(),
            id: None,
        });
        t.duration = Some(Duration::from_millis(200_400));
        let q = query_for(&t);
        assert_eq!(q.artist, "A, B");
        assert_eq!(q.first_artist, "A");
        assert_eq!(q.album.as_deref(), Some("Album"));
        assert_eq!(q.duration_secs, Some(200));
        let bare = query_for(&Track::new("abcdefghijk", "T"));
        assert_eq!(
            (bare.album, bare.duration_secs, bare.artist),
            (None, None, String::new())
        );
    }

    fn fits_in(chars: usize) -> impl Fn(&str) -> bool {
        move |text| text.chars().count() <= chars
    }

    #[test]
    fn a_line_that_fits_is_one_page_and_a_long_one_splits_at_words() {
        assert_eq!(page_chunks("short line", fits_in(20)), vec!["short line"]);
        assert_eq!(
            page_chunks("Call it morning driving through the south", fits_in(20)),
            vec!["Call it morning", "driving through the", "south"]
        );
        assert!(page_chunks("   ", fits_in(10)).is_empty());
        assert!(page_chunks("", fits_in(10)).is_empty());
    }

    #[test]
    fn pages_are_the_fewest_that_fit_and_each_fits() {
        let text = "one two three four five six seven eight nine ten eleven twelve";
        for width in [10usize, 14, 20, 30] {
            let pages = page_chunks(text, fits_in(width));
            assert!(
                pages.iter().all(|p| p.chars().count() <= width),
                "{pages:?}"
            );
            assert_eq!(pages.join(" "), text, "nothing lost or reordered");
            // Fewest: no page could have taken the first word of the next one.
            for pair in pages.windows(2) {
                let first_of_next = pair[1].split(' ').next().unwrap();
                assert!(
                    pair[0].chars().count() + 1 + first_of_next.chars().count() > width,
                    "{pair:?}"
                );
            }
        }
    }

    #[test]
    fn a_word_too_long_for_a_line_is_a_page_of_its_own() {
        assert_eq!(
            page_chunks("a Supercalifragilistic b", fits_in(8)),
            vec!["a", "Supercalifragilistic", "b"]
        );
    }

    #[test]
    fn pages_share_the_window_in_proportion_to_their_length() {
        let pages: Vec<String> = ["aaaaaa", "bbbbbbbbbbbb", "cccccc"] // 6, 12, 6 of 24
            .iter()
            .map(|s| s.to_string())
            .collect();
        // 4800 ms: the first page takes 1200, the second 2400, the third 1200.
        assert_eq!(page_index(&pages, 4800, 0), 0);
        assert_eq!(page_index(&pages, 4800, 1199), 0);
        assert_eq!(page_index(&pages, 4800, 1200), 1);
        assert_eq!(page_index(&pages, 4800, 3599), 1);
        assert_eq!(page_index(&pages, 4800, 3600), 2);
        assert_eq!(page_index(&pages, 4800, 4799), 2);
        assert_eq!(page_index(&pages, 4800, 99_999), 2, "stays on the last");
        assert_eq!(page_index(&pages[..1], 4800, 3000), 0);
        assert_eq!(page_index(&[], 4800, 0), 0);
    }

    #[test]
    fn the_window_is_the_gap_to_the_next_line_within_what_the_line_is_worth() {
        // A 40-character line sung over its 6 s gap.
        assert_eq!(
            page_window_ms(40, 10_000, Some(16_000)),
            4_000,
            "capped at 100 ms a char"
        );
        assert_eq!(
            page_window_ms(80, 10_000, Some(16_000)),
            6_000,
            "the whole gap"
        );
        assert_eq!(
            page_window_ms(80, 10_000, Some(10_500)),
            500,
            "a short gap is a short gap"
        );
        assert_eq!(
            page_window_ms(10, 10_000, Some(60_000)),
            3_000,
            "never stretched over a break"
        );
        assert_eq!(page_window_ms(60, 10_000, None), 5_000, "the last line");
        assert_eq!(page_window_ms(60, 10_000, Some(9_000)), 1, "never zero");
    }
}
