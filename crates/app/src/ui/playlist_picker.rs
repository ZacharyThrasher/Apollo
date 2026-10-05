//! The add-to-playlist picker: a small popover for putting one song (or several) into your
//! playlists, anchored to the action (or the selected row) that opened it. With no anchor it sits
//! in the middle of the window. There is no scrim: the page behind stays as it is, and a click on
//! it closes the popover.
//!
//! The keyboard is the main way in. Focus lands in the filter box at the top; typing narrows the
//! list (every word must be in the name). The playlists added to most recently sit on top, then a
//! divider, then the rest in the order the service gave. When what you typed is not the name of
//! any playlist, a last row offers to create it, with the song in it.
//!
//! * Up and Down move the cursor.
//! * Space marks the playlist under the cursor (a check in the accent colour) while the filter is
//!   empty, since a space is a letter once you are typing; Ctrl+Space marks at any time.
//! * Enter adds to every marked playlist, or to the one under the cursor when none is marked, and
//!   closes. On the Create row it creates. Ctrl+Enter creates the typed name from anywhere.
//! * Escape closes. A click on a row adds to it at once.
//! * Marks are add-only: a playlist that already holds the song shows its check and cannot be
//!   marked or unmarked. Taking a song out is done on the playlist's own page.
//!
//! The same panel also chooses the target playlist (see `target_playlist`): opened that way its
//! caption says so, a click or Enter on a playlist makes it the target (and adds the songs it was
//! opened with, if any) instead of just adding, and a Clear button takes the target away. In both
//! modes the current target's row carries a star in the accent colour. Typing a new name makes a playlist and targets it.
//! There is nothing to mark there, so Space does nothing.
//!
//! The add is optimistic and rolls back with an error if the service refuses (see
//! `library::writes`). Settings > Playback can keep the picker open after an add, and switch the
//! P key off. The playlists added to are remembered as a settings value (`RECENT_PLAYLISTS`).
//!
//! The picker takes a list of songs ([`PickerSong`]); several songs go into each chosen playlist
//! one after another, a short pause apart, and a new playlist is made holding all of them in one
//! write. `playlists_for` is asked about the first song only, so for several songs only that one's
//! membership is known up front.

use apollo_domain::library::{
    bare_playlist_id, clean_playlist_title, PlaylistTarget, MAX_PLAYLIST_TITLE,
};
use apollo_domain::listing::LoadFailure;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, AnyElement, App, Bounds, Context, Entity, FocusHandle,
    Focusable, Global, IntoElement, KeyBinding, MouseButton, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, Styled, Subscription, Window,
};

use crate::assets::Icon;
use crate::crate_list;
use crate::library::edits::{Edit, EditEvent, LibraryEdits};
use crate::library::model::Load;
use crate::library::source::PROBE_VIDEO;
use crate::library::writes::LibraryWrites;
use crate::player::PlayerModel;
use crate::settings::{SettingsModel, PICKER_KEY, PICKER_STAY_OPEN, RECENT_PLAYLISTS};
use crate::style::{self, color};
use crate::target_playlist::{Target, TargetHandle};
use crate::ui::button::keyboard_focused;
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::text_field::{self, TextField, TextFieldEvent};

actions!(
    apollo_picker,
    [PickerUp, PickerDown, PickerMark, PickerCreate, PickerClose, PickerStay]
);

/// Key context of the picker.
pub const KEY_CONTEXT: &str = "PlaylistPicker";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", PickerUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", PickerDown, Some(KEY_CONTEXT)),
        // The filter box types its own spaces once it has text, and its own Escape.
        KeyBinding::new("space", PickerMark, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-space", PickerMark, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-enter", PickerCreate, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", PickerClose, Some(KEY_CONTEXT)),
        // Focus stays in the panel while it is open.
        KeyBinding::new("tab", PickerStay, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", PickerStay, Some(KEY_CONTEXT)),
    ]);
}

pub const PANEL_WIDTH: f32 = 280.0;
pub const PANEL_MAX_HEIGHT: f32 = 320.0;
/// Rows match the track menu's: 32px.
const ROW_HEIGHT: f32 = layout::MIN_HIT_TARGET;
/// The shortest the popover gets, however little room there is.
const MIN_HEIGHT: f32 = 160.0;
const SKELETON_ROWS: usize = 5;
/// Gap between the anchor and the popover, and the least it keeps from the window's edge.
const ANCHOR_GAP: f32 = spacing::S1;
const EDGE_MARGIN: f32 = spacing::S2;
/// How many recently used playlists are remembered.
pub const RECENTS_MAX: usize = 8;
/// The filter box takes no more than a playlist name can be.
const FILTER_MAX: usize = MAX_PLAYLIST_TITLE;

/// A song the picker is opened for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerSong {
    pub video_id: String,
    pub title: SharedString,
}

impl PickerSong {
    pub fn new(video_id: impl Into<String>, title: impl Into<SharedString>) -> Self {
        Self {
            video_id: video_id.into(),
            title: title.into(),
        }
    }
}

// ---- what the list shows -----------------------------------------------------------------------

/// One row of the list, from the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A playlist: its position in the loaded list.
    Playlist(usize),
    /// Make a playlist of this name.
    Create(String),
}

/// Which playlists the list shows and in what order: recent ones, then the rest, then the Create
/// row (when the typed name is not a playlist). The numbers are positions in the loaded list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub recent: Vec<usize>,
    pub rest: Vec<usize>,
    pub create: Option<String>,
}

