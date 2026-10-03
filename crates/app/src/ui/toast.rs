//! The short error a failed library write leaves above the player bar: a warning icon and what
//! happened in six words or fewer, on bg.overlay (so text.secondary, never text.tertiary). It
//! goes away by itself after a few seconds, or when clicked. No action: the heart or the
//! playlist has already been put back, and doing it again is the retry.

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{div, prelude::*, px, App, IntoElement};

use crate::assets::Icon;
use crate::library::writes::Notice;
use crate::style::{self, color};

/// `on_dismiss` runs when the toast is clicked.
pub fn toast(
    notice: &Notice,
    on_dismiss: impl Fn(&mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(layout::PLAYER_BAR_HEIGHT + spacing::S4))
        .flex()
        .flex_row()
        .justify_center()
        .child(
            style::text_style(div(), typography::BODY, cx)
                .id(("toast", notice.id))
                .debug_selector(|| "toast".into())
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing::S2))
                .px(px(spacing::S3))
                .py(px(spacing::S2))
                .rounded(px(radius::LARGE))
                .bg(color::bg_overlay())
                .border_1()
                .border_color(color::border_subtle())
                .text_color(color::text_secondary())
                .role(gpui::Role::Alert)
                .aria_label(notice.text.clone())
                .cursor_pointer()
                .on_click(move |_, _, cx| on_dismiss(cx))
                .child(
                    gpui::svg()
                        .path(Icon::Warning.path())
                        .size(px(layout::ICON_SIZE))
                        .flex_none()
                        .text_color(color::warning()),
                )
                .child(notice.text.clone()),
        )
}
