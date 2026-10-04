//! Text button: the player bar's "Retry" pattern as a reusable control. No fill and no accent;
//! it underlines on hover, shows the keyboard-only accent focus ring, and can carry a spinner
//! while its action runs (the one place spinners are allowed).
//!
//! [`TextButton::primary`] is the page's single main action: a neutral filled button
//! (bg.overlay, text.primary, no accent) whose fill brightens on hover. At most one per view.
//!
//! The artist page's button row adds two more boxes: [`TextButton::accent_fill`] (the play
//! button's colors) and [`TextButton::outlined`] (a toggle: `on` tints it as selected).

use std::time::{Duration, Instant};

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, percentage, prelude::*, px, svg, Animation, AnimationExt, ElementId, FocusHandle,
    KeyboardButton, MouseButton, SharedString, Transformation, Window,
};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, Handler};

const SPIN_PERIOD: Duration = Duration::from_millis(900);

#[derive(Default)]
struct PrimaryState {
    fade: HoverFade,
    pressed: bool,
}

#[derive(IntoElement)]
pub struct TextButton {
    id: ElementId,
    focus: FocusHandle,
    label: SharedString,
    /// Body-strong in text.primary (the action the page is about) instead of body in
    /// text.secondary.
    strong: bool,
    primary: bool,
    /// Filled with the accent, label in accent.fg: a play button.
    accent_fill: Option<(gpui::Hsla, gpui::Hsla)>,
    /// A border and no fill; `Some(true)` is the toggled-on state (selected tint).
    outline: Option<bool>,
    /// Keeps a label that changes ("Subscribe", "Subscribed") from moving its neighbours.
    min_width: Option<f32>,
    busy: bool,
    enabled: bool,
    ring: gpui::Hsla,
    on_click: Option<Handler>,
    accent: Accent,
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
            primary: false,
            accent_fill: None,
            outline: None,
            min_width: None,
            busy: false,
            enabled: true,
            ring: color::accent(accent),
            on_click: None,
            accent,
        }
    }

    pub fn strong(mut self) -> Self {
        self.strong = true;
        self
    }

    /// The page's single main action: neutral filled button, label in text.primary.
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self.strong = true;
        self
    }

    /// Filled with the accent (the play button's colors), label in text.accent_fg.
    pub fn accent_fill(mut self) -> Self {
        self.accent_fill = Some((color::accent(self.accent), color::accent_fg(self.accent)));
        self.strong = true;
        self
    }

    /// A bordered toggle with no fill; `on` tints it as selected.
    pub fn outlined(mut self, on: bool) -> Self {
        self.outline = Some(on);
        self.strong = true;
        self
    }

    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = Some(width);
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
        let (text_style, rest) = if let Some((_, fg)) = self.accent_fill {
            (typography::BODY_STRONG, fg)
        } else if self.strong {
            (typography::BODY_STRONG, color::text_primary())
        } else {
            (typography::BODY, color::text_secondary())
        };
        let text_color = if self.enabled {
            rest
        } else if self.accent_fill.is_some() {
            // Text on a disabled accent fill would vanish: dim the whole button instead.
            rest.opacity(0.5)
        } else {
            color::text_disabled()
        };
        let handler = self.on_click.filter(|_| live);
        let spinner_color = self
            .accent_fill
            .map_or_else(color::text_secondary, |(_, fg)| fg);
        let spinner = self.busy.then(|| {
            svg()
                .path(Icon::Spinner.path())
                .size(px(layout::ICON_SIZE))
                .flex_none()
                .text_color(spinner_color)
                .with_animation("spin", Animation::new(SPIN_PERIOD).repeat(), |el, t| {
                    el.with_transformation(Transformation::rotate(percentage(t)))
                })
        });
        let primary = self.primary;
        let (accent_fill, outline, min_width) = (self.accent_fill, self.outline, self.min_width);
        // Any button with a box shares the primary's hover tint, press tint and padding.
        let boxed = primary || accent_fill.is_some() || outline.is_some();
        let state = boxed
            .then(|| window.use_keyed_state(self.id.clone(), cx, |_, _| PrimaryState::default()));
        let (level, moving, pressed) = state.as_ref().map_or((0.0, false, false), |st| {
            let st = st.read(cx);
            let (level, moving) = st.fade.level(Instant::now());
            (level, moving, st.pressed)
        });
        if moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        // Fill brightens by the hover tint (and the press tint, instantly).
        let tint = if live && pressed {
            color::state_pressed()
        } else if live {
            color::state_hover().opacity(level)
        } else {
            color::state_hover().opacity(0.0)
        };
        let hover_state = state.clone();
        let down_state = state.clone();
        let up_state = state.clone();
        style::text_style(div(), text_style, cx)
            .id(self.id)
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(if boxed { spacing::S4 } else { spacing::S2 }))
            .rounded(px(radius::SMALL))
            .whitespace_nowrap()
            .text_color(text_color)
            .when_some(min_width, |d, w| d.min_w(px(w)))
            .when(primary, |d| d.bg(color::bg_overlay()))
            .when_some(accent_fill, |d, (fill, _)| {
                d.bg(if self.enabled { fill } else { fill.opacity(0.5) })
            })
            .when_some(outline, |d, on| {
                d.border_1()
                    .border_color(color::border_strong())
                    .when(on, |d| d.bg(color::state_selected()))
            })
            .track_focus(&self.focus.clone().tab_stop(live))
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .when(live, |d| {
                d.cursor_pointer()
                    .when(!boxed, |d| {
                        d.hover(|s| s.underline().text_color(color::text_primary()))
                    })
                    .when_some(hover_state, |d, st| {
                        d.on_hover(move |hovered, _, cx| {
                            st.update(cx, |s, cx| {
                                s.fade.set(*hovered, Instant::now(), animations);
                                cx.notify();
                            });
                        })
                    })
                    .when_some(down_state, |d, st| {
                        d.on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            st.update(cx, |s, cx| {
                                s.pressed = true;
                                cx.notify();
                            });
                        })
                    })
                    .when_some(up_state, |d, st| {
                        d.on_mouse_up(MouseButton::Left, move |_, _, cx| {
                            st.update(cx, |s, cx| {
                                s.pressed = false;
                                cx.notify();
                            });
                        })
                    })
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
            .when(boxed, |d| {
                d.child(
                    div()
                        .absolute()
                        .size_full()
                        .rounded(px(radius::SMALL))
                        .bg(tint),
                )
            })
            .children(spinner)
            .child(self.label)
            .when(ring, |d| d.child(focus_ring(self.ring, radius::SMALL)))
    }
}
