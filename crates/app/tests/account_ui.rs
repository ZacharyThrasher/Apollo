//! Headless tests of the sign-in and account screen: the real model and views over the real
//! account service, with a scripted validator and sign-in source (no network, no helper
//! process, no vault).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use apollo_account::identity::{AccountValidator, Identity, Validation};
use apollo_account::jar::CookieJar;
use apollo_account::service::AccountService;
use apollo_app::account::{
    AccountModel, AccountPorts, AvatarSource, Method, NoAvatars, Notice, Screen, SessionSource,
    SignInError,
};
use apollo_app::backend::account::sim_jar;
use apollo_app::player::PlayerModel;
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use apollo_domain::playback::Intent;
use gpui::{AppContext, Entity, TestAppContext, VisualTestContext};

fn who(n: usize) -> Identity {
    Identity {
        name: format!("Listener {n}"),
        handle: Some(format!("@listener{n}")),
        avatar_url: None,
        page_id: Some(format!("PAGE{n}")),
        selected_by_service: n == 0,
    }
}

/// Answers from a script; once it runs out, `fallback` repeats.
struct Scripted {
    script: Mutex<VecDeque<Validation>>,
    fallback: Validation,
    calls: AtomicUsize,
}

impl Scripted {
    fn new(script: Vec<Validation>, fallback: Validation) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            fallback,
            calls: AtomicUsize::new(0),
        })
    }
}

impl AccountValidator for Scripted {
    fn validate(&self, _: &CookieJar) -> Validation {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone())
    }
}

fn valid(n: usize) -> Validation {
    Validation::Valid {
        identities: vec![who(n)],
    }
}

/// A browser sign-in that returns what it was told to.
struct FakeSource {
    result: Mutex<Result<(), SignInError>>,
    cancelled: AtomicBool,
}

impl FakeSource {
    fn ok() -> Arc<Self> {
        Self::failing(Ok(()))
    }
    fn failing(result: Result<(), SignInError>) -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(result),
            cancelled: AtomicBool::new(false),
        })
    }
}

impl SessionSource for FakeSource {
    fn browser(&self) -> Result<CookieJar, SignInError> {
        (*self.result.lock().unwrap()).map(|()| sim_jar())
    }
    fn cancel_browser(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    fn firefox(&self) -> Result<CookieJar, SignInError> {
        Err(SignInError::NoFirefox)
    }
}

struct Rig {
    account: Entity<AccountModel>,
    player: Entity<PlayerModel>,
    shell: Entity<Shell>,
    source: Arc<FakeSource>,
    validator: Arc<Scripted>,
}

fn rig(
    cx: &mut TestAppContext,
    validator: Arc<Scripted>,
    source: Arc<FakeSource>,
    finish_startup: bool,
) -> (Rig, &mut VisualTestContext) {
    rig_with(cx, validator, source, Arc::new(NoAvatars), finish_startup)
}

fn rig_with(
    cx: &mut TestAppContext,
    validator: Arc<Scripted>,
    source: Arc<FakeSource>,
    avatars: Arc<dyn AvatarSource>,
    finish_startup: bool,
) -> (Rig, &mut VisualTestContext) {
    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let ports = AccountPorts {
        service: AccountService::new(validator.clone(), None),
        source: source.clone(),
        avatars,
        startup_error: None,
    };
    let account = cx.new(|cx| AccountModel::new(ports, cx));
    let player = cx.new(|_| PlayerModel::new());
    let (shell, vcx) = cx.add_window_view({
        let (player, account) = (player.clone(), account.clone());
        move |_, cx| Shell::with_account(player, account, cx)
    });
    shell.update_in(vcx, |s, window, cx| s.focus_root(window, cx));
    if finish_startup {
        account.update(vcx, |m, cx| m.startup_finished(cx));
    }
    vcx.run_until_parked();
    (
        Rig {
            account,
            player,
            shell,
            source,
            validator,
        },
        vcx,
    )
}

fn screen(r: &Rig, cx: &mut VisualTestContext) -> Screen {
    r.account.read_with(cx, |m, _| m.screen())
}

#[gpui::test]
fn the_window_shows_a_skeleton_until_the_startup_check_is_over(cx: &mut TestAppContext) {
    let (r, vcx) = rig(
        cx,
        Scripted::new(vec![], Validation::Expired),
        FakeSource::ok(),
        false,
    );
    assert_eq!(screen(&r, vcx), Screen::Checking);
    r.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Home));
    r.account.update(vcx, |m, cx| m.startup_finished(cx));
    vcx.run_until_parked();
    assert_eq!(screen(&r, vcx), Screen::SignedOut { error: None });
}

