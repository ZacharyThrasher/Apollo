use std::sync::Arc;

use apollo_domain::account::{Validation, ValidationError};
use apollo_domain::library::{Collection, CollectionKind, PlaylistTarget, WriteFailure};
use apollo_domain::listing::{LoadFailure, Page};
use innertube::{Clients, InnerTube, Rating, Session, YouTubeClient, METADATA_CLIENT};
use tokio::sync::Notify;

use crate::artist::{page_from_artist, ArtistPage};
use crate::home::{home_page, HomePage};
use crate::map::{
    account_validation, classify, classify_write, collections_from_grid, history_days,
    liked_from_rating, page_from_album, page_from_continuation, page_from_playlist, target_from_option, track_from_song,
    HistoryDay, ListedTrack,
};
use crate::search::{hit_from_item, search_results, Hit, SearchResults};

/// Network-backed metadata source. Every call is authenticated: there is no guest mode, and a
/// rejected session is reported to the caller instead of being retried or healed here.
#[derive(Clone)]
pub struct InnerTubeSource {
    tube: InnerTube,
    client: YouTubeClient,
}

impl InnerTubeSource {
    /// A signed-in source from the cookie header for `music.youtube.com` and, for a non-default
    /// channel, its delegation id. The cookie is a credential: it is never logged here.
    pub fn signed_in(
        cookie_header: String,
        data_sync_id: Option<String>,
    ) -> Result<Self, innertube::Error> {
        Self::new(
            Session {
                cookie: Some(cookie_header),
                data_sync_id,
                ..Session::default()
            },
            None,
        )
    }

    pub fn new(session: Session, proxy: Option<&str>) -> Result<Self, innertube::Error> {
        let client = Clients::bundled()
            .get(METADATA_CLIENT)
            .cloned()
            .ok_or_else(|| innertube::Error::Other("metadata client missing".into()))?;
        Ok(Self {
            tube: InnerTube::new(session, proxy)?,
            client,
        })
    }

    /// Bootstraps the anonymous `visitorData` the service expects on requests, once. A failure
    /// is not an error: signed-in requests are identified by the session either way.
    pub async fn ensure_visitor_data(&self) {
        if self.tube.visitor_data().is_none() {
            if let Ok(visitor) = self.tube.fetch_visitor_data().await {
                self.tube.set_visitor_data(Some(visitor));
            }
        }
    }

    /// Raised when a signed-in request is refused (401/403 or a logged-out payload). Feed it to
    /// the account model as `AuthorizationRejected` for the current epoch.
    pub fn session_rejected(&self) -> Arc<Notify> {
        self.tube.session_rejected()
    }

    /// Raised when a response rotated the stored cookies; read [`Self::cookie`] and write it to
    /// the credential vault. The value is a secret: never log it.
    pub fn cookie_changed(&self) -> Arc<Notify> {
        self.tube.cookie_changed()
    }

    pub fn cookie(&self) -> Option<String> {
        self.tube.cookie()
    }

    /// Point the session at a different cookie header (the account's jar changed) and
    /// delegation id, keeping the connection pool.
    pub fn set_session(&self, cookie_header: String, data_sync_id: Option<String>) {
        self.tube.set_cookie(Some(cookie_header));
        self.tube.set_data_sync_id(data_sync_id);
    }

    /// The account's library playlists (every page of the grid, deduplicated by the client).
    pub async fn library_playlists(&self) -> Result<Vec<Collection>, LoadFailure> {
        let cards = self
            .tube
            .library_playlists(&self.client)
            .await
            .map_err(|e| classify(&e))?;
        Ok(collections_from_grid(&cards, CollectionKind::Playlist))
    }

    /// The albums saved to the account's library.
    pub async fn library_albums(&self) -> Result<Vec<Collection>, LoadFailure> {
        let cards = self
            .tube
            .library_albums(&self.client)
            .await
            .map_err(|e| classify(&e))?;
        Ok(collections_from_grid(&cards, CollectionKind::Album))
    }

