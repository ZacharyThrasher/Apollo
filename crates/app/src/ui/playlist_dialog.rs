//! The playlist dialog: one small panel for the three questions playlist editing asks. Create and
//! Rename show a name field; Delete shows the playlist's name and asks once more. Cancel and the
//! action button close it; there is no scrim, like the popovers, and a click outside cancels.
//!
//! The dialog stays open while the write is out, with the spinner in its main button, and closes
//! when the edit succeeds. A failure leaves the usual error toast and the dialog open, so the
//! name is not lost. While a write is out Escape and clicks outside do nothing: closing could not
//! take the write back, and the dialog is where its outcome shows.
//!
//! Keyboard: Enter in the name field confirms, Tab and Shift+Tab move between the field, Cancel
//! and the main button, Escape cancels. Delete opens with Cancel focused, so a stray Enter keeps
//! the playlist.

use apollo_domain::library::{clean_playlist_title, Collection, MAX_PLAYLIST_TITLE};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, SharedString, Styled, Window,
};

use crate::library::edits::{Edit, EditEvent, LibraryEdits};
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::text_button::TextButton;
use crate::ui::text_field::{self, TextField, TextFieldEvent};

actions!(
    apollo_playlist_dialog,
    [DialogCancel, DialogNext, DialogPrevious]
);

/// Key context of the dialog.
pub const KEY_CONTEXT: &str = "PlaylistDialog";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", DialogCancel, Some(KEY_CONTEXT)),
        // Focus stays in the dialog while it is open.
        KeyBinding::new("tab", DialogNext, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", DialogPrevious, Some(KEY_CONTEXT)),
    ]);
}

pub const PANEL_WIDTH: f32 = 360.0;

/// What the dialog is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Name a new playlist. With `add_video`, that song goes into it once it exists.
    Create { add_video: Option<String> },
    Rename(Collection),
    Delete(Collection),
}

impl Mode {
    pub fn title(&self) -> &'static str {
        match self {
            Mode::Create { .. } => "New playlist",
            Mode::Rename(_) => "Rename playlist",
            Mode::Delete(_) => "Delete playlist?",
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            Mode::Create { .. } => "Create",
            Mode::Rename(_) => "Rename",
            Mode::Delete(_) => "Delete",
        }
    }

    /// The edit whose outcome closes (or reopens) the dialog.
    fn edit(&self) -> Edit {
        match self {
            Mode::Create { .. } => Edit::Create,
            Mode::Rename(_) => Edit::Rename,
            Mode::Delete(_) => Edit::Delete,
        }
    }

    fn has_field(&self) -> bool {
        !matches!(self, Mode::Delete(_))
    }
}

/// The controls Tab moves between, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stop {
    Field,
    Cancel,
    Action,
}

struct Open {
    mode: Mode,
    /// A write is out.
    busy: bool,
    /// The edit succeeded: the dialog closes on the next frame (events carry no window).
    done: bool,
    /// Where keyboard focus returns to when the dialog closes.
    return_focus: Option<FocusHandle>,
}

/// The shell's dialog, reachable from any screen.
#[derive(Clone)]
pub struct DialogHandle(pub Entity<PlaylistDialog>);

impl Global for DialogHandle {}

/// Open the dialog. Does nothing where there is no shell (a view on its own).
pub fn open(mode: Mode, window: &mut Window, cx: &mut App) {
    let Some(handle) = cx.try_global::<DialogHandle>().map(|h| h.0.clone()) else {
        return;
    };
    handle.update(cx, |dialog, cx| dialog.open(mode, window, cx));
}

pub struct PlaylistDialog {
    player: Entity<PlayerModel>,
    edits: Entity<LibraryEdits>,
    focus: FocusHandle,
    field: Entity<TextField>,
    cancel_focus: FocusHandle,
    action_focus: FocusHandle,
    open: Option<Open>,
}

