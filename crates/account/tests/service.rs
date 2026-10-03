//! Account service tests with a scripted validator (no network) and in-memory / failing vaults,
//! over the real encrypted store on disk.

use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use apollo_account::identity::{AccountValidator, Identity, Validation};
use apollo_account::jar::{Cookie, CookieJar};
use apollo_account::service::{
    AccountService, IdentityState, ImportOutcome, PremiumEvidence, Problem, RejectReason,
    RestoreOutcome, SessionState, StorageMode,
};
use apollo_account::store::EncryptedStore;
use apollo_account::vault::{MemoryVault, UnavailableVault, Vault};
use apollo_services_common::{random_hex, SecretString, SessionProvider, SessionUnavailable};

const CANARY: &str = "COOKIECANARY-abcdef0123456789";

// ------------------------------------------------------------------------ helpers

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
            cookie("SAPISID", ".youtube.com", CANARY),
            cookie("__Secure-3PAPISID", ".youtube.com", "v3papisid"),
            cookie("SID", ".google.com", "vsid"),
            cookie("__Secure-3PSID", ".google.com", "v3psid"),
            cookie("tracker", ".example.com", "should-be-dropped"),
        ],
        ephemeral_seed: false,
    }
}

fn ident(n: usize) -> Identity {
    Identity {
        name: format!("Channel {n}"),
        handle: Some(format!("@c{n}")),
        page_id: Some(format!("PAGE{n}")),
        selected_by_service: n == 0,
    }
}

fn valid(n: usize) -> Validation {
    Validation::Valid {
        identities: (0..n).map(ident).collect(),
    }
}

struct Scripted {
    results: Mutex<VecDeque<Validation>>,
    calls: AtomicUsize,
    gate: Mutex<Option<(Sender<()>, Receiver<()>)>>,
}

impl Scripted {
    fn new(results: Vec<Validation>) -> Arc<Self> {
        Arc::new(Self {
            results: Mutex::new(results.into()),
            calls: AtomicUsize::new(0),
            gate: Mutex::new(None),
        })
    }
    fn push(&self, v: Validation) {
        self.results.lock().unwrap().push_back(v);
    }
    /// The next validate() call announces itself, then blocks until released.
    fn arm_gate(&self) -> (Receiver<()>, Sender<()>) {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *self.gate.lock().unwrap() = Some((entered_tx, release_rx));
        (entered_rx, release_tx)
    }
}

impl AccountValidator for Scripted {
    fn validate(&self, _jar: &CookieJar) -> Validation {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let gate = self.gate.lock().unwrap().take();
        if let Some((entered, release)) = gate {
            let _ = entered.send(());
            let _ = release.recv_timeout(Duration::from_secs(10));
        }
        self.results.lock().unwrap().pop_front().unwrap_or(Validation::Unreachable)
    }
}

struct Env {
    dir: PathBuf,
    vault: Arc<MemoryVault>,
}

impl Env {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("apollo-account-test-{}", random_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        Self {
            dir,
            vault: Arc::new(MemoryVault::new()),
        }
    }
    fn store(&self) -> EncryptedStore {
        EncryptedStore::new(self.vault.clone() as Arc<dyn Vault>, &self.dir).unwrap()
    }
    fn service(&self, v: &Arc<Scripted>) -> AccountService {
        AccountService::new(v.clone() as Arc<dyn AccountValidator>, Some(self.store()))
    }
    fn files(&self) -> usize {
        fs::read_dir(&self.dir).map(|r| r.count()).unwrap_or(0)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

// -------------------------------------------------------------------------- tests

// TEST-01: valid session -> identity shown, content gated on session + identity + declaration.
#[test]
fn valid_import_binds_identity_and_gates_content_on_all_three_concepts() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    assert!(svc.snapshot().is_none(), "no session yet");
    assert_eq!(svc.import(jar(), false), ImportOutcome::Valid);

    let view = svc.view();
    assert_eq!(view.session, SessionState::Valid);
    assert!(matches!(view.identity, IdentityState::Selected(_)));
    assert_eq!(view.storage, StorageMode::Durable);
    // Valid session + identity, but Premium not declared: content stays disabled, and the
    // wording never claims verification.
    assert_eq!(view.premium, PremiumEvidence::Undeclared);
    assert!(!view.can_use_content());
    assert_eq!(view.premium_label(), "Premium not declared");
    svc.declare_premium(true);
    let view = svc.view();
    assert!(view.can_use_content());
    assert_eq!(view.premium_label(), "Premium assumed for this account");
    // Out-of-scope cookies never entered the jar the resolver sees.
    let snap = svc.snapshot().expect("snapshot while valid");
    assert!(snap.cookies_netscape.expose().contains("SAPISID"));
    assert!(!snap.cookies_netscape.expose().contains("tracker"));
}

#[test]
fn a_jar_without_auth_cookies_is_rejected_without_asking_the_service() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    let weak = CookieJar {
        cookies: vec![cookie("PREF", ".youtube.com", "x"), cookie("SID", ".google.com", "y")],
        ephemeral_seed: false,
    };
    assert_eq!(svc.import(weak, true), ImportOutcome::Rejected(RejectReason::NoAuthCookies));
    assert_eq!(v.calls.load(Ordering::SeqCst), 0);
    assert_eq!(svc.view().problem, Problem::NoAuthCookies);
    assert!(svc.snapshot().is_none());
}

