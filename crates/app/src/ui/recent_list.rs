//! The "Recently played" list: the songs heard in the last week, newest first (`crate::recent`),
//! so one loved an hour ago and forgotten can still be set aside. It is drawn twice:
//!
//! * [`Layout::Section`]: a collapsible section under the Crate page's list. Collapsed it is one
//!   line ("Recently played" with a chevron); the choice is remembered. Opening it shows four
//!   rows of the list (it scrolls).
//! * [`Layout::Dock`]: the body of the docked panel's **Recent** tab (the Queue · Crate · Recent
//!   control above it is the Crate page's dock header).
//!
//! Rows are song rows like the other lists': Enter (or a click) plays the row with the list as
//! the queue, C sets it aside in the Crate (and the cursor steps on), L likes, P and A file it,
//! the arrows, Shift+Arrows, Ctrl+A and Esc move and pick, a picked row drags (into the docked
//! Crate) and opens the menu of every picked row, and More or right-click open the usual menu.
//! A hover button beside the heart sets the song aside too. The list is read from the local play
//! log only; nothing here asks YouTube for anything.

use std::ops::Range;
use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, svg, uniform_list, AnyElement, App, ClickEvent, Context, Entity,
    FocusHandle, Focusable, IntoElement, KeyBinding, KeyboardButton, ParentElement, Render,
    ScrollStrategy, SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::assets::Icon;
use crate::crate_list::CrateModel;
use crate::library::LibraryWrites;
use crate::player::PlayerModel;
use crate::recent::{self, RecentModel, RecentRow};
use crate::style::{self, color};
use crate::ui::art_tile::art_tile;
use crate::ui::button::{focus_ring, keyboard_focused, Handler};
use crate::ui::player_bar::text_line;
use crate::ui::playing_row;
use crate::ui::row_actions::{heart_action, more_slot, row_action, ACTION_SIZE};
use crate::ui::selection::{self, Click, Selection};
use crate::ui::song_drag;
use crate::ui::track_menu::{self, Command, Subject, TrackSubject};

actions!(
    apollo_recent,
    [
        CursorUp,
        CursorDown,
        PlayCursor,
        LikeCursor,
        PlaylistCursor,
        TargetCursor,
        CrateCursor,
        ExtendUp,
        ExtendDown,
        SelectAll,
        ClearPicks
    ]
);

/// Key context of the list's focus handle.
pub const KEY_CONTEXT: &str = "RecentList";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", CursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", CursorDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", PlayCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("l", LikeCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("p", PlaylistCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("a", TargetCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("c", CrateCursor, Some(KEY_CONTEXT)),
        // On the page, not in the panel on the right: from there Left is just the previous control.
        KeyBinding::new("left", crate::ui::shell::FocusSidebar, Some("RecentList && !Docked")),
        KeyBinding::new("shift-up", ExtendUp, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", ClearPicks, Some(KEY_CONTEXT)),
    ]);
}

/// Where the list is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The collapsible section under the Crate page's list.
    Section,
    /// The body of the docked panel's Recent tab.
    Dock,
}

/// Same 56px rows as every other song list, with the focus ring's room around the content.
const ROW_HEIGHT: f32 = 56.0;
const ROW_INSET: f32 = spacing::S1;
const ART: f32 = 40.0;
const END_WIDTH: f32 = spacing::S12;
/// The section's header line, and how many rows it shows when open.
const HEADER_HEIGHT: f32 = spacing::S12;
const SECTION_ROWS: f32 = 4.0;
const CHEVRON: f32 = 16.0;

/// The page's own tab stops, in the slot after the Crate page's controls; the dock's, in the
/// slot of the Crate panel's list (the two panels never show together).
const TAB_SECTION: isize = 16;
const DOCK_TAB_LIST: isize = 17;

/// A row: the song and when it was played (a song heard twice, not in a row, is two rows).
type Key = (String, i64);

fn key_of(row: &RecentRow) -> Key {
    (row.track.video_id.as_str().to_owned(), row.played_unix)
}

