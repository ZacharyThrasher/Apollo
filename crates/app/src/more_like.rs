//! More like this: songs to add to a playlist of your own, shown under its last row.
//!
//! * The songs are the radio of one of the playlist's own songs (the same feed Start radio
//!   uses), cleaned: songs the playlist already holds and blocked artists' songs are dropped, and
//!   a song that has been offered once is not offered again.
//! * The first ten are shown. Refresh shows the next ten from what was already fetched, and
//!   asks for the radio of another song of the playlist when that runs out.
//! * [`MoreLikeModel::add`] puts a song in the playlist: the page shows the row at once (the
//!   library model does that), [`LibraryWrites::add_to_playlist`] sends the write, and a refused
//!   one hands the song back here and takes the row off the page again.
//! * Nothing is fetched unless the page on top is a playlist the account owns with all its rows
//!   loaded, and the Settings switch is on. Answers for a page that is no longer on screen are
//!   dropped (`rev`).

use std::collections::{HashMap, HashSet, VecDeque};

use apollo_domain::blocklist::retain_allowed;
use apollo_domain::library::{CollectionKind, PlaylistTarget};
use apollo_domain::listing::LoadFailure;
use gpui::{Context, Entity};

use crate::library::source::TrackRow;
use crate::library::model::LibraryModel;
use crate::library::{LibraryWrites, WritesEvent};
use crate::player::PlayerModel;

/// How many songs the section shows at once.
pub const SUGGESTIONS: usize = 10;

/// What the section shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Not for this page (not a playlist of yours, switched off) or nothing to offer.
    Hidden,
    /// The first answer is on its way: the skeleton.
    Loading,
    Ready,
    Failed(LoadFailure),
}

pub struct MoreLikeModel {
    player: Entity<PlayerModel>,
    library: Entity<LibraryModel>,
    writes: Entity<LibraryWrites>,
    /// Settings > Playlists > Show More like this.
    enabled: bool,
    /// The playlist the songs are for (its library key).
    key: Option<String>,
    shown: Vec<TrackRow>,
    /// Fetched and not shown yet, in the service's order.
    pool: VecDeque<TrackRow>,
    /// Every song that was shown or pooled for this page: never offered twice.
    seen: HashSet<String>,
    /// The songs of the playlist, as of the last look: what the suggestions must not repeat.
    held: HashSet<String>,
    held_rows: usize,
    /// Songs added from here and not yet answered, so a refused one can come back.
    adding: HashMap<String, TrackRow>,
    /// Radios asked for this page: which song seeds the next.
    asked: usize,
    loading: bool,
    /// Whether the answer in flight replaces what is shown (Refresh) or only tops it up.
    replace: bool,
    failure: Option<LoadFailure>,
    /// Bumped by every reset; an answer for an older one is dropped.
    rev: u64,
    /// The source has been asked this many times (tests and diagnostics).
    requests: u32,
}

