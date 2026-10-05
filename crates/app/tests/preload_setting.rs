//! Settings > Playback > Preload songs on hover: a switch, off until turned on, that the player
//! follows. The real Settings page and the binding the app installs, on GPUI's test platform.

use std::sync::{Arc, Mutex};

use apollo_app::account::AccountModel;
use apollo_app::player::PlayerModel;
use apollo_app::settings::{SettingsBackend, SettingsModel, PRELOAD_ON_HOVER};
use apollo_app::style::Fonts;
use apollo_app::ui::nav::Route;
use apollo_app::ui::shell::{self, Shell};
use gpui::{AppContext, Modifiers, TestAppContext};

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

#[gpui::test]
fn the_playback_row_is_off_by_default_flips_and_the_player_follows(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        Fonts::install(cx);
        shell::bind_keys(cx);
    });
    let player = cx.new(|_| PlayerModel::new());
    let settings = cx.new(|_| SettingsModel::new(Some(Arc::new(Memory::default()))));
    cx.update(|cx| apollo_app::preload::bind(&player, &settings, cx));
    let (shell, vcx) = cx.add_window_view({
        let (player, settings) = (player.clone(), settings.clone());
        move |_, cx| {
            let ports = apollo_app::backend::account::ports();
            apollo_app::backend::account::start(&ports, true);
            let account = cx.new(|cx| AccountModel::new(ports, cx));
            account.update(cx, |m, cx| m.startup_finished(cx));
            Shell::with_settings(player, account, settings, cx)
        }
    });
    shell.update(vcx, |s, cx| s.show_route(Route::Settings, cx));
    vcx.run_until_parked();

    let gapless = vcx.debug_bounds("setting-gapless-playback").expect("gapless row");
    let row = vcx.debug_bounds("setting-preload-on-hover").expect("the Preload songs on hover row");
    assert_eq!(row.size.height, gpui::px(48.0), "a settings row is 48px");
    assert!(row.top() >= gapless.bottom(), "in the Playback card, under Gapless playback");
    assert!(!settings.read_with(vcx, |m, _| m.get(&PRELOAD_ON_HOVER)), "off by default");
    assert!(!player.read_with(vcx, |m, _| m.preload_hover()));

    vcx.simulate_click(row.center(), Modifiers::none());
    vcx.run_until_parked();
    assert!(settings.read_with(vcx, |m, _| m.get(&PRELOAD_ON_HOVER)));
    assert!(player.read_with(vcx, |m, _| m.preload_hover()), "the player follows the switch");

    vcx.simulate_click(row.center(), Modifiers::none());
    vcx.run_until_parked();
    assert!(!settings.read_with(vcx, |m, _| m.get(&PRELOAD_ON_HOVER)));
    assert!(!player.read_with(vcx, |m, _| m.preload_hover()));
}

#[gpui::test]
fn a_saved_on_is_applied_at_startup(cx: &mut TestAppContext) {
    let backend = Arc::new(Memory::default());
    backend.set(PRELOAD_ON_HOVER.key, "true");
    let player = cx.new(|_| PlayerModel::new());
    let settings = cx.new(|_| SettingsModel::new(Some(backend)));
    cx.update(|cx| apollo_app::preload::bind(&player, &settings, cx));
    assert!(player.read_with(cx, |m, _| m.preload_hover()));
}
