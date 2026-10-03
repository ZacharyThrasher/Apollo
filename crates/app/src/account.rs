//! The account as the UI sees it: what to show, and the actions behind the sign-in screen.
//!
//! The account service owns the truth (session, identity, Premium evidence, storage). This model
//! adds only what the UI needs on top: which sign-in is running, the last thing that went wrong
//! and whether the startup check is still in flight. Everything blocking (the WebView2 helper,
//! cookie imports, the validation call) runs on the background executor; the service reports
//! its changes through a channel, so the UI never calls into it while it is busy.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use apollo_account::identity::Identity;
use apollo_account::import::{self, ImportError};
use apollo_account::jar::CookieJar;
use apollo_account::service::{
    AccountService, AccountView, IdentityState, ImportOutcome, PremiumEvidence, Problem,
    RejectReason, SessionState, StorageMode,
};
use gpui::{Context, RenderImage, SharedString};

use crate::art::DecodedArt;

/// Where a session can come from besides the cookie file and clipboard (which need nothing
/// platform specific). Both calls block until they are done.
pub trait SessionSource: Send + Sync {
    /// The WebView2 sign-in window. Returns when the owner finishes, closes it or it is cancelled.
    fn browser(&self) -> Result<CookieJar, SignInError>;
    /// Ends a running [`SessionSource::browser`] call.
    fn cancel_browser(&self);
    /// The cookies of the default Firefox profile.
    fn firefox(&self) -> Result<CookieJar, SignInError> {
        firefox_jar()
    }
}

/// Downloads a channel picture. Blocking; runs on the background executor.
pub trait AvatarSource: Send + Sync {
    fn fetch(&self, url: &str) -> Option<DecodedArt>;
}

/// No pictures: every account shows its initial.
pub struct NoAvatars;

impl AvatarSource for NoAvatars {
    fn fetch(&self, _: &str) -> Option<DecodedArt> {
        None
    }
}

/// The service plus the platform's way of getting a session into it.
#[derive(Clone)]
pub struct AccountPorts {
    pub service: AccountService,
    pub source: Arc<dyn SessionSource>,
    pub avatars: Arc<dyn AvatarSource>,
    /// Set when account storage could not be opened at startup; shown as a short error.
    pub startup_error: Option<SignInError>,
}

/// Which sign-in is running (decides where the spinner shows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Browser,
    Firefox,
    File,
    Clipboard,
}

/// Why a sign-in did not produce a session. Each has a line of at most six words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignInError {
    /// The owner closed the window or cancelled. Not an error; never shown.
    Cancelled,
    BrowserUnavailable,
    TimedOut,
    NoFirefox,
    NoFirefoxCookies,
    FirefoxUnreadable,
    FileUnreadable,
    NoFileCookies,
    ClipboardEmpty,
    NoClipboardCookies,
    NoAuthCookies,
    Expired,
    NoAccount,
    Storage,
}

impl SignInError {
    pub fn text(self) -> &'static str {
        match self {
            SignInError::Cancelled => "",
            SignInError::BrowserUnavailable => "Couldn't open sign-in",
            SignInError::TimedOut => "Sign-in timed out",
            SignInError::NoFirefox => "Firefox not found",
            SignInError::NoFirefoxCookies => "No YouTube cookies in Firefox",
            SignInError::FirefoxUnreadable => "Couldn't read Firefox cookies",
            SignInError::FileUnreadable => "Couldn't read that file",
            SignInError::NoFileCookies => "No YouTube cookies in file",
            SignInError::ClipboardEmpty => "Nothing on the clipboard",
            SignInError::NoClipboardCookies => "No YouTube cookies on clipboard",
            SignInError::NoAuthCookies => "Not signed in",
            SignInError::Expired => "Session expired",
            SignInError::NoAccount => "No account found",
            SignInError::Storage => "Couldn't open account storage",
        }
    }
}

