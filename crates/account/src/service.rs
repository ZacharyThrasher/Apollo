//! The account service (BUILD_SPEC sections 3 and 7). Holds the canonical cookie jar and keeps
//! three concepts apart:
//!
//! * **Session**: absent / validating / valid / expired / temporarily unreachable.
//! * **Identity**: none / selected / ambiguous / changed. Never inferred from email or order.
//! * **Premium evidence**: only ever *owner-declared*. There is no documented entitlement
//!   endpoint, so nothing here claims more, and an unknown signal is never a negative.
//!
//! Content actions need all three ([`AccountView::can_use_content`]). The resolver is handed a
//! snapshot only while the session is valid and an identity is selected, so a missing or dead
//! session can never fall through to a guest resolve. Validation calls run outside the lock and
//! every result is checked against the epoch it started under, so a login/logout/import that
//! happened meanwhile wins.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apollo_services_common::{SecretString, SessionProvider, SessionSnapshot};

use crate::identity::{AccountValidator, Identity, Validation};
use crate::jar::CookieJar;
use crate::store::{DeleteReport, EncryptedStore, StoreError, StoredAccount};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Absent,
    Validating,
    Valid,
    Expired,
    /// Could not confirm (network, 5xx/429, or an unrecognised answer). Not a logout.
    Unreachable,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdentityState {
    None,
    Selected(Identity),
    /// The login can act as several channels; the owner must choose.
    Ambiguous(Vec<Identity>),
    /// The previously selected identity is gone or differs: block until re-confirmed.
    Changed(Vec<Identity>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PremiumEvidence {
    /// The owner said this account has Premium. The default and, today, the only value.
    OwnerDeclared,
    /// Nothing declared yet.
    Undeclared,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageMode {
    /// Encrypted on disk, key in the OS vault.
    Durable,
    /// The vault could not be used: the session lives in memory only and is lost at exit.
    /// Shown to the owner; never silently downgraded to plaintext.
    NotPersisted(String),
    /// The owner accepted memory-only operation.
    MemoryOnly,
    /// No session to store.
    Nothing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    None,
    Network,
    /// The service answered in a shape we do not understand (compatibility problem).
    ServiceChanged,
    NoAuthCookies,
}

/// A copy of everything the UI may show. Contains no secrets.
#[derive(Clone, Debug)]
pub struct AccountView {
    pub session: SessionState,
    pub identity: IdentityState,
    pub premium: PremiumEvidence,
    pub storage: StorageMode,
    pub problem: Problem,
    pub epoch: u64,
    /// The jar came from a bare `Cookie:` header (no attributes, session-only seed).
    pub jar_is_seed: bool,
}

impl AccountView {
    /// Default gate (AUTH-01): valid session + confirmed identity + owner declaration.
    pub fn can_use_content(&self) -> bool {
        self.session == SessionState::Valid
            && matches!(self.identity, IdentityState::Selected(_))
            && self.premium == PremiumEvidence::OwnerDeclared
    }

    /// The wording the spec asks for; never "verified".
    pub fn premium_label(&self) -> &'static str {
        match self.premium {
            PremiumEvidence::OwnerDeclared => "Premium assumed for this account",
            PremiumEvidence::Undeclared => "Premium not declared",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RejectReason {
    NoAuthCookies,
    /// The service says the session is not logged in.
    Expired,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImportOutcome {
    /// Validated and bound to one identity.
    Valid,
    /// Validated, but the login can act as several channels: call `select_identity`.
    NeedsIdentityChoice,
    Rejected(RejectReason),
    /// The service could not be reached; the jar is kept in memory for `revalidate()` and is
    /// not persisted (nothing unvalidated is stored).
    Pending(Problem),
    /// A newer login/logout/import happened while validating; this result was discarded.
    Superseded,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RestoreOutcome {
    NothingStored,
    /// Stored session found; the resulting state is in the view.
    Restored,
    /// The vault is locked/unavailable: nothing could be read. Offer re-import or memory-only.
    VaultUnavailable,
    /// Stored data was unreadable (tampered, truncated, key gone) and has been removed.
    DiscardedUnreadable,
}

#[derive(Debug)]
pub struct LogoutReport {
    pub hooks_run: usize,
    pub stored: DeleteReport,
    pub was_logged_in: bool,
}

struct State {
    epoch: u64,
    session: SessionState,
    jar: Option<CookieJar>,
    identity: IdentityState,
    selected_key: Option<String>,
    premium: PremiumEvidence,
    storage: StorageMode,
    problem: Problem,
    validated_at: Option<Instant>,
}

impl State {
    fn view(&self) -> AccountView {
        AccountView {
            session: self.session,
            identity: self.identity.clone(),
            premium: self.premium,
            storage: self.storage.clone(),
            problem: self.problem,
            epoch: self.epoch,
            jar_is_seed: self.jar.as_ref().is_some_and(|j| j.ephemeral_seed),
        }
    }
}

type Hook = Box<dyn Fn() + Send + Sync>;
type Observer = Box<dyn Fn(&AccountView) + Send + Sync>;

struct Inner {
    validator: Arc<dyn AccountValidator>,
    store: Option<EncryptedStore>,
    state: Mutex<State>,
    logout_hooks: Mutex<Vec<Hook>>,
    observers: Mutex<Vec<Observer>>,
    refreshing: AtomicBool,
}

#[derive(Clone)]
pub struct AccountService {
    inner: Arc<Inner>,
}

impl fmt::Debug for AccountService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountService(<redacted>)")
    }
}

/// Default revalidation interval: a tunable policy, not a claimed cookie lifetime.
pub const REVALIDATE_AFTER: Duration = Duration::from_secs(30 * 60);

impl AccountService {
    /// `store: None` means memory-only operation (chosen explicitly by the caller).
    pub fn new(validator: Arc<dyn AccountValidator>, store: Option<EncryptedStore>) -> Self {
        Self {
            inner: Arc::new(Inner {
                validator,
                store,
                state: Mutex::new(State {
                    epoch: 0,
                    session: SessionState::Absent,
                    jar: None,
                    identity: IdentityState::None,
                    selected_key: None,
                    premium: PremiumEvidence::Undeclared,
                    storage: StorageMode::Nothing,
                    problem: Problem::None,
                    validated_at: None,
                }),
                logout_hooks: Mutex::new(Vec::new()),
                observers: Mutex::new(Vec::new()),
                refreshing: AtomicBool::new(false),
            }),
        }
    }

    pub fn view(&self) -> AccountView {
        self.inner.state.lock().unwrap().view()
    }

    /// Called (in registration order) at the start of logout, before any secret is dropped:
    /// stop playback, cancel resolver jobs, clear leases (SEC-03).
    pub fn on_logout(&self, hook: impl Fn() + Send + Sync + 'static) {
        self.inner.logout_hooks.lock().unwrap().push(Box::new(hook));
    }

    pub fn subscribe(&self, observer: impl Fn(&AccountView) + Send + Sync + 'static) {
        self.inner.observers.lock().unwrap().push(Box::new(observer));
    }

    fn notify(&self) {
        let view = self.view();
        for o in self.inner.observers.lock().unwrap().iter() {
            o(&view);
        }
    }

    pub fn declare_premium(&self, declared: bool) {
        self.inner.state.lock().unwrap().premium = if declared {
            PremiumEvidence::OwnerDeclared
        } else {
            PremiumEvidence::Undeclared
        };
        self.notify();
    }

    /// The owner accepted that the session lives in memory only.
    pub fn accept_memory_only(&self) {
        let mut s = self.inner.state.lock().unwrap();
        if s.jar.is_some() {
            s.storage = StorageMode::MemoryOnly;
        }
        drop(s);
        self.notify();
    }

    // ------------------------------------------------------------------ startup

    /// Load a previously stored session and re-validate it with the service.
    pub fn restore(&self) -> RestoreOutcome {
        let Some(store) = &self.inner.store else {
            return RestoreOutcome::NothingStored;
        };
        match store.load() {
            Ok(None) => RestoreOutcome::NothingStored,
            Ok(Some(acc)) => {
                {
                    let mut s = self.inner.state.lock().unwrap();
                    s.epoch += 1;
                    s.jar = Some(acc.jar);
                    s.selected_key = acc.selected_identity;
                    s.session = SessionState::Validating;
                    s.storage = StorageMode::Durable;
                    s.problem = Problem::None;
                }
                self.notify();
                self.revalidate();
                RestoreOutcome::Restored
            }
            Err(StoreError::VaultUnavailable(reason)) => {
                let mut s = self.inner.state.lock().unwrap();
                s.storage = StorageMode::NotPersisted(reason);
                drop(s);
                self.notify();
                RestoreOutcome::VaultUnavailable
            }
            Err(StoreError::Corrupt | StoreError::KeyMissing | StoreError::Io(_)) => {
                store.delete();
                RestoreOutcome::DiscardedUnreadable
            }
        }
    }

    // ------------------------------------------------------------------- import

    /// Validate `jar` with the service and, if it is a live session, install it. A jar is never
    /// stored or used before the service has confirmed it.
    pub fn import(&self, jar: CookieJar, declare_premium: bool) -> ImportOutcome {
        let jar = jar.scoped();
        if !jar.has_auth_cookies() {
            self.set_problem(Problem::NoAuthCookies);
            return ImportOutcome::Rejected(RejectReason::NoAuthCookies);
        }
        let my_epoch = {
            let mut s = self.inner.state.lock().unwrap();
            s.epoch += 1;
            s.session = SessionState::Validating;
            s.problem = Problem::None;
            if declare_premium {
                s.premium = PremiumEvidence::OwnerDeclared;
            }
            s.epoch
        };
        self.notify();

        let verdict = self.inner.validator.validate(&jar); // no lock held
        let outcome = {
            let mut s = self.inner.state.lock().unwrap();
            if s.epoch != my_epoch {
                return ImportOutcome::Superseded;
            }
            match verdict {
                Validation::Valid { identities } => {
                    s.jar = Some(jar);
                    s.selected_key = None;
                    s.session = SessionState::Valid;
                    s.problem = Problem::None;
                    s.validated_at = Some(Instant::now());
                    apply_identities(&mut s, identities);
                    if matches!(s.identity, IdentityState::Selected(_)) {
                        ImportOutcome::Valid
                    } else {
                        ImportOutcome::NeedsIdentityChoice
                    }
                }
                Validation::Expired => {
                    s.session = SessionState::Expired;
                    s.jar = None;
                    s.problem = Problem::None;
                    ImportOutcome::Rejected(RejectReason::Expired)
                }
                Validation::Unreachable | Validation::Unrecognized => {
                    let p = if verdict == Validation::Unreachable {
                        Problem::Network
                    } else {
                        Problem::ServiceChanged
                    };
                    s.jar = Some(jar);
                    s.session = SessionState::Unreachable;
                    s.problem = p;
                    ImportOutcome::Pending(p)
                }
            }
        };
        if matches!(outcome, ImportOutcome::Valid | ImportOutcome::NeedsIdentityChoice) {
            self.persist();
        }
        self.notify();
        outcome
    }

    /// Choose the identity the library and writes belong to. Only a key the service listed is
    /// accepted.
    pub fn select_identity(&self, key: &str) -> bool {
        {
            let mut s = self.inner.state.lock().unwrap();
            let candidates: Vec<Identity> = match &s.identity {
                IdentityState::Ambiguous(v) | IdentityState::Changed(v) => v.clone(),
                IdentityState::Selected(i) => vec![i.clone()],
                IdentityState::None => return false,
            };
            let Some(chosen) = candidates.into_iter().find(|i| i.key() == key) else {
                return false;
            };
            s.selected_key = Some(chosen.key());
            s.identity = IdentityState::Selected(chosen);
            s.epoch += 1; // a different identity: nothing derived from the old one may be reused
        }
        self.persist();
        self.notify();
        true
    }

    // ------------------------------------------------------------- revalidation

    pub fn needs_revalidation(&self) -> bool {
        let s = self.inner.state.lock().unwrap();
        s.jar.is_some()
            && s.session != SessionState::Validating
            && s.validated_at.is_none_or(|t| t.elapsed() >= REVALIDATE_AFTER)
    }

    /// Re-check the current session with the service. Concurrent calls are deduplicated (the
    /// second returns the current state immediately). A network failure keeps the session; only
    /// the service saying "logged out" ends it.
    pub fn revalidate(&self) -> SessionState {
        if self
            .inner
            .refreshing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return self.view().session;
        }
        let result = self.revalidate_inner();
        self.inner.refreshing.store(false, Ordering::SeqCst);
        result
    }

    fn revalidate_inner(&self) -> SessionState {
        let (jar, my_epoch) = {
            let mut s = self.inner.state.lock().unwrap();
            let Some(jar) = s.jar.clone() else {
                return s.session;
            };
            s.session = SessionState::Validating;
            (jar, s.epoch)
        };
        self.notify();
        let verdict = self.inner.validator.validate(&jar);
        let mut delete_stored = false;
        let state = {
            let mut s = self.inner.state.lock().unwrap();
            if s.epoch != my_epoch {
                return s.session; // superseded by logout / import / selection
            }
            match verdict {
                Validation::Valid { identities } => {
                    s.session = SessionState::Valid;
                    s.problem = Problem::None;
                    s.validated_at = Some(Instant::now());
                    let prev = s.selected_key.clone();
                    apply_identities(&mut s, identities);
                    // A previously chosen identity that vanished is a change, not a new default.
                    if let (Some(prev), IdentityState::Selected(i)) = (&prev, &s.identity) {
                        if i.key() != *prev {
                            let all = vec![i.clone()];
                            s.identity = IdentityState::Changed(all);
                        }
                    }
                }
                Validation::Expired => {
                    s.session = SessionState::Expired;
                    s.problem = Problem::None;
                    s.jar = None; // a dead session is not kept
                    s.identity = IdentityState::None;
                    delete_stored = true;
                }
                Validation::Unreachable => {
                    s.session = SessionState::Unreachable;
                    s.problem = Problem::Network;
                }
                Validation::Unrecognized => {
                    s.session = SessionState::Unreachable;
                    s.problem = Problem::ServiceChanged;
                }
            }
            s.session
        };
        // A session that only now got validated (it was pending, or the vault was down) is
        // persisted at this point, never before the service confirmed it.
        if state == SessionState::Valid {
            let needs_store = matches!(
                self.view().storage,
                StorageMode::Nothing | StorageMode::NotPersisted(_)
            );
            if needs_store {
                self.persist();
            }
        }
        if delete_stored {
            if let Some(store) = &self.inner.store {
                store.delete();
            }
            let mut s = self.inner.state.lock().unwrap();
            if s.epoch == my_epoch {
                s.storage = StorageMode::Nothing;
            }
        }
        self.notify();
        state
    }

    // ------------------------------------------------------------------- logout

    /// SEC-03: stop playback and cancel resolver work (hooks), drop in-memory cookies, delete
    /// the vault key and ciphertext. Writes already accepted by the remote service cannot be
    /// undone locally; the caller reports that separately.
    pub fn logout(&self) -> LogoutReport {
        let hooks_run = {
            let hooks = self.inner.logout_hooks.lock().unwrap();
            for h in hooks.iter() {
                h();
            }
            hooks.len()
        };
        let was_logged_in = {
            let mut s = self.inner.state.lock().unwrap();
            let was = s.jar.is_some();
            s.epoch += 1; // in-flight validations and resolver snapshots are now stale
            s.jar = None; // zeroed on drop
            s.identity = IdentityState::None;
            s.selected_key = None;
            s.session = SessionState::Absent;
            s.premium = PremiumEvidence::Undeclared;
            s.problem = Problem::None;
            s.validated_at = None;
            s.storage = StorageMode::Nothing;
            was
        };
        let stored = match &self.inner.store {
            Some(store) => store.delete(),
            None => DeleteReport {
                key_deleted: true,
                ciphertext_deleted: true,
            },
        };
        self.notify();
        LogoutReport {
            hooks_run,
            stored,
            was_logged_in,
        }
    }

    // ------------------------------------------------------------------ internals

    fn set_problem(&self, p: Problem) {
        self.inner.state.lock().unwrap().problem = p;
        self.notify();
    }

    /// Write the current jar + selected identity. Failure never downgrades to plaintext: the
    /// session simply stays memory-only and the view says why.
    fn persist(&self) {
        let Some(store) = &self.inner.store else {
            let mut s = self.inner.state.lock().unwrap();
            if s.jar.is_some() && s.storage != StorageMode::MemoryOnly {
                s.storage = StorageMode::MemoryOnly;
            }
            return;
        };
        let acc = {
            let s = self.inner.state.lock().unwrap();
            let Some(jar) = s.jar.clone() else { return };
            StoredAccount {
                jar,
                selected_identity: s.selected_key.clone(),
            }
        };
        let result = store.save(&acc);
        let mut s = self.inner.state.lock().unwrap();
        s.storage = match result {
            Ok(()) => StorageMode::Durable,
            Err(e) => match s.storage {
                StorageMode::MemoryOnly => StorageMode::MemoryOnly,
                _ => StorageMode::NotPersisted(e.to_string()),
            },
        };
    }
}

fn apply_identities(s: &mut State, identities: Vec<Identity>) {
    // The owner's earlier explicit choice wins when the service still lists it.
    if let Some(key) = &s.selected_key {
        if let Some(i) = identities.iter().find(|i| i.key() == *key) {
            s.identity = IdentityState::Selected(i.clone());
            return;
        }
    }
    s.identity = match identities.len() {
        0 => IdentityState::None,
        1 => {
            let only = identities.into_iter().next().unwrap();
            s.selected_key = Some(only.key());
            IdentityState::Selected(only)
        }
        _ => IdentityState::Ambiguous(identities),
    };
}

/// The resolver's window onto the account: a snapshot only while the session is valid and an
/// identity is selected (no guest fallback), and epoch-guarded rotation merging (TEST-15).
impl SessionProvider for AccountService {
    fn snapshot(&self) -> Option<SessionSnapshot> {
        let s = self.inner.state.lock().unwrap();
        if s.session != SessionState::Valid || !matches!(s.identity, IdentityState::Selected(_)) {
            return None;
        }
        let jar = s.jar.as_ref()?;
        Some(SessionSnapshot {
            epoch: s.epoch,
            cookies_netscape: jar.to_netscape(),
        })
    }

    fn offer_rotation(&self, epoch: u64, cookies_netscape: SecretString) -> bool {
        let updated = CookieJar::from_netscape(cookies_netscape.expose());
        {
            let mut s = self.inner.state.lock().unwrap();
            if s.epoch != epoch || s.session != SessionState::Valid {
                return false; // a newer jar exists (or the session ended): the old run loses
            }
            let Some(jar) = s.jar.as_mut() else { return false };
            jar.merge_rotation(&updated);
            s.epoch += 1;
        }
        self.persist();
        true
    }
}