#[gpui::test]
fn signed_out_at_launch_opens_settings_once(cx: &mut TestAppContext) {
    let (r, vcx) = rig(
        cx,
        Scripted::new(vec![], Validation::Expired),
        FakeSource::ok(),
        true,
    );
    r.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Settings));
    // Leaving Settings sticks: the shell does not pull the owner back.
    r.shell.update(vcx, |_, _| {});
    r.account
        .update(vcx, |m, cx| m.report(SignInError::NoFirefox, cx));
    vcx.run_until_parked();
    r.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Settings));
}

#[gpui::test]
fn after_a_sign_out_the_library_says_signed_out_and_signs_in_through_settings(
    cx: &mut TestAppContext,
) {
    let (r, vcx) = rig(
        cx,
        Scripted::new(vec![], Validation::Expired),
        FakeSource::ok(),
        true,
    );
    r.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    let button = vcx
        .debug_bounds("library-sign-in")
        .expect("the signed-out Library offers Sign in");
    // The wrapper spans the row; the button is at its start.
    let on_button = button.origin + gpui::point(gpui::px(24.0), gpui::px(16.0));
    vcx.simulate_click(on_button, gpui::Modifiers::none());
    vcx.run_until_parked();
    r.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Settings));
}

#[gpui::test]
fn an_expired_session_keeps_the_librarys_own_expired_message(cx: &mut TestAppContext) {
    let (r, vcx) = rig(
        cx,
        Scripted::new(vec![], Validation::Expired),
        FakeSource::ok(),
        false,
    );
    r.account.update(vcx, |m, _| {
        m.service().import(sim_jar(), true);
    });
    r.account.update(vcx, |m, cx| m.startup_finished(cx));
    vcx.run_until_parked();
    assert!(matches!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::Expired)
        }
    ));
    r.shell
        .update(vcx, |s, cx| s.show_route(Route::Library, cx));
    vcx.run_until_parked();
    assert!(vcx.debug_bounds("library-sign-in").is_none());
}

#[gpui::test]
fn signed_in_at_launch_stays_on_home(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), FakeSource::ok(), false);
    r.account.update(vcx, |m, _| {
        m.service().import(sim_jar(), true);
    });
    r.account.update(vcx, |m, cx| m.startup_finished(cx));
    vcx.run_until_parked();
    r.shell
        .read_with(vcx, |s, _| assert_eq!(s.route(), Route::Home));
    assert!(matches!(screen(&r, vcx), Screen::SignedIn { .. }));
}

#[gpui::test]
fn browser_sign_in_ends_signed_in(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(1)), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    assert_eq!(
        r.account.read_with(vcx, |m, _| m.busy()),
        Some(Method::Browser)
    );
    vcx.run_until_parked();
    let Screen::SignedIn {
        who,
        premium,
        notice,
    } = screen(&r, vcx)
    else {
        panic!("signed in, got {:?}", screen(&r, vcx));
    };
    assert_eq!(who.name, "Listener 1");
    assert_eq!(who.handle.as_deref(), Some("@listener1"));
    assert_eq!(premium, None, "no hedged Premium text");
    assert_eq!(notice, None);
    assert_eq!(r.account.read_with(vcx, |m, _| m.busy()), None);
    assert!(!r.account.read_with(vcx, |m, _| m.attention().is_some()));
}

#[gpui::test]
fn a_second_sign_in_while_one_runs_is_ignored(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| {
        m.sign_in_browser(cx);
        m.sign_in_firefox(cx);
    });
    assert_eq!(
        r.account.read_with(vcx, |m, _| m.busy()),
        Some(Method::Browser)
    );
    vcx.run_until_parked();
    assert_eq!(r.validator.calls.load(Ordering::SeqCst), 1);
}

#[gpui::test]
fn a_closed_sign_in_window_is_not_an_error(cx: &mut TestAppContext) {
    let source = FakeSource::failing(Err(SignInError::Cancelled));
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), source, true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert_eq!(screen(&r, vcx), Screen::SignedOut { error: None });
    assert_eq!(
        r.validator.calls.load(Ordering::SeqCst),
        0,
        "nothing to validate"
    );
}

