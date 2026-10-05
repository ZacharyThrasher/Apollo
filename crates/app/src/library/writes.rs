//! Library writes: liking and unliking tracks, telling YouTube Music a song is not wanted, and
//! adding a track to a playlist.
//!
//! The model keeps what the screens show about each track's like state and applies a change
//! the moment it is asked for (optimistically). The write then goes to the service on a
//! background thread through [`LibrarySource`]:
//!
//! * success keeps what is shown;
//! * a definite refusal rolls the heart back and raises a short [`Notice`];
//! * a write whose outcome is unknown (a timeout, a dropped connection, an unreadable reply)
//!   is never retried blindly: the state is read back from the service and that answer decides.
//!
//! Only one write per track is ever in flight. Toggling again while one is out records the new
//! wish, which goes out after the first settles, so two requests can never reorder.
//!
//! Answers that belong to an account that has since been replaced are dropped (`generation`).
//! No views here; the player bar, Now Playing and the track lists draw from it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use apollo_domain::library::{AddedRow, PlaylistTarget, WriteFailure};
use gpui::{Context, Entity, EventEmitter, SharedString};

use super::source::LibrarySource;
use super::undo::{Action, Made, PlaylistLog, PlaylistRef, Step};
use crate::page_cache::PageCache;
use crate::player::PlayerModel;

/// How long an error stays on screen.
pub const NOTICE_FOR: Duration = Duration::from_secs(4);

/// Pause between one bulk like settling and the next going out.
pub const BULK_PACE: Duration = Duration::from_millis(250);
/// How often a bulk run looks at whether the write it is waiting for has settled.
const BULK_POLL: Duration = Duration::from_millis(50);
/// The longest a bulk run waits for one write before it moves on.
const BULK_WAIT: Duration = Duration::from_secs(30);

/// What the screens learn from the writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WritesEvent {
    /// The service holds this like state for the track now (a write landed, or a read-back
    /// showed that one had). Liked Music follows these.
    LikeSettled { video_id: String, liked: bool },
    /// A song's add to a playlist was answered: `added` is whether the service holds it now. A
    /// page showing the playlist reads it again (landed) or takes the row it showed back (not).
    PlaylistAddSettled {
        playlist_id: String,
        video_id: String,
        added: bool,
    },
    /// Several songs were added to a playlist in one request and the service holds them now.
    PlaylistAddedMany {
        playlist_id: String,
        video_ids: Vec<String>,
    },
}

/// A short error, shown for [`NOTICE_FOR`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub id: u64,
    pub text: SharedString,
}

#[derive(Clone, Debug)]
struct LikeEntry {
    /// What the heart shows.
    shown: bool,
    /// What the service is known to hold, when known.
    confirmed: Option<bool>,
    /// The write (or read-back) in flight, with the value it carries.
    inflight: Option<bool>,
    /// A change made here. It outranks rows loaded from the service until the account changes:
    /// a page fetched before the write must not undo it.
    local: bool,
}

pub struct LibraryWrites {
    player: Entity<PlayerModel>,
    likes: HashMap<String, LikeEntry>,
    /// Songs a "Not interested" is out for.
    disliking: HashSet<String>,
    /// Songs the service is known to hold a thumbs-down for (this session).
    disliked: HashSet<String>,
    /// `(playlist id, video id)` pairs added here: shown as held right away, taken back on failure.
    added: HashSet<(String, String)>,
    /// The current track last asked about, so each track is asked once.
    checked: Option<String>,
    /// Bumped when the account changes; older answers are dropped.
    generation: u64,
    /// Likes of a selection still to go out, one at a time (see [`Self::set_liked_many`]).
    bulk: std::collections::VecDeque<(String, bool)>,
    bulk_running: bool,
    /// Likes of the run in progress that the service has taken: one confirmation says how many.
    bulk_liked: usize,
    notice: Option<Notice>,
    next_notice: u64,
    /// The playlist edits Ctrl+Z can take back (see [`super::undo`]).
    log: PlaylistLog,
}

impl EventEmitter<WritesEvent> for LibraryWrites {}

