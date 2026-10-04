//! System-wide hotkeys (the `global-hotkey` crate, `RegisterHotKey` underneath): Ctrl+Alt with
//! Space, Left, Right, Up or Down work while Apollo is hidden or something else has focus.
//! The crate's hidden window lives on GPUI's own thread and is serviced by GPUI's message loop,
//! so there is no second event loop (ADR 0001, like the tray icon).
//!
//! They are registered only while Settings > Keyboard > Global hotkeys is on. A hotkey another
//! program already owns is skipped, with a line on stderr; the rest still work. What each key
//! does is `keymap::GlobalCommand::apply`.

use std::collections::HashMap;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{App, AppContext, Entity};
use tokio::sync::mpsc::unbounded_channel;

use crate::keymap::{GlobalCommand, GlobalKey, GLOBAL_BINDINGS};
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, GLOBAL_HOTKEYS};

fn hotkey(key: GlobalKey) -> HotKey {
    let code = match key {
        GlobalKey::Space => Code::Space,
        GlobalKey::Left => Code::ArrowLeft,
        GlobalKey::Right => Code::ArrowRight,
        GlobalKey::Up => Code::ArrowUp,
        GlobalKey::Down => Code::ArrowDown,
        GlobalKey::H => Code::KeyH,
    };
    HotKey::new(Some(Modifiers::CONTROL | Modifiers::ALT), code)
}

/// The registrations Windows has accepted, by hotkey id.
struct Registered {
    manager: GlobalHotKeyManager,
    active: HashMap<u32, (HotKey, GlobalCommand)>,
}

impl Registered {
    fn set_enabled(&mut self, on: bool) {
        if !on {
            for (hotkey, _) in self.active.drain().map(|(_, entry)| entry) {
                let _ = self.manager.unregister(hotkey);
            }
            return;
        }
        for (command, key) in GLOBAL_BINDINGS {
            let hotkey = hotkey(key);
            if self.active.contains_key(&hotkey.id()) {
                continue;
            }
            match self.manager.register(hotkey) {
                Ok(()) => {
                    self.active.insert(hotkey.id(), (hotkey, command));
                }
                // Most likely another program owns the combination: skip it quietly.
                Err(err) => eprintln!("hotkeys: could not register {command:?}: {err}"),
            }
        }
    }
}

/// Follows the Global hotkeys setting for the life of the app. Does nothing (after saying why
/// on stderr) when Windows will not give the app a hotkey window.
pub fn install(cx: &mut App, settings: Entity<SettingsModel>, player: Entity<PlayerModel>) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(err) => {
            eprintln!("hotkeys: unavailable: {err}");
            return;
        }
    };

    let (tx, mut rx) = unbounded_channel::<u32>();
    GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
        if event.state == HotKeyState::Pressed {
            let _ = tx.send(event.id);
        }
    }));

    let registered = cx.new(|cx| {
        cx.observe(&settings, |registered: &mut Registered, settings, cx| {
            let on = settings.read(cx).get(&GLOBAL_HOTKEYS);
            registered.set_enabled(on);
        })
        .detach();
        Registered {
            manager,
            active: HashMap::new(),
        }
    });
    let on = settings.read(cx).get(&GLOBAL_HOTKEYS);
    registered.update(cx, |registered, _| registered.set_enabled(on));

    let weak_registered = registered.downgrade();
    let weak_player = player.downgrade();
    cx.spawn(async move |cx| {
        while let Some(id) = rx.recv().await {
            let command = weak_registered
                .read_with(cx, |r, _| r.active.get(&id).map(|(_, command)| *command))
                .ok()
                .flatten();
            if command == Some(GlobalCommand::ToggleHud) {
                cx.update(|cx| crate::mini::toggle_visibility(cx));
            } else if let Some(command) = command {
                let _ = weak_player.update(cx, |player, cx| command.apply(player, cx));
            }
        }
    })
    .detach();

    // The registrations leave with the app instead of lingering until the process is gone.
    cx.on_app_quit(move |cx| {
        registered.update(cx, |registered, _| registered.set_enabled(false));
        async {}
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_binding_is_ctrl_alt_and_has_its_own_id() {
        let mut ids = std::collections::HashSet::new();
        for (_, key) in GLOBAL_BINDINGS {
            let hotkey = hotkey(key);
            assert_eq!(hotkey.mods, Modifiers::CONTROL | Modifiers::ALT);
            assert!(ids.insert(hotkey.id()));
        }
    }
}