#[gpui::test]
fn cancel_reaches_the_helper_only_while_the_browser_runs(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), FakeSource::ok(), true);
    r.account.update(vcx, |m, _| m.cancel());
    assert!(!r.source.cancelled.load(Ordering::SeqCst));
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    r.account.update(vcx, |m, _| m.cancel());
    assert!(r.source.cancelled.load(Ordering::SeqCst));
}

#[gpui::test]
fn a_helper_failure_says_so_in_few_words(cx: &mut TestAppContext) {
    let source = FakeSource::failing(Err(SignInError::BrowserUnavailable));
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), source, true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::BrowserUnavailable)
        }
    );
    // The next attempt clears the line.
    *r.source.result.lock().unwrap() = Ok(());
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    assert_eq!(screen(&r, vcx), Screen::SignedOut { error: None });
}

#[gpui::test]
fn firefox_without_a_profile_reports_it(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_firefox(cx));
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::NoFirefox)
        }
    );
}

#[gpui::test]
fn a_cookie_file_signs_in_and_a_bad_one_does_not(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let good: PathBuf = dir.path().join("cookies.txt");
    std::fs::write(&good, sim_jar().to_netscape().expose()).unwrap();
    let bad: PathBuf = dir.path().join("notes.txt");
    std::fs::write(&bad, "hello").unwrap();

    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(2)), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_file(bad, cx));
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::NoFileCookies)
        }
    );
    r.account.update(vcx, |m, cx| {
        m.sign_in_file(dir.path().join("missing.txt"), cx)
    });
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::FileUnreadable)
        }
    );
    r.account.update(vcx, |m, cx| m.sign_in_file(good, cx));
    vcx.run_until_parked();
    assert!(matches!(screen(&r, vcx), Screen::SignedIn { .. }));
}

#[gpui::test]
fn pasted_cookies_sign_in_and_an_empty_clipboard_says_so(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(3)), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_clipboard(None, cx));
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::ClipboardEmpty)
        }
    );
    let header = "SAPISID=a; __Secure-3PAPISID=b; SID=c; __Secure-3PSID=d".to_string();
    r.account
        .update(vcx, |m, cx| m.sign_in_clipboard(Some(header), cx));
    vcx.run_until_parked();
    assert!(matches!(screen(&r, vcx), Screen::SignedIn { .. }));
}

#[gpui::test]
fn a_session_the_service_calls_logged_out_is_rejected_with_a_reason(cx: &mut TestAppContext) {
    let (r, vcx) = rig(
        cx,
        Scripted::new(vec![], Validation::Expired),
        FakeSource::ok(),
        true,
    );
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::Expired)
        }
    );
}

