//! The blocked artists dialog, opened from Settings > Playback > Blocked artists > Manage: a
//! plain list of the artists whose songs radio, autoplay and Home's personal shelves skip, each
//! with an Unblock text button. The look and keys are the playlist dialogs': bg.overlay panel,
//! no scrim, a click outside or Esc closes, Tab stays inside.
//!
//! Unblocking takes the row away at once and sends nothing anywhere: the list is local.

use apollo_domain::blocklist::BlockedArtist;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, SharedString, Styled, Window,
};

use crate::blocked_artists::BlockModel;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::text_button::TextButton;

actions!(
    apollo_blocked_dialog,
    [BlockedClose, BlockedNext, BlockedPrevious]
);

/// Key context of the dialog.
pub const KEY_CONTEXT: &str = "BlockedArtistsDialog";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", BlockedClose, Some(KEY_CONTEXT)),
        // Focus stays in the dialog while it is open.
        KeyBinding::new("tab", BlockedNext, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", BlockedPrevious, Some(KEY_CONTEXT)),
    ]);
}

pub const PANEL_WIDTH: f32 = 360.0;
/// One artist's row.
pub const ROW_HEIGHT: f32 = layout::MIN_HIT_TARGET;
/// The list scrolls past this many rows.
const MAX_ROWS: f32 = 6.0;
pub const TITLE: &str = "Blocked artists";
pub const EMPTY: &str = "No blocked artists";

struct Open {
    /// Where keyboard focus returns to when the dialog closes.
    return_focus: Option<FocusHandle>,
}

/// The shell's dialog, reachable from Settings.
#[derive(Clone)]
pub struct BlockedDialogHandle(pub Entity<BlockedDialog>);

impl Global for BlockedDialogHandle {}

/// Open the dialog. Does nothing where there is no shell.
pub fn open(window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<BlockedDialogHandle>().map(|h| h.0.clone()) else {
        return;
    };
    handle.update(cx, |dialog, cx| dialog.open(window, cx));
}

pub struct BlockedDialog {
    player: Entity<PlayerModel>,
    blocked: Entity<BlockModel>,
    focus: FocusHandle,
    /// One Unblock button per row, grown as the list does.
    unblock_focus: Vec<FocusHandle>,
    done_focus: FocusHandle,
    scroll: gpui::ScrollHandle,
    open: Option<Open>,
}

impl BlockedDialog {
    pub fn new(
        player: Entity<PlayerModel>,
        blocked: Entity<BlockModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&blocked, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            blocked,
            focus: cx.focus_handle(),
            unblock_focus: Vec::new(),
            done_focus: cx.focus_handle().tab_stop(true),
            scroll: gpui::ScrollHandle::new(),
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
        // Done takes focus, so a stray Enter closes the dialog instead of unblocking someone.
        window.focus(&self.done_focus, cx);
        cx.notify();
    }

    /// Close without touching focus: the account changed under the dialog.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
        }
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.take() else {
            return;
        };
        match open.return_focus {
            Some(handle) => window.focus(&handle, cx),
            None => window.focus(&self.focus, cx),
        }
        cx.notify();
    }

    /// Let `artist` back in, and keep focus inside the dialog.
    pub fn unblock(&mut self, artist: &BlockedArtist, window: &mut Window, cx: &mut Context<Self>) {
        self.blocked.update(cx, |m, cx| {
            m.unblock(artist, cx);
        });
        window.focus(&self.done_focus, cx);
    }

    /// The artists shown, newest block first (tests).
    pub fn rows(&self, cx: &App) -> Vec<BlockedArtist> {
        self.blocked.read(cx).list().entries().to_vec()
    }

    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.blocked.read(cx).len().min(self.unblock_focus.len());
        let mut stops: Vec<FocusHandle> = self.unblock_focus[..rows].to_vec();
        stops.push(self.done_focus.clone());
        let at = stops.iter().position(|h| h.is_focused(window));
        let next = match at {
            Some(at) => (at as isize + delta).rem_euclid(stops.len() as isize) as usize,
            None => stops.len() - 1,
        };
        window.focus(&stops[next], cx);
    }
}

impl Focusable for BlockedDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for BlockedDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.open.is_none() {
            return div().into_any_element();
        }
        let (accent, _) = self.player.read(cx).accent_now();
        let entries = self.blocked.read(cx).list().entries().to_vec();
        while self.unblock_focus.len() < entries.len() {
            self.unblock_focus.push(cx.focus_handle().tab_stop(true));
        }
        let entity = cx.entity();

        let list = if entries.is_empty() {
            style::text_style(div(), typography::BODY, cx)
                .debug_selector(|| "blocked-dialog-empty".into())
                .h(px(ROW_HEIGHT))
                .flex()
                .items_center()
                .text_color(color::text_secondary())
                .child(EMPTY)
                .into_any_element()
        } else {
            let rows: Vec<_> = entries
                .iter()
                .enumerate()
                .map(|(i, artist)| {
                    let entity = entity.clone();
                    let target = artist.clone();
                    div()
                        .debug_selector(move || format!("blocked-row-{i}"))
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
                                .child(SharedString::from(artist.name.clone())),
                        )
                        .child(
                            div().debug_selector(move || format!("blocked-unblock-{i}")).child(
                                TextButton::new(
                                    ("blocked-unblock", i),
                                    &self.unblock_focus[i],
                                    "Unblock",
                                    accent,
                                )
                                .on_click(move |window, cx| {
                                    entity.update(cx, |d, cx| d.unblock(&target, window, cx))
                                }),
                            ),
                        )
                })
                .collect();
            div()
                .id("blocked-dialog-list")
                .flex()
                .flex_col()
                .max_h(px(ROW_HEIGHT * MAX_ROWS))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children(rows)
                .into_any_element()
        };

        let panel = div()
            .id("blocked-dialog")
            .debug_selector(|| "blocked-dialog".into())
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
            .on_action(cx.listener(|d, _: &BlockedClose, window, cx| d.close(window, cx)))
            .on_action(cx.listener(|d, _: &BlockedNext, window, cx| d.move_focus(1, window, cx)))
            .on_action(
                cx.listener(|d, _: &BlockedPrevious, window, cx| d.move_focus(-1, window, cx)),
            )
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child(TITLE),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .justify_end()
                    .child(
                        div().debug_selector(|| "blocked-done".into()).child(
                            TextButton::new("blocked-done", &self.done_focus, "Done", accent)
                                .primary()
                                .on_click({
                                    let entity = entity.clone();
                                    move |window, cx| entity.update(cx, |d, cx| d.close(window, cx))
                                }),
                        ),
                    ),
            );

        // A transparent layer under the panel catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("blocked-dialog-layer")
            .debug_selector(|| "blocked-dialog-layer".into())
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
                cx.listener(|d, _, window, cx| d.close(window, cx)),
            )
            .child(panel)
            .into_any_element()
    }
}