/// The default Firefox profile's cookies. Only the profile list is read until one is chosen.
/// Windows only (the import reads Firefox's database through the account crate's Windows file
/// handling); elsewhere there is no Firefox to find.
pub fn firefox_jar() -> Result<CookieJar, SignInError> {
    #[cfg(windows)]
    {
        let dir = std::env::var_os("APPDATA")
            .map(|d| PathBuf::from(d).join("Mozilla").join("Firefox"))
            .ok_or(SignInError::NoFirefox)?;
        let profiles = import::firefox_profiles(&dir);
        let profile = profiles
            .iter()
            .find(|p| p.is_default)
            .or(profiles.first())
            .ok_or(SignInError::NoFirefox)?;
        import::firefox_profile(&profile.path).map_err(|e| match e {
            ImportError::NoRelevantCookies => SignInError::NoFirefoxCookies,
            _ => SignInError::FirefoxUnreadable,
        })
    }
    #[cfg(not(windows))]
    Err(SignInError::NoFirefox)
}

/// A cookie file chosen by the owner.
pub fn file_jar(path: &Path) -> Result<CookieJar, SignInError> {
    import::netscape_file(path).map_err(|e| match e {
        ImportError::NoRelevantCookies => SignInError::NoFileCookies,
        _ => SignInError::FileUnreadable,
    })
}

/// A `Cookie:` header on the clipboard.
pub fn clipboard_jar(text: Option<&str>) -> Result<CookieJar, SignInError> {
    let text = text.map(str::trim).filter(|t| !t.is_empty());
    let Some(text) = text else {
        return Err(SignInError::ClipboardEmpty);
    };
    import::cookie_header(text).map_err(|e| match e {
        ImportError::NoRelevantCookies => SignInError::NoClipboardCookies,
        _ => SignInError::ClipboardEmpty,
    })
}

/// A channel the account page can show and pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Who {
    pub key: String,
    pub name: SharedString,
    pub handle: Option<SharedString>,
    /// The channel picture, when the service gave one; the page looks the image up by this.
    pub avatar_url: Option<String>,
}

impl From<&Identity> for Who {
    fn from(i: &Identity) -> Self {
        Who {
            key: i.key(),
            name: i.name.clone().into(),
            handle: i.handle.clone().map(Into::into),
            avatar_url: i.avatar_url.clone(),
        }
    }
}

/// A short line under the signed-in account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    /// The last check could not reach the service. The session is kept; Retry re-checks.
    CantReach,
    /// The service answered in a shape the app does not understand.
    CantVerify,
    /// The credential vault could not be used, so the session ends with the app.
    NotSaved,
}

impl Notice {
    pub fn text(self) -> &'static str {
        match self {
            Notice::CantReach => "Couldn't reach YouTube Music",
            Notice::CantVerify => "Couldn't verify the account",
            Notice::NotSaved => "Won't stay signed in",
        }
    }

    /// Whether Retry makes sense.
    pub fn retryable(self) -> bool {
        !matches!(self, Notice::NotSaved)
    }
}

/// What the account page shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The startup check is still running.
    Checking,
    SignedOut {
        error: Option<SignInError>,
    },
    SignedIn {
        who: Who,
        /// "Premium" once the app's own account check has confirmed it; `None` shows nothing.
        premium: Option<&'static str>,
        notice: Option<Notice>,
    },
    /// The login can act as several channels; the owner picks one.
    Choose(Vec<Who>),
    /// A jar is held but could not be confirmed yet and there is no identity to show.
    CantConfirm(Notice),
}

/// What the Premium slot shows. The account service only has the owner's declaration (there is
/// no entitlement endpoint to ask), and hedged text such as "Premium assumed" is not shown, so
/// the slot stays empty until the account check can confirm Premium.
pub fn premium_text(p: PremiumEvidence) -> Option<&'static str> {
    match p {
        PremiumEvidence::OwnerDeclared | PremiumEvidence::Undeclared => None,
    }
}

