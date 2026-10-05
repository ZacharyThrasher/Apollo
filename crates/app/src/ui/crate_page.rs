//! The Crate page: the songs set aside with C or "Add to Crate", top to bottom in the order the
//! owner arranged them (a new song lands at the bottom), in the same song rows as the other
//! lists. A header row plays, shuffles, adds the whole crate to the queue, saves it as a new
//! playlist, adds it to an existing playlist, or empties it (Clear asks first). Rows play from
//! there with the crate as the queue; More opens the usual song menu (add to the queue or a
//! playlist, start a radio, remove from the crate).
//!
//! Arranging is local and free: a row drags to a new place (a thin line shows where it
//! lands; a footer after the last row takes a drop at the end), or Alt+Up / Alt+Down moves the
//! row under the cursor one place. Save as playlist and Add all follow the order shown.
//!
//! The list is virtual and keyboard-first like the queue panel's: one tab stop, an arrow-key
//! cursor, Enter plays, L likes, Delete removes.
//!
//! Songs can be picked for bulk verbs (`ui::selection`: Ctrl+click, Shift+click, Shift+Arrows,
//! Ctrl+A, Esc). With 2 or more picked, L likes them all, Delete takes them out of the Crate, and
//! the menu of a picked row (right-click or More) acts on every picked row, with Remove from
//! Crate. A drag still moves the one row it is started on.

use std::ops::Range;
use std::rc::Rc;

use apollo_store::CrateRow;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, uniform_list, AnyElement, App, ClickEvent, Context, Entity,
    FocusHandle, Focusable, IntoElement, KeyBinding, KeyboardButton, ParentElement, Render,
    ScrollStrategy, SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::assets::Icon;
use crate::crate_list::CrateModel;
use crate::library::LibraryWrites;
use crate::ui::anchor;
use crate::ui::playlist_dialog::{self, Mode, CRATE_PLAYLIST_NAME};
use crate::ui::playlist_picker::PlaylistPicker;
use crate::player::{format_time, PlayerModel};
use crate::style::{self, color};
use crate::ui::art_tile::art_tile;
use crate::ui::button::{focus_ring, keyboard_focused, Handler};
use crate::ui::player_bar::text_line;
use crate::ui::playing_row;
use crate::ui::row_actions::{heart_action, more_slot, row_action, ACTION_SIZE};
use crate::ui::selection::{self, Click, Selection};
use crate::ui::text_button::TextButton;
use crate::ui::track_menu::{self, Command, Subject, TrackSubject};

actions!(
    apollo_crate,
    [
        CursorUp,
        CursorDown,
        PlayCursor,
        RemoveCursor,
        LikeCursor,
        PlaylistCursor,
        MoveCursorUp,
        MoveCursorDown,
        ExtendUp,
        ExtendDown,
        SelectAll,
        ClearPicks
    ]
);

/// Key context of the list's focus handle.
pub const KEY_CONTEXT: &str = "CrateList";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", CursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", CursorDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", PlayCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", RemoveCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("backspace", RemoveCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("l", LikeCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("p", PlaylistCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-up", MoveCursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-down", MoveCursorDown, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-up", ExtendUp, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", ClearPicks, Some(KEY_CONTEXT)),
    ]);
}

/// Same 56px rows as every other song list, with the focus ring's room around the content.
const ROW_HEIGHT: f32 = 56.0;
const ROW_INSET: f32 = spacing::S1;
const ART: f32 = 40.0;
const DURATION_WIDTH: f32 = spacing::S12;
/// The insertion line a dragged row shows where it would land.
const DROP_LINE: f32 = 2.0;
/// The ghost that follows the pointer while a row is dragged.
const DRAG_GHOST_WIDTH: f32 = 360.0;

/// Tab order: after the nav rail (1-5), in the slots the Library page uses.
const TAB_PLAY: isize = 6;
const TAB_SHUFFLE: isize = 7;
const TAB_ADD_ALL: isize = 8;
const TAB_SAVE: isize = 9;
const TAB_ADD_TO: isize = 10;
const TAB_CLEAR: isize = 11;
const TAB_LIST: isize = 12;
const TAB_SEARCH: isize = 13;

