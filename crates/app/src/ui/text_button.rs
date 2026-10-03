//! Text button: the quiet "Retry" / "Clear" action under a short message. body-strong in
//! text.primary, underlined on hover, keyboard-only accent focus ring (DIRECTION.md, Shell
//! decisions 7).

use apollo_theme::tokens::{radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, App, ClickEvent, ElementId, FocusHandle, KeyboardButton, SharedString,
    Window,
};

use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, Handler};

#[derive(IntoElement)]
pub struct TextButton {
    id: ElementId,
    focus: FocusHandle,
    label: SharedString,
    accent: Accent,
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
            accent,
            on_click: None,
        }
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TextButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let ring = keyboard_focused(&self.focus, window);
        let accent = color::accent(self.accent);
        let handler = self.on_click;
        style::text_style(div(), typography::BODY_STRONG, cx)
            .id(self.id)
            .relative()
            .flex_none()
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .track_focus(&self.focus.tab_stop(true))
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .text_color(color::text_primary())
            .hover(|s| s.underline())
            .when_some(handler, |d, handler| {
                d.on_click(move |event: &ClickEvent, window, cx| {
                    // Space is the global play/pause key.
                    if let ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(window, cx);
                })
            })
            .child(self.label)
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)))
    }
}
