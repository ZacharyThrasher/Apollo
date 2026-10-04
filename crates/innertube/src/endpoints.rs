//! High-level endpoint facade over the transport. Read paths plus the writes Apollo makes: `rate`
//! (like / unlike), adding one track to a playlist, `subscribe` (an artist's channel), and
//! playlist editing (create, rename, delete, remove or move a row, save or unsave a playlist or
//! album). The library `feedback` toggles stay out until a milestone needs them (see NOTICE.md).

use serde::Serialize;

use crate::clients::YouTubeClient;
use crate::models::browse::{
    self, AlbumPage, ArtistPage, BrowseItem, HistoryGroup, HomePage, MoodSection,
    PlaylistContinuation, PlaylistOption, PlaylistPage, PlaylistSort, SearchResults,
    SearchSuggestions,
};
use crate::models::context::Context;
use crate::models::lyrics::parse_lyrics_page;
use crate::models::metadata::{
    self, AccountIdentity, AccountInfo, NextResult, Rating, SearchResult,
};
use crate::models::player::{parse_player_loudness, PlayerLoudness};
use crate::tracking::{self, parse_playback_tracking, PlaybackTracking, WatchtimePing};
use crate::transport::{Error, InnerTube};

/// Search filter params (opaque base64). context/08.
pub const FILTER_SONG: &str = "EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D";
pub const FILTER_ALBUM: &str = "EgWKAQIYAWoKEAkQChAFEAMQBA%3D%3D";
pub const FILTER_ARTIST: &str = "EgWKAQIgAWoKEAkQChAFEAMQBA%3D%3D";
pub const FILTER_COMMUNITY_PLAYLIST: &str = "EgeKAQQoAEABagoQAxAEEAoQCRAF";
/// YouTube Music's own ("featured") playlists, the service's "Playlists" tab.
pub const FILTER_FEATURED_PLAYLIST: &str = "EgeKAQQoADgBagwQAxAKEAkQBRAE";

impl InnerTube {
    /// Raw `search` POST. `params` = a filter (None = the mixed, unfiltered search). context/08.
    ///
    /// `record_history` decides whether the request carries the account at all. A signed-in
    /// `search` is written to the account's YouTube search history, and the typeahead fires one
    /// per debounce, so every half-typed prefix used to show up later in YouTube's and YTM's own
    /// search box (#203). Only the query the user actually submitted passes `true`; a preview goes
    /// out with no cookie and no `onBehalfOfUser`, which YouTube cannot attribute to anyone.
    /// Nothing is lost by that: a search response carries no per-account field, not even
    /// `likeStatus`, so the anonymous rows are the same rows minus personalised ranking.
    async fn search_raw(
        &self,
        client: &YouTubeClient,
        query: &str,
        params: Option<&str>,
        record_history: bool,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct SearchBody {
            context: Context,
            query: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            params: Option<String>,
        }
        let body = SearchBody {
            context: if record_history {
                self.context_for(client)
            } else {
                self.context_anonymous(client)
            },
            query: query.to_owned(),
            params: params.map(str::to_owned),
        };
        self.post("search", client, &body, /* set_login */ record_history).await
    }

    /// Search songs only (`FILTER_SONG`). context/08.
    pub async fn search_songs(
        &self,
        metadata_client: &YouTubeClient,
        query: &str,
        record_history: bool,
    ) -> Result<SearchResult, Error> {
        let value =
            self.search_raw(metadata_client, query, Some(FILTER_SONG), record_history).await?;
        let r = metadata::parse_search(&value);
        Ok(r)
    }

    /// Unfiltered search → categorized sections (top / songs / albums / artists / playlists).
    pub async fn search_all(
        &self,
        client: &YouTubeClient,
        query: &str,
        record_history: bool,
    ) -> Result<SearchResults, Error> {
        let value = self.search_raw(client, query, None, record_history).await?;
        let r = browse::parse_search_all(&value);
        Ok(r)
    }

    /// Typeahead for the search field and Ctrl+K: query completions plus a few matching songs,
    /// artists and albums. context/08.
    ///
    /// Sent with the account, unlike a preview `search` (#203): this is the request YTM's own
    /// search box sends signed in on every keystroke, so it is not written to search history, and
    /// the account is what ranks the user's own listening first and returns their past searches.
    pub async fn search_suggestions(
        &self,
        client: &YouTubeClient,
        input: &str,
    ) -> Result<SearchSuggestions, Error> {
        #[derive(Serialize)]
        struct Body {
            context: Context,
            input: String,
        }
        let body = Body { context: self.context_for(client), input: input.to_owned() };
        let value = self.post("music/get_search_suggestions", client, &body, true).await?;
        let r = browse::parse_search_suggestions(&value);
        Ok(r)
    }

    /// First page of a filtered search (`filter` is one of the `FILTER_*` params). Carries the
    /// account: an anonymous filtered search answers "No results" (checked 2026-10-03), and the
    /// repeat of a query the page already recorded adds nothing new to the account's history.
    pub async fn search_filtered(
        &self,
        client: &YouTubeClient,
        query: &str,
        filter: &str,
    ) -> Result<browse::SearchPage, Error> {
        let value = self.search_raw(client, query, Some(filter), true).await?;
        Ok(browse::parse_search_page(&value))
    }

