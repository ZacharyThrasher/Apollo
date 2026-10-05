//! Switching between the main window and the mini player.
//!
//! Opening the mini player hides the main window (playback carries on) and shows the mini
//! window where it was last left; going back closes the mini window and brings the main window
//! forward. Only one of the two is on screen at a time, so everything that asks for the main
//! window (the tray, a second launch of the app) goes through [`show_main`] and the mini
//! player gives way.
//!
//! A main window expanded *from* the mini player (the expand button, double-clicking the art,
//! Esc) is the mini player's guest: its close button collapses it back to the mini player
//! rather than closing or hiding it, and, with "Collapse when Apollo loses focus" on, it
//! collapses by itself [`COLLAPSE_GRACE`] after Apollo stops being the active app (cancelled if
//! Apollo comes back first). Showing the main window any other way (the tray, a second
//! launch) is ordinary: close keeps its usual meaning.
//!
//! The view is `ui/mini_player.rs`; the remembered position is `window_state.rs`.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, AppContext, Entity, Global, WindowBackgroundAppearance, WindowBounds, WindowHandle,
    WindowKind, WindowOptions,
};

use crate::player::PlayerModel;
use crate::settings::{SettingsBackend, SettingsModel, MINI_COLLAPSE_ON_BLUR, MINI_PLAYER_HUD};
use crate::ui::mini_player::{self, MiniPlayer};
use crate::ui::shell::Shell;
use crate::window_state::{self, Constraints, Home, KEY_HUD, KEY_MINI};

/// Where the mini player parks the first time, and the one size it ever has.
pub const PLACEMENT: Constraints = Constraints {
    default_size: (mini_player::WIDTH, mini_player::HEIGHT),
    min_size: (mini_player::WIDTH, mini_player::HEIGHT),
    home: Home::BottomRight,
    fixed: true,
};

/// The HUD's window: the panel and nothing round it, parked in the same corner.
pub const HUD_PLACEMENT: Constraints = Constraints {
    default_size: (mini_player::HUD_WIDTH, mini_player::HUD_HEIGHT),
    min_size: (mini_player::HUD_WIDTH, mini_player::HUD_HEIGHT),
    home: Home::BottomRight,
    fixed: true,
};

/// How long Apollo may be out of focus before an expanded main window collapses back.
pub const COLLAPSE_GRACE: Duration = Duration::from_millis(1500);

/// What the controller needs, kept for the life of the app.
struct Host {
    main: WindowHandle<Shell>,
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    store: Option<Arc<dyn SettingsBackend>>,
    mini: Option<WindowHandle<MiniPlayer>>,
    /// The mini player is open but hidden (the HUD hotkey).
    hidden: bool,
    /// The main window is showing because it was expanded from the mini player.
    from_mini: bool,
    /// Counts focus changes of the main window, so a pending collapse can tell it was
    /// overtaken.
    focus_changes: u64,
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
        hidden: false,
        from_mini: false,
        focus_changes: 0,
    });
    // The main window's close button, Alt+F4 and the taskbar's Close, wherever no tray icon
    // takes the request over (the tray's own handler asks `collapse_on_close` first).
    let _ = main.update(cx, |_, window, cx| {
        window.on_window_should_close(cx, |_, cx| !collapse_on_close(cx));
    });
    // Losing the keyboard to another app starts the grace period.
    let _ = main.update(cx, |_, window, cx| {
        cx.observe_window_activation(window, |_, window, cx| {
            main_activation_changed(window.is_window_active(), cx);
        })
        .detach();
    });
}

/// Whether the main window is showing as the mini player's guest.
pub fn expanded_from_mini(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.from_mini) && !is_open(cx)
}

/// A close request on the main window: when it was expanded from the mini player it collapses
/// back instead. True when it did (the window must then not close or hide).
pub fn collapse_on_close(cx: &mut App) -> bool {
    if !expanded_from_mini(cx) {
        return false;
    }
    // Deferred: the request comes from inside the main window's own update.
    cx.defer(open);
    true
}

