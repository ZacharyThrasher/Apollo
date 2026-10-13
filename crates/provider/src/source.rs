use std::sync::Arc;

use apollo_domain::account::{Validation, ValidationError};
use apollo_domain::library::{
    bare_playlist_id, AddedRow, Collection, CollectionKind, PageMeta, PlaylistEdit, PlaylistTarget, Privacy,
    WriteFailure,
};
use apollo_domain::links::ExternalLink;
use apollo_domain::listing::{LoadFailure, Page};
use apollo_domain::model::BrowseId;
use innertube::endpoints::RowEdit;
use innertube::{
    AddedVideo, Clients, InnerTube, PlaybackTracking, Rating, Session, WatchtimePing, YouTubeClient,
    METADATA_CLIENT,
};
use tokio::sync::Notify;

use crate::artist::{page_from_artist, ArtistPage};
use crate::explore::{explore_page, mood_playlists, worst, ExplorePage};
use crate::home::{home_page, HomePage};
use crate::marks::LibraryMarks;
use crate::map::{
    account_validation, classify, classify_write, collections_from_grid, history_days,
    liked_from_rating, meta_from_album, meta_from_playlist, page_from_album, page_from_continuation,
    page_from_playlist, target_from_option, track_from_song, HistoryDay, ListedTrack,
};
use crate::search::{
    hit_from_item, search_page, search_results, Hit, SearchFilter, SearchPage, SearchResults,
};

/// The rows the service says an add made, as the app's own type.
fn added_rows(rows: Vec<AddedVideo>) -> Vec<AddedRow> {
    rows.into_iter()
        .map(|r| AddedRow { video_id: r.video_id, set_video_id: r.set_video_id })
        .collect()
}

/// The browse id of Library > Songs: the songs saved to the library, apart from Liked Music.
const LIBRARY_SONGS_BROWSE_ID: &str = "FEmusic_liked_videos";

/// What a pasted link points at, with enough about it to open it without a second lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Track(ListedTrack),
    Collection(Collection),
    Artist {
        id: BrowseId,
        name: String,
        thumbnail: Option<String>,
    },
}

