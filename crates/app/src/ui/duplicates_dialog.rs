//! The Remove duplicates dialog, opened from the More menu of a playlist page of your own: a
//! preview of the rows that would go (a song listed again after its first copy, matched by video
//! id; the first copy always stays), each with the place it has in the playlist. Cancel keeps
//! everything, "Remove N" takes them out. The look and keys are the other dialogs': bg.overlay
//! panel, no scrim, a click outside or Esc cancels, Tab stays inside.
//!
//! The list is read when the dialog opens and the removal names those very rows by their own
//! tokens, so the copy that was shown is the one that goes. A playlist with no repeats says so
//! and offers only Done. Removing sends nothing from here: the page's own edit queue takes the
//! rows out at once and writes them to the account one at a time, and a refused one puts the
//! playlist back as the service holds it (with the usual notice).

use apollo_domain::library::Collection;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, SharedString, Styled, Window,
};

use crate::library::model::{LibraryModel, Repeat};
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::text_button::TextButton;

actions!(
    apollo_duplicates_dialog,
    [DuplicatesCancel, DuplicatesNext, DuplicatesPrevious]
);

/// Key context of the dialog.
pub const KEY_CONTEXT: &str = "DuplicatesDialog";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DuplicatesCancel, Some(KEY_CONTEXT)),
        // Focus stays in the dialog while it is open.
        KeyBinding::new("tab", DuplicatesNext, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", DuplicatesPrevious, Some(KEY_CONTEXT)),
    ]);
}

pub const PANEL_WIDTH: f32 = 360.0;
/// One repeat's row: the title over the artist.
pub const ROW_HEIGHT: f32 = 48.0;
/// The list scrolls past this many rows.
const MAX_ROWS: f32 = 6.0;
pub const TITLE: &str = "Remove duplicates";
pub const NONE_FOUND: &str = "No duplicates";

struct Open {
    collection: Collection,
    /// The rows that would go, as they were when the dialog opened.
    repeats: Vec<Repeat>,
    /// Where keyboard focus returns to when the dialog closes.
    return_focus: Option<FocusHandle>,
}

/// The shell's dialog, reachable from the playlist menu.
#[derive(Clone)]
pub struct DuplicatesDialogHandle(pub Entity<DuplicatesDialog>);

impl Global for DuplicatesDialogHandle {}

/// Open the dialog for the playlist page on top. Does nothing where there is no shell.
pub fn open(collection: Collection, window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<DuplicatesDialogHandle>().map(|h| h.0.clone()) else {
        return;
    };
    handle.update(cx, |dialog, cx| dialog.open(collection, window, cx));
}

/// "3 songs", "1 song".
fn songs(n: usize) -> String {
    if n == 1 {
        "1 song".into()
    } else {
        format!("{n} songs")
    }
}

pub struct DuplicatesDialog {
    player: Entity<PlayerModel>,
    model: Entity<LibraryModel>,
    focus: FocusHandle,
    cancel_focus: FocusHandle,
    remove_focus: FocusHandle,
    scroll: gpui::ScrollHandle,
    open: Option<Open>,
}

impl DuplicatesDialog {
    pub fn new(
        player: Entity<PlayerModel>,
        model: Entity<LibraryModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            model,
            focus: cx.focus_handle(),
            cancel_focus: cx.focus_handle().tab_stop(true),
            remove_focus: cx.focus_handle().tab_stop(true),
            scroll: gpui::ScrollHandle::new(),
            open: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Whether `collection`'s page is the one open and can be arranged: the dialog has something
    /// to read.
    pub fn can_open(&self, collection: &Collection, cx: &App) -> bool {
        self.model
            .read(cx)
            .arrangeable()
            .is_some_and(|c| c.library_key() == collection.library_key())
    }

    /// Read the repeats of `collection`'s page and show them. Nothing opens for a page that is
    /// not on top (or cannot be edited): there is nothing to preview.
    pub fn open(&mut self, collection: Collection, window: &mut Window, cx: &mut Context<Self>) {
        let key = collection.library_key().to_owned();
        let (arrangeable, repeats) = {
            let model = self.model.read(cx);
            (
                model.arrangeable().is_some_and(|c| c.library_key() == key),
                model.repeats(&key),
            )
        };
        if !arrangeable {
            return;
        }
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        self.open = Some(Open {
            collection,
            repeats,
            return_focus,
        });
        // Cancel takes focus, so a stray Enter keeps every song.
        window.focus(&self.cancel_focus, cx);
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

    /// Take the listed rows out of the playlist and close.
    pub fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let (key, tokens): (String, Vec<String>) = (
            open.collection.library_key().to_owned(),
            open.repeats.iter().map(|r| r.occurrence.clone()).collect(),
        );
        self.model.update(cx, |m, cx| {
            m.remove_rows(&key, &tokens, cx);
        });
        self.close(window, cx);
    }

    /// The rows listed, top to bottom (tests).
    pub fn rows(&self) -> Vec<Repeat> {
        self.open
            .as_ref()
            .map(|o| o.repeats.clone())
            .unwrap_or_default()
    }

    fn move_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_remove = self.open.as_ref().is_some_and(|o| !o.repeats.is_empty());
        let next = if has_remove && self.cancel_focus.is_focused(window) {
            &self.remove_focus
        } else {
            &self.cancel_focus
        };
        window.focus(next, cx);
    }
}