// TEST-02: an expired/negative session never yields a snapshot and is never stored.
#[test]
fn expired_import_is_rejected_and_nothing_is_stored_or_resolvable() {
    let env = Env::new();
    let v = Scripted::new(vec![Validation::Expired]);
    let svc = env.service(&v);
    assert_eq!(svc.import(jar(), true), ImportOutcome::Rejected(RejectReason::Expired));
    assert_eq!(svc.view().session, SessionState::Expired);
    assert!(svc.snapshot().is_none(), "no guest fallback");
    assert_eq!(env.files(), 0);
    assert!(env.vault.get("jar-key-v1").unwrap().is_none());
}

// AUTH-04: unknown is not "expired", and nothing unvalidated is persisted.
#[test]
fn unreachable_import_is_pending_unstored_and_later_confirmed() {
    let env = Env::new();
    let v = Scripted::new(vec![Validation::Unreachable]);
    let svc = env.service(&v);
    assert_eq!(svc.import(jar(), true), ImportOutcome::Pending(Problem::Network));
    assert_eq!(svc.view().session, SessionState::Unreachable);
    assert!(svc.snapshot().is_none(), "unvalidated session must not reach the resolver");
    assert_eq!(env.files(), 0, "nothing unvalidated is persisted");

    v.push(valid(1));
    assert_eq!(svc.revalidate(), SessionState::Valid);
    assert!(svc.snapshot().is_some());
    assert_eq!(svc.view().storage, StorageMode::Durable);
    assert_eq!(env.files(), 1, "persisted once the service confirmed it");

    // An unrecognised answer is a compatibility problem, not a login.
    let v2 = Scripted::new(vec![Validation::Unrecognized]);
    let svc2 = Env::new().service(&v2);
    assert_eq!(svc2.import(jar(), true), ImportOutcome::Pending(Problem::ServiceChanged));
    assert!(svc2.snapshot().is_none());
}

// TEST-03: ambiguous identity blocks until the owner chooses; the choice survives restart.
#[test]
fn ambiguous_identity_blocks_until_an_explicit_choice_and_the_choice_persists() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(3)]);
    let svc = env.service(&v);
    svc.declare_premium(true);
    assert_eq!(svc.import(jar(), true), ImportOutcome::NeedsIdentityChoice);
    assert!(matches!(svc.view().identity, IdentityState::Ambiguous(ref l) if l.len() == 3));
    assert!(!svc.view().can_use_content());
    assert!(svc.snapshot().is_none(), "resolver is blocked until an identity is chosen");
    assert!(!svc.select_identity("PAGE-NOT-LISTED"), "only listed identities are accepted");

    let epoch_before = svc.view().epoch;
    assert!(svc.select_identity("PAGE2"));
    assert!(svc.view().epoch > epoch_before, "a new identity invalidates old snapshots");
    assert!(svc.view().can_use_content());
    assert!(svc.snapshot().is_some());

    // New process: restore + revalidate keeps the owner's earlier choice, not the service default.
    let v2 = Scripted::new(vec![valid(3)]);
    let svc2 = env.service(&v2);
    assert_eq!(svc2.restore(), RestoreOutcome::Restored);
    let IdentityState::Selected(i) = svc2.view().identity else { panic!("{:?}", svc2.view()) };
    assert_eq!(i.key(), "PAGE2");
}

