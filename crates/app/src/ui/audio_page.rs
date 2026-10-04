//! Settings > Audio: the output device, and the equalizer with its editor.
//!
//! "Output device" is a select row: the device that is playing and a chevron, and a popover
//! under it lists "System default" and the devices. "Equalizer" is a switch (off until turned
//! on) with an Edit button that opens the editor under the card; the button reads Done while it
//! is open. The playback speed lives in Now Playing, not here.

use apollo_domain::audio::{resolve_device, Equalizer};
use apollo_theme::tokens::spacing;
use gpui::{
    div, prelude::*, px, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, Styled,
    Window,
};

use crate::audio::{save_audio_device, save_equalizer};
use crate::player::PlayerModel;
use crate::settings::SettingsModel;
use crate::ui::account_page::TAB_INDEX;
use crate::ui::choice_menu::{self, Choice};
use crate::ui::equalizer_panel::EqualizerPanel;
use crate::ui::settings_row::{anchor_key, SettingsRow, SettingsSection};
use crate::ui::text_button::TextButton;

/// Row ids. The device row's popover hangs from `anchor_key(ROW_DEVICE)`.
pub const ROW_DEVICE: &str = "setting-output-device";
pub const ROW_EQUALIZER: &str = "setting-equalizer";

/// How wide the device popover is: room for a typical "Speakers (Realtek(R) Audio)".
const DEVICE_MENU_WIDTH: f32 = 320.0;

pub struct AudioPage {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    panel: Entity<EqualizerPanel>,
    /// The editor is open under the card.
    editing: bool,
    focus_device: FocusHandle,
    focus_equalizer: FocusHandle,
    focus_edit: FocusHandle,
}

impl AudioPage {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let panel = cx.new(|cx| EqualizerPanel::new(player.clone(), settings.clone(), cx));
        let stop = |cx: &mut Context<Self>| cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true);
        Self {
            focus_device: stop(cx),
            focus_equalizer: stop(cx),
            focus_edit: stop(cx),
            player,
            settings,
            panel,
            editing: false,
        }
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn set_editing(&mut self, editing: bool, cx: &mut Context<Self>) {
        if self.editing != editing {
            self.editing = editing;
            cx.notify();
        }
    }

    /// Opens the device popover: "System default", then the devices, with a check on the one in
    /// force. A device that is chosen but unplugged is not in force: the default plays.
    fn open_devices(&self, window: &mut Window, cx: &mut gpui::App) {
        let (chosen, devices) = {
            let p = self.player.read(cx);
            (
                p.audio_device().map(str::to_owned),
                p.audio_devices().to_vec(),
            )
        };
        let in_force = resolve_device(chosen.as_deref(), &devices).map(str::to_owned);
        let mut choices = vec![Choice::new("System default", in_force.is_none())];
        choices.extend(
            devices
                .iter()
                .map(|d| Choice::new(d.name.clone(), in_force.as_deref() == Some(d.id.as_str()))),
        );
        let (player, settings) = (self.player.clone(), self.settings.clone());
        choice_menu::open(
            &anchor_key(ROW_DEVICE),
            DEVICE_MENU_WIDTH,
            choices,
            move |index, _, cx| {
                let device = index
                    .checked_sub(1)
                    .and_then(|i| devices.get(i))
                    .map(|d| d.id.clone());
                player.update(cx, |p, cx| p.set_audio_device(device.clone(), cx));
                save_audio_device(&settings, device.as_deref(), cx);
            },
            window,
            cx,
        );
    }
}

impl Render for AudioPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, eq, accent) = {
            let p = self.player.read(cx);
            (p.audio_device_label(), p.equalizer(), p.view().accent)
        };
        let page = cx.weak_entity();
        let edit = {
            let page = page.clone();
            TextButton::new(
                "setting-equalizer-edit",
                &self.focus_edit,
                if self.editing { "Done" } else { "Edit" },
                accent,
            )
            .on_click(move |_, cx| {
                let _ = page.update(cx, |p, cx| p.set_editing(!p.editing, cx));
            })
        };
        let (player, settings) = (self.player.clone(), self.settings.clone());

        let section = SettingsSection::new("Audio")
            .row(
                SettingsRow::select(
                    ROW_DEVICE,
                    &self.focus_device,
                    "Output device",
                    label,
                    accent,
                )
                .on_open(move |window, cx| {
                    let _ = page.update(cx, |p, cx| p.open_devices(window, cx));
                }),
            )
            .row(
                SettingsRow::switch(
                    ROW_EQUALIZER,
                    &self.focus_equalizer,
                    "Equalizer",
                    eq.enabled,
                    accent,
                )
                .action(
                    div()
                        .debug_selector(|| "setting-equalizer-edit".into())
                        .child(edit),
                )
                .on_toggle(move |value, _, cx| {
                    let eq = Equalizer {
                        enabled: value,
                        ..player.read(cx).equalizer()
                    };
                    player.update(cx, |p, cx| p.set_equalizer(eq, cx));
                    save_equalizer(&settings, eq, cx);
                }),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .w_full()
            .child(section)
            .children(self.editing.then(|| self.panel.clone()))
    }
}