impl LibraryWrites {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        // The playing track's state is fetched when it changes (and once the backend is there).
        cx.observe(&player, |m, _, cx| m.check_current(cx)).detach();
        Self {
            player,
            likes: HashMap::new(),
            disliking: HashSet::new(),
            disliked: HashSet::new(),
            added: HashSet::new(),
            checked: None,
            generation: 0,
            bulk: std::collections::VecDeque::new(),
            bulk_running: false,
            bulk_liked: 0,
            notice: None,
            next_notice: 1,
            log: PlaylistLog::default(),
        }
    }

    // ---- reading -----------------------------------------------------------------------

    /// What the heart shows for a track: `None` while nothing is known (drawn as not liked).
    pub fn liked(&self, video_id: &str) -> Option<bool> {
        self.likes.get(video_id).map(|e| e.shown)
    }

    pub fn is_liked(&self, video_id: &str) -> bool {
        self.liked(video_id) == Some(true)
    }

    /// A like write (or its read-back) for this track is out.
    pub fn is_pending(&self, video_id: &str) -> bool {
        self.likes
            .get(video_id)
            .is_some_and(|e| e.inflight.is_some())
    }

    /// The playlist holds the track: the service said so when the list was loaded, or it was
    /// added here.
    pub fn holds(&self, playlist: &PlaylistTarget, video_id: &str) -> bool {
        playlist.holds_track
            || self
                .added
                .contains(&(playlist.id.clone(), video_id.to_owned()))
    }

    /// This session added the track to the playlist (see [`LibraryWrites::holds`], which also
    /// trusts the service's own flag, true only for the track the list was asked about).
    pub fn added_here(&self, playlist: &PlaylistTarget, video_id: &str) -> bool {
        self.added
            .contains(&(playlist.id.clone(), video_id.to_owned()))
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    fn source(&self, cx: &gpui::App) -> Option<Arc<dyn LibrarySource>> {
        self.player.read(cx).library_source()
    }

    // ---- account ------------------------------------------------------------------------

    /// Forget everything: the account changed, and nothing known about the old one applies.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.bulk.clear();
        self.bulk_running = false;
        self.bulk_liked = 0;
        self.likes.clear();
        self.disliking.clear();
        self.disliked.clear();
        self.added.clear();
        self.log.clear();
        self.checked = None;
        self.notice = None;
        self.check_current(cx);
        cx.notify();
    }

    // ---- like state loaded from the service -----------------------------------------------

    /// Tracks the service just said something about (a loaded page). Never overrides a change
    /// made here, nor a track with a write out.
    pub fn seed(&mut self, rows: impl IntoIterator<Item = (String, bool)>, cx: &mut Context<Self>) {
        let mut changed = false;
        for (video_id, liked) in rows {
            changed |= self.seed_one(&video_id, liked);
        }
        if changed {
            cx.notify();
        }
    }

    fn seed_one(&mut self, video_id: &str, liked: bool) -> bool {
        match self.likes.get_mut(video_id) {
            Some(e) if e.local || e.inflight.is_some() => false,
            Some(e) => {
                let changed = e.shown != liked;
                e.shown = liked;
                e.confirmed = Some(liked);
                changed
            }
            None => {
                self.likes.insert(
                    video_id.to_owned(),
                    LikeEntry {
                        shown: liked,
                        confirmed: Some(liked),
                        inflight: None,
                        local: false,
                    },
                );
                true
            }
        }
    }

    /// Ask the service about the playing track when its state is not known yet, the way YouTube
    /// Music's own player bar does.
    fn check_current(&mut self, cx: &mut Context<Self>) {
        let Some(video) = self.player.read(cx).current_video() else {
            return;
        };
        if self.checked.as_deref() == Some(video.as_str()) {
            return;
        }
        if self.likes.contains_key(&video) {
            self.checked = Some(video);
            return;
        }
        // No backend yet: `checked` stays unset, so the next player change asks again.
        let Some(source) = self.source(cx) else {
            return;
        };
        self.checked = Some(video.clone());
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let asked = video.clone();
            let answer = cx
                .background_executor()
                .spawn(async move { source.liked(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                if let Ok(Some(liked)) = answer {
                    if m.seed_one(&video, liked) {
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    // ---- like / unlike ------------------------------------------------------------------

    /// The heart was pressed: flip what is shown.
    pub fn toggle_like(&mut self, video_id: &str, cx: &mut Context<Self>) {
        let now = self.is_liked(video_id);
        self.set_liked(video_id, !now, cx);
    }

    pub fn set_liked(&mut self, video_id: &str, liked: bool, cx: &mut Context<Self>) {
        let entry = self
            .likes
            .entry(video_id.to_owned())
            .or_insert_with(|| LikeEntry {
                // Unknown until now: a failure puts it back to the opposite of this request.
                shown: !liked,
                confirmed: None,
                inflight: None,
                local: true,
            });
        entry.local = true;
        if entry.shown == liked {
            return;
        }
        entry.shown = liked;
        let idle = entry.inflight.is_none();
        // Liked Music is about to change: a cached copy must not be drawn meanwhile.
        if let Some(cache) = PageCache::try_global(cx) {
            cache.invalidate_liked_music();
        }
        cx.notify();
        if idle {
            self.send_like(video_id, liked, cx);
        }
    }

    /// Like (or unlike) a selection's songs. Each goes through [`Self::set_liked`] (shown at once,
    /// rolled back with a notice if the service refuses), but one at a time: the next is asked
    /// for only once the one before has settled, and a short pause later, so a long selection
    /// never reaches the service as a burst. Songs already as asked are skipped.
    pub fn set_liked_many(&mut self, video_ids: Vec<String>, liked: bool, cx: &mut Context<Self>) {
        for id in video_ids {
            if self.liked(&id) != Some(liked) && !self.bulk.iter().any(|(queued, _)| *queued == id) {
                self.bulk.push_back((id, liked));
            }
        }
        if self.bulk_running || self.bulk.is_empty() {
            return;
        }
        self.bulk_running = true;
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |m, cx| {
                    if m.generation != generation {
                        return None;
                    }
                    let next = m.bulk.pop_front();
                    match &next {
                        Some((id, liked)) => m.set_liked(id, *liked, cx),
                        None => {
                            m.bulk_running = false;
                            let liked = std::mem::take(&mut m.bulk_liked);
                            if liked > 0 {
                                crate::confirm::liked(liked, cx);
                            }
                        }
                    }
                    next
                });
                let Ok(Some((id, _))) = next else { return };
                // Wait for that write (or its read-back) to settle, then pause.
                let mut waited = Duration::ZERO;
                loop {
                    cx.background_executor().timer(BULK_POLL).await;
                    waited += BULK_POLL;
                    let pending = this.update(cx, |m, _| m.generation == generation && m.is_pending(&id));
                    if !matches!(pending, Ok(true)) || waited >= BULK_WAIT {
                        break;
                    }
                }
                cx.background_executor().timer(BULK_PACE).await;
            }
        })
        .detach();
    }

    fn send_like(&mut self, video_id: &str, liked: bool, cx: &mut Context<Self>) {
        let Some(source) = self.source(cx) else {
            // Nothing to write through yet: a definite no.
            self.like_failed(video_id, liked, WriteFailure::Unsupported, cx);
            return;
        };
        if let Some(e) = self.likes.get_mut(video_id) {
            e.inflight = Some(liked);
        }
        let (video, generation) = (video_id.to_owned(), self.generation);
        cx.spawn(async move |this, cx| {
            let sent = video.clone();
            let result = cx
                .background_executor()
                .spawn(async move { source.set_liked(&sent, liked) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.generation == generation {
                    m.like_answered(&video, liked, result, cx);
                }
            });
        })
        .detach();
    }

    fn like_answered(
        &mut self,
        video_id: &str,
        sent: bool,
        result: Result<(), WriteFailure>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(()) => self.like_succeeded(video_id, sent, cx),
            Err(failure) if failure.outcome_unknown() => {
                self.read_back_like(video_id, sent, failure, cx)
            }
            Err(failure) => self.like_failed(video_id, sent, failure, cx),
        }
    }

    fn like_succeeded(&mut self, video_id: &str, sent: bool, cx: &mut Context<Self>) {
        let Some(e) = self.likes.get_mut(video_id) else {
            return;
        };
        e.inflight = None;
        e.confirmed = Some(sent);
        let wish = e.shown;
        // Dropped again now that the service holds the change: a page fetched while the write
        // was out may predate it, and must not be what the next visit draws.
        if let Some(cache) = PageCache::try_global(cx) {
            cache.invalidate_liked_music();
        }
        cx.emit(WritesEvent::LikeSettled {
            video_id: video_id.to_owned(),
            liked: sent,
        });
        // Toggled again while the write was out: send the latest wish now.
        if wish != sent {
            self.send_like(video_id, wish, cx);
        } else if sent {
            // A run of likes says how many once it ends; a single one says so now.
            if self.bulk_running {
                self.bulk_liked += 1;
            } else {
                crate::confirm::liked(1, cx);
            }
        }
        cx.notify();
    }

    /// Roll the heart back to what the service holds (or the opposite of the request when that
    /// was never known) and say so.
    fn like_failed(
        &mut self,
        video_id: &str,
        sent: bool,
        failure: WriteFailure,
        cx: &mut Context<Self>,
    ) {
        if let Some(e) = self.likes.get_mut(video_id) {
            e.inflight = None;
            e.shown = e.confirmed.unwrap_or(!sent);
        }
        self.raise(like_failure_text(sent, failure), cx);
        cx.notify();
    }

    /// The write's outcome is unknown. Ask the service what it holds; that answer settles it.
    fn read_back_like(
        &mut self,
        video_id: &str,
        sent: bool,
        failure: WriteFailure,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.source(cx) else {
            self.like_failed(video_id, sent, failure, cx);
            return;
        };
        // Stays "in flight" until the answer: another toggle waits behind it.
        if let Some(e) = self.likes.get_mut(video_id) {
            e.inflight = Some(sent);
        }
        let (video, generation) = (video_id.to_owned(), self.generation);
        cx.spawn(async move |this, cx| {
            let asked = video.clone();
            let held = cx
                .background_executor()
                .spawn(async move { source.liked(&asked) })
                .await;
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                match held {
                    Ok(Some(held)) => {
                        if let Some(e) = m.likes.get_mut(&video) {
                            e.confirmed = Some(held);
                        }
                        if held == sent {
                            m.like_succeeded(&video, sent, cx);
                        } else {
                            m.like_failed(&video, sent, failure, cx);
                        }
                    }
                    _ => m.like_failed(&video, sent, failure, cx),
                }
            });
        })
        .detach();
    }

    // ---- not interested -------------------------------------------------------------------

    /// "Not interested": the thumbs-down YouTube Music's own menu sends. The request goes out
    /// once and is never retried; when its outcome is unknown the service is asked what it holds,
    /// and that answer settles it. A liked song stops being liked at once (the service drops the
    /// like with the dislike) and is put back if the write fails. Success is silent.
    pub fn not_interested(&mut self, video_id: &str, cx: &mut Context<Self>) {
        if self.disliking.contains(video_id) || self.disliked.contains(video_id) {
            return;
        }
        if self.is_pending(video_id) {
            // A like or unlike for this song is still out: the two must not cross.
            self.raise(BUSY_TEXT, cx);
            return;
        }
        let Some(source) = self.source(cx) else {
            self.raise(not_interested_failure_text(WriteFailure::Unsupported), cx);
            return;
        };
        let was_liked = self.is_liked(video_id);
        if was_liked {
            if let Some(e) = self.likes.get_mut(video_id) {
                e.shown = false;
                e.local = true;
            }
            if let Some(cache) = PageCache::try_global(cx) {
                cache.invalidate_liked_music();
            }
        }
        self.disliking.insert(video_id.to_owned());
        cx.notify();
        let (video, generation) = (video_id.to_owned(), self.generation);
        cx.spawn(async move |this, cx| {
            let sent = video.clone();
            let result = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { source.set_disliked(&sent) })
                    .await
            };
            // An unknown outcome is read back, never sent again.
            let result = match result {
                Err(failure) if failure.outcome_unknown() => {
                    let asked = video.clone();
                    let held = cx
                        .background_executor()
                        .spawn(async move { source.disliked(&asked) })
                        .await;
                    match held {
                        Ok(Some(true)) => Ok(()),
                        _ => Err(failure),
                    }
                }
                other => other,
            };
            let _ = this.update(cx, |m, cx| {
                if m.generation == generation {
                    m.not_interested_answered(&video, was_liked, result, cx);
                }
            });
        })
        .detach();
    }

    fn not_interested_answered(
        &mut self,
        video_id: &str,
        was_liked: bool,
        result: Result<(), WriteFailure>,
        cx: &mut Context<Self>,
    ) {
        self.disliking.remove(video_id);
        match result {
            Ok(()) => {
                self.disliked.insert(video_id.to_owned());
                if let Some(e) = self.likes.get_mut(video_id) {
                    e.shown = false;
                    e.confirmed = Some(false);
                }
                if let Some(cache) = PageCache::try_global(cx) {
                    cache.invalidate_liked_music();
                }
                if was_liked {
                    cx.emit(WritesEvent::LikeSettled {
                        video_id: video_id.to_owned(),
                        liked: false,
                    });
                }
            }
            Err(failure) => {
                if was_liked {
                    if let Some(e) = self.likes.get_mut(video_id) {
                        e.shown = true;
                    }
                    if let Some(cache) = PageCache::try_global(cx) {
                        cache.invalidate_liked_music();
                    }
                }
                self.raise(not_interested_failure_text(failure), cx);
            }
        }
        cx.notify();
    }

    /// A "Not interested" for this song is out.
    pub fn is_disliking(&self, video_id: &str) -> bool {
        self.disliking.contains(video_id)
    }

    /// The service is known to hold a thumbs-down for this song (sent or read back this session).
    pub fn is_disliked(&self, video_id: &str) -> bool {
        self.disliked.contains(video_id)
    }

    // ---- add to a playlist ---------------------------------------------------------------

    /// Add a track to a playlist: shown as held at once, taken back (with an error) if the
    /// service does not take it. The add is a step for Ctrl+Z from the moment it is asked for.
    pub fn add_to_playlist(
        &mut self,
        playlist: &PlaylistTarget,
        video_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.holds(playlist, video_id) {
            crate::confirm::already_in_playlist(&playlist.title, cx);
            return;
        }
        let key = (playlist.id.clone(), video_id.to_owned());
        self.added.insert(key.clone());
        // The playlist's page and the library list (its track count) are about to change.
        if let Some(cache) = PageCache::try_global(cx) {
            cache.invalidate_collection(&playlist.id);
        }
        cx.notify();
        let Some(source) = self.source(cx) else {
            self.add_failed(&key, WriteFailure::Unsupported, cx);
            return;
        };
        let step = self.record_added(playlist, vec![video_id.to_owned()], cx);
        let generation = self.generation;
        let title = playlist.title.clone();
        cx.spawn(async move |this, cx| {
            let (playlist_id, video) = key.clone();
            let result = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move {
                        source.add_to_playlist_rows(&playlist_id, std::slice::from_ref(&video))
                    })
                    .await
            };
            // `Some`: the rows the service named. `None`: the add is known to have landed (read
            // back), but which rows it made is not.
            let result = match result {
                Err(failure) if failure.outcome_unknown() => {
                    // Read it back: the list says which playlists hold the track.
                    let (playlist_id, video) = key.clone();
                    let held = cx
                        .background_executor()
                        .spawn(async move {
                            source.playlists_for(&video).ok().map(|list| {
                                list.iter().any(|p| p.id == playlist_id && p.holds_track)
                            })
                        })
                        .await;
                    match held {
                        Some(true) => Ok(None),
                        _ => Err(failure),
                    }
                }
                other => other.map(Some),
            };
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                // Landed or not, a page fetched while the write was out may predate it.
                if let Some(cache) = PageCache::try_global(cx) {
                    cache.invalidate_collection(&key.0);
                }
                match result {
                    Ok(rows) => {
                        m.log.settle_added(step, rows);
                        cx.emit(WritesEvent::PlaylistAddSettled {
                            playlist_id: key.0.clone(),
                            video_id: key.1.clone(),
                            added: true,
                        });
                        // Said only now: the service has the song.
                        crate::confirm::added_to_playlist(&key.0, &title, 1, cx);
                    }
                    Err(failure) => {
                        m.log.drop_step(step);
                        m.add_failed(&key, failure, cx)
                    }
                }
            });
        })
        .detach();
    }

    /// Add several tracks to a playlist, in order, as one request (the Crate's "Add all to…"):
    /// shown as held at once, taken back (with one error) if the service does not take them.
    /// Tracks the playlist already holds are skipped by the service. One step for Ctrl+Z.
    pub fn add_many_to_playlist(
        &mut self,
        playlist: &PlaylistTarget,
        video_ids: &[String],
        cx: &mut Context<Self>,
    ) {
        // What this call marks as held, so a failure takes back only that.
        let fresh: Vec<(String, String)> = video_ids
            .iter()
            .map(|video| (playlist.id.clone(), video.clone()))
            .filter(|key| self.added.insert(key.clone()))
            .collect();
        if video_ids.is_empty() {
            return;
        }
        if let Some(cache) = PageCache::try_global(cx) {
            cache.invalidate_collection(&playlist.id);
        }
        cx.notify();
        let Some(source) = self.source(cx) else {
            self.add_many_failed(&fresh, WriteFailure::Unsupported, cx);
            return;
        };
        let step = (!fresh.is_empty()).then(|| {
            let videos = fresh.iter().map(|(_, video)| video.clone()).collect();
            self.record_added(playlist, videos, cx)
        });
        let generation = self.generation;
        let playlist_id = playlist.id.clone();
        let title = playlist.title.clone();
        let videos = video_ids.to_vec();
        cx.spawn(async move |this, cx| {
            let result = {
                let (source, playlist_id, videos) =
                    (source.clone(), playlist_id.clone(), videos.clone());
                cx.background_executor()
                    .spawn(async move { source.add_to_playlist_rows(&playlist_id, &videos) })
                    .await
            };
            let result = match result {
                Err(failure) if failure.outcome_unknown() => {
                    // One request carries them all, so the last one being there means they are.
                    let (playlist_id, last) = (playlist_id.clone(), videos.last().cloned());
                    let held = cx
                        .background_executor()
                        .spawn(async move {
                            let last = last?;
                            source.playlists_for(&last).ok().map(|list| {
                                list.iter().any(|p| p.id == playlist_id && p.holds_track)
                            })
                        })
                        .await;
                    match held {
                        Some(true) => Ok(None),
                        _ => Err(failure),
                    }
                }
                other => other.map(Some),
            };
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                if let Some(cache) = PageCache::try_global(cx) {
                    cache.invalidate_collection(&playlist_id);
                }
                match result {
                    Ok(rows) => {
                        if let Some(step) = step {
                            m.log.settle_added(step, rows);
                        }
                        cx.emit(WritesEvent::PlaylistAddedMany {
                            playlist_id: playlist_id.clone(),
                            video_ids: videos.clone(),
                        });
                        // Songs the playlist already held are not counted; none new says so.
                        if fresh.is_empty() {
                            crate::confirm::already_in_playlist(&title, cx);
                        } else {
                            crate::confirm::added_to_playlist(&playlist_id, &title, fresh.len(), cx);
                        }
                    }
                    Err(failure) => {
                        if let Some(step) = step {
                            m.log.drop_step(step);
                        }
                        m.add_many_failed(&fresh, failure, cx)
                    }
                }
            });
        })
        .detach();
    }

    fn add_many_failed(
        &mut self,
        keys: &[(String, String)],
        failure: WriteFailure,
        cx: &mut Context<Self>,
    ) {
        for key in keys {
            self.added.remove(key);
        }
        self.raise(add_failure_text(failure), cx);
        cx.notify();
    }

    fn add_failed(
        &mut self,
        key: &(String, String),
        failure: WriteFailure,
        cx: &mut Context<Self>,
    ) {
        self.added.remove(key);
        cx.emit(WritesEvent::PlaylistAddSettled {
            playlist_id: key.0.clone(),
            video_id: key.1.clone(),
            added: false,
        });
        self.raise(add_failure_text(failure), cx);
        cx.notify();
    }

    // ---- undo ----------------------------------------------------------------------------

    fn record_added(
        &mut self,
        playlist: &PlaylistTarget,
        videos: Vec<String>,
        cx: &gpui::App,
    ) -> u64 {
        self.record_playlist_step(
            PlaylistRef { id: playlist.id.clone(), title: playlist.title.clone() },
            Action::Added { videos, made: Made::Pending },
            cx,
        )
    }

    /// Note a playlist edit for Ctrl+Z; returns the step's id. The queue history's depth is noted
    /// with it, which is how Ctrl+Z knows what came first.
    pub fn record_playlist_step(
        &mut self,
        playlist: PlaylistRef,
        action: Action,
        cx: &gpui::App,
    ) -> u64 {
        let note = self.player.read(cx).queue_undo_depth();
        self.log.record(playlist, action, note)
    }

    /// Where the newest playlist edit that can be undone sits in time (see
    /// `apollo_domain::undo::newest`).
    pub fn playlist_undo_side(&self) -> Option<apollo_domain::undo::Side> {
        self.log.newest_side()
    }

    /// The newest playlist edit is an add the service has not answered yet.
    pub fn newest_edit_pending(&self) -> bool {
        self.log.last().is_some_and(Step::is_pending)
    }

    /// Take the newest playlist edit to undo it.
    pub fn take_playlist_step(&mut self) -> Option<Step> {
        self.log.take_last()
    }

    /// The edit did not happen after all: it is nothing to undo.
    pub fn drop_playlist_step(&mut self, id: u64) {
        self.log.drop_step(id);
    }

    /// Some rows of a removal did not go (a refused write ended it): they are not the step's.
    pub fn keep_removed_except(&mut self, id: u64, not_removed: &[String]) {
        self.log.keep_removed_except(id, not_removed);
    }

    /// Songs taken back out of a playlist are no longer ones this session added to it.
    pub fn forget_added(&mut self, playlist_id: &str, videos: &[String]) {
        for video in videos {
            self.added.remove(&(playlist_id.to_owned(), video.clone()));
        }
    }

    /// The rows the service named for the newest add, for tests.
    pub fn newest_added_rows(&self) -> Option<Vec<AddedRow>> {
        match &self.log.last()?.action {
            Action::Added { made: Made::Rows(rows), .. } => Some(rows.clone()),
            _ => None,
        }
    }

    // ---- notices ------------------------------------------------------------------------

    /// Show a short error (the track menu's radio and collection loads use it too).
    pub fn raise(&mut self, text: &'static str, cx: &mut Context<Self>) {
        let id = self.next_notice;
        self.next_notice += 1;
        self.notice = Some(Notice {
            id,
            text: text.into(),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(NOTICE_FOR).await;
            let _ = this.update(cx, |m, cx| {
                if m.notice.as_ref().is_some_and(|n| n.id == id) {
                    m.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Take the notice away now (it was read, or something else took its place on screen).
    pub fn dismiss_notice(&mut self, cx: &mut Context<Self>) {
        if self.notice.take().is_some() {
            cx.notify();
        }
    }
}

/// What a failed like or unlike says: what happened in six words or fewer.
pub fn like_failure_text(liked: bool, failure: WriteFailure) -> &'static str {
    match failure {
        WriteFailure::Session => "Sign-in expired",
        WriteFailure::Throttled => "Too many requests",
        _ if liked => "Couldn't like this song",
        _ => "Couldn't unlike this song",
    }
}

/// What a "Not interested" says when a like for the same song is still out.
pub const BUSY_TEXT: &str = "Try again in a moment";

/// What a failed "Not interested" says.
pub fn not_interested_failure_text(failure: WriteFailure) -> &'static str {
    match failure {
        WriteFailure::Session => "Sign-in expired",
        WriteFailure::Throttled => "Too many requests",
        _ => "Couldn't send that",
    }
}

/// What a failed add to a playlist says.
pub fn add_failure_text(failure: WriteFailure) -> &'static str {
    match failure {
        WriteFailure::Session => "Sign-in expired",
        WriteFailure::Throttled => "Too many requests",
        _ => "Couldn't add to playlist",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_text_is_six_words_or_fewer() {
        for failure in [
            WriteFailure::Session,
            WriteFailure::Throttled,
            WriteFailure::Rejected,
            WriteFailure::Unknown,
            WriteFailure::Unsupported,
        ] {
            for text in [
                like_failure_text(true, failure),
                like_failure_text(false, failure),
                add_failure_text(failure),
                not_interested_failure_text(failure),
            ] {
                assert!(text.split_whitespace().count() <= 6, "{text}");
            }
        }
    }

    #[test]
    fn a_dead_session_is_named_whatever_the_write_was() {
        assert_eq!(
            like_failure_text(true, WriteFailure::Session),
            "Sign-in expired"
        );
        assert_eq!(add_failure_text(WriteFailure::Session), "Sign-in expired");
    }
}