impl Layout {
    pub fn len(&self) -> usize {
        self.recent.len() + self.rest.len() + usize::from(self.create.is_some())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `n`th row from the top.
    pub fn row(&self, n: usize) -> Option<Row> {
        let playlists = self.recent.len() + self.rest.len();
        if n < self.recent.len() {
            Some(Row::Playlist(self.recent[n]))
        } else if n < playlists {
            Some(Row::Playlist(self.rest[n - self.recent.len()]))
        } else if n == playlists {
            self.create.clone().map(Row::Create)
        } else {
            None
        }
    }

    /// Where the playlist at `list_ix` sits among the rows.
    pub fn position_of(&self, list_ix: usize) -> Option<usize> {
        self.recent
            .iter()
            .chain(self.rest.iter())
            .position(|&i| i == list_ix)
    }

    /// A divider sits between the recent playlists and the rest.
    fn divider_after_recent(&self) -> bool {
        !self.recent.is_empty() && !self.rest.is_empty()
    }

    /// A divider sits above the Create row when something is above it.
    fn divider_before_create(&self) -> bool {
        self.create.is_some() && self.recent.len() + self.rest.len() > 0
    }

    /// The `n`th row's place among the list's children, dividers counted (for scrolling to it).
    fn child_index(&self, n: usize) -> usize {
        let mut child = n;
        if self.divider_after_recent() && n >= self.recent.len() {
            child += 1;
        }
        if self.divider_before_create() && n >= self.recent.len() + self.rest.len() {
            child += 1;
        }
        child
    }
}

/// Every word of `query` appears in `title`, in any case; an empty query keeps everything.
fn matches(title: &str, words: &[String]) -> bool {
    let title = title.to_lowercase();
    words.iter().all(|w| title.contains(w.as_str()))
}

/// Lay the list out for what is typed. `recents` are bare playlist ids, newest first; ones the
/// list does not hold are ignored.
pub fn arrange(list: &[PlaylistTarget], recents: &[String], query: &str) -> Layout {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let recent_all: Vec<usize> = recents
        .iter()
        .filter_map(|id| list.iter().position(|t| &t.id == id))
        .collect();
    let keep = |ix: &usize| matches(&list[*ix].title, &words);
    let recent: Vec<usize> = {
        let mut seen = Vec::new();
        recent_all
            .iter()
            .copied()
            .filter(|ix| {
                let fresh = !seen.contains(ix);
                seen.push(*ix);
                fresh
            })
            .filter(keep)
            .collect()
    };
    let rest: Vec<usize> = (0..list.len())
        .filter(|ix| !recent_all.contains(ix))
        .filter(keep)
        .collect();
    let create = clean_playlist_title(query).filter(|name| {
        let lower = name.to_lowercase();
        !list.iter().any(|t| t.title.to_lowercase() == lower)
    });
    Layout {
        recent,
        rest,
        create,
    }
}

/// What Enter does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Playlists to add the songs to, as positions in the loaded list.
    pub add: Vec<usize>,
    /// A playlist to make, holding the songs.
    pub create: Option<String>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.create.is_none()
    }
}

/// What Enter does with `marked` playlists (ids, in the order they were marked) and the cursor on
/// row `cursor`: with marks, add to those (and create when the cursor is on the Create row);
/// without, act on the row under the cursor. A playlist that already holds the songs is left out.
pub fn plan(
    layout: &Layout,
    list: &[PlaylistTarget],
    marked: &[String],
    cursor: usize,
    held: &dyn Fn(usize) -> bool,
) -> Plan {
    let on_cursor = layout.row(cursor);
    if marked.is_empty() {
        return match on_cursor {
            Some(Row::Playlist(ix)) if !held(ix) => Plan {
                add: vec![ix],
                create: None,
            },
            Some(Row::Create(name)) => Plan {
                add: Vec::new(),
                create: Some(name),
            },
            _ => Plan::default(),
        };
    }
    Plan {
        add: marked
            .iter()
            .filter_map(|id| list.iter().position(|t| &t.id == id))
            .filter(|&ix| !held(ix))
            .collect(),
        create: match on_cursor {
            Some(Row::Create(name)) => Some(name),
            _ => None,
        },
    }
}

