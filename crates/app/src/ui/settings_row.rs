//! Building blocks of the Settings screen: a titled section holding a card of 48px rows, each a
//! label on the left and a switch on the right. No helper text. A row is one focus stop and one
//! click target; Enter toggles it (Space stays the global play/pause key).

use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, AnyElement, App, ElementId, FocusHandle, KeyboardButton, MouseButton,
    SharedString, Window,
};

use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::keyboard_focused;

/// Height of one row.
pub const ROW_HEIGHT: f32 = 48.0;
const TRACK_WIDTH: f32 = 40.0;
const TRACK_HEIGHT: f32 = 20.0;
const KNOB_OFF: f32 = 12.0;
const KNOB_ON: f32 = 14.0;

/// A titled card of rows. Title in the section style, card on bg.raised with an 8px radius,
/// 1px border.subtle between rows.
#[derive(IntoElement)]
pub struct SettingsSection {
    title: SharedString,
    rows: Vec<AnyElement>,
}

impl SettingsSection {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            rows: Vec::new(),
        }
    }

    pub fn row(mut self, row: impl IntoElement) -> Self {
        self.rows.push(row.into_any_element());
        self
    }
}

impl RenderOnce for SettingsSection {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut card = div()
            .flex()
            .flex_col()
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_raised());
        for (i, row) in self.rows.into_iter().enumerate() {
            card = card.child(
                div()
                    .when(i > 0, |d| {
                        d.border_t_1().border_color(color::border_subtle())
                    })
                    .child(row),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .w_full()
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child(self.title),
            )
            .child(card)
    }
}

/// What a row remembers between frames: the pointer tint and the knob's slide, both 120 ms.
struct RowState {
    hover: HoverFade,
    knob: HoverFade,
}

/// One setting: label left, switch right.
#[derive(IntoElement)]
pub struct SettingsRow {
    id: &'static str,
    focus: FocusHandle,
    label: SharedString,
    on: bool,
    ring: gpui::Hsla,
    accent: Accent,
    on_toggle: Option<Box<dyn Fn(bool, &mut Window, &mut App) + 'static>>,
}

impl SettingsRow {
    pub fn switch(
        id: &'static str,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        on: bool,
        accent: Accent,
    ) -> Self {
        Self {
            id,
            focus: focus.clone(),
            label: label.into(),
            on,
            ring: color::accent(accent),
            accent,
            on_toggle: None,
        }
    }

    /// Called with the value the owner asked for (the opposite of what is shown).
    pub fn on_toggle(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SettingsRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let on = self.on;
        let animations = !cx.reduce_motion();
        let state = window.use_keyed_state(ElementId::from(self.id), cx, |_, _| {
            let mut knob = HoverFade::default();
            knob.set(on, Instant::now(), false);
            RowState {
                hover: HoverFade::default(),
                knob,
            }
        });
        let now = Instant::now();
        let (tint, knob, moving) = {
            state.update(cx, |s, _| s.knob.set(on, now, animations));
            let s = state.read(cx);
            let (tint, hover_moving) = s.hover.level(now);
            let (knob, knob_moving) = s.knob.level(now);
            (tint, knob, hover_moving || knob_moving)
        };
        if moving {
            window.request_animation_frame();
        }
        let ring = keyboard_focused(&self.focus, window);
        let handler = self.on_toggle;
        let hover_state = state.clone();

        let accent = color::accent(self.accent);
        // Off: an outlined track and a small, quiet knob. On: the accent fills the track and the
        // knob grows and darkens to the accent's foreground.
        let knob_size = KNOB_OFF + (KNOB_ON - KNOB_OFF) * knob;
        let travel = TRACK_WIDTH - KNOB_ON - 2.0 * ((TRACK_HEIGHT - KNOB_ON) / 2.0);
        let inset = (TRACK_HEIGHT - knob_size) / 2.0;
        let left = inset + travel * knob;
        let off_fill = color::text_secondary();
        let on_fill = color::accent_fg(self.accent);
        let track = div()
            .relative()
            .flex_none()
            .w(px(TRACK_WIDTH))
            .h(px(TRACK_HEIGHT))
            .rounded(px(TRACK_HEIGHT / 2.0))
            .border_1()
            .border_color(mix(color::border_strong(), accent, knob))
            .bg(accent.opacity(knob))
            .child(
                div()
                    .absolute()
                    // The 1px border sits inside the box, so offsets are from the padding edge.
                    .top(px(inset - 1.0))
                    .left(px(left - 1.0))
                    .size(px(knob_size))
                    .rounded_full()
                    .bg(mix(off_fill, on_fill, knob)),
            );

        div()
            .id(self.id)
            .debug_selector(|| self.id.to_owned())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(spacing::S4))
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .track_focus(&self.focus.clone().tab_stop(true))
            .role(gpui::Role::Switch)
            .aria_label(self.label.clone())
            .aria_toggled(if on {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .cursor_pointer()
            .bg(color::state_hover().opacity(tint))
            .on_hover(move |hovered, _, cx| {
                hover_state.update(cx, |s, cx| {
                    s.hover.set(*hovered, Instant::now(), animations);
                    cx.notify();
                });
            })
            .when_some(handler, |d, handler| {
                d.on_click(move |event, window, cx| {
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(!on, window, cx);
                })
            })
            .on_mouse_down(MouseButton::Left, |_, _, _| {})
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_primary())
                    .child(self.label),
            )
            .child(track)
            // Inside the row: a ring drawn outside would spill past the card's edge.
            .when(ring, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border(px(layout::FOCUS_RING_WIDTH))
                        .border_color(self.ring),
                )
            })
    }
}

/// `from` blended toward `to` by `t` (0-1), in RGB; good enough for a 120 ms tween.
fn mix(from: gpui::Hsla, to: gpui::Hsla, t: f32) -> gpui::Hsla {
    let (a, b) = (gpui::Rgba::from(from), gpui::Rgba::from(to));
    gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_ends_are_the_inputs() {
        let a = gpui::rgb(0x102030);
        let b = gpui::rgb(0xf0e0d0);
        let start = gpui::Rgba::from(mix(a.into(), b.into(), 0.0));
        assert!((start.r - a.r).abs() < 1e-3 && (start.b - a.b).abs() < 1e-3);
        let end = gpui::Rgba::from(mix(a.into(), b.into(), 1.0));
        assert!((end.r - b.r).abs() < 1e-3 && (end.b - b.b).abs() < 1e-3);
    }

    #[test]
    fn layout_constants_leave_the_knob_inside_the_track() {
        assert!(KNOB_ON < TRACK_HEIGHT && KNOB_OFF <= KNOB_ON);
    }
}
