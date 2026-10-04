//! Where plays are reported in the real app: the web client's playback and watch-time pings,
//! sent as the app's signed-in account (cargo feature `provider`).
//!
//! The account service stays the owner of the cookies; every call takes a fresh snapshot of its
//! session the way search and loudness do. The tracking lookup is one metadata-only `player`
//! request per play, the same request the loudness fetch makes. Nothing here logs a cookie, a
//! tracking URL or an id: a failure is only a step name (see `reporting`).

use apollo_account::service::{AccountService, IdentityState};
use apollo_provider::{
    new_nonce, InnerTubeSource, PingState, PlaybackTracking, WatchtimePing,
};
use apollo_services_common::unix_now;

use super::real_search::{hand_back_rotation, request_session, SyncedSource};
use crate::reporting::{PingFailed, Pinger, Schedule, ThreadReporter, WatchReport, WatchState};

/// One play's tracking: the URLs and the nonce every ping about it carries.
pub struct Tracked {
    tracking: PlaybackTracking,
    nonce: String,
}

pub struct AccountPinger {
    account: AccountService,
    /// One worker: pings go out one at a time and nothing waits on them.
    runtime: tokio::runtime::Runtime,
    sources: SyncedSource,
}

impl AccountPinger {
    pub fn new(account: AccountService) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("apollo-play-report-io")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            account,
            runtime,
            sources: SyncedSource::default(),
        })
    }

    fn source(&self) -> Result<InnerTubeSource, PingFailed> {
        let session = request_session(&self.account, unix_now()).map_err(|_| PingFailed)?;
        self.sources.source_for(&session).map_err(|_| PingFailed)
    }
}

impl Pinger for AccountPinger {
    type Playback = Tracked;

    fn account(&mut self) -> Option<String> {
        match self.account.view().identity {
            IdentityState::Selected(identity) => Some(identity.key()),
            _ => None,
        }
    }

    fn tracking(&mut self, video_id: &str) -> Result<Option<Tracked>, PingFailed> {
        let session = request_session(&self.account, unix_now()).map_err(|_| PingFailed)?;
        let source = self.sources.source_for(&session).map_err(|_| PingFailed)?;
        let result = self.runtime.block_on(async {
            source.bootstrap_visitor_data().await;
            source.playback_tracking(video_id).await
        });
        if let Some(header_now) = source.cookie().filter(|h| *h != session.header) {
            hand_back_rotation(&self.account, &session, &header_now);
        }
        match result {
            Ok(Some(tracking)) => Ok(Some(Tracked {
                tracking,
                nonce: new_nonce(),
            })),
            Ok(None) => Ok(None),
            Err(_) => Err(PingFailed),
        }
    }

    fn schedule(&mut self, playback: &Tracked) -> Schedule {
        Schedule {
            first_secs: playback.tracking.schedule_secs.clone(),
            interval_secs: playback.tracking.interval_secs,
        }
    }

    fn start(&mut self, playback: &Tracked, position_ms: u64) -> Result<(), PingFailed> {
        let source = self.source()?;
        self.runtime
            .block_on(source.report_start(&playback.tracking, &playback.nonce, position_ms))
            .map_err(|_| PingFailed)
    }

    fn watch(&mut self, playback: &Tracked, ping: &WatchReport) -> Result<(), PingFailed> {
        let source = self.source()?;
        let ping = WatchtimePing {
            position_ms: ping.position_ms,
            ranges: ping.ranges.clone(),
            elapsed_ms: ping.elapsed_ms,
            duration_ms: ping.duration_ms,
            state: match ping.state {
                WatchState::Playing => PingState::Playing,
                WatchState::Paused => PingState::Paused,
            },
            is_final: ping.is_final,
            volume: ping.volume,
            muted: ping.muted,
        };
        self.runtime
            .block_on(source.report_watchtime(&playback.tracking, &playback.nonce, &ping))
            .map_err(|_| PingFailed)
    }
}

/// The reporter for the signed-in account. `None` if its threads cannot start: nothing is
/// reported.
pub fn reporter(account: &AccountService) -> Option<ThreadReporter> {
    ThreadReporter::spawn(AccountPinger::new(account.clone()).ok()?)
}