/// Parse the stored recents: bare ids separated by commas.
pub fn parse_recents(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Put `id` first (it was just used), keeping at most [`RECENTS_MAX`].
pub fn push_recent(recents: &mut Vec<String>, id: &str) {
    recents.retain(|r| r != id);
    recents.insert(0, id.to_owned());
    recents.truncate(RECENTS_MAX);
}

// ---- the picker ---------------------------------------------------------------------------------

struct Open {
    /// The songs being added; in target mode, the ones to add once the target is chosen (maybe
    /// none).
    songs: Vec<PickerSong>,
    /// Choosing the target playlist rather than adding: see the module docs.
    target_mode: bool,
    playlists: Load<Vec<PlaylistTarget>>,
    /// The row the keyboard is on, counted from the top of the list (Create row included).
    cursor: usize,
    /// Ids of the playlists marked for Enter, in the order they were marked.
    marked: Vec<String>,
    /// Playlists made here, which hold every song.
    created: Vec<String>,
    /// Answers for an older request (Retry, reopen) are dropped.
    request: u64,
    /// Where keyboard focus returns to when the panel closes.
    return_focus: Option<FocusHandle>,
    /// Window-space bounds of what opened it; none centres the popover.
    anchor: Option<Bounds<Pixels>>,
}

pub struct PlaylistPicker {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    edits: Entity<LibraryEdits>,
    settings: Entity<SettingsModel>,
    focus: FocusHandle,
    retry_focus: FocusHandle,
    /// The Clear button of the target mode's caption.
    clear_focus: FocusHandle,
    field: Entity<TextField>,
    /// Listens to the filter box (Enter, typing); made on the first open, which has a window.
    field_events: Option<Subscription>,
    scroll: ScrollHandle,
    open: Option<Open>,
    next_request: u64,
    /// The name of a playlist this picker asked to be made, until it exists.
    awaiting_create: Option<String>,
    /// The playlist awaited is to become the target when it exists.
    awaiting_target: bool,
    /// The songs the awaited playlist is made with; they are filed (leave the Crate, if that is
    /// on) once it exists.
    awaiting_filed: Vec<String>,
}

/// The shell's picker, reachable from any screen.
#[derive(Clone)]
pub struct PickerHandle(pub Entity<PlaylistPicker>);

impl Global for PickerHandle {}

/// Open the shell's picker for `songs`, next to the element that recorded `anchor` (see
/// `ui::anchor`) or in the middle of the window. Does nothing where there is no shell.
pub fn open_songs(
    songs: Vec<PickerSong>,
    anchor: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(handle) = cx.try_global::<PickerHandle>().map(|h| h.0.clone()) else {
        return;
    };
    let bounds = anchor.and_then(|key| crate::ui::anchor::bounds_of(key, cx));
    handle.update(cx, |p, cx| p.open_songs(songs, bounds, window, cx));
}

/// Open the shell's picker to choose the target playlist (what A adds to). `songs` are added to
/// the playlist as it is chosen; none just changes the target. `anchor` as for [`open_songs`].
pub fn open_target(
    songs: Vec<PickerSong>,
    anchor: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(handle) = cx.try_global::<PickerHandle>().map(|h| h.0.clone()) else {
        return;
    };
    let bounds = anchor.and_then(|key| crate::ui::anchor::bounds_of(key, cx));
    handle.update(cx, |p, cx| p.open_target(songs, bounds, window, cx));
}

/// The P key is on (Settings > Playback), for the shell's picker.
pub fn key_enabled(cx: &App) -> bool {
    cx.try_global::<PickerHandle>()
        .is_none_or(|h| h.0.read(cx).key_enabled(cx))
}

impl PlaylistPicker {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        edits: Entity<LibraryEdits>,
        settings: Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&edits, |m, _, event: &EditEvent, cx| m.edited(event, cx))
            .detach();
        let (field_focus, clear_focus) = (cx.focus_handle(), cx.focus_handle());
        let field = cx.new(|_| {
            TextField::new("Find or create a playlist", field_focus, clear_focus)
                .plain(FILTER_MAX)
                .space_to_owner_when_empty()
        });
        Self {
            player,
            writes,
            edits,
            settings,
            focus: cx.focus_handle(),
            retry_focus: cx.focus_handle().tab_stop(true),
            clear_focus: cx.focus_handle().tab_stop(true),
            field,
            field_events: None,
            scroll: ScrollHandle::new(),
            open: None,
            next_request: 1,
            awaiting_create: None,
            awaiting_target: false,
            awaiting_filed: Vec::new(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The first song the panel is open for.
    pub fn video(&self) -> Option<&str> {
        self.open
            .as_ref()
            .and_then(|o| o.songs.first())
            .map(|s| s.video_id.as_str())
    }

    /// Every song the panel is open for.
    pub fn songs(&self) -> &[PickerSong] {
        self.open.as_ref().map_or(&[], |o| o.songs.as_slice())
    }

    pub fn playlists(&self) -> Option<&Load<Vec<PlaylistTarget>>> {
        self.open.as_ref().map(|o| &o.playlists)
    }

    /// The bounds the popover is anchored to.
    pub fn anchor(&self) -> Option<Bounds<Pixels>> {
        self.open.as_ref().and_then(|o| o.anchor)
    }

    /// The row the keyboard is on, counted from the top of the list.
    pub fn cursor(&self) -> Option<usize> {
        self.open.as_ref().map(|o| o.cursor)
    }

    /// The playlists marked for Enter (ids, in the order marked).
    pub fn marked(&self) -> &[String] {
        self.open.as_ref().map_or(&[], |o| o.marked.as_slice())
    }

    /// The filter box.
    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The playlists the list is made of: the loaded ones, none when loading them failed (a new
    /// playlist can still be made), nothing yet while they load.
    fn list(&self) -> Option<&[PlaylistTarget]> {
        match &self.open.as_ref()?.playlists {
            Load::Ready(list) => Some(list.as_slice()),
            Load::Failed(_) => Some(&[]),
            Load::Idle | Load::Loading => None,
        }
    }

    /// The rows the list shows now (none until the playlists are loaded).
    pub fn layout(&self, cx: &App) -> Option<Layout> {
        let list = self.list()?;
        let recents = self.recent_ids(cx);
        Some(arrange(list, &recents, self.field.read(cx).text()))
    }

    /// The P key is on (Settings > Playback).
    pub fn key_enabled(&self, cx: &App) -> bool {
        self.settings.read(cx).get(&PICKER_KEY)
    }

    fn stay_open(&self, cx: &App) -> bool {
        self.settings.read(cx).get(&PICKER_STAY_OPEN)
    }

    /// The recently used playlists' ids, newest first.
    pub fn recent_ids(&self, cx: &App) -> Vec<String> {
        parse_recents(&self.settings.read(cx).get_text(&RECENT_PLAYLISTS))
    }

    fn remember(&self, ids: &[String], cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let mut recents = self.recent_ids(cx);
        for id in ids {
            push_recent(&mut recents, id);
        }
        let text = recents.join(",");
        self.settings
            .update(cx, |s, cx| s.set_text(&RECENT_PLAYLISTS, &text, cx));
    }

    /// Open the panel for a track in the middle of the window. Focus moves into it and comes
    /// back on close.
    pub fn open(
        &mut self,
        video_id: &str,
        title: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_with(video_id, title, None, window, cx);
    }

    /// Open the panel next to the element that recorded `anchor` (see `ui::anchor`); centred
    /// when nothing is recorded.
    pub fn open_at(
        &mut self,
        video_id: &str,
        title: impl Into<SharedString>,
        anchor: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = crate::ui::anchor::bounds_of(anchor, cx);
        self.open_with(video_id, title, bounds, window, cx);
    }

    /// Open the panel anchored to `anchor` (window space); centred when there is none.
    pub fn open_with(
        &mut self,
        video_id: &str,
        title: impl Into<SharedString>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_songs(vec![PickerSong::new(video_id, title)], anchor, window, cx);
    }

    /// Open the panel to add all of `video_ids` (in order) to one playlist, the caption saying
    /// `title` (the Crate's "Add all to\u{2026}"). Nothing opens for an empty list.
    pub fn open_many(
        &mut self,
        video_ids: Vec<String>,
        title: impl Into<SharedString>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        let songs = video_ids
            .into_iter()
            .map(|id| PickerSong::new(id, title.clone()))
            .collect();
        self.open_songs(songs, anchor, window, cx);
    }

    /// Open the panel for one or more songs (none does nothing).
    pub fn open_songs(
        &mut self,
        songs: Vec<PickerSong>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if songs.is_empty() {
            return;
        }
        self.open_panel(songs, false, anchor, window, cx);
    }

    /// Open the panel to choose the target playlist; `songs` (maybe none) go into it as it is
    /// chosen.
    pub fn open_target(
        &mut self,
        songs: Vec<PickerSong>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_panel(songs, true, anchor, window, cx);
    }

    fn open_panel(
        &mut self,
        songs: Vec<PickerSong>,
        target_mode: bool,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx))
            // Focus that sits in the panel itself is not a place to come back to.
            .filter(|handle| !self.focus.contains(handle, window));
        if self.field_events.is_none() {
            let field = self.field.clone();
            self.field_events = Some(cx.subscribe_in(
                &field,
                window,
                |m, _, event: &TextFieldEvent, window, cx| match event {
                    TextFieldEvent::Submit => m.commit(window, cx),
                    TextFieldEvent::Changed => m.typed(cx),
                },
            ));
        }
        if !target_mode {
            // Songs filed through this panel leave the Crate (if that is on) once it closes.
            let videos: Vec<String> = songs.iter().map(|s| s.video_id.clone()).collect();
            if let Some(model) = crate_list::handle(cx) {
                model.update(cx, |m, _| m.hold(&videos));
            }
        }
        self.open = Some(Open {
            songs,
            target_mode,
            playlists: Load::Loading,
            cursor: 0,
            marked: Vec::new(),
            created: Vec::new(),
            request: 0,
            return_focus,
            anchor,
        });
        self.field.update(cx, |f, cx| f.set_text("", cx));
        text_field::focus(&self.field, window, cx);
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.load(cx);
        cx.notify();
    }

    /// Close without touching focus: the account changed under the panel.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            self.release_crate(cx);
            cx.notify();
        }
    }

    /// The panel is gone: songs filed through it leave the Crate now.
    fn release_crate(&self, cx: &mut Context<Self>) {
        if let Some(model) = crate_list::handle(cx) {
            model.update(cx, |m, cx| m.release(cx));
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
        self.release_crate(cx);
        cx.notify();
    }

    /// Ask for the list again after it failed to load.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        self.load(cx);
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let request = self.next_request;
        self.next_request += 1;
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.request = request;
        open.playlists = Load::Loading;
        // The service says which playlists hold one song: the first. Choosing a target with no
        // song at hand asks about the playing one, or a fixed public video (only the list of
        // playlists matters there).
        let video = match open.songs.first() {
            Some(song) => song.video_id.clone(),
            None => self
                .player
                .read(cx)
                .current_video()
                .unwrap_or_else(|| PROBE_VIDEO.to_owned()),
        };
        let source = self.player.read(cx).library_source();
        cx.spawn(async move |this, cx| {
            let result = match source {
                Some(source) => {
                    cx.background_executor()
                        .spawn(async move { source.playlists_for(&video) })
                        .await
                }
                None => Err(LoadFailure::Other),
            };
            let _ = this.update(cx, |m, cx| m.loaded(request, result, cx));
        })
        .detach();
        cx.notify();
    }

    fn loaded(
        &mut self,
        request: u64,
        result: Result<Vec<PlaylistTarget>, LoadFailure>,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.open.as_mut().filter(|o| o.request == request) else {
            return;
        };
        open.playlists = match result {
            Ok(list) => Load::Ready(list),
            Err(failure) => Load::Failed(failure),
        };
        open.cursor = 0;
        cx.notify();
    }

    /// The filter changed: the cursor goes back to the first row.
    fn typed(&mut self, cx: &mut Context<Self>) {
        if let Some(open) = self.open.as_mut() {
            open.cursor = 0;
            self.scroll.scroll_to_item(0);
        }
        cx.notify();
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(len) = self.layout(cx).map(|l| l.len()) else {
            return;
        };
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if len == 0 {
            return;
        }
        open.cursor = (open.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        let cursor = open.cursor;
        if let Some(layout) = self.layout(cx) {
            self.scroll.scroll_to_item(layout.child_index(cursor));
        }
        cx.notify();
    }

    /// Whether the playlist at `list_ix` already holds every song (for several songs, only what
    /// was added here is known beyond the first).
    pub fn holds_all(&self, list_ix: usize, cx: &App) -> bool {
        let Some(open) = self.open.as_ref() else {
            return false;
        };
        // Choosing a target: nothing is "held", any playlist can be chosen.
        if open.target_mode {
            return false;
        }
        let Load::Ready(list) = &open.playlists else {
            return false;
        };
        let Some(target) = list.get(list_ix) else {
            return false;
        };
        if open.created.contains(&target.id) {
            return true;
        }
        let writes = self.writes.read(cx);
        open.songs.iter().enumerate().all(|(n, song)| {
            if n == 0 {
                writes.holds(target, &song.video_id)
            } else {
                writes.added_here(target, &song.video_id)
            }
        })
    }

    /// Space: mark the playlist under the cursor for Enter, or take its mark back. A playlist
    /// that already holds the songs, and the Create row, take no mark.
    pub fn mark_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(layout) = self.layout(cx) else {
            return;
        };
        let Some(cursor) = self.open.as_ref().map(|o| o.cursor) else {
            return;
        };
        if self.open.as_ref().is_some_and(|o| o.target_mode) {
            return;
        }
        let Some(Row::Playlist(ix)) = layout.row(cursor) else {
            return;
        };
        if self.holds_all(ix, cx) {
            return;
        }
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Load::Ready(list) = &open.playlists else {
            return;
        };
        let id = list[ix].id.clone();
        if let Some(at) = open.marked.iter().position(|m| m == &id) {
            open.marked.remove(at);
        } else {
            open.marked.push(id);
        }
        cx.notify();
    }

    /// What Enter would do now.
    pub fn plan(&self, cx: &App) -> Plan {
        let (Some(layout), Some(list), Some(open)) =
            (self.layout(cx), self.list(), self.open.as_ref())
        else {
            return Plan::default();
        };
        plan(&layout, list, &open.marked, open.cursor, &|ix| {
            self.holds_all(ix, cx)
        })
    }

    /// Enter: add to the marked playlists, or the one under the cursor, or create; then close
    /// (or stay, with Keep picker open after adding). Nothing to do leaves the panel as it is.
    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let plan = self.plan(cx);
        self.run(plan, window, cx);
    }

    /// Ctrl+Enter: create the typed name, whatever row the cursor is on.
    pub fn create_typed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let create = self.layout(cx).and_then(|l| l.create);
        if create.is_some() {
            self.run(
                Plan {
                    add: Vec::new(),
                    create,
                },
                window,
                cx,
            );
        }
    }

    /// Add the songs to the playlist at `index` of the loaded list at once (a click on its row).
    /// A playlist that already holds them does nothing.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.holds_all(index, cx) {
            return;
        }
        self.run(
            Plan {
                add: vec![index],
                create: None,
            },
            window,
            cx,
        );
    }

    /// Make a playlist named `name` holding the songs (a click on the Create row).
    pub fn create(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.run(
            Plan {
                add: Vec::new(),
                create: Some(name),
            },
            window,
            cx,
        );
    }

    fn run(&mut self, plan: Plan, window: &mut Window, cx: &mut Context<Self>) {
        if plan.is_empty() {
            return;
        }
        if self.open.as_ref().is_some_and(|o| o.target_mode) {
            self.run_target(plan, window, cx);
            return;
        }
        let (Some(list), Some(open)) = (self.list(), self.open.as_ref()) else {
            return;
        };
        let videos: Vec<String> = open.songs.iter().map(|s| s.video_id.clone()).collect();
        let targets: Vec<PlaylistTarget> =
            plan.add.iter().filter_map(|&ix| list.get(ix).cloned()).collect();
        for target in &targets {
            // Several songs go as one request; one song as the usual single add.
            self.writes.update(cx, |w, cx| match videos.as_slice() {
                [video] => w.add_to_playlist(target, video, cx),
                _ => w.add_many_to_playlist(target, &videos, cx),
            });
        }
        let ids: Vec<String> = targets.iter().map(|t| t.id.clone()).collect();
        self.remember(&ids, cx);
        if let Some(name) = plan.create {
            self.awaiting_create = Some(name.clone());
            self.awaiting_filed = videos.clone();
            self.edits
                .update(cx, |e, cx| e.create_songs(Edit::Create, name, videos, cx));
        }
        if self.stay_open(cx) {
            // Ready for the next playlist: marks and filter cleared, the checks stay.
            if let Some(open) = self.open.as_mut() {
                open.marked.clear();
                open.cursor = 0;
            }
            self.field.update(cx, |f, cx| f.set_text("", cx));
            text_field::focus(&self.field, window, cx);
            cx.notify();
        } else {
            self.close(window, cx);
        }
    }

    /// Choosing the target: the playlist picked (or made) becomes the target, the songs the panel
    /// was opened with go into it, and the panel closes.
    fn run_target(&mut self, plan: Plan, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let songs = open.songs.clone();
        if let Some(name) = plan.create {
            // The playlist becomes the target once it exists (see `edited`).
            self.awaiting_create = Some(name.clone());
            self.awaiting_target = true;
            if songs.is_empty() {
                self.edits.update(cx, |e, cx| e.create(name, None, cx));
            } else {
                let videos = songs.iter().map(|s| s.video_id.clone()).collect();
                self.edits
                    .update(cx, |e, cx| e.create_songs(Edit::Create, name, videos, cx));
            }
            self.close(window, cx);
            return;
        }
        let Some(chosen) = plan
            .add
            .first()
            .and_then(|&ix| self.list().and_then(|list| list.get(ix)))
            .cloned()
        else {
            return;
        };
        self.remember(std::slice::from_ref(&chosen.id), cx);
        if let Some(handle) = cx.try_global::<TargetHandle>().map(|h| h.0.clone()) {
            handle.update(cx, |m, cx| {
                m.set(&chosen.id, &chosen.title, cx);
                m.add(&songs, cx);
            });
        }
        self.close(window, cx);
    }

    /// Clear: there is no target any more.
    pub fn clear_target(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open.as_ref().is_some_and(|o| o.target_mode) {
            return;
        }
        if let Some(handle) = cx.try_global::<TargetHandle>().map(|h| h.0.clone()) {
            handle.update(cx, |m, cx| m.clear(cx));
        }
        self.close(window, cx);
    }

    /// The playlist chosen as the target now: what the list marks with a check.
    pub fn current_target(&self, cx: &App) -> Option<Target> {
        cx.try_global::<TargetHandle>()
            .and_then(|h| h.0.read(cx).target(cx))
    }

    /// Whether the panel is open to choose the target.
    pub fn is_choosing_target(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.target_mode)
    }

    /// A playlist this picker asked for now exists: it is on top of the recents, and in the list
    /// if the panel is still open.
    fn edited(&mut self, event: &EditEvent, cx: &mut Context<Self>) {
        match event {
            EditEvent::Created(collection)
                if self.awaiting_create.as_deref() == Some(collection.title.as_str()) =>
            {
                self.awaiting_create = None;
                let filed = std::mem::take(&mut self.awaiting_filed);
                if let (false, Some(model)) = (filed.is_empty(), crate_list::handle(cx)) {
                    model.update(cx, |m, cx| m.filed(&filed, cx));
                }
                let id = bare_playlist_id(collection.id.as_str()).to_owned();
                self.remember(std::slice::from_ref(&id), cx);
                if std::mem::take(&mut self.awaiting_target) {
                    if let Some(handle) = cx.try_global::<TargetHandle>().map(|h| h.0.clone()) {
                        handle.update(cx, |m, cx| m.set(&id, &collection.title, cx));
                    }
                }
                if let Some(open) = self.open.as_mut() {
                    if let Load::Ready(list) = &mut open.playlists {
                        list.insert(
                            0,
                            PlaylistTarget {
                                id: id.clone(),
                                title: collection.title.clone(),
                                thumbnail: None,
                                holds_track: true,
                            },
                        );
                        open.created.push(id);
                    }
                }
                cx.notify();
            }
            EditEvent::Failed(Edit::Create) => {
                self.awaiting_create = None;
                self.awaiting_filed.clear();
                self.awaiting_target = false;
            }
            _ => {}
        }
    }
}