    /// The page after one a filtered search answered, by its paging token.
    pub async fn search_continuation(
        &self,
        client: &YouTubeClient,
        token: &str,
    ) -> Result<browse::SearchPage, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ContinuationBody {
            context: Context,
        }
        let body = ContinuationBody { context: self.context_for(client) };
        let enc = urlencoding::encode(token);
        let path = format!("search?ctoken={enc}&continuation={enc}&type=next");
        let value = self.post(&path, client, &body, true).await?;
        Ok(browse::parse_search_page(&value))
    }

    /// Up-next queue / radio for a video. context/08. Uses the metadata client.
    ///
    /// `video_id` is optional: an artist/mood radio is a playlist id with no seed track
    /// (`playlistId` alone), which is how YouTube itself opens one.
    pub async fn next(
        &self,
        metadata_client: &YouTubeClient,
        video_id: Option<&str>,
        playlist_id: Option<&str>,
    ) -> Result<NextResult, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct NextBody {
            context: Context,
            #[serde(skip_serializing_if = "Option::is_none")]
            video_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            playlist_id: Option<String>,
            is_audio_only: bool,
        }
        let body = NextBody {
            context: self.context_for(metadata_client),
            video_id: video_id.map(str::to_owned),
            playlist_id: playlist_id.map(str::to_owned),
            is_audio_only: true,
        };
        let value = self.post("next", metadata_client, &body, true).await?;
        let next = metadata::parse_next(&value);
        Ok(next)
    }

    /// The Related tab of a video: a `next` for the related browse id, then that page read like
    /// the home feed (a shelf of songs, then album, artist and playlist shelves). A video whose
    /// response names no related tab gives an empty page. Read-only; uses the same client as
    /// `next`.
    pub async fn related(
        &self,
        client: &YouTubeClient,
        video_id: &str,
    ) -> Result<HomePage, Error> {
        let next = self.next(client, Some(video_id), None).await?;
        let Some(browse_id) = next.related_browse_id else {
            return Ok(HomePage {
                chips: Vec::new(),
                sections: Vec::new(),
                continuation: None,
                skipped_shelves: 0,
            });
        };
        let value = self.browse(client, Some(&browse_id), None).await?;
        let mut page = browse::parse_home(&value);
        page.sections.retain(|s| !s.items.is_empty());
        // The related page has no further batches.
        page.continuation = None;
        Ok(page)
    }

    /// YouTube Music's own lyrics for a video, as plain text. `next` names the track's lyrics tab
    /// and a `browse` of that tab holds the text; `Ok(None)` is an answer (no tab, or an empty
    /// one). Read-only.
    pub async fn lyrics(
        &self,
        metadata_client: &YouTubeClient,
        video_id: &str,
    ) -> Result<Option<String>, Error> {
        let next = self.next(metadata_client, Some(video_id), None).await?;
        let Some(browse_id) = next.lyrics_browse_id else {
            return Ok(None);
        };
        let value = self.browse(metadata_client, Some(&browse_id), None).await?;
        Ok(parse_lyrics_page(&value))
    }

    /// How loud YouTube measured a video, from the `player` endpoint (`playerConfig.audioConfig`).
    ///
    /// Metadata only. The response also carries stream URLs; they are dropped unread (audio comes
    /// from yt-dlp). The request is the plain one the web client sends before it has a stream:
    /// no signature timestamp, no PO token, nothing that would let the answer be played.
    /// Read-only and idempotent.
    pub async fn player_loudness(
        &self,
        client: &YouTubeClient,
        video_id: &str,
    ) -> Result<PlayerLoudness, Error> {
        let body = player_body(self.context_for(client), video_id);
        let value = self.post("player", client, &body, true).await?;
        Ok(parse_player_loudness(&value))
    }

    /// The URLs to report a play of `video_id` to, from the same metadata-only `player` request
    /// as [`player_loudness`](Self::player_loudness). `Ok(None)` when the response has none.
    /// Read-only; reporting is a separate step ([`report_playback_start`](Self::report_playback_start)).
    pub async fn player_tracking(
        &self,
        client: &YouTubeClient,
        video_id: &str,
    ) -> Result<Option<PlaybackTracking>, Error> {
        let body = player_body(self.context_for(client), video_id);
        let value = self.post("player", client, &body, true).await?;
        Ok(parse_playback_tracking(&value))
    }

    /// Tell YouTube Music a playback has started (the web client's `videostatsPlaybackUrl`
    /// ping). `nonce` is this playback's [`new_nonce`]; every later ping about it reuses it.
    /// One attempt, no retry: a lost ping is a play that is not in History, not an error.
    pub async fn report_playback_start(
        &self,
        client: &YouTubeClient,
        tracking: &PlaybackTracking,
        nonce: &str,
        position_ms: u64,
    ) -> Result<(), Error> {
        let url = tracking::playback_url(
            &tracking.playback_url,
            nonce,
            &client.client_version,
            position_ms,
        )
        .ok_or_else(|| Error::Other("tracking url is not a url".into()))?;
        self.send_tracking_ping(client, &url).await
    }

    /// Send one watch-time ping for the playback `nonce` (the web client's
    /// `videostatsWatchtimeUrl`). One attempt, no retry.
    pub async fn report_watchtime(
        &self,
        client: &YouTubeClient,
        tracking: &PlaybackTracking,
        nonce: &str,
        ping: &WatchtimePing,
    ) -> Result<(), Error> {
        let url =
            tracking::watchtime_url(&tracking.watchtime_url, nonce, &client.client_version, ping)
                .ok_or_else(|| Error::Other("tracking url is not a url".into()))?;
        self.send_tracking_ping(client, &url).await
    }

    /// Logged-in account summary (`account/account_menu`, context/01). Requires a cookie. Also the
    /// source of `dataSyncId` (context/04A) and a login-bound visitorData (context/15).
    pub async fn account_menu(&self, client: &YouTubeClient) -> Result<AccountInfo, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AccountMenuBody {
            context: Context,
        }
        let body = AccountMenuBody { context: self.context_for(client) };
        let value = self.post("account/account_menu", client, &body, true).await?;
        Ok(metadata::parse_account_menu(&value))
    }

    /// Validate and refresh one delegated identity without mutating the transport's shared
    /// selection. This keeps unrelated in-flight requests on the previously committed channel.
    pub async fn account_menu_for_identity(
        &self,
        client: &YouTubeClient,
        data_sync_id: &str,
    ) -> Result<AccountInfo, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AccountMenuBody {
            context: Context,
        }
        let body = AccountMenuBody { context: self.context_for_identity(client, data_sync_id) };
        let value = self.post("account/account_menu", client, &body, true).await?;
        Ok(metadata::parse_account_menu(&value))
    }

    /// Every usable YouTube identity under the signed-in Google account. The official web client
    /// opens this sibling endpoint from the account menu; `account/account_menu` itself only
    /// carries the active header.
    pub async fn account_identities(
        &self,
        client: &YouTubeClient,
    ) -> Result<Vec<AccountIdentity>, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AccountsListBody {
            context: Context,
        }
        let body = AccountsListBody { context: self.context_for(client) };
        let value = self.post("account/accounts_list", client, &body, true).await?;
        Ok(metadata::parse_account_identities(&value))
    }

    /// Raw `browse` call (context/01, context/08). `browse_id`/`params` optional; response is the
    /// deeply-nested renderer tree the browse parsers walk.
    async fn browse(
        &self,
        client: &YouTubeClient,
        browse_id: Option<&str>,
        params: Option<&str>,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct BrowseBody {
            context: Context,
            #[serde(skip_serializing_if = "Option::is_none")]
            browse_id: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            params: Option<String>,
        }
        let body = BrowseBody {
            context: self.context_for(client),
            browse_id: browse_id.map(str::to_owned),
            params: params.map(str::to_owned),
        };
        let value = self.post("browse", client, &body, true).await?;
        // A stale cookie authenticates transport-wise but YouTube returns a logged-out "Sign in"
        // state for account-scoped browse. That is a rejected session, not an empty library.
        if self.is_logged_in() && browse::is_signed_out(&value) {
            return Err(self.reject_session());
        }
        Ok(value)
    }

    /// POST a paging token. The ctoken is carried in the query, matching Metrolist's
    /// browse-continuation call; every paged surface (home, playlist tracks, library grids) uses
    /// this same carrier.
    async fn browse_continuation(
        &self,
        client: &YouTubeClient,
        token: &str,
    ) -> Result<serde_json::Value, Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ContinuationBody {
            context: Context,
        }
        let body = ContinuationBody { context: self.context_for(client) };
        let enc = urlencoding::encode(token);
        let path = format!("browse?ctoken={enc}&continuation={enc}&type=next");
        self.post(&path, client, &body, true).await
    }

    /// A library grid, paged to the end. YouTube hands these out ~25 at a time, so a single browse
    /// silently truncated anyone's bigger library (issue #72).
    ///
    /// Library grids only. `browse_grid`'s "More" targets deliberately do NOT page: they are read
    /// as cards, and some of them (an album's `VL…` shelf) are really 100-track pages, so paging
    /// one would spend a dozen sequential requests building a card wall nobody asked for. The
    /// card grids behind those buttons come back whole anyway (a 91-album discography arrived in
    /// one response, no token).
    async fn library_grid(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
    ) -> Result<Vec<BrowseItem>, Error> {
        let value = self.browse(client, Some(browse_id), None).await?;
        // A card the grid hands out twice is fatal on the UI side: every library list is keyed by
        // browseId, and one repeat blanks the whole list (the sidebar, the Library page, the
        // add-to-playlist picker) with an each_key_duplicate. Only libraries past the first page
        // saw it, which is why removing a few playlists in YouTube Music "fixed" it. Issues #258,
        // #260.
        let mut seen = std::collections::HashSet::new();
        let mut dropped = 0usize;
        let mut keep = |items: &mut Vec<BrowseItem>| {
            items.retain(|i: &BrowseItem| {
                let fresh = seen.insert(i.id.clone());
                dropped += usize::from(!fresh);
                fresh
            })
        };
        let mut items = browse::parse_library(&value);
        keep(&mut items);
        let mut token = browse::continuation_token(&value);
        // ponytail: page cap, so a token that never resolves can't spin forever. Raise it if
        // anyone turns up with a library past ~500 entries.
        for _ in 0..20 {
            let Some(t) = token else { break };
            // A failed page must not throw away the ones that worked: a short library beats an
            // error where the grid should be.
            let Ok(value) = self.browse_continuation(client, &t).await.inspect_err(|e| {
                tracing::warn!(error = %e, browse_id, "grid continuation failed; keeping what loaded")
            }) else {
                break;
            };
            let mut page = browse::parse_library(&value);
            if page.is_empty() {
                // A spurious token: some grids carry one that resolves to nothing.
                break;
            }
            // Dedupe after the emptiness test, not before: a page that is entirely cards we
            // already have still carries the token for the page after it, and that one can hold
            // cards found nowhere else. A token that cycles is bounded by the self-reference
            // filter below and by the page cap above.
            keep(&mut page);
            items.extend(page);
            token = browse::continuation_token(&value).filter(|next| *next != t);
        }
        // Says so in a log the next reporter pastes: without it, a library that still looks wrong
        // can't be told apart from one that was never duplicating in the first place.
        if dropped > 0 {
            tracing::warn!(
                browse_id,
                dropped,
                "library grid repeated cards; kept the first of each"
            );
        }
        Ok(items)
    }

    /// Home feed (`FEmusic_home`). `params` is a mood/genre chip token from a previous home
    /// response — pass it to get that chip's filtered feed. context/08.
    pub async fn home(
        &self,
        client: &YouTubeClient,
        params: Option<&str>,
    ) -> Result<HomePage, Error> {
        let value = self.browse(client, Some("FEmusic_home"), params).await?;
        let mut page = browse::parse_home(&value);
        // A shelf the filter emptied (an all-videos row) would render as a bare heading.
        page.sections.retain(|s| !s.items.is_empty());
        Ok(page)
    }

    /// Next batch of home shelves via a continuation token. Same ctoken carrier as
    /// `playlist_continuation`; the response's shelves parse with `parse_home` (its find_all walk
    /// doesn't care whether shelves sit under `contents` or `continuationContents`).
    pub async fn home_continuation(
        &self,
        client: &YouTubeClient,
        token: &str,
    ) -> Result<HomePage, Error> {
        let value = self.browse_continuation(client, token).await?;
        let mut page = browse::parse_home(&value);
        page.sections.retain(|s| !s.items.is_empty());
        Ok(page)
    }

    /// Moods & Genres (`FEmusic_moods_and_genres`): the tiles the search page browses from. A tile's
    /// `params` opens its playlists via `browse_grid("FEmusic_moods_and_genres_category", ..)`.
    /// Works signed out. context/08.
    pub async fn moods(&self, client: &YouTubeClient) -> Result<Vec<MoodSection>, Error> {
        let value = self.browse(client, Some("FEmusic_moods_and_genres"), None).await?;
        Ok(browse::parse_moods(&value))
    }

    /// Explore (`FEmusic_explore`): the shelves under the New releases / Charts / Moods buttons
    /// (trending songs, new releases when the account is offered them). Which shelves come back
    /// depends on the account and country, so callers pick by what a shelf holds.
    pub async fn explore(&self, client: &YouTubeClient) -> Result<HomePage, Error> {
        let value = self.browse(client, Some("FEmusic_explore"), None).await?;
        let mut page = browse::parse_explore(&value);
        page.sections.retain(|s| !s.items.is_empty());
        Ok(page)
    }

    /// Charts (`FEmusic_charts`) for the session's country (its locale's `gl`): chart playlists,
    /// top artists, and a ranked song list where the service sends one.
    pub async fn charts(&self, client: &YouTubeClient) -> Result<HomePage, Error> {
        let value = self.browse(client, Some("FEmusic_charts"), None).await?;
        let mut page = browse::parse_explore(&value);
        page.sections.retain(|s| !s.items.is_empty());
        Ok(page)
    }

    /// New albums and singles (`FEmusic_new_releases_albums`) as cards.
    pub async fn new_releases(&self, client: &YouTubeClient) -> Result<Vec<BrowseItem>, Error> {
        let value = self.browse(client, Some("FEmusic_new_releases_albums"), None).await?;
        Ok(browse::parse_cards(&value))
    }

    /// The playlists of one mood or genre tile (`params` from [`Self::moods`]).
    pub async fn mood_playlists(
        &self,
        client: &YouTubeClient,
        params: &str,
    ) -> Result<Vec<BrowseItem>, Error> {
        self.browse_grid(client, "FEmusic_moods_and_genres_category", Some(params)).await
    }

    /// Play history (`FEmusic_history`), in YouTube's own date buckets (Today, Yesterday, …).
    /// context/08. Needs login: signed out, YouTube has nothing to return.
    pub async fn history(&self, client: &YouTubeClient) -> Result<Vec<HistoryGroup>, Error> {
        let value = self.browse(client, Some("FEmusic_history"), None).await?;
        let mut groups = browse::parse_history(&value);
        groups.retain(|g| !g.items.is_empty()); // a bucket the filter emptied is a bare heading
        Ok(groups)
    }

    /// Library playlists grid (`FEmusic_liked_playlists`). context/08. Needs login.
    pub async fn library_playlists(
        &self,
        client: &YouTubeClient,
    ) -> Result<Vec<BrowseItem>, Error> {
        self.library_grid(client, "FEmusic_liked_playlists").await
    }

    /// Saved albums grid (`FEmusic_liked_albums`). context/08. Needs login.
    pub async fn library_albums(&self, client: &YouTubeClient) -> Result<Vec<BrowseItem>, Error> {
        self.library_grid(client, "FEmusic_liked_albums").await
    }

    /// The albums the signed-in user uploaded themselves
    /// (`FEmusic_library_privately_owned_releases`). Cards come back as ordinary `MPREb_…` album
    /// browseIds, so they open on the album page like any other. context/08. Needs login.
    pub async fn upload_albums(&self, client: &YouTubeClient) -> Result<Vec<BrowseItem>, Error> {
        self.library_grid(client, "FEmusic_library_privately_owned_releases").await
    }

    /// Library artists (`FEmusic_library_corpus_track_artists`) — the artists behind the songs and
    /// albums in your library, which is what YouTube Music's own Artists tab shows (subscriptions
    /// live under `FEmusic_library_corpus_artists`). context/08. Needs login.
    pub async fn library_artists(&self, client: &YouTubeClient) -> Result<Vec<BrowseItem>, Error> {
        self.library_grid(client, "FEmusic_library_corpus_track_artists").await
    }

    /// The artists the account subscribes to (`FEmusic_library_corpus_artists`), as list rows
    /// like the library artists. Read-only; the export's "subscribed artists". Needs login.
    pub async fn subscribed_artists(&self, client: &YouTubeClient) -> Result<Vec<BrowseItem>, Error> {
        self.library_grid(client, "FEmusic_library_corpus_artists").await
    }

    /// A playlist or album page by browseId (`VL…` / `MPRE…`). context/08.
    ///
    /// `sort` asks YouTube to order the tracks — see `PlaylistSort::params`. Passing `None` gets
    /// whatever order the account already has the list in, which is the one thing a fresh visit
    /// wants: it is what YouTube Music would show.
    pub async fn playlist(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
        sort: Option<(PlaylistSort, bool)>,
    ) -> Result<PlaylistPage, Error> {
        let params = sort.map(|(s, desc)| s.params(desc));
        let value = self.browse(client, Some(browse_id), params).await?;
        if let Some(show) = browse::podcast_show_id(browse_id, &value) {
            let value = self.browse(client, Some(&show), params).await?;
            return Ok(browse::parse_playlist(&value));
        }
        Ok(browse::parse_playlist(&value))
    }

    /// An album page by album browseId (`MPRE…`). context/08.
    ///
    /// Some album rows link the official music video (`musicVideoType` ≠ ATV), so playing them
    /// streams the MV's audio (intros, skits, crowd) instead of the album track. The album's
    /// `OLAK5uy_` *audio* playlist carries the album-audio uploads in track order, so for those
    /// rows we swap in its videoId (one extra fetch, only for affected albums).
    pub async fn album(&self, client: &YouTubeClient, browse_id: &str) -> Result<AlbumPage, Error> {
        let value = self.browse(client, Some(browse_id), None).await?;
        let mut page = browse::parse_album(&value);
        // Album track rows never carry the album's own browseId (live-checked 2026-09-02, every
        // release type), so without this every track played off a release page reaches the queue
        // with no `album_id` and the ⋮ menu offers no "Go to album". We are holding the id.
        for item in &mut page.items {
            item.album_id.get_or_insert_with(|| browse_id.to_owned());
        }
        let video = browse::album_video_flags(&value);
        if video.contains(&true) {
            if let Some(pl) = &page.playlist_id {
                match self.playlist(client, &format!("VL{pl}"), None).await {
                    // ponytail: positional match, guarded on equal track counts — the OLAK
                    // playlist is the same album in the same order. Mismatch → keep the MV ids.
                    Ok(audio) if audio.items.len() == page.items.len() => {
                        for ((item, is_video), audio_item) in
                            page.items.iter_mut().zip(&video).zip(audio.items)
                        {
                            if *is_video {
                                item.video_id = audio_item.video_id;
                                item.duration = audio_item.duration.or(item.duration.take());
                                // The row linked the MV, but the id it now carries is the audio
                                // track: leaving the flag set would put the player view's video
                                // mode on a still album-art stream.
                                item.is_video = false;
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "audio-playlist fetch failed; keeping MV ids")
                    }
                }
            }
        }
        for c in &mut page.sections {
            // YouTube lists the album you are already on under "Other versions". A card that
            // reopens the current page is noise, so drop it.
            c.items.retain(|i| i.id != browse_id);
        }
        page.sections.retain(|c| !c.items.is_empty());
        Ok(page)
    }

    /// An artist page by channel browseId (`UC…`). context/08.
    pub async fn artist(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
    ) -> Result<ArtistPage, Error> {
        let value = self.browse(client, Some(browse_id), None).await?;
        let page = browse::parse_artist(&value, browse_id);
        Ok(page)
    }

    /// A browse target that returns a grid of cards (e.g. an artist's "all albums" page reached
    /// via a carousel's "More" button). context/08.
    pub async fn browse_grid(
        &self,
        client: &YouTubeClient,
        browse_id: &str,
        params: Option<&str>,
    ) -> Result<Vec<BrowseItem>, Error> {
        let value = self.browse(client, Some(browse_id), params).await?;
        let mut items = browse::parse_library(&value);
        // A mood category is several shelves flattened into one grid, and YouTube puts the same
        // playlist on more than one of them (Chill had 85 repeats). A repeat is fatal on the UI
        // side for the same reason as in `library_grid`: the grid is keyed, and one duplicate key
        // leaves the page on its skeleton forever. Issue #355.
        let mut seen = std::collections::HashSet::new();
        items.retain(|i| seen.insert(i.id.clone()));
        Ok(items)
    }

    /// Next page of playlist tracks via a continuation token. context/08.
    pub async fn playlist_continuation(
        &self,
        client: &YouTubeClient,
        token: &str,
    ) -> Result<PlaylistContinuation, Error> {
        let value = self.browse_continuation(client, token).await?;
        Ok(browse::parse_playlist_continuation(&value))
    }

    // --- write actions (context/01 ✎, context/15 D7). All auth-gated (SAPISIDHASH). ---------

    /// Rate a video, or clear its rating. context/01. The three states are mutually exclusive on
    /// YouTube's side: disliking a liked track removes it from Liked Music in the same call.
    ///
    /// Idempotent: liking a liked track, or clearing a clear one, changes nothing, so the
    /// transport's retry on a connect error or timeout cannot double anything.
    pub async fn rate(
        &self,
        client: &YouTubeClient,
        video_id: &str,
        rating: Rating,
    ) -> Result<(), Error> {
        let body = rate_body(self.context_for(client), video_id);
        self.post(rate_path(rating), client, &body, true).await?;
        Ok(())
    }

    /// [`Self::rate`] sent exactly once, whatever happens (no transport retry). For a rating the
    /// owner asked for by hand where the outcome of a timeout is read back rather than guessed.
    pub async fn rate_once(
        &self,
        client: &YouTubeClient,
        video_id: &str,
        rating: Rating,
    ) -> Result<(), Error> {
        let body = rate_body(self.context_for(client), video_id);
        self.post_once(rate_path(rating), client, &body, true).await?;
        Ok(())
    }

    /// Subscribe to or unsubscribe from an artist's channel (`subscription/subscribe` and
    /// `subscription/unsubscribe`, the requests the artist page's own button makes). `channel_id`
    /// is the artist's `UC…` id.
    ///
    /// Idempotent on the service's side, but sent exactly once all the same: a timeout leaves the
    /// outcome unknown, and the caller reads the artist page back to learn what the account
    /// holds instead of firing the request again.
    pub async fn subscribe(
        &self,
        client: &YouTubeClient,
        channel_id: &str,
        subscribed: bool,
    ) -> Result<(), Error> {
        let body = subscribe_body(self.context_for(client), channel_id);
        self.post_once(subscribe_path(subscribed), client, &body, true).await?;
        Ok(())
    }

    /// The playlists the signed-in account can add this video to, and whether each already holds
    /// it (`playlist/get_add_to_playlist`, the request YouTube Music's own "Save to playlist"
    /// sheet makes). Only playlists the account may edit come back, which is why the picker asks
    /// here instead of listing the library's saved playlists.
    pub async fn add_to_playlist_options(
        &self,
        client: &YouTubeClient,
        video_id: &str,
    ) -> Result<Vec<PlaylistOption>, Error> {
        let body = add_options_body(self.context_for(client), video_id);
        let value = self.post("playlist/get_add_to_playlist", client, &body, true).await?;
        // A reply with no sheet in it is a shape this parser does not know. `Json` is what the
        // callers already treat as "could not be read", as opposed to "there is nothing".
        browse::parse_add_to_playlist(&value).ok_or_else(|| {
            Error::Json(serde::de::Error::custom("no add-to-playlist sheet in the response"))
        })
    }

    /// Add one video to a playlist the account owns (`browse/edit_playlist`, `ACTION_ADD_VIDEO`).
    /// `playlist_id` is the bare id (`PL…`), not the `VL…` browse id.
    ///
    /// Sent with `DEDUPE_OPTION_SKIP`, so a track already in the playlist is skipped rather than
    /// added a second time. That also makes the request safe to repeat: the transport retries a
    /// connect error or timeout, and a request that did land the first time must not add the
    /// track twice. The service answers `STATUS_SUCCEEDED` or says no; anything else is
    /// [`Error::Refused`].
    pub async fn add_to_playlist(
        &self,
        client: &YouTubeClient,
        playlist_id: &str,
        video_id: &str,
    ) -> Result<(), Error> {
        let body = add_to_playlist_body(self.context_for(client), playlist_id, video_id);
        let value = self.post("browse/edit_playlist", client, &body, true).await?;
        if browse::edit_succeeded(&value) {
            Ok(())
        } else {
            Err(Error::Refused("playlist edit was not accepted".into()))
        }
    }

    // --- playlist editing (Apollo addition, see NOTICE.md). ------------------------------------
    //
    // Every one goes out once (`post_once`): none of them is retried by the transport, because a
    // timeout leaves the outcome unknown and a blind resend of a create, remove or move could do
    // it twice. The caller reads the state back instead.

    /// Create an empty, private playlist (`playlist/create`) and return its bare id (`PL…`).
    ///
    /// A reply that carries no playlist id is [`Error::Json`]: the playlist may well exist, so
    /// the caller treats it as an unknown outcome and looks for it in the library.
    pub async fn create_playlist(
        &self,
        client: &YouTubeClient,
        title: &str,
    ) -> Result<String, Error> {
        let body = create_playlist_body(self.context_for(client), title);
        let value = self.post_once("playlist/create", client, &body, true).await?;
        value
            .get("playlistId")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::Json(serde::de::Error::custom("no playlistId in the create response"))
            })
    }

    /// Delete a playlist the account owns (`playlist/delete`). `playlist_id` is the bare id.
    pub async fn delete_playlist(
        &self,
        client: &YouTubeClient,
        playlist_id: &str,
    ) -> Result<(), Error> {
        let body = delete_playlist_body(self.context_for(client), playlist_id);
        self.post_once("playlist/delete", client, &body, true).await?;
        Ok(())
    }

    /// Rename a playlist the account owns (`ACTION_SET_PLAYLIST_NAME`).
    pub async fn rename_playlist(
        &self,
        client: &YouTubeClient,
        playlist_id: &str,
        title: &str,
    ) -> Result<(), Error> {
        let body = edit_body(
            self.context_for(client),
            playlist_id,
            serde_json::json!({ "action": "ACTION_SET_PLAYLIST_NAME", "playlistName": title }),
        );
        self.edit_once(client, &body).await
    }

    /// Remove one row of a playlist the account owns (`ACTION_REMOVE_VIDEO`). `set_video_id` is
    /// the row's own token (`playlistSetVideoId`), so a video listed twice loses only that row.
    pub async fn remove_from_playlist(
        &self,
        client: &YouTubeClient,
        playlist_id: &str,
        video_id: &str,
        set_video_id: &str,
    ) -> Result<(), Error> {
        let body = edit_body(
            self.context_for(client),
            playlist_id,
            serde_json::json!({
                "action": "ACTION_REMOVE_VIDEO",
                "setVideoId": set_video_id,
                "removedVideoId": video_id,
            }),
        );
        self.edit_once(client, &body).await
    }

    /// Move one row of a playlist the account owns to sit before another (`ACTION_MOVE_VIDEO_BEFORE`);
    /// `before_set_video_id` `None` moves it to the end. Both are row tokens.
    pub async fn move_in_playlist(
        &self,
        client: &YouTubeClient,
        playlist_id: &str,
        set_video_id: &str,
        before_set_video_id: Option<&str>,
    ) -> Result<(), Error> {
        let body = edit_body(
            self.context_for(client),
            playlist_id,
            move_action(set_video_id, before_set_video_id),
        );
        self.edit_once(client, &body).await
    }

    /// Save a playlist or album to the library, or take it out again (`like/like`,
    /// `like/removelike` with a playlist target). `playlist_id` is the bare playlist id; for an
    /// album it is the album's audio playlist (`OLAK5uy_…`), not its `MPRE…` browse id.
    pub async fn save_playlist(
        &self,
        client: &YouTubeClient,
        playlist_id: &str,
        saved: bool,
    ) -> Result<(), Error> {
        let body = save_playlist_body(self.context_for(client), playlist_id);
        self.post_once(save_playlist_path(saved), client, &body, true).await?;
        Ok(())
    }

    async fn edit_once(
        &self,
        client: &YouTubeClient,
        body: &serde_json::Value,
    ) -> Result<(), Error> {
        let value = self.post_once("browse/edit_playlist", client, body, true).await?;
        if browse::edit_succeeded(&value) {
            Ok(())
        } else {
            Err(Error::Refused("playlist edit was not accepted".into()))
        }
    }
}