impl PlaylistDialog {
    pub fn new(
        player: Entity<PlayerModel>,
        edits: Entity<LibraryEdits>,
        cx: &mut Context<Self>,
    ) -> Self {
        let field_focus = cx.focus_handle();
        let clear_focus = cx.focus_handle();
        let field = cx.new(|_| {
            TextField::new("Playlist name", field_focus, clear_focus).plain(MAX_PLAYLIST_TITLE)
        });
        cx.subscribe(&field, |dialog, _, event, cx| {
            if matches!(event, TextFieldEvent::Submit) {
                dialog.submit(cx);
            } else {
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&edits, |dialog, _, event, cx| dialog.edited(event, cx))
            .detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            edits,
            focus: cx.focus_handle(),
            field,
            cancel_focus: cx.focus_handle().tab_stop(true),
            action_focus: cx.focus_handle().tab_stop(true),
            open: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn mode(&self) -> Option<&Mode> {
        self.open.as_ref().map(|o| &o.mode)
    }

    pub fn is_busy(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.busy)
    }

    /// The name field, for tests and for the shell's focus handling.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    pub fn open(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        let initial = match &mode {
            Mode::Rename(collection) => collection.title.clone(),
            _ => String::new(),
        };
        self.field.update(cx, |f, cx| {
            f.set_text(&initial, cx);
            f.select_everything(cx);
        });
        let delete = !mode.has_field();
        self.open = Some(Open {
            mode,
            busy: false,
            done: false,
            return_focus,
        });
        if delete {
            window.focus(&self.cancel_focus, cx);
        } else {
            text_field::focus(&self.field, window, cx);
        }
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

    /// Cancel: closes unless a write is out.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_busy() {
            self.close(window, cx);
        }
    }

    /// The name as it would be sent, `None` while there is nothing to send.
    pub fn cleaned(&self, cx: &App) -> Option<String> {
        clean_playlist_title(self.field.read(cx).text())
    }

    /// Whether the main button can be pressed.
    pub fn can_submit(&self, cx: &App) -> bool {
        let Some(open) = self.open.as_ref().filter(|o| !o.busy) else {
            return false;
        };
        !open.mode.has_field() || self.cleaned(cx).is_some()
    }

    /// The main button: send the edit.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if !self.can_submit(cx) {
            return;
        }
        let Some(mode) = self.open.as_ref().map(|o| o.mode.clone()) else {
            return;
        };
        let title = self.cleaned(cx);
        match (mode, title) {
            (Mode::Create { add_video }, Some(title)) => {
                self.start(cx);
                self.edits.update(cx, |e, cx| e.create(title, add_video, cx));
            }
            (Mode::Rename(collection), Some(title)) => {
                if title == collection.title {
                    // Nothing changed: nothing to send.
                    self.finish(cx);
                    return;
                }
                self.start(cx);
                self.edits.update(cx, |e, cx| e.rename(collection, title, cx));
            }
            (Mode::Delete(collection), _) => {
                self.start(cx);
                self.edits.update(cx, |e, cx| e.delete(collection, cx));
            }
            _ => {}
        }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            open.busy = true;
        }
        cx.notify();
    }

    fn finish(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            open.busy = false;
            open.done = true;
        }
        cx.notify();
    }

    fn edited(&mut self, event: &EditEvent, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref().filter(|o| o.busy) else {
            return;
        };
        let mine = match (event, &open.mode) {
            (EditEvent::Created(_), Mode::Create { .. }) => true,
            (EditEvent::Renamed(c), Mode::Rename(asked)) => c.library_key() == asked.library_key(),
            (EditEvent::Deleted(c), Mode::Delete(asked)) => c.library_key() == asked.library_key(),
            _ => false,
        };
        if mine {
            self.finish(cx);
        } else if let EditEvent::Failed(edit) = event {
            if *edit == open.mode.edit() {
                if let Some(open) = self.open.as_mut() {
                    open.busy = false;
                }
                cx.notify();
            }
        }
    }

    /// The controls Tab moves between, in order.
    fn stops(&self) -> Vec<Stop> {
        match self.open.as_ref().map(|o| o.mode.has_field()) {
            Some(true) => vec![Stop::Field, Stop::Cancel, Stop::Action],
            _ => vec![Stop::Cancel, Stop::Action],
        }
    }

    fn current_stop(&self, window: &Window, cx: &App) -> Option<Stop> {
        if self.field.read(cx).focus_handle(cx).is_focused(window) {
            Some(Stop::Field)
        } else if self.cancel_focus.is_focused(window) {
            Some(Stop::Cancel)
        } else if self.action_focus.is_focused(window) {
            Some(Stop::Action)
        } else {
            None
        }
    }

    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let stops = self.stops();
        if stops.is_empty() {
            return;
        }
        let at = self
            .current_stop(window, cx)
            .and_then(|s| stops.iter().position(|x| *x == s));
        let next = match at {
            Some(at) => (at as isize + delta).rem_euclid(stops.len() as isize) as usize,
            None => 0,
        };
        match stops[next] {
            Stop::Field => text_field::focus(&self.field, window, cx),
            Stop::Cancel => window.focus(&self.cancel_focus, cx),
            Stop::Action => window.focus(&self.action_focus, cx),
        }
    }
}