/// Network-backed metadata source. Every call is authenticated: there is no guest mode, and a
/// rejected session is reported to the caller instead of being retried or healed here.
#[derive(Clone)]
pub struct InnerTubeSource {
    tube: InnerTube,
    client: YouTubeClient,
    /// What rows said about the library (shared with every source built from the same marks).
    marks: Arc<LibraryMarks>,
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
            marks: Arc::new(LibraryMarks::default()),
        })
    }

    /// Note library tokens in `marks` instead of a store of this source's own, so a source made
    /// for search and one made for the library see the same songs.
    pub fn with_marks(mut self, marks: Arc<LibraryMarks>) -> Self {
        self.marks = marks;
        self
    }

    /// Whether the library holds a song, as the last row of it said, when that row also gave the
    /// means to change it.
    pub fn song_saved(&self, video_id: &str) -> Option<bool> {
        self.marks.state(video_id)
    }

    /// Forget every library token (the account changed).
    pub fn forget_library_marks(&self) {
        self.marks.clear();
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

    fn note_sections(&self, sections: &[innertube::Section]) {
        for section in sections {
            self.marks.note_items(&section.items);
        }
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

    /// The artists the account subscribes to, in the service's order. Unlike
    /// [`library_artists`](Self::library_artists) (the artists behind the saved songs), these
    /// are the ones the account chose to follow. A card with no id or title is dropped.
    pub async fn subscribed_artists(&self) -> Result<Vec<Hit>, LoadFailure> {
        let cards = self
            .tube
            .subscribed_artists(&self.client)
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
        for group in &groups {
            self.marks.note_songs(&group.items);
        }
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
        self.marks.note_songs(&page.items);
        Ok(page_from_playlist(&page))
    }

    /// The songs saved to the library, first page (Library > Songs): a plain list of rows with a
    /// paging token, read through [`Self::continuation`] like a playlist's.
    pub async fn library_songs(&self) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .playlist(&self.client, LIBRARY_SONGS_BROWSE_ID, None)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&page.items);
        Ok(page_from_playlist(&page))
    }

    /// [`Self::playlist`] with what the page says about editing and saving it.
    pub async fn playlist_with_meta(
        &self,
        browse_id: &str,
    ) -> Result<(Page<ListedTrack>, PageMeta), LoadFailure> {
        let browse = playlist_browse_id(browse_id);
        let page = self
            .tube
            .playlist(&self.client, &browse, None)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&page.items);
        Ok((page_from_playlist(&page), meta_from_playlist(&page, &browse)))
    }

    /// [`Self::album`] with the audio playlist the save is made on and the saved flag.
    pub async fn album_with_meta(
        &self,
        browse_id: &str,
    ) -> Result<(Page<ListedTrack>, PageMeta), LoadFailure> {
        let page = self
            .tube
            .album(&self.client, browse_id)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&page.items);
        Ok((page_from_album(&page), meta_from_album(&page)))
    }

    pub async fn album(&self, browse_id: &str) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .album(&self.client, browse_id)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&page.items);
        Ok(page_from_album(&page))
    }

    /// An artist page by channel browse id (`UC…`): top songs and the album rows.
    pub async fn artist(&self, browse_id: &str) -> Result<ArtistPage, LoadFailure> {
        let page = self
            .tube
            .artist(&self.client, browse_id)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&page.top_songs);
        page_from_artist(&page, browse_id)
    }

    /// The account's home feed: its first batch of shelves and the token for the next.
    pub async fn home(&self) -> Result<HomePage, LoadFailure> {
        let parsed = self
            .tube
            .home(&self.client, None)
            .await
            .map_err(|e| classify(&e))?;
        self.note_sections(&parsed.sections);
        home_page(&parsed)
    }

    /// The home feed filtered to one mood chip (`params` is a chip's token from an earlier home
    /// answer): the same shelves as the feed, for that mood.
    pub async fn home_chip(&self, params: &str) -> Result<HomePage, LoadFailure> {
        let parsed = self
            .tube
            .home(&self.client, Some(params))
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
        self.note_sections(&parsed.sections);
        home_page(&parsed)
    }

    /// The Related tab of a track: related songs, then album, artist and playlist shelves.
    /// Read-only. A track YouTube gives no related tab for comes back with no shelves.
    pub async fn related(&self, video_id: &str) -> Result<HomePage, LoadFailure> {
        let parsed = self
            .tube
            .related(&self.client, video_id)
            .await
            .map_err(|e| classify(&e))?;
        self.note_sections(&parsed.sections);
        home_page(&parsed)
    }

    /// Explore: new releases, the song chart, chart shelves and the mood tiles. The four answers
    /// it is made of are asked for together and any of them may fail or be refused (some are
    /// not offered to every account or country); the page shows what came back. It fails only
    /// when nothing could be shown, naming the worst failure, and a rejected session fails it
    /// at once.
    pub async fn explore(&self) -> Result<ExplorePage, LoadFailure> {
        let (explore, charts, releases, moods) = tokio::join!(
            self.tube.explore(&self.client),
            self.tube.charts(&self.client),
            self.tube.new_releases(&self.client),
            self.tube.moods(&self.client),
        );
        let failures: Vec<LoadFailure> = [
            explore.as_ref().err(),
            charts.as_ref().err(),
            releases.as_ref().err(),
            moods.as_ref().err(),
        ]
        .into_iter()
        .flatten()
        .map(classify)
        .collect();
        if failures.contains(&LoadFailure::Session) {
            return Err(LoadFailure::Session);
        }
        let page = explore_page(
            explore.as_ref().ok(),
            charts.as_ref().ok(),
            releases.as_deref().ok(),
            moods.as_deref().ok(),
        );
        match worst(failures) {
            Some(failure) if page.is_empty() => Err(failure),
            _ => Ok(page),
        }
    }

    /// The playlists of one mood or genre tile (`params` from the Explore page's tiles).
    pub async fn mood_playlists(&self, params: &str) -> Result<Vec<Collection>, LoadFailure> {
        let cards = self
            .tube
            .mood_playlists(&self.client, params)
            .await
            .map_err(|e| classify(&e))?;
        Ok(mood_playlists(&cards))
    }

    pub async fn continuation(&self, token: &str) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .playlist_continuation(&self.client, token)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&page.items);
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
        self.marks.note_items(&parsed.top);
        self.marks.note_items(&parsed.songs);
        let results = search_results(&parsed);
        if results.is_empty() && results.skipped > 0 {
            return Err(LoadFailure::Parse);
        }
        Ok(results)
    }

    /// First page of one kind of result (the service's own search tabs). Carries the account,
    /// like [`Self::search`]; an anonymous filtered search answers "No results".
    pub async fn search_filtered(
        &self,
        query: &str,
        filter: SearchFilter,
    ) -> Result<SearchPage, LoadFailure> {
        let parsed = self
            .tube
            .search_filtered(&self.client, query, filter.params())
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_items(&parsed.items);
        let page = search_page(&parsed);
        if page.results.is_empty() && page.results.skipped > 0 {
            return Err(LoadFailure::Parse);
        }
        Ok(page)
    }

    /// The page after one [`Self::search_filtered`] answered, by its token.
    pub async fn search_more(&self, token: &str) -> Result<SearchPage, LoadFailure> {
        let parsed = self
            .tube
            .search_continuation(&self.client, token)
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_items(&parsed.items);
        Ok(search_page(&parsed))
    }

    /// Typeahead for the search box: the query completions the service offers for `input`, in
    /// its own order. Not written to the account's search history (unlike a search).
    pub async fn suggestions(&self, input: &str) -> Result<Vec<String>, LoadFailure> {
        let parsed = self
            .tube
            .search_suggestions(&self.client, input)
            .await
            .map_err(|e| classify(&e))?;
        Ok(parsed.queries.into_iter().map(|q| q.text).collect())
    }

    /// What a pasted link opens: the track (looked up through the panel `next` answers for it),
    /// or the title and cover of the playlist, album or artist page it names.
    pub async fn resolve_link(&self, link: &ExternalLink) -> Result<LinkTarget, LoadFailure> {
        match link {
            ExternalLink::Track { video, .. } => {
                let next = self
                    .tube
                    .next(&self.client, Some(video.as_str()), None)
                    .await
                    .map_err(|e| classify(&e))?;
                next.items
                    .iter()
                    .filter(|s| s.video_id == video.as_str())
                    .find_map(track_from_song)
                    .map(LinkTarget::Track)
                    .ok_or(LoadFailure::Parse)
            }
            ExternalLink::Playlist(id) => {
                let browse_id = playlist_browse_id(id.as_str());
                let page = self
                    .tube
                    .playlist(&self.client, &browse_id, None)
                    .await
                    .map_err(|e| classify(&e))?;
                Ok(LinkTarget::Collection(Collection {
                    kind: CollectionKind::Playlist,
                    id: BrowseId::new(browse_id),
                    title: page.title.unwrap_or_else(|| "Playlist".to_owned()),
                    subtitle: page.subtitle,
                    thumbnail: page.thumbnail,
                }))
            }
            ExternalLink::Album(id) => {
                let page = self
                    .tube
                    .album(&self.client, id.as_str())
                    .await
                    .map_err(|e| classify(&e))?;
                Ok(LinkTarget::Collection(Collection {
                    kind: CollectionKind::Album,
                    id: id.clone(),
                    title: page.title.ok_or(LoadFailure::Parse)?,
                    subtitle: page.subtitle,
                    thumbnail: page.thumbnail,
                }))
            }
            ExternalLink::Artist(id) => {
                let page = self.artist(id.as_str()).await?;
                Ok(LinkTarget::Artist {
                    id: page.id,
                    name: page.name,
                    thumbnail: page.thumbnail,
                })
            }
        }
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

    /// Tell YouTube Music the account is not interested in a track (`like/dislike`, the
    /// thumbs-down). Sent once, never retried: when the outcome is unknown the caller reads the
    /// rating back with [`Self::disliked`]. A liked track stops being liked by the same call.
    pub async fn set_disliked(&self, video_id: &str) -> Result<(), WriteFailure> {
        self.tube
            .rate_once(&self.client, video_id, Rating::Dislike)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Whether the account has thumbs-downed a track (a `next` for the video, whose overlay
    /// carries the rating). `None` when the answer did not say.
    pub async fn disliked(&self, video_id: &str) -> Result<Option<bool>, LoadFailure> {
        let next = self
            .tube
            .next(&self.client, Some(video_id), None)
            .await
            .map_err(|e| classify(&e))?;
        Ok(next.rating.map(|r| r == Rating::Dislike))
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
        self.marks.note_songs(&next.items);
        Ok(radio_tracks(&next.items, video_id))
    }

    /// The tracks of a radio or mix playlist that has no seed song (`RDEM…` an artist's radio,
    /// `RDAO…` its shuffle mix), as the `next` panel lists them. Repeats and unplayable rows are
    /// left out.
    pub async fn mix(&self, playlist_id: &str) -> Result<Vec<ListedTrack>, LoadFailure> {
        let next = self
            .tube
            .next(&self.client, None, Some(playlist_id))
            .await
            .map_err(|e| classify(&e))?;
        self.marks.note_songs(&next.items);
        Ok(radio_tracks(&next.items, ""))
    }

    /// Subscribe to or unsubscribe from an artist's channel (`UC…`) as the signed-in account.
    /// Sent once: a failure that says [`WriteFailure::outcome_unknown`] means the account may or
    /// may not have changed, and the caller reads the artist page back to find out.
    pub async fn set_subscribed(
        &self,
        channel_id: &str,
        subscribed: bool,
    ) -> Result<(), WriteFailure> {
        self.tube
            .subscribe(&self.client, channel_id, subscribed)
            .await
            .map_err(|e| classify_write(&e))
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
    /// twice. `playlist_id` may carry the `VL` browse prefix. The row the service says it made
    /// comes back (none for a skipped duplicate, or an answer that does not name it).
    pub async fn add_to_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        let bare = playlist_id.strip_prefix("VL").unwrap_or(playlist_id);
        self.tube
            .add_to_playlist(&self.client, bare, video_id)
            .await
            .map(added_rows)
            .map_err(|e| classify_write(&e))
    }

    /// Add several tracks to a playlist the account owns in one request, in the order given.
    /// Tracks already there are skipped. `playlist_id` may carry the `VL` browse prefix. The rows
    /// the service says it made come back.
    pub async fn add_many_to_playlist(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        self.tube
            .add_many_to_playlist(&self.client, bare_playlist_id(playlist_id), video_ids)
            .await
            .map(added_rows)
            .map_err(|e| classify_write(&e))
    }

    /// Add tracks to a playlist the account owns even when it holds them already (a removal being
    /// undone). Sent once: it is not safe to repeat, so a lost reply is for the caller to settle
    /// by reading the playlist. The rows the service says it made come back.
    pub async fn add_again_to_playlist(
        &self,
        playlist_id: &str,
        video_ids: &[String],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        self.tube
            .add_again_to_playlist(&self.client, bare_playlist_id(playlist_id), video_ids)
            .await
            .map(added_rows)
            .map_err(|e| classify_write(&e))
    }

    /// The playlists the account can edit, as bare ids: the "Save to playlist" sheet asked about
    /// a fixed, public video (`probe_video`). Only the sheet's playlist list is read; what it
    /// holds of that video is ignored.
    pub async fn editable_playlists(&self, probe_video: &str) -> Result<Vec<String>, LoadFailure> {
        let options = self
            .tube
            .add_to_playlist_options(&self.client, probe_video)
            .await
            .map_err(|e| classify(&e))?;
        Ok(options.into_iter().map(|o| o.id).collect())
    }

    /// Create an empty private playlist and return its bare id. Never retried: an unknown outcome
    /// comes back as [`WriteFailure::Unknown`] for the caller to read back.
    pub async fn create_playlist(&self, title: &str) -> Result<String, WriteFailure> {
        self.tube
            .create_playlist(&self.client, title)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Delete a playlist the account owns (`playlist_id` may carry the `VL` prefix).
    pub async fn delete_playlist(&self, playlist_id: &str) -> Result<(), WriteFailure> {
        self.tube
            .delete_playlist(&self.client, bare_playlist_id(playlist_id))
            .await
            .map_err(|e| classify_write(&e))
    }

    pub async fn rename_playlist(
        &self,
        playlist_id: &str,
        title: &str,
    ) -> Result<(), WriteFailure> {
        self.tube
            .rename_playlist(&self.client, bare_playlist_id(playlist_id), title)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Create a playlist that already holds `video_ids`, in order, and return its bare id. One
    /// write, never retried: an unknown outcome comes back as [`WriteFailure::Unknown`].
    pub async fn create_playlist_with(
        &self,
        title: &str,
        description: &str,
        privacy: Privacy,
        video_ids: &[String],
    ) -> Result<String, WriteFailure> {
        self.tube
            .create_playlist_with(&self.client, title, description, privacy.service_word(), video_ids)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Change the title, description and visibility of a playlist the account owns: only what
    /// `edit` carries is sent, in one request. Never retried.
    pub async fn edit_playlist(
        &self,
        playlist_id: &str,
        edit: &PlaylistEdit,
    ) -> Result<(), WriteFailure> {
        self.tube
            .edit_playlist(
                &self.client,
                bare_playlist_id(playlist_id),
                edit.title.as_deref(),
                edit.description.as_deref(),
                edit.privacy.map(Privacy::service_word),
            )
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Add a song to the library (`saved`) or take it out, by the token its row carried. A song
    /// no row has offered the change for is [`WriteFailure::Rejected`] without anything sent.
    /// Never retried: an unknown outcome is read back from Library > Songs by the caller.
    pub async fn set_song_saved(&self, video_id: &str, saved: bool) -> Result<(), WriteFailure> {
        let Some(token) = self.marks.token(video_id, saved) else {
            return Err(WriteFailure::Rejected);
        };
        self.tube
            .library_feedback(&self.client, &token)
            .await
            .map_err(|e| classify_write(&e))?;
        self.marks.set(video_id, saved);
        Ok(())
    }

    /// The library holds the song (`saved`) or not, learned some other way than by this source's
    /// own write (the caller read the library back).
    pub fn note_song_saved(&self, video_id: &str, saved: bool) {
        if saved {
            self.marks.listed(video_id);
        } else {
            self.marks.set(video_id, false);
        }
    }

    /// Remove one row (`set_video_id` is the row's own token) of a playlist the account owns.
    pub async fn remove_from_playlist(
        &self,
        playlist_id: &str,
        video_id: &str,
        set_video_id: &str,
    ) -> Result<(), WriteFailure> {
        self.tube
            .remove_from_playlist(&self.client, bare_playlist_id(playlist_id), video_id, set_video_id)
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Move one row in front of another (`None`: to the end).
    pub async fn move_in_playlist(
        &self,
        playlist_id: &str,
        set_video_id: &str,
        before_set_video_id: Option<&str>,
    ) -> Result<(), WriteFailure> {
        self.tube
            .move_in_playlist(
                &self.client,
                bare_playlist_id(playlist_id),
                set_video_id,
                before_set_video_id,
            )
            .await
            .map_err(|e| classify_write(&e))
    }

    /// Swap rows of a playlist the account owns in one request: add `adds` (videos, in order) and
    /// take out `removes` (video id and row token). Sent once; the rows the add made come back.
    pub async fn swap_rows(
        &self,
        playlist_id: &str,
        adds: &[String],
        removes: &[(String, String)],
    ) -> Result<Vec<AddedRow>, WriteFailure> {
        let edits: Vec<RowEdit> = adds
            .iter()
            .map(|v| RowEdit::Add(v.clone()))
            .chain(removes.iter().map(|(video_id, set_video_id)| RowEdit::Remove {
                video_id: video_id.clone(),
                set_video_id: set_video_id.clone(),
            }))
            .collect();
        self.tube
            .edit_rows(&self.client, bare_playlist_id(playlist_id), &edits)
            .await
            .map(added_rows)
            .map_err(|e| classify_write(&e))
    }

    /// Move several rows in one request, in order (each `(row, before)` as
    /// [`Self::move_in_playlist`]).
    pub async fn move_rows(
        &self,
        playlist_id: &str,
        moves: &[(String, Option<String>)],
    ) -> Result<(), WriteFailure> {
        let edits: Vec<RowEdit> = moves
            .iter()
            .map(|(set_video_id, before)| RowEdit::MoveBefore {
                set_video_id: set_video_id.clone(),
                before: before.clone(),
            })
            .collect();
        self.tube
            .edit_rows(&self.client, bare_playlist_id(playlist_id), &edits)
            .await
            .map(|_| ())
            .map_err(|e| classify_write(&e))
    }

    /// Save a playlist or album to the library, or take it out (see `InnerTube::save_playlist`).
    pub async fn set_saved(&self, save_id: &str, saved: bool) -> Result<(), WriteFailure> {
        self.tube
            .save_playlist(&self.client, bare_playlist_id(save_id), saved)
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
    /// the track. Nothing else in the response is read.
    pub async fn loudness_db(&self, video_id: &str) -> Result<Option<f32>, LoadFailure> {
        let loudness = self
            .tube
            .player_loudness(&self.client, video_id)
            .await
            .map_err(|e| classify(&e))?;
        Ok(loudness.loudness_db)
    }

    /// Where a play of `video_id` is reported, from one metadata-only `player` request (the same
    /// one [`loudness_db`](Self::loudness_db) makes). `Ok(None)` is an answer: the response had
    /// no tracking URLs. Reporting itself is [`report_start`](Self::report_start) and
    /// [`report_watchtime`](Self::report_watchtime).
    pub async fn playback_tracking(
        &self,
        video_id: &str,
    ) -> Result<Option<PlaybackTracking>, LoadFailure> {
        self.tube
            .player_tracking(&self.client, video_id)
            .await
            .map_err(|e| classify(&e))
    }

    /// Tell YouTube Music the playback `nonce` has started. One attempt; the caller decides what
    /// a failure means (the app only logs it).
    pub async fn report_start(
        &self,
        tracking: &PlaybackTracking,
        nonce: &str,
        position_ms: u64,
    ) -> Result<(), LoadFailure> {
        self.tube
            .report_playback_start(&self.client, tracking, nonce, position_ms)
            .await
            .map_err(|e| classify(&e))
    }

    /// One watch-time ping for the playback `nonce`. One attempt.
    pub async fn report_watchtime(
        &self,
        tracking: &PlaybackTracking,
        nonce: &str,
        ping: &WatchtimePing,
    ) -> Result<(), LoadFailure> {
        self.tube
            .report_watchtime(&self.client, tracking, nonce, ping)
            .await
            .map_err(|e| classify(&e))
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
