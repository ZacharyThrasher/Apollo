//! Building blocks of the Settings screen: a titled section holding a card of 48px rows, each a
//! label on the left and the app's switch pill on the right. No helper text. A row is one focus stop and one
//! click target; Enter toggles it (Space stays the global play/pause key).

use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, AnyElement, App, ElementId, FocusHandle, KeyboardButton, SharedString,
    Window,
};

use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::keyboard_focused;
use crate::ui::switch;

/// Height of one row.
pub const ROW_HEIGHT: f32 = 48.0;

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

/// What a row remembers between frames: the pointer tint, 120 ms like the other controls.
struct RowState {
    hover: HoverFade,
}

/// Called with the value the owner asked for.
type Toggle = Box<dyn Fn(bool, &mut Window, &mut App) + 'static>;

/// One setting: label left, switch right.
#[derive(IntoElement)]
pub struct SettingsRow {
    id: &'static str,
    focus: FocusHandle,
    label: SharedString,
    on: bool,
    ring: gpui::Hsla,
    accent: Accent,
    on_toggle: Option<Toggle>,
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
        let state = window.use_keyed_state(ElementId::from(self.id), cx, |_, _| RowState {
            hover: HoverFade::default(),
        });
        let (tint, moving) = state.read(cx).hover.level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let ring = keyboard_focused(&self.focus, window);
        let handler = self.on_toggle;
        let hover_state = state.clone();

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
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_primary())
                    .child(self.label),
            )
            .child(
                div()
                    .debug_selector(|| format!("{}-switch", self.id))
                    .child(switch::pill(on, true, self.accent)),
            )
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
