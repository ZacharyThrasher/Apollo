//! Application bootstrap: window, shortcuts, and the bridge from the service adapters to the UI.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    px, size, App, AppContext, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowOptions,
};

use crate::account::{AccountModel, AccountPorts};
use crate::backend::{channel_sink, Mode};
use crate::player::PlayerModel;
use crate::search::Unavailable;
use crate::settings::{SqliteVolume as SettingsVolume, VolumeStore};
use crate::ui::shell::{self, Quit, Shell};
use crate::url_thumbs::NoThumbs;

/// Smallest window that still fits the player bar's three columns at their minimum widths.
const MIN_WIDTH: f32 = crate::ui::player_bar::MIN_BAR_WIDTH;
const MIN_HEIGHT: f32 = 480.0;

/// How often the system's animation setting is looked at while the app runs.
pub const MOTION_POLL: Duration = Duration::from_secs(2);

/// Keeps GPUI's reduced-motion flag in step with the system setting: read now, and again every
/// [`MOTION_POLL`], so switching Windows "Animation effects" off (or back on) while Apollo is
/// open takes effect without a restart. Every transition and loading pulse already follows the
/// flag; windows are redrawn when it changes.
pub fn follow_reduced_motion(cx: &mut App, animations_enabled: impl Fn() -> bool + 'static) {
    cx.set_reduce_motion(!animations_enabled());
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(MOTION_POLL).await;
        let reduce = !animations_enabled();
        cx.update(|cx| cx.set_reduce_motion(reduce));
    })
    .detach();
}

pub fn run(cx: &mut App, mode: Mode) -> WindowHandle<Shell> {
    // Windows "Animation effects" off means every transition is instant.
    follow_reduced_motion(cx, crate::platform::animations_enabled);
    crate::style::Fonts::install(cx);
    shell::bind_keys(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());

    let volume_store: Option<Arc<dyn VolumeStore>> =
        SettingsVolume::default_location().map(|s| Arc::new(s) as Arc<dyn VolumeStore>);
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, _| {
        m.set_volume_store(volume_store.clone());
        m.set_latency_log(crate::latency::default_log());
    });
    let ports = mode.account_ports();
    let account = cx.new(|cx| AccountModel::new(ports.clone(), cx));
    let window = open_window(cx, player.clone(), account.clone());
    // Media keys and the SMTC overlay, on the window just opened.
    #[cfg(windows)]
    crate::smtc::install(cx, window, player.clone());
    start_services(
        cx,
        mode,
        player.clone(),
        window,
        account,
        ports,
        volume_store,
    );

    cx.on_app_quit(move |cx| {
        player.update(cx, |m, _| m.shutdown());
        async {}
    })
    .detach();
    cx.activate(true);
    window
}

fn open_window(
    cx: &mut App,
    player: gpui::Entity<PlayerModel>,
    account: gpui::Entity<AccountModel>,
) -> WindowHandle<Shell> {
    let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
    let window = cx
        .open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Apollo".into()),
                    appears_transparent: true,
                    traffic_light_position: None,
                }),
                window_background: WindowBackgroundAppearance::MicaBackdrop,
                window_min_size: Some(size(px(MIN_WIDTH), px(MIN_HEIGHT))),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Shell::with_account(player, account, cx)),
        )
        .expect("open window");
    window
        .update(cx, |shell, window, cx| {
            shell.focus_root(window, cx);
            shell.install_window_hooks(window, cx);
        })
        .expect("focus root");
    window
}

/// Restores the account, prepares the backend (both off the UI thread), installs it, then feeds
/// adapter completions to the model until the window goes away.
fn start_services(
    cx: &mut App,
    mode: Mode,
    player: gpui::Entity<PlayerModel>,
    window: WindowHandle<Shell>,
    account: gpui::Entity<AccountModel>,
    ports: AccountPorts,
    volume_store: Option<Arc<dyn VolumeStore>>,
) {
    let (sink, mut rx) = channel_sink();
    let weak = player.downgrade();
    cx.spawn(async move |cx| {
        // The account first: the sign-in screen can show as soon as the stored session has
        // been checked, without waiting for the player's backend.
        let mode = {
            let ports = ports.clone();
            cx.background_executor()
                .spawn(async move {
                    mode.start_account(&ports);
                    mode
                })
                .await
        };
        account.update(cx, |m, cx| m.startup_finished(cx));
        let service = ports.service.clone();
        let (prepared, saved_volume) = cx
            .background_executor()
            .spawn(async move {
                let saved = volume_store.and_then(|s| s.load());
                (mode.prepare(sink, &service), saved)
            })
            .await;
        match prepared {
            Ok(prepared) => {
                let (search, thumbs) = (prepared.search.clone(), prepared.thumbs.clone());
                let _ = weak.update(cx, |m, cx| {
                    m.restore_volume(saved_volume);
                    m.install(prepared, cx);
                });
                let _ = window.update(cx, |shell, _, cx| shell.install_search(search, thumbs, cx));
            }
            Err(err) => {
                let _ = weak.update(cx, |m, cx| m.fail(&err, cx));
                // Search shows its own short error instead of waiting for a backend that is not coming.
                let _ = window.update(cx, |shell, _, cx| {
                    shell.install_search(Arc::new(Unavailable), Arc::new(NoThumbs), cx)
                });
                return;
            }
        }
        while let Some(first) = rx.recv().await {
            let mut batch = vec![first];
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            let alive = weak.update(cx, |m, cx| {
                for event in batch {
                    m.handle(event, cx);
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}
