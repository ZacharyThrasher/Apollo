//! The small actions a track row offers on hover (like, add to playlist), shared by the track
//! lists, Search and the queue panel so they cannot drift apart.
//!
//! Each is a 32px icon with the hover tint and a tooltip. Rows are not tab stops (a list has
//! one keyboard selection), so these take no focus; the lists bind keys for the same actions.
//! The heart is the filled Fluent heart in text.primary when liked and the outline in
//! text.secondary otherwise, the same on/off pair the shuffle and repeat buttons use.

use apollo_theme::tokens::{layout, radius};
use gpui::{
    div, prelude::*, px, svg, App, ClickEvent, ElementId, Hsla, IntoElement, KeyboardButton,
    MouseButton, SharedString, Window,
};

use crate::assets::Icon;
use crate::style::color;
use crate::ui::tooltip::Tooltip;
use crate::ui::transport::on_off;

/// Width of one action; a row reserves this much per action so nothing shifts when they appear.
pub const ACTION_SIZE: f32 = layout::MIN_HIT_TARGET;

pub fn heart_icon(liked: bool) -> Icon {
    if liked {
        Icon::HeartFilled
    } else {
        Icon::HeartRegular
    }
}

pub fn heart_label(liked: bool) -> &'static str {
    if liked {
        "Unlike"
    } else {
        "Like"
    }
}

/// One row action.
pub fn row_action(
    id: impl Into<ElementId>,
    icon: Icon,
    label: impl Into<SharedString>,
    icon_color: Hsla,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let label: SharedString = label.into();
    let tip = label.clone();
    div()
        .id(id)
        .size(px(ACTION_SIZE))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .hover(|s| s.bg(color::state_hover()))
        .active(|s| s.bg(color::state_pressed()))
        .role(gpui::Role::Button)
        .aria_label(label)
        // The row's own click (play) must not fire as well.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |event: &ClickEvent, window, cx| {
            if let ClickEvent::Keyboard(k) = event {
                if k.button == KeyboardButton::Space {
                    return;
                }
            }
            cx.stop_propagation();
            on_click(window, cx)
        })
        .tooltip(move |_, cx| Tooltip::view(tip.clone(), cx))
        .child(
            svg()
                .path(icon.path())
                .size(px(layout::ICON_SIZE))
                .flex_none()
                .text_color(icon_color),
        )
}

/// The heart of a row: filled when liked, outline otherwise.
pub fn heart_action(
    id: impl Into<ElementId>,
    liked: bool,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    row_action(
        id,
        heart_icon(liked),
        heart_label(liked),
        on_off(liked),
        on_click,
    )
}

/// The add-to-playlist action of a row.
pub fn playlist_action(
    id: impl Into<ElementId>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    row_action(
        id,
        Icon::PlaylistAdd,
        "Add to playlist",
        color::text_secondary(),
        on_click,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heart_is_filled_exactly_when_liked() {
        assert_eq!(heart_icon(true), Icon::HeartFilled);
        assert_eq!(heart_icon(false), Icon::HeartRegular);
        assert_eq!(heart_label(true), "Unlike");
        assert_eq!(heart_label(false), "Like");
    }
}