fn notice_for(view: &AccountView) -> Option<Notice> {
    match (view.session, view.problem) {
        (SessionState::Unreachable, Problem::ServiceChanged) => Some(Notice::CantVerify),
        (SessionState::Unreachable, _) => Some(Notice::CantReach),
        _ if matches!(view.storage, StorageMode::NotPersisted(_)) => Some(Notice::NotSaved),
        _ => None,
    }
}

/// Pure mapping from the service's view to what is drawn.
pub fn screen(
    view: &AccountView,
    checking: bool,
    signing_in: bool,
    error: Option<SignInError>,
) -> Screen {
    if checking {
        return Screen::Checking;
    }
    match (&view.identity, view.session) {
        (IdentityState::Selected(who), _) => Screen::SignedIn {
            who: who.into(),
            premium: premium_text(view.premium),
            notice: notice_for(view),
        },
        (IdentityState::Ambiguous(list) | IdentityState::Changed(list), _) => {
            Screen::Choose(list.iter().map(Who::from).collect())
        }
        // Restore or revalidation of a session we hold no identity for yet.
        (IdentityState::None, SessionState::Validating) if !signing_in => Screen::Checking,
        (IdentityState::None, SessionState::Unreachable) if !signing_in => {
            Screen::CantConfirm(notice_for(view).unwrap_or(Notice::CantReach))
        }
        (IdentityState::None, session) => Screen::SignedOut {
            error: error.or(match session {
                SessionState::Expired => Some(SignInError::Expired),
                SessionState::Valid => Some(SignInError::NoAccount),
                _ => None,
            }),
        },
    }
}

pub struct AccountModel {
    service: AccountService,
    source: Arc<dyn SessionSource>,
    view: AccountView,
    /// True from launch until the startup restore has finished.
    checking: bool,
    pending: Option<Method>,
    error: Option<SignInError>,
    avatar_source: Arc<dyn AvatarSource>,
    avatars: HashMap<String, AvatarSlot>,
}

enum AvatarSlot {
    Loading,
    /// `None`: the picture could not be fetched or decoded; the initial stays.
    Ready(Option<Arc<RenderImage>>),
}

