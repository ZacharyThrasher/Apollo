//! Account state (BUILD_SPEC AUTH-01, AUTH-04, SEC-03).
//!
//! Three independent concepts are tracked, never conflated:
//! * **session**: can the service authenticate us?
//! * **identity**: which account/channel's library are we using?
//! * **Premium evidence**: what do we actually know about the membership?
//!
//! There is no public Premium entitlement API, so the app never reports
//! "verified Premium" from a resolver result, a bitrate or an absent ad.
//! Every asynchronous result carries the account epoch it was started under;
//! results from an older epoch are ignored.

/// Opaque selected account/channel identifier (not derived from cookies).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct IdentityId(String);

impl IdentityId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Session {
    Absent,
    Validating,
    Valid,
    Expired,
    /// Could not reach the service. Not the same as signed out.
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Identity {
    None,
    Selected(IdentityId),
    /// More than one account/channel is available and none was chosen.
    Ambiguous(Vec<IdentityId>),
    /// A revalidation no longer offers the identity that was selected.
    Changed {
        was: IdentityId,
        available: Vec<IdentityId>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PremiumEvidence {
    /// The owner says the account has full Premium. The default for this app.
    Declared,
    /// A maintained account signal supports Premium.
    ObservedPositive,
    /// A reliable account signal says it is not Premium.
    ObservedNegative,
    /// No usable signal (an unavailable undocumented signal is not "negative").
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    SignedOut,
    Validating,
    SessionExpired,
    IdentityNeeded,
    IdentityChanged,
    PremiumNegative,
    PremiumNotDeclared,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Content browsing, playback and writes are allowed.
    Full,
    /// Service unreachable after a successful validation: cached views stay
    /// usable, new private requests are not attempted until it recovers.
    CachedOnly,
    Blocked(Blocked),
}

/// What the interface may say about Premium. Never a stronger claim than the evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PremiumLabel {
    /// "Premium assumed for this account".
    Assumed,
    /// A maintained signal supports it.
    Detected,
    /// A reliable signal says no.
    NotPremium,
    /// Nothing declared, nothing observed.
    Unconfirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Validation {
    /// Accounts/channels reachable with this session, as reported by the service.
    pub identities: Vec<IdentityId>,
    /// `Some(true/false)` only when a maintained account signal exists.
    pub premium_signal: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValidationError {
    /// The service reliably says the session is not valid.
    Expired,
    /// Network or service trouble: no verdict.
    Unreachable,
}

#[derive(Clone, Debug)]
pub enum AccountEvent {
    /// New credentials were imported or captured.
    Imported,
    Validated {
        epoch: u64,
        result: Result<Validation, ValidationError>,
    },
    SelectIdentity(IdentityId),
    /// Owner declaration of full Premium.
    DeclarePremium(bool),
    /// The service rejected a request as unauthorized outside validation.
    AuthorizationRejected {
        epoch: u64,
    },
    /// Periodic or startup revalidation.
    Revalidate,
    Logout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountEffect {
    /// Run an authenticated account-specific validation under this epoch.
    Validate { epoch: u64 },
    /// Cancel and reap in-flight private work, stop audio, drop leases.
    StopPlayback,
    /// Drop caches and persisted queue state belonging to the previous epoch.
    PurgePrivateState,
    /// Delete stored credentials (logout only).
    ForgetCredentials,
}

pub struct AccountState {
    epoch: u64,
    session: Session,
    identity: Identity,
    premium: PremiumEvidence,
    declared: bool,
    /// A validation succeeded under the current epoch (so Unreachable is not "never validated").
    validated_this_epoch: bool,
}

impl Default for AccountState {
    fn default() -> Self {
        Self::new()
    }
}

impl AccountState {
    pub fn new() -> Self {
        Self {
            epoch: 0,
            session: Session::Absent,
            identity: Identity::None,
            premium: PremiumEvidence::Unknown,
            declared: false,
            validated_this_epoch: false,
        }
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn session(&self) -> Session {
        self.session
    }
    pub fn identity(&self) -> &Identity {
        &self.identity
    }
    pub fn premium_evidence(&self) -> PremiumEvidence {
        self.premium
    }
    pub fn declared_premium(&self) -> bool {
        self.declared
    }

    pub fn selected_identity(&self) -> Option<&IdentityId> {
        match &self.identity {
            Identity::Selected(id) => Some(id),
            _ => None,
        }
    }

    pub fn access(&self) -> Access {
        match self.session {
            Session::Absent => return Access::Blocked(Blocked::SignedOut),
            Session::Validating if !self.validated_this_epoch => {
                return Access::Blocked(Blocked::Validating)
            }
            Session::Expired => return Access::Blocked(Blocked::SessionExpired),
            _ => {}
        }
        match self.identity {
            Identity::None | Identity::Ambiguous(_) => {
                return Access::Blocked(Blocked::IdentityNeeded)
            }
            Identity::Changed { .. } => return Access::Blocked(Blocked::IdentityChanged),
            Identity::Selected(_) => {}
        }
        if self.premium == PremiumEvidence::ObservedNegative {
            return Access::Blocked(Blocked::PremiumNegative);
        }
        if !self.declared && self.premium != PremiumEvidence::ObservedPositive {
            return Access::Blocked(Blocked::PremiumNotDeclared);
        }
        match self.session {
            Session::Unreachable => Access::CachedOnly,
            _ => Access::Full,
        }
    }

    pub fn premium_label(&self) -> PremiumLabel {
        match self.premium {
            PremiumEvidence::ObservedNegative => PremiumLabel::NotPremium,
            PremiumEvidence::ObservedPositive => PremiumLabel::Detected,
            PremiumEvidence::Declared | PremiumEvidence::Unknown if self.declared => {
                PremiumLabel::Assumed
            }
            _ => PremiumLabel::Unconfirmed,
        }
    }

    pub fn handle(&mut self, event: AccountEvent) -> Vec<AccountEffect> {
        let mut fx = Vec::new();
        match event {
            AccountEvent::Imported => {
                // New credentials: everything private from before is suspect.
                self.reset_private(&mut fx);
                self.session = Session::Validating;
                fx.push(AccountEffect::Validate { epoch: self.epoch });
            }
            AccountEvent::Revalidate => {
                if matches!(self.session, Session::Absent) {
                    return fx;
                }
                // Keep serving the current state while revalidating; no epoch bump.
                if !self.validated_this_epoch {
                    self.session = Session::Validating;
                }
                fx.push(AccountEffect::Validate { epoch: self.epoch });
            }
            AccountEvent::Validated { epoch, result } => {
                if epoch != self.epoch {
                    return fx; // stale: belongs to an earlier credential/identity epoch
                }
                match result {
                    Ok(v) => self.apply_validation(v, &mut fx),
                    Err(ValidationError::Expired) => {
                        self.session = Session::Expired;
                        self.validated_this_epoch = false;
                        fx.push(AccountEffect::StopPlayback);
                    }
                    Err(ValidationError::Unreachable) => {
                        // Not a logout. Keep the identity; only the session is in doubt.
                        self.session = Session::Unreachable;
                    }
                }
            }
            AccountEvent::SelectIdentity(id) => {
                // Only an identity the service offered can be chosen. The epoch was
                // already bumped when the identity became ambiguous or changed.
                let offered = match &self.identity {
                    Identity::Ambiguous(c) => c.contains(&id),
                    Identity::Changed { available, .. } => available.contains(&id),
                    Identity::Selected(_) | Identity::None => false,
                };
                if offered {
                    self.identity = Identity::Selected(id);
                    self.session = Session::Valid;
                    self.validated_this_epoch = true;
                }
            }
            AccountEvent::DeclarePremium(declared) => {
                self.declared = declared;
                if declared && self.premium == PremiumEvidence::Unknown {
                    self.premium = PremiumEvidence::Declared;
                } else if !declared && self.premium == PremiumEvidence::Declared {
                    self.premium = PremiumEvidence::Unknown;
                }
            }
            AccountEvent::AuthorizationRejected { epoch } => {
                if epoch == self.epoch {
                    // Suspicious: do not guess. Revalidate and let the verdict decide.
                    fx.push(AccountEffect::Validate { epoch: self.epoch });
                }
            }
            AccountEvent::Logout => {
                fx.push(AccountEffect::StopPlayback);
                fx.push(AccountEffect::PurgePrivateState);
                fx.push(AccountEffect::ForgetCredentials);
                self.epoch += 1;
                self.session = Session::Absent;
                self.identity = Identity::None;
                self.premium = PremiumEvidence::Unknown;
                self.validated_this_epoch = false;
                // The owner's declaration is a preference, not private account data.
            }
        }
        fx
    }

    fn reset_private(&mut self, fx: &mut Vec<AccountEffect>) {
        self.epoch += 1;
        self.identity = Identity::None;
        self.validated_this_epoch = false;
        fx.push(AccountEffect::StopPlayback);
        fx.push(AccountEffect::PurgePrivateState);
    }

    fn apply_validation(&mut self, v: Validation, fx: &mut Vec<AccountEffect>) {
        self.session = Session::Valid;
        self.validated_this_epoch = true;
        // Premium signal: only a maintained signal moves evidence off the declaration.
        self.premium = match v.premium_signal {
            Some(true) => PremiumEvidence::ObservedPositive,
            Some(false) => PremiumEvidence::ObservedNegative,
            None if self.declared => PremiumEvidence::Declared,
            None => PremiumEvidence::Unknown,
        };
        let previous = std::mem::replace(&mut self.identity, Identity::None);
        self.identity = match (previous, v.identities.len()) {
            (Identity::Selected(cur), _) if v.identities.contains(&cur) => Identity::Selected(cur),
            (Identity::Selected(was), _) => {
                // The selected identity vanished: block until the owner decides.
                self.epoch += 1;
                fx.push(AccountEffect::StopPlayback);
                fx.push(AccountEffect::PurgePrivateState);
                Identity::Changed {
                    was,
                    available: v.identities,
                }
            }
            (_, 0) => Identity::None,
            (_, 1) => Identity::Selected(v.identities.into_iter().next().expect("len 1")),
            (_, _) => Identity::Ambiguous(v.identities),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> IdentityId {
        IdentityId::new(s)
    }
    fn ok(ids: &[&str], signal: Option<bool>) -> Result<Validation, ValidationError> {
        Ok(Validation {
            identities: ids.iter().map(|s| id(s)).collect(),
            premium_signal: signal,
        })
    }
    fn signed_in(ids: &[&str]) -> AccountState {
        let mut a = AccountState::new();
        a.handle(AccountEvent::DeclarePremium(true));
        a.handle(AccountEvent::Imported);
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(ids, None),
        });
        a
    }

    // TEST-01
    #[test]
    fn valid_session_single_identity_and_declaration_gives_full_access() {
        let a = signed_in(&["me"]);
        assert_eq!(a.access(), Access::Full);
        assert_eq!(a.selected_identity(), Some(&id("me")));
        assert_eq!(
            a.premium_label(),
            PremiumLabel::Assumed,
            "declared is not verified"
        );
    }

    #[test]
    fn content_is_blocked_until_validation_and_declaration() {
        let mut a = AccountState::new();
        assert_eq!(a.access(), Access::Blocked(Blocked::SignedOut));
        a.handle(AccountEvent::Imported);
        assert_eq!(a.access(), Access::Blocked(Blocked::Validating));
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["me"], None),
        });
        assert_eq!(a.access(), Access::Blocked(Blocked::PremiumNotDeclared));
        assert_eq!(a.premium_label(), PremiumLabel::Unconfirmed);
        a.handle(AccountEvent::DeclarePremium(true));
        assert_eq!(a.access(), Access::Full);
    }

    // TEST-02
    #[test]
    fn expired_blocks_with_recovery_but_unreachable_is_not_logout() {
        let mut a = signed_in(&["me"]);
        let e = a.epoch();
        a.handle(AccountEvent::Revalidate);
        a.handle(AccountEvent::Validated {
            epoch: e,
            result: Err(ValidationError::Unreachable),
        });
        assert_eq!(a.access(), Access::CachedOnly);
        assert_eq!(
            a.selected_identity(),
            Some(&id("me")),
            "identity retained through an outage"
        );
        assert_eq!(a.epoch(), e);

        let fx = a.handle(AccountEvent::Validated {
            epoch: e,
            result: Err(ValidationError::Expired),
        });
        assert_eq!(a.access(), Access::Blocked(Blocked::SessionExpired));
        assert!(fx.contains(&AccountEffect::StopPlayback));
    }

    #[test]
    fn unknown_premium_signal_is_not_mislabeled_negative() {
        let mut a = signed_in(&["me"]);
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["me"], None),
        });
        assert_ne!(a.premium_label(), PremiumLabel::NotPremium);
        assert_eq!(a.access(), Access::Full);
    }