// TEST-04 / SEC-03: logout order and completeness.
#[test]
fn logout_runs_hooks_first_then_drops_secrets_and_deletes_storage() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    svc.import(jar(), true);
    assert_eq!(env.files(), 1);

    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let (o1, s1) = (order.clone(), svc.clone());
    svc.on_logout(move || {
        // Hooks run BEFORE secrets are dropped: the snapshot still exists at this point.
        o1.lock().unwrap().push(format!("hook1 snapshot={}", s1.snapshot().is_some()));
    });
    let o2 = order.clone();
    svc.on_logout(move || o2.lock().unwrap().push("hook2".into()));

    let report = svc.logout();
    assert_eq!(report.hooks_run, 2);
    assert!(report.was_logged_in);
    assert!(report.stored.key_deleted && report.stored.ciphertext_deleted);
    assert_eq!(*order.lock().unwrap(), vec!["hook1 snapshot=true", "hook2"]);

    let view = svc.view();
    assert_eq!(view.session, SessionState::Absent);
    assert!(matches!(view.identity, IdentityState::None));
    assert!(!view.can_use_content());
    assert!(svc.snapshot().is_none());
    assert_eq!(env.files(), 0, "ciphertext removed");
    assert!(env.vault.get("jar-key-v1").unwrap().is_none(), "vault key removed");
    // Premium declaration belongs to the account that left.
    assert_eq!(view.premium, PremiumEvidence::Undeclared);
}

#[test]
fn logout_during_validation_wins_and_the_late_result_is_discarded() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    let (entered, release) = v.arm_gate();
    let svc2 = svc.clone();
    let t = thread::spawn(move || svc2.import(jar(), true));
    entered.recv_timeout(Duration::from_secs(5)).expect("validation started");
    svc.logout(); // while the service call is "in flight"
    release.send(()).unwrap();
    assert_eq!(t.join().unwrap(), ImportOutcome::Superseded);
    assert_eq!(svc.view().session, SessionState::Absent);
    assert!(svc.snapshot().is_none(), "a result that arrived after logout must not log us back in");
    assert_eq!(env.files(), 0);
}

// Concurrent refreshes are deduplicated.
#[test]
fn concurrent_revalidations_make_one_service_call() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1), valid(1)]);
    let svc = env.service(&v);
    svc.import(jar(), true);
    let calls_before = v.calls.load(Ordering::SeqCst);
    let (entered, release) = v.arm_gate();
    let a = svc.clone();
    let t = thread::spawn(move || a.revalidate());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    // A second request while the first is in flight returns immediately without calling out.
    let state = svc.revalidate();
    assert_eq!(state, SessionState::Validating);
    release.send(()).unwrap();
    assert_eq!(t.join().unwrap(), SessionState::Valid);
    assert_eq!(v.calls.load(Ordering::SeqCst), calls_before + 1);
}

// Restore / restart behaviour.
#[test]
fn restore_revalidates_and_a_network_failure_is_not_a_logout() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    env.service(&v).import(jar(), true);

    // Restart while offline: the session is kept (Unreachable), nothing is deleted.
    let offline = Scripted::new(vec![Validation::Unreachable]);
    let svc = env.service(&offline);
    assert_eq!(svc.restore(), RestoreOutcome::Restored);
    assert_eq!(svc.view().session, SessionState::Unreachable);
    assert!(svc.snapshot().is_none());
    assert_eq!(env.files(), 1, "an unreachable service must not delete the session");

    // Back online: valid again.
    offline.push(valid(1));
    assert_eq!(svc.revalidate(), SessionState::Valid);
    assert!(svc.snapshot().is_some());

    // The service later says the session is dead: blocked, stored data removed, re-import asked.
    let dead = Scripted::new(vec![Validation::Expired]);
    let svc = env.service(&dead);
    assert_eq!(svc.restore(), RestoreOutcome::Restored);
    assert_eq!(svc.view().session, SessionState::Expired);
    assert!(svc.snapshot().is_none());
    assert_eq!(env.files(), 0);
    assert!(env.vault.get("jar-key-v1").unwrap().is_none());
}

// TEST-16: vault unavailable: memory-only offer, no durable plaintext, safe on restart.
#[test]
fn unavailable_vault_never_produces_a_plaintext_file() {
    let dir = std::env::temp_dir().join(format!("apollo-account-test-{}", random_hex(6)));
    fs::create_dir_all(&dir).unwrap();
    let store = EncryptedStore::new(Arc::new(UnavailableVault("locked".into())), &dir).unwrap();
    let v = Scripted::new(vec![valid(1)]);
    let svc = AccountService::new(v.clone(), Some(store));
    svc.declare_premium(true);
    assert_eq!(svc.import(jar(), true), ImportOutcome::Valid, "usable in memory");
    assert!(matches!(svc.view().storage, StorageMode::NotPersisted(_)), "and says so");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0, "nothing durable of any kind");
    svc.accept_memory_only();
    assert_eq!(svc.view().storage, StorageMode::MemoryOnly);

    // Restart with the vault still unavailable: told plainly, nothing invented.
    let svc2 = AccountService::new(
        Scripted::new(vec![]),
        Some(EncryptedStore::new(Arc::new(UnavailableVault("locked".into())), &dir).unwrap()),
    );
    assert_eq!(svc2.restore(), RestoreOutcome::NothingStored); // no file, nothing to read
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn unreadable_stored_data_is_discarded_safely() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    env.service(&v).import(jar(), true);
    let path = env.dir.join("session.v1");
    let mut raw = fs::read(&path).unwrap();
    let n = raw.len();
    raw[n - 3] ^= 0xff;
    fs::write(&path, raw).unwrap();
    let svc = env.service(&Scripted::new(vec![]));
    assert_eq!(svc.restore(), RestoreOutcome::DiscardedUnreadable);
    assert_eq!(svc.view().session, SessionState::Absent);
    assert!(!path.exists());
}

