//! Labelled switch: a compact on/off control for a setting (the queue panel's Autoplay). A
//! 28x16 pill whose knob sits right when on, with the label beside it in text.secondary. On is
//! the accent, off is a quiet grey pill. It tints on hover like the icon buttons and shows the
//! keyboard-only accent focus ring; Enter flips it (Space stays the global play/pause key).

use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, App, ClickEvent, ElementId, FocusHandle, KeyboardButton, SharedString,
    Window,
};

use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, Handler};

const TRACK_W: f32 = 28.0;
const TRACK_H: f32 = 16.0;
const KNOB: f32 = 12.0;
/// Gap between the track's edge and the knob.
const KNOB_INSET: f32 = (TRACK_H - KNOB) / 2.0;

#[derive(Default)]
struct SwitchState {
    fade: HoverFade,
}

/// The 28x16 pill alone: accent track and dark knob on, quiet grey track and knob off. Shared
/// with the Settings rows so every switch in the app looks the same.
pub fn pill(on: bool, enabled: bool, accent: Accent) -> impl IntoElement {
    let track_color = if !enabled {
        color::state_hover()
    } else if on {
        color::accent(accent)
    } else {
        color::border_strong()
    };
    let knob_color = if !enabled {
        color::text_disabled()
    } else if on {
        color::accent_fg(accent)
    } else {
        color::text_secondary()
    };
    let knob_left = if on {
        TRACK_W - KNOB - KNOB_INSET
    } else {
        KNOB_INSET
    };
    div()
        .relative()
        .flex_none()
        .w(px(TRACK_W))
        .h(px(TRACK_H))
        .rounded_full()
        .bg(track_color)
        .child(
            div()
                .absolute()
                .top(px(KNOB_INSET))
                .left(px(knob_left))
                .size(px(KNOB))
                .rounded_full()
                .bg(knob_color),
        )
}

#[derive(IntoElement)]
pub struct LabeledSwitch {
    id: ElementId,
    focus: FocusHandle,
    label: SharedString,
    /// The label is only the accessible name: the caller draws its own text elsewhere (a settings
    /// row with its label on the left).
    label_hidden: bool,
    on: bool,
    enabled: bool,
    accent: Accent,
    on_toggle: Option<Handler>,
}

impl LabeledSwitch {
    pub fn new(
        id: impl Into<ElementId>,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        on: bool,
        accent: Accent,
    ) -> Self {
        Self {
            id: id.into(),
            focus: focus.clone(),
            label: label.into(),
            label_hidden: false,
            on,
            enabled: true,
            accent,
            on_toggle: None,
        }
    }

    /// Draws the pill alone; the label stays as its accessible name.
    pub fn label_hidden(mut self) -> Self {
        self.label_hidden = true;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Runs when the switch is clicked or activated from the keyboard; the caller flips the
    /// setting it stands for.
    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for LabeledSwitch {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| SwitchState::default());
        let (level, moving) = state.read(cx).fade.level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        let enabled = self.enabled;
        let ring = enabled && keyboard_focused(&self.focus, window);
        let accent = color::accent(self.accent);

        let label_color = if enabled {
            color::text_secondary()
        } else {
            color::text_disabled()
        };
        let hover_state = state.clone();
        let handler = self.on_toggle.filter(|_| enabled);
        div()
            .id(self.id.clone())
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .track_focus(&self.focus.clone().tab_stop(enabled))
            .role(gpui::Role::Switch)
            .aria_label(self.label.clone())
            .aria_toggled(if self.on {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .bg(color::state_hover().opacity(if enabled { level } else { 0.0 }))
            .when(enabled, |d| {
                d.cursor_pointer().on_hover(move |hovered, _, cx| {
                    hover_state.update(cx, |s, cx| {
                        s.fade.set(*hovered, Instant::now(), animations);
                        cx.notify();
                    });
                })
            })
            .when_some(handler, |d, handler| {
                d.on_click(move |event: &ClickEvent, window, cx| {
                    if let ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(window, cx);
                })
            })
            .child(pill(self.on, enabled, self.accent))
            .when(!self.label_hidden, |d| {
                d.child(
                    style::text_style(div(), typography::BODY, cx)
                        .text_color(label_color)
                        .child(self.label.clone()),
                )
            })
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)))
    }
}
