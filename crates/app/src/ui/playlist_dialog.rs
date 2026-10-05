//! The playlist dialog: one small panel for the questions playlist editing asks. Create (and
//! saving the queue as a playlist) shows a name field; Edit shows the name, the description
//! (three wrapped rows) and who can see the playlist; Delete shows the playlist's name and asks
//! once more. Cancel and the action button close it; there is no scrim, like the popovers, and a
//! click outside cancels.
//!
//! Edit's description and visibility are known only once the playlist's page has been read (see
//! `LibraryEdits::load_details`); until then, and if that read fails, only the name is offered.
//! Only what changed is sent.
//!
//! The dialog stays open while the write is out, with the spinner in its main button, and closes
//! when the edit succeeds. A failure leaves the usual error toast and the dialog open, so the
//! name is not lost. While a write is out Escape and clicks outside do nothing: closing could not
//! take the write back, and the dialog is where its outcome shows.
//!
//! With Settings > Playlists > Create playlists instantly on, the three ways to make a playlist
//! (the Library's New playlist, Save the queue, Save the Crate) never show the dialog: they write
//! a private "Oct 5 \u{b7} Untitled" at once, and the shell opens its page ready to rename (see
//! [`PlaylistDialog::take_instant`]). Saving the Crate that way leaves the Crate as it is.
//!
//! Keyboard: Enter in the name field confirms (in the description it is a line break), Tab and
//! Shift+Tab move between the fields, the visibility button, Cancel and the main button, Escape
//! cancels. Delete opens with Cancel focused, so a stray Enter keeps
//! the playlist.

use apollo_domain::library::{
    clean_playlist_description, clean_playlist_title, Collection, PlaylistEdit, Privacy,
    MAX_PLAYLIST_DESCRIPTION, MAX_PLAYLIST_TITLE,
};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    actions, div, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, Global,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, SharedString, Styled, Window,
};

use crate::crate_list;
use crate::instant_playlist;
use crate::library::edits::{Edit, EditEvent, LibraryEdits};
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, INSTANT_PLAYLISTS};
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::choice_menu::{self, Choice};
use crate::ui::switch::LabeledSwitch;
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

/// The name a playlist made from the Crate starts with.
pub const CRATE_PLAYLIST_NAME: &str = "My Crate";

/// Rows of the description field.
const DESCRIPTION_ROWS: usize = 3;
/// Where the visibility popover hangs from, and how wide it is.
const PRIVACY_ANCHOR: &str = "playlist-dialog-privacy";
const PRIVACY_MENU_WIDTH: f32 = 160.0;

/// What the dialog is asking.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Name a new playlist. With `add_video`, that song goes into it once it exists.
    Create { add_video: Option<String> },
    /// Save the queue: a new playlist that holds `video_ids`, `name` being the one it starts with.
    FromQueue {
        name: String,
        video_ids: Vec<String>,
    },
    /// Save the Crate: a new playlist that holds `video_ids` (the Crate's order), `name` being the
    /// one it starts with. Offers to clear those songs from the Crate afterwards.
    FromCrate {
        name: String,
        video_ids: Vec<String>,
    },
    /// Change a playlist's name, description and visibility.
    Edit(Collection),
    Delete(Collection),
}

