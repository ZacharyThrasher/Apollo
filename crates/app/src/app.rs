//! Application bootstrap: window, shortcuts, and the bridge from the service adapters to the UI.

use std::sync::Arc;
use std::time::Duration;

use apollo_lyrics::{Lrclib, LyricsCache, LyricsService, LyricsSource, SqliteCache};
use gpui::{
    px, size, App, AppContext, Bounds, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowOptions,
};

use crate::account::{AccountModel, AccountPorts};
use crate::backend::{channel_sink, Mode};
use crate::lyrics::SimLyrics;
use crate::player::PlayerModel;
use crate::search::Unavailable;
use crate::settings::{
    AutoplayStore, GaplessStore, LibrarySortStore, LyricsOpenStore, NormalizeStore, QueueStore,
    RelatedOpenStore, SettingsBackend, SettingsModel, SqliteSettings, VolumeStore,
    ADD_PLAYS_TO_HISTORY,
};
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

/// Everything kept between runs, each behind its own port (all `None` where there is no
/// `%LOCALAPPDATA%`, which leaves persistence off).
#[derive(Clone, Default)]
struct Stores {
    volume: Option<Arc<dyn VolumeStore>>,
    autoplay: Option<Arc<dyn AutoplayStore>>,
    normalize: Option<Arc<dyn NormalizeStore>>,
    gapless: Option<Arc<dyn GaplessStore>>,
    queue: Option<Arc<dyn QueueStore>>,
    lyrics_open: Option<Arc<dyn LyricsOpenStore>>,
    related_open: Option<Arc<dyn RelatedOpenStore>>,
    library_sort: Option<Arc<dyn LibrarySortStore>>,
    /// The store's file, where the lyrics cache keeps its rows too.
    db_path: Option<std::path::PathBuf>,
}

impl Stores {
    fn default_location() -> Self {
        match SqliteSettings::default_location().map(Arc::new) {
            Some(db) => Self {
                volume: Some(db.clone()),
                autoplay: Some(db.clone()),
                normalize: Some(db.clone()),
                gapless: Some(db.clone()),
                lyrics_open: Some(db.clone()),
                related_open: Some(db.clone()),
                library_sort: Some(db.clone()),
                db_path: Some(db.path().to_path_buf()),
                queue: Some(db),
            },
            None => Self::default(),
        }
    }
}

/// The opaque id the saved queue is bound to: the channel the owner chose. `None` while nobody
/// is signed in, which keeps the queue from being saved or restored.
fn queue_account_key(account: &apollo_account::service::AccountService) -> Option<String> {
    match account.view().identity {
        apollo_account::service::IdentityState::Selected(identity) => Some(identity.key()),
        _ => None,
    }
}

pub fn run(cx: &mut App, mode: Mode) -> WindowHandle<Shell> {
    // Windows "Animation effects" off means every transition is instant.
    follow_reduced_motion(cx, crate::platform::animations_enabled);
    crate::style::Fonts::install(cx);
    shell::bind_keys(cx);
    cx.on_action(|_: &Quit, cx| cx.quit());

    let stores = Stores::default_location();
    // Pages cached between runs (Home, the library, album, artist and playlist pages), kept in
    // the app's SQLite file; memory only without one.
    crate::page_cache::PageCache::new(stores.db_path.clone()).install(cx);
    let player = cx.new(|_| PlayerModel::new());
    player.update(cx, |m, _| {
        m.set_volume_store(stores.volume.clone());
        m.set_autoplay_store(stores.autoplay.clone());
        m.set_normalize_store(stores.normalize.clone());
        m.set_gapless_store(stores.gapless.clone());
        m.set_latency_log(crate::latency::default_log());
    });
    let ports = mode.account_ports();
    let account = cx.new(|cx| AccountModel::new(ports.clone(), cx));
    let settings = cx.new(|_| {
        SettingsModel::new(
            SqliteSettings::default_location().map(|s| Arc::new(s) as Arc<dyn SettingsBackend>),
        )
    });
    // The saved output device and equalizer, and any change to them later.
    crate::audio::bind(&player, &settings, cx);
    // Plays are reported to YouTube Music only while "Add plays to history" is on.
    let reporter = mode.play_reporter(&ports.service);
    let history_on = settings.read(cx).get(&ADD_PLAYS_TO_HISTORY);
    player.update(cx, |m, _| {
        m.set_play_reporter(reporter);
        m.set_report_plays(history_on);
    });
    {
        let player = player.clone();
        cx.observe(&settings, move |settings, cx| {
            let on = settings.read(cx).get(&ADD_PLAYS_TO_HISTORY);
            player.update(cx, |m, _| m.set_report_plays(on));
        })
        .detach();
    }
    // The tray first: with one, "Start minimized to tray" opens the window hidden (no flash),
    // and without one the window always opens.
    #[cfg(windows)]
    let tray = crate::tray_win::Tray::create();
    #[cfg(windows)]
    let start_hidden = tray.is_some()
        && settings
            .read(cx)
            .get(&crate::settings::START_MINIMIZED_TO_TRAY);
    #[cfg(not(windows))]
    let start_hidden = false;
    let window = open_window(
        cx,
        player.clone(),
        account.clone(),
        settings.clone(),
        !start_hidden,
    );
    let lyrics = lyrics_service(&mode, stores.db_path.clone(), &ports);
    let _ = window.update(cx, |shell, _, cx| {
        shell.install_lyrics(lyrics, stores.lyrics_open.clone(), cx);
        shell.install_related(stores.related_open.clone(), cx);
        shell.install_library_sort(stores.library_sort.clone(), cx);
    });
    // Media keys and the SMTC overlay, on the window just opened.
    #[cfg(windows)]
    crate::smtc::install(cx, window, player.clone());
    // The notification-area icon: transport menu, and the close button hides to it.
    #[cfg(windows)]
    if let Some(tray) = tray {
        crate::tray_win::install(cx, window, tray, player.clone(), settings.clone());
    }
    // The PC around the player: the output device going away, sleep and wake, other apps playing.
    #[cfg(windows)]
    crate::pc_watch_win::install(cx, player.clone(), settings.clone());
    // Optional system-wide hotkeys (Settings > Keyboard).
    #[cfg(windows)]
    crate::hotkeys_win::install(cx, settings.clone(), player.clone());
    // Discord Rich Presence (Settings > Discord); inert until the Discord application id is set.
    crate::discord::install(cx, player.clone(), settings.clone());
    // A second launch asks this window to come forward, whether it is hidden in the tray,
    // minimized or behind others.
    #[cfg(windows)]
    {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        crate::single_instance::listen(move || {
            let _ = tx.send(());
        });
        cx.spawn(async move |cx| {
            while rx.recv().await.is_some() {
                cx.update(|cx| {
                    let _ = window.update(cx, |_, window, _| crate::tray_win::show(window));
                });
            }
        })
        .detach();
    }
    start_services(
        cx,
        mode,
        player.clone(),
        window,
        account,
        ports,
        stores,
    );

    cx.on_app_quit(move |cx| {
        player.update(cx, |m, _| m.shutdown());
        async {}
    })
    .detach();
    cx.activate(true);
    window
}

