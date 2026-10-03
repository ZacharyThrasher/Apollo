use std::sync::Arc;

use apollo_domain::account::{Validation, ValidationError};
use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::{LoadFailure, Page};
use innertube::{Clients, InnerTube, Session, YouTubeClient, METADATA_CLIENT};
use tokio::sync::Notify;

use crate::map::{
    account_validation, classify, collections_from_grid, page_from_album, page_from_continuation,
    page_from_playlist, ListedTrack,
};
use crate::search::{search_results, SearchResults};

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

    /// First page of a playlist (`VL…`) or any page that lists tracks.
    pub async fn playlist(&self, browse_id: &str) -> Result<Page<ListedTrack>, LoadFailure> {
        let page = self
            .tube
            .playlist(&self.client, browse_id, None)
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

fn to_validation_error(err: innertube::Error) -> ValidationError {
    match classify(&err) {
        LoadFailure::Session => ValidationError::Expired,
        _ => ValidationError::Unreachable,
    }
}
