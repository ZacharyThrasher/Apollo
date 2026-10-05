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
//! The same list also lives docked on the right of the main window ([`Layout::Dock`]): 320px
//! wide, kept open while you move between pages, with Save as playlist… and Add all to… in its
//! header. It takes songs dragged in from any list (the drop point is where they land; C still
//! adds at the bottom) and arranges, picks, removes and plays like the page.
//!
//! A filter field in the header (the playlist page's) narrows the list to the songs whose title,
//! artist or album hold every word typed. While it has text, Play, Shuffle, Add all and Save act
//! on the songs shown, Clear takes only those out, and arranging by place (drag, Alt+Up/Down,
//! Arrange) is off; songs dropped on the docked list go to the bottom.
//!
//! The Arrange menu rewrites the whole Crate's order, free and local (`crate_arrange`): Sort by
//! Title, Artist, Album, Date added or Length, Shuffle, Reverse, Spread artists. Each is one step
//! for Ctrl+Z, and Alt+Home / Alt+End send the row under the cursor (or every picked row) to the
//! top or bottom.
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
use std::time::Duration;

use apollo_store::CrateRow;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, deferred, div, prelude::*, px, uniform_list, AnyElement, App, ClickEvent, Context,
    Entity, FocusHandle, Focusable, IntoElement, KeyBinding, KeyboardButton, MouseButton,
    ParentElement, Render, ScrollStrategy, SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::assets::Icon;
use crate::crate_add::{CrateAddEvent, CrateAddModel};
use crate::crate_arrange::{self, Arrange};
use crate::crate_list::{CrateEvent, CrateModel};
use crate::library::LibraryWrites;
use crate::ui::anchor;
use crate::ui::playlist_dialog::{self, Mode, CRATE_PLAYLIST_NAME};
use crate::ui::playlist_picker::PlaylistPicker;
use crate::player::{format_time, PlayerModel};
use crate::style::{self, color};
use crate::ui::art_tile::art_tile;
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::crate_add::{CrateAddBox, Leave};
use crate::ui::dock_tabs::{Dock, DockFocus, DockTabs, OnPick as DockPick};
use crate::ui::library::DraggedRow as LibraryRow;
use crate::ui::song_drag::SongDrag;
use crate::ui::player_bar::text_line;
use crate::ui::playing_row;
use crate::ui::row_actions::{heart_action, more_slot, row_action, ACTION_SIZE};
use crate::ui::selection::{self, Click, Selection};
use crate::ui::text_button::TextButton;
use crate::ui::text_field::{TextField, TextFieldEvent};
use crate::ui::track_menu::{self, Command, Subject, TrackSubject, ITEM_HEIGHT, MENU_WIDTH};

actions!(
    apollo_crate,
    [
        CursorUp,
        CursorDown,
        PlayCursor,
        RemoveCursor,
        LikeCursor,
        PlaylistCursor,
        TargetCursor,
        MoveCursorUp,
        MoveCursorDown,
        CrateMoveTop,
        CrateMoveBottom,
        ExtendUp,
        ExtendDown,
        SelectAll,
        ClearPicks,
        CrateFocusAdd,
        CratePaste
    ]
);

actions!(
    apollo_crate_arrange,
    [ArrangeUp, ArrangeDown, ArrangeChoose, ArrangeClose, ArrangeStay]
);

/// Key context of the list's focus handle.
pub const KEY_CONTEXT: &str = "CrateList";

/// Key context of the header's button row: `/` and Ctrl+V work from there too (not from the filter
/// or add fields, which have their own contexts).
pub const BAR_CONTEXT: &str = "CrateBar";