#[gpui::test]
fn an_unreachable_service_offers_retry_and_retry_recovers(cx: &mut TestAppContext) {
    let validator = Scripted::new(vec![Validation::Unreachable], valid(0));
    let (r, vcx) = rig(cx, validator, FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert_eq!(screen(&r, vcx), Screen::CantConfirm(Notice::CantReach));
    r.account.update(vcx, |m, cx| m.retry(cx));
    vcx.run_until_parked();
    assert!(matches!(
        screen(&r, vcx),
        Screen::SignedIn { notice: None, .. }
    ));
}

#[gpui::test]
fn a_session_that_expires_later_asks_for_a_new_sign_in(cx: &mut TestAppContext) {
    let validator = Scripted::new(vec![valid(0)], Validation::Expired);
    let (r, vcx) = rig(cx, validator, FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert!(!r.account.read_with(vcx, |m, _| m.attention().is_some()));

    // The periodic re-check (or a retry) learns the service no longer accepts the session.
    r.account.update(vcx, |m, cx| m.retry(cx));
    vcx.run_until_parked();
    assert_eq!(
        screen(&r, vcx),
        Screen::SignedOut {
            error: Some(SignInError::Expired)
        }
    );
    assert!(r.account.read_with(vcx, |m, _| m.attention().is_some()));
    // Signing in again clears it.
    *r.validator.script.lock().unwrap() = vec![valid(0)].into();
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert!(matches!(screen(&r, vcx), Screen::SignedIn { .. }));
    assert!(!r.account.read_with(vcx, |m, _| m.attention().is_some()));
}

#[gpui::test]
fn several_channels_ask_which_one_and_the_choice_sticks(cx: &mut TestAppContext) {
    let two = Validation::Valid {
        identities: vec![who(0), who(1)],
    };
    let (r, vcx) = rig(cx, Scripted::new(vec![], two), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    let Screen::Choose(list) = screen(&r, vcx) else {
        panic!("choose, got {:?}", screen(&r, vcx));
    };
    assert_eq!(list.len(), 2);
    assert!(r.account.read_with(vcx, |m, _| m.attention().is_some()));
    r.account
        .update(vcx, |m, cx| m.choose(list[1].key.clone(), cx));
    vcx.run_until_parked();
    let Screen::SignedIn { who, .. } = screen(&r, vcx) else {
        panic!("signed in");
    };
    assert_eq!(who.name, "Listener 1");
}

#[gpui::test]
fn sign_out_stops_playback_and_forgets_the_session(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert!(r
        .account
        .read_with(vcx, |m, _| m.service().view().can_use_content()));

    r.player.update(vcx, |m, cx| m.toggle_play(cx));
    // The page's handler does exactly this pair, in this order.
    r.player.update(vcx, |m, cx| {
        m.handle(apollo_domain::playback::Event::Stop, cx)
    });
    r.account.update(vcx, |m, cx| m.sign_out(cx));
    vcx.run_until_parked();

    assert_eq!(screen(&r, vcx), Screen::SignedOut { error: None });
    assert!(!r
        .account
        .read_with(vcx, |m, _| m.service().view().can_use_content()));
    // No session is left for the resolver to use (no guest fallback).
    use apollo_services_common::SessionProvider;
    assert!(r
        .account
        .read_with(vcx, |m, _| m.service().snapshot().is_none()));
    if let Some(intent) = r
        .player
        .read_with(vcx, |m, _| m.coordinator().map(|c| c.intent()))
    {
        assert_eq!(intent, Intent::Paused);
    }
}

/// A real key press: activation happens on key up, on the element that was drawn as focused.
fn press_enter(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| {
        let needed = window.draw(cx);
        needed.clear(cx);
    });
    let keystroke = gpui::Keystroke::parse("enter").unwrap();
    vcx.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    vcx.simulate_event(gpui::KeyUpEvent { keystroke });
}

#[gpui::test]
fn the_tab_order_reaches_sign_in_and_enter_presses_it(cx: &mut TestAppContext) {
    let (r, vcx) = rig(cx, Scripted::new(vec![], valid(0)), FakeSource::ok(), true);
    // Signed out: Settings is open. Tab walks the nav (toggle, four items) and then lands on
    // the page's first control, Sign in; Enter presses it.
    vcx.simulate_keystrokes("tab tab tab tab tab tab");
    press_enter(vcx);
    vcx.run_until_parked();
    assert!(matches!(screen(&r, vcx), Screen::SignedIn { .. }));
    // The page re-renders for the signed-in state too.
    vcx.update(|window, cx| {
        let needed = window.draw(cx);
        needed.clear(cx);
    });
}

struct FakeAvatars {
    fetched: Mutex<Vec<String>>,
}

impl AvatarSource for FakeAvatars {
    fn fetch(&self, url: &str) -> Option<apollo_app::art::DecodedArt> {
        self.fetched.lock().unwrap().push(url.to_owned());
        (!url.contains("broken")).then(|| {
            apollo_app::art::DecodedArt::from_rgba(
                apollo_app::art::SyntheticArt::Hue(120.0).render(32),
            )
        })
    }
}

fn with_avatar(n: usize, url: &str) -> Validation {
    let mut id = who(n);
    id.avatar_url = Some(url.into());
    Validation::Valid {
        identities: vec![id],
    }
}

#[gpui::test]
fn the_channel_picture_is_fetched_once_shown_and_dropped_at_sign_out(cx: &mut TestAppContext) {
    let url = "https://lh3.googleusercontent.com/me=s176";
    let avatars = Arc::new(FakeAvatars {
        fetched: Mutex::new(Vec::new()),
    });
    let (r, vcx) = rig_with(
        cx,
        Scripted::new(vec![], with_avatar(0, url)),
        FakeSource::ok(),
        avatars.clone(),
        true,
    );
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    let Screen::SignedIn { who, .. } = screen(&r, vcx) else {
        panic!("signed in");
    };
    assert_eq!(who.avatar_url.as_deref(), Some(url));
    assert!(r.account.read_with(vcx, |m, _| m.avatar(url).is_some()));
    // A re-check does not fetch it again.
    r.account.update(vcx, |m, cx| m.retry(cx));
    vcx.run_until_parked();
    assert_eq!(avatars.fetched.lock().unwrap().len(), 1);

    r.account.update(vcx, |m, cx| m.sign_out(cx));
    vcx.run_until_parked();
    assert!(r.account.read_with(vcx, |m, _| m.avatar(url).is_none()));
}

#[gpui::test]
fn a_picture_that_cannot_be_fetched_falls_back_to_the_initial(cx: &mut TestAppContext) {
    let url = "https://lh3.googleusercontent.com/broken";
    let avatars = Arc::new(FakeAvatars {
        fetched: Mutex::new(Vec::new()),
    });
    let (r, vcx) = rig_with(
        cx,
        Scripted::new(vec![], with_avatar(0, url)),
        FakeSource::ok(),
        avatars.clone(),
        true,
    );
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert!(r.account.read_with(vcx, |m, _| m.avatar(url).is_none()));
    assert!(matches!(screen(&r, vcx), Screen::SignedIn { .. }));
    // It is not retried on every refresh.
    r.account.update(vcx, |m, cx| m.retry(cx));
    vcx.run_until_parked();
    assert_eq!(avatars.fetched.lock().unwrap().len(), 1);
}

#[gpui::test]
fn the_attention_dot_names_its_reason(cx: &mut TestAppContext) {
    let validator = Scripted::new(vec![valid(0)], Validation::Expired);
    let (r, vcx) = rig(cx, validator, FakeSource::ok(), true);
    r.account.update(vcx, |m, cx| m.sign_in_browser(cx));
    vcx.run_until_parked();
    assert_eq!(r.account.read_with(vcx, |m, _| m.attention()), None);
    r.account.update(vcx, |m, cx| m.retry(cx));
    vcx.run_until_parked();
    assert_eq!(
        r.account.read_with(vcx, |m, _| m.attention()),
        Some("Session expired")
    );
}

#[gpui::test]
fn settings_shows_the_window_switches_and_a_click_flips_one(cx: &mut TestAppContext) {
    use apollo_app::settings::{SettingsBackend, SettingsModel, CLOSE_TO_TRAY};

    #[derive(Default)]
    struct Memory(Mutex<std::collections::HashMap<String, String>>);
    impl SettingsBackend for Memory {
        fn get(&self, key: &str) -> Option<String> {
            self.0.lock().unwrap().get(key).cloned()
        }
        fn set(&self, key: &str, value: &str) {
            self.0.lock().unwrap().insert(key.into(), value.into());
        }
    }

    cx.update(|cx| {
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let backend = Arc::new(Memory::default());
    let ports = AccountPorts {
        service: AccountService::new(Scripted::new(vec![], Validation::Expired), None),
        source: FakeSource::ok(),
        avatars: Arc::new(NoAvatars),
        startup_error: None,
    };
    let account = cx.new(|cx| AccountModel::new(ports, cx));
    let player = cx.new(|_| PlayerModel::new());
    let settings = cx.new(|_| SettingsModel::new(Some(backend.clone())));
    let (shell, vcx) = cx.add_window_view({
        let (player, account, settings) = (player.clone(), account.clone(), settings.clone());
        move |_, cx| Shell::with_settings(player, account, settings, cx)
    });
    account.update(vcx, |m, cx| m.startup_finished(cx));
    shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();

    for id in ["setting-close-to-tray", "setting-start-minimized"] {
        assert!(vcx.debug_bounds(id).is_some(), "{id} is on the page");
    }
    assert!(
        settings.read_with(vcx, |m, _| m.get(&CLOSE_TO_TRAY)),
        "on by default"
    );

    let row = vcx.debug_bounds("setting-close-to-tray").unwrap();
    vcx.simulate_click(row.center(), gpui::Modifiers::none());
    vcx.run_until_parked();
    assert!(!settings.read_with(vcx, |m, _| m.get(&CLOSE_TO_TRAY)));
}