/// Where the "Add all to…" popover hangs from.
const ADD_TO_ANCHOR: &str = "crate-add-to-playlist";

struct Focus {
    play: FocusHandle,
    shuffle: FocusHandle,
    add_all: FocusHandle,
    save: FocusHandle,
    add_to: FocusHandle,
    clear: FocusHandle,
    confirm: FocusHandle,
    cancel: FocusHandle,
    list: FocusHandle,
    search: FocusHandle,
}

/// Where the cursor lands after a removal: the next row, else the previous one.
fn cursor_after_removal(ids: &[String], removed: &str) -> Option<String> {
    let pos = ids.iter().position(|id| id == removed)?;
    ids.get(pos + 1)
        .or_else(|| pos.checked_sub(1).and_then(|p| ids.get(p)))
        .cloned()
}

/// The song row being dragged to a new place in the crate.
#[derive(Clone)]
struct DraggedRow {
    from: usize,
    title: SharedString,
    artist: SharedString,
}

struct DragGhost(DraggedRow);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // On bg.overlay, so the lowest text tier is text.secondary.
        div()
            .w(px(DRAG_GHOST_WIDTH))
            .h(px(ROW_HEIGHT - 2.0 * ROW_INSET))
            .px(px(spacing::S3))
            .flex()
            .flex_col()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_strong())
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .truncate()
                    .text_color(color::text_primary())
                    .child(self.0.title.clone()),
            )
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .truncate()
                    .text_color(color::text_secondary())
                    .child(self.0.artist.clone()),
            )
    }
}

/// The line (text.secondary, like the queue's and the playlist page's) a dragged row shows above the row it would land before.
fn drop_line(group: SharedString) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left(px(ROW_INSET))
        .right(px(ROW_INSET))
        .h(px(DROP_LINE))
        .rounded_full()
        .bg(color::text_secondary())
        .opacity(0.0)
        .group_drag_over::<DraggedRow>(group, |s| s.opacity(1.0))
}

/// A header button wrapped so tests (and the layout) can find it by name.
fn named(name: &'static str, button: impl IntoElement) -> gpui::Div {
    div().debug_selector(move || name.into()).child(button)
}

/// "1 song" / "12 songs".
fn count_text(n: usize) -> SharedString {
    if n == 1 {
        "1 song".into()
    } else {
        format!("{n} songs").into()
    }
}

pub struct CratePage {
    crate_: Entity<CrateModel>,
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    picker: Entity<PlaylistPicker>,
    focus: Focus,
    scroll: UniformListScrollHandle,
    /// The row the keyboard acts on (by video id), shown only while the list has focus.
    cursor: Option<String>,
    /// Clear was pressed and waits for a yes.
    confirming: bool,
    /// Songs picked for bulk verbs, by video id.
    selection: Selection<String>,
    /// What a picked row's More button and right-click open: the menu of the whole selection.
    bulk_menu: Option<Subject>,
    /// Copied at each render for the list's row callback.
    rows: Vec<CrateRow>,
    accent: Accent,
    on_search: Rc<Handler>,
}

impl CratePage {
    pub fn new(
        crate_: Entity<CrateModel>,
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        on_search: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        cx.observe(&crate_, |this, _, cx| {
            if this.crate_.read(cx).is_empty() {
                this.confirming = false;
            }
            cx.notify();
        })
        .detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        let focus = Focus {
            play: stop(cx, TAB_PLAY),
            shuffle: stop(cx, TAB_SHUFFLE),
            add_all: stop(cx, TAB_ADD_ALL),
            save: stop(cx, TAB_SAVE),
            add_to: stop(cx, TAB_ADD_TO),
            clear: stop(cx, TAB_CLEAR),
            // The confirmation takes the Clear button's place in the tab order.
            confirm: stop(cx, TAB_CLEAR),
            cancel: stop(cx, TAB_CLEAR),
            list: stop(cx, TAB_LIST),
            search: stop(cx, TAB_SEARCH),
        };
        Self {
            crate_,
            player,
            writes,
            picker,
            focus,
            scroll: UniformListScrollHandle::new(),
            cursor: None,
            confirming: false,
            selection: Selection::new(),
            bulk_menu: None,
            rows: Vec::new(),
            accent: Accent::fallback(),
            on_search: Rc::new(Box::new(on_search)),
        }
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    pub fn confirming(&self) -> bool {
        self.confirming
    }

    /// How many songs are picked for bulk verbs (tests and diagnostics).
    pub fn picked(&self) -> usize {
        self.selection.len()
    }

    /// The page was left: a half-answered Clear is forgotten.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        self.cursor = None;
        self.selection.clear();
        cx.notify();
    }