impl AccountModel {
    pub fn new(ports: AccountPorts, cx: &mut Context<Self>) -> Self {
        // The service reports from whatever thread changed it; a channel hands that over to the
        // UI thread without ever blocking either side.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        ports.service.subscribe(move |_| {
            let _ = tx.send(());
        });
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_some() {
                while rx.try_recv().is_ok() {}
                if this.update(cx, |m, cx| m.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            view: ports.service.view(),
            service: ports.service,
            source: ports.source,
            checking: true,
            pending: None,
            error: ports.startup_error,
            avatar_source: ports.avatars,
            avatars: HashMap::new(),
        }
    }

    /// The service this model drives (the resolver reads the session from the same one).
    pub fn service(&self) -> &AccountService {
        &self.service
    }

    pub fn screen(&self) -> Screen {
        screen(
            &self.view,
            self.checking,
            self.pending.is_some(),
            self.error,
        )
    }

    /// Which sign-in is running, if any.
    pub fn busy(&self) -> Option<Method> {
        self.pending
    }

    /// Why the owner has to do something about the account for content to work (sign in again
    /// or pick a channel), as the accessible name of the dot on the Settings item.
    pub fn attention(&self) -> Option<&'static str> {
        match self.screen() {
            Screen::SignedOut {
                error: Some(SignInError::Expired),
            } => Some("Session expired"),
            Screen::Choose(_) => Some("Choose account"),
            _ => None,
        }
    }

    /// The decoded picture for `url`, once it has been fetched.
    pub fn avatar(&self, url: &str) -> Option<Arc<RenderImage>> {
        match self.avatars.get(url) {
            Some(AvatarSlot::Ready(image)) => image.clone(),
            _ => None,
        }
    }

    /// True once the startup check is over and nobody is signed in.
    pub fn is_signed_out(&self) -> bool {
        !self.checking && matches!(self.screen(), Screen::SignedOut { .. })
    }

    pub fn startup_finished(&mut self, cx: &mut Context<Self>) {
        self.checking = false;
        self.refresh(cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.view = self.service.view();
        self.load_avatars(cx);
        cx.notify();
    }

    /// Fetches the pictures of the accounts on show, once each. Nothing is kept once no
    /// account is held.
    fn load_avatars(&mut self, cx: &mut Context<Self>) {
        let urls: Vec<String> = match &self.view.identity {
            IdentityState::None => Vec::new(),
            IdentityState::Selected(i) => i.avatar_url.iter().cloned().collect(),
            IdentityState::Ambiguous(v) | IdentityState::Changed(v) => {
                v.iter().filter_map(|i| i.avatar_url.clone()).collect()
            }
        };
        self.avatars.retain(|url, _| urls.contains(url));
        for url in urls {
            if self.avatars.contains_key(&url) {
                continue;
            }
            self.avatars.insert(url.clone(), AvatarSlot::Loading);
            let source = self.avatar_source.clone();
            cx.spawn(async move |this, cx| {
                let fetch = url.clone();
                let image = cx
                    .background_executor()
                    .spawn(async move { source.fetch(&fetch).map(|art| art.render_image()) })
                    .await;
                let _ = this.update(cx, |m, cx| {
                    // Only if it is still wanted (the account may have gone meanwhile).
                    if let Some(slot) = m.avatars.get_mut(&url) {
                        *slot = AvatarSlot::Ready(image);
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    // ---- sign in ----------------------------------------------------------

    pub fn sign_in_browser(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        self.begin(Method::Browser, move || source.browser(), cx);
    }

    pub fn sign_in_firefox(&mut self, cx: &mut Context<Self>) {
        let source = self.source.clone();
        self.begin(Method::Firefox, move || source.firefox(), cx);
    }

    pub fn sign_in_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.begin(Method::File, move || file_jar(&path), cx);
    }

    /// `text` is the clipboard's current text, if it has any.
    pub fn sign_in_clipboard(&mut self, text: Option<String>, cx: &mut Context<Self>) {
        self.begin(
            Method::Clipboard,
            move || clipboard_jar(text.as_deref()),
            cx,
        );
    }

    /// Ends a running browser sign-in.
    pub fn cancel(&mut self) {
        if self.pending == Some(Method::Browser) {
            self.source.cancel_browser();
        }
    }

    fn begin(
        &mut self,
        method: Method,
        obtain: impl FnOnce() -> Result<CookieJar, SignInError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        if self.pending.is_some() {
            return;
        }
        self.pending = Some(method);
        self.error = None;
        cx.notify();
        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            // The owner declares Premium by signing in with a subscription account; the service
            // keeps calling it "assumed", never verified.
            let result = cx
                .background_executor()
                .spawn(async move { obtain().map(|jar| service.import(jar, true)) })
                .await;
            let _ = this.update(cx, |m, cx| {
                m.pending = None;
                m.error = match result {
                    Ok(outcome) => outcome_error(&outcome),
                    Err(SignInError::Cancelled) => None,
                    Err(e) => Some(e),
                };
                m.refresh(cx);
            });
        })
        .detach();
    }

    /// Re-check the current session with the service.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { service.revalidate() })
                .await;
            let _ = this.update(cx, |m, cx| m.refresh(cx));
        })
        .detach();
    }

    /// Pick the channel the library and writes belong to.
    pub fn choose(&mut self, key: String, cx: &mut Context<Self>) {
        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { service.select_identity(&key) })
                .await;
            let _ = this.update(cx, |m, cx| m.refresh(cx));
        })
        .detach();
    }

    /// Drop the session everywhere (memory, vault, ciphertext). The caller stops playback first.
    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        self.cancel();
        self.error = None;
        let service = self.service.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move { service.logout() })
                .await;
            let _ = this.update(cx, |m, cx| m.refresh(cx));
        })
        .detach();
    }

    /// A problem that happened outside a sign-in (a clipboard with nothing on it).
    pub fn report(&mut self, error: SignInError, cx: &mut Context<Self>) {
        self.error = Some(error);
        cx.notify();
    }
}

