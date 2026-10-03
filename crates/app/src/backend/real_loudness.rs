//! Where the loudness of a track comes from in the real app: one metadata-only `player` request
//! per track, signed in as the app's account (cargo feature `real-services`).
//!
//! The account service stays the owner of the cookies; this takes a snapshot of its session the
//! way search does, and offers back any cookie the service rotated during the request. The
//! response is read for one number and dropped: no stream is taken from it, nothing is played or
//! reported, and nothing here logs a cookie, an id or the number itself. Any failure, including a
//! session the service rejected, only means "loudness unknown": the track plays at unity and the
//! account model is not told anything.

use apollo_account::service::AccountService;
use apollo_services_common::{unix_now, LoudnessError, LoudnessSource};

use super::real_search::{hand_back_rotation, request_session, SyncedSource};

pub struct AccountLoudness {
    account: AccountService,
    /// One worker: the fetcher asks for one track at a time and nothing waits on the answer.
    runtime: tokio::runtime::Runtime,
    sources: SyncedSource,
}

impl AccountLoudness {
    pub fn new(account: AccountService) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("apollo-loudness-io")
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

impl LoudnessSource for AccountLoudness {
    fn fetch(&self, video_id: &str) -> Result<Option<f32>, LoudnessError> {
        let session = request_session(&self.account, unix_now()).map_err(|_| LoudnessError)?;
        let source = self.sources.source_for(&session).map_err(|_| LoudnessError)?;
        let result = self.runtime.block_on(async {
            source.bootstrap_visitor_data().await;
            source.loudness_db(video_id).await
        });
        if let Some(header_now) = source.cookie().filter(|h| *h != session.header) {
            hand_back_rotation(&self.account, &session, &header_now);
        }
        result.map_err(|_| LoudnessError)
    }
}