    // ---- commands ----------------------------------------------------------------------

    /// Hand the player the covers the crate knows, so queued songs have art.
    fn hint_covers(&self, cx: &mut Context<Self>) {
        let rows: Vec<(String, Option<String>)> = self
            .crate_
            .read(cx)
            .items()
            .iter()
            .map(|r| (r.track.video_id.as_str().to_owned(), r.thumbnail.clone()))
            .collect();
        self.player.update(cx, |p, _| {
            for (video, thumbnail) in &rows {
                p.hint_art(video, thumbnail.as_deref());
            }
        });
    }

    /// Play the crate from row `start`; the rest of it is the queue.
    pub fn play_from(&mut self, start: usize, shuffle: Option<bool>, cx: &mut Context<Self>) {
        let tracks = self.crate_.read(cx).tracks();
        if tracks.is_empty() {
            return;
        }
        self.hint_covers(cx);
        let start = if shuffle == Some(true) { None } else { Some(start) };
        self.player
            .update(cx, |p, cx| p.play_tracks(tracks, start, shuffle, cx));
    }

    pub fn add_all_to_queue(&mut self, cx: &mut Context<Self>) {
        let tracks = self.crate_.read(cx).tracks();
        if tracks.is_empty() {
            return;
        }
        self.hint_covers(cx);
        self.player
            .update(cx, |p, cx| p.add_tracks_to_queue(tracks, cx));
    }

    /// The video ids of the whole crate, in the order shown.
    fn video_ids(&self, cx: &App) -> Vec<String> {
        self.ids(cx)
    }