// TEST-15: rotation merges only under the current epoch and persists across restart.
#[test]
fn rotation_is_epoch_guarded_and_persisted() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    svc.import(jar(), true);
    let snap = svc.snapshot().unwrap();

    let mut rotated = jar();
    rotated.cookies[0].value = "ROTATED-VALUE".into();
    rotated.cookies.push(cookie("evil", ".example.com", "no"));
    let text = rotated.to_netscape();
    assert!(svc.offer_rotation(snap.epoch, SecretString::new(text.expose())));
    // The same (now stale) epoch cannot overwrite the newer jar again.
    let mut older = jar();
    older.cookies[0].value = "OLDER-SNAPSHOT-VALUE".into();
    assert!(!svc.offer_rotation(snap.epoch, SecretString::new(older.to_netscape().expose())));
    let now = svc.snapshot().unwrap().cookies_netscape;
    assert!(now.expose().contains("ROTATED-VALUE"));
    assert!(!now.expose().contains("OLDER-SNAPSHOT-VALUE"));
    assert!(!now.expose().contains("evil"), "out-of-scope cookies from a helper are ignored");

    // Survives a restart.
    let svc2 = env.service(&Scripted::new(vec![valid(1)]));
    svc2.restore();
    assert!(svc2.snapshot().unwrap().cookies_netscape.expose().contains("ROTATED-VALUE"));

    // After logout, an old helper's rotation can never resurrect cookies.
    svc.logout();
    assert!(!svc.offer_rotation(svc.view().epoch, SecretString::new(rotated.to_netscape().expose())));
    assert!(svc.snapshot().is_none());
}

#[test]
fn a_changed_identity_blocks_until_reconfirmed() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(2)]);
    let svc = env.service(&v);
    svc.declare_premium(true);
    svc.import(jar(), true);
    assert!(svc.select_identity("PAGE1"));
    assert!(svc.view().can_use_content());

    // The service now lists only another identity: a change, not a silent switch.
    v.push(Validation::Valid { identities: vec![ident(0)] });
    svc.revalidate();
    assert!(matches!(svc.view().identity, IdentityState::Changed(_)), "{:?}", svc.view());
    assert!(!svc.view().can_use_content());
    assert!(svc.snapshot().is_none(), "writes and resolves must not reach a different library");
    assert!(svc.select_identity("PAGE0"));
    assert!(svc.view().can_use_content());
}

// TEST-23 (account half): no canary anywhere it should not be.
#[test]
fn canary_values_never_leak() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s2 = seen.clone();
    svc.subscribe(move |view| s2.lock().unwrap().push(format!("{view:?}")));
    let outcome = svc.import(jar(), true);
    let mut surfaces = vec![
        format!("{outcome:?}"),
        format!("{:?}", svc.view()),
        format!("{svc:?}"),
        format!("{:?}", svc.snapshot()),
        format!("{:?}", jar()),
    ];
    surfaces.extend(seen.lock().unwrap().iter().cloned());
    surfaces.push(format!("{:?}", svc.logout()));
    for s in &surfaces {
        assert!(!s.contains(CANARY), "cookie value leaked into Debug output: {}", &s[..s.len().min(60)]);
    }
    // On disk: encrypted, so neither the value nor the cookie names appear (checked while stored).
    let env2 = Env::new();
    let svc = env2.service(&Scripted::new(vec![valid(1)]));
    svc.import(jar(), true);
    let raw = fs::read(env2.dir.join("session.v1")).unwrap();
    let text = String::from_utf8_lossy(&raw).to_string();
    assert!(!text.contains(CANARY) && !text.contains("SAPISID"), "plaintext found on disk");
}