pub struct RecentList {
    model: Entity<RecentModel>,
    crate_: Entity<CrateModel>,
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    layout: Layout,
    scroll: UniformListScrollHandle,
    focus_list: FocusHandle,
    /// The section's header (a tab stop; Enter or Space opens and closes the section).
    focus_toggle: FocusHandle,
    /// The row the keyboard acts on, shown only while the list has focus.
    cursor: Option<Key>,
    selection: Selection<Key>,
    /// What a picked row's More button and right-click open: the menu of the whole selection.
    bulk_menu: Option<Subject>,
    /// Copied at each render for the list's row callback.
    rows: Vec<RecentRow>,
    accent: Accent,
    /// Esc in the docked list with nothing picked: focus goes back to where it was before the
    /// jump into the dock (the shell's).
    on_leave: Option<Rc<Handler>>,
}

impl RecentList {
    pub fn new(
        layout: Layout,
        model: Entity<RecentModel>,
        crate_: Entity<CrateModel>,
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&crate_, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        let docked = layout == Layout::Dock;
        let index = if docked { DOCK_TAB_LIST } else { TAB_SECTION };
        Self {
            model,
            crate_,
            player,
            writes,
            layout,
            scroll: UniformListScrollHandle::new(),
            focus_list: cx.focus_handle().tab_index(index).tab_stop(true),
            focus_toggle: cx.focus_handle().tab_index(TAB_SECTION).tab_stop(true),
            cursor: None,
            selection: Selection::new(),
            bulk_menu: None,
            rows: Vec::new(),
            accent: Accent::fallback(),
            on_leave: None,
        }
    }

    /// What Esc does in the docked list when nothing is picked.
    pub fn set_on_leave(&mut self, on_leave: Rc<Handler>) {
        self.on_leave = Some(on_leave);
    }

    fn docked(&self) -> bool {
        self.layout == Layout::Dock
    }

    /// Whether the rows are on screen: always in the dock, while the section is open on the page.
    fn showing(&self, cx: &App) -> bool {
        self.docked() || self.model.read(cx).is_open()
    }

    /// The list's and the section header's focus handles.
    pub fn focus_handles(&self) -> [FocusHandle; 2] {
        [self.focus_list.clone(), self.focus_toggle.clone()]
    }