    /// "Save as playlist…": the dialog opens with the whole crate as the playlist's songs.
    pub fn save_as_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let video_ids = self.video_ids(cx);
        if video_ids.is_empty() {
            return;
        }
        playlist_dialog::open(
            Mode::FromCrate {
                name: CRATE_PLAYLIST_NAME.to_owned(),
                video_ids,
            },
            window,
            cx,
        );
    }

    /// "Add all to…": the playlist picker opens to add the whole crate, in order, to one playlist.
    pub fn add_all_to_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let video_ids = self.video_ids(cx);
        if video_ids.is_empty() {
            return;
        }
        let title = count_text(video_ids.len());
        let at = anchor::bounds_of(ADD_TO_ANCHOR, cx);
        self.picker
            .update(cx, |p, cx| p.open_many(video_ids, title, at, window, cx));
    }

    /// Clear was pressed: ask first.
    pub fn ask_to_clear(&mut self, cx: &mut Context<Self>) {
        if !self.crate_.read(cx).is_empty() {
            self.confirming = true;
            cx.notify();
        }
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        self.cursor = None;
        self.selection.clear();
        self.crate_.update(cx, |m, cx| m.clear(cx));
        cx.notify();
    }

    pub fn cancel_clear(&mut self, cx: &mut Context<Self>) {
        self.confirming = false;
        cx.notify();
    }

    fn ids(&self, cx: &App) -> Vec<String> {
        self.crate_
            .read(cx)
            .items()
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect()
    }

    pub fn remove(&mut self, video: &str, cx: &mut Context<Self>) {
        if self.cursor.as_deref() == Some(video) {
            self.cursor = cursor_after_removal(&self.ids(cx), video);
        }
        self.crate_.update(cx, |m, cx| {
            m.remove(video, cx);
        });
        cx.notify();
    }

    // ---- keyboard ----------------------------------------------------------------------

    /// The cursor, or the row that is playing when the keyboard has not chosen one yet.
    fn cursor_or_playing(&self, ids: &[String], cx: &App) -> Option<String> {
        let playing = self.player.read(cx).current_video();
        self.cursor
            .clone()
            .filter(|c| ids.contains(c))
            .or_else(|| playing.filter(|p| ids.contains(p)))
            .or_else(|| ids.first().cloned())
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        if ids.is_empty() {
            return;
        }
        let at = self
            .cursor_or_playing(&ids, cx)
            .and_then(|c| ids.iter().position(|id| *id == c))
            .unwrap_or(0);
        // The first press only shows the cursor.
        let first = self.cursor.is_none();
        let to = if first {
            at
        } else {
            (at as isize + delta).clamp(0, ids.len() as isize - 1) as usize
        };
        self.cursor = Some(ids[to].clone());
        self.scroll.scroll_to_item(to, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn cursor_index(&self, cx: &App) -> Option<usize> {
        let cursor = self.cursor.as_deref()?;
        self.ids(cx).iter().position(|id| id == cursor)
    }

    fn play_cursor(&mut self, cx: &mut Context<Self>) {
        let Some(ix) = self.cursor_index(cx) else {
            return;
        };
        self.activate(ix, cx);
    }

    /// A row was chosen: play it with the crate as the queue, or pause it if it is the one playing.
    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        let playing = self.player.read(cx).current_video();
        let Some(video) = self
            .crate_
            .read(cx)
            .items()
            .get(ix)
            .map(|r| r.track.video_id.as_str().to_owned())
        else {
            return;
        };
        if playing.as_deref() == Some(video.as_str()) {
            self.player.update(cx, |m, cx| m.toggle_play(cx));
        } else {
            self.play_from(ix, None, cx);
        }
    }

    /// Alt+Up / Alt+Down: the row under the cursor moves one place; the cursor goes with it.
    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(video) = self.cursor.clone() else {
            return;
        };
        let moved = self
            .crate_
            .update(cx, |m, cx| m.nudge(&video, delta, cx));
        if moved {
            if let Some(ix) = self.cursor_index(cx) {
                self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            cx.notify();
        }
    }

    /// A dragged row was let go before slot `slot` (`len` is the end): the crate reorders. The
    /// row keeps the keyboard cursor with it.
    fn drop_row(&mut self, from: usize, slot: usize, cx: &mut Context<Self>) {
        let video = self
            .crate_
            .read(cx)
            .items()
            .get(from)
            .map(|r| r.track.video_id.as_str().to_owned());
        let moved = self.crate_.update(cx, |m, cx| m.move_row(from, slot, cx));
        if moved {
            if self.cursor.is_some() {
                self.cursor = video;
            }
            cx.notify();
        }
    }

    fn remove_cursor(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_bulk() {
            self.remove_picked(cx);
            return;
        }
        if let Some(video) = self.cursor.clone() {
            self.remove(&video, cx);
        }
    }

    fn like_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_bulk(Command::ToggleLike, window, cx) {
            return;
        }
        if let Some(video) = self.cursor.clone() {
            self.writes.update(cx, |w, cx| w.toggle_like(&video, cx));
        }
    }

    // ---- picking songs -----------------------------------------------------------------

    /// The picked songs in the crate's order, as the track menu takes them.
    fn picked_songs(&self) -> Vec<TrackSubject> {
        self.rows
            .iter()
            .filter(|r| self.selection.contains(&r.track.video_id.as_str().to_owned()))
            .map(|r| TrackSubject::of_row(r.track.clone(), r.thumbnail.clone(), None))
            .collect()
    }

    /// Per frame: drop picks that left the crate and work out the selection's menu.
    fn refresh_selection(&mut self) {
        let ids: Vec<String> = self
            .rows
            .iter()
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect();
        self.selection.keep(&ids);
        self.bulk_menu = if self.selection.is_bulk() {
            Subject::tracks(self.picked_songs())
        } else {
            None
        };
    }

    /// A click on row `ix`. Whether it was a Ctrl or Shift click (a pick), which the row's own
    /// action then leaves alone. A plain click clears the picks.
    fn pick_row(&mut self, ix: usize, how: Click, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let ids = self.ids(cx);
        let Some(video) = ids.get(ix) else {
            return false;
        };
        let picked = self
            .selection
            .click(how, video, self.cursor.as_ref(), &ids);
        if picked {
            // The keyboard carries on from the clicked row: Shift+Arrows, Ctrl+A and Esc work.
            self.cursor = Some(video.clone());
            window.focus(&self.focus.list, cx);
        }
        cx.notify();
        picked
    }

    /// Shift+Up / Shift+Down: the cursor moves and the picked rows stretch (or shrink) to it.
    fn extend(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        if ids.is_empty() {
            return;
        }
        let at = self
            .cursor_or_playing(&ids, cx)
            .and_then(|c| ids.iter().position(|id| *id == c))
            .unwrap_or(0);
        let to = (at as isize + delta).clamp(0, ids.len() as isize - 1) as usize;
        self.selection.extend(ids.get(at), &ids[to], &ids);
        self.cursor = Some(ids[to].clone());
        self.scroll.scroll_to_item(to, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Ctrl+A: every song.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        self.selection.select_all(&ids);
        cx.notify();
    }

    /// Esc: picked songs go first; with none picked Esc does what it did before.
    fn clear_picks(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            cx.propagate();
            return;
        }
        self.selection.clear();
        cx.notify();
    }

    /// Run a menu verb on the picked songs, when two or more are picked. Whether it did.
    fn run_bulk(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.selection.is_bulk() {
            return false;
        }
        if let Some(subject) = Subject::tracks(self.picked_songs()) {
            track_menu::run_now(command, subject, window, cx);
        }
        true
    }

    /// Delete with 2 or more picked: they all leave the crate, and the cursor lands on the row
    /// after them (else the one before).
    fn remove_picked(&mut self, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        let picked: Vec<String> = self.selection.members(&ids).into_iter().cloned().collect();
        if picked.is_empty() {
            return;
        }
        if let Some(at) = self.cursor.as_ref().and_then(|c| ids.iter().position(|i| i == c)) {
            let survives = |id: &&String| !picked.contains(*id);
            self.cursor = ids[at..]
                .iter()
                .find(survives)
                .or_else(|| ids[..at].iter().rev().find(survives))
                .cloned();
        }
        self.selection.clear();
        self.crate_.update(cx, |m, cx| {
            for video in &picked {
                m.remove(video, cx);
            }
        });
        cx.notify();
    }

    /// The P key: the playlist picker for the song under the cursor, or for every picked song.
    fn playlist_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::ui::playlist_picker::key_enabled(cx) {
            return;
        }
        let songs: Vec<crate::ui::playlist_picker::PickerSong> = if self.selection.is_bulk() {
            self.picked_songs()
                .into_iter()
                .map(|s| {
                    crate::ui::playlist_picker::PickerSong::new(
                        s.track.video_id.as_str(),
                        s.track.title.clone(),
                    )
                })
                .collect()
        } else {
            let Some(ix) = self.cursor_index(cx) else {
                return;
            };
            let Some(track) = self.crate_.read(cx).items().get(ix).map(|r| r.track.clone()) else {
                return;
            };
            vec![crate::ui::playlist_picker::PickerSong::new(
                track.video_id.as_str(),
                track.title.clone(),
            )]
        };
        crate::ui::playlist_picker::open_songs(songs, None, window, cx);
    }

    // ---- rows --------------------------------------------------------------------------

    fn rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Artwork loads only for what is on screen.
        let ids: Vec<String> = range
            .clone()
            .filter_map(|ix| self.rows.get(ix))
            .map(|r| r.track.video_id.as_str().to_owned())
            .collect();
        if !ids.is_empty() {
            self.player.update(cx, |m, cx| m.request_thumbs(ids, cx));
        }
        range.map(|ix| self.row(ix, window, cx)).collect()
    }

    /// The empty slot after the last row: a row dropped here goes to the bottom.
    fn end_slot(&self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let group: SharedString = format!("crate-drop-{ix}").into();
        div()
            .debug_selector(|| "crate-end".into())
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .group(group.clone())
            .child(drop_line(group))
            .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
                this.drop_row(dragged.from, ix, cx)
            }))
            .into_any_element()
    }

    fn row(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return self.end_slot(ix, cx);
        };
        let video: String = row.track.video_id.as_str().to_owned();
        let playing = self.player.read(cx).current_video().as_deref() == Some(video.as_str());
        let list_focused = self.focus.list.is_focused(window);
        let at_cursor = list_focused && self.cursor.as_deref() == Some(video.as_str());
        let ring = at_cursor && keyboard_focused(&self.focus.list, window);
        let accent = color::accent(self.accent);
        let tint = selection::row_tint(self.accent);
        let selected = playing || at_cursor;
        let group: SharedString = format!("crate-row-{ix}").into();
        let thumb = self.player.read(cx).thumb(&video);
        let liked = self.writes.read(cx).is_liked(&video);
        let title: SharedString = row.track.title.clone().into();
        let artists_line: SharedString = row
            .track
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
            .into();
        let artists = artists_line.clone();
        let duration: SharedString = row
            .track
            .duration
            .map(|d| format_time(d.as_millis() as u64))
            .unwrap_or_default()
            .into();

        // Room for every action is always reserved; they show on hover or under the cursor,
        // and the heart stays once the song is liked.
        let heart = div()
            .debug_selector(move || format!("crate-like-{ix}"))
            .flex_none()
            .size(px(ACTION_SIZE))
            .when(!(liked || at_cursor), |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .child(heart_action(("crate-like", ix), liked, {
                let (writes, video) = (self.writes.clone(), video.clone());
                move |_, cx| writes.update(cx, |w, cx| w.toggle_like(&video, cx))
            }));
        let picked = self.selection.contains(&video);
        // A picked row speaks for every picked row.
        let subject = match &self.bulk_menu {
            Some(bulk) if picked => bulk.clone(),
            _ => Subject::track(row.track.clone(), row.thumbnail.clone()),
        };
        let more = more_slot(format!("crate-more-{ix}"), true, subject.clone())
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            });
        let remove = div()
            .debug_selector(move || format!("crate-remove-{ix}"))
            .flex_none()
            .size(px(ACTION_SIZE))
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .child(row_action(
                ("crate-remove", ix),
                Icon::Dismiss,
                "Remove from Crate",
                color::text_secondary(),
                {
                    let (page, video) = (cx.entity(), video.clone());
                    move |window, cx| {
                        page.update(cx, |p, cx| {
                            window.focus(&p.focus.list, cx);
                            p.remove(&video, cx);
                        })
                    }
                },
            ));

        let surface = div()
            .id(("crate-row", ix))
            .debug_selector(move || format!("crate-row-{ix}"))
            .group(group.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .h(px(ROW_HEIGHT - 2.0 * ROW_INSET))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .when(selected, |d| d.bg(playing_row::tint()))
            .when(picked, |d| d.bg(tint))
            .hover(move |s| {
                s.bg(if picked {
                    tint
                } else if selected {
                    playing_row::tint()
                } else {
                    color::state_hover()
                })
            })
            .role(gpui::Role::ListItem)
            .aria_label(title.clone())
            .aria_selected(playing || picked)
            .on_click(cx.listener({
                let video = video.clone();
                move |this, event: &ClickEvent, window, cx| {
                    // Space is the global play/pause key.
                    if let ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    // Ctrl and Shift pick songs; a plain click clears the picks and plays.
                    if this.pick_row(ix, Click::of_event(event), window, cx) {
                        return;
                    }
                    window.focus(&this.focus.list, cx);
                    this.cursor = Some(video.clone());
                    this.activate(ix, cx);
                    cx.notify();
                }
            }))
            .when(playing, |d| d.child(playing_row::marker(accent)))
            .child(art_tile(ART, thumb))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text_line(
                        "title",
                        title.clone(),
                        playing_row::title_style(typography::LIST_TITLE, playing),
                        color::text_primary(),
                        cx,
                    ))
                    .child(text_line(
                        "artist",
                        artists,
                        typography::BODY,
                        color::text_secondary(),
                        cx,
                    )),
            )
            .child(heart)
            .child(
                div()
                    .w(px(DURATION_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(text_line(
                        "duration",
                        duration,
                        typography::CAPTION,
                        color::text_tertiary(),
                        cx,
                    )),
            )
            .child(more)
            .child(remove)
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)))
            .on_drag(
                DraggedRow {
                    from: ix,
                    title: title.clone(),
                    artist: artists_line.clone(),
                },
                |dragged, _, _, cx| cx.new(|_| DragGhost(dragged.clone())),
            );

        let surface = track_menu::on_context_menu(surface, subject);
        // Any row takes a dragged row's place: the line above it shows where it would land.
        let drop_group: SharedString = format!("crate-drop-{ix}").into();
        div()
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .py(px(ROW_INSET))
            .group(drop_group.clone())
            .child(drop_line(drop_group))
            .child(surface)
            .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
                this.drop_row(dragged.from, ix, cx)
            }))
            .into_any_element()
    }

    // ---- page --------------------------------------------------------------------------

    fn header(&self, count: usize, accent: Accent, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.entity();
        let has_rows = count > 0;
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .mt(px(spacing::S3))
            .child(named(
                "crate-play",
                TextButton::new("crate-play", &self.focus.play, "Play", accent)
                    .primary()
                    .enabled(has_rows)
                    .on_click({
                        let page = page.clone();
                        move |_, cx| page.update(cx, |p, cx| p.play_from(0, None, cx))
                    }),
            ))
            .child(named(
                "crate-shuffle",
                TextButton::new("crate-shuffle", &self.focus.shuffle, "Shuffle", accent)
                    .enabled(has_rows)
                    .on_click({
                        let page = page.clone();
                        move |_, cx| page.update(cx, |p, cx| p.play_from(0, Some(true), cx))
                    }),
            ))
            .child(named(
                "crate-add-all",
                TextButton::new(
                    "crate-add-all",
                    &self.focus.add_all,
                    "Add all to queue",
                    accent,
                )
                .enabled(has_rows)
                .on_click({
                    let page = page.clone();
                    move |_, cx| page.update(cx, |p, cx| p.add_all_to_queue(cx))
                }),
            ))
            .child(named(
                "crate-save-playlist",
                TextButton::new(
                    "crate-save-playlist",
                    &self.focus.save,
                    "Save as playlist\u{2026}",
                    accent,
                )
                .enabled(has_rows)
                .on_click({
                    let page = page.clone();
                    move |window, cx| page.update(cx, |p, cx| p.save_as_playlist(window, cx))
                }),
            ))
            .child(named(
                "crate-add-to-playlist",
                anchor::anchored(
                    ADD_TO_ANCHOR,
                    TextButton::new(
                        "crate-add-to-playlist",
                        &self.focus.add_to,
                        "Add all to\u{2026}",
                        accent,
                    )
                    .enabled(has_rows)
                    .on_click({
                        let page = page.clone();
                        move |window, cx| {
                            page.update(cx, |p, cx| p.add_all_to_playlist(window, cx))
                        }
                    }),
                ),
            ))
            .map(|row| {
                if self.confirming {
                    // Clear asks first: the button turns into the question and its two answers.
                    row.child(
                        style::text_style(div(), typography::BODY, cx)
                            .ml(px(spacing::S2))
                            .text_color(color::text_secondary())
                            .child(format!("Clear {}?", count_text(count))),
                    )
                    .child(named(
                        "crate-clear-yes",
                        TextButton::new("crate-clear-yes", &self.focus.confirm, "Clear", accent)
                            .strong()
                            .on_click({
                                let page = page.clone();
                                move |_, cx| page.update(cx, |p, cx| p.clear(cx))
                            }),
                    ))
                    .child(named(
                        "crate-clear-no",
                        TextButton::new("crate-clear-no", &self.focus.cancel, "Cancel", accent)
                            .on_click({
                                let page = page.clone();
                                move |_, cx| page.update(cx, |p, cx| p.cancel_clear(cx))
                            }),
                    ))
                } else {
                    row.child(named(
                        "crate-clear",
                        TextButton::new("crate-clear", &self.focus.clear, "Clear", accent)
                            .enabled(has_rows)
                            .on_click({
                                let page = page.clone();
                                move |_, cx| page.update(cx, |p, cx| p.ask_to_clear(cx))
                            }),
                    ))
                }
            })
            // "N selected" at the right end while 2 or more songs are picked.
            .child(
                div()
                    .ml_auto()
                    .debug_selector(|| "crate-selected-count".into())
                    .child(selection::count_slot(self.selection.len(), cx)),
            );
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                style::text_style(div(), typography::PAGE_TITLE, cx)
                    .text_color(color::text_primary())
                    .child("Crate"),
            )
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .id("crate-count")
                    // A non-breaking space keeps the line's height while the crate is empty.
                    .text_color(color::text_tertiary())
                    .child(if has_rows {
                        count_text(count)
                    } else {
                        "\u{a0}".into()
                    }),
            )
            .child(buttons)
            .into_any_element()
    }

    fn empty(&self, accent: Accent, cx: &mut Context<Self>) -> AnyElement {
        let on_search = self.on_search.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child("Nothing set aside yet"),
            )
            .child(
                div()
                    .debug_selector(|| "crate-search".into())
                    .child(
                        TextButton::new("crate-search", &self.focus.search, "Search", accent)
                            .on_click(move |window, cx| on_search(window, cx)),
                    ),
            )
            .into_any_element()
    }
}

