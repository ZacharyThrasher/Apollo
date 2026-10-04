//! What an artist page can do beyond listing: Subscribe, Radio, Shuffle, "Show all" and the
//! bio. Lives under the model so it can reach the open page's private state.
//!
//! Subscribe is a write to the real account:
//!
//! * it is sent once and only through the source the current account built, and an answer for a
//!   page that has since been closed (an account change closes them all) is dropped;
//! * a definite refusal puts the button back and raises a short notice;
//! * a write whose outcome is unknown (a timeout, a dropped connection, an unreadable reply) is
//!   never sent again: the artist page is read back and what the service says settles it.

use std::sync::Arc;

use apollo_domain::library::{Collection, CollectionKind, WriteFailure};
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use gpui::Context;

use super::{LibraryModel, Load, PlayRequest};
use crate::library::source::TrackRow;
use crate::page_cache::PageCache;

/// What a mix (a playlist with no seed song) is being fetched for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixKind {
    Radio,
    Shuffle,
}

/// What a radio that could not be fetched says.
pub const RADIO_FAILED: &str = "Couldn't start radio";

/// What a failed subscribe or unsubscribe says: what happened in six words or fewer.
pub fn subscribe_failure_text(subscribed: bool, failure: WriteFailure) -> &'static str {
    match failure {
        WriteFailure::Session => "Sign-in expired",
        WriteFailure::Throttled => "Too many requests",
        _ if subscribed => "Couldn't subscribe",
        _ => "Couldn't unsubscribe",
    }
}

impl LibraryModel {
    /// The artist page on top of the stack, by id.
    fn top_artist_id(&self) -> Option<u64> {
        self.artist().map(|a| a.id)
    }

    /// Play or Shuffle on an artist page. Shuffle plays the artist's own shuffle mix when the
    /// page has one, and otherwise (or when the mix cannot be had) the top songs shuffled.
    pub(super) fn play_artist_request(
        &mut self,
        id: u64,
        request: PlayRequest,
        cx: &mut Context<Self>,
    ) {
        if request.shuffle == Some(true) && request.start.is_none() {
            self.shuffle_artist(id, cx);
        } else {
            self.play_artist(id, request, cx);
        }
    }

    fn shuffle_artist(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        let mix = detail
            .content()
            .and_then(|c| c.shuffle_playlist_id.clone());
        let Some(mix) = mix else {
            self.play_artist(id, PlayRequest::SHUFFLE, cx);
            return;
        };
        if detail.mix.is_some() {
            return;
        }
        detail.mix = Some(MixKind::Shuffle);
        cx.notify();
        self.fetch_mix(id, MixKind::Shuffle, mix, cx);
    }

    /// Start the artist's radio: its tracks replace the queue and play from the first. A page
    /// with no radio of its own seeds one from its first top song instead.
    pub fn start_artist_radio(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.top_artist_id() else {
            return;
        };
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        if detail.mix.is_some() {
            return;
        }
        let Some(content) = detail.content().cloned() else {
            return;
        };
        match (content.radio_playlist_id.clone(), content.top_songs.first()) {
            (Some(mix), _) => {
                detail.mix = Some(MixKind::Radio);
                cx.notify();
                self.fetch_mix(id, MixKind::Radio, mix, cx);
            }
            (None, Some(first)) => {
                detail.mix = Some(MixKind::Radio);
                cx.notify();
                self.seeded_radio(id, first.clone(), cx);
            }
            (None, None) => self.raise(RADIO_FAILED, cx),
        }
    }

