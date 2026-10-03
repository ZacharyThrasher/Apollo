//! The simulated account: a validator that accepts any session as a demo listener and a
//! browser sign-in that "finishes" after a moment. It runs the real `AccountService`, so the
//! sign-in screen is exercised against the real state machine without a network.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use apollo_account::identity::{AccountValidator, Identity, Validation};
use apollo_account::jar::{Cookie, CookieJar};
use apollo_account::service::AccountService;

use crate::account::{AccountPorts, NoAvatars, SessionSource, SignInError};

const SIGN_IN_DELAY: Duration = Duration::from_millis(600);
const POLL: Duration = Duration::from_millis(30);

pub struct SimValidator;

impl AccountValidator for SimValidator {
    fn validate(&self, _: &CookieJar) -> Validation {
        Validation::Valid {
            identities: vec![Identity {
                name: "Demo Listener".into(),
                handle: Some("@demolistener".into()),
                avatar_url: None,
                page_id: None,
                selected_by_service: true,
            }],
        }
    }
}

/// A jar with the four cookies the service looks for and values that mean nothing.
pub fn sim_jar() -> CookieJar {
    let cookie = |name: &str, domain: &str| Cookie {
        name: name.into(),
        value: "sim".into(),
        domain: domain.into(),
        path: "/".into(),
        secure: true,
        http_only: true,
        expires_unix: Some(4_102_444_800),
    };
    CookieJar {
        cookies: vec![
            cookie("SAPISID", ".youtube.com"),
            cookie("__Secure-3PAPISID", ".youtube.com"),
            cookie("SID", ".google.com"),
            cookie("__Secure-3PSID", ".google.com"),
        ],
        ephemeral_seed: false,
    }
}

#[derive(Default)]
pub struct SimSource {
    cancelled: AtomicBool,
}

impl SessionSource for SimSource {
    fn browser(&self) -> Result<CookieJar, SignInError> {
        self.cancelled.store(false, Ordering::SeqCst);
        let mut waited = Duration::ZERO;
        while waited < SIGN_IN_DELAY {
            if self.cancelled.load(Ordering::SeqCst) {
                return Err(SignInError::Cancelled);
            }
            thread::sleep(POLL);
            waited += POLL;
        }
        Ok(sim_jar())
    }

    fn cancel_browser(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
}

/// A memory-only service over the simulated validator.
pub fn ports() -> AccountPorts {
    AccountPorts {
        service: AccountService::new(Arc::new(SimValidator), None),
        source: Arc::new(SimSource::default()),
        avatars: Arc::new(NoAvatars),
        startup_error: None,
    }
}

/// The simulation's startup: signed in as the demo listener, unless it should start signed out.
pub fn start(ports: &AccountPorts, signed_in: bool) {
    if signed_in {
        ports.service.import(sim_jar(), true);
    }
}
