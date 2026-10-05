//! Settings > Playback: the playback switches. Normalize volume, Gapless playback, Preload songs
//! on hover, Add plays to history and Pause when other audio plays, then Skim (a switch) with its Seconds per song
//! select, then Blocked artists: how many there are and a Manage button that opens the list.

use gpui::{div, prelude::*, Context, Entity, FocusHandle, IntoElement, Render, Window};

use crate::player::PlayerModel;
use crate::player::SKIM_SECONDS_CHOICES;
use crate::settings::{
    SettingsModel, ADD_PLAYS_TO_HISTORY, PAUSE_ON_OTHER_AUDIO, PICKER_KEY, PICKER_STAY_OPEN,
    PRELOAD_ON_HOVER, SKIM, SKIM_SECONDS,
};
use crate::ui::account_page::TAB_INDEX;
use crate::ui::blocked_dialog;
use crate::ui::choice_menu::{self, Choice};
use crate::ui::settings_row::{anchor_key, SettingsRow, SettingsSection};
use crate::ui::text_button::TextButton;

/// The Blocked artists row.
pub const ROW_BLOCKED: &str = "setting-blocked-artists";
/// The Preload songs on hover switch.
pub const ROW_PRELOAD: &str = "setting-preload-on-hover";
/// The Skim switch and its Seconds per song select.
pub const ROW_SKIM: &str = "setting-skim";
pub const ROW_SKIM_SECONDS: &str = "setting-skim-seconds";
const SKIM_MENU_WIDTH: f32 = 160.0;

/// What the Seconds per song row says and its choices offer: "20 seconds".
pub fn seconds_text(seconds: u32) -> String {
    format!("{seconds} seconds")
}

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
    focus_preload: FocusHandle,
    focus_history: FocusHandle,
    focus_other_audio: FocusHandle,
    focus_picker_key: FocusHandle,
    focus_picker_stay: FocusHandle,
    focus_skim: FocusHandle,
    focus_skim_seconds: FocusHandle,
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
            focus_preload: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_history: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_other_audio: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_picker_key: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_picker_stay: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_skim: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            focus_skim_seconds: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
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
        let preload = self.settings.read(cx).get(&PRELOAD_ON_HOVER);
        let for_preload = self.settings.clone();
        let (for_normalize, for_gapless) = (self.player.clone(), self.player.clone());
        let other_audio = self.settings.read(cx).get(&PAUSE_ON_OTHER_AUDIO);
        let (picker_key, picker_stay) = {
            let s = self.settings.read(cx);
            (s.get(&PICKER_KEY), s.get(&PICKER_STAY_OPEN))
        };
        let (for_picker_key, for_picker_stay) = (self.settings.clone(), self.settings.clone());
        let settings = self.settings.clone();
        let skim = self.settings.read(cx).get(&SKIM);
        let skim_seconds = self.settings.read(cx).skim_seconds();
        let (for_skim, for_seconds) = (self.settings.clone(), self.settings.clone());
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
                    ROW_PRELOAD,
                    &self.focus_preload,
                    "Preload songs on hover",
                    preload,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    for_preload.update(cx, |m, cx| m.set(&PRELOAD_ON_HOVER, value, cx));
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
                SettingsRow::switch(
                    "setting-picker-key",
                    &self.focus_picker_key,
                    "P opens the playlist picker",
                    picker_key,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    for_picker_key.update(cx, |m, cx| m.set(&PICKER_KEY, value, cx));
                }),
            )
            .row(
                SettingsRow::switch(
                    "setting-picker-stay-open",
                    &self.focus_picker_stay,
                    "Keep picker open after adding",
                    picker_stay,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    for_picker_stay.update(cx, |m, cx| m.set(&PICKER_STAY_OPEN, value, cx));
                }),
            )
            .row(
                SettingsRow::switch(ROW_SKIM, &self.focus_skim, "Skim", skim, accent).on_toggle(
                    move |value, _, cx| {
                        for_skim.update(cx, |m, cx| m.set(&SKIM, value, cx));
                    },
                ),
            )
            .row(
                SettingsRow::select(
                    ROW_SKIM_SECONDS,
                    &self.focus_skim_seconds,
                    "Seconds per song",
                    seconds_text(skim_seconds),
                    accent,
                )
                .on_open(move |window, cx| {
                    let choices = SKIM_SECONDS_CHOICES
                        .iter()
                        .map(|s| Choice::new(seconds_text(*s), *s == skim_seconds))
                        .collect();
                    let settings = for_seconds.clone();
                    choice_menu::open(
                        &anchor_key(ROW_SKIM_SECONDS),
                        SKIM_MENU_WIDTH,
                        choices,
                        move |index, _, cx| {
                            if let Some(seconds) = SKIM_SECONDS_CHOICES.get(index) {
                                settings.update(cx, |m, cx| {
                                    m.set_text(&SKIM_SECONDS, &seconds.to_string(), cx)
                                });
                            }
                        },
                        window,
                        cx,
                    );
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
