//! Settings > Playback > Preload songs on hover, between the saved setting and the player: the
//! player resolves ahead of a click on hover, a resting keyboard cursor or a page opening only
//! while the switch is on (it is off until the owner turns it on; see
//! [`crate::settings::PRELOAD_ON_HOVER`]).

use gpui::{App, Entity};

use crate::player::PlayerModel;
use crate::settings::{SettingsModel, PRELOAD_ON_HOVER};

/// Give the player the saved switch now and every change of it later.
pub fn bind(player: &Entity<PlayerModel>, settings: &Entity<SettingsModel>, cx: &mut App) {
    let on = settings.read(cx).get(&PRELOAD_ON_HOVER);
    player.update(cx, |p, _| p.set_preload_hover(on));
    let player = player.clone();
    cx.observe(settings, move |settings, cx| {
        let on = settings.read(cx).get(&PRELOAD_ON_HOVER);
        if player.read(cx).preload_hover() != on {
            player.update(cx, |p, _| p.set_preload_hover(on));
        }
    })
    .detach();
}