fn subscribe_path(subscribed: bool) -> &'static str {
    if subscribed {
        "subscription/subscribe"
    } else {
        "subscription/unsubscribe"
    }
}

fn subscribe_body(context: Context, channel_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        channel_ids: [String; 1],
    }
    serde_json::to_value(Body { context, channel_ids: [channel_id.to_owned()] })
        .expect("a plain struct serialises")
}

fn rate_path(rating: Rating) -> &'static str {
    match rating {
        Rating::Like => "like/like",
        Rating::Dislike => "like/dislike",
        Rating::Indifferent => "like/removelike",
    }
}

fn rate_body(context: Context, video_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct LikeBody {
        context: Context,
        target: Target,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Target {
        video_id: String,
    }
    serde_json::to_value(LikeBody { context, target: Target { video_id: video_id.to_owned() } })
        .expect("a plain struct serialises")
}

/// The `player` request. `contentCheckOk`/`racyCheckOk` make a track that sits behind a content
/// warning answer like any other, so its loudness is still readable.
fn player_body(context: Context, video_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        video_id: String,
        playback_context: PlaybackContext,
        content_check_ok: bool,
        racy_check_ok: bool,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PlaybackContext {
        content_playback_context: ContentPlaybackContext,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ContentPlaybackContext {
        html5_preference: &'static str,
    }
    serde_json::to_value(Body {
        context,
        video_id: video_id.to_owned(),
        playback_context: PlaybackContext {
            content_playback_context: ContentPlaybackContext {
                html5_preference: "HTML5_PREF_WANTS",
            },
        },
        content_check_ok: true,
        racy_check_ok: true,
    })
    .expect("a plain struct serialises")
}

fn add_options_body(context: Context, video_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        /// An array: the service answers `videoId` (a single string) with a 400. Found against
        /// the real account; the first version of this request guessed the singular.
        video_ids: [String; 1],
        exclude_watch_later: bool,
    }
    serde_json::to_value(Body {
        context,
        video_ids: [video_id.to_owned()],
        // Watch Later is a YouTube (not YouTube Music) list: not a place a song goes here.
        exclude_watch_later: true,
    })
    .expect("a plain struct serialises")
}

fn create_playlist_body(context: Context, title: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        title: String,
        description: &'static str,
        privacy_status: &'static str,
    }
    serde_json::to_value(Body {
        context,
        title: title.to_owned(),
        description: "",
        // Always private: a new playlist is never public until its owner says so on the site.
        privacy_status: "PRIVATE",
    })
    .expect("a plain struct serialises")
}

