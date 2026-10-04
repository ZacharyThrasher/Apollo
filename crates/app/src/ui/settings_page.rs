//! Settings: one scrolling column of sections. Account first (its own page), then the sections
//! of switches built from [`SettingsSection`] and [`SettingsRow`]: Playback, Search, Window,
//! Discord, then Keyboard.

use apollo_theme::tokens::spacing;
use gpui::{
    div, prelude::*, px, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, Styled,
    Window,
};

use crate::autostart;
use crate::player::PlayerModel;
use crate::search::{HIDE_MUSIC_VIDEOS, SAVE_RECENT_SEARCHES};
use crate::settings::{
    BoolSetting, SettingsModel, CLOSE_TO_TRAY, DISCORD_PRESENCE, GLOBAL_HOTKEYS,
    MINI_PLAYER_ON_TOP, START_MINIMIZED_TO_TRAY,
};
use crate::ui::account_page::{self, AccountPage};
use crate::ui::playback_page::PlaybackPage;
use crate::ui::settings_row::{SettingsRow, SettingsSection};

/// The column's widest extent; the page title shares it (see the shell).
pub const MAX_WIDTH: f32 = 720.0;

pub struct SettingsPage {
    account_page: Entity<AccountPage>,
    playback_page: Entity<PlaybackPage>,
    settings: Entity<SettingsModel>,
    player: Entity<PlayerModel>,
    focus: Vec<FocusHandle>,
    /// The sign-in launch as the registry last said; the registry is the source of truth.
    launch_at_signin: bool,
}

impl SettingsPage {
    pub fn new(
        account_page: Entity<AccountPage>,
        playback_page: Entity<PlaybackPage>,
        settings: Entity<SettingsModel>,
        player: Entity<PlayerModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&settings, |_, _, cx| cx.notify()).detach();
        Self {
            account_page,
            playback_page,
            settings,
            player,
            focus: Vec::new(),
            launch_at_signin: autostart::is_enabled(),
        }
    }

    /// The first `n` focus handles, created as needed. They share the Account page's tab
    /// position, so tabbing follows paint order down the column.
    fn handles(&mut self, n: usize, cx: &mut Context<Self>) -> Vec<FocusHandle> {
        while self.focus.len() < n {
            self.focus.push(
                cx.focus_handle()
                    .tab_index(account_page::TAB_INDEX)
                    .tab_stop(true),
            );
        }
        self.focus[..n].to_vec()
    }

    /// A row bound to `setting`.
    fn bool_row(
        &self,
        id: &'static str,
        label: &'static str,
        setting: &'static BoolSetting,
        focus: &FocusHandle,
        accent: apollo_theme::Accent,
        cx: &Context<Self>,
    ) -> SettingsRow {
        let on = self.settings.read(cx).get(setting);
        let model = self.settings.clone();
        SettingsRow::switch(id, focus, label, on, accent).on_toggle(move |value, _, cx| {
            model.update(cx, |m, cx| m.set(setting, value, cx));
        })
    }
}

impl Render for SettingsPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let h = self.handles(8, cx);

        let search_section = SettingsSection::new("Search")
            .row(self.bool_row(
                "setting-save-recent-searches",
                "Save recent searches",
                &SAVE_RECENT_SEARCHES,
                &h[5],
                accent,
                cx,
            ))
            .row(self.bool_row(
                "setting-hide-music-videos",
                "Hide music videos",
                &HIDE_MUSIC_VIDEOS,
                &h[6],
                accent,
                cx,
            ));

        let mut window_section = SettingsSection::new("Window")
            .row(self.bool_row(
                "setting-close-to-tray",
                "Close to tray",
                &CLOSE_TO_TRAY,
                &h[0],
                accent,
                cx,
            ))
            .row(self.bool_row(
                "setting-start-minimized",
                "Start minimized to tray",
                &START_MINIMIZED_TO_TRAY,
                &h[1],
                accent,
                cx,
            ));
        if autostart::AVAILABLE {
            let weak = cx.weak_entity();
            window_section = window_section.row(
                SettingsRow::switch(
                    "setting-launch-at-signin",
                    &h[2],
                    "Launch at sign-in",
                    self.launch_at_signin,
                    accent,
                )
                .on_toggle(move |value, _, cx| {
                    autostart::set_enabled(value);
                    // Show what the registry says, which differs if the write was refused.
                    let _ = weak.update(cx, |page, cx| {
                        page.launch_at_signin = autostart::is_enabled();
                        cx.notify();
                    });
                }),
            );
        }

        window_section = window_section.row(self.bool_row(
            "setting-mini-player-on-top",
            "Keep mini player on top",
            &MINI_PLAYER_ON_TOP,
            &h[7],
            accent,
            cx,
        ));

        let discord_section = SettingsSection::new("Discord").row(self.bool_row(
            "setting-discord-presence",
            "Show what I'm playing",
            &DISCORD_PRESENCE,
            &h[3],
            accent,
            cx,
        ));

        let keyboard_section = SettingsSection::new("Keyboard").row(self.bool_row(
            "setting-global-hotkeys",
            "Global hotkeys",
            &GLOBAL_HOTKEYS,
            &h[4],
            accent,
            cx,
        ));

        div()
            .id("settings-page")
            .flex()
            .flex_col()
            .gap(px(spacing::S8))
            .w_full()
            .max_w(px(MAX_WIDTH))
            .mt(px(spacing::S6))
            .pb(px(spacing::S8))
            .child(self.account_page.clone())
            .child(self.playback_page.clone())
            .child(search_section)
            .child(window_section)
            .child(discord_section)
            .child(keyboard_section)
    }
}
