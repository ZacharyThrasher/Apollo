//! Settings > Playback: the playback switches. Normalize volume and Gapless playback.

use gpui::{Context, Entity, FocusHandle, IntoElement, Render, Window};

use crate::player::PlayerModel;
use crate::ui::account_page::TAB_INDEX;
use crate::ui::settings_row::{SettingsRow, SettingsSection};

pub struct PlaybackPage {
    player: Entity<PlayerModel>,
    focus_normalize: FocusHandle,
    focus_gapless: FocusHandle,
}

impl PlaybackPage {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            focus_normalize: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_gapless: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
        }
    }
}

impl Render for PlaybackPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (normalize, gapless, accent) = {
            let p = self.player.read(cx);
            (p.normalize(), p.gapless(), p.view().accent)
        };
        let (for_normalize, for_gapless) = (self.player.clone(), self.player.clone());
        SettingsSection::new("Playback")
            .row(
                SettingsRow::switch(
                    "setting-normalize-volume",
                    &self.focus_normalize,
                    "Normalize volume",
                    normalize,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    for_normalize.update(cx, |m, cx| m.set_normalize(value, cx));
                }),
            )
            .row(
                SettingsRow::switch(
                    "setting-gapless-playback",
                    &self.focus_gapless,
                    "Gapless playback",
                    gapless,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    for_gapless.update(cx, |m, cx| m.set_gapless(value, cx));
                }),
            )
    }
}