    fn fetch_mix(&mut self, id: u64, kind: MixKind, playlist_id: String, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            self.mix_arrived(id, kind, Err(LoadFailure::Other), cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.mix(&playlist_id) })
                .await;
            let _ = this.update(cx, |m, cx| m.mix_arrived(id, kind, result, cx));
        })
        .detach();
    }

    fn mix_arrived(
        &mut self,
        id: u64,
        kind: MixKind,
        result: Result<Vec<TrackRow>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        detail.mix = None;
        cx.notify();
        let rows: Vec<(Track, Option<String>)> = match result {
            Ok(rows) => rows
                .into_iter()
                .map(|r| (r.track, r.thumbnail))
                .collect(),
            Err(_) => Vec::new(),
        };
        match (kind, rows.is_empty()) {
            (MixKind::Radio, false) => {
                // The radio carries on from its first track; the player's own shuffle stays.
                let request = PlayRequest {
                    start: Some(0),
                    shuffle: None,
                };
                self.play_tracks(rows, request, cx);
            }
            (MixKind::Shuffle, false) => self.play_tracks(rows, PlayRequest::SHUFFLE, cx),
            // The mix could not be had: the top songs are a fair shuffle.
            (MixKind::Shuffle, true) => self.play_artist(id, PlayRequest::SHUFFLE, cx),
            (MixKind::Radio, true) => self.raise(RADIO_FAILED, cx),
        }
    }

    /// A radio for a page without one: the first top song plays now and its radio replaces the
    /// rest of the queue when it arrives. A radio that cannot be fetched leaves the queue as it
    /// was.
    fn seeded_radio(&mut self, id: u64, seed: TrackRow, cx: &mut Context<Self>) {
        let Some(source) = self.source.clone() else {
            if let Some(detail) = self.artist_mut(id) {
                detail.mix = None;
            }
            self.raise(RADIO_FAILED, cx);
            return;
        };
        let video = seed.track.video_id.as_str().to_owned();
        self.player.update(cx, |p, cx| {
            p.hint_art(&video, seed.thumbnail.as_deref());
            p.start_radio(seed.track, cx);
        });
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { source.radio(&video) })
                .await;
            let _ = this.update(cx, |m, cx| {
                let Some(detail) = m.artist_mut(id) else {
                    return;
                };
                detail.mix = None;
                match result {
                    Ok(rows) if !rows.is_empty() => {
                        let tracks: Vec<Track> = rows
                            .into_iter()
                            .map(|r| {
                                m.player.update(cx, |p, _| {
                                    p.hint_art(r.track.video_id.as_str(), r.thumbnail.as_deref())
                                });
                                r.track
                            })
                            .collect();
                        m.player.update(cx, |p, cx| p.finish_radio(tracks, cx));
                    }
                    _ => m.raise(RADIO_FAILED, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn raise(&mut self, text: &'static str, cx: &mut Context<Self>) {
        self.writes.update(cx, |w, cx| w.raise(text, cx));
    }

    // ---- subscribe -------------------------------------------------------------------------

    /// The Subscribe button was pressed: show the opposite state at once and ask the service.
    /// Ignored while a request for this page is out, so two can never reorder.
    pub fn toggle_subscribe(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.top_artist_id() else {
            return;
        };
        let Some(source) = self.source.clone() else {
            return;
        };
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        if detail.subscribing.is_some() {
            return;
        }
        let Some(content) = detail.content() else {
            return;
        };
        if content.channel_id.is_empty() {
            return;
        }
        let want = !content.subscribed;
        let channel = content.channel_id.clone();
        let page = detail.target.id.clone();
        detail.subscribing = Some(want);
        // The page (its button) and the Artists tab are about to change: a cached copy must not
        // be drawn meanwhile, and again once the write settles.
        let changed = page.clone();
        if let Some(cache) = PageCache::try_global(cx) {
            cache.invalidate_subscription(&changed);
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            let sent = {
                let (source, channel) = (source.clone(), channel.clone());
                cx.background_executor()
                    .spawn(async move { source.set_subscribed(&channel, want) })
                    .await
            };
            let result = match sent {
                // Unknown: never send it again. Ask what the account holds.
                Err(failure) if failure.outcome_unknown() => {
                    let held = cx
                        .background_executor()
                        .spawn(async move { source.artist(&page) })
                        .await;
                    match held {
                        Ok(content) if content.subscribed == want => Ok(()),
                        _ => Err(failure),
                    }
                }
                other => other,
            };
            let _ = this.update(cx, |m, cx| {
                if let Some(cache) = PageCache::try_global(cx) {
                    cache.invalidate_subscription(&changed);
                }
                m.subscription_settled(id, want, result, cx)
            });
        })
        .detach();
    }

    fn subscription_settled(
        &mut self,
        id: u64,
        want: bool,
        result: Result<(), WriteFailure>,
        cx: &mut Context<Self>,
    ) {
        // Gone with its page: an account change closes every page, and nothing here applies to
        // the account that replaced it.
        let Some(detail) = self.artist_mut(id) else {
            return;
        };
        detail.subscribing = None;
        match result {
            Ok(()) => {
                if let Load::Ready(content) = &mut detail.state {
                    Arc::make_mut(content).subscribed = want;
                }
            }
            Err(failure) => self.raise(subscribe_failure_text(want, failure), cx),
        }
        cx.notify();
    }

    // ---- the rest of the page ---------------------------------------------------------------

    /// "Show all" on the Top songs shelf: the playlist of every top song. The view opens it over
    /// this page, so focus and scroll state follow like any other page.
    pub fn top_songs_collection(&self) -> Option<Collection> {
        let content = self.artist()?.content()?;
        Some(Collection {
            kind: CollectionKind::Playlist,
            id: content.top_songs_id.clone()?,
            title: "Top songs".into(),
            subtitle: Some(content.name.clone()),
            thumbnail: None,
        })
    }

    /// Opens or clamps the bio.
    pub fn toggle_bio(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.top_artist_id() {
            if let Some(detail) = self.artist_mut(id) {
                detail.bio_open = !detail.bio_open;
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_subscribe_failure_text_is_six_words_or_fewer() {
        for failure in [
            WriteFailure::Session,
            WriteFailure::Throttled,
            WriteFailure::Rejected,
            WriteFailure::Unknown,
            WriteFailure::Unsupported,
        ] {
            for on in [true, false] {
                assert!(subscribe_failure_text(on, failure).split_whitespace().count() <= 6);
            }
        }
    }

    #[test]
    fn the_failure_names_the_direction_unless_the_session_is_the_problem() {
        assert_eq!(
            subscribe_failure_text(true, WriteFailure::Rejected),
            "Couldn't subscribe"
        );
        assert_eq!(
            subscribe_failure_text(false, WriteFailure::Unknown),
            "Couldn't unsubscribe"
        );
        assert_eq!(
            subscribe_failure_text(true, WriteFailure::Session),
            "Sign-in expired"
        );
    }
}
