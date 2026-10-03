//! A card that opens a page: artwork over a title and a quiet second line. Shared by Search's
//! album, artist and playlist groups and by an artist page's album rows.
//!
//! The whole card is one control: a hover tint that fades over 120 ms, a keyboard-only accent
//! focus ring, and Enter or a click to open. Space is left to the global play/pause shortcut.
//! Enter is an action in the card's own key context: the lists the cards sit in bind Enter to
//! play the selected row, and the card's deeper context takes it first.

use std::rc::Rc;
use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, img, prelude::*, px, AnyElement, App, FocusHandle, ImageSource, KeyBinding,
    ObjectFit, SharedString, Window,
};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::tooltip::Tooltip;
use crate::url_thumbs::Thumb;

actions!(card, [OpenCard]);

/// Key context of a card.
pub const KEY_CONTEXT: &str = "Card";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", OpenCard, Some(KEY_CONTEXT))]);
}

/// The artwork square.
pub const ART: f32 = 144.0;
/// Space between the artwork and the card's edge (the hover tint and focus ring sit out here).
pub const INSET: f32 = spacing::S2;
/// The card's outer width: artwork plus its inset on both sides.
pub const WIDTH: f32 = ART + 2.0 * INSET;

/// Artwork or the no-art tile: bg.overlay with a quiet music note (Shell decisions 3). A cover
/// still downloading is the plain tile, with nothing to animate.
pub fn tile(thumb: &Thumb) -> AnyElement {
    shaped_tile(thumb, false)
}

/// [`tile`] as a circle (an artist) or a rounded square. The image carries the corners itself:
/// GPUI clips a parent's children to its rectangle, not to its rounded corners.
pub fn shaped_tile(thumb: &Thumb, circle: bool) -> AnyElement {
    match thumb {
        Thumb::Image(image) => img(ImageSource::Render(image.clone()))
            .size_full()
            .object_fit(ObjectFit::Cover)
            .map(|i| {
                if circle {
                    i.rounded_full()
                } else {
                    i.rounded(px(radius::MEDIUM))
                }
            })
            .into_any_element(),
        Thumb::Pending => div().into_any_element(),
        Thumb::Missing => gpui::svg()
            .path(Icon::MusicNote.path())
            .size(px(layout::ICON_SIZE))
            .flex_none()
            .text_color(color::text_tertiary())
            .into_any_element(),
    }
}

/// Section heading: `section` type, with room above unless it opens the page.
pub fn heading(title: impl Into<SharedString>, spaced: bool, cx: &App) -> AnyElement {
    style::text_style(div(), typography::SECTION, cx)
        .flex_none()
        .when(spaced, |d| d.mt(px(spacing::S6)))
        .mb(px(spacing::S2))
        .text_color(color::text_primary())
        .child(title.into())
        .into_any_element()
}

/// Cards wrap in rows; the negative margin lines the artwork (not the card's inset) up with
/// the heading above it.
pub fn grid() -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(spacing::S4))
        .ml(px(-INSET))
}

#[derive(IntoElement)]
pub struct Card {
    /// Unique on its page: names the hover state and the text's tooltips.
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    thumb: Thumb,
    /// An artist's artwork is a circle; albums and playlists are rounded squares.
    circle: bool,
    focus: FocusHandle,
    accent: Accent,
    on_open: Rc<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl Card {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        subtitle: Option<SharedString>,
        thumb: Thumb,
        focus: &FocusHandle,
        accent: Accent,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle,
            thumb,
            circle: false,
            focus: focus.clone(),
            accent,
            on_open: Rc::new(on_open),
        }
    }
}

impl Card {
    /// Draws the artwork as a circle (an artist).
    pub fn circular(mut self, circle: bool) -> Self {
        self.circle = circle;
        self
    }
}

impl RenderOnce for Card {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let ring_color = color::accent(self.accent);
        let label: SharedString = match &self.subtitle {
            Some(s) => format!("{}, {s}", self.title).into(),
            None => self.title.clone(),
        };
        let open = self.on_open;
        let open_key = open.clone();
        let title = self.title.clone();
        let id = self.id.clone();

        div()
            .debug_selector({
                let id = id.to_string();
                move || id.clone()
            })
            .id(id.clone())
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(spacing::S1))
            .w(px(WIDTH))
            .p(px(INSET))
            .rounded(px(radius::MEDIUM))
            .bg(color::state_hover().opacity(level))
            .cursor_pointer()
            .track_focus(&self.focus)
            .role(gpui::Role::Button)
            .aria_label(label)
            .on_hover(move |hovered, _, cx| {
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animations);
                    cx.notify();
                });
            })
            .key_context(KEY_CONTEXT)
            .on_action(move |_: &OpenCard, window, cx| open_key(window, cx))
            .on_click(move |event, window, cx| {
                // The keyboard opens a card through `OpenCard`; Space is the play/pause key.
                if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    open(window, cx);
                }
            })
            .child(
                div()
                    .size(px(ART))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| {
                        if self.circle {
                            d.rounded_full()
                        } else {
                            d.rounded(px(radius::MEDIUM))
                        }
                    })
                    .overflow_hidden()
                    .bg(color::bg_overlay())
                    .child(shaped_tile(&self.thumb, self.circle)),
            )
            .child(
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id((id.clone(), 0usize))
                    .truncate()
                    .text_color(color::text_primary())
                    .tooltip({
                        let full = title.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(title),
            )
            .children(self.subtitle.map(|text| {
                style::text_style(div(), typography::BODY, cx)
                    .id((id, 1usize))
                    .truncate()
                    .text_color(color::text_secondary())
                    .tooltip({
                        let full = text.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(text)
            }))
            .when(ring, move |d| {
                d.child(focus_ring(ring_color, radius::MEDIUM))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_is_its_artwork_plus_an_inset_on_each_side() {
        assert_eq!(WIDTH, 160.0);
    }
}