fn outcome_error(outcome: &ImportOutcome) -> Option<SignInError> {
    match outcome {
        ImportOutcome::Rejected(RejectReason::NoAuthCookies) => Some(SignInError::NoAuthCookies),
        ImportOutcome::Rejected(RejectReason::Expired) => Some(SignInError::Expired),
        // A pending import is shown from the service's own state (it could not be confirmed).
        ImportOutcome::Valid
        | ImportOutcome::NeedsIdentityChoice
        | ImportOutcome::Pending(_)
        | ImportOutcome::Superseded => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(session: SessionState, identity: IdentityState) -> AccountView {
        AccountView {
            session,
            identity,
            premium: PremiumEvidence::OwnerDeclared,
            storage: StorageMode::Durable,
            problem: Problem::None,
            epoch: 1,
            jar_is_seed: false,
        }
    }

    fn ident(n: usize) -> Identity {
        Identity {
            name: format!("Channel {n}"),
            handle: Some(format!("@c{n}")),
            avatar_url: None,
            page_id: Some(format!("PAGE{n}")),
            selected_by_service: n == 0,
        }
    }

    fn at_rest(v: &AccountView) -> Screen {
        screen(v, false, false, None)
    }

    #[test]
    fn startup_shows_the_skeleton_until_the_check_is_over() {
        let v = view(SessionState::Absent, IdentityState::None);
        assert_eq!(screen(&v, true, false, None), Screen::Checking);
        assert_eq!(at_rest(&v), Screen::SignedOut { error: None });
    }

    #[test]
    fn a_valid_selected_identity_is_signed_in_without_hedged_premium_text() {
        let v = view(SessionState::Valid, IdentityState::Selected(ident(0)));
        let Screen::SignedIn {
            who,
            premium,
            notice,
        } = at_rest(&v)
        else {
            panic!("signed in");
        };
        assert_eq!(who.name, "Channel 0");
        assert_eq!(who.handle.as_deref(), Some("@c0"));
        assert_eq!(premium, None);
        assert_eq!(notice, None);
    }

    #[test]
    fn premium_shows_nothing_until_the_account_check_confirms_it() {
        for evidence in [PremiumEvidence::OwnerDeclared, PremiumEvidence::Undeclared] {
            let mut v = view(SessionState::Valid, IdentityState::Selected(ident(0)));
            v.premium = evidence;
            let Screen::SignedIn { premium, .. } = at_rest(&v) else {
                panic!("signed in");
            };
            assert_eq!(premium, None, "{evidence:?}");
        }
    }

    #[test]
    fn revalidating_does_not_flash_the_signed_out_page() {
        let v = view(SessionState::Validating, IdentityState::Selected(ident(0)));
        assert!(matches!(at_rest(&v), Screen::SignedIn { notice: None, .. }));
    }

    #[test]
    fn an_unreachable_service_keeps_the_account_and_says_so() {
        let mut v = view(SessionState::Unreachable, IdentityState::Selected(ident(0)));
        v.problem = Problem::Network;
        let Screen::SignedIn { notice, .. } = at_rest(&v) else {
            panic!("signed in");
        };
        assert_eq!(notice, Some(Notice::CantReach));
        v.problem = Problem::ServiceChanged;
        let Screen::SignedIn { notice, .. } = at_rest(&v) else {
            panic!("signed in");
        };
        assert_eq!(notice, Some(Notice::CantVerify));
    }

    #[test]
    fn a_pending_import_without_identity_offers_retry() {
        let mut v = view(SessionState::Unreachable, IdentityState::None);
        v.problem = Problem::Network;
        assert_eq!(at_rest(&v), Screen::CantConfirm(Notice::CantReach));
    }

    #[test]
    fn a_vault_that_cannot_store_the_session_is_surfaced() {
        let mut v = view(SessionState::Valid, IdentityState::Selected(ident(0)));
        v.storage = StorageMode::NotPersisted("locked".into());
        let Screen::SignedIn { notice, .. } = at_rest(&v) else {
            panic!("signed in");
        };
        assert_eq!(notice, Some(Notice::NotSaved));
        assert!(!Notice::NotSaved.retryable());
        // Memory-only that the owner accepted is not a problem.
        v.storage = StorageMode::MemoryOnly;
        assert!(matches!(at_rest(&v), Screen::SignedIn { notice: None, .. }));
    }

    #[test]
    fn several_channels_or_a_changed_one_ask_the_owner_to_choose() {
        let v = view(
            SessionState::Valid,
            IdentityState::Ambiguous(vec![ident(0), ident(1)]),
        );
        let Screen::Choose(list) = at_rest(&v) else {
            panic!("choose");
        };
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].key, "PAGE1");
        let v = view(SessionState::Valid, IdentityState::Changed(vec![ident(2)]));
        assert!(matches!(at_rest(&v), Screen::Choose(_)));
    }

    #[test]
    fn an_expired_session_is_signed_out_with_the_reason() {
        let v = view(SessionState::Expired, IdentityState::None);
        assert_eq!(
            at_rest(&v),
            Screen::SignedOut {
                error: Some(SignInError::Expired)
            }
        );
        // The last sign-in's own error wins over the stale reason.
        assert_eq!(
            screen(&v, false, false, Some(SignInError::NoFirefox)),
            Screen::SignedOut {
                error: Some(SignInError::NoFirefox)
            }
        );
    }

    #[test]
    fn a_running_sign_in_never_shows_the_startup_skeleton() {
        let v = view(SessionState::Validating, IdentityState::None);
        assert_eq!(screen(&v, false, false, None), Screen::Checking);
        assert_eq!(
            screen(&v, false, true, None),
            Screen::SignedOut { error: None }
        );
    }

    #[test]
    fn every_error_line_is_six_words_or_fewer() {
        use SignInError::*;
        for e in [
            BrowserUnavailable,
            TimedOut,
            NoFirefox,
            NoFirefoxCookies,
            FirefoxUnreadable,
            FileUnreadable,
            NoFileCookies,
            ClipboardEmpty,
            NoClipboardCookies,
            NoAuthCookies,
            Expired,
            NoAccount,
            Storage,
        ] {
            let words = e.text().split_whitespace().count();
            assert!((1..=6).contains(&words), "{e:?}: {}", e.text());
        }
        for n in [Notice::CantReach, Notice::CantVerify, Notice::NotSaved] {
            assert!(n.text().split_whitespace().count() <= 6, "{n:?}");
        }
    }

    #[test]
    fn clipboard_text_is_checked_before_anything_is_sent() {
        assert_eq!(
            clipboard_jar(None).unwrap_err(),
            SignInError::ClipboardEmpty
        );
        assert_eq!(
            clipboard_jar(Some("  \n")).unwrap_err(),
            SignInError::ClipboardEmpty
        );
        // Cookies without the account's own are accepted here; the service rejects them.
        assert!(!clipboard_jar(Some("tracker=1; other=2"))
            .unwrap()
            .has_auth_cookies());
        assert!(clipboard_jar(Some(
            "SAPISID=a; SID=b; __Secure-3PSID=c; __Secure-3PAPISID=d"
        ))
        .is_ok());
    }
}
