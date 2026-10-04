//! The Windows notification-area icon (the `tray-icon` crate): icon, tooltip, native popup menu,
//! and the window hide/show it drives. The icon's hidden window lives on GPUI's own thread and
//! is serviced by GPUI's message loop, so there is no second event loop (ADR 0001, like the
//! SMTC bridge). Click and menu events arrive in the window procedure and are forwarded down a
//! channel; a GPUI task applies them on the UI thread.
//!
//! The decisions (tooltip text, when close hides, double-click filtering) are in `tray.rs`.

use std::ffi::c_void;
use std::time::Instant;

use apollo_theme::Accent;
use gpui::{App, AppContext, Entity, Window, WindowHandle};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::player::PlayerModel;
use crate::settings::{SettingsModel, CLOSE_TO_TRAY};
use crate::sleep_timer::SleepChoice;
use crate::smtc::hwnd_of;
use crate::tray::{
    icon_rgba, on_close_requested, ClickGate, CloseAction, TrayBridge, TrayCommand, TraySink,
    APP_NAME,
};
use crate::ui::shell::Shell;

const ICON_SIZE: u32 = 32;
const ID_PLAY_PAUSE: &str = "tray-play-pause";
const ID_NEXT: &str = "tray-next";
const ID_PREVIOUS: &str = "tray-previous";
const ID_MINI: &str = "tray-mini-player";
const ID_SHOW: &str = "tray-show";
const ID_SLEEP_OFF: &str = "tray-sleep-off";
const ID_SLEEP_END_OF_TRACK: &str = "tray-sleep-end-of-track";
const ID_QUIT: &str = "tray-quit";

const SW_HIDE: i32 = 0;
const SW_SHOW: i32 = 5;
const SW_RESTORE: i32 = 9;
const WM_CLOSE: u32 = 0x0010;

#[link(name = "user32")]
unsafe extern "system" {
    fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
    fn IsWindowVisible(hwnd: *mut c_void) -> i32;
    fn IsIconic(hwnd: *mut c_void) -> i32;
    fn PostMessageW(hwnd: *mut c_void, msg: u32, wparam: usize, lparam: isize) -> i32;
}

/// The tray icon and the channel its clicks arrive on. Created before the window so that
/// "start minimized" can open the window hidden, and handed to [`install`] afterwards.
pub struct Tray {
    sink: WinSink,
    commands: UnboundedReceiver<TrayCommand>,
}

struct WinSink {
    icon: TrayIcon,
    play_pause: MenuItem,
    /// The "Sleep timer" submenu's entries, one per choice.
    sleep: Vec<(SleepChoice, CheckMenuItem)>,
}

impl TraySink for WinSink {
    fn set_tooltip(&mut self, text: &str) {
        if let Err(err) = self.icon.set_tooltip(Some(text)) {
            eprintln!("tray: could not set tooltip: {err}");
        }
    }

    fn set_playing(&mut self, playing: bool) {
        self.play_pause
            .set_text(if playing { "Pause" } else { "Play" });
    }

    fn set_sleep(&mut self, choice: SleepChoice) {
        for (c, item) in &self.sleep {
            item.set_checked(*c == choice);
        }
    }
}

/// The menu id of a sleep timer entry (`tray-sleep-15`, ...).
fn sleep_id(choice: SleepChoice) -> String {
    match choice {
        SleepChoice::Off => ID_SLEEP_OFF.to_owned(),
        SleepChoice::Minutes(m) => format!("tray-sleep-{m}"),
        SleepChoice::EndOfTrack => ID_SLEEP_END_OF_TRACK.to_owned(),
    }
}

