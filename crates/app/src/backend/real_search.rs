//! Search against the live service, signed in as the app's account (cargo feature
//! `real-services`).
//!
//! The account service stays the owner of the cookies. Each search takes a snapshot of its jar,
//! turns it into the `Cookie:` header a browser would send to music.youtube.com and talks to the
//! service through the metadata provider. If the service rotated a cookie during the request
//! (Google replaces the `*PSIDTS` pair on ordinary requests and invalidates the old value), the
//! change is offered back to the account, which merges it only if its epoch still matches: the
//! same rule the resolver follows. Nothing here logs a cookie, a query or a result.

use std::sync::Mutex;

use apollo_account::identity::cookie_header_for;
use apollo_account::jar::CookieJar;
use apollo_account::service::{AccountService, IdentityState};
use apollo_domain::listing::LoadFailure;
use apollo_provider::{InnerTubeSource, SearchResults};
use apollo_services_common::{unix_now, SessionProvider, SessionUnavailable};

use crate::search::SearchSource;

const HOST: &str = "music.youtube.com";

/// What one request needs from the account, taken at one epoch.
struct RequestSession {
    epoch: u64,
    jar: CookieJar,
    header: String,
    /// Only for a channel the owner chose that is not the login's default one. The default needs
    /// none, which is also how the account's own validation talks to the service.
    data_sync_id: Option<String>,
}

fn request_session(account: &AccountService, now: u64) -> Result<RequestSession, LoadFailure> {
    let Some(snapshot) = account.snapshot() else {
        // A session the service has not confirmed yet is "try again", not "sign in again".
        return Err(match account.unavailable_reason() {
            SessionUnavailable::Unconfirmed => LoadFailure::Network,
            SessionUnavailable::NoSession => LoadFailure::Session,
        });
    };
    let jar = CookieJar::from_netscape(snapshot.cookies_netscape.expose());
    let header = cookie_header_for(&jar, HOST, now);
    if header.is_empty() {
        return Err(LoadFailure::Session);
    }
    let data_sync_id = match account.view().identity {
        IdentityState::Selected(identity) if !identity.selected_by_service => identity.page_id,
        _ => None,
    };
    Ok(RequestSession {
        epoch: snapshot.epoch,
        jar,
        header,
        data_sync_id,
    })
}

/// Offer the cookies the service changed during a request back to the account. `false` when
/// nothing changed or the account has moved on (its epoch no longer matches).
fn hand_back_rotation(
    account: &AccountService,
    session: &RequestSession,
    header_now: &str,
) -> bool {
    match session.jar.rotated_from_header(HOST, header_now) {
        Some(rotated) => account.offer_rotation(session.epoch, rotated.to_netscape()),
        None => false,
    }
}

struct Synced {
    epoch: u64,
    source: InnerTubeSource,
}

pub struct AccountSearch {
    account: AccountService,
    /// One worker: searches are I/O and the UI never waits on them.
    runtime: tokio::runtime::Runtime,
    synced: Mutex<Option<Synced>>,
}

impl AccountSearch {
    pub fn new(account: AccountService) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("apollo-search")
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            account,
            runtime,
            synced: Mutex::new(None),
        })
    }

    /// The provider source for `session`: reused while the account's epoch is unchanged (it may
    /// hold a cookie the service just rotated), pointed at the new jar when the epoch moved.
    fn source_for(&self, session: &RequestSession) -> Result<InnerTubeSource, LoadFailure> {
        let mut synced = self.synced.lock().unwrap();
        match synced.as_mut() {
            Some(s) if s.epoch == session.epoch => {}
            Some(s) => {
                s.source
                    .set_session(session.header.clone(), session.data_sync_id.clone());
                s.epoch = session.epoch;
            }
            None => {
                let source = InnerTubeSource::signed_in(
                    session.header.clone(),
                    session.data_sync_id.clone(),
                )
                .map_err(|_| LoadFailure::Other)?;
                *synced = Some(Synced {
                    epoch: session.epoch,
                    source,
                });
            }
        }
        Ok(synced.as_ref().expect("set above").source.clone())
    }
}