fn delete_playlist_body(context: Context, playlist_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        playlist_id: String,
    }
    serde_json::to_value(Body { context, playlist_id: playlist_id.to_owned() })
        .expect("a plain struct serialises")
}

/// A `browse/edit_playlist` request carrying one action.
fn edit_body(context: Context, playlist_id: &str, action: serde_json::Value) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        playlist_id: String,
        actions: [serde_json::Value; 1],
    }
    serde_json::to_value(Body { context, playlist_id: playlist_id.to_owned(), actions: [action] })
        .expect("a plain struct serialises")
}

/// The move action: before the row `before`, or to the end when there is none.
fn move_action(set_video_id: &str, before: Option<&str>) -> serde_json::Value {
    let mut action = serde_json::json!({
        "action": "ACTION_MOVE_VIDEO_BEFORE",
        "setVideoId": set_video_id,
    });
    if let Some(before) = before {
        action["movedSetVideoIdSuccessor"] = before.into();
    }
    action
}

fn save_playlist_path(saved: bool) -> &'static str {
    if saved {
        "like/like"
    } else {
        "like/removelike"
    }
}

fn save_playlist_body(context: Context, playlist_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        target: Target,
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Target {
        playlist_id: String,
    }
    serde_json::to_value(Body { context, target: Target { playlist_id: playlist_id.to_owned() } })
        .expect("a plain struct serialises")
}