impl Focusable for PlaylistPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Keyboard focus sits in the filter box.
        self.field.read(cx).focus_handle(cx)
    }
}

/// What the panel says when the list could not be loaded.
pub const LOAD_FAILED: &str = "Couldn't load playlists";
pub const NO_PLAYLISTS: &str = "No playlists yet";
/// What the panel says when the filter leaves nothing and no name can be made of it.
pub const NO_MATCHES: &str = "No matches";

/// The Create row's label for a typed name.
pub fn create_label(name: &str) -> String {
    format!("Create \u{201C}{name}\u{201D}")
}

/// What one playlist row shows.
struct RowView {
    ix: usize,
    title: SharedString,
    /// Already in the playlist: a dim check, nothing to do.
    held: bool,
    /// Marked for Enter: a check in the accent colour.
    marked: bool,
    /// The target playlist (what A adds to): a star in the accent colour.
    target: bool,
    cursor: bool,
}

impl Render for PlaylistPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let (accent, _) = self.player.read(cx).accent_now();
        let ring = keyboard_focused(&self.field.read(cx).focus_handle(cx), window);
        let target_mode = open.target_mode;
        // The playlist that is the target now carries a star.
        let current_target = self.current_target(cx);

        let body: AnyElement = match &open.playlists {
            Load::Idle | Load::Loading => skeleton_rows(!cx.reduce_motion()),
            Load::Failed(_) => {
                let entity = cx.entity();
                let failed = message(
                    LOAD_FAILED,
                    Some(
                        TextButton::new("picker-retry", &self.retry_focus, "Retry", accent)
                            .strong()
                            .on_click(move |_, cx| entity.update(cx, |m, cx| m.load(cx))),
                    ),
                    cx,
                );
                // A new playlist can still be made from what is typed.
                let create = self.layout(cx).and_then(|rows| rows.create).map(|name| {
                    create_row(name, open.cursor == 0, cx)
                });
                div()
                    .flex()
                    .flex_col()
                    .child(failed)
                    .children(create)
                    .into_any_element()
            }
            Load::Ready(list) => {
                let rows = self.layout(cx).unwrap_or_default();
                if list.is_empty() && rows.is_empty() {
                    message(NO_PLAYLISTS, None, cx)
                } else if rows.is_empty() {
                    message(NO_MATCHES, None, cx)
                } else {
                    let mut children: Vec<AnyElement> = Vec::new();
                    for n in 0..rows.len() {
                        let cursor = n == open.cursor;
                        match rows.row(n) {
                            Some(Row::Playlist(ix)) => {
                                if n == rows.recent.len() && rows.divider_after_recent() {
                                    children.push(divider());
                                }
                                let target = &list[ix];
                                children.push(playlist_row(
                                    RowView {
                                        ix,
                                        title: target.title.clone().into(),
                                        held: self.holds_all(ix, cx),
                                        marked: open.marked.contains(&target.id),
                                        target: current_target
                                            .as_ref()
                                            .is_some_and(|t| t.id == target.id),
                                        cursor,
                                    },
                                    accent,
                                    cx,
                                ));
                            }
                            Some(Row::Create(name)) => {
                                if rows.divider_before_create() {
                                    children.push(divider());
                                }
                                children.push(create_row(name, cursor, cx));
                            }
                            None => {}
                        }
                    }
                    div()
                        .id("picker-list")
                        .role(gpui::Role::ListBox)
                        .flex()
                        .flex_col()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .children(children)
                        .into_any_element()
                }
            }
        };

        let caption = if target_mode {
            let clear = current_target.is_some().then(|| {
                let entity = cx.entity();
                div().debug_selector(|| "picker-clear-target".into()).child(
                    TextButton::new("picker-clear-target", &self.clear_focus, "Clear", accent)
                        .on_click(move |window, cx| {
                            entity.update(cx, |m, cx| m.clear_target(window, cx))
                        }),
                )
            });
            Some(
                style::text_style(div(), typography::CAPTION, cx)
                    .id("picker-caption")
                    .debug_selector(|| "picker-caption".into())
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .px(px(spacing::S3))
                    .pb(px(spacing::S1))
                    .text_color(color::text_secondary())
                    .child("Target playlist")
                    .children(clear),
            )
        } else {
            (open.songs.len() > 1).then(|| {
                style::text_style(div(), typography::CAPTION, cx)
                    .id("picker-caption")
                    .debug_selector(|| "picker-caption".into())
                    .flex_none()
                    .px(px(spacing::S3))
                    .pb(px(spacing::S1))
                    .text_color(color::text_secondary())
                    .child(format!("{} songs", open.songs.len()))
            })
        };

        let panel = div()
            .id("playlist-picker")
            .debug_selector(|| "playlist-picker".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Dialog)
            .aria_label(if target_mode {
                "Choose target playlist"
            } else {
                "Add to playlist"
            })
            .flex()
            .flex_col()
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .max_h(px(PANEL_MAX_HEIGHT))
            .p(px(spacing::S1))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the panel are not clicks on the page behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|m, _: &PickerUp, _, cx| m.move_cursor(-1, cx)))
            .on_action(cx.listener(|m, _: &PickerDown, _, cx| m.move_cursor(1, cx)))
            .on_action(cx.listener(|m, _: &PickerMark, _, cx| m.mark_cursor(cx)))
            .on_action(cx.listener(|m, _: &PickerCreate, window, cx| m.create_typed(window, cx)))
            .on_action(cx.listener(|m, _: &PickerClose, window, cx| m.close(window, cx)))
            // The plain filter box passes Escape on to here.
            .on_action(cx.listener(|m, _: &text_field::Escape, window, cx| m.close(window, cx)))
            .on_action(cx.listener(|_, _: &PickerStay, _, _| {}))
            .children(caption)
            .child(
                div()
                    .debug_selector(|| "picker-filter".into())
                    .relative()
                    .flex_none()
                    .mb(px(spacing::S1))
                    .child(self.field.clone())
                    .when(ring, |d| d.child(focus_ring_inset(accent, radius::SMALL))),
            )
            .child(body);

        // A transparent layer under the popover catches the click that closes it (no scrim);
        // the title bar stays out of reach so the window controls keep working.
        let layer = div()
            .id("playlist-picker-layer")
            .debug_selector(|| "playlist-picker-layer".into())
            .absolute()
            .top(px(layout::TITLE_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|m, _, window, cx| m.close(window, cx)),
            );
        match open.anchor {
            Some(anchor) => {
                let at = popover_position(anchor, window.viewport_size());
                let mut placed = div().absolute().left(px(at.left));
                placed = match at.vertical {
                    Vertical::Top(y) => placed.top(px(y - layout::TITLE_BAR_HEIGHT)),
                    Vertical::Bottom(y) => placed.bottom(px(y)),
                };
                layer.child(placed.child(panel.max_h(px(at.max_height))))
            }
            None => layer
                .flex()
                .items_center()
                .justify_center()
                .p(px(spacing::S4))
                .child(panel),
        }
            .into_any_element()
    }
}

