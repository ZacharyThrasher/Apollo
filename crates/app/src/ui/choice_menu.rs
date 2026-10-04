//! A pick-one popover: a short list of choices with a check on the current one, opened under the
//! control that asked for it (the Output device row, the speed button). It is the track menu's
//! surface and rules: bg.overlay, 8px radius, border.subtle, no scrim, 32px rows, a 16px slot at
//! the left for the check, 4px inside padding. It lines its right edge up with the control,
//! flips above it when there is no room below and stays clear of the title bar. Up, Down, Home,
//! End and Enter work it; Esc or a click outside closes it.
//!
//! One entity is hosted by the shell and reached through [`open`], so a view only says where the
//! menu goes (a key recorded with `ui::anchor`) and what the choices are.

use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, point, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::track_menu::{menu_height, menu_position, Placed, ITEM_HEIGHT};

actions!(
    apollo_choice_menu,
    [
        ChoiceUp,
        ChoiceDown,
        ChoiceFirst,
        ChoiceLast,
        ChoiceChoose,
        ChoiceClose,
        ChoiceStay
    ]
);

/// Key context of the menu.
pub const KEY_CONTEXT: &str = "ChoiceMenu";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", ChoiceUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", ChoiceDown, Some(KEY_CONTEXT)),
        KeyBinding::new("home", ChoiceFirst, Some(KEY_CONTEXT)),
        KeyBinding::new("end", ChoiceLast, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", ChoiceChoose, Some(KEY_CONTEXT)),
        KeyBinding::new("space", ChoiceChoose, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", ChoiceClose, Some(KEY_CONTEXT)),
        // Focus stays in the menu while it is open.
        KeyBinding::new("tab", ChoiceStay, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", ChoiceStay, Some(KEY_CONTEXT)),
    ]);
}

/// Space around the rows, as in the track menu.
const PAD: f32 = spacing::S1;
const ICON: f32 = 16.0;

/// One choice: what it says and whether it is the one in force.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: SharedString,
    pub selected: bool,
}

impl Choice {
    pub fn new(label: impl Into<SharedString>, selected: bool) -> Self {
        Self {
            label: label.into(),
            selected,
        }
    }
}

/// Called with the index of the choice picked, after the menu has closed.
pub type OnChoose = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// The shell's menu, reachable from any view.
#[derive(Clone)]
pub struct ChoiceHandle(pub Entity<ChoiceMenu>);

impl Global for ChoiceHandle {}