fn add_to_playlist_body(context: Context, playlist_id: &str, video_id: &str) -> serde_json::Value {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Body {
        context: Context,
        playlist_id: String,
        actions: [Action; 1],
    }
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Action {
        action: &'static str,
        added_video_id: String,
        dedupe_option: &'static str,
    }
    serde_json::to_value(Body {
        context,
        playlist_id: playlist_id.to_owned(),
        actions: [Action {
            action: "ACTION_ADD_VIDEO",
            added_video_id: video_id.to_owned(),
            dedupe_option: "DEDUPE_OPTION_SKIP",
        }],
    })
    .expect("a plain struct serialises")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::clients::Clients;
    use crate::transport::Session;

    fn tube() -> (InnerTube, YouTubeClient) {
        let client = Clients::bundled().get(crate::METADATA_CLIENT).cloned().unwrap();
        let session = Session {
            cookie: Some("SAPISID=x".into()),
            data_sync_id: Some("PAGE1".into()),
            ..Session::default()
        };
        (InnerTube::new(session, None).unwrap(), client)
    }

    #[test]
    fn the_player_request_asks_for_one_video_and_nothing_to_play_it_with() {
        let (tube, client) = tube();
        let body = player_body(tube.context_for(&client), "dQw4w9WgXcQ");
        assert_eq!(body["videoId"], "dQw4w9WgXcQ");
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
        assert_eq!(body["contentCheckOk"], true);
        // No signature timestamp and no PO token: the answer cannot be used to fetch a stream.
        let text = body.to_string();
        assert!(!text.contains("signatureTimestamp"), "{text}");
        assert!(!text.contains("poToken"), "{text}");
    }

    #[test]
    fn each_rating_has_its_own_endpoint_and_the_video_is_the_target() {
        assert_eq!(rate_path(Rating::Like), "like/like");
        assert_eq!(rate_path(Rating::Dislike), "like/dislike");
        assert_eq!(rate_path(Rating::Indifferent), "like/removelike");
        let (tube, client) = tube();
        let body = rate_body(tube.context_for(&client), "dQw4w9WgXcQ");
        assert_eq!(body["target"], json!({ "videoId": "dQw4w9WgXcQ" }));
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
        // The signed-in channel rides along, as on every other authenticated request.
        assert_eq!(body["context"]["user"]["onBehalfOfUser"], "PAGE1");
    }

    #[test]
    fn subscribing_names_one_channel_and_acts_as_the_signed_in_account() {
        assert_eq!(subscribe_path(true), "subscription/subscribe");
        assert_eq!(subscribe_path(false), "subscription/unsubscribe");
        let (tube, client) = tube();
        let body = subscribe_body(tube.context_for(&client), "UCdrake");
        assert_eq!(body["channelIds"], json!(["UCdrake"]));
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
        assert_eq!(body["context"]["user"]["onBehalfOfUser"], "PAGE1");
    }

    #[test]
    fn adding_to_a_playlist_asks_to_skip_a_duplicate_and_names_one_video() {
        let (tube, client) = tube();
        let body = add_to_playlist_body(tube.context_for(&client), "PLabc", "dQw4w9WgXcQ");
        assert_eq!(body["playlistId"], "PLabc");
        assert_eq!(
            body["actions"],
            json!([{
                "action": "ACTION_ADD_VIDEO",
                "addedVideoId": "dQw4w9WgXcQ",
                "dedupeOption": "DEDUPE_OPTION_SKIP"
            }])
        );
    }

    #[test]
    fn the_playlist_sheet_request_leaves_out_watch_later() {
        let (tube, client) = tube();
        let body = add_options_body(tube.context_for(&client), "dQw4w9WgXcQ");
        assert_eq!(body["videoIds"], serde_json::json!(["dQw4w9WgXcQ"]));
        assert!(body.get("videoId").is_none(), "the service rejects the singular with a 400");
        assert_eq!(body["excludeWatchLater"], true);
    }
    #[test]
    fn a_new_playlist_is_private_and_untouched_by_a_description() {
        let (tube, client) = tube();
        let body = create_playlist_body(tube.context_for(&client), "Apollo test");
        assert_eq!(body["title"], "Apollo test");
        assert_eq!(body["privacyStatus"], "PRIVATE");
        assert_eq!(body["description"], "");
        assert!(body.get("videoIds").is_none(), "created empty");
        assert_eq!(body["context"]["user"]["onBehalfOfUser"], "PAGE1");
    }

    #[test]
    fn delete_names_only_the_playlist() {
        let (tube, client) = tube();
        let body = delete_playlist_body(tube.context_for(&client), "PLabc");
        assert_eq!(body["playlistId"], "PLabc");
        assert_eq!(body["context"]["client"]["clientName"], "WEB_REMIX");
        let mut keys: Vec<_> = body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["context", "playlistId"]);
    }

    #[test]
    fn rename_remove_and_move_are_single_edit_actions() {
        let (tube, client) = tube();
        let rename = edit_body(
            tube.context_for(&client),
            "PLabc",
            json!({ "action": "ACTION_SET_PLAYLIST_NAME", "playlistName": "New" }),
        );
        assert_eq!(rename["playlistId"], "PLabc");
        assert_eq!(
            rename["actions"],
            json!([{ "action": "ACTION_SET_PLAYLIST_NAME", "playlistName": "New" }])
        );
        let remove = edit_body(
            tube.context_for(&client),
            "PLabc",
            json!({ "action": "ACTION_REMOVE_VIDEO", "setVideoId": "SV1", "removedVideoId": "vid" }),
        );
        assert_eq!(remove["actions"].as_array().unwrap().len(), 1);
        assert_eq!(remove["actions"][0]["setVideoId"], "SV1");
        assert_eq!(remove["actions"][0]["removedVideoId"], "vid");
    }

    #[test]
    fn a_move_names_the_row_it_goes_before_and_goes_to_the_end_without_one() {
        assert_eq!(
            move_action("SV1", Some("SV9")),
            json!({
                "action": "ACTION_MOVE_VIDEO_BEFORE",
                "setVideoId": "SV1",
                "movedSetVideoIdSuccessor": "SV9",
            })
        );
        let to_end = move_action("SV1", None);
        assert_eq!(to_end["action"], "ACTION_MOVE_VIDEO_BEFORE");
        assert!(to_end.get("movedSetVideoIdSuccessor").is_none());
    }

    #[test]
    fn saving_is_a_like_on_the_playlist_and_unsaving_clears_it() {
        assert_eq!(save_playlist_path(true), "like/like");
        assert_eq!(save_playlist_path(false), "like/removelike");
        let (tube, client) = tube();
        let body = save_playlist_body(tube.context_for(&client), "OLAK5uy_x");
        assert_eq!(body["target"], json!({ "playlistId": "OLAK5uy_x" }));
        assert!(body["target"].get("videoId").is_none());
    }
}