impl Focusable for PlaylistDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PlaylistDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        if open.done {
            let entity = cx.entity();
            window.defer(cx, move |window, cx| {
                entity.update(cx, |d, cx| d.close(window, cx));
            });
        }
        let (accent, _) = self.player.read(cx).accent_now();
        let busy = open.busy;
        let mode = open.mode.clone();
        let can_submit = self.can_submit(cx);
        let entity = cx.entity();

        let body = match &mode {
            Mode::Delete(collection) => style::text_style(div(), typography::BODY, cx)
                .id("playlist-dialog-name")
                .debug_selector(|| "playlist-dialog-name".into())
                .truncate()
                .text_color(color::text_secondary())
                .child(SharedString::from(collection.title.clone()))
                .into_any_element(),
            _ => div()
                .debug_selector(|| "playlist-dialog-field".into())
                .child(self.field.clone())
                .into_any_element(),
        };

        let buttons = div()
            .flex()
            .flex_row()
            .flex_none()
            .justify_end()
            .gap(px(spacing::S2))
            .child(
                TextButton::new("playlist-dialog-cancel", &self.cancel_focus, "Cancel", accent)
                    .enabled(!busy)
                    .on_click({
                        let entity = entity.clone();
                        move |window, cx| entity.update(cx, |d, cx| d.cancel(window, cx))
                    }),
            )
            .child(
                TextButton::new(
                    "playlist-dialog-action",
                    &self.action_focus,
                    mode.action(),
                    accent,
                )
                .primary()
                .busy(busy)
                .enabled(can_submit || busy)
                .on_click({
                    let entity = entity.clone();
                    move |_, cx| entity.update(cx, |d, cx| d.submit(cx))
                }),
            );

        let panel = div()
            .id("playlist-dialog")
            .debug_selector(|| "playlist-dialog".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Dialog)
            .aria_label(mode.title())
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
            .on_action(cx.listener(|d, _: &DialogCancel, window, cx| d.cancel(window, cx)))
            // The name field hands Escape on to the dialog.
            .on_action(cx.listener(|d, _: &text_field::Escape, window, cx| d.cancel(window, cx)))
            .on_action(cx.listener(|d, _: &DialogNext, window, cx| d.move_focus(1, window, cx)))
            .on_action(
                cx.listener(|d, _: &DialogPrevious, window, cx| d.move_focus(-1, window, cx)),
            )
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child(mode.title()),
            )
            .child(body)
            .child(buttons);

        // A transparent layer under the panel catches the click that closes it (no scrim); the
        // title bar stays out of reach so the window controls keep working.
        div()
            .id("playlist-dialog-layer")
            .debug_selector(|| "playlist-dialog-layer".into())
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
                cx.listener(|d, _, window, cx| d.cancel(window, cx)),
            )
            .child(panel)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_has_its_words() {
        let c = Collection {
            kind: apollo_domain::library::CollectionKind::Playlist,
            id: apollo_domain::model::BrowseId::new("VLPLx"),
            title: "Mix".into(),
            subtitle: None,
            thumbnail: None,
        };
        assert_eq!(Mode::Create { add_video: None }.title(), "New playlist");
        assert_eq!(Mode::Create { add_video: None }.action(), "Create");
        assert_eq!(Mode::Rename(c.clone()).action(), "Rename");
        assert_eq!(Mode::Delete(c.clone()).title(), "Delete playlist?");
        assert_eq!(Mode::Delete(c).action(), "Delete");
    }
}
