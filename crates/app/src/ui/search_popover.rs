//! The popover under the Search field: the recent searches while the field is empty (each with
//! an x, and a "Clear" in the header), the service's completions while typing.
//!
//! Styled like the track menu: bg.overlay, 8px radius, border.subtle, 4px inside padding and
//! 32px rows with a 16px icon in text.secondary. The page owns the state (which rows, which one
//! the keyboard is on) and the key handling; this draws it and reports clicks. Rows are picked
//! on mouse-down so the field keeps its focus.

use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{div, prelude::*, px, svg, App, IntoElement, MouseButton, SharedString, Window};

use crate::assets::Icon;
use crate::style::{self, color};

/// Height of a row and of the recents header.
pub const ROW_HEIGHT: f32 = layout::MIN_HIT_TARGET;
const PAD: f32 = spacing::S1;
const ICON: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The field is empty: past searches, removable.
    Recents,
    /// The field has text: completions for it.
    Suggestions,
}

pub type RowAction = Rc<dyn Fn(usize, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SearchPopover {
    pub kind: Kind,
    pub rows: Vec<SharedString>,
    /// The row under the keyboard cursor or the pointer.
    pub active: Option<usize>,
    pub on_pick: RowAction,
    pub on_hover: RowAction,
    pub on_remove: RowAction,
    pub on_clear: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl RenderOnce for SearchPopover {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let recents = self.kind == Kind::Recents;
        let header = recents.then(|| {
            let on_clear = self.on_clear.clone();
            div()
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .justify_between()
                .h(px(ROW_HEIGHT))
                .px(px(spacing::S3))
                .child(
                    style::text_style(div(), typography::BODY_STRONG, cx)
                        .text_color(color::text_secondary())
                        .child("Recent searches"),
                )
                .child(
                    style::text_style(div(), typography::BODY, cx)
                        .id("search-recents-clear")
                        .debug_selector(|| "search-recents-clear".into())
                        .px(px(spacing::S2))
                        .h(px(spacing::S6))
                        .flex()
                        .items_center()
                        .rounded(px(radius::SMALL))
                        .cursor_pointer()
                        .text_color(color::text_secondary())
                        .hover(|s| s.text_color(color::text_primary()).bg(color::state_hover()))
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            cx.stop_propagation();
                            on_clear(window, cx)
                        })
                        .child("Clear"),
                )
        });

        let rows = self.rows.into_iter().enumerate().map(|(ix, text)| {
            let (pick, hover, remove) = (
                self.on_pick.clone(),
                self.on_hover.clone(),
                self.on_remove.clone(),
            );
            let active = self.active == Some(ix);
            let x = recents.then(|| {
                div()
                    .id(("search-recent-remove", ix))
                    .debug_selector(move || format!("search-recent-remove-{ix}"))
                    .size(px(ROW_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::SMALL))
                    .cursor_pointer()
                    .hover(|s| s.bg(color::state_hover()))
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        remove(ix, window, cx)
                    })
                    .child(
                        svg()
                            .path(Icon::Dismiss.path())
                            .size(px(ICON))
                            .flex_none()
                            .text_color(color::text_secondary()),
                    )
            });
            style::text_style(div(), typography::BODY, cx)
                .id(("search-suggestion", ix))
                .debug_selector(move || format!("search-suggestion-{ix}"))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(px(spacing::S3))
                .h(px(ROW_HEIGHT))
                .pl(px(spacing::S3))
                .when(!recents, |d| d.pr(px(spacing::S3)))
                .rounded(px(radius::SMALL))
                .cursor_pointer()
                .text_color(color::text_primary())
                .bg(if active {
                    color::state_hover()
                } else {
                    gpui::transparent_black()
                })
                .on_hover(move |hovered, window, cx| {
                    if *hovered {
                        hover(ix, window, cx)
                    }
                })
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    pick(ix, window, cx)
                })
                .child(
                    svg()
                        .path(Icon::SearchRegular.path())
                        .size(px(ICON))
                        .flex_none()
                        .text_color(color::text_secondary()),
                )
                .child(div().flex_1().min_w_0().truncate().child(text))
                .children(x)
        });

        div()
            .id("search-popover")
            .debug_selector(|| "search-popover".into())
            .flex()
            .flex_col()
            .w_full()
            .p(px(PAD))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Nothing behind the popover reacts to the pointer.
            .occlude()
            .children(header)
            .children(rows)
    }
}
