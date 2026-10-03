//! Icon button: 32px hit target, 120 ms hover tint, keyboard-only accent focus ring, tooltip.

use std::time::{Duration, Instant};

use apollo_theme::tokens::{layout, radius};
use apollo_theme::Accent;
use gpui::{
    div, percentage, prelude::*, px, svg, Animation, AnimationExt, App, ClickEvent, ElementId,
    FocusHandle, Hsla, KeyboardButton, MouseButton, SharedString, Transformation, Window,
};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::style::color;
use crate::ui::tooltip::Tooltip;

#[derive(Default)]
struct ButtonState {
    fade: HoverFade,
    pressed: bool,
}

/// One turn of the in-button spinner.
const SPIN_PERIOD: Duration = Duration::from_millis(900);

/// A click callback: the window and app context it runs with.
pub type Handler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    focus: FocusHandle,
    icon: Icon,
    label: SharedString,
    /// What assistive technology calls the button when that differs from the tooltip.
    name: Option<SharedString>,
    tooltip: bool,
    size: f32,
    icon_color: Hsla,
    fill: Option<Hsla>,
    circle: bool,
    ring: Hsla,
    enabled: bool,
    spin: bool,
    toggled: Option<bool>,
    on_click: Option<Handler>,
}

impl IconButton {
    pub fn new(
        id: impl Into<ElementId>,
        focus: &FocusHandle,
        icon: Icon,
        label: impl Into<SharedString>,
        accent: Accent,
    ) -> Self {
        Self {
            id: id.into(),
            focus: focus.clone(),
            icon,
            label: label.into(),
            name: None,
            tooltip: true,
            size: layout::MIN_HIT_TARGET,
            icon_color: color::text_secondary(),
            fill: None,
            circle: false,
            ring: color::accent(accent),
            enabled: true,
            spin: false,
            toggled: None,
            on_click: None,
        }
    }

    /// The accessible name, when the tooltip text is not the right one: a toggle already
    /// reports its state, so its name must not flip with it ("Like", not "Unlike"), and a
    /// button whose name another control on screen shares needs saying apart.
    pub fn accessible_name(mut self, name: impl Into<SharedString>) -> Self {
        self.name = Some(name.into());
        self
    }

    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    pub fn icon_color(mut self, c: Hsla) -> Self {
        self.icon_color = c;
        self
    }

    pub fn filled(mut self, bg: Hsla) -> Self {
        self.fill = Some(bg);
        self
    }

    pub fn circle(mut self) -> Self {
        self.circle = true;
        self
    }

    pub fn tooltip(mut self, on: bool) -> Self {
        self.tooltip = on;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Marks a toggle button (shuffle, queue) so assistive tech reads its state.
    pub fn toggled(mut self, on: bool) -> Self {
        self.toggled = Some(on);
        self
    }

    pub fn spinning(mut self, spin: bool) -> Self {
        self.spin = spin;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

/// The accent focus ring (2px, 2px offset) drawn around `radius_px`-rounded content. Only
/// meaningful for keyboard focus; callers decide when to show it.
pub fn focus_ring(ring: Hsla, radius_px: f32) -> impl IntoElement {
    let reach = layout::FOCUS_RING_OFFSET + layout::FOCUS_RING_WIDTH;
    div()
        .absolute()
        .top(px(-reach))
        .left(px(-reach))
        .right(px(-reach))
        .bottom(px(-reach))
        .rounded(px(radius_px + reach))
        .border(px(layout::FOCUS_RING_WIDTH))
        .border_color(ring)
}

/// True when `focus` holds keyboard focus (not just a mouse click).
pub fn keyboard_focused(focus: &FocusHandle, window: &Window) -> bool {
    focus.is_focused(window) && window.last_input_was_keyboard()
}

impl RenderOnce for IconButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ButtonState::default());
        let (level, moving, pressed) = {
            let s = state.read(cx);
            let (level, moving) = s.fade.level(Instant::now());
            (level, moving, s.pressed)
        };
        if moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        let enabled = self.enabled;
        let radius_px = if self.circle {
            self.size / 2.0
        } else {
            radius::SMALL
        };
        let ring = enabled && keyboard_focused(&self.focus, window);

        let tint = if enabled && pressed {
            color::state_pressed()
        } else if enabled {
            color::state_hover().opacity(level)
        } else {
            color::state_hover().opacity(0.0)
        };
        let icon_color = if enabled {
            self.icon_color
        } else {
            color::text_disabled()
        };

        let glyph = svg()
            .path(self.icon.path())
            .size(px(layout::ICON_SIZE))
            .flex_none()
            .text_color(icon_color);
        let glyph: gpui::AnyElement = if self.spin {
            glyph
                .with_animation("spin", Animation::new(SPIN_PERIOD).repeat(), |el, t| {
                    el.with_transformation(Transformation::rotate(percentage(t)))
                })
                .into_any_element()
        } else {
            glyph.into_any_element()
        };

        let handlers = self.on_click.filter(|_| enabled);
        let hover_state = state.clone();
        let down_state = state.clone();
        let up_state = state.clone();
        div()
            .id(self.id.clone())
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(px(self.size))
            .rounded(px(radius_px))
            .when_some(self.fill, |d, c| d.bg(c))
            .track_focus(&self.focus.clone().tab_stop(enabled))
            .aria_label(self.name.clone().unwrap_or_else(|| self.label.clone()))
            .role(gpui::Role::Button)
            .when_some(self.toggled, |d, on| {
                d.aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
            })
            .when(enabled, |d| {
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
            })
            .when_some(handlers, |d, handler| {
                d.on_click(move |event: &ClickEvent, window, cx| {
                    // Space is the global play/pause key, so a focused button answers to Enter
                    // only (keyboard Space would otherwise fire both).
                    if let ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(window, cx);
                })
            })
            .when(self.tooltip, {
                let label = self.label.clone();
                move |d| d.tooltip(move |_, cx| Tooltip::view(label.clone(), cx))
            })
            .child(div().absolute().size_full().rounded(px(radius_px)).bg(tint))
            .child(glyph)
            .when(ring, |d| d.child(focus_ring(self.ring, radius_px)))
    }
}
