//! Text button for page actions (Play, Shuffle, Retry): 32px tall, 120 ms hover tint,
//! keyboard-only accent focus ring. `Primary` is the accent-filled call to action; `Secondary`
//! is a quiet tinted button; `Plain` is bare text that underlines on hover (like the player
//! bar's Retry).

use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, svg, Animation, AnimationExt, App, ClickEvent, ElementId, FocusHandle,
    Hsla, KeyboardButton, MouseButton, SharedString, Transformation, Window,
};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, Handler};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Plain,
}

#[derive(Default)]
struct State {
    fade: HoverFade,
    pressed: bool,
}

#[derive(IntoElement)]
pub struct TextButton {
    id: ElementId,
    focus: FocusHandle,
    label: SharedString,
    icon: Option<Icon>,
    style: ButtonStyle,
    accent: Accent,
    enabled: bool,
    busy: bool,
    on_click: Option<Handler>,
}

impl TextButton {
    pub fn new(
        id: impl Into<ElementId>,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        style: ButtonStyle,
        accent: Accent,
    ) -> Self {
        Self {
            id: id.into(),
            focus: focus.clone(),
            label: label.into(),
            icon: None,
            style,
            accent,
            enabled: true,
            busy: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Shows a spinner in place of the icon (spinners live only inside buttons) and ignores
    /// clicks until it is cleared.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for TextButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| State::default());
        let (level, moving, pressed) = {
            let s = state.read(cx);
            let (level, moving) = s.fade.level(Instant::now());
            (level, moving, s.pressed)
        };
        if moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        let active = self.enabled && !self.busy;
        let ring = self.enabled && keyboard_focused(&self.focus, window);

        // Resting fill, hover bump and press tint per style. Primary text/icon sit on the accent
        // (accent.fg); the others use text.primary.
        let (rest, fg): (Option<Hsla>, Hsla) = match self.style {
            ButtonStyle::Primary => (
                Some(color::accent(self.accent)),
                color::accent_fg(self.accent),
            ),
            ButtonStyle::Secondary => (Some(color::state_hover()), color::text_primary()),
            ButtonStyle::Plain => (None, color::text_primary()),
        };
        let fg = if self.enabled {
            fg
        } else {
            color::text_disabled()
        };
        let overlay = match self.style {
            // Hover on the accent fill darkens it a touch; the others brighten.
            ButtonStyle::Primary => color::bg_base(),
            _ => color::state_hover(),
        };
        let tint = if active && pressed {
            color::state_pressed()
        } else if active && self.style != ButtonStyle::Plain {
            overlay.opacity(level * 0.5)
        } else {
            overlay.opacity(0.0)
        };
        let fill = if self.enabled {
            rest
        } else {
            rest.map(|_| color::state_hover())
        };

        let glyph = if self.busy {
            Some(
                svg()
                    .path(Icon::Spinner.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(fg)
                    .with_animation(
                        "spin",
                        Animation::new(std::time::Duration::from_millis(900)).repeat(),
                        |el, t| el.with_transformation(Transformation::rotate(gpui::percentage(t))),
                    )
                    .into_any_element(),
            )
        } else {
            self.icon.map(|icon| {
                svg()
                    .path(icon.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(fg)
                    .into_any_element()
            })
        };

        let handler = self.on_click.filter(|_| active);
        let (hover_state, down_state, up_state) = (state.clone(), state.clone(), state);
        let plain = self.style == ButtonStyle::Plain;
        style::text_style(div(), typography::BODY_STRONG, cx)
            .id(self.id)
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(if plain { spacing::S2 } else { spacing::S4 }))
            .rounded(px(radius::SMALL))
            .when_some(fill, |d, c| d.bg(c))
            .text_color(fg)
            .track_focus(&self.focus.clone().tab_stop(self.enabled))
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .when(active, |d| {
                d.cursor_pointer()
                    .on_hover(move |hovered, _, cx| {
                        hover_state.update(cx, |s, cx| {
                            s.fade.set(*hovered, Instant::now(), animations);
                            cx.notify();
                        });
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        down_state.update(cx, |s, cx| {
                            s.pressed = true;
                            cx.notify();
                        });
                    })
                    .on_mouse_up(MouseButton::Left, move |_, _, cx| {
                        up_state.update(cx, |s, cx| {
                            s.pressed = false;
                            cx.notify();
                        });
                    })
                    .when(plain, |d| d.hover(|s| s.underline()))
            })
            .when_some(handler, |d, handler| {
                d.on_click(move |event: &ClickEvent, window, cx| {
                    // Space is the global play/pause key, so a focused button answers to Enter
                    // only.
                    if let ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(window, cx);
                })
            })
            .child(
                div()
                    .absolute()
                    .size_full()
                    .rounded(px(radius::SMALL))
                    .bg(tint),
            )
            .children(glyph)
            .child(self.label)
            .when(ring, |d| {
                d.child(focus_ring(color::accent(self.accent), radius::SMALL))
            })
    }
}
