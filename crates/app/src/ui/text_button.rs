//! Text button: the player bar's "Retry" pattern as a reusable control. No fill and no accent;
//! it underlines on hover, shows the keyboard-only accent focus ring, and can carry a spinner
//! while its action runs (the one place spinners are allowed).

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, percentage, prelude::*, px, svg, Animation, AnimationExt, ElementId, FocusHandle,
    KeyboardButton, SharedString, Transformation, Window,
};
use std::time::Duration;

use crate::assets::Icon;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, Handler};

const SPIN_PERIOD: Duration = Duration::from_millis(900);

#[derive(IntoElement)]
pub struct TextButton {
    id: ElementId,
    focus: FocusHandle,
    label: SharedString,
    /// Body-strong in text.primary (the action the page is about) instead of body in
    /// text.secondary.
    strong: bool,
    busy: bool,
    enabled: bool,
    ring: gpui::Hsla,
    on_click: Option<Handler>,
}

impl TextButton {
    pub fn new(
        id: impl Into<ElementId>,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        accent: Accent,
    ) -> Self {
        Self {
            id: id.into(),
            focus: focus.clone(),
            label: label.into(),
            strong: false,
            busy: false,
            enabled: true,
            ring: color::accent(accent),
            on_click: None,
        }
    }

    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }

    /// Shows the in-button spinner. A busy button does not react.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut gpui::App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TextButton {
    fn render(self, window: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let live = self.enabled && !self.busy;
        let ring = live && keyboard_focused(&self.focus, window);
        let (text_style, rest) = if self.strong {
            (typography::BODY_STRONG, color::text_primary())
        } else {
            (typography::BODY, color::text_secondary())
        };
        let text_color = if self.enabled {
            rest
        } else {
            color::text_disabled()
        };
        let handler = self.on_click.filter(|_| live);
        let spinner = self.busy.then(|| {
            svg()
                .path(Icon::Spinner.path())
                .size(px(layout::ICON_SIZE))
                .flex_none()
                .text_color(color::text_secondary())
                .with_animation("spin", Animation::new(SPIN_PERIOD).repeat(), |el, t| {
                    el.with_transformation(Transformation::rotate(percentage(t)))
                })
        });
        style::text_style(div(), text_style, cx)
            .id(self.id)
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .whitespace_nowrap()
            .text_color(text_color)
            .track_focus(&self.focus.clone().tab_stop(live))
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .when(live, |d| {
                d.cursor_pointer()
                    .hover(|s| s.underline().text_color(color::text_primary()))
            })
            .when_some(handler, |d, handler| {
                d.on_click(move |event, window, cx| {
                    // Space is the global play/pause key, so a focused button answers to Enter
                    // only.
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(window, cx);
                })
            })
            .children(spinner)
            .child(self.label)
            .when(ring, |d| d.child(focus_ring(self.ring, radius::SMALL)))
    }
}