impl MoreLikeModel {
    pub fn new(
        player: Entity<PlayerModel>,
        library: Entity<LibraryModel>,
        writes: Entity<LibraryWrites>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&library, |m, _, cx| m.sync(cx)).detach();
        cx.subscribe(&writes, |m, _, event, cx| {
            if let WritesEvent::PlaylistAddSettled {
                playlist_id,
                video_id,
                added,
            } = event
            {
                m.add_settled(playlist_id, video_id, *added, cx);
            }
        })
        .detach();
        Self {
            player,
            library,
            writes,
            enabled: true,
            key: None,
            shown: Vec::new(),
            pool: VecDeque::new(),
            seen: HashSet::new(),
            held: HashSet::new(),
            held_rows: 0,
            adding: HashMap::new(),
            asked: 0,
            loading: false,
            replace: false,
            failure: None,
            rev: 0,
            requests: 0,
        }
    }

    // ---- reading -----------------------------------------------------------------------

    pub fn phase(&self) -> Phase {
        if self.key.is_none() {
            return Phase::Hidden;
        }
        if !self.shown.is_empty() {
            return Phase::Ready;
        }
        if let Some(f) = self.failure {
            return Phase::Failed(f);
        }
        if self.loading {
            return Phase::Loading;
        }
        Phase::Hidden
    }

    /// The songs on offer, in the order shown.
    pub fn songs(&self) -> &[TrackRow] {
        &self.shown
    }

    /// A newer set is on its way while the previous one stays on screen.
    pub fn is_refreshing(&self) -> bool {
        self.loading && !self.shown.is_empty()
    }

    pub fn requests(&self) -> u32 {
        self.requests
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    // ---- loading -----------------------------------------------------------------------

    /// The Settings switch changed.
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        self.sync(cx);
        cx.notify();
    }

    /// Follows the page on top: a playlist of your own with every row loaded gets suggestions,
    /// anything else none. Runs on every change of the library model, so it is cheap when
    /// nothing moved.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let (key, rows) = {
            let model = self.library.read(cx);
            match model.detail() {
                Some(d)
                    if self.enabled
                        && d.collection.kind == CollectionKind::Playlist
                        && d.can_edit()
                        && !d.listing.items().is_empty() =>
                {
                    (
                        Some(d.collection.library_key().to_owned()),
                        d.listing.items().len(),
                    )
                }
                _ => (None, 0),
            }
        };
        if key != self.key {
            self.reset(key, cx);
            return;
        }
        if self.key.is_some() && rows != self.held_rows {
            self.refresh_held(cx);
            cx.notify();
        }
    }

    /// Another page (or none): forget everything and, for a playlist, ask for its first set.
    fn reset(&mut self, key: Option<String>, cx: &mut Context<Self>) {
        self.rev += 1;
        self.key = key;
        self.shown.clear();
        self.pool.clear();
        self.seen.clear();
        self.held.clear();
        self.held_rows = 0;
        self.adding.clear();
        self.asked = 0;
        self.loading = false;
        self.replace = false;
        self.failure = None;
        if self.key.is_some() {
            self.refresh_held(cx);
            self.fetch(false, cx);
        }
        cx.notify();
    }

    /// Reads the playlist's songs again and takes any that are now in it off the offer.
    fn refresh_held(&mut self, cx: &mut Context<Self>) {
        let held: HashSet<String> = match self.library.read(cx).detail() {
            Some(d) => d
                .listing
                .items()
                .iter()
                .map(|r| r.track.video_id.as_str().to_owned())
                .collect(),
            None => HashSet::new(),
        };
        self.held_rows = self.library.read(cx).detail().map_or(0, |d| d.listing.items().len());
        self.shown.retain(|r| !held.contains(r.track.video_id.as_str()));
        self.pool.retain(|r| !held.contains(r.track.video_id.as_str()));
        self.held = held;
    }

    /// Show another set: the next ten of what is fetched, else the radio of another song.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.key.is_none() || self.loading {
            return;
        }
        if self.pool.len() >= SUGGESTIONS {
            self.shown = self.pool.drain(..SUGGESTIONS).collect();
            cx.notify();
            return;
        }
        self.fetch(true, cx);
    }

    /// Try again after a failure.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if self.key.is_some() && !self.loading {
            self.failure = None;
            self.fetch(false, cx);
        }
    }

    /// Asks for the radio of the next seed song of the playlist. `replace`: the answer takes the
    /// place of what is shown (Refresh) instead of filling the gaps.
    fn fetch(&mut self, replace: bool, cx: &mut Context<Self>) {
        let seed = {
            let model = self.library.read(cx);
            let Some(d) = model.detail() else { return };
            let rows = d.listing.items();
            if rows.is_empty() {
                return;
            }
            rows[seed_index(self.asked, rows.len())]
                .track
                .video_id
                .as_str()
                .to_owned()
        };
        let Some(source) = self.player.read(cx).library_source() else {
            self.failure = Some(LoadFailure::Other);
            return;
        };
        self.asked += 1;
        self.requests += 1;
        self.loading = true;
        self.replace = replace;
        self.failure = None;
        let rev = self.rev;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.radio(&seed) })
                .await;
            let _ = this.update(cx, |m, cx| m.arrived(rev, result, cx));
        })
        .detach();
    }

    fn arrived(
        &mut self,
        rev: u64,
        result: Result<Vec<TrackRow>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        if rev != self.rev {
            return;
        }
        self.loading = false;
        match result {
            Ok(rows) => {
                // A blocked artist's songs are not offered, as in a radio.
                let blocked = crate::blocked_artists::snapshot(cx);
                let rows = retain_allowed(rows, &blocked, |r| &r.track);
                let fresh: Vec<TrackRow> = rows
                    .into_iter()
                    .filter(|r| {
                        let id = r.track.video_id.as_str();
                        !self.held.contains(id) && self.seen.insert(id.to_owned())
                    })
                    .collect();
                self.pool.extend(fresh);
                // Refresh shows a new set when there is one; otherwise the old stays.
                if self.replace && !self.pool.is_empty() {
                    let take = self.pool.len().min(SUGGESTIONS);
                    self.shown = self.pool.drain(..take).collect();
                } else {
                    while self.shown.len() < SUGGESTIONS {
                        match self.pool.pop_front() {
                            Some(row) => self.shown.push(row),
                            None => break,
                        }
                    }
                }
                self.failure = None;
            }
            Err(failure) => self.failure = Some(failure),
        }
        self.replace = false;
        cx.notify();
    }

    // ---- adding ------------------------------------------------------------------------

    /// Put a suggested song in the playlist: its row shows on the page at once and leaves the
    /// offer, the write follows, and a refused write brings both back.
    pub fn add(&mut self, video_id: &str, cx: &mut Context<Self>) {
        let Some(key) = self.key.clone() else { return };
        let Some(pos) = self
            .shown
            .iter()
            .position(|r| r.track.video_id.as_str() == video_id)
        else {
            return;
        };
        let (title, thumbnail) = match self.library.read(cx).detail() {
            Some(d) => (d.collection.title.clone(), d.collection.thumbnail.clone()),
            None => return,
        };
        let row = self.shown[pos].clone();
        let shown = self
            .library
            .update(cx, |m, cx| m.show_added_row(&key, row.clone(), cx));
        if !shown {
            return;
        }
        self.shown.remove(pos);
        if let Some(next) = self.pool.pop_front() {
            self.shown.push(next);
        }
        self.adding.insert(video_id.to_owned(), row);
        let target = PlaylistTarget {
            id: key,
            title,
            thumbnail,
            holds_track: false,
        };
        self.writes
            .update(cx, |w, cx| w.add_to_playlist(&target, video_id, cx));
        // The row is in the playlist now (the library model said so), which `sync` sees.
        if self.shown.len() < SUGGESTIONS && !self.loading {
            self.fetch(false, cx);
        }
        cx.notify();
    }

    /// A write for a song added from here was answered: a refused one comes back to the offer.
    fn add_settled(&mut self, playlist_id: &str, video_id: &str, added: bool, cx: &mut Context<Self>) {
        if self.key.as_deref() != Some(playlist_id) {
            return;
        }
        let Some(row) = self.adding.remove(video_id) else {
            return;
        };
        if !added {
            self.shown.insert(0, row);
            self.shown.truncate(SUGGESTIONS);
            cx.notify();
        }
    }
}

/// The song whose radio is asked for the `asked`th time: the first song, then steps that skip
/// about the playlist so each set comes from somewhere else in it. Every song gets its turn
/// before one repeats.
pub fn seed_index(asked: usize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    // A step that shares no factor with the length visits every song once.
    let step = [7usize, 11, 13, 17, 19, 23]
        .into_iter()
        .find(|s| gcd(*s, len) == 1)
        .unwrap_or(1);
    (asked * step) % len
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_seed_is_the_first_song() {
        for len in 1..40 {
            assert_eq!(seed_index(0, len), 0);
        }
    }

    #[test]
    fn every_song_is_a_seed_once_before_any_repeats() {
        for len in 1..60 {
            let mut seen = HashSet::new();
            for asked in 0..len {
                assert!(seen.insert(seed_index(asked, len)), "len {len}, asked {asked}");
            }
        }
    }

    #[test]
    fn an_empty_playlist_has_no_seed_to_step_to() {
        assert_eq!(seed_index(3, 0), 0);
    }
}