// TEST-16 (restart half): ciphertext exists but the vault is locked/unavailable. We say so, keep
// the file (nothing is destroyed because of a temporary condition) and never prompt for plaintext.
#[test]
fn restart_with_a_locked_vault_keeps_the_ciphertext_and_reports_it() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    env.service(&v).import(jar(), true);
    assert_eq!(env.files(), 1);

    let locked = EncryptedStore::new(Arc::new(UnavailableVault("locked".into())), &env.dir).unwrap();
    let svc = AccountService::new(Scripted::new(vec![]), Some(locked));
    assert_eq!(svc.restore(), RestoreOutcome::VaultUnavailable);
    assert_eq!(svc.view().session, SessionState::Absent);
    assert!(matches!(svc.view().storage, StorageMode::NotPersisted(_)));
    assert!(svc.snapshot().is_none());
    assert_eq!(env.files(), 1, "a temporarily locked vault must not delete the stored session");

    // Once the vault is back, the same stored session restores.
    let svc = env.service(&Scripted::new(vec![valid(1)]));
    assert_eq!(svc.restore(), RestoreOutcome::Restored);
    assert_eq!(svc.view().session, SessionState::Valid);
}

// AUTH-04: only the service saying "logged out" ends a confirmed session. While it is being
// re-checked, or the service is unreachable, the resolver keeps working with the same cookies.
#[test]
fn a_confirmed_session_rides_through_validating_and_unreachable() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(1)]);
    let svc = env.service(&v);
    svc.import(jar(), true);
    assert!(svc.snapshot().is_some());

    let (entered, release) = v.arm_gate();
    let a = svc.clone();
    v.push(Validation::Unreachable);
    let t = thread::spawn(move || a.revalidate());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(svc.view().session, SessionState::Validating);
    assert!(svc.snapshot().is_some(), "a re-check in flight must not block playback");
    release.send(()).unwrap();
    assert_eq!(t.join().unwrap(), SessionState::Unreachable);
    assert!(svc.snapshot().is_some(), "one failed check is not a logout");
    assert_eq!(svc.view().problem, Problem::Network);

    // The service later says it is dead: now (and only now) work is blocked.
    v.push(Validation::Expired);
    assert_eq!(svc.revalidate(), SessionState::Expired);
    assert!(svc.snapshot().is_none());
}

// yt-dlp rewrites the cookie file on essentially every run, so rotations are frequent. A rotation
// (or an identity choice) landing during a re-check must not discard the check's result and leave
// the session stuck in Validating.
#[test]
fn rotation_or_identity_choice_during_revalidation_does_not_strand_the_session() {
    let env = Env::new();
    let v = Scripted::new(vec![valid(2)]);
    let svc = env.service(&v);
    svc.import(jar(), true);
    assert!(svc.select_identity("PAGE0"));

    let (entered, release) = v.arm_gate();
    v.push(valid(2));
    let a = svc.clone();
    let t = thread::spawn(move || a.revalidate());
    entered.recv_timeout(Duration::from_secs(5)).unwrap();

    // While the check is in flight: a helper rotates cookies, and the owner re-selects.
    let snap = svc.snapshot().expect("usable during the check");
    let mut rotated = jar();
    rotated.cookies[0].value = "ROTATED-DURING-CHECK".into();
    assert!(svc.offer_rotation(snap.epoch, SecretString::new(rotated.to_netscape().expose())));
    assert!(svc.select_identity("PAGE1"));

    release.send(()).unwrap();
    assert_eq!(t.join().unwrap(), SessionState::Valid, "the check still completes");
    let view = svc.view();
    assert_eq!(view.session, SessionState::Valid, "not stuck in Validating");
    assert!(matches!(view.identity, IdentityState::Selected(ref i) if i.key() == "PAGE1"));
    assert!(svc.snapshot().unwrap().cookies_netscape.expose().contains("ROTATED-DURING-CHECK"));
    // The older snapshot cannot overwrite it (epoch guard still works).
    assert!(!svc.offer_rotation(snap.epoch, SecretString::new(jar().to_netscape().expose())));
}

// The resolver reports different failure classes for "sign in again" and "not confirmed yet".
#[test]
fn unconfirmed_and_no_session_are_distinguished_for_the_resolver() {
    let env = Env::new();
    let v = Scripted::new(vec![Validation::Unreachable]);
    let svc = env.service(&v);
    assert_eq!(svc.unavailable_reason(), SessionUnavailable::NoSession, "nothing imported yet");
    svc.import(jar(), true);
    assert!(svc.snapshot().is_none(), "never confirmed: not usable");
    assert_eq!(svc.unavailable_reason(), SessionUnavailable::Unconfirmed);
    svc.logout();
    assert_eq!(svc.unavailable_reason(), SessionUnavailable::NoSession);
}