impl Focusable for CratePage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.list.clone()
    }
}

impl Render for CratePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.accent = self.player.read(cx).view().accent;
        self.rows = self.crate_.read(cx).items().to_vec();
        self.refresh_selection();
        let accent = self.accent;
        let count = self.rows.len();

        let header = self.header(count, accent, cx);
        let body: AnyElement = if count == 0 {
            self.empty(accent, cx)
        } else {
            div()
                .id("crate-list")
                .key_context(KEY_CONTEXT)
                .track_focus(&self.focus.list)
                .role(gpui::Role::List)
                .aria_label("Crate")
                .on_action(cx.listener(|this, _: &CursorUp, _, cx| {
                    // The plain arrows only move the cursor: picked songs do not follow it.
                    this.selection.clear();
                    this.step(-1, cx)
                }))
                .on_action(cx.listener(|this, _: &CursorDown, _, cx| {
                    this.selection.clear();
                    this.step(1, cx)
                }))
                .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.extend(-1, cx)))
                .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.extend(1, cx)))
                .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)))
                .on_action(cx.listener(|this, _: &ClearPicks, _, cx| this.clear_picks(cx)))
                .on_action(cx.listener(|this, _: &PlayCursor, _, cx| this.play_cursor(cx)))
                .on_action(cx.listener(|this, _: &RemoveCursor, _, cx| this.remove_cursor(cx)))
                .on_action(cx.listener(|this, _: &LikeCursor, window, cx| this.like_cursor(window, cx)))
                .on_action(cx.listener(|this, _: &PlaylistCursor, window, cx| {
                    this.playlist_cursor(window, cx)
                }))
                .on_action(cx.listener(|this, _: &MoveCursorUp, _, cx| this.move_cursor(-1, cx)))
                .on_action(cx.listener(|this, _: &MoveCursorDown, _, cx| this.move_cursor(1, cx)))
                .size_full()
                .child(
                    uniform_list(
                        "crate-rows",
                        // One more than the songs: the slot a row can be dropped into at the end.
                        count + 1,
                        cx.processor(|this, range: Range<usize>, window, cx| {
                            this.rows(range, window, cx)
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .into_any_element()
        };

        div()
            .id("crate-page")
            .debug_selector(|| "crate-page".into())
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().mt(px(spacing::S4)).child(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_a_removal_the_cursor_takes_the_next_row_else_the_previous() {
        let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        assert_eq!(cursor_after_removal(&ids, "b").as_deref(), Some("c"));
        assert_eq!(cursor_after_removal(&ids, "c").as_deref(), Some("b"));
        assert_eq!(cursor_after_removal(&["a".to_owned()], "a"), None);
        assert_eq!(cursor_after_removal(&ids, "z"), None);
    }

    #[test]
    fn the_count_reads_naturally() {
        assert_eq!(count_text(1), "1 song");
        assert_eq!(count_text(12), "12 songs");
    }
}
