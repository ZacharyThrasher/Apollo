//! The short error a failed library write leaves above the player bar: a warning icon and what
//! happened in six words or fewer, on bg.overlay (so text.secondary, never text.tertiary). It
//! goes away by itself after a few seconds, or when clicked. No action: the heart or the
//! playlist has already been put back, and doing it again is the retry.

use std::time::Duration;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{div, prelude::*, px, Animation, AnimationExt, AnyElement, App, IntoElement};

use crate::assets::Icon;
use crate::library::writes::Notice;
use crate::style::{self, color};

/// How long it takes to fade in and to fade out. Under reduced motion it does neither.
pub const FADE: Duration = Duration::from_millis(150);

/// Where the toast is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fade {
    /// Fading in, then fully shown.
    In,
    /// Fading out; it is removed once [`FADE`] has passed.
    Out,
}

/// `on_dismiss` runs when the toast is clicked. `animate` is false under reduced motion: the
/// toast then appears and goes at once.
pub fn toast(
    notice: &Notice,
    fade: Fade,
    animate: bool,
    on_dismiss: impl Fn(&mut App) + 'static,
    cx: &App,
) -> impl IntoElement {
    let id = notice.id;
    let pill = style::text_style(div(), typography::BODY, cx)
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
        .child(notice.text.clone());
    let pill: AnyElement = if animate {
        let leaving = fade == Fade::Out;
        pill.with_animation(
            ("toast-fade", id * 2 + u64::from(leaving)),
            Animation::new(FADE),
            move |el, t| el.opacity(if leaving { 1.0 - t } else { t }),
        )
        .into_any_element()
    } else {
        pill.into_any_element()
    };
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom(px(layout::PLAYER_BAR_HEIGHT + spacing::S4))
        .flex()
        .flex_row()
        .justify_center()
        .child(pill)
}
