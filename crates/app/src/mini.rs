//! Switching between the main window and the mini player.
//!
//! Opening the mini player hides the main window (playback carries on) and shows the mini
//! window where it was last left; going back closes the mini window and brings the main window
//! forward. Only one of the two is on screen at a time, so everything that asks for the main
//! window (the tray, a second launch of the app) goes through [`show_main`] and the mini
//! player gives way.
//!
//! The view is `ui/mini_player.rs`; the remembered position is `window_state.rs`.

use std::sync::Arc;

use gpui::{
    App, AppContext, Entity, Global, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowKind, WindowOptions,
};

use crate::player::PlayerModel;
use crate::settings::{SettingsBackend, SettingsModel};
use crate::ui::mini_player::{self, MiniPlayer};
use crate::ui::shell::Shell;
use crate::window_state::{self, Constraints, Home, KEY_MINI};

/// Where the mini player parks the first time, and the one size it ever has.
pub const PLACEMENT: Constraints = Constraints {
    default_size: (mini_player::WIDTH, mini_player::HEIGHT),
    min_size: (mini_player::WIDTH, mini_player::HEIGHT),
    home: Home::BottomRight,
};

/// What the controller needs, kept for the life of the app.
struct Host {
    main: WindowHandle<Shell>,
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    store: Option<Arc<dyn SettingsBackend>>,
    mini: Option<WindowHandle<MiniPlayer>>,
}

impl Global for Host {}

/// Makes the mini player available. `store` keeps its position between runs (none: it opens
/// in its corner each time).
pub fn install(
    cx: &mut App,
    main: WindowHandle<Shell>,
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    store: Option<Arc<dyn SettingsBackend>>,
) {
    cx.set_global(Host {
        main,
        player,
        settings,
        store,
        mini: None,
    });
}

/// The mini player's window, if it is open.
pub fn window(cx: &App) -> Option<WindowHandle<MiniPlayer>> {
    cx.try_global::<Host>()?
        .mini
        .filter(|mini| mini.read(cx).is_ok())
}

pub fn is_open(cx: &App) -> bool {
    window(cx).is_some()
}

/// Opens the mini player and hides the main window; with it already open, brings it forward.
pub fn open(cx: &mut App) {
    if let Some(mini) = window(cx) {
        let _ = mini.update(cx, |_, window, _| window.activate_window());
        return;
    }
    let Some(host) = cx.try_global::<Host>() else {
        return;
    };
    let (main, player, settings, store) = (
        host.main,
        host.player.clone(),
        host.settings.clone(),
        host.store.clone(),
    );
    let place = window_state::restore(cx, store.as_ref(), KEY_MINI, &PLACEMENT);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(place.rect.to_bounds())),
            titlebar: None,
            // Floating above other windows and off the taskbar; the always-on-top setting
            // takes it back down.
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::MicaBackdrop,
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            focus: true,
            show: true,
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx| MiniPlayer::new(player, settings, |_, cx| cx.defer(return_to_main), cx))
        },
    );
    let mini = match opened {
        Ok(mini) => mini,
        Err(err) => {
            // The main window stays up: a mini player that cannot open must not take it away.
            eprintln!("mini player: could not open the window: {err}");
            return;
        }
    };
    let _ = mini.update(cx, |view, window, cx| {
        view.focus_root(window, cx);
        // Closing it any other way (Alt+F4) still brings the main window back.
        window.on_window_should_close(cx, |_, cx| {
            cx.defer(return_to_main);
            true
        });
        // Its position is remembered like the main window's.
        let remembered = window_state::remember(window, cx, store, KEY_MINI, false);
        view.keep(remembered);
    });
    if cx.has_global::<Host>() {
        cx.global_mut::<Host>().mini = Some(mini);
    }
    let _ = main.update(cx, |_, window, _| crate::platform::hide_window(window));
}

/// Closes the mini player, leaving the main window as it is.
pub fn dismiss(cx: &mut App) {
    let Some(mini) = window(cx) else {
        return;
    };
    let _ = mini.update(cx, |_, window, _| window.remove_window());
    if cx.has_global::<Host>() {
        cx.global_mut::<Host>().mini = None;
    }
}

/// Closes the mini player and brings the main window back to the front.
pub fn return_to_main(cx: &mut App) {
    dismiss(cx);
    let Some(main) = cx.try_global::<Host>().map(|h| h.main) else {
        return;
    };
    let _ = main.update(cx, |_, window, _| crate::platform::show_window(window));
}

/// Brings the main window forward for the tray or a second launch, whichever of the two
/// windows was showing.
pub fn show_main(cx: &mut App, main: WindowHandle<Shell>) {
    dismiss(cx);
    let _ = main.update(cx, |_, window, _| crate::platform::show_window(window));
}