/// Key context of the Arrange popover.
pub const ARRANGE_CONTEXT: &str = "CrateArrange";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", CursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", CursorDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", PlayCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", RemoveCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("backspace", RemoveCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("l", LikeCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("p", PlaylistCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("a", TargetCursor, Some(KEY_CONTEXT)),
        // On the page, not in the panel on the right: from there Left is just the previous control.
        KeyBinding::new("left", crate::ui::shell::FocusSidebar, Some("CrateList && !Docked")),
        KeyBinding::new("alt-up", MoveCursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-down", MoveCursorDown, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-home", CrateMoveTop, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-end", CrateMoveBottom, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-up", ExtendUp, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAll, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", ClearPicks, Some(KEY_CONTEXT)),
        KeyBinding::new("/", CrateFocusAdd, Some(KEY_CONTEXT)),
        KeyBinding::new("/", CrateFocusAdd, Some(BAR_CONTEXT)),
        KeyBinding::new("ctrl-v", CratePaste, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-v", CratePaste, Some(BAR_CONTEXT)),
        KeyBinding::new("up", ArrangeUp, Some(ARRANGE_CONTEXT)),
        KeyBinding::new("down", ArrangeDown, Some(ARRANGE_CONTEXT)),
        KeyBinding::new("enter", ArrangeChoose, Some(ARRANGE_CONTEXT)),
        KeyBinding::new("space", ArrangeChoose, Some(ARRANGE_CONTEXT)),
        KeyBinding::new("escape", ArrangeClose, Some(ARRANGE_CONTEXT)),
        // Focus stays in the popover while it is open.
        KeyBinding::new("tab", ArrangeStay, Some(ARRANGE_CONTEXT)),
        KeyBinding::new("shift-tab", ArrangeStay, Some(ARRANGE_CONTEXT)),
    ]);
}

/// Where the list is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// The Crate page, with its wide header.
    Page,
    /// The docked panel on the right of the main window.
    Dock,
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
const TAB_MORE: isize = 12;
const TAB_FILTER: isize = 13;
const TAB_ARRANGE: isize = 14;
const TAB_LIST: isize = 15;
const TAB_SEARCH: isize = 16;

/// The docked panel's tab stops: its controls sit in the queue panel's slots (13-17), after the
/// Queue · Crate control.
const DOCK_TAB_SAVE: isize = 14;
const DOCK_TAB_ADD_TO: isize = 15;
const DOCK_TAB_MORE: isize = 15;
const DOCK_TAB_LIST: isize = 17;
/// The filter and Arrange share the slot between the panel's buttons and its list (the filter
/// comes first in the header, so Tab reaches it first).
const DOCK_TAB_FILTER: isize = 16;

/// Width of the filter field in the page's header, as on a playlist page.
const FILTER_WIDTH: f32 = 240.0;
/// How many of the Arrange menu's choices are sorts (the rest follow a divider).
const SORTS: usize = crate_arrange::SortKey::ALL.len();

/// Where the "Add all to…" popover hangs from.
const ADD_TO_ANCHOR: &str = "crate-add-to-playlist";
const DOCK_ADD_TO_ANCHOR: &str = "crate-dock-add-to-playlist";
/// Where the header's More menu hangs from.
const MORE_ANCHOR: &str = "crate-more";
const DOCK_MORE_ANCHOR: &str = "crate-dock-more";

struct Focus {
    play: FocusHandle,
    shuffle: FocusHandle,
    add_all: FocusHandle,
    save: FocusHandle,
    add_to: FocusHandle,
    more: FocusHandle,
    clear: FocusHandle,
    list: FocusHandle,
    search: FocusHandle,
    /// The Arrange button, and the popover it opens (not a tab stop).
    arrange: FocusHandle,
    arrange_menu: FocusHandle,
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
fn drop_line(group: SharedString, dock: bool) -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left(px(ROW_INSET))
        .right(px(ROW_INSET))
        .h(px(DROP_LINE))
        .rounded_full()
        .bg(color::text_secondary())
        .opacity(0.0)
        .group_drag_over::<DraggedRow>(group.clone(), |s| s.opacity(1.0))
        // The docked list also takes songs from the other lists.
        .when(dock, |d| {
            d.group_drag_over::<SongDrag>(group.clone(), |s| s.opacity(1.0))
                .group_drag_over::<LibraryRow>(group, |s| s.opacity(1.0))
        })
}

/// A header button wrapped so tests (and the layout) can find it by name.
fn named(name: &'static str, button: impl IntoElement) -> gpui::Div {
    div().debug_selector(move || name.into()).child(button)
}

/// "3 of 42 songs": what the filter keeps, of the whole Crate.
fn filtered_count_text(shown: usize, total: usize) -> SharedString {
    format!("{shown} of {total} songs").into()
}

/// "1 song" / "12 songs".
fn count_text(n: usize) -> SharedString {
    if n == 1 {
        "1 song".into()
    } else {
        format!("{n} songs").into()
    }
}

/// How long songs run, as the header says it: `m:ss` under an hour, else "2 h 51 min" (rounded
/// to the minute, a whole hour as "2 h").
pub fn length_text(total: Duration) -> String {
    let secs = total.as_secs();
    if secs < 3600 {
        return format!("{}:{:02}", secs / 60, secs % 60);
    }
    let minutes = (secs + 30) / 60;
    match (minutes / 60, minutes % 60) {
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

/// The header's second line: "42 songs · 2 h 51 min", with " + 2 unknown" for songs that carry no
/// length. With no length known at all (or no songs) it is just `label`.
pub fn summary_text(label: &str, lengths: impl IntoIterator<Item = Option<Duration>>) -> String {
    let (mut known, mut unknown) = (Duration::ZERO, 0usize);
    let mut any = false;
    for length in lengths {
        match length {
            Some(d) => {
                known += d;
                any = true;
            }
            None => unknown += 1,
        }
    }
    if !any {
        return label.to_owned();
    }
    let mut text = format!("{label} \u{b7} {}", length_text(known));
    if unknown > 0 {
        text.push_str(&format!(" + {unknown} unknown"));
    }
    text
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
    /// The filter field, made once the Crate has songs to filter.
    filter: Option<Entity<TextField>>,
    /// The "Add songs" box above the list, and what it holds.
    add: Entity<CrateAddBox>,
    add_model: Entity<CrateAddModel>,
    /// The Arrange popover is open, and the choice the keyboard is on.
    arrange_open: bool,
    arrange_cursor: usize,
    /// Songs picked for bulk verbs, by video id.
    selection: Selection<String>,
    /// What a picked row's More button and right-click open: the menu of the whole selection.
    bulk_menu: Option<Subject>,
    /// Copied at each render for the list's row callback.
    rows: Vec<CrateRow>,
    accent: Accent,
    on_search: Rc<Handler>,
    layout: Layout,
    /// The Queue · Crate control, drawn above the docked header.
    dock_tabs: Option<(DockFocus, DockPick)>,
    /// Esc in the docked list with nothing picked: focus goes back to where it was before the
    /// jump into the dock (the shell's).
    on_leave: Option<Rc<Handler>>,
}

impl CratePage {
    /// How far the list is scrolled; the window's history keeps it.
    pub fn scroll_offset(&self) -> gpui::Point<gpui::Pixels> {
        self.scroll.0.borrow().base_handle.offset()
    }

    pub fn set_scroll_offset(&self, offset: gpui::Point<gpui::Pixels>) {
        self.scroll.0.borrow().base_handle.set_offset(offset);
    }

    pub fn new(
        crate_: Entity<CrateModel>,
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        on_search: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(Layout::Page, crate_, player, writes, picker, on_search, cx)
    }

    /// The docked panel over the same crate (see the module docs).
    pub fn dock(
        crate_: Entity<CrateModel>,
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::build(Layout::Dock, crate_, player, writes, picker, |_, _| {}, cx)
    }

    fn build(
        layout: Layout,
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
        cx.observe(&crate_, |_, _, cx| cx.notify()).detach();
        // A song swapped for another version of itself keeps its place, its pick and the cursor.
        cx.subscribe(&crate_, |this, _, event: &CrateEvent, cx| {
            if let CrateEvent::Swapped(pairs) = event {
                this.selection.remap(pairs);
                if let Some(cursor) = this.cursor.as_mut() {
                    if let Some((_, now)) = pairs.iter().find(|(was, _)| was == cursor) {
                        *cursor = now.clone();
                    }
                }
                cx.notify();
            }
        })
        .detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        crate::confirm::watch(cx);
        let dock = layout == Layout::Dock;
        let add_model = cx.new(|_| CrateAddModel::new(crate_.clone(), player.clone()));
        // Esc and Down in the add box hand the keyboard to the list.
        let on_leave: Leave = {
            let page = cx.weak_entity();
            Rc::new(move |window, cx| {
                let _ = page.update(cx, |p, cx| {
                    p.enter(window, cx);
                });
            })
        };
        let add_tab = if dock { DOCK_TAB_FILTER } else { TAB_ARRANGE };
        let add = cx.new(|cx| CrateAddBox::new(add_model.clone(), add_tab, dock, on_leave, cx));
        // Songs added by search or paste: the list scrolls to the first one.
        cx.subscribe(&add_model, |this, _, event: &CrateAddEvent, cx| {
            let CrateAddEvent::Added(ids) = event;
            this.scroll_to_added(ids, cx);
        })
        .detach();
        // The docked panel's stops are in the queue panel's slots; the page's own are as before.
        let at = |page: isize, docked: isize| if dock { docked } else { page };
        let focus = Focus {
            play: stop(cx, at(TAB_PLAY, DOCK_TAB_SAVE)),
            shuffle: stop(cx, at(TAB_SHUFFLE, DOCK_TAB_SAVE)),
            add_all: stop(cx, at(TAB_ADD_ALL, DOCK_TAB_SAVE)),
            save: stop(cx, at(TAB_SAVE, DOCK_TAB_SAVE)),
            add_to: stop(cx, at(TAB_ADD_TO, DOCK_TAB_ADD_TO)),
            more: stop(cx, at(TAB_MORE, DOCK_TAB_MORE)),
            clear: stop(cx, at(TAB_CLEAR, DOCK_TAB_ADD_TO)),
            list: stop(cx, at(TAB_LIST, DOCK_TAB_LIST)),
            search: stop(cx, at(TAB_SEARCH, DOCK_TAB_LIST)),
            arrange: stop(cx, at(TAB_ARRANGE, DOCK_TAB_FILTER)),
            arrange_menu: cx.focus_handle(),
        };
        Self {
            crate_,
            player,
            writes,
            picker,
            focus,
            scroll: UniformListScrollHandle::new(),
            cursor: None,
            filter: None,
            add,
            add_model,
            arrange_open: false,
            arrange_cursor: 0,
            selection: Selection::new(),
            bulk_menu: None,
            rows: Vec::new(),
            accent: Accent::fallback(),
            on_search: Rc::new(Box::new(on_search)),
            layout,
            dock_tabs: None,
            on_leave: None,
        }
    }

    /// The control that swaps the docked panel for the queue.
    pub fn set_dock_tabs(&mut self, focus: DockFocus, on_pick: DockPick) {
        self.dock_tabs = Some((focus, on_pick));
    }

    /// What Esc does in the docked list when nothing is picked.
    pub fn set_on_leave(&mut self, on_leave: Rc<Handler>) {
        self.on_leave = Some(on_leave);
    }

    fn docked(&self) -> bool {
        self.layout == Layout::Dock
    }

    /// Ctrl+Shift+C: the keyboard lands in the list, on the row it was last on, else the first.
    /// Whether there was a list to land on (an empty crate shows only its hint).
    pub fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let ids = self.ids(cx);
        if ids.is_empty() {
            return false;
        }
        let at = self
            .cursor
            .as_ref()
            .and_then(|c| ids.iter().position(|id| id == c))
            .unwrap_or(0);
        self.cursor = Some(ids[at].clone());
        self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        window.focus(&self.focus.list, cx);
        cx.notify();
        true
    }

    /// Whether the keyboard is on the list or on one of the header's controls.
    pub fn has_focus(&self, window: &Window) -> bool {
        let f = &self.focus;
        [&f.list, &f.save, &f.add_to, &f.clear]
            .iter()
            .any(|h| h.is_focused(window))
    }

    /// The header line: "42 songs · 2 h 51 min", or "8 picked · 31:20" while two or more are
    /// picked.
    pub fn summary(&self, cx: &App) -> SharedString {
        let rows = self.crate_.read(cx).items();
        if self.selection.is_bulk() {
            let picked: Vec<_> = rows
                .iter()
                .filter(|r| self.selection.contains(&r.track.video_id.as_str().to_owned()))
                .collect();
            if picked.len() >= 2 {
                return summary_text(
                    &format!("{} picked", picked.len()),
                    picked.iter().map(|r| r.track.duration),
                )
                .into();
            }
        }
        // With a filter: "3 of 42 songs", and the length of the songs shown.
        let shown = self.shown(cx);
        let label = if self.filtering(cx) {
            filtered_count_text(shown.len(), rows.len())
        } else {
            count_text(rows.len())
        };
        summary_text(&label, shown.iter().map(|r| r.track.duration)).into()
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// How many songs are picked for bulk verbs (tests and diagnostics).
    pub fn picked(&self) -> usize {
        self.selection.len()
    }

    /// The page was left: the cursor and picks are forgotten.
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.cursor = None;
        self.selection.clear();
        self.arrange_open = false;
        if let Some(field) = self.filter.clone() {
            field.update(cx, |f, cx| f.set_text("", cx));
        }
        self.add_model.update(cx, |m, cx| m.reset(cx));
        cx.notify();
    }

    // ---- the add box ---------------------------------------------------------------------

    /// The "Add songs" box (tests).
    pub fn add_box(&self) -> &Entity<CrateAddBox> {
        &self.add
    }

    /// What the add box holds and looks up (tests).
    pub fn add_model(&self) -> &Entity<CrateAddModel> {
        &self.add_model
    }

    /// `/`: the keyboard goes to the add box.
    pub fn focus_add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add.update(cx, |b, cx| b.focus(window, cx));
    }

    /// Ctrl+V on the list or the header's buttons: links and lines on the clipboard are added.
    pub fn paste_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        self.add.update(cx, |b, cx| {
            b.paste(&text, false, window, cx);
        });
    }

    /// Songs went in by search or paste: show the first of them.
    fn scroll_to_added(&mut self, ids: &[String], cx: &mut Context<Self>) {
        let Some(first) = ids.first() else { return };
        let shown = self.shown(cx);
        if let Some(at) = shown.iter().position(|r| r.track.video_id.as_str() == first) {
            self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
        }
    }

    // ---- the filter --------------------------------------------------------------------

    /// The filter field, once the Crate has had songs (tests and the window's history).
    pub fn filter_field(&self) -> Option<&Entity<TextField>> {
        self.filter.as_ref()
    }

    /// What the filter field holds, trimmed; empty when there is no filter.
    fn needle(&self, cx: &App) -> String {
        self.filter
            .as_ref()
            .map(|f| f.read(cx).text().trim().to_owned())
            .unwrap_or_default()
    }

    /// Whether the filter has text: some songs may be hidden.
    pub fn filtering(&self, cx: &App) -> bool {
        !self.needle(cx).is_empty()
    }

    /// The filter field, made on first use. Typing starts the list over from its top; Enter
    /// hands the keyboard to the list.
    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(field) = self.filter.clone() {
            return field;
        }
        let index = if self.docked() { DOCK_TAB_FILTER } else { TAB_FILTER };
        let focus = cx.focus_handle().tab_index(index).tab_stop(true);
        let clear = cx.focus_handle().tab_index(index).tab_stop(true);
        let field = cx.new(|_| TextField::new("Filter", focus, clear));
        cx.subscribe_in(&field, window, |this, _, event, window, cx| match event {
            TextFieldEvent::Changed => {
                this.scroll.scroll_to_item(0, ScrollStrategy::Top);
                cx.notify();
            }
            TextFieldEvent::Submit => window.focus(&this.focus.list, cx),
        })
        .detach();
        self.filter = Some(field.clone());
        field
    }

    /// The songs the filter keeps, in the Crate's order.
    fn shown(&self, cx: &App) -> Vec<CrateRow> {
        let items = self.crate_.read(cx).items();
        crate_arrange::visible(items, &self.needle(cx))
            .into_iter()
            .map(|i| items[i].clone())
            .collect()
    }

    /// The page's field and Arrange button as the header lays them out; `None` until there are
    /// songs.
    fn controls(
        &mut self,
        window: &mut Window,
        accent: Accent,
        total: usize,
        cx: &mut Context<Self>,
    ) -> Option<(AnyElement, AnyElement)> {
        if total == 0 {
            return None;
        }
        let field = self.ensure_filter(window, cx);
        let docked = self.docked();
        let filter = div()
            .debug_selector(move || if docked { "dock-filter" } else { "crate-filter" }.into())
            .when(docked, |d| d.flex_1().min_w_0())
            .when(!docked, |d| d.w(px(FILTER_WIDTH)).flex_none())
            .child(field)
            .into_any_element();
        let can = total >= 2 && !self.filtering(cx);
        Some((filter, self.arrange_control(accent, can, cx)))
    }

    // ---- the Arrange menu ----------------------------------------------------------------

    /// The "Arrange" button and, while it is open, its popover.
    fn arrange_control(&mut self, accent: Accent, can: bool, cx: &mut Context<Self>) -> AnyElement {
        let docked = self.docked();
        let name = if docked { "dock-arrange" } else { "crate-arrange" };
        let button = named(
            name,
            TextButton::new(name, &self.focus.arrange, "Arrange", accent)
                .enabled(can)
                .on_click({
                    let page = cx.entity();
                    move |window, cx| page.update(cx, |p, cx| p.toggle_arrange(window, cx))
                }),
        );
        let panel = (self.arrange_open && can).then(|| self.arrange_menu(cx));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when_some(panel, |d, panel| {
                // A transparent layer, far larger than the window, catches the click that closes
                // the popover (no scrim); the popover paints above the list below.
                let backdrop = div()
                    .id(if docked { "dock-arrange-backdrop" } else { "crate-arrange-backdrop" })
                    .absolute()
                    .top(px(-3000.0))
                    .left(px(-3000.0))
                    .w(px(6000.0))
                    .h(px(6000.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_arrange(window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, window, cx| this.close_arrange(window, cx)),
                    );
                d.child(deferred(backdrop))
                    .child(deferred(panel).with_priority(1))
            })
            .into_any_element()
    }

    fn arrange_menu(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let docked = self.docked();
        let cursor = self.arrange_cursor.min(Arrange::ALL.len() - 1);
        let mut body: Vec<AnyElement> = vec![style::text_style(div(), typography::CAPTION, cx)
            .flex_none()
            .h(px(ITEM_HEIGHT - spacing::S2))
            .px(px(spacing::S3))
            .flex()
            .items_center()
            .text_color(color::text_secondary())
            .child("Sort by")
            .into_any_element()];
        for (ix, how) in Arrange::ALL.into_iter().enumerate() {
            if ix == SORTS {
                body.push(
                    div()
                        .flex_none()
                        .h(px(1.0))
                        .mx(px(spacing::S1))
                        .my(px(spacing::S1))
                        .bg(color::border_subtle())
                        .into_any_element(),
                );
            }
            body.push(self.arrange_item(ix, how, ix == cursor, cx));
        }
        div()
            .id(if docked { "dock-arrange-menu" } else { "crate-arrange-menu" })
            .debug_selector(move || if docked { "dock-arrange-menu" } else { "crate-arrange-menu" }.into())
            .key_context(ARRANGE_CONTEXT)
            .track_focus(&self.focus.arrange_menu)
            .role(gpui::Role::Menu)
            .absolute()
            .top(px(layout::MIN_HIT_TARGET + spacing::S1))
            .right_0()
            .flex()
            .flex_col()
            .w(px(MENU_WIDTH))
            .p(px(spacing::S1))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &ArrangeUp, _, cx| this.step_arrange(-1, cx)))
            .on_action(cx.listener(|this, _: &ArrangeDown, _, cx| this.step_arrange(1, cx)))
            .on_action(cx.listener(|this, _: &ArrangeChoose, window, cx| {
                let at = this.arrange_cursor;
                this.choose_arrange(at, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ArrangeClose, window, cx| this.close_arrange(window, cx)))
            .on_action(cx.listener(|_, _: &ArrangeStay, _, _| {}))
            .children(body)
            .into_any_element()
    }

    fn arrange_item(&mut self, ix: usize, how: Arrange, cursor: bool, cx: &mut Context<Self>) -> AnyElement {
        let name = if self.docked() { "dock-arrange-item" } else { "crate-arrange-item" };
        style::text_style(div(), typography::BODY, cx)
            .id((name, ix))
            .debug_selector(move || format!("{name}-{ix}"))
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(ITEM_HEIGHT))
            .px(px(spacing::S3))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .role(gpui::Role::MenuItem)
            .aria_label(how.label())
            .text_color(color::text_primary())
            .bg(if cursor {
                color::state_hover()
            } else {
                gpui::transparent_black()
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.arrange_cursor != ix {
                    this.arrange_cursor = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.choose_arrange(ix, window, cx)
            }))
            .child(how.label())
            .into_any_element()
    }

    /// Whether the Arrange menu is open.
    pub fn arrange_open(&self) -> bool {
        self.arrange_open
    }

    fn toggle_arrange(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.arrange_open {
            self.close_arrange(window, cx);
            return;
        }
        if self.crate_.read(cx).len() < 2 || self.filtering(cx) {
            return;
        }
        self.arrange_cursor = 0;
        self.arrange_open = true;
        window.focus(&self.focus.arrange_menu, cx);
        cx.notify();
    }

    fn close_arrange(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.arrange_open {
            self.arrange_open = false;
            window.focus(&self.focus.arrange, cx);
            cx.notify();
        }
    }

    fn step_arrange(&mut self, delta: isize, cx: &mut Context<Self>) {
        let last = Arrange::ALL.len() as isize - 1;
        self.arrange_cursor = (self.arrange_cursor as isize + delta).clamp(0, last) as usize;
        cx.notify();
    }

    /// Choose one rewrite. The Crate rewrites at once as one step for Ctrl+Z, and the list shows
    /// its new top.
    fn choose_arrange(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(how) = Arrange::ALL.get(ix).copied() else {
            return;
        };
        self.close_arrange(window, cx);
        if self.filtering(cx) {
            return;
        }
        self.crate_.update(cx, |m, cx| {
            m.arrange(how, cx);
        });
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    // ---- commands ----------------------------------------------------------------------

    /// Hand the player the covers the crate knows, so queued songs have art.
    fn hint_covers(&self, cx: &mut Context<Self>) {
        let rows: Vec<(String, Option<String>)> = self
            .shown(cx)
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
        let tracks: Vec<_> = self.shown(cx).into_iter().map(|r| r.track).collect();
        if tracks.is_empty() {
            return;
        }
        self.hint_covers(cx);
        let start = if shuffle == Some(true) { None } else { Some(start) };
        self.player
            .update(cx, |p, cx| p.play_tracks(tracks, start, shuffle, cx));
    }

    pub fn add_all_to_queue(&mut self, cx: &mut Context<Self>) {
        let tracks: Vec<_> = self.shown(cx).into_iter().map(|r| r.track).collect();
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
        let at = anchor::bounds_of(self.add_to_anchor(), cx);
        self.picker
            .update(cx, |p, cx| p.open_many(video_ids, title, at, window, cx));
    }

    fn add_to_anchor(&self) -> &'static str {
        if self.docked() {
            DOCK_ADD_TO_ANCHOR
        } else {
            ADD_TO_ANCHOR
        }
    }

    /// Clear: no question asked, Ctrl+Z puts the songs back. With a filter, only the songs it
    /// shows go.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.cursor = None;
        self.selection.clear();
        if self.filtering(cx) {
            let shown = self.ids(cx);
            self.crate_.update(cx, |m, cx| {
                m.remove_many(&shown, cx);
            });
        } else {
            self.crate_.update(cx, |m, cx| m.clear(cx));
        }
        cx.notify();
    }

    /// The header's More menu: its verbs act on the picked songs, or on the songs shown when
    /// none is picked (the whole Crate when no filter is on), as Clear does.
    pub fn open_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        let mut picked: Vec<String> = self.selection.members(&ids).into_iter().cloned().collect();
        if picked.is_empty() && self.filtering(cx) {
            if ids.is_empty() {
                return;
            }
            picked = ids;
        }
        let key = if self.docked() { DOCK_MORE_ANCHOR } else { MORE_ANCHOR };
        track_menu::open(
            Subject::Crate(picked),
            track_menu::Anchor::Key(key.into()),
            window,
            cx,
        );
    }

    /// The video ids of the songs shown, top to bottom.
    fn ids(&self, cx: &App) -> Vec<String> {
        self.shown(cx)
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
            .shown(cx)
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
        // A place in the list on screen is not a place in the Crate while a filter hides songs.
        if self.filtering(cx) {
            return;
        }
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

    /// Alt+Home / Alt+End: the row under the cursor, or every picked row, goes to the top or the
    /// bottom of the Crate (a filter does not matter: an end is the same end). The cursor goes
    /// with it.
    fn move_to_edge(&mut self, top: bool, cx: &mut Context<Self>) {
        let ids: Vec<String> = if self.selection.is_bulk() {
            let all = self.ids(cx);
            self.selection.members(&all).into_iter().cloned().collect()
        } else if let Some(video) = self.cursor.clone() {
            vec![video]
        } else {
            return;
        };
        let moved = self.crate_.update(cx, |m, cx| m.move_to_edge(&ids, top, cx));
        if moved {
            if let Some(ix) = self.cursor_index(cx) {
                self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
            }
            cx.notify();
        }
    }

    /// A dragged row was let go before slot `slot` (`len` is the end): the crate reorders. The
    /// row keeps the keyboard cursor with it.
    pub fn drop_row(&mut self, from: usize, slot: usize, cx: &mut Context<Self>) {
        if self.filtering(cx) {
            return;
        }
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

    /// Songs let go on the docked list before slot `slot` (`len` is the end): each lands there in
    /// the order dragged, and one the crate already holds moves there instead.
    fn drop_songs(&mut self, songs: &[TrackSubject], slot: usize, cx: &mut Context<Self>) {
        // With a filter the drop point is not a place in the Crate: the songs go to the bottom.
        let slot = if self.filtering(cx) { usize::MAX } else { slot };
        let songs: Vec<(apollo_domain::model::Track, Option<String>)> = songs
            .iter()
            .map(|s| {
                let thumbnail = s
                    .thumbnail
                    .clone()
                    .or_else(|| self.player.read(cx).art_url(s.track.video_id.as_str()));
                (s.track.clone(), thumbnail)
            })
            .collect();
        self.crate_.update(cx, |m, cx| m.insert_at(songs, slot, cx));
        cx.notify();
    }

    /// Make `el` take dropped songs before slot `slot`: a row of the crate (arranging), and in the
    /// docked list a song dragged from a playlist, an album, the Songs tab or Search.
    fn with_drops<E: InteractiveElement>(
        &self,
        el: E,
        slot: usize,
        cx: &mut Context<Self>,
    ) -> E {
        let el = el.on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
            this.drop_row(dragged.from, slot, cx)
        }));
        if !self.docked() {
            return el;
        }
        el.on_drop(cx.listener(move |this, dragged: &SongDrag, _, cx| {
            this.drop_songs(&dragged.songs, slot, cx)
        }))
        .on_drop(cx.listener(move |this, dragged: &LibraryRow, _, cx| {
            if let Some(song) = &dragged.song {
                this.drop_songs(&song.songs, slot, cx)
            }
        }))
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
            .map(|r| TrackSubject::of_crate_row(r.track.clone(), r.thumbnail.clone()))
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
            m.remove_many(&picked, cx);
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
            let Some(track) = self.shown(cx).get(ix).map(|r| r.track.clone()) else {
                return;
            };
            vec![crate::ui::playlist_picker::PickerSong::new(
                track.video_id.as_str(),
                track.title.clone(),
            )]
        };
        crate::ui::playlist_picker::open_songs(songs, None, window, cx);
    }

    /// The A key: every picked song, or the one under the cursor, to the target playlist.
    fn target_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let Some(track) = self.shown(cx).get(ix).map(|r| r.track.clone()) else {
                return;
            };
            vec![crate::ui::playlist_picker::PickerSong::new(
                track.video_id.as_str(),
                track.title.clone(),
            )]
        };
        // With the key acting on one song and a target set it goes straight in, so the cursor
        // steps on; with none the picker opens and the cursor stays.
        let direct = !songs.is_empty()
            && !self.selection.is_bulk()
            && crate::target_playlist::has_target(cx);
        crate::target_playlist::add_songs(songs, None, window, cx);
        if direct {
            self.advance(cx);
        }
    }

    /// After A acted on the cursor row: the cursor steps to the next row (Settings > Playlists),
    /// the last row staying put.
    fn advance(&mut self, cx: &mut Context<Self>) {
        let ids = self.ids(cx);
        let Some(at) = self.cursor_index(cx) else {
            return;
        };
        if let Some(next) = crate::auto_advance::next_row(at, ids.len(), cx) {
            self.cursor = Some(ids[next].clone());
            self.scroll.scroll_to_item(next, ScrollStrategy::Nearest);
            cx.notify();
        }
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
        let slot = div()
            .debug_selector(|| "crate-end".into())
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .group(group.clone())
            .child(drop_line(group, self.docked()));
        self.with_drops(slot, ix, cx).into_any_element()
    }

    fn row(&self, ix: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(ix).cloned() else {
            return self.end_slot(ix, cx);
        };
        let docked = self.docked();
        // Names that tell the docked list's rows from the page's, so tests find the right ones.
        let (name, row_id, like_id, remove_id, explicit_id) = if docked {
            ("dock", "dock-row", "dock-like", "dock-remove", "dock-explicit")
        } else {
            ("crate", "crate-row", "crate-like", "crate-remove", "crate-explicit")
        };
        let video: String = row.track.video_id.as_str().to_owned();
        let playing = self.player.read(cx).current_video().as_deref() == Some(video.as_str());
        let list_focused = self.focus.list.is_focused(window);
        let at_cursor = list_focused && self.cursor.as_deref() == Some(video.as_str());
        let ring = at_cursor && keyboard_focused(&self.focus.list, window);
        let accent = color::accent(self.accent);
        let tint = selection::row_tint(self.accent);
        let selected = playing || at_cursor;
        let group: SharedString = format!("{name}-row-{ix}").into();
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
        let picked = self.selection.contains(&video);
        // A picked row speaks for every picked row.
        let subject = match &self.bulk_menu {
            Some(bulk) if picked => bulk.clone(),
            _ => Subject::crate_track(row.track.clone(), row.thumbnail.clone()),
        };
        let more = more_slot(format!("{name}-more-{ix}"), true, subject.clone())
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            });
        let remove = div()
            .debug_selector(move || format!("{name}-remove-{ix}"))
            .flex_none()
            .size(px(ACTION_SIZE))
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .child(row_action(
                (remove_id, ix),
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

        // The docked list, like the queue panel, shows More and Remove in the place of the
        // duration (and only while the pointer or the cursor is on the row), so the title keeps
        // its width in 320px.
        let row_tint = if picked {
            tint
        } else if selected {
            playing_row::tint()
        } else {
            color::state_hover()
        };
        let duration_cell = if docked {
            let duration = style::tabular(style::text_style(div(), typography::CAPTION, cx))
                .absolute()
                .right_0()
                .h(px(ACTION_SIZE))
                .flex()
                .items_center()
                .text_color(color::text_tertiary())
                .when(!at_cursor, |d| d.group_hover(group.clone(), |s| s.opacity(0.0)))
                .when(at_cursor, |d| d.opacity(0.0))
                .child(duration);
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
                        .child(more)
                        .child(remove),
                );
            vec![div()
                .relative()
                .flex_none()
                .w(px(DURATION_WIDTH))
                .h(px(ACTION_SIZE))
                .child(duration)
                .child(actions)
                .into_any_element()]
        } else {
            vec![
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
                    ))
                    .into_any_element(),
                more.into_any_element(),
                remove.into_any_element(),
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
            // A song just set aside lights up where it landed.
            .children(
                crate::confirm::row_pulse(crate::confirm::Place::Crate, &video, cx)
                    .map(|seq| crate::confirm::highlight(seq, radius::SMALL)),
            )
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
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(spacing::S1))
                            .min_w_0()
                            // The service's "E", as on the other song lists.
                            .when(row.explicit, |d| {
                                d.child(
                                    style::text_style(div(), typography::CAPTION, cx)
                                        .id((explicit_id, ix))
                                        .debug_selector(move || format!("{name}-explicit-{ix}"))
                                        .flex_none()
                                        .text_color(color::text_tertiary())
                                        .aria_label("Explicit")
                                        .child("E"),
                                )
                            })
                            .child(div().min_w_0().overflow_hidden().child(text_line(
                                "artist",
                                artists,
                                typography::BODY,
                                color::text_secondary(),
                                cx,
                            ))),
                    ),
            )
            .child(heart)
            .children(duration_cell)
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)))
            // With a filter hiding songs a row's place on screen is not its place in the Crate:
            // rows cannot be dragged.
            .when(!self.filtering(cx), |d| {
                d.on_drag(
                    DraggedRow {
                        from: ix,
                        title: title.clone(),
                        artist: artists_line.clone(),
                    },
                    |dragged, _, _, cx| cx.new(|_| DragGhost(dragged.clone())),
                )
            });

        let surface = track_menu::on_context_menu(surface, subject);
        // Any row takes a dragged row's place: the line above it shows where it would land.
        let drop_group: SharedString = format!("crate-drop-{ix}").into();
        let wrapper = div()
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .py(px(ROW_INSET))
            .when(docked, |d| d.px(px(spacing::S2)))
            .group(drop_group.clone())
            .child(drop_line(drop_group, docked))
            .child(surface);
        self.with_drops(wrapper, ix, cx).into_any_element()
    }

    // ---- page --------------------------------------------------------------------------

    fn header(
        &self,
        count: usize,
        controls: Option<(AnyElement, AnyElement)>,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = cx.entity();
        let has_rows = count > 0;
        let buttons = div()
            .key_context(BAR_CONTEXT)
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
            .child(named(
                "crate-clear",
                TextButton::new("crate-clear", &self.focus.clear, "Clear", accent)
                    .enabled(has_rows)
                    .on_click({
                        let page = page.clone();
                        move |_, cx| page.update(cx, |p, cx| p.clear(cx))
                    }),
            ))
            // Swap to explicit / clean versions live in this menu.
            .child(
                div().debug_selector(|| "crate-more".into()).child(anchor::anchored(
                    MORE_ANCHOR,
                    IconButton::new(
                        "crate-more",
                        &self.focus.more,
                        Icon::MoreHorizontal,
                        "More",
                        accent,
                    )
                    .enabled(has_rows)
                    .on_click({
                        let page = page.clone();
                        move |window, cx| page.update(cx, |p, cx| p.open_more(window, cx))
                    }),
                )),
            )
            // "N selected" at the right end while 2 or more songs are picked.
            .child(
                div()
                    .ml_auto()
                    .debug_selector(|| "crate-selected-count".into())
                    .child(selection::count_slot(self.selection.len(), cx)),
            );
        // The filter and Arrange sit at the right end of the title's row.
        let controls = controls.map(|(filter, arrange)| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .gap(px(spacing::S2))
                .child(filter)
                .child(arrange)
        });
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(spacing::S4))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                style::text_style(div(), typography::PAGE_TITLE, cx)
                                    .text_color(color::text_primary())
                                    .child("Crate"),
                            )
                            .child(
                                style::text_style(div(), typography::BODY, cx)
                                    .id("crate-count")
                                    .debug_selector(|| "crate-count".into())
                                    // A non-breaking space keeps the line's height while the crate is empty.
                                    .text_color(color::text_secondary())
                                    .child(if has_rows {
                                        self.summary(cx)
                                    } else {
                                        "\u{a0}".into()
                                    }),
                            ),
                    )
                    .children(controls),
            )
            .child(buttons)
            .into_any_element()
    }

    /// The docked panel's top: the Queue · Crate control, then Save as playlist… and Add all
    /// to… (and how many songs are picked, at the right end).
    fn dock_header(
        &self,
        count: usize,
        controls: Option<(AnyElement, AnyElement)>,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = cx.entity();
        let has_rows = count > 0;
        let tabs = self
            .dock_tabs
            .clone()
            .map(|(focus, on_pick)| DockTabs::new(Dock::Crate, &focus, accent, on_pick));
        let actions = div()
            .key_context(BAR_CONTEXT)
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .gap(px(spacing::S1))
            .h(px(layout::MIN_HIT_TARGET + spacing::S2))
            .px(px(spacing::S3))
            .child(named(
                "dock-save-playlist",
                TextButton::new(
                    "dock-save-playlist",
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
                "dock-add-to-playlist",
                anchor::anchored(
                    DOCK_ADD_TO_ANCHOR,
                    TextButton::new(
                        "dock-add-to-playlist",
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
            .child(
                div().debug_selector(|| "dock-more".into()).child(anchor::anchored(
                    DOCK_MORE_ANCHOR,
                    IconButton::new(
                        "dock-more",
                        &self.focus.more,
                        Icon::MoreHorizontal,
                        "More",
                        accent,
                    )
                    .enabled(has_rows)
                    .on_click({
                        let page = page.clone();
                        move |window, cx| page.update(cx, |p, cx| p.open_more(window, cx))
                    }),
                )),
            )
            .child(
                div()
                    .ml_auto()
                    .debug_selector(|| "dock-selected-count".into())
                    .child(selection::count_slot(self.selection.len(), cx)),
            );
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            // Songs set aside light the header up (the pulse is one fade of bg.overlay).
            .children(
                crate::confirm::pulse(crate::confirm::Place::Crate, cx)
                    .map(|seq| crate::confirm::highlight(seq, 0.0)),
            )
            .children(tabs)
            .child(actions)
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .id("dock-count")
                    .debug_selector(|| "dock-count".into())
                    .px(px(spacing::S3))
                    .pb(px(spacing::S2))
                    .truncate()
                    .text_color(color::text_secondary())
                    // A non-breaking space keeps the line's height while the crate is empty.
                    .child(if has_rows {
                        self.summary(cx)
                    } else {
                        "\u{a0}".into()
                    }),
            )
            // The filter, and Arrange at its right.
            .children(controls.map(|(filter, arrange)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .flex_none()
                    .gap(px(spacing::S2))
                    .h(px(layout::MIN_HIT_TARGET + spacing::S2))
                    .px(px(spacing::S3))
                    .child(filter)
                    .child(arrange)
            }))
            .into_any_element()
    }

    /// The docked list with nothing in it: the hint, and a place to drop the first song.
    fn dock_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let hover = |s: gpui::StyleRefinement| s.bg(color::state_hover());
        let body = div()
            .id("crate-dock-empty")
            .debug_selector(|| "crate-dock-empty".into())
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(spacing::S4))
            .pt(px(spacing::S3))
            .flex()
            .flex_col()
            .gap(px(spacing::S1))
            .drag_over::<DraggedRow>(move |s, _, _, _| hover(s))
            .drag_over::<SongDrag>(move |s, _, _, _| hover(s))
            .drag_over::<LibraryRow>(move |s, _, _, _| hover(s))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child("Nothing set aside yet"),
            )
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_tertiary())
                    .child("Drag songs here, or press C."),
            );
        self.with_drops(body, 0, cx).into_any_element()
    }

    /// The filter keeps no song: say so, and offer to clear it.
    fn no_matches(&self, accent: Accent, cx: &mut Context<Self>) -> AnyElement {
        let page = cx.entity();
        div()
            .debug_selector(|| "crate-no-matches".into())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .when(self.docked(), |d| d.px(px(spacing::S4)))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child("No songs match"),
            )
            .child(
                div().debug_selector(|| "crate-clear-filter".into()).child(
                    TextButton::new("crate-clear-filter", &self.focus.search, "Clear filter", accent)
                        .on_click(move |_, cx| {
                            page.update(cx, |p, cx| {
                                if let Some(field) = p.filter.clone() {
                                    field.update(cx, |f, cx| f.set_text("", cx));
                                }
                            })
                        }),
                ),
            )
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.accent = self.player.read(cx).view().accent;
        let total = self.crate_.read(cx).len();
        let controls = self.controls(window, self.accent, total, cx);
        self.rows = self.shown(cx);
        self.refresh_selection();
        let accent = self.accent;
        let count = self.rows.len();

        let docked = self.docked();
        self.add.update(cx, |a, _| a.set_accent(accent));
        let add = self.add.clone();
        let header = if docked {
            self.dock_header(count, controls, accent, cx)
        } else {
            self.header(count, controls, accent, cx)
        };
        let body: AnyElement = if total == 0 && docked {
            self.dock_empty(cx)
        } else if total == 0 {
            self.empty(accent, cx)
        } else if count == 0 {
            self.no_matches(accent, cx)
        } else {
            div()
                .id(if docked { "crate-dock-list" } else { "crate-list" })
                .key_context(if docked { "CrateList Docked" } else { KEY_CONTEXT })
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
                .on_action(cx.listener(|this, _: &ClearPicks, window, cx| this.clear_picks(window, cx)))
                .on_action(cx.listener(|this, _: &PlayCursor, _, cx| this.play_cursor(cx)))
                .on_action(cx.listener(|this, _: &RemoveCursor, _, cx| this.remove_cursor(cx)))
                .on_action(cx.listener(|this, _: &LikeCursor, window, cx| this.like_cursor(window, cx)))
                .on_action(cx.listener(|this, _: &PlaylistCursor, window, cx| {
                    this.playlist_cursor(window, cx)
                }))
                .on_action(cx.listener(|this, _: &TargetCursor, window, cx| {
                    this.target_cursor(window, cx)
                }))
                .on_action(cx.listener(|this, _: &MoveCursorUp, _, cx| this.move_cursor(-1, cx)))
                .on_action(cx.listener(|this, _: &MoveCursorDown, _, cx| this.move_cursor(1, cx)))
                .on_action(cx.listener(|this, _: &CrateMoveTop, _, cx| this.move_to_edge(true, cx)))
                .on_action(cx.listener(|this, _: &CrateMoveBottom, _, cx| this.move_to_edge(false, cx)))
                .size_full()
                .child(
                    uniform_list(
                        if docked { "crate-dock-rows" } else { "crate-rows" },
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

        if docked {
            return div()
                .id("crate-dock")
                .debug_selector(|| "crate-dock".into())
                .flex()
                .flex_col()
                .flex_none()
                .w(px(layout::QUEUE_PANEL_WIDTH))
                .h_full()
                .min_h_0()
                .bg(color::bg_raised())
                .border_l_1()
                .border_color(color::border_subtle())
                .on_action(cx.listener(|this, _: &CrateFocusAdd, window, cx| this.focus_add(window, cx)))
                .on_action(cx.listener(|this, _: &CratePaste, window, cx| this.paste_clipboard(window, cx)))
                .child(header)
                .child(
                    div()
                        .flex_none()
                        .px(px(spacing::S3))
                        .pb(px(spacing::S2))
                        .child(add),
                )
                .child(div().flex_1().min_h_0().child(body))
                .into_any_element();
        }
        div()
            .id("crate-page")
            .debug_selector(|| "crate-page".into())
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .on_action(cx.listener(|this, _: &CrateFocusAdd, window, cx| this.focus_add(window, cx)))
            .on_action(cx.listener(|this, _: &CratePaste, window, cx| this.paste_clipboard(window, cx)))
            .child(header)
            .child(div().flex_none().mt(px(spacing::S4)).child(add))
            .child(div().flex_1().min_h_0().mt(px(spacing::S4)).child(body))
            .into_any_element()
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
    fn lengths_read_as_minutes_then_hours() {
        let secs = Duration::from_secs;
        assert_eq!(length_text(secs(0)), "0:00");
        assert_eq!(length_text(secs(31 * 60 + 20)), "31:20");
        assert_eq!(length_text(secs(59 * 60 + 59)), "59:59");
        assert_eq!(length_text(secs(3600)), "1 h");
        assert_eq!(length_text(secs(2 * 3600 + 51 * 60 + 10)), "2 h 51 min");
        assert_eq!(length_text(secs(2 * 3600 + 5 * 60)), "2 h 5 min");
        // Rounded to the minute, carrying into the hour.
        assert_eq!(length_text(secs(3600 + 59 * 60 + 40)), "2 h");
    }

    #[test]
    fn the_summary_adds_the_known_lengths_and_names_the_unknown() {
        let m = |m: u64| Some(Duration::from_secs(m * 60));
        assert_eq!(summary_text("3 songs", [m(10), m(20), m(1)]), "3 songs \u{b7} 31:00");
        assert_eq!(
            summary_text("42 songs", (0..42).map(|_| m(4)).chain([None, None])),
            "42 songs \u{b7} 2 h 48 min + 2 unknown"
        );
        assert_eq!(summary_text("8 picked", [m(31), None]), "8 picked \u{b7} 31:00 + 1 unknown");
        // Nothing known: just the label.
        assert_eq!(summary_text("2 songs", [None, None]), "2 songs");
        assert_eq!(summary_text("1 song", []), "1 song");
    }

    #[test]
    fn the_count_reads_naturally() {
        assert_eq!(count_text(1), "1 song");
        assert_eq!(count_text(12), "12 songs");
    }
}
