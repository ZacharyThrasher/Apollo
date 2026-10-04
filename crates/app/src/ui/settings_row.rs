//! Building blocks of the Settings screen: a titled section holding a card of 48px rows, each a
//! label on the left and the app's switch pill on the right. No helper text. A row is one focus stop and one
//! click target; Enter toggles it (Space stays the global play/pause key).
//!
//! Four variants share the row: [`SettingsRow::select`] shows the current choice and a chevron
//! on the right and opens a popover when pressed, [`SettingsRow::action`] puts a control (a text
//! button) beside a switch or a select's choice (pressing the control does not flip the switch
//! or open the popover), [`SettingsRow::plain`] is a label with only such a control: no
//! switch, no row click, and no focus stop of its own, and [`SettingsRow::manage`] shows a short
//! fact (a count) and a control, with no switch and no chevron.

use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, AnyElement, App, ElementId, FocusHandle, KeyboardButton, MouseButton,
    SharedString, Window,
};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::anchor;
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

/// A row's control, wrapped so its own clicks stay its own: stopped here, the row never sees the
/// press and so does not flip (or open its popover).
fn control_slot(control: Option<AnyElement>) -> Option<impl IntoElement> {
    control.map(|control| {
        div()
            .flex_none()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(control)
    })
}

/// Where a select row's popover hangs from: the key its choice and chevron record their bounds
/// under (see `ui::anchor`).
pub fn anchor_key(id: &str) -> String {
    format!("{id}-anchor")
}

/// What a row remembers between frames: the pointer tint, 120 ms like the other controls.
struct RowState {
    hover: HoverFade,
}

/// Called with the value the owner asked for.
type Toggle = Box<dyn Fn(bool, &mut Window, &mut App) + 'static>;

/// The widest the current choice of a select row is shown before it is cut with an ellipsis.
const SELECT_VALUE_MAX_WIDTH: f32 = 320.0;

/// One setting: label left, switch right (or, for a select row, the current choice and a chevron).
#[derive(IntoElement)]
pub struct SettingsRow {
    id: &'static str,
    /// The row's own focus stop; `None` for a [`plain`](Self::plain) row, whose control is the
    /// only thing to reach.
    focus: Option<FocusHandle>,
    label: SharedString,
    on: bool,
    /// Set for a select row: the current choice, shown with a chevron instead of a switch.
    value: Option<SharedString>,
    /// A control shown before the switch.
    action: Option<AnyElement>,
    /// A manage row: this text (a count) in text.secondary and the control, no switch.
    fact: Option<SharedString>,
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
            focus: Some(focus.clone()),
            label: label.into(),
            on,
            value: None,
            action: None,
            fact: None,
            ring: color::accent(accent),
            accent,
            on_toggle: None,
        }
    }

    /// A label and a control ([`action`](Self::action)) and nothing else: the row itself is not
    /// pressable and not a tab stop.
    pub fn plain(id: &'static str, label: impl Into<SharedString>, accent: Accent) -> Self {
        Self {
            id,
            focus: None,
            label: label.into(),
            on: false,
            value: None,
            action: None,
            fact: None,
            ring: color::accent(accent),
            accent,
            on_toggle: None,
        }
    }

    /// A row that shows `value` (the current choice) and a chevron, and opens a popover when
    /// pressed: see [`SettingsRow::on_open`] and [`anchor_key`], the place the popover hangs
    /// from.
    pub fn select(
        id: &'static str,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
        accent: Accent,
    ) -> Self {
        let mut row = Self::switch(id, focus, label, false, accent);
        row.value = Some(value.into());
        row
    }

    /// A row with no switch: `fact` (a count, say) in text.secondary and the control given to
    /// [`SettingsRow::action`] on the right. Pressing the row itself runs
    /// [`SettingsRow::on_open`], like pressing the control.
    pub fn manage(
        id: &'static str,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        fact: impl Into<SharedString>,
        accent: Accent,
    ) -> Self {
        let mut row = Self::switch(id, focus, label, false, accent);
        row.fact = Some(fact.into());
        row
    }

    /// Called when a select row is pressed (a click, or Enter on the focused row).
    pub fn on_open(self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle(move |_, window, cx| handler(window, cx))
    }

    /// A control beside the switch, for a row that has more to do than flip. It takes its own
    /// clicks: pressing it does not flip the row.
    pub fn action(mut self, control: impl IntoElement) -> Self {
        self.action = Some(control.into_any_element());
        self
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
        let ring = self
            .focus
            .as_ref()
            .is_some_and(|focus| keyboard_focused(focus, window));
        let plain = self.focus.is_none();
        let handler = self.on_toggle;
        let fact = self.fact.clone();
        let is_manage = fact.is_some();
        let mut action = self.action;
        // A manage row keeps its control next to the fact.
        let beside_fact = if is_manage { action.take() } else { None };
        let (beside_switch, beside_choice) = if self.value.is_some() {
            (None, action)
        } else {
            (action, None)
        };
        let hover_state = state.clone();
        let id = self.id;
        let value = self.value.clone();
        let is_select = value.is_some() || is_manage;
        let accessible = match (&value, &fact) {
            (Some(value), _) => SharedString::from(format!("{}, {}", self.label, value)),
            (None, Some(fact)) => SharedString::from(format!("{}, {}", self.label, fact)),
            (None, None) => self.label.clone(),
        };

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
            .when_some(self.focus.clone(), |d, focus| {
                d.track_focus(&focus.tab_stop(true))
            })
            .role(if plain {
                gpui::Role::Group
            } else if is_select {
                gpui::Role::Button
            } else {
                gpui::Role::Switch
            })
            .aria_label(accessible)
            .when(!is_select && !plain, |d| {
                d.aria_toggled(if on {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
            })
            .when(!plain, |d| {
                d.cursor_pointer()
                    .bg(color::state_hover().opacity(tint))
                    .on_hover(move |hovered, _, cx| {
                        hover_state.update(cx, |s, cx| {
                            s.hover.set(*hovered, Instant::now(), animations);
                            cx.notify();
                        });
                    })
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
            .children(control_slot(beside_switch))
            .when_some(fact, |d, fact| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(spacing::S3))
                        .child(
                            style::text_style(div(), typography::BODY, cx)
                                .debug_selector(|| format!("{}-fact", id))
                                .text_color(color::text_secondary())
                                .child(fact),
                        )
                        .children(control_slot(beside_fact)),
                )
            })
            .when(!is_select && !plain, |d| {
                d.child(
                    div()
                        .debug_selector(|| format!("{}-switch", id))
                        .child(switch::pill(on, true, self.accent)),
                )
            })
            .when_some(value, |d, value| {
                // The popover hangs from the choice and the chevron, right edges lined up.
                let choice = anchor::anchored(
                    anchor_key(id),
                    div()
                        .debug_selector(|| format!("{}-value", id))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(spacing::S2))
                        .child(
                            style::text_style(div(), typography::BODY, cx)
                                .max_w(px(SELECT_VALUE_MAX_WIDTH))
                                .truncate()
                                .text_color(color::text_secondary())
                                .child(value),
                        )
                        .child(
                            gpui::svg()
                                .path(Icon::ChevronRight.path())
                                .size(px(layout::ICON_SIZE))
                                .flex_none()
                                .text_color(color::text_secondary()),
                        ),
                );
                match control_slot(beside_choice) {
                    // A select with a control keeps both on the right: the choice, then the control.
                    Some(control) => d.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(spacing::S4))
                            .child(choice)
                            .child(control),
                    ),
                    None => d.child(choice),
                }
            })
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