/// Where lyrics come from: LRCLIB, then YouTube Music's own plain lyrics, with the SQLite cache;
/// or the fixtures under the simulation.
fn lyrics_service(
    mode: &Mode,
    db_path: Option<std::path::PathBuf>,
    ports: &AccountPorts,
) -> Arc<LyricsService> {
    if matches!(mode, Mode::Sim | Mode::SimSignedOut) {
        return Arc::new(LyricsService::new(vec![Arc::new(SimLyrics)], None));
    }
    let cache = db_path.map(|path| Arc::new(SqliteCache::at(path)) as Arc<dyn LyricsCache>);
    let mut sources: Vec<Arc<dyn LyricsSource>> = vec![Arc::new(Lrclib::new())];
    #[cfg(feature = "real-services")]
    match crate::backend::real_lyrics::AccountLyrics::new(ports.service.clone()) {
        Ok(source) => sources.push(Arc::new(source)),
        Err(err) => eprintln!("YouTube Music lyrics unavailable: {err}"),
    }
    #[cfg(not(feature = "real-services"))]
    let _ = ports;
    Arc::new(LyricsService::new(sources, cache))
}

fn open_window(
    cx: &mut App,
    player: gpui::Entity<PlayerModel>,
    account: gpui::Entity<AccountModel>,
    settings: gpui::Entity<SettingsModel>,
    show: bool,
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
                show,
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Shell::with_settings(player, account, settings, cx)),
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

/// What a previous run left behind, read before the player is installed.
struct Saved {
    volume: Option<u8>,
    autoplay: Option<bool>,
    normalize: Option<bool>,
    gapless: Option<bool>,
    queue: Option<(apollo_domain::queue::Queue, u64)>,
    queue_store: Option<Arc<dyn QueueStore>>,
    key: Option<String>,
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
    stores: Stores,
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
        let (prepared, saved) = cx
            .background_executor()
            .spawn(async move {
                let key = queue_account_key(&service);
                let saved = Saved {
                    volume: stores.volume.and_then(|s| s.load()),
                    autoplay: stores.autoplay.and_then(|s| s.load()),
                    normalize: stores.normalize.and_then(|s| s.load()),
                    gapless: stores.gapless.and_then(|s| s.load()),
                    queue: key
                        .as_deref()
                        .zip(stores.queue.as_ref())
                        .and_then(|(key, s)| s.load(key)),
                    queue_store: stores.queue,
                    key,
                };
                (mode.prepare(sink, &service), saved)
            })
            .await;
        match prepared {
            Ok(prepared) => {
                let (search, thumbs) = (prepared.search.clone(), prepared.thumbs.clone());
                let _ = weak.update(cx, |m, cx| {
                    m.restore_volume(saved.volume);
                    m.restore_autoplay(saved.autoplay);
                    m.restore_normalize(saved.normalize);
                    m.restore_gapless(saved.gapless);
                    m.set_queue_store(saved.queue_store, saved.key);
                    m.install_with(prepared, saved.queue, cx);
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