impl Tray {
    /// Adds the icon to the notification area. `None` (after saying why on stderr) leaves the
    /// app without a tray: the window then closes normally and always opens visible.
    pub fn create() -> Option<Self> {
        let (tx, commands) = unbounded_channel::<TrayCommand>();

        let play_pause = MenuItem::with_id(ID_PLAY_PAUSE, "Play", true, None);
        let next = MenuItem::with_id(ID_NEXT, "Next", true, None);
        let previous = MenuItem::with_id(ID_PREVIOUS, "Previous", true, None);
        let mini = MenuItem::with_id(ID_MINI, "Mini player", true, None);
        let show = MenuItem::with_id(ID_SHOW, "Show Apollo", true, None);
        let quit = MenuItem::with_id(ID_QUIT, "Quit", true, None);
        let sleep: Vec<(SleepChoice, CheckMenuItem)> = SleepChoice::MENU
            .iter()
            .map(|c| {
                (
                    *c,
                    CheckMenuItem::with_id(sleep_id(*c), c.label(), true, *c == SleepChoice::Off, None),
                )
            })
            .collect();
        let sleep_menu = Submenu::new("Sleep timer", true);
        for (_, item) in &sleep {
            sleep_menu
                .append(item)
                .map_err(|err| eprintln!("tray: could not build the sleep menu: {err}"))
                .ok()?;
        }
        let menu = Menu::new();
        menu.append_items(&[
            &play_pause,
            &next,
            &previous,
            &PredefinedMenuItem::separator(),
            &mini,
            &sleep_menu,
            &PredefinedMenuItem::separator(),
            &show,
            &quit,
        ])
        .map_err(|err| eprintln!("tray: could not build the menu: {err}"))
        .ok()?;

        let rgba = icon_rgba(ICON_SIZE, Accent::fallback());
        let icon = Icon::from_rgba(rgba, ICON_SIZE, ICON_SIZE)
            .map_err(|err| eprintln!("tray: could not build the icon: {err}"))
            .ok()?;

        let menu_tx = tx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some(command) = menu_command(event.id.as_ref()) {
                let _ = menu_tx.send(command);
            }
        }));
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = tx.send(TrayCommand::ToggleWindow);
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_icon(icon)
            .with_tooltip(APP_NAME)
            .with_menu(Box::new(menu))
            // Left click shows or hides the window; the menu is the right click's.
            .with_menu_on_left_click(false)
            .build()
            .map_err(|err| eprintln!("tray: could not add the icon: {err}"))
            .ok()?;
        Some(Self {
            sink: WinSink {
                icon,
                play_pause,
                sleep,
            },
            commands,
        })
    }
}

fn menu_command(id: &str) -> Option<TrayCommand> {
    Some(match id {
        ID_PLAY_PAUSE => TrayCommand::PlayPause,
        ID_NEXT => TrayCommand::Next,
        ID_PREVIOUS => TrayCommand::Previous,
        ID_MINI => TrayCommand::MiniPlayer,
        ID_SHOW => TrayCommand::ShowWindow,
        ID_QUIT => TrayCommand::Quit,
        other => {
            return SleepChoice::MENU
                .iter()
                .find(|c| sleep_id(**c) == other)
                .map(|c| TrayCommand::Sleep(*c))
        }
    })
}

/// Hooks the tray to the player and the window for the life of the app: the icon follows the
/// player, its commands are applied, and the window's close button hides it while "Close to
/// tray" is on.
pub fn install(
    cx: &mut App,
    window: WindowHandle<Shell>,
    tray: Tray,
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
) {
    let Tray { sink, mut commands } = tray;

    struct Host {
        /// Dropped on quit so the icon leaves the notification area with the app instead of
        /// lingering as a ghost until the pointer passes over it.
        bridge: Option<TrayBridge<WinSink>>,
    }
    let host = cx.new(|cx| {
        cx.observe(&player, |host: &mut Host, player, cx| {
            let (state, sleep) = {
                let p = player.read(cx);
                (p.media_state(), p.sleep_choice())
            };
            if let Some(bridge) = host.bridge.as_mut() {
                bridge.apply(&state);
                bridge.apply_sleep(sleep);
            }
        })
        .detach();
        Host {
            bridge: Some(TrayBridge::new(sink)),
        }
    });
    let (state, sleep) = {
        let p = player.read(cx);
        (p.media_state(), p.sleep_choice())
    };
    host.update(cx, |host, _| {
        if let Some(bridge) = host.bridge.as_mut() {
            bridge.apply(&state);
            bridge.apply_sleep(sleep);
        }
    });

    let _ = window.update(cx, |_, window, cx| {
        window.on_window_should_close(cx, move |window, cx| {
            let close_to_tray = settings.read(cx).get(&CLOSE_TO_TRAY);
            match on_close_requested(close_to_tray, true) {
                CloseAction::Hide => {
                    hide(window);
                    false
                }
                CloseAction::Close => true,
            }
        });
    });

    let weak = player.downgrade();
    let host_for_commands = host.clone();
    cx.spawn(async move |cx| {
        let mut gate = ClickGate::default();
        while let Some(command) = commands.recv().await {
            if matches!(command, TrayCommand::ToggleWindow) && !gate.accept(Instant::now()) {
                continue;
            }
            cx.update(|cx| match command {
                // With the mini player up, the main window is the one that is away: a click on
                // the icon or "Show Apollo" brings it back and the mini player gives way.
                TrayCommand::ToggleWindow if crate::mini::is_open(cx) => {
                    crate::mini::show_main(cx, window);
                }
                TrayCommand::ToggleWindow => {
                    let _ = window.update(cx, |_, window, _| toggle(window));
                }
                TrayCommand::ShowWindow => crate::mini::show_main(cx, window),
                TrayCommand::MiniPlayer => crate::mini::open(cx),
                TrayCommand::Quit => cx.quit(),
                TrayCommand::Sleep(choice) => {
                    let _ = weak.update(cx, |m, cx| m.set_sleep(choice, cx));
                    // The menu flipped the clicked entry's mark by itself: put it back to what
                    // the player now says.
                    let now = weak.read_with(cx, |m, _| m.sleep_choice()).unwrap_or(choice);
                    host_for_commands.update(cx, |host, _| {
                        if let Some(bridge) = host.bridge.as_mut() {
                            bridge.force_sleep(now);
                        }
                    });
                }
                transport => {
                    if let Some(os) = transport.player_command() {
                        let _ = weak.update(cx, |m, cx| m.os_command(os, cx));
                    }
                }
            });
        }
    })
    .detach();

    cx.on_app_quit(move |cx| {
        host.update(cx, |host, _| host.bridge = None);
        async {}
    })
    .detach();
}