/// Where the popover goes, in window coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    pub left: f32,
    pub vertical: Vertical,
    /// The tallest the popover may be here: its usual maximum, or the room that is left on the
    /// side it opens to (the list scrolls inside).
    pub max_height: f32,
}

/// Either the popover's top edge, or its bottom edge measured up from the window's bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vertical {
    Top(f32),
    Bottom(f32),
}

/// Below the anchor with the right edges aligned (the actions sit at a row's right), above it
/// when the room below is too short for the tallest popover and the room above is longer, and
/// always inside the window: where neither side fits it, the popover shrinks to the roomier one.
pub fn popover_position(anchor: Bounds<Pixels>, window: gpui::Size<Pixels>) -> Position {
    let (window_w, window_h) = (f32::from(window.width), f32::from(window.height));
    let right = f32::from(anchor.origin.x + anchor.size.width);
    let top = f32::from(anchor.origin.y);
    let bottom = f32::from(anchor.origin.y + anchor.size.height);
    let max_left = (window_w - PANEL_WIDTH - EDGE_MARGIN).max(EDGE_MARGIN);
    let left = (right - PANEL_WIDTH).clamp(EDGE_MARGIN, max_left);
    let room_below = window_h - bottom - ANCHOR_GAP - EDGE_MARGIN;
    // The title bar stays clear, so the room above starts under it.
    let room_above = top - layout::TITLE_BAR_HEIGHT - ANCHOR_GAP - EDGE_MARGIN;
    let (vertical, room) = if room_below >= PANEL_MAX_HEIGHT || room_below >= room_above {
        (Vertical::Top(bottom + ANCHOR_GAP), room_below)
    } else {
        (Vertical::Bottom(window_h - top + ANCHOR_GAP), room_above)
    };
    Position {
        left,
        vertical,
        // Never so short that the header and a row or two do not fit.
        max_height: room.min(PANEL_MAX_HEIGHT).max(MIN_HEIGHT),
    }
}

