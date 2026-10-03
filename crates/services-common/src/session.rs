//! The resolver never owns credentials: it asks a [`SessionProvider`] (implemented by
//! the account service) for an isolated snapshot per request (SEC-02).

use crate::secret::SecretString;

/// An isolated copy of the canonical cookie jar, tagged with the account epoch it
/// was taken at. `cookies_netscape` is the Netscape cookie-file text yt-dlp reads.
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    pub epoch: u64,
    pub cookies_netscape: SecretString,
}

pub trait SessionProvider: Send + Sync {
    /// `None` when there is no validated account session. The resolver then refuses
    /// to run: there is no guest fallback (AUTH-02).
    fn snapshot(&self) -> Option<SessionSnapshot>;

    /// Offer cookies the helper rewrote (rotation). Returns whether they were merged;
    /// a snapshot from an older epoch must never overwrite a newer jar (TEST-15).
    fn offer_rotation(&self, epoch: u64, cookies_netscape: SecretString) -> bool;
}