    /// The artists behind the songs and albums in the account's library (the Library's Artists
    /// tab), in the service's order. A card with no id or title is dropped.
    pub async fn library_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        let cards = self
            .tube
            .library_artists(&self.client)
            .await
            .map_err(|e| classify(&e))?;
        Ok(cards
            .iter()
            .filter(|c| c.kind == "artist")
            .filter_map(hit_from_item)
            .collect())
    }

    /// The play history in YouTube Music's own date buckets (the Library's History tab).
    pub async fn history(&self) -> Result<Vec<HistoryDay>, LoadFailure> {
        let groups = self
            .tube
            .history(&self.client)
            .await
            .map_err(|e| classify(&e))?;
        Ok(history_days(&groups))
    }

    /// First page of a playlist (`VL…`) or any page that lists tracks. A bare playlist id
    /// (`PL…`) gets the `VL` the browse endpoint wants.
    pub async fn playlist(&self, browse_id: &str) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .playlist(&self.client, &playlist_browse_id(browse_id), None)
            .await
            .map_err(|e| classify(&e))?;
        Ok(page_from_playlist(&page))
    }

    pub async fn album(&self, browse_id: &str) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .album(&self.client, browse_id)
            .await
            .map_err(|e| classify(&e))?;
        Ok(page_from_album(&page))
    }

    /// An artist page by channel browse id (`UC…`): top songs and the album rows.
    pub async fn artist(&self, browse_id: &str) -> Result<ArtistPage, LoadFailure> {
        let page = self
            .tube
            .artist(&self.client, browse_id)
            .await
            .map_err(|e| classify(&e))?;
        page_from_artist(&page, browse_id)
    }

    /// The account's home feed: its first batch of shelves and the token for the next.
    pub async fn home(&self) -> Result<HomePage, LoadFailure> {
        let parsed = self
            .tube
            .home(&self.client, None)
            .await
            .map_err(|e| classify(&e))?;
        home_page(&parsed)
    }

    /// The batch of home shelves after the one a token came from.
    pub async fn home_continuation(&self, token: &str) -> Result<HomePage, LoadFailure> {
        let parsed = self
            .tube
            .home_continuation(&self.client, token)
            .await
            .map_err(|e| classify(&e))?;
        home_page(&parsed)
    }

    pub async fn continuation(&self, token: &str) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .playlist_continuation(&self.client, token)
            .await
            .map_err(|e| classify(&e))?;
        Ok(page_from_continuation(&page))
    }

    /// Unfiltered search, grouped by kind. Carries the account.
    ///
    /// Checked against the live service (2026-10-03): an anonymous search answers a different
    /// mix (videos, episodes, channels, no albums) and an anonymous song or album search answers
    /// "No results", so a usable search needs the session. The service writes a signed-in search
    /// to the account's search history, which is why callers debounce typing and skip very
    /// short queries. Rows that came back but could not be read, with nothing readable beside
    /// them, are a `Parse` failure, not an empty result.
    pub async fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        let parsed = self
            .tube
            .search_all(&self.client, query, true)
            .await
            .map_err(|e| classify(&e))?;
        let results = search_results(&parsed);
        if results.is_empty() && results.skipped > 0 {
            return Err(LoadFailure::Parse);
        }
        Ok(results)
    }

    /// Like or unlike a track on the account (`like/like`, `like/removelike`). Idempotent. A
    /// failure says whether the write is known not to have happened or might have: see
    /// [`WriteFailure::outcome_unknown`].
    pub async fn set_liked(&self, video_id: &str, liked: bool) -> Result<(), WriteFailure> {
        let rating = if liked {
            Rating::Like
        } else {
            Rating::Indifferent
        };
        self.tube
            .rate(&self.client, video_id, rating)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Whether the account has liked a track: what YouTube Music's own player bar asks (a `next`
    /// for the video, whose overlay carries the rating). `None` when the answer did not say.
    pub async fn liked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        let next = self
            .tube
            .next(&self.client, Some(video_id), None)
            .await
            .map_err(|e| classify(&e))?;
        Ok(next.rating.map(liked_from_rating))
    }

    /// The tracks YouTube Music's own autoplay would play after `video_id`: the song radio
    /// (`RDAMVM<id>`), signed in, so it follows the account's taste. The seed itself and
    /// repeats are left out. A radio YouTube reports as dead (the panel holds only the seed and
    /// names the mix the song really belongs to) is asked for again through that mix.
    pub async fn radio(&self, video_id: &str) -> Result<Vec<ListedTrack>, LoadFailure> {
        let mix = format!("RDAMVM{video_id}");
        let mut next = self
            .tube
            .next(&self.client, Some(video_id), Some(&mix))
            .await
            .map_err(|e| classify(&e))?;
        if next.items.iter().all(|s| s.video_id == video_id) {
            if let Some(automix) = next.automix_playlist_id.take() {
                next = self
                    .tube
                    .next(&self.client, Some(video_id), Some(&automix))
                    .await
                    .map_err(|e| classify(&e))?;
            }
        }
        Ok(radio_tracks(&next.items, video_id))
    }

    /// The playlists the account can add `video_id` to, each flagged when it already holds it.
    pub async fn playlists_for(&self, video_id: &str) -> Result<Vec<PlaylistTarget>, LoadFailure> {
        let options = self
            .tube
            .add_to_playlist_options(&self.client, video_id)
            .await
            .map_err(|e| classify(&e))?;
        Ok(options.iter().map(target_from_option).collect())
    }

    /// Add one track to a playlist the account owns. A track already there is skipped, not added
    /// twice. `playlist_id` may carry the `VL` browse prefix.
    pub async fn add_to_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
    ) -> Result<(), WriteFailure> {
        let bare = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
        self.tube
            .add_to_playlist(&self.client, bare, video_id)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// YouTube Music's own lyrics for a track, as plain text. `Ok(None)` is an answer: the track
    /// has no lyrics tab, or the tab is empty.
    pub async fn lyrics_text(&self, video_id: &str) -> Result<Option<String>, LoadFailure> {
        self.tube
            .lyrics(&self.client, video_id)
            .await
            .map_err(|e| classify(&e))
    }

    /// How loud YouTube measured a track (`loudnessDb`, dB against its reference level), from one
    /// metadata-only `player` request. `Ok(None)` is an answer: the service had no measurement for
    /// the track. Nothing else in the response is read, and it carries no plays or history.
    pub async fn loudness_db(&self, video_id: &str) -> Result<Option<f32>, LoadFailure> {
        let loudness = self
            .tube
            .player_loudness(&self.client, video_id)
            .await
            .map_err(|e| classify(&e))?;
        Ok(loudness.loudness_db)
    }

    /// Best-effort `visitorData`, which the service expects alongside the cookie. A failure
    /// leaves the session without one; requests then report whatever the service says about it.
    pub async fn bootstrap_visitor_data(&self) {
        if self.tube.visitor_data().is_some() {
            return;
        }
        if let Ok(vd) = self.tube.fetch_visitor_data().await {
            self.tube.set_visitor_data(Some(vd));
        }
    }

    /// The account's own validation: the gate for playback access. Resolver success proves
    /// nothing about the account, so this is the only thing that may mark the session valid.
    pub async fn validate_account(&self) -> Result<Validation, ValidationError> {
        let menu = self
            .tube
            .account_menu(&self.client)
            .await
            .map_err(to_validation_error)?;
        let identities = match self.tube.account_identities(&self.client).await {
            Ok(list) => list,
            Err(innertube::Error::SessionExpired) => return Err(ValidationError::Expired),
            // The menu already proved the session; a failed sibling call only costs the
            // multi-identity list.
            Err(_) => Vec::new(),
        };
        account_validation(&menu, &identities)
    }
}

