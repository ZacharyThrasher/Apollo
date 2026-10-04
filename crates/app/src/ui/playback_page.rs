//! Settings > Playback: the playback switches. Normalize volume, Gapless playback, Add plays to
//! history and Pause when other audio plays, then Blocked artists: how many there are and a
//! Manage button that opens the list.

use gpui::{div, prelude::*, Context, Entity, FocusHandle, IntoElement, Render, Window};

use crate::player::PlayerModel;
use crate::settings::{SettingsModel, ADD_PLAYS_TO_HISTORY, PAUSE_ON_OTHER_AUDIO};
use crate::ui::account_page::TAB_INDEX;
use crate::ui::blocked_dialog;
use crate::ui::settings_row::{SettingsRow, SettingsSection};
use crate::ui::text_button::TextButton;

/// The Blocked artists row.
pub const ROW_BLOCKED: &str = "setting-blocked-artists";

/// What the row says about the list: a count, or "None".
pub fn blocked_fact(count: usize) -> String {
    if count == 0 {
        "None".to_owned()
    } else {
        count.to_string()
    }
}

pub struct PlaybackPage {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    focus_normalize: FocusHandle,
    focus_gapless: FocusHandle,
    focus_history: FocusHandle,
    focus_other_audio: FocusHandle,
    focus_blocked: FocusHandle,
    focus_manage: FocusHandle,
}

impl PlaybackPage {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        if let Some(blocked) = crate::blocked_artists::handle(cx) {
            // The count follows the list.
            cx.observe(&blocked, |_, _, cx| cx.notify()).detach();
        }
        Self {
            player,
            settings,
            focus_normalize: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_gapless: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_history: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_other_audio: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_blocked: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_manage: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
        }
    }
}

impl Render for PlaybackPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (normalize, gapless, accent) = {
            let p = self.player.read(cx);
            (p.normalize(), p.gapless(), p.view().accent)
        };
        let history = self.settings.read(cx).get(&ADD_PLAYS_TO_HISTORY);
        let (for_normalize, for_gapless) = (self.player.clone(), self.player.clone());
        let other_audio = self.settings.read(cx).get(&PAUSE_ON_OTHER_AUDIO);
        let settings = self.settings.clone();
        let for_history = self.settings.clone();
        let blocked_count = crate::blocked_artists::handle(cx).map_or(0, |b| b.read(cx).len());
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
            .row(
                SettingsRow::switch(
                    "setting-add-plays-to-history",
                    &self.focus_history,
                    "Add plays to history",
                    history,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    for_history.update(cx, |m, cx| m.set(&ADD_PLAYS_TO_HISTORY, value, cx));
                }),
            )
            .row(
                SettingsRow::switch(
                    "setting-pause-on-other-audio",
                    &self.focus_other_audio,
                    "Pause when other audio plays",
                    other_audio,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    settings.update(cx, |m, cx| m.set(&PAUSE_ON_OTHER_AUDIO, value, cx));
                }),
            )
            .row(
                SettingsRow::manage(
                    ROW_BLOCKED,
                    &self.focus_blocked,
                    "Blocked artists",
                    blocked_fact(blocked_count),
                    accent,
                )
                .action(
                    div().debug_selector(|| "setting-blocked-artists-manage".into()).child(
                        TextButton::new(
                            "setting-blocked-artists-manage-button",
                            &self.focus_manage,
                            "Manage",
                            accent,
                        )
                        .on_click(|window, cx| blocked_dialog::open(window, cx)),
                    ),
                )
                .on_open(|window, cx| blocked_dialog::open(window, cx)),
            )
    }
}