/// Whether the window is on screen: shown and not minimized to the taskbar.
fn is_showing(hwnd: *mut c_void) -> bool {
    // SAFETY: plain queries on a window handle GPUI owns.
    unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 }
}

/// Hides the window (and its taskbar button). Playback and the app carry on.
pub fn hide(window: &Window) {
    if let Some(hwnd) = hwnd_of(window) {
        // SAFETY: SW_HIDE on a window handle GPUI owns.
        unsafe { ShowWindow(hwnd, SW_HIDE) };
    }
}

/// Asks the window to close the way its own close button does, so the close-to-tray choice
/// applies. Returns false when there is no native window to ask.
pub fn request_close(window: &Window) -> bool {
    let Some(hwnd) = hwnd_of(window) else {
        return false;
    };
    // SAFETY: posts WM_CLOSE to a window handle GPUI owns; the window procedure decides.
    unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0 }
}

/// Brings the window back (restoring it if minimized) and to the front. A window that was opened
/// hidden gets its first placement from GPUI's `activate_window`.
pub fn show(window: &mut Window) {
    if let Some(hwnd) = hwnd_of(window) {
        // SAFETY: shows a window handle GPUI owns; SW_RESTORE undoes a minimize.
        unsafe {
            let cmd = if IsIconic(hwnd) != 0 {
                SW_RESTORE
            } else {
                SW_SHOW
            };
            ShowWindow(hwnd, cmd);
        }
    }
    window.activate_window();
}

fn toggle(window: &mut Window) {
    let showing = hwnd_of(window).is_some_and(is_showing);
    if showing {
        hide(window);
    } else {
        show(window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_commands() {
        assert_eq!(menu_command(ID_PLAY_PAUSE), Some(TrayCommand::PlayPause));
        assert_eq!(menu_command(ID_NEXT), Some(TrayCommand::Next));
        assert_eq!(menu_command(ID_PREVIOUS), Some(TrayCommand::Previous));
        assert_eq!(menu_command(ID_MINI), Some(TrayCommand::MiniPlayer));
        assert_eq!(menu_command(ID_SHOW), Some(TrayCommand::ShowWindow));
        assert_eq!(menu_command(ID_QUIT), Some(TrayCommand::Quit));
        assert_eq!(menu_command("other"), None);
    }

    #[test]
    fn every_sleep_entry_has_its_own_id_that_maps_back() {
        let mut ids = std::collections::HashSet::new();
        for choice in SleepChoice::MENU {
            let id = sleep_id(choice);
            assert!(ids.insert(id.clone()), "{id} repeats");
            assert_eq!(menu_command(&id), Some(TrayCommand::Sleep(choice)));
        }
    }
}
