//! YouTube Music's own lyrics as the last lyrics source (cargo feature `real-services`): plain
//! text from the track's lyrics tab, for songs LRCLIB has nothing for.
//!
//! Like search and loudness, this borrows the account service's session for the request and
//! offers back any cookie the service rotated. Nothing here logs a cookie, an id or the text.
//! No session, or a service that cannot be reached, is a failed lookup (never a remembered "no
//! lyrics"): the service then just does not cache the miss.

use apollo_account::service::AccountService;
use apollo_lyrics::{FetchError, Lyrics, LyricsQuery, LyricsSource};
use apollo_services_common::unix_now;

use super::real_search::{hand_back_rotation, request_session, SyncedSource};

pub struct AccountLyrics {
    account: AccountService,
    /// One worker: lookups come one at a time (current track, then the next).
    runtime: tokio::runtime::Runtime,
    sources: SyncedSource,
}

impl AccountLyrics {
    pub fn new(account: AccountService) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("apollo-lyrics-io")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            account,
            runtime,
            sources: SyncedSource::default(),
        })
    }
}

impl LyricsSource for AccountLyrics {
    fn plain_only(&self) -> bool {
        true
    }

    fn fetch(&self, query: &LyricsQuery) -> Result<Option<Lyrics>, FetchError> {
        let unavailable = |what: &str| FetchError(what.to_owned());
        let session = request_session(&self.account, unix_now())
            .map_err(|_| unavailable("no YouTube Music session"))?;
        let source = self
            .sources
            .source_for(&session)
            .map_err(|_| unavailable("YouTube Music source"))?;
        let result = self.runtime.block_on(async {
            source.bootstrap_visitor_data().await;
            source.lyrics_text(&query.video_id).await
        });
        if let Some(header_now) = source.cookie().filter(|h| *h != session.header) {
            hand_back_rotation(&self.account, &session, &header_now);
        }
        match result {
            Ok(text) => Ok(text.map(Lyrics::Plain)),
            Err(_) => Err(unavailable("YouTube Music lyrics request failed")),
        }
    }
}