    /// Ctrl+Shift+C with the Recent tab showing: the keyboard lands in the list, on the row it
    /// was last on, else the first. Whether there was a row to land on.
    pub fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.rows_now(cx);
        if rows.is_empty() {
            return false;
        }
        let at = self
            .cursor
            .as_ref()
            .and_then(|c| rows.iter().position(|r| key_of(r) == *c))
            .unwrap_or(0);
        self.cursor = Some(key_of(&rows[at]));
        self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        window.focus(&self.focus_list, cx);
        cx.notify();
        true
    }

    /// The rows as the model has them now (what a key acts on).
    fn rows_now(&self, cx: &App) -> Vec<RecentRow> {
        self.model.read(cx).rows().to_vec()
    }

    fn cursor_index(&self, rows: &[RecentRow]) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        rows.iter().position(|r| key_of(r) == *cursor)
    }

    // ---- keys --------------------------------------------------------------------------

    fn cursor_or_playing(&self, rows: &[RecentRow], cx: &App) -> Option<usize> {
        let playing = self.player.read(cx).current_video();
        self.cursor_index(rows)
            .or_else(|| {
                playing.and_then(|p| rows.iter().position(|r| r.track.video_id.as_str() == p))
            })
            .or((!rows.is_empty()).then_some(0))
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows_now(cx);
        if rows.is_empty() {
            return;
        }
        let at = self.cursor_or_playing(&rows, cx).unwrap_or(0);
        // The first press only shows the cursor.
        let to = if self.cursor.is_none() {
            at
        } else {
            (at as isize + delta).clamp(0, rows.len() as isize - 1) as usize
        };
        self.cursor = Some(key_of(&rows[to]));
        self.scroll.scroll_to_item(to, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn play_cursor(&mut self, cx: &mut Context<Self>) {
        let rows = self.rows_now(cx);
        if let Some(ix) = self.cursor_index(&rows) {
            self.activate(ix, cx);
        }
    }

    /// A row was chosen: play it with the list as the queue, or pause it if it is the one playing.
    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = self.rows_now(cx);
        let Some(row) = rows.get(ix) else { return };
        let playing = self.player.read(cx).current_video();
        if playing.as_deref() == Some(row.track.video_id.as_str()) {
            self.player.update(cx, |m, cx| m.toggle_play(cx));
            return;
        }
        self.player.update(cx, |p, cx| {
            for r in &rows {
                p.hint_art(r.track.video_id.as_str(), r.art.as_deref());
            }
            p.play_tracks(rows.iter().map(|r| r.track.clone()).collect(), Some(ix), None, cx);
        });
    }

    /// C: the song under the cursor (or every picked song) is set aside in the Crate, and the
    /// cursor steps to the next row.
    fn crate_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_bulk(Command::AddToCrate, window, cx) {
            return;
        }
        let rows = self.rows_now(cx);
        let Some(at) = self.cursor_index(&rows) else {
            return;
        };
        let row = rows[at].clone();
        crate::crate_list::add_track(&self.player, row.track, row.art, cx);
        self.advance(&rows, at, cx);
    }

    /// After C or A acted on the cursor row: the cursor steps to the next row (Settings >
    /// Playlists), the last row staying put.
    fn advance(&mut self, rows: &[RecentRow], at: usize, cx: &mut Context<Self>) {
        if let Some(next) = crate::auto_advance::next_row(at, rows.len(), cx) {
            self.cursor = Some(key_of(&rows[next]));
            self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    fn like_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_bulk(Command::ToggleLike, window, cx) {
            return;
        }
        let rows = self.rows_now(cx);
        if let Some(ix) = self.cursor_index(&rows) {
            let video = rows[ix].track.video_id.as_str().to_owned();
            self.writes.update(cx, |w, cx| w.toggle_like(&video, cx));
        }
    }

    /// The songs P and A act on: every picked song, or the one under the cursor.
    fn filing_songs(&self, rows: &[RecentRow]) -> Vec<crate::ui::playlist_picker::PickerSong> {
        let song = |r: &RecentRow| {
            crate::ui::playlist_picker::PickerSong::new(
                r.track.video_id.as_str(),
                r.track.title.clone(),
            )
        };
        if self.selection.is_bulk() {
            rows.iter()
                .filter(|r| self.selection.contains(&key_of(r)))
                .map(song)
                .collect()
        } else {
            self.cursor_index(rows).map(|ix| song(&rows[ix])).into_iter().collect()
        }
    }

    /// The P key: the playlist picker for the song under the cursor, or for every picked song.
    fn playlist_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !crate::ui::playlist_picker::key_enabled(cx) {
            return;
        }
        let songs = self.filing_songs(&self.rows_now(cx));
        crate::ui::playlist_picker::open_songs(songs, None, window, cx);
    }

    /// The A key: every picked song, or the one under the cursor, to the target playlist.
    fn target_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows_now(cx);
        let songs = self.filing_songs(&rows);
        // With the key acting on one song and a target set it goes straight in, so the cursor
        // steps on; with none the picker opens and the cursor stays.
        let direct = !songs.is_empty()
            && !self.selection.is_bulk()
            && crate::target_playlist::has_target(cx);
        crate::target_playlist::add_songs(songs, None, window, cx);
        if direct {
            if let Some(at) = self.cursor_index(&rows) {
                self.advance(&rows, at, cx);
            }
        }
    }

    // ---- picking songs -----------------------------------------------------------------

    /// The picked songs in list order, as the track menu takes them.
    fn picked_songs(&self, rows: &[RecentRow]) -> Vec<TrackSubject> {
        rows.iter()
            .filter(|r| self.selection.contains(&key_of(r)))
            .map(|r| TrackSubject::of_row(r.track.clone(), r.art.clone(), None))
            .collect()
    }

    /// Per frame: drop picks that left the list and work out the selection's menu.
    fn refresh_selection(&mut self) {
        let keys: Vec<Key> = self.rows.iter().map(key_of).collect();
        self.selection.keep(&keys);
        self.bulk_menu = if self.selection.is_bulk() {
            Subject::tracks(self.picked_songs(&self.rows))
        } else {
            None
        };
    }

    /// A click on row `ix`. Whether it was a Ctrl or Shift click (a pick), which the row's own
    /// action then leaves alone. A plain click clears the picks.
    fn pick_row(&mut self, ix: usize, how: Click, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.rows_now(cx);
        let Some(row) = rows.get(ix) else {
            return false;
        };
        let keys: Vec<Key> = rows.iter().map(key_of).collect();
        let key = key_of(row);
        let picked = self.selection.click(how, &key, self.cursor.as_ref(), &keys);
        if picked {
            // The keyboard carries on from the clicked row: Shift+Arrows, Ctrl+A and Esc work.
            self.cursor = Some(key);
            window.focus(&self.focus_list, cx);
        }
        cx.notify();
        picked
    }

    /// Shift+Up / Shift+Down: the cursor moves and the picked rows stretch (or shrink) to it.
    fn extend(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows_now(cx);
        if rows.is_empty() {
            return;
        }
        let keys: Vec<Key> = rows.iter().map(key_of).collect();
        let at = self.cursor_or_playing(&rows, cx).unwrap_or(0);
        let to = (at as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        self.selection.extend(keys.get(at), &keys[to], &keys);
        self.cursor = Some(keys[to].clone());
        self.scroll.scroll_to_item(to, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Ctrl+A: every song.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let keys: Vec<Key> = self.rows_now(cx).iter().map(key_of).collect();
        self.selection.select_all(&keys);
        cx.notify();
    }

    /// Esc: picked songs go first; with none picked Esc does what it did before.
    fn clear_picks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            // In the docked list Esc goes back to where the jump into it came from.
            match self.on_leave.clone().filter(|_| self.docked()) {
                // Deferred: the shell looks at this very list while it finds somewhere to land.
                Some(leave) => window.defer(cx, move |window, cx| leave(window, cx)),
                None => cx.propagate(),
            }
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
        let rows = self.rows_now(cx);
        if let Some(subject) = Subject::tracks(self.picked_songs(&rows)) {
            track_menu::run_now(command, subject, window, cx);
        }
        true
    }

    // ---- rows --------------------------------------------------------------------------

    fn rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // Artwork loads only for what is on screen, from the cover the log kept.
        let shown: Vec<(String, Option<String>)> = range
            .clone()
            .filter_map(|ix| self.rows.get(ix))
            .map(|r| (r.track.video_id.as_str().to_owned(), r.art.clone()))
            .collect();
        if !shown.is_empty() {
            self.player.update(cx, |p, cx| {
                for (video, art) in &shown {
                    p.hint_art(video, art.as_deref());
                }
                p.request_thumbs(shown.iter().map(|(v, _)| v.clone()).collect(), cx);
            });
        }
        range.map(|ix| self.row(ix, window, cx)).collect()
    }

    fn row(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return div().h(px(ROW_HEIGHT)).into_any_element();
        };
        let docked = self.docked();
        // Names that tell the dock's rows from the page's, so tests find the right ones.
        let (name, row_id, like_id, add_id) = if docked {
            ("recent-dock", "recent-dock-row", "recent-dock-like", "recent-dock-add")
        } else {
            ("recent", "recent-row", "recent-like", "recent-add")
        };
        let video: String = row.track.video_id.as_str().to_owned();
        let key = key_of(&row);
        let playing = self.player.read(cx).current_video().as_deref() == Some(video.as_str());
        let list_focused = self.focus_list.is_focused(window);
        let at_cursor = list_focused && self.cursor.as_ref() == Some(&key);
        let ring = at_cursor && keyboard_focused(&self.focus_list, window);
        let accent = color::accent(self.accent);
        let tint = selection::row_tint(self.accent);
        let selected = playing || at_cursor;
        let picked = self.selection.contains(&key);
        let group: SharedString = format!("{name}-row-{ix}").into();
        let thumb = self.player.read(cx).thumb(&video);
        let liked = self.writes.read(cx).is_liked(&video);
        let in_crate = self.crate_.read(cx).contains(&video);
        let title: SharedString = row.track.title.clone().into();
        let artists: SharedString = row
            .track
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
            .into();
        let now = apollo_services_common::unix_now() as i64;
        let when: SharedString = recent::ago_text(now, row.played_unix).into();

        // A picked row speaks for every picked row.
        let subject = match &self.bulk_menu {
            Some(bulk) if picked => bulk.clone(),
            _ => Subject::track(row.track.clone(), row.art.clone()),
        };

        // Room for every action is always reserved; they show on hover or under the cursor, and
        // the heart stays once the song is liked.
        let heart = div()
            .debug_selector(move || format!("{name}-like-{ix}"))
            .flex_none()
            .size(px(ACTION_SIZE))
            .when(!(liked || at_cursor), |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .child(heart_action((like_id, ix), liked, {
                let (writes, video) = (self.writes.clone(), video.clone());
                move |_, cx| writes.update(cx, |w, cx| w.toggle_like(&video, cx))
            }));
        let more = more_slot(format!("{name}-more-{ix}"), true, subject.clone())
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            });
        // Set aside in one click; the slot stays (empty) for a song that is in the Crate already.
        let add = div()
            .debug_selector(move || format!("{name}-add-{ix}"))
            .flex_none()
            .size(px(ACTION_SIZE))
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .when(!in_crate, |d| {
                d.child(row_action(
                    (add_id, ix),
                    Icon::BoxRegular,
                    "Add to Crate",
                    color::text_secondary(),
                    {
                        let (player, row) = (self.player.clone(), row.clone());
                        move |_, cx| {
                            crate::crate_list::add_track(&player, row.track.clone(), row.art.clone(), cx)
                        }
                    },
                ))
            });

        let row_tint = if picked {
            tint
        } else if selected {
            playing_row::tint()
        } else {
            color::state_hover()
        };
        // The dock, like the queue panel and the Crate, shows its actions in the place of the
        // time (and only while the pointer or the cursor is on the row), so the title keeps its
        // width in 320px.
        let end: Vec<AnyElement> = if docked {
            let when_cell = style::tabular(style::text_style(div(), typography::CAPTION, cx))
                .absolute()
                .right_0()
                .h(px(ACTION_SIZE))
                .flex()
                .items_center()
                .text_color(color::text_tertiary())
                .when(!at_cursor, |d| d.group_hover(group.clone(), |s| s.opacity(0.0)))
                .when(at_cursor, |d| d.opacity(0.0))
                .child(when);
            let actions = div()
                .absolute()
                .right_0()
                .flex()
                .flex_row()
                .rounded(px(radius::SMALL))
                // Opaque, so the title never shows through the buttons.
                .bg(color::bg_raised())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .rounded(px(radius::SMALL))
                        .bg(row_tint)
                        .child(add)
                        .child(more),
                );
            vec![div()
                .relative()
                .flex_none()
                .w(px(END_WIDTH))
                .h(px(ACTION_SIZE))
                .child(when_cell)
                .child(actions)
                .into_any_element()]
        } else {
            vec![
                div()
                    .w(px(END_WIDTH))
                    .flex_none()
                    .flex()
                    .justify_end()
                    .child(text_line(
                        "when",
                        when,
                        typography::CAPTION,
                        color::text_tertiary(),
                        cx,
                    ))
                    .into_any_element(),
                add.into_any_element(),
                more.into_any_element(),
            ]
        };

        let surface = div()
            .id((row_id, ix))
            .debug_selector(move || format!("{name}-row-{ix}"))
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
            .hover(move |s| s.bg(row_tint))
            .role(gpui::Role::ListItem)
            .aria_label(title.clone())
            .aria_selected(playing || picked)
            .on_click(cx.listener({
                let key = key.clone();
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
                    window.focus(&this.focus_list, cx);
                    this.cursor = Some(key.clone());
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
            .children(end)
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)));

        // A row drags (into the docked Crate) with the songs of its menu: every picked song when
        // it is one of them.
        let surface = song_drag::draggable(surface, &subject);
        let surface = track_menu::on_context_menu(surface, subject);
        div()
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .py(px(ROW_INSET))
            .when(docked, |d| d.px(px(spacing::S2)))
            .child(surface)
            .into_any_element()
    }

    // ---- pieces ------------------------------------------------------------------------

    /// The list, or the line that says there is nothing to list.
    fn body(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        if count == 0 {
            return div()
                .debug_selector(|| "recent-empty".into())
                .flex()
                .items_center()
                .h(px(layout::MIN_HIT_TARGET))
                .px(px(if self.docked() { spacing::S4 } else { 0.0 }))
                .child(
                    style::text_style(div(), typography::BODY, cx)
                        .text_color(color::text_secondary())
                        .child("Nothing played yet"),
                )
                .into_any_element();
        }
        let docked = self.docked();
        div()
            .id(if docked { "recent-dock-list" } else { "recent-list" })
            .key_context(if docked { "RecentList Docked" } else { KEY_CONTEXT })
            .track_focus(&self.focus_list)
            .role(gpui::Role::List)
            .aria_label("Recently played")
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
            .on_action(cx.listener(|this, _: &ClearPicks, window, cx| this.clear_picks(window, cx)))
            .on_action(cx.listener(|this, _: &PlayCursor, _, cx| this.play_cursor(cx)))
            .on_action(cx.listener(|this, _: &LikeCursor, window, cx| this.like_cursor(window, cx)))
            .on_action(cx.listener(|this, _: &PlaylistCursor, window, cx| {
                this.playlist_cursor(window, cx)
            }))
            .on_action(cx.listener(|this, _: &TargetCursor, window, cx| {
                this.target_cursor(window, cx)
            }))
            .on_action(cx.listener(|this, _: &CrateCursor, window, cx| this.crate_cursor(window, cx)))
            .size_full()
            .child(
                uniform_list(
                    if docked { "recent-dock-rows" } else { "recent-rows" },
                    count,
                    cx.processor(|this, range: Range<usize>, window, cx| {
                        this.rows(range, window, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
            .into_any_element()
    }

    /// The section's header: the chevron and "Recently played". A click, Enter or Space opens or
    /// closes the section.
    fn toggle(&self, open: bool, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.clone();
        let accent = color::accent(self.accent);
        let ring = keyboard_focused(&self.focus_toggle, window);
        let chevron = if open { Icon::ChevronDown } else { Icon::ChevronRight };
        style::text_style(div(), typography::SECTION, cx)
            .id("recent-toggle")
            .debug_selector(|| "recent-toggle".into())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(HEADER_HEIGHT))
            .w_full()
            .flex_none()
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .track_focus(&self.focus_toggle)
            .role(gpui::Role::Button)
            .aria_label("Recently played")
            .aria_expanded(open)
            .text_color(color::text_primary())
            .on_click(move |event: &ClickEvent, _, cx| {
                // Space is the play/pause key.
                if let ClickEvent::Keyboard(k) = event {
                    if k.button == KeyboardButton::Space {
                        return;
                    }
                }
                model.update(cx, |m, cx| m.set_open(!open, cx));
            })
            .child(
                svg()
                    .path(chevron.path())
                    .size(px(CHEVRON))
                    .flex_none()
                    .text_color(color::text_secondary()),
            )
            .child("Recently played")
            .when(ring, move |d| d.child(focus_ring(accent, radius::SMALL)))
            .into_any_element()
    }
}

impl Focusable for RecentList {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_list.clone()
    }
}

impl Render for RecentList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.accent = self.player.read(cx).view().accent;
        let showing = self.showing(cx);
        if showing {
            // Read the log again when a play counted since the rows were read.
            self.model.update(cx, |m, cx| m.ensure_fresh(cx));
        }
        self.rows = if showing { self.rows_now(cx) } else { Vec::new() };
        self.refresh_selection();
        let count = self.rows.len();

        if self.docked() {
            return div()
                .id("recent-dock")
                .debug_selector(|| "recent-dock".into())
                .flex_1()
                .min_h_0()
                .w_full()
                .child(self.body(count, cx))
                .into_any_element();
        }
        let open = self.model.read(cx).is_open();
        div()
            .id("recent-section")
            .debug_selector(|| "recent-section".into())
            .flex()
            .flex_col()
            .flex_none()
            .w_full()
            .child(self.toggle(open, window, cx))
            .when(open, |d| {
                d.child(
                    div()
                        .h(px(SECTION_ROWS * ROW_HEIGHT))
                        .w_full()
                        .flex_none()
                        .child(self.body(count, cx)),
                )
            })
            .into_any_element()
    }
}