/// The main window gained or lost the keyboard. An expanded window that lost it collapses after
/// [`COLLAPSE_GRACE`] unless it has it back by then.
pub fn main_activation_changed(active: bool, cx: &mut App) {
    if !cx.has_global::<Host>() {
        return;
    }
    let host = cx.global_mut::<Host>();
    host.focus_changes += 1;
    let seen = host.focus_changes;
    if active {
        return;
    }
    let (main, settings) = (host.main, host.settings.clone());
    if !expanded_from_mini(cx) || !settings.read(cx).get(&MINI_COLLAPSE_ON_BLUR) {
        return;
    }
    cx.spawn(async move |cx| {
        cx.background_executor().timer(COLLAPSE_GRACE).await;
        cx.update(|cx| {
            let unchanged = cx
                .try_global::<Host>()
                .is_some_and(|h| h.focus_changes == seen);
            let away = main
                .update(cx, |_, window, _| !window.is_window_active())
                .unwrap_or(false);
            if unchanged
                && away
                && expanded_from_mini(cx)
                && settings.read(cx).get(&MINI_COLLAPSE_ON_BLUR)
            {
                open(cx);
            }
        });
    })
    .detach();
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
    if cx.has_global::<Host>() {
        cx.global_mut::<Host>().from_mini = false;
    }
    if let Some(mini) = window(cx) {
        if cx.global::<Host>().hidden {
            set_hidden(cx, false);
        } else {
            let _ = mini.update(cx, |view, window, _| {
                if !view.is_hud() {
                    window.activate_window();
                }
            });
        }
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
    let hud = settings.read(cx).get(&MINI_PLAYER_HUD);
    // The HUD keeps its own place: it is a different size and sits somewhere else.
    let (key, placement) = if hud {
        (KEY_HUD, &HUD_PLACEMENT)
    } else {
        (KEY_MINI, &PLACEMENT)
    };
    let place = window_state::restore(cx, store.as_ref(), key, placement);
    let Ok((writes, lyrics)) = main.read_with(cx, |shell, _| {
        (shell.writes().clone(), shell.lyrics().clone())
    }) else {
        return;
    };
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(place.rect.to_bounds())),
            titlebar: None,
            // Floating above other windows and off the taskbar; the always-on-top setting
            // takes it back down.
            kind: WindowKind::PopUp,
            // The HUD is see-through and draws its own rounded fill; the plain mini player sits
            // on Mica like the main window.
            window_background: if hud {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::MicaBackdrop
            },
            is_movable: true,
            is_resizable: false,
            is_minimizable: false,
            // The HUD must never take the keyboard from a game.
            focus: !hud,
            show: true,
            ..Default::default()
        },
        |_, cx| {
            cx.new(|cx| {
                MiniPlayer::new(
                    player,
                    settings,
                    writes,
                    lyrics,
                    hud,
                    |_, cx| cx.defer(return_to_main),
                    cx,
                )
            })
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
        view.start(window, cx);
        if !hud {
            view.focus_root(window, cx);
        }
        // Closing it any other way (Alt+F4) still brings the main window back.
        window.on_window_should_close(cx, |_, cx| {
            cx.defer(return_to_main);
            true
        });
        // Its position is remembered like the main window's.
        let remembered = window_state::remember(window, cx, store, key, Some(placement.default_size));
        view.keep(remembered);
    });
    if cx.has_global::<Host>() {
        cx.global_mut::<Host>().mini = Some(mini);
    }
    let _ = main.update(cx, |_, window, _| crate::platform::hide_window(window));
}

/// Shows or hides the mini player in place (the HUD hotkey), leaving the main window alone;
/// opens it when it is not open at all.
pub fn toggle_visibility(cx: &mut App) {
    if !is_open(cx) {
        open(cx);
        return;
    }
    let hidden = cx.global::<Host>().hidden;
    set_hidden(cx, !hidden);
}

/// Whether the mini player is open but hidden.
pub fn is_hidden(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.hidden) && is_open(cx)
}

fn set_hidden(cx: &mut App, hidden: bool) {
    let Some(mini) = window(cx) else { return };
    let _ = mini.update(cx, |view, window, _| {
        if hidden {
            crate::platform::hide_window(window);
        } else if view.is_hud() {
            // Back without taking the keyboard from whatever has it.
            crate::platform::show_window_inactive(window);
        } else {
            crate::platform::show_window(window);
        }
    });
    cx.global_mut::<Host>().hidden = hidden;
}

/// Closes the mini player, leaving the main window as it is.
pub fn dismiss(cx: &mut App) {
    let Some(mini) = window(cx) else {
        return;
    };
    let _ = mini.update(cx, |_, window, _| window.remove_window());
    if cx.has_global::<Host>() {
        let host = cx.global_mut::<Host>();
        host.mini = None;
        host.hidden = false;
    }
}

/// Closes the mini player and brings the main window back to the front.
pub fn return_to_main(cx: &mut App) {
    dismiss(cx);
    let Some(main) = cx.try_global::<Host>().map(|h| h.main) else {
        return;
    };
    cx.global_mut::<Host>().from_mini = true;
    let _ = main.update(cx, |_, window, _| crate::platform::show_window(window));
}

/// Brings the main window forward for the tray or a second launch, whichever of the two
/// windows was showing.
pub fn show_main(cx: &mut App, main: WindowHandle<Shell>) {
    if cx.has_global::<Host>() {
        cx.global_mut::<Host>().from_mini = false;
    }
    dismiss(cx);
    let _ = main.update(cx, |_, window, _| crate::platform::show_window(window));
}
