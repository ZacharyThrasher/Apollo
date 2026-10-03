//! Settings > Playback: the playback switches. Today only "Normalize volume". The section is a
//! title over a raised card of 48px rows, label on the left and the switch on the right, with no
//! helper text.

use apollo_theme::tokens::{radius, spacing, typography};
use gpui::{
    div, prelude::*, px, Context, Entity, FocusHandle, IntoElement, ParentElement, Render, Styled,
    Window,
};

use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::account_page::TAB_INDEX;
use crate::ui::switch::LabeledSwitch;

const MAX_WIDTH: f32 = 560.0;
const ROW_HEIGHT: f32 = spacing::S12;

pub struct PlaybackPage {
    player: Entity<PlayerModel>,
    focus_normalize: FocusHandle,
}

impl PlaybackPage {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            focus_normalize: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
        }
    }
}

impl Render for PlaybackPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (on, accent) = {
            let p = self.player.read(cx);
            (p.normalize(), p.view().accent)
        };
        let player = self.player.clone();

        let row = div()
            .id("setting-normalize-volume")
            .debug_selector(|| "setting-normalize-volume".into())
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_primary())
                    .child("Normalize volume"),
            )
            .child(
                div().debug_selector(|| "normalize-volume".into()).child(
                    LabeledSwitch::new(
                        "normalize-volume",
                        &self.focus_normalize,
                        "Normalize volume",
                        on,
                        accent,
                    )
                    .label_hidden()
                    .on_toggle(move |_, cx| {
                        player.update(cx, |m, cx| {
                            let on = !m.normalize();
                            m.set_normalize(on, cx)
                        });
                    }),
                ),
            );

        div()
            .id("playback-page")
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .w_full()
            .max_w(px(MAX_WIDTH))
            .pb(px(spacing::S6))
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child("Playback"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(radius::MEDIUM))
                    .bg(color::bg_raised())
                    .overflow_hidden()
                    .child(row),
            )
    }
}
