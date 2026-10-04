//! The sleep timer popover: Off, 15, 30, 45 and 60 minutes, End of track. Opened from the moon
//! button in Now Playing; the same look as the track menu (bg.overlay, 8px radius, border.subtle,
//! no scrim, 32px rows, 220px wide, 4px padding) and the same keys: Up and Down move, Enter
//! chooses, Esc or a click outside closes. The row that is on carries a check mark. Choosing
//! closes the menu at once; there is no toast.
//!
//! One entity hosted by the shell and reached through [`open`].

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    div, point, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, MouseButton, ParentElement, Render, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::player::PlayerModel;
use crate::sleep_timer::SleepChoice;
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::track_menu::{
    menu_height, menu_position, MenuChoose, MenuClose, MenuDown, MenuFirst, MenuLast, MenuStay,
    MenuUp, Placed, ITEM_HEIGHT, KEY_CONTEXT, MENU_WIDTH,
};

const ICON: f32 = 16.0;
/// Space between the panel's edge and its rows, and above and below a separator.
const PAD: f32 = spacing::S1;
const SEPARATOR: f32 = 1.0;

/// The rows in groups (a separator sits between groups).
pub fn groups() -> [Vec<SleepChoice>; 3] {
    let m = SleepChoice::MENU;
    [vec![m[0]], m[1..5].to_vec(), vec![m[5]]]
}

/// The row the keyboard cursor starts on: the one that is on.
fn start_cursor(current: SleepChoice) -> usize {
    SleepChoice::MENU
        .iter()
        .position(|c| *c == current)
        .unwrap_or(0)
}

/// The shell's popover, reachable from the moon button.
#[derive(Clone)]
pub struct SleepMenuHandle(pub Entity<SleepMenu>);

impl Global for SleepMenuHandle {}

/// Open the popover under the element recorded as `anchor_key`. Does nothing where there is no
/// shell.
pub fn open(anchor_key: &str, window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<SleepMenuHandle>().map(|h| h.0.clone()) else {
        return;
    };
    handle.update(cx, |menu, cx| menu.open(anchor_key, window, cx));
}

struct Open {
    placed: Placed,
    cursor: usize,
    return_focus: Option<FocusHandle>,
}

pub struct SleepMenu {
    player: Entity<PlayerModel>,
    focus: FocusHandle,
    open: Option<Open>,
}

impl SleepMenu {
    pub fn new(player: Entity<PlayerModel>, cx: &mut Context<Self>) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
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

    pub fn open(&mut self, anchor_key: &str, window: &mut Window, cx: &mut Context<Self>) {
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
        let cursor = start_cursor(self.player.read(cx).sleep_choice());
        self.open = Some(Open {
            placed,
            cursor,
            return_focus,
        });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.take_closed(window, cx);
    }

    /// Close without touching focus (the window is going away, or the account changed).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
        }
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
        let len = SleepChoice::MENU.len() as isize;
        if let Some(open) = self.open.as_mut() {
            open.cursor = to.clamp(0, len - 1) as usize;
            cx.notify();
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(at) = self.cursor() {
            self.move_cursor(at as isize + delta, cx);
        }
    }

    /// Pick the row at `index` and close.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(choice) = SleepChoice::MENU.get(index).copied() else {
            return;
        };
        if self.take_closed(window, cx).is_none() {
            return;
        }
        self.player.update(cx, |p, cx| p.set_sleep(choice, cx));
    }
}

impl Focusable for SleepMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SleepMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let current = self.player.read(cx).sleep_choice();
        let cursor = open.cursor;
        let groups = groups();
        let rows = SleepChoice::MENU.len();
        let size = gpui::size(px(MENU_WIDTH), px(menu_height(rows, groups.len())));
        let at = menu_position(open.placed, size, window.viewport_size());

        let mut body: Vec<gpui::AnyElement> = Vec::new();
        let mut ix = 0usize;
        for (g, group) in groups.iter().enumerate() {
            if g > 0 {
                body.push(
                    div()
                        .flex_none()
                        .h(px(SEPARATOR))
                        .mx(px(PAD))
                        .my(px(PAD))
                        .bg(color::border_subtle())
                        .into_any_element(),
                );
            }
            for choice in group {
                body.push(menu_item(ix, *choice, *choice == current, ix == cursor, cx));
                ix += 1;
            }
        }

        let panel = div()
            .id("sleep-menu")
            .debug_selector(|| "sleep-menu".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Menu)
            .flex()
            .flex_col()
            .w(px(MENU_WIDTH))
            .p(px(PAD))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the menu are not clicks on the page behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|m, _: &MenuUp, _, cx| m.step(-1, cx)))
            .on_action(cx.listener(|m, _: &MenuDown, _, cx| m.step(1, cx)))
            .on_action(cx.listener(|m, _: &MenuFirst, _, cx| m.move_cursor(0, cx)))
            .on_action(cx.listener(|m, _: &MenuLast, _, cx| m.move_cursor(isize::MAX, cx)))
            .on_action(cx.listener(|m, _: &MenuChoose, window, cx| {
                if let Some(ix) = m.cursor() {
                    m.choose(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|m, _: &MenuClose, window, cx| m.close(window, cx)))
            .on_action(cx.listener(|_, _: &MenuStay, _, _| {}))
            .children(body);

        // A transparent layer under the menu catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("sleep-menu-layer")
            .debug_selector(|| "sleep-menu-layer".into())
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

fn menu_item(
    ix: usize,
    choice: SleepChoice,
    on: bool,
    cursor: bool,
    cx: &mut Context<SleepMenu>,
) -> gpui::AnyElement {
    let label: SharedString = choice.label().into();
    style::text_style(div(), typography::BODY, cx)
        .id(("sleep-menu-item", ix))
        .debug_selector(move || format!("sleep-menu-item-{ix}"))
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ITEM_HEIGHT))
        .px(px(spacing::S3))
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .role(gpui::Role::MenuItem)
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
        // The mark column is always there, so the labels do not move when the choice does.
        .child(div().flex_none().size(px(ICON)).when(on, |d| {
            d.child(
                gpui::svg()
                    .path(Icon::Checkmark.path())
                    .size(px(ICON))
                    .text_color(color::text_primary()),
            )
        }))
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_are_off_then_the_lengths_then_end_of_track() {
        let g = groups();
        assert_eq!(g[0], [SleepChoice::Off]);
        assert_eq!(
            g[1],
            [
                SleepChoice::Minutes(15),
                SleepChoice::Minutes(30),
                SleepChoice::Minutes(45),
                SleepChoice::Minutes(60)
            ]
        );
        assert_eq!(g[2], [SleepChoice::EndOfTrack]);
        let flat: Vec<_> = g.into_iter().flatten().collect();
        assert_eq!(flat, SleepChoice::MENU);
    }

    #[test]
    fn the_cursor_starts_on_the_row_that_is_on() {
        assert_eq!(start_cursor(SleepChoice::Off), 0);
        assert_eq!(start_cursor(SleepChoice::Minutes(45)), 3);
        assert_eq!(start_cursor(SleepChoice::EndOfTrack), 5);
        // A length that is not on the menu falls back to the first row.
        assert_eq!(start_cursor(SleepChoice::Minutes(7)), 0);
    }
}