fn divider() -> AnyElement {
    div()
        .flex_none()
        .h(px(1.0))
        .my(px(spacing::S1))
        .bg(color::border_subtle())
        .into_any_element()
}

fn playlist_row(
    view: RowView,
    accent: Accent,
    cx: &mut Context<PlaylistPicker>,
) -> AnyElement {
    let RowView {
        ix,
        title,
        held,
        marked,
        target,
        cursor,
    } = view;
    let title_color = if held {
        color::text_secondary()
    } else {
        color::text_primary()
    };
    div()
        .id(("picker-row", ix))
        .debug_selector(move || format!("picker-row-{ix}"))
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ROW_HEIGHT))
        .px(px(spacing::S3))
        .rounded(px(radius::SMALL))
        .role(gpui::Role::ListBoxOption)
        .aria_label(title.clone())
        .aria_selected(cursor)
        .when(!held, |d| d.cursor_pointer())
        .bg(if cursor {
            color::state_selected()
        } else {
            gpui::transparent_black()
        })
        .hover(|s| s.bg(color::state_hover()))
        .on_click(cx.listener(move |m, _: &gpui::ClickEvent, window, cx| {
            m.choose(ix, window, cx);
        }))
        .child(
            style::text_style(div(), typography::BODY, cx)
                .id(("picker-title", ix))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(title_color)
                .child(title),
        )
        .when(target, |d| {
            d.child(
                gpui::svg()
                    .path(Icon::StarFilled.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(color::accent(accent)),
            )
        })
        .when(held || marked, |d| {
            d.child(
                gpui::svg()
                    .path(Icon::Checkmark.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(if marked {
                        color::accent(accent)
                    } else {
                        color::text_secondary()
                    }),
            )
        })
        .into_any_element()
}

/// The last row: make a playlist of what was typed, with the songs in it.
fn create_row(name: String, cursor: bool, cx: &mut Context<PlaylistPicker>) -> AnyElement {
    let label = create_label(&name);
    div()
        .id("picker-create")
        .debug_selector(|| "picker-create".into())
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ROW_HEIGHT))
        .px(px(spacing::S3))
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .bg(if cursor {
            color::state_selected()
        } else {
            gpui::transparent_black()
        })
        .hover(|s| s.bg(color::state_hover()))
        .on_click(cx.listener(move |m, _: &gpui::ClickEvent, window, cx| {
            m.create(name.clone(), window, cx)
        }))
        .child(
            gpui::svg()
                .path(Icon::Add.path())
                .size(px(layout::ICON_SIZE))
                .flex_none()
                .text_color(color::text_secondary()),
        )
        .child(
            style::text_style(div(), typography::BODY, cx)
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(color::text_primary())
                .child(label),
        )
        .into_any_element()
}