impl Mode {
    pub fn title(&self) -> &'static str {
        match self {
            Mode::Create { .. } | Mode::FromQueue { .. } | Mode::FromCrate { .. } => "New playlist",
            Mode::Edit(_) => "Edit playlist",
            Mode::Delete(_) => "Delete playlist?",
        }
    }

    pub fn action(&self) -> &'static str {
        match self {
            Mode::Create { .. } | Mode::FromQueue { .. } | Mode::FromCrate { .. } => "Create",
            Mode::Edit(_) => "Save",
            Mode::Delete(_) => "Delete",
        }
    }

    /// The edit whose outcome closes (or reopens) the dialog.
    fn edit(&self) -> Edit {
        match self {
            Mode::Create { .. } => Edit::Create,
            Mode::FromQueue { .. } => Edit::SaveQueue,
            Mode::FromCrate { .. } => Edit::SaveCrate,
            Mode::Edit(_) => Edit::Update,
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
    Description,
    Privacy,
    ClearCrate,
    Cancel,
    Action,
}

struct Open {
    mode: Mode,
    /// A write is out.
    busy: bool,
    /// Edit: the playlist's description and visibility have been read into the fields, so the
    /// dialog offers them. `privacy` is what the picker shows; `None` when the page did not say
    /// (nothing to choose then).
    synced: bool,
    privacy: Option<Privacy>,
    /// Save the Crate: take the saved songs out of the Crate once the playlist exists.
    clear_crate: bool,
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
    settings: Entity<SettingsModel>,
    /// An instant playlist's write is out, under this name.
    instant: Option<String>,
    /// An instant playlist now exists: the shell opens it on the next frame (events carry no
    /// window).
    instant_made: Option<Collection>,
    focus: FocusHandle,
    field: Entity<TextField>,
    description: Entity<TextField>,
    privacy_focus: FocusHandle,
    clear_crate_focus: FocusHandle,
    cancel_focus: FocusHandle,
    action_focus: FocusHandle,
    open: Option<Open>,
}

impl PlaylistDialog {
    pub fn new(
        player: Entity<PlayerModel>,
        edits: Entity<LibraryEdits>,
        settings: Entity<SettingsModel>,
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
        let description_focus = cx.focus_handle();
        let description_clear = cx.focus_handle();
        let description = cx.new(|_| {
            TextField::new("Description", description_focus, description_clear)
                .multiline(DESCRIPTION_ROWS, MAX_PLAYLIST_DESCRIPTION)
        });
        cx.subscribe(&description, |_, _, _, cx| cx.notify()).detach();
        cx.subscribe(&edits, |dialog, _, event, cx| dialog.edited(event, cx))
            .detach();
        // What the playlist's page says arrives some time after the dialog opened.
        cx.observe(&edits, |dialog, _, cx| dialog.sync_details(cx)).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            edits,
            settings,
            instant: None,
            instant_made: None,
            focus: cx.focus_handle(),
            field,
            description,
            privacy_focus: cx.focus_handle().tab_stop(true),
            clear_crate_focus: cx.focus_handle().tab_stop(true),
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

    /// The description field, for tests.
    pub fn description_field(&self) -> &Entity<TextField> {
        &self.description
    }

    /// Edit: whether the description and visibility are offered yet.
    pub fn details_shown(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.synced)
    }

    /// The visibility the picker shows, once it is known.
    pub fn privacy(&self) -> Option<Privacy> {
        self.open.as_ref().and_then(|o| o.privacy)
    }

    pub fn open(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let makes = matches!(
            mode,
            Mode::Create { .. } | Mode::FromQueue { .. } | Mode::FromCrate { .. }
        );
        if makes && self.settings.read(cx).get(&INSTANT_PLAYLISTS) {
            self.make_instantly(mode, cx);
            return;
        }
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        let initial = match &mode {
            Mode::Edit(collection) => collection.title.clone(),
            Mode::FromQueue { name, .. } | Mode::FromCrate { name, .. } => name.clone(),
            _ => String::new(),
        };
        self.field.update(cx, |f, cx| {
            f.set_text(&initial, cx);
            f.select_everything(cx);
        });
        self.description.update(cx, |f, cx| f.set_text("", cx));
        let delete = !mode.has_field();
        if let Mode::Edit(collection) = &mode {
            let collection = collection.clone();
            self.edits.update(cx, |e, cx| e.load_details(&collection, cx));
        }
        self.open = Some(Open {
            mode,
            busy: false,
            synced: false,
            privacy: None,
            clear_crate: false,
            done: false,
            return_focus,
        });
        self.sync_details(cx);
        if delete {
            window.focus(&self.cancel_focus, cx);
        } else {
            text_field::focus(&self.field, window, cx);
        }
        cx.notify();
    }

    /// Make the playlist `mode` asks for without asking for a name: private, named for today,
    /// holding the songs in one write. Does nothing while another playlist is being made.
    fn make_instantly(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.instant.is_some() || self.edits.read(cx).is_busy("create") {
            return;
        }
        let (month, day) = instant_playlist::today();
        let name = {
            let edits = self.edits.read(cx);
            instant_playlist::default_name(month, day, |n| edits.has_playlist_titled(n))
        };
        self.instant = Some(name.clone());
        self.edits.update(cx, |e, cx| match mode {
            Mode::Create { add_video } => e.create(name, add_video, cx),
            Mode::FromQueue { video_ids, .. } => e.create_with_songs(name, video_ids, cx),
            // The Crate is left as it is: its dialog's "Clear Crate after saving" is not asked.
            Mode::FromCrate { video_ids, .. } => e.create_with_crate(name, video_ids, cx),
            Mode::Edit(_) | Mode::Delete(_) => {}
        });
    }

    /// The playlist an instant create just made, once, for the shell to open and rename.
    pub fn take_instant(&mut self) -> Option<Collection> {
        self.instant_made.take()
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

    /// Edit: once the playlist's description and visibility are known, put them in the fields.
    fn sync_details(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref().filter(|o| !o.synced) else {
            return;
        };
        let Mode::Edit(collection) = &open.mode else {
            return;
        };
        let Some(details) = self.edits.read(cx).details(collection).cloned() else {
            return;
        };
        self.description
            .update(cx, |f, cx| f.set_text(&details.description, cx));
        if let Some(open) = self.open.as_mut() {
            open.synced = true;
            open.privacy = details.privacy;
        }
        cx.notify();
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
            (Mode::FromQueue { video_ids, .. }, Some(title)) => {
                self.start(cx);
                self.edits
                    .update(cx, |e, cx| e.create_with_songs(title, video_ids, cx));
            }
            (Mode::FromCrate { video_ids, .. }, Some(title)) => {
                self.start(cx);
                self.edits
                    .update(cx, |e, cx| e.create_with_crate(title, video_ids, cx));
            }
            (Mode::Edit(collection), Some(title)) => {
                let change = self.change(&collection, title, cx);
                if change.is_empty() {
                    // Nothing changed: nothing to send.
                    self.finish(cx);
                    return;
                }
                self.start(cx);
                self.edits.update(cx, |e, cx| e.edit(collection, change, cx));
            }
            (Mode::Delete(collection), _) => {
                self.start(cx);
                self.edits.update(cx, |e, cx| e.delete(collection, cx));
            }
            _ => {}
        }
    }

    /// What the fields change about `collection`: the name if it differs, and, once they are
    /// known, the description and visibility if they differ.
    fn change(&self, collection: &Collection, title: String, cx: &App) -> PlaylistEdit {
        let mut change = PlaylistEdit::default();
        if title != collection.title {
            change.title = Some(title);
        }
        let Some(open) = self.open.as_ref().filter(|o| o.synced) else {
            return change;
        };
        let Some(details) = self.edits.read(cx).details(collection) else {
            return change;
        };
        let description = clean_playlist_description(self.description.read(cx).text());
        if description != clean_playlist_description(&details.description) {
            change.description = Some(description);
        }
        if let (Some(asked), Some(held)) = (open.privacy, details.privacy) {
            if asked != held {
                change.privacy = Some(asked);
            }
        }
        change
    }

    /// The visibility button: the three choices, the one shown checked.
    fn pick_privacy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref().filter(|o| !o.busy) else {
            return;
        };
        let Some(current) = open.privacy else {
            return;
        };
        let choices = Privacy::ALL
            .iter()
            .map(|p| Choice::new(p.label(), *p == current))
            .collect();
        let entity = cx.entity();
        choice_menu::open(
            PRIVACY_ANCHOR,
            PRIVACY_MENU_WIDTH,
            choices,
            move |index, _, cx| {
                if let Some(privacy) = Privacy::ALL.get(index) {
                    entity.update(cx, |d, cx| d.set_privacy(*privacy, cx));
                }
            },
            window,
            cx,
        );
    }

    /// Whether the Crate is to be cleared once the playlist is made (Save the Crate only).
    pub fn clear_crate(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.clear_crate)
    }

    pub fn set_clear_crate(&mut self, clear: bool, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut().filter(|o| !o.busy) {
            open.clear_crate = clear;
            cx.notify();
        }
    }

    pub fn set_privacy(&mut self, privacy: Privacy, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut().filter(|o| o.privacy.is_some() && !o.busy) {
            open.privacy = Some(privacy);
            cx.notify();
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
        if let Some(name) = self.instant.as_ref() {
            match event {
                EditEvent::Created(c) if c.title == *name => {
                    self.instant = None;
                    self.instant_made = Some(c.clone());
                    cx.notify();
                }
                EditEvent::Failed(Edit::Create | Edit::SaveQueue | Edit::SaveCrate) => {
                    self.instant = None;
                }
                _ => {}
            }
            return;
        }
        let Some(open) = self.open.as_ref().filter(|o| o.busy) else {
            return;
        };
        let mine = match (event, &open.mode) {
            (
                EditEvent::Created(_),
                Mode::Create { .. } | Mode::FromQueue { .. } | Mode::FromCrate { .. },
            ) => true,
            (EditEvent::Edited(c), Mode::Edit(asked)) => c.library_key() == asked.library_key(),
            (EditEvent::Deleted(c), Mode::Delete(asked)) => c.library_key() == asked.library_key(),
            _ => false,
        };
        if mine {
            // The songs are in the playlist now: the ones saved leave the Crate, if asked.
            if let (true, Mode::FromCrate { video_ids, .. }) = (open.clear_crate, &open.mode) {
                if let Some(model) = crate_list::handle(cx) {
                    let video_ids = video_ids.clone();
                    model.update(cx, |m, cx| {
                        m.remove_many(&video_ids, cx);
                    });
                }
            }
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
        let Some(open) = self.open.as_ref() else {
            return vec![Stop::Cancel, Stop::Action];
        };
        let mut stops = Vec::new();
        if open.mode.has_field() {
            stops.push(Stop::Field);
        }
        if matches!(open.mode, Mode::FromCrate { .. }) {
            stops.push(Stop::ClearCrate);
        }
        if open.synced {
            stops.push(Stop::Description);
            if open.privacy.is_some() {
                stops.push(Stop::Privacy);
            }
        }
        stops.extend([Stop::Cancel, Stop::Action]);
        stops
    }

    fn current_stop(&self, window: &Window, cx: &App) -> Option<Stop> {
        if self.field.read(cx).focus_handle(cx).is_focused(window) {
            Some(Stop::Field)
        } else if self.description.read(cx).focus_handle(cx).is_focused(window) {
            Some(Stop::Description)
        } else if self.privacy_focus.is_focused(window) {
            Some(Stop::Privacy)
        } else if self.clear_crate_focus.is_focused(window) {
            Some(Stop::ClearCrate)
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
            Stop::Description => text_field::focus(&self.description, window, cx),
            Stop::Privacy => window.focus(&self.privacy_focus, cx),
            Stop::ClearCrate => window.focus(&self.clear_crate_focus, cx),
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
            _ => {
                let details = open.synced.then(|| {
                    let description = div()
                        .debug_selector(|| "playlist-dialog-description".into())
                        .child(self.description.clone());
                    let privacy = open.privacy.map(|privacy| {
                        let entity = entity.clone();
                        div()
                            .debug_selector(|| "playlist-dialog-privacy-row".into())
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .child(
                                style::text_style(div(), typography::BODY, cx)
                                    .text_color(color::text_secondary())
                                    .child("Privacy"),
                            )
                            .child(anchor::anchored(
                                PRIVACY_ANCHOR,
                                TextButton::new(
                                    "playlist-dialog-privacy",
                                    &self.privacy_focus,
                                    privacy.label(),
                                    accent,
                                )
                                .enabled(!busy)
                                .on_click(move |window, cx| {
                                    entity.update(cx, |d, cx| d.pick_privacy(window, cx))
                                }),
                            ))
                    });
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(spacing::S3))
                        .child(description)
                        .children(privacy)
                });
                let clear_crate = matches!(mode, Mode::FromCrate { .. }).then(|| {
                    let entity = entity.clone();
                    let on = open.clear_crate;
                    // Only part of the Crate is being saved (the picked songs): only those leave.
                    let partial = match (&mode, crate_list::handle(cx)) {
                        (Mode::FromCrate { video_ids, .. }, Some(model)) => {
                            video_ids.len() < model.read(cx).len()
                        }
                        _ => false,
                    };
                    let label = if partial {
                        "Take these songs out of the Crate after saving"
                    } else {
                        "Clear Crate after saving"
                    };
                    div()
                        .debug_selector(|| "playlist-dialog-clear-crate".into())
                        .child(
                            LabeledSwitch::new(
                                "playlist-dialog-clear-crate",
                                &self.clear_crate_focus,
                                label,
                                on,
                                accent,
                            )
                            .enabled(!busy)
                            .on_toggle(move |_, cx| {
                                entity.update(cx, |d, cx| d.set_clear_crate(!on, cx))
                            }),
                        )
                });
                div()
                    .flex()
                    .flex_col()
                    .gap(px(spacing::S3))
                    .child(
                        div()
                            .debug_selector(|| "playlist-dialog-field".into())
                            .child(self.field.clone()),
                    )
                    .children(details)
                    .children(clear_crate)
                    .into_any_element()
            }
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
        assert_eq!(Mode::Edit(c.clone()).title(), "Edit playlist");
        assert_eq!(Mode::Edit(c.clone()).action(), "Save");
        let queue = Mode::FromQueue {
            name: "My queue".into(),
            video_ids: vec![],
        };
        assert_eq!(queue.title(), "New playlist");
        assert_eq!(queue.action(), "Create");
        assert_eq!(Mode::Delete(c.clone()).title(), "Delete playlist?");
        assert_eq!(Mode::Delete(c).action(), "Delete");
    }
}
