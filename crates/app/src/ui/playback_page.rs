//! Settings > Playback: the playback switches. Normalize volume and Gapless playback. The section is a
//! title over a raised card of 48px rows, label on the left and the switch on the right, with no
//! helper text.

use apollo_theme::tokens::{radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, IntoElement, ParentElement, Render,
    SharedString, Styled, Window,
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

/// One 48px row: the label on the left and its switch on the right. `id` names the row and
/// `<id>` the switch for tests (`setting-<id>` and `<id>`).
fn switch_row(
    id: &'static str,
    label: &'static str,
    focus: &FocusHandle,
    on: bool,
    accent: Accent,
    divider: bool,
    on_toggle: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let row_id = SharedString::from(format!("setting-{id}"));
    let selector = row_id.clone();
    div()
        .id(row_id)
        .debug_selector(move || selector.to_string())
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(ROW_HEIGHT))
        .px(px(spacing::S4))
        .when(divider, |row| {
            row.border_t_1().border_color(color::border_subtle())
        })
        .child(
            style::text_style(div(), typography::BODY, cx)
                .text_color(color::text_primary())
                .child(label),
        )
        .child(
            div().debug_selector(move || id.into()).child(
                LabeledSwitch::new(id, focus, label, on, accent)
                    .label_hidden()
                    .on_toggle(on_toggle),
            ),
        )
}

impl Render for PlaybackPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (normalize, gapless, accent) = {
            let p = self.player.read(cx);
            (p.normalize(), p.gapless(), p.view().accent)
        };
        let (for_normalize, for_gapless) = (self.player.clone(), self.player.clone());

        let normalize_row = switch_row(
            "normalize-volume",
            "Normalize volume",
            &self.focus_normalize,
            normalize,
            accent,
            false,
            move |_, cx| {
                for_normalize.update(cx, |m, cx| {
                    let on = !m.normalize();
                    m.set_normalize(on, cx)
                });
            },
            cx,
        );
        let gapless_row = switch_row(
            "gapless-playback",
            "Gapless playback",
            &self.focus_gapless,
            gapless,
            accent,
            true,
            move |_, cx| {
                for_gapless.update(cx, |m, cx| {
                    let on = !m.gapless();
                    m.set_gapless(on, cx)
                });
            },
            cx,
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
                    .child(normalize_row)
                    .child(gapless_row),
            )
    }
}