    #[test]
    fn a_reliable_negative_signal_blocks_and_a_positive_one_is_detected() {
        let mut a = signed_in(&["me"]);
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["me"], Some(false)),
        });
        assert_eq!(a.access(), Access::Blocked(Blocked::PremiumNegative));
        assert_eq!(a.premium_label(), PremiumLabel::NotPremium);
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["me"], Some(true)),
        });
        assert_eq!(a.access(), Access::Full);
        assert_eq!(a.premium_label(), PremiumLabel::Detected);
        // A later validation with no signal falls back to the declaration, not to "negative".
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["me"], None),
        });
        assert_eq!(a.premium_label(), PremiumLabel::Assumed);
    }

    // TEST-03
    #[test]
    fn ambiguous_identity_blocks_until_the_owner_chooses() {
        let mut a = AccountState::new();
        a.handle(AccountEvent::DeclarePremium(true));
        a.handle(AccountEvent::Imported);
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["personal", "brand"], None),
        });
        assert_eq!(a.access(), Access::Blocked(Blocked::IdentityNeeded));
        assert!(
            a.selected_identity().is_none(),
            "never guessed from order or email"
        );
        a.handle(AccountEvent::SelectIdentity(id("stranger")));
        assert_eq!(
            a.access(),
            Access::Blocked(Blocked::IdentityNeeded),
            "only offered candidates can be chosen"
        );
        a.handle(AccountEvent::SelectIdentity(id("brand")));
        assert_eq!(a.access(), Access::Full);
        assert_eq!(a.selected_identity(), Some(&id("brand")));
    }

    #[test]
    fn a_vanished_identity_blocks_bumps_the_epoch_and_purges() {
        let mut a = signed_in(&["me", "other"]);
        a.handle(AccountEvent::SelectIdentity(id("me")));
        let before = a.epoch();
        let fx = a.handle(AccountEvent::Validated {
            epoch: before,
            result: ok(&["other", "third"], None),
        });
        assert!(matches!(a.identity(), Identity::Changed { .. }));
        assert_eq!(a.access(), Access::Blocked(Blocked::IdentityChanged));
        assert_eq!(a.epoch(), before + 1);
        assert!(
            fx.contains(&AccountEffect::PurgePrivateState)
                && fx.contains(&AccountEffect::StopPlayback)
        );
        a.handle(AccountEvent::SelectIdentity(id("other")));
        assert_eq!(a.access(), Access::Full);
    }

    #[test]
    fn stale_validation_results_are_ignored() {
        let mut a = AccountState::new();
        a.handle(AccountEvent::DeclarePremium(true));
        a.handle(AccountEvent::Imported);
        let first = a.epoch();
        a.handle(AccountEvent::Imported); // re-imported before the first validation returned
        assert!(a.epoch() > first);
        let fx = a.handle(AccountEvent::Validated {
            epoch: first,
            result: ok(&["old"], None),
        });
        assert!(fx.is_empty());
        assert_eq!(a.access(), Access::Blocked(Blocked::Validating));
        a.handle(AccountEvent::Validated {
            epoch: a.epoch(),
            result: ok(&["new"], None),
        });
        assert_eq!(a.selected_identity(), Some(&id("new")));
    }

    // TEST-04 (logic)
    #[test]
    fn logout_stops_purges_forgets_and_invalidates_in_flight_work() {
        let mut a = signed_in(&["me"]);
        let e = a.epoch();
        let fx = a.handle(AccountEvent::Logout);
        assert_eq!(
            fx,
            [
                AccountEffect::StopPlayback,
                AccountEffect::PurgePrivateState,
                AccountEffect::ForgetCredentials
            ]
        );
        assert_eq!(a.access(), Access::Blocked(Blocked::SignedOut));
        assert!(a.epoch() > e);
        // A validation or request that was already in flight cannot resurrect the session.
        let fx = a.handle(AccountEvent::Validated {
            epoch: e,
            result: ok(&["me"], None),
        });
        assert!(fx.is_empty());
        assert_eq!(a.session(), Session::Absent);
        assert_eq!(a.selected_identity(), None);
    }

    #[test]
    fn unexpected_authorization_failure_triggers_revalidation_not_logout() {
        let mut a = signed_in(&["me"]);
        let e = a.epoch();
        let fx = a.handle(AccountEvent::AuthorizationRejected { epoch: e });
        assert_eq!(fx, [AccountEffect::Validate { epoch: e }]);
        assert_eq!(a.access(), Access::Full, "no verdict yet");
        assert!(
            a.handle(AccountEvent::AuthorizationRejected { epoch: e - 1 })
                .is_empty(),
            "stale epoch ignored"
        );
    }

    #[test]
    fn revalidation_does_not_flicker_a_working_session() {
        let mut a = signed_in(&["me"]);
        a.handle(AccountEvent::Revalidate);
        assert_eq!(a.access(), Access::Full);
        assert_eq!(a.session(), Session::Valid);
    }

    #[test]
    fn declaration_is_not_private_state_and_survives_logout() {
        let mut a = signed_in(&["me"]);
        a.handle(AccountEvent::Logout);
        assert!(a.declared_premium());
        assert_eq!(a.premium_label(), PremiumLabel::Assumed);
    }
}