/// The browse id a playlist page is fetched by: the playlist id with a `VL` prefix. Library and
/// search cards already carry it; a bare id (a pasted link, a radio id) gets it here.
pub(crate) fn playlist_browse_id(id: &str) -> String {
    if id.starts_with("VL") {
        id.to_owned()
    } else {
        format!("VL{id}")
    }
}

/// A radio panel as the tracks to queue: playable rows only, without the seed and without a
/// video that appears twice.
pub(crate) fn radio_tracks(items: &[innertube::SongItem], seed: &str) -> Vec<ListedTrack> {
    let mut seen = std::collections::HashSet::new();
    seen.insert(seed.to_owned());
    items
        .iter()
        .filter_map(track_from_song)
        .filter(|listed| seen.insert(listed.track.video_id.as_str().to_owned()))
        .collect()
}

fn to_validation_error(err: innertube::Error) -> ValidationError {
    match classify(&err) {
        LoadFailure::Session => ValidationError::Expired,
        _ => ValidationError::Unreachable,
    }
}

#[cfg(test)]
mod tests {
    use super::{playlist_browse_id, radio_tracks};
    use innertube::SongItem;

    fn song(id: &str, title: &str) -> SongItem {
        SongItem {
            video_id: id.into(),
            title: title.into(),
            artists: "Artist".into(),
            ..SongItem::default()
        }
    }

    #[test]
    fn a_radio_drops_the_seed_repeats_and_unplayable_rows() {
        let items = [
            song("seed0000001", "Seed"),
            song("next0000001", "One"),
            song("next0000002", "Two"),
            song("next0000001", "One again"),
            song("", "No id"),
        ];
        let ids: Vec<String> = radio_tracks(&items, "seed0000001")
            .into_iter()
            .map(|l| l.track.video_id.as_str().to_owned())
            .collect();
        assert_eq!(ids, ["next0000001", "next0000002"]);
    }

    #[test]
    fn a_bare_playlist_id_gets_the_browse_prefix_once() {
        assert_eq!(playlist_browse_id("PLabc"), "VLPLabc");
        assert_eq!(playlist_browse_id("VLPLabc"), "VLPLabc");
        assert_eq!(playlist_browse_id("OLAK5uy_x"), "VLOLAK5uy_x");
    }
}
