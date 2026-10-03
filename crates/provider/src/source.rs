use std::sync::Arc;

use apollo_domain::account::{Validation, ValidationError};
use apollo_domain::listing::{LoadFailure, Page};
use innertube::{Clients, InnerTube, Session, YouTubeClient, METADATA_CLIENT};
use tokio::sync::Notify;

use crate::map::{
    account_validation, classify, page_from_album, page_from_continuation, page_from_playlist,
    ListedTrack,
};

/// Network-backed metadata source. Every call is authenticated: there is no guest mode, and a
/// rejected session is reported to the caller instead of being retried or healed here.
#[derive(Clone)]
pub struct InnerTubeSource {
    tube: InnerTube,
    client: YouTubeClient,
}

impl InnerTubeSource {
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