impl Focusable for DuplicatesDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DuplicatesDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let (accent, _) = self.player.read(cx).accent_now();
        let repeats = open.repeats.clone();
        let entity = cx.entity();

        // The panel's own text is on bg.overlay: text.secondary is the lowest tier there.
        let lead = if repeats.is_empty() {
            NONE_FOUND.to_owned()
        } else {
            format!(
                "{} listed again. The first copy of each stays.",
                songs(repeats.len())
            )
        };
        let list = if repeats.is_empty() {
            div().into_any_element()
        } else {
            let rows: Vec<_> = repeats
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    let place: SharedString = format!("Row {}", r.number).into();
                    div()
                        .debug_selector(move || format!("duplicate-row-{i}"))
                        .flex()
                        .flex_row()
                        .flex_none()
                        .items_center()
                        .justify_between()
                        .gap(px(spacing::S3))
                        .h(px(ROW_HEIGHT))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .child(
                                    style::text_style(div(), typography::BODY, cx)
                                        .truncate()
                                        .text_color(color::text_primary())
                                        .child(SharedString::from(r.title.clone())),
                                )
                                .child(
                                    style::text_style(div(), typography::CAPTION, cx)
                                        .truncate()
                                        .text_color(color::text_secondary())
                                        .child(SharedString::from(r.artist.clone())),
                                ),
                        )
                        .child(
                            style::tabular(style::text_style(div(), typography::CAPTION, cx))
                                .flex_none()
                                .text_color(color::text_secondary())
                                .child(place),
                        )
                })
                .collect();
            div()
                .id("duplicates-dialog-list")
                .flex()
                .flex_col()
                .max_h(px(ROW_HEIGHT * MAX_ROWS))
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .children(rows)
                .into_any_element()
        };

        let buttons = if repeats.is_empty() {
            div().debug_selector(|| "duplicates-done".into()).child(
                TextButton::new("duplicates-done", &self.cancel_focus, "Done", accent)
                    .primary()
                    .on_click({
                        let entity = entity.clone();
                        move |window, cx| entity.update(cx, |d, cx| d.close(window, cx))
                    }),
            )
        } else {
            div()
                .flex()
                .flex_row()
                .gap(px(spacing::S2))
                .child(
                    div().debug_selector(|| "duplicates-cancel".into()).child(
                        TextButton::new("duplicates-cancel", &self.cancel_focus, "Cancel", accent)
                            .on_click({
                                let entity = entity.clone();
                                move |window, cx| entity.update(cx, |d, cx| d.close(window, cx))
                            }),
                    ),
                )
                .child(
                    div().debug_selector(|| "duplicates-remove".into()).child(
                        TextButton::new(
                            "duplicates-remove",
                            &self.remove_focus,
                            format!("Remove {}", repeats.len()),
                            accent,
                        )
                        .primary()
                        .on_click({
                            let entity = entity.clone();
                            move |window, cx| entity.update(cx, |d, cx| d.confirm(window, cx))
                        }),
                    ),
                )
        };

        let panel = div()
            .id("duplicates-dialog")
            .debug_selector(|| "duplicates-dialog".into())
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
            .on_action(cx.listener(|d, _: &DuplicatesCancel, window, cx| d.close(window, cx)))
            .on_action(cx.listener(|d, _: &DuplicatesNext, window, cx| d.move_focus(window, cx)))
            .on_action(
                cx.listener(|d, _: &DuplicatesPrevious, window, cx| d.move_focus(window, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(spacing::S1))
                    .child(
                        style::text_style(div(), typography::SECTION, cx)
                            .text_color(color::text_primary())
                            .child(TITLE),
                    )
                    .child(
                        style::text_style(div(), typography::BODY, cx)
                            .debug_selector(|| "duplicates-lead".into())
                            .text_color(color::text_secondary())
                            .child(SharedString::from(lead)),
                    ),
            )
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .justify_end()
                    .child(buttons),
            );

        // A transparent layer under the panel catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("duplicates-dialog-layer")
            .debug_selector(|| "duplicates-dialog-layer".into())
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
