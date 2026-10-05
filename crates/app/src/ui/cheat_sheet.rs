//! The shortcut cheat sheet, opened with `?`: every key the keymap has, grouped by what it does
//! (Play, Collect, Navigate, Window), each as a label and its key caps. The rows come from
//! `shortcuts::sheet`, so a key bound anywhere shows up here without this file knowing it.
//!
//! The look is the dialogs': a bg.overlay panel, no scrim, a click outside closes it. Esc, `?`,
//! Enter and Space close it too, so the keys that would otherwise act behind it do not.

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle, Focusable,
    Global, IntoElement, KeyBinding, MouseButton, ParentElement, Render, Styled, Window,
};

use crate::shortcuts::{self, Group, Row};
use crate::style::{self, color};

actions!(apollo_cheat_sheet, [CloseSheet]);

/// Key context of the sheet; the plain keys stand down while it is up.
pub const KEY_CONTEXT: &str = "CheatSheet";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", CloseSheet, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-/", CloseSheet, Some(KEY_CONTEXT)),
        KeyBinding::new("?", CloseSheet, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", CloseSheet, Some(KEY_CONTEXT)),
        KeyBinding::new("space", CloseSheet, Some(KEY_CONTEXT)),
    ]);
}

pub const TITLE: &str = "Keyboard shortcuts";
const PANEL_WIDTH: f32 = 640.0;
const ROW_HEIGHT: f32 = 28.0;

struct Open {
    /// Where keyboard focus returns to when the sheet closes.
    return_focus: Option<FocusHandle>,
}

/// The shell's sheet, reachable from anywhere (the `?` key, a menu).
#[derive(Clone)]
pub struct CheatSheetHandle(pub Entity<CheatSheet>);

impl Global for CheatSheetHandle {}

/// Open the sheet, or close it when it is already open. Does nothing where there is no shell.
pub fn toggle(window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<CheatSheetHandle>().map(|h| h.0.clone()) else {
        return;
    };
    handle.update(cx, |sheet, cx| {
        if sheet.is_open() {
            sheet.close(window, cx);
        } else {
            sheet.open(window, cx);
        }
    });
}

pub struct CheatSheet {
    focus: FocusHandle,
    open: Option<Open>,
}

impl CheatSheet {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            open: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        self.open = Some(Open { return_focus });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else {
            return;
        };
        if let Some(handle) = open.return_focus {
            window.focus(&handle, cx);
        }
        cx.notify();
    }
}

impl Focusable for CheatSheet {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// One key cap: a small bg.raised pill.
fn cap(text: String, cx: &App) -> AnyElement {
    style::text_style(div(), typography::CAPTION, cx)
        .flex_none()
        .px(px(spacing::S2))
        .py(px(1.0))
        .rounded(px(radius::SMALL))
        .bg(color::bg_raised())
        .border_1()
        .border_color(color::border_subtle())
        .text_color(color::text_primary())
        .child(text)
        .into_any_element()
}

fn row(r: &Row, cx: &App) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .gap(px(spacing::S3))
        .h(px(ROW_HEIGHT))
        .child(
            style::text_style(div(), typography::BODY, cx)
                .min_w_0()
                .truncate()
                .text_color(color::text_primary())
                .child(r.label.clone()),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .gap(px(spacing::S1))
                .children(r.keys.iter().map(|k| cap(k.clone(), cx))),
        )
        .into_any_element()
}

fn section(group: Group, rows: &[Row], cx: &App) -> AnyElement {
    let (title, id) = (group.title(), group.title().to_lowercase());
    div()
        .debug_selector(move || format!("cheat-group-{id}"))
        .flex()
        .flex_col()
        .gap(px(spacing::S1))
        .child(
            style::text_style(div(), typography::CAPTION, cx)
                .text_color(color::text_secondary())
                .child(title),
        )
        .children(rows.iter().map(|r| row(r, cx)))
        .into_any_element()
}

impl Render for CheatSheet {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.open.is_none() {
            return div().into_any_element();
        }
        // Groups alternate between two columns, so each stays short.
        let sections = shortcuts::sheet(cx);
        let (mut left, mut right) = (Vec::new(), Vec::new());
        let (mut left_rows, mut right_rows) = (0usize, 0usize);
        for (group, rows) in &sections {
            let el = section(*group, rows, cx);
            if left_rows <= right_rows {
                left_rows += rows.len() + 1;
                left.push(el);
            } else {
                right_rows += rows.len() + 1;
                right.push(el);
            }
        }
        let column = |children: Vec<AnyElement>| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(spacing::S4))
                .children(children)
        };

        let panel = div()
            .id("cheat-sheet")
            .debug_selector(|| "cheat-sheet".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Dialog)
            .aria_label(TITLE)
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .p(px(spacing::S4))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the panel are not clicks on the page behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|s, _: &CloseSheet, window, cx| s.close(window, cx)))
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child(TITLE),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(spacing::S8))
                    .child(column(left))
                    .child(column(right)),
            );

        // A transparent layer under the panel catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("cheat-sheet-layer")
            .debug_selector(|| "cheat-sheet-layer".into())
            .absolute()
            .top(px(layout::TITLE_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(spacing::S4))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|s, _, window, cx| s.close(window, cx)),
            )
            .child(panel)
            .into_any_element()
    }
}