/// Open a menu of `choices`, `width` px wide, under the element that recorded its bounds under
/// `anchor_key`. Does nothing where there is no shell (a view on its own).
pub fn open(
    anchor_key: &str,
    width: f32,
    choices: Vec<Choice>,
    on_choose: impl Fn(usize, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(handle) = cx.try_global::<ChoiceHandle>().map(|h| h.0.clone()) else {
        return;
    };
    let on_choose: OnChoose = Rc::new(on_choose);
    handle.update(cx, |menu, cx| {
        menu.open(anchor_key, width, choices, on_choose, window, cx)
    });
}

struct Open {
    choices: Vec<Choice>,
    width: f32,
    placed: Placed,
    cursor: usize,
    on_choose: OnChoose,
    /// Where keyboard focus returns to when the menu closes.
    return_focus: Option<FocusHandle>,
}

pub struct ChoiceMenu {
    focus: FocusHandle,
    open: Option<Open>,
}

impl ChoiceMenu {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            open: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn cursor(&self) -> Option<usize> {
        self.open.as_ref().map(|o| o.cursor)
    }

    pub fn labels(&self) -> Vec<SharedString> {
        self.open
            .as_ref()
            .map(|o| o.choices.iter().map(|c| c.label.clone()).collect())
            .unwrap_or_default()
    }

    pub fn open(
        &mut self,
        anchor_key: &str,
        width: f32,
        choices: Vec<Choice>,
        on_choose: OnChoose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if choices.is_empty() {
            return;
        }
        let placed = match anchor::bounds_of(anchor_key, cx) {
            Some(bounds) => Placed::Below(bounds),
            None => {
                let size = window.viewport_size();
                Placed::Point(point(size.width / 2.0, size.height / 2.0))
            }
        };
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        // The cursor starts on the choice in force.
        let cursor = choices.iter().position(|c| c.selected).unwrap_or(0);
        self.open = Some(Open {
            choices,
            width,
            placed,
            cursor,
            on_choose,
            return_focus,
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.take_closed(window, cx);
    }

    fn take_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Open> {
        let open = self.open.take()?;
        match &open.return_focus {
            Some(handle) => window.focus(handle, cx),
            None => window.focus(&self.focus, cx),
        }
        cx.notify();
        Some(open)
    }

    fn move_cursor(&mut self, to: isize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let len = open.choices.len() as isize;
        open.cursor = to.clamp(0, len - 1) as usize;
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(at) = self.cursor() {
            self.move_cursor(at as isize + delta, cx);
        }
    }

    /// Pick the choice at `index`: the menu closes first, then the choice is reported.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.as_ref().is_none_or(|o| index >= o.choices.len()) {
            return;
        }
        let Some(open) = self.take_closed(window, cx) else {
            return;
        };
        (open.on_choose)(index, window, cx);
    }
}

impl Focusable for ChoiceMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ChoiceMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let size = gpui::size(px(open.width), px(menu_height(open.choices.len(), 1)));
        let at = menu_position(open.placed, size, window.viewport_size());
        let cursor = open.cursor;
        let rows: Vec<gpui::AnyElement> = open
            .choices
            .iter()
            .enumerate()
            .map(|(ix, choice)| row(ix, choice, ix == cursor, cx))
            .collect();

        let panel = div()
            .id("choice-menu")
            .debug_selector(|| "choice-menu".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Menu)
            .flex()
            .flex_col()
            .w(px(open.width))
            .p(px(PAD))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the menu are not clicks on the page behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|m, _: &ChoiceUp, _, cx| m.step(-1, cx)))
            .on_action(cx.listener(|m, _: &ChoiceDown, _, cx| m.step(1, cx)))
            .on_action(cx.listener(|m, _: &ChoiceFirst, _, cx| m.move_cursor(0, cx)))
            .on_action(cx.listener(|m, _: &ChoiceLast, _, cx| m.move_cursor(isize::MAX, cx)))
            .on_action(cx.listener(|m, _: &ChoiceChoose, window, cx| {
                if let Some(ix) = m.cursor() {
                    m.choose(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|m, _: &ChoiceClose, window, cx| m.close(window, cx)))
            .on_action(cx.listener(|_, _: &ChoiceStay, _, _| {}))
            .children(rows);

        // A transparent layer under the menu catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("choice-menu-layer")
            .debug_selector(|| "choice-menu-layer".into())
            .absolute()
            .top(px(layout::TITLE_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|m, _, window, cx| m.close(window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|m, _, window, cx| m.close(window, cx)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(at.left))
                    .top(px(at.top - layout::TITLE_BAR_HEIGHT))
                    .child(panel),
            )
            .into_any_element()
    }
}

fn row(ix: usize, choice: &Choice, cursor: bool, cx: &mut Context<ChoiceMenu>) -> gpui::AnyElement {
    let label = choice.label.clone();
    style::text_style(div(), typography::BODY, cx)
        .id(("choice-menu-item", ix))
        .debug_selector(move || format!("choice-menu-item-{ix}"))
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ITEM_HEIGHT))
        .px(px(spacing::S3))
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .role(gpui::Role::MenuItemRadio)
        .aria_label(label.clone())
        .text_color(color::text_primary())
        .bg(if cursor {
            color::state_hover()
        } else {
            gpui::transparent_black()
        })
        .on_hover(cx.listener(move |m, hovered: &bool, _, cx| {
            if *hovered {
                m.move_cursor(ix as isize, cx);
            }
        }))
        .on_click(cx.listener(move |m, _: &gpui::ClickEvent, window, cx| m.choose(ix, window, cx)))
        .child(div().flex_none().size(px(ICON)).when(choice.selected, |d| {
            d.child(
                gpui::svg()
                    .path(Icon::Checkmark.path())
                    .size(px(ICON))
                    .text_color(color::text_secondary()),
            )
        }))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_remembers_what_it_says_and_whether_it_is_in_force() {
        let c = Choice::new("1.5\u{d7}", true);
        assert_eq!(c.label.as_ref(), "1.5\u{d7}");
        assert!(c.selected);
    }
}