impl SearchSource for AccountSearch {
    fn search(&self, query: &str) -> Result<SearchResults, LoadFailure> {
        let session = request_session(&self.account, unix_now())?;
        let source = self.source_for(&session)?;
        let result = self.runtime.block_on(async {
            source.bootstrap_visitor_data().await;
            source.search(query).await
        });
        if let Some(header_now) = source.cookie().filter(|h| *h != session.header) {
            hand_back_rotation(&self.account, &session, &header_now);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use apollo_account::identity::{AccountValidator, Identity, Validation};
    use apollo_account::jar::Cookie;
    use apollo_account::service::ImportOutcome;

    use super::*;

    const NOW: u64 = 1_800_000_000;

    struct Scripted(Validation);

    impl AccountValidator for Scripted {
        fn validate(&self, _: &CookieJar) -> Validation {
            self.0.clone()
        }
    }

    fn cookie(name: &str, domain: &str, value: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            path: "/".into(),
            secure: true,
            http_only: true,
            expires_unix: Some(4_102_444_800),
        }
    }

    fn jar() -> CookieJar {
        CookieJar {
            cookies: vec![
                cookie("SAPISID", ".youtube.com", "sapisid-value"),
                cookie("__Secure-3PAPISID", ".youtube.com", "papisid-value"),
                cookie("__Secure-3PSIDTS", ".youtube.com", "psidts-old"),
                cookie("SID", ".google.com", "google-only"),
                cookie("__Secure-3PSID", ".google.com", "psid-value"),
            ],
            ephemeral_seed: false,
        }
    }

    fn identity(n: usize) -> Identity {
        Identity {
            name: format!("Channel {n}"),
            handle: None,
            avatar_url: None,
            page_id: Some(format!("PAGE{n}")),
            selected_by_service: n == 0,
        }
    }

    fn account_with(identities: Vec<Identity>) -> AccountService {
        let account =
            AccountService::new(Arc::new(Scripted(Validation::Valid { identities })), None);
        let outcome = account.import(jar(), true);
        assert!(matches!(
            outcome,
            ImportOutcome::Valid | ImportOutcome::NeedsIdentityChoice
        ));
        account
    }

    #[test]
    fn no_session_means_sign_in_and_never_a_request() {
        let account = AccountService::new(Arc::new(Scripted(Validation::Expired)), None);
        assert!(matches!(
            request_session(&account, NOW),
            Err(LoadFailure::Session)
        ));
    }

    #[test]
    fn the_header_is_what_a_browser_sends_to_music_youtube_com() {
        let account = account_with(vec![identity(0)]);
        let session = request_session(&account, NOW).ok().unwrap();
        assert!(session.header.contains("SAPISID=sapisid-value"));
        assert!(session.header.contains("__Secure-3PSIDTS=psidts-old"));
        assert!(
            !session.header.contains("google-only"),
            "a .google.com cookie is not sent to music.youtube.com"
        );
        assert_eq!(
            session.data_sync_id, None,
            "the default channel needs no delegation"
        );
    }

    #[test]
    fn a_chosen_non_default_channel_is_addressed_by_its_page_id() {
        let account = account_with(vec![identity(0), identity(1)]);
        assert!(account.select_identity(&identity(1).key()));
        let session = request_session(&account, NOW).ok().unwrap();
        assert_eq!(session.data_sync_id.as_deref(), Some("PAGE1"));
        assert!(account.select_identity(&identity(0).key()));
        assert_eq!(
            request_session(&account, NOW).ok().unwrap().data_sync_id,
            None
        );
    }

    #[test]
    fn a_cookie_the_service_rotated_is_handed_back_once_per_epoch() {
        let account = account_with(vec![identity(0)]);
        let session = request_session(&account, NOW).ok().unwrap();
        let rotated = session.header.replace("psidts-old", "psidts-new");

        assert!(hand_back_rotation(&account, &session, &rotated));
        let after = request_session(&account, NOW).ok().unwrap();
        assert!(after.header.contains("__Secure-3PSIDTS=psidts-new"));
        assert!(after.epoch > session.epoch, "a merge starts a new epoch");

        // Another search that started from the old snapshot loses: it cannot undo the merge.
        let stale = session.header.replace("psidts-old", "psidts-stale");
        assert!(!hand_back_rotation(&account, &session, &stale));
        assert!(request_session(&account, NOW)
            .ok()
            .unwrap()
            .header
            .contains("psidts-new"));
    }

    #[test]
    fn an_unchanged_header_hands_nothing_back() {
        let account = account_with(vec![identity(0)]);
        let session = request_session(&account, NOW).ok().unwrap();
        assert!(!hand_back_rotation(&account, &session, &session.header));
    }
}