/// The filter box's focus ring, drawn inside it.
fn focus_ring_inset(accent: Accent, corner: f32) -> impl IntoElement {
    div()
        .absolute()
        .size_full()
        .rounded(px(corner))
        .border(px(layout::FOCUS_RING_WIDTH))
        .border_color(color::accent(accent))
}

fn skeleton_rows(animate: bool) -> AnyElement {
    let column = div()
        .flex()
        .flex_col()
        .children((0..SKELETON_ROWS).map(|ix| {
            div()
                .flex()
                .items_center()
                .h(px(ROW_HEIGHT))
                .px(px(spacing::S3))
                .child(
                    div()
                        .h(px(12.0))
                        .w(gpui::relative(if ix % 2 == 0 { 0.6 } else { 0.4 }))
                        .rounded(px(radius::SMALL))
                        .bg(color::skeleton()),
                )
        }));
    skeleton::pulse(column, "picker-skeleton", animate)
}

fn message(text: &'static str, action: Option<TextButton>, cx: &App) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ROW_HEIGHT))
        .px(px(spacing::S3))
        .child(
            style::text_style(div(), typography::BODY, cx)
                .text_color(color::text_secondary())
                .child(text),
        )
        .children(action)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, size};

    fn target(id: &str, title: &str, holds: bool) -> PlaylistTarget {
        PlaylistTarget {
            id: id.into(),
            title: title.into(),
            thumbnail: None,
            holds_track: holds,
        }
    }

    fn sample() -> Vec<PlaylistTarget> {
        vec![
            target("PLa", "Road Trip", false),
            target("PLb", "Gym Mix", false),
            target("PLc", "Focus", false),
            target("PLd", "Late Night Gym", false),
        ]
    }

    fn strings(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn with_no_recents_and_no_query_every_playlist_is_listed_in_the_services_order() {
        let at = arrange(&sample(), &[], "");
        assert_eq!(at.recent, Vec::<usize>::new());
        assert_eq!(at.rest, vec![0, 1, 2, 3]);
        assert_eq!(at.create, None);
        assert_eq!(at.len(), 4);
    }

    #[test]
    fn recents_come_first_newest_first_and_leave_the_rest_in_place() {
        let at = arrange(&sample(), &strings(&["PLc", "PLa", "PLgone"]), "");
        assert_eq!(at.recent, vec![2, 0], "an id the list lacks is ignored");
        assert_eq!(at.rest, vec![1, 3]);
        assert_eq!(at.row(0), Some(Row::Playlist(2)));
        assert_eq!(at.row(2), Some(Row::Playlist(1)));
        assert_eq!(at.row(4), None);
    }

    #[test]
    fn typing_keeps_playlists_with_every_word_in_any_case_in_both_groups() {
        let at = arrange(&sample(), &strings(&["PLd", "PLc"]), "  gYM ");
        assert_eq!(at.recent, vec![3]);
        assert_eq!(at.rest, vec![1]);
        let at = arrange(&sample(), &[], "night gym");
        assert_eq!(at.rest, vec![3]);
    }

    #[test]
    fn a_typed_name_that_is_no_playlist_offers_to_create_it_last() {
        let at = arrange(&sample(), &[], "  Gym   Rats ");
        assert_eq!(at.rest, Vec::<usize>::new());
        assert_eq!(at.create.as_deref(), Some("Gym Rats"), "cleaned like any playlist name");
        assert_eq!(at.row(0), Some(Row::Create("Gym Rats".into())));
        let at = arrange(&sample(), &[], "gym");
        assert_eq!(at.rest, vec![1, 3]);
        assert_eq!(at.row(2), Some(Row::Create("gym".into())), "after the matches");
    }

    #[test]
    fn a_name_that_is_a_playlist_already_does_not_offer_to_create_it() {
        let at = arrange(&sample(), &[], "gym mix");
        assert_eq!(at.create, None);
        assert_eq!(at.rest, vec![1]);
        assert_eq!(arrange(&sample(), &[], "   ").create, None, "nothing typed, nothing to make");
    }

    #[test]
    fn dividers_sit_between_the_groups_and_scrolling_counts_them() {
        let at = arrange(&sample(), &strings(&["PLa"]), "");
        assert!(at.divider_after_recent());
        assert_eq!(at.child_index(0), 0);
        assert_eq!(at.child_index(1), 2, "the divider is child 1");
        let at = arrange(&sample(), &strings(&["PLa"]), "zzz");
        assert!(!at.divider_after_recent() && !at.divider_before_create());
        assert_eq!(at.child_index(0), 0);
        let at = arrange(&sample(), &strings(&["PLa"]), "road");
        assert!(at.divider_before_create());
        assert_eq!(at.child_index(1), 2);
    }

    #[test]
    fn enter_adds_the_row_under_the_cursor_when_nothing_is_marked() {
        let list = sample();
        let at = arrange(&list, &[], "");
        let none = |_: usize| false;
        assert_eq!(plan(&at, &list, &[], 2, &none), Plan { add: vec![2], create: None });
        assert!(plan(&at, &list, &[], 2, &|ix| ix == 2).is_empty(), "already held");
        assert!(plan(&at, &list, &[], 9, &none).is_empty(), "no such row");
    }

    #[test]
    fn enter_on_the_create_row_creates_the_typed_name() {
        let list = sample();
        let at = arrange(&list, &[], "Brand New");
        let p = plan(&at, &list, &[], 0, &|_| false);
        assert_eq!(p, Plan { add: vec![], create: Some("Brand New".into()) });
    }

    #[test]
    fn enter_adds_to_the_marked_playlists_in_the_order_marked_and_ignores_the_cursor_row() {
        let list = sample();
        let at = arrange(&list, &[], "");
        let marked = strings(&["PLd", "PLb"]);
        let p = plan(&at, &list, &marked, 0, &|_| false);
        assert_eq!(p, Plan { add: vec![3, 1], create: None });
        let p = plan(&at, &list, &marked, 0, &|ix| ix == 3);
        assert_eq!(p.add, vec![1], "a playlist that holds the songs is not added to again");
        // Marks and the Create row together do both.
        let at = arrange(&list, &[], "Fresh");
        let p = plan(&at, &list, &marked, 0, &|_| false);
        assert_eq!(p, Plan { add: vec![3, 1], create: Some("Fresh".into()) });
    }

    #[test]
    fn recents_are_parsed_and_kept_short_with_the_newest_first() {
        assert_eq!(parse_recents(" PLa,,PLb , "), strings(&["PLa", "PLb"]));
        assert!(parse_recents("").is_empty());
        let mut r = strings(&["PLa", "PLb", "PLc"]);
        push_recent(&mut r, "PLb");
        assert_eq!(r, strings(&["PLb", "PLa", "PLc"]));
        for n in 0..20 {
            push_recent(&mut r, &format!("PLx{n}"));
        }
        assert_eq!(r.len(), RECENTS_MAX);
        assert_eq!(r[0], "PLx19");
    }

    #[test]
    fn the_create_row_names_what_will_be_made() {
        assert_eq!(create_label("gym"), "Create \u{201C}gym\u{201D}");
    }

    fn anchor(x: f32, y: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(32.0), px(32.0)))
    }

    #[test]
    fn it_opens_below_the_anchor_with_right_edges_aligned() {
        let at = popover_position(anchor(900.0, 200.0), size(px(1200.0), px(800.0)));
        assert_eq!(at.left, 932.0 - PANEL_WIDTH);
        assert_eq!(at.vertical, Vertical::Top(200.0 + 32.0 + ANCHOR_GAP));
    }

    #[test]
    fn it_opens_above_when_the_room_below_is_short() {
        let at = popover_position(anchor(900.0, 700.0), size(px(1200.0), px(800.0)));
        assert_eq!(at.vertical, Vertical::Bottom(800.0 - 700.0 + ANCHOR_GAP));
    }

    #[test]
    fn it_stays_inside_the_window_sideways() {
        let at = popover_position(anchor(10.0, 100.0), size(px(1200.0), px(800.0)));
        assert_eq!(at.left, EDGE_MARGIN);
        let at = popover_position(anchor(1190.0, 100.0), size(px(1200.0), px(800.0)));
        assert_eq!(at.left, 1200.0 - PANEL_WIDTH - EDGE_MARGIN);
    }

    #[test]
    fn a_short_window_takes_the_roomier_side_and_shrinks_to_fit_it() {
        // 480 high, anchor in the upper part: more room below than above, but not 320px of it.
        let window = size(px(864.0), px(480.0));
        let at = popover_position(anchor(500.0, 200.0), window);
        assert!(matches!(at.vertical, Vertical::Top(_)));
        let room_below = 480.0 - 232.0 - ANCHOR_GAP - EDGE_MARGIN;
        assert_eq!(at.max_height, room_below);
        // A row lower down has more room above, so it opens upward and fits there.
        let at = popover_position(anchor(500.0, 297.0), window);
        assert!(matches!(at.vertical, Vertical::Bottom(_)));
        assert!(at.max_height <= PANEL_MAX_HEIGHT);
        assert!(297.0 - at.max_height - ANCHOR_GAP >= layout::TITLE_BAR_HEIGHT);
        // Whatever the window, the popover is never taller than its usual maximum.
        let tall = popover_position(anchor(500.0, 200.0), size(px(1200.0), px(1200.0)));
        assert_eq!(tall.max_height, PANEL_MAX_HEIGHT);
    }
}
