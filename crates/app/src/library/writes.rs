//! Library writes: liking and unliking tracks and adding a track to a playlist.
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

use apollo_domain::library::{PlaylistTarget, WriteFailure};
use gpui::{Context, Entity, EventEmitter, SharedString};

use super::source::LibrarySource;
use crate::page_cache::PageCache;
use crate::player::PlayerModel;

/// How long an error stays on screen.
pub const NOTICE_FOR: Duration = Duration::from_secs(4);

/// What the screens learn from the writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WritesEvent {
    /// The service holds this like state for the track now (a write landed, or a read-back
    /// showed that one had). Liked Music follows these.
    LikeSettled { video_id: String, liked: bool },
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
    /// `(playlist id, video id)` pairs added here: shown as held right away, taken back on failure.
    added: HashSet<(String, String)>,
    /// The current track last asked about, so each track is asked once.
    checked: Option<String>,
    /// Bumped when the account changes; older answers are dropped.
    generation: u64,
    notice: Option<Notice>,
    next_notice: u64,
}

impl EventEmitter<WritesEvent> for LibraryWrites {}

impl LibraryWrites {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        // The playing track's state is fetched when it changes (and once the backend is there).
        cx.observe(&player, |m, _, cx| m.check_current(cx)).detach();
        Self {
            player,
            likes: HashMap::new(),
            added: HashSet::new(),
            checked: None,
            generation: 0,
            notice: None,
            next_notice: 1,
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
        self.likes.clear();
        self.added.clear();
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

    // ---- add to a playlist ---------------------------------------------------------------

    /// Add a track to a playlist: shown as held at once, taken back (with an error) if the
    /// service does not take it.
    pub fn add_to_playlist(
        &mut self,
        playlist: &PlaylistTarget,
        video_id: &str,
        cx: &mut Context<Self>,
    ) {
        if self.holds(playlist, video_id) {
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
        let generation = self.generation;
        cx.spawn(async move |this, cx| {
            let (playlist_id, video) = key.clone();
            let result = {
                let source = source.clone();
                cx.background_executor()
                    .spawn(async move { source.add_to_playlist(&playlist_id, &video) })
                    .await
            };
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
                        Some(true) => Ok(()),
                        _ => Err(failure),
                    }
                }
                other => other,
            };
            let _ = this.update(cx, |m, cx| {
                if m.generation != generation {
                    return;
                }
                // Landed or not, a page fetched while the write was out may predate it.
                if let Some(cache) = PageCache::try_global(cx) {
                    cache.invalidate_collection(&key.0);
                }
                if let Err(failure) = result {
                    m.add_failed(&key, failure, cx);
                }
            });
        })
        .detach();
    }

    fn add_failed(
        &mut self,
        key: &(String, String),
        failure: WriteFailure,
        cx: &mut Context<Self>,
    ) {
        self.added.remove(key);
        self.raise(add_failure_text(failure), cx);
        cx.notify();
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
