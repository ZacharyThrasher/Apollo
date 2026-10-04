//! Queue panel: a 320px right panel with what has played, what is playing and what is up next.
//!
//! The list is virtual (a queue can hold thousands of tracks) and keyboard-first: the list is
//! one tab stop with an arrow-key cursor. Enter plays, L likes, Delete removes, Alt+Up/Down reorders;
//! the mouse can click a row to play it, drag an up-next row to reorder, or use the row's
//! heart and remove button.

use std::ops::Range;
use std::rc::Rc;

use apollo_domain::model::QueueEntryId;
use apollo_domain::queue::Repeat;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, uniform_list, AnyElement, App, AppContext, ClickEvent, Context,
    Entity, FocusHandle, Focusable, IntoElement, KeyBinding, KeyboardButton, MouseButton,
    ParentElement, Render, ScrollStrategy, SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::assets::Icon;
use crate::library::LibraryWrites;
use crate::player::{format_time, PlayerModel, QueueHeading, QueueRow, QueueView, Status};
use crate::queue_source;
use crate::url_thumbs::{Thumb, ThumbStore};
use crate::style::{self, color};
use crate::ui::art_tile::art_tile;
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::pane_tabs::{OnPick, Pane, PaneFocus, PaneTabs};
use crate::ui::player_bar::text_line;
use crate::ui::playing_row;
use crate::ui::row_actions::{heart_action, more_slot, ACTION_SIZE};
use crate::ui::search_page::OpenTarget;
use crate::ui::switch::LabeledSwitch;
use crate::ui::track_menu::{self, Subject};
use crate::ui::tooltip::Tooltip;
use crate::ui::transport::on_off;

actions!(
    apollo_queue,
    [
        CursorUp,
        CursorDown,
        PlayCursor,
        RemoveCursor,
        LikeCursor,
        MoveCursorUp,
        MoveCursorDown
    ]
);

/// Key context of the list's focus handle.
pub const KEY_CONTEXT: &str = "QueueList";

pub const ROW_HEIGHT: f32 = 56.0;
/// Row content sits inside the row with a 4px margin, so the focus ring (2px + 2px offset)
/// fits between neighbouring rows.
const ROW_INSET: f32 = spacing::S1;
const ART: f32 = 40.0;
const DROP_LINE: f32 = 2.0;
const DURATION_WIDTH: f32 = spacing::S12;
/// The "Playing from" row under the header: a 32px cover, two lines of text, the Autoplay switch.
const SOURCE_ROW: f32 = 48.0;
const SOURCE_ART: f32 = 32.0;

/// Tab order: after the page, before the player bar.
const TAB_SHUFFLE: isize = 13;
const TAB_REPEAT: isize = 14;
const TAB_CLEAR: isize = 15;
/// Shares Clear's index: the source link follows it in the tree, and the indices up to the
/// player bar's are all taken.
const TAB_SOURCE: isize = TAB_CLEAR;
const TAB_AUTOPLAY: isize = 16;
const TAB_LIST: isize = 17;
const TAB_SEARCH: isize = 18;

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", CursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", CursorDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", PlayCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("delete", RemoveCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("backspace", RemoveCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("l", LikeCursor, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-up", MoveCursorUp, Some(KEY_CONTEXT)),
        KeyBinding::new("alt-down", MoveCursorDown, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    History,
    UpNext,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::History => "History",
            Section::UpNext => "Up next",
        }
    }
}

/// One line of the list, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Header(Section),
    History(usize),
    Current,
    Upcoming(usize),
    /// Nothing left up next.
    Empty,
}

/// The lines for a queue: history (with its header) when there is any, what is playing, then
/// up next (with its header), or a single empty line when nothing is queued.
pub fn flatten(view: &QueueView) -> Vec<Item> {
    let mut items = Vec::new();
    if !view.history.is_empty() {
        items.push(Item::Header(Section::History));
        items.extend((0..view.history.len()).map(Item::History));
    }
    if view.current.is_some() {
        items.push(Item::Current);
    }
    if view.current.is_some() || !view.upcoming.is_empty() {
        items.push(Item::Header(Section::UpNext));
    }
    if view.upcoming.is_empty() {
        items.push(Item::Empty);
    } else {
        items.extend((0..view.upcoming.len()).map(Item::Upcoming));
    }
    items
}

impl QueueView {
    fn row(&self, item: Item) -> Option<&QueueRow> {
        match item {
            Item::History(i) => self.history.get(i),
            Item::Current => self.current.as_ref(),
            Item::Upcoming(i) => self.upcoming.get(i),
            Item::Header(_) | Item::Empty => None,
        }
    }

    fn row_by_id(&self, id: QueueEntryId) -> Option<&QueueRow> {
        self.history
            .iter()
            .chain(self.current.iter())
            .chain(self.upcoming.iter())
            .find(|r| r.id == id)
    }

    /// Entry ids in the order the cursor visits them.
    fn ordered_ids(&self) -> Vec<QueueEntryId> {
        self.history
            .iter()
            .chain(&self.current)
            .chain(&self.upcoming)
            .map(|r| r.id)
            .collect()
    }
}

/// Where the cursor lands after a removal: the next row, else the previous one.
fn cursor_after_removal(ids: &[QueueEntryId], removed: QueueEntryId) -> Option<QueueEntryId> {
    let pos = ids.iter().position(|id| *id == removed)?;
    ids.get(pos + 1)
        .or_else(|| pos.checked_sub(1).and_then(|p| ids.get(p)))
        .copied()
}

/// The dragged up-next row.
#[derive(Clone)]
struct DraggedEntry {
    id: QueueEntryId,
    title: SharedString,
    artist: SharedString,
}

struct DragGhost(DraggedEntry);

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // On bg.overlay, so the lowest text tier is text.secondary.
        div()
            .w(px(layout::QUEUE_PANEL_WIDTH - 2.0 * spacing::S2))
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

pub struct QueuePanel {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    focus_shuffle: FocusHandle,
    focus_repeat: FocusHandle,
    focus_clear: FocusHandle,
    focus_autoplay: FocusHandle,
    focus_source: FocusHandle,
    focus_list: FocusHandle,
    focus_search: FocusHandle,
    scroll: UniformListScrollHandle,
    /// The row the keyboard acts on; shown only while the list has focus.
    cursor: Option<QueueEntryId>,
    /// Copied at each render for the list's row callback.
    view: QueueView,
    items: Vec<Item>,
    accent: Accent,
    on_search: Rc<Handler>,
    thumbs: Entity<ThumbStore>,
    /// Opens the page the "Playing from" name stands for.
    on_open: OpenTarget,
    /// Now Playing's pane control, drawn above the header while Now Playing is open (the control
    /// is then the panel's title).
    tabs: Option<(PaneFocus, OnPick)>,
    tabs_visible: bool,
}

impl QueuePanel {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        thumbs: Entity<ThumbStore>,
        on_search: impl Fn(&mut Window, &mut App) + 'static,
        on_open: OpenTarget,
        cx: &mut Context<Self>,
    ) -> Self {
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        Self {
            focus_shuffle: stop(cx, TAB_SHUFFLE),
            focus_repeat: stop(cx, TAB_REPEAT),
            focus_clear: stop(cx, TAB_CLEAR),
            focus_autoplay: stop(cx, TAB_AUTOPLAY),
            focus_source: stop(cx, TAB_SOURCE),
            focus_list: stop(cx, TAB_LIST),
            focus_search: stop(cx, TAB_SEARCH),
            player,
            writes,
            scroll: UniformListScrollHandle::new(),
            cursor: None,
            view: QueueView::default(),
            items: Vec::new(),
            accent: Accent::fallback(),
            on_search: Rc::new(Box::new(on_search)),
            thumbs,
            on_open,
            tabs: None,
            tabs_visible: false,
        }
    }

    /// The control that switches Now Playing's right pane, shown while [`Self::set_tabs_visible`].
    pub fn set_tabs(&mut self, focus: PaneFocus, on_pick: OnPick) {
        self.tabs = Some((focus, on_pick));
    }

    /// Whether the pane control sits above the header (Now Playing is open).
    pub fn set_tabs_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.tabs_visible != visible {
            self.tabs_visible = visible;
            cx.notify();
        }
    }

    pub fn cursor(&self) -> Option<QueueEntryId> {
        self.cursor
    }

    /// Scroll so what is playing sits at the top, with the history above it.
    pub fn reveal_current(&mut self, cx: &mut Context<Self>) {
        let items = flatten(&self.player.read(cx).queue_view());
        if let Some(ix) = items.iter().position(|i| *i == Item::Current) {
            self.scroll.scroll_to_item_strict(ix, ScrollStrategy::Top);
            cx.notify();
        }
    }

    fn fresh_view(&self, cx: &App) -> QueueView {
        self.player.read(cx).queue_view()
    }

    fn scroll_to(&self, view: &QueueView, id: QueueEntryId) {
        if let Some(ix) = flatten(view)
            .iter()
            .position(|item| view.row(*item).is_some_and(|r| r.id == id))
        {
            self.scroll.scroll_to_item(ix, ScrollStrategy::Nearest);
        }
    }

    // ---- keyboard ----------------------------------------------------------------------

    /// The cursor, or the playing row when the keyboard has not chosen one yet.
    fn cursor_or_current(&self, view: &QueueView) -> Option<QueueEntryId> {
        let ids = view.ordered_ids();
        self.cursor
            .filter(|c| ids.contains(c))
            .or_else(|| view.current.as_ref().map(|r| r.id))
            .or_else(|| ids.first().copied())
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let view = self.fresh_view(cx);
        let ids = view.ordered_ids();
        if ids.is_empty() {
            return;
        }
        let at = self
            .cursor_or_current(&view)
            .and_then(|c| ids.iter().position(|id| *id == c))
            .unwrap_or(0);
        // The first press only shows the cursor on the playing row.
        let first = self.cursor.is_none();
        let to = if first {
            at
        } else {
            (at as isize + delta).clamp(0, ids.len() as isize - 1) as usize
        };
        self.cursor = Some(ids[to]);
        self.scroll_to(&view, ids[to]);
        cx.notify();
    }

    fn play_cursor(&mut self, cx: &mut Context<Self>) {
        let view = self.fresh_view(cx);
        let Some(id) = self.cursor.filter(|c| view.ordered_ids().contains(c)) else {
            return;
        };
        if view.current.as_ref().is_some_and(|r| r.id == id) {
            self.player.update(cx, |m, cx| m.toggle_play(cx));
        } else {
            self.player.update(cx, |m, cx| m.play_entry(id, cx));
        }
        cx.notify();
    }

    fn remove(&mut self, id: QueueEntryId, cx: &mut Context<Self>) {
        let view = self.fresh_view(cx);
        // The playing row is not removable from here: removing it would leave it on screen
        // and playing, which reads as a bug.
        if view.current.as_ref().is_some_and(|r| r.id == id) {
            return;
        }
        if self.cursor == Some(id) {
            self.cursor = cursor_after_removal(&view.ordered_ids(), id);
        }
        self.player.update(cx, |m, cx| m.remove_entry(id, cx));
        cx.notify();
    }

    fn remove_cursor(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.cursor {
            self.remove(id, cx);
        }
    }

    /// Like or unlike the track under the cursor.
    fn like_cursor(&mut self, cx: &mut Context<Self>) {
        let view = self.fresh_view(cx);
        let Some(id) = self.cursor else {
            return;
        };
        let video = view.row_by_id(id).map(|r| r.video_id.clone());
        if let Some(video) = video {
            self.writes.update(cx, |w, cx| w.toggle_like(&video, cx));
        }
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let view = self.fresh_view(cx);
        let Some(id) = self.cursor else {
            return;
        };
        let Some(ix) = view.upcoming.iter().position(|r| r.id == id) else {
            return;
        };
        let to = ix as isize + delta;
        if to < 0 || to >= view.upcoming.len() as isize {
            return;
        }
        self.player
            .update(cx, |m, cx| m.move_upcoming(id, to as usize, cx));
        let moved = self.fresh_view(cx);
        self.scroll_to(&moved, id);
        cx.notify();
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
            .filter_map(|ix| self.view.row(self.items[ix]))
            .map(|r| r.video_id.clone())
            .collect();
        if !ids.is_empty() {
            self.player.update(cx, |m, cx| m.request_thumbs(ids, cx));
        }
        range.map(|ix| self.row(ix, window, cx)).collect()
    }

    fn row(&self, ix: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.items[ix] {
            Item::Header(section) => self.header_row(section, cx).into_any_element(),
            Item::Empty => self.empty_row(window, cx).into_any_element(),
            item => self.entry_row(ix, item, window, cx).into_any_element(),
        }
    }

    fn header_row(&self, section: Section, cx: &App) -> impl IntoElement {
        style::text_style(div(), typography::BODY_STRONG, cx)
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .flex()
            .items_end()
            .pb(px(spacing::S2))
            .text_color(color::text_secondary())
            .child(section.label())
    }

    fn empty_row(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ring = keyboard_focused(&self.focus_search, window);
        let accent = color::accent(self.accent);
        let on_search = self.on_search.clone();
        div()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .flex()
            .items_center()
            .gap(px(spacing::S3))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child("Nothing up next"),
            )
            .child(
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id("queue-search")
                    .relative()
                    .px(px(spacing::S2))
                    .rounded(px(radius::SMALL))
                    .cursor_pointer()
                    .track_focus(&self.focus_search)
                    .role(gpui::Role::Button)
                    .aria_label("Search")
                    .text_color(color::text_primary())
                    .hover(|s| s.underline())
                    .on_click(move |event, window, cx| {
                        if let ClickEvent::Keyboard(k) = event {
                            if k.button == KeyboardButton::Space {
                                return;
                            }
                        }
                        on_search(window, cx)
                    })
                    .child("Search")
                    .when(ring, |d| d.child(focus_ring(accent, radius::SMALL))),
            )
    }

    fn entry_row(
        &self,
        ix: usize,
        item: Item,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let row = self
            .view
            .row(item)
            .expect("entry items point at rows")
            .clone();
        let id = row.id;
        let is_current = item == Item::Current;
        let is_history = matches!(item, Item::History(_));
        let upcoming_ix = match item {
            Item::Upcoming(i) => Some(i),
            _ => None,
        };
        let list_focused = self.focus_list.is_focused(window);
        let at_cursor = list_focused && self.cursor == Some(id);
        let ring = at_cursor && keyboard_focused(&self.focus_list, window);
        let accent = color::accent(self.accent);
        let selected = is_current || at_cursor;
        let group: SharedString = format!("queue-row-{ix}").into();
        let thumb = self.player.read(cx).thumb(&row.video_id);

        let title_style = playing_row::title_style(typography::BODY, is_current);
        // Played rows sit back: their title drops a tier.
        let title_color = if is_history {
            color::text_secondary()
        } else {
            color::text_primary()
        };

        let duration = style::tabular(style::text_style(div(), typography::CAPTION, cx))
            .absolute()
            .right_0()
            .h(px(layout::MIN_HIT_TARGET))
            .flex()
            .items_center()
            .text_color(color::text_tertiary())
            .when(!at_cursor, |d| {
                d.group_hover(group.clone(), |s| s.opacity(0.0))
            })
            .when(at_cursor, |d| d.opacity(0.0))
            .child(row.duration_ms.map(format_time).unwrap_or_default());
        let remove = div()
            .id(("queue-remove", ix))
            .debug_selector(|| format!("queue-remove-{ix}"))
            .flex_none()
            .size(px(layout::MIN_HIT_TARGET))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .hover(|s| s.bg(color::state_hover()))
            .role(gpui::Role::Button)
            .aria_label("Remove")
            // Keep the row's own click (play) from firing too.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                window.focus(&this.focus_list, cx);
                this.remove(id, cx);
            }))
            .child(
                gpui::svg()
                    .path(Icon::Dismiss.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(color::text_secondary()),
            );

        // The heart: room is always reserved; it shows on hover or under the cursor, and stays
        // once the track is liked.
        let liked = self.writes.read(cx).is_liked(&row.video_id);
        let heart = div()
            .debug_selector(|| format!("queue-like-{ix}"))
            .flex_none()
            .size(px(ACTION_SIZE))
            .when(!(liked || at_cursor), |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .child(heart_action(("queue-like", ix), liked, {
                let (writes, video) = (self.writes.clone(), row.video_id.clone());
                move |_, cx| writes.update(cx, |w, cx| w.toggle_like(&video, cx))
            }));

        // More (and Remove) take the place of the duration while the pointer is on the row or
        // the cursor is on it, so the title keeps its full width at rest. The playing row has
        // no remove, so its menu has none either.
        let menu = if is_current {
            Subject::track(row.track.clone(), None)
        } else {
            Subject::queued(row.track.clone(), id)
        };
        let row_tint = if selected {
            playing_row::tint()
        } else {
            color::state_hover()
        };
        let actions = div()
            .absolute()
            .right_0()
            .flex()
            .flex_row()
            .rounded(px(radius::SMALL))
            // Opaque, so the title never shows through the buttons.
            .bg(color::bg_raised())
            .when(!at_cursor, |d| {
                d.opacity(0.0).group_hover(group.clone(), |s| s.opacity(1.0))
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .rounded(px(radius::SMALL))
                    .bg(row_tint)
                    .child(more_slot(format!("queue-more-{ix}"), true, menu.clone()))
                    .when(!is_current, |d| d.child(remove)),
            );

        let surface = div()
            .id(("queue-row", ix))
            .debug_selector(|| format!("queue-row-{ix}"))
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
            .hover(move |s| {
                s.bg(if selected {
                    playing_row::tint()
                } else {
                    color::state_hover()
                })
            })
            .role(gpui::Role::ListItem)
            .aria_label(row.title.clone())
            .aria_selected(is_current)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                // Space is the global play/pause key.
                if let ClickEvent::Keyboard(k) = event {
                    if k.button == KeyboardButton::Space {
                        return;
                    }
                }
                window.focus(&this.focus_list, cx);
                this.cursor = Some(id);
                if !is_current {
                    this.player.update(cx, |m, cx| m.play_entry(id, cx));
                }
                cx.notify();
            }))
            .when(is_current, |d| {
                // The selected-row marker, as on the nav rail.
                d.child(playing_row::marker(accent))
            })
            .child(art_tile(ART, thumb))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(text_line(
                        "title",
                        row.title.clone(),
                        title_style,
                        title_color,
                        cx,
                    ))
                    .child(text_line(
                        "artist",
                        row.artist.clone(),
                        typography::BODY,
                        color::text_secondary(),
                        cx,
                    )),
            )
            .child(heart)
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(DURATION_WIDTH))
                    .h(px(layout::MIN_HIT_TARGET))
                    .child(duration)
                    .child(actions),
            )
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)))
            .when_some(upcoming_ix, |d, _| {
                let drag = DraggedEntry {
                    id,
                    title: row.title.clone(),
                    artist: row.artist.clone(),
                };
                d.on_drag(drag, |dragged, _, _, cx| {
                    cx.new(|_| DragGhost(dragged.clone()))
                })
            });

        let surface = track_menu::on_context_menu(surface, menu);
        let wrapper = div()
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .px(px(spacing::S2))
            .py(px(ROW_INSET))
            .child(surface);
        match upcoming_ix {
            // An up-next row takes a dragged row's place: a 2px rounded line shows above it,
            // as wide as the row's content.
            Some(target) => {
                let drop_group: SharedString = format!("queue-drop-{ix}").into();
                wrapper
                    .group(drop_group.clone())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left(px(spacing::S2))
                            .right(px(spacing::S2))
                            .h(px(DROP_LINE))
                            .rounded_full()
                            .bg(color::text_secondary())
                            .opacity(0.0)
                            .group_drag_over::<DraggedEntry>(drop_group, |s| s.opacity(1.0)),
                    )
                    .on_drop(cx.listener(move |this, dragged: &DraggedEntry, _, cx| {
                        let id = dragged.id;
                        this.player
                            .update(cx, |m, cx| m.move_upcoming(id, target, cx));
                    }))
                    .into_any_element()
            }
            None => wrapper.into_any_element(),
        }
    }
}

/// What the "Playing from" line says: caption, name, cover URL and the page the name opens.
struct Playing {
    caption: &'static str,
    name: String,
    cover: Option<String>,
    page: Option<crate::library::Target>,
}

fn playing_from(heading: &QueueHeading) -> Playing {
    match heading {
        QueueHeading::Source(source) => Playing {
            caption: "Playing from",
            name: source.label(),
            cover: source.thumbnail.clone(),
            page: queue_source::page_of(source),
        },
        QueueHeading::Autoplay(seed) => Playing {
            caption: "Autoplay",
            name: seed.clone(),
            cover: None,
            page: None,
        },
        QueueHeading::Yours => Playing {
            caption: "Playing from",
            name: "Your queue".to_owned(),
            cover: None,
            page: None,
        },
    }
}

impl QueuePanel {
    /// Under the header: the place the queue plays from (a cover, "Playing from" over its name;
    /// the name opens the page when there is one) and the Autoplay switch at the right.
    fn source_row(
        &mut self,
        player_view: &crate::player::PlayerView,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let accent = player_view.accent;
        let ready = player_view.status == Status::Ready;
        let playing = self.view.heading.as_ref().map(playing_from);

        let place = playing.map(|playing| {
            if let Some(url) = &playing.cover {
                self.thumbs.update(cx, |store, cx| store.request(url, cx));
            }
            let cover = playing
                .cover
                .as_deref()
                .and_then(|url| match self.thumbs.read(cx).get(url) {
                    Thumb::Image(image) => Some(image),
                    _ => None,
                });
            let linked = playing.page.is_some();
            let ring = linked && keyboard_focused(&self.focus_source, window);
            let text = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(
                    style::text_style(div(), typography::CAPTION, cx)
                        .text_color(color::text_secondary())
                        .child(playing.caption),
                )
                .child(
                    {
                        let full: SharedString = playing.name.clone().into();
                        style::text_style(div(), typography::BODY_STRONG, cx)
                            .id("queue-source-name")
                            .truncate()
                            .text_color(color::text_primary())
                            .when(linked, |d| d.group_hover("queue-source", |s| s.underline()))
                            .tooltip({
                                let full = full.clone();
                                move |_, cx| Tooltip::view(full.clone(), cx)
                            })
                            .child(full)
                    },
                );
            let open = self.on_open.clone();
            div()
                .id("queue-source")
                .debug_selector(|| "queue-source".into())
                .group("queue-source")
                .relative()
                .flex()
                .flex_row()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(px(spacing::S2))
                .rounded(px(radius::SMALL))
                .child(art_tile(SOURCE_ART, cover))
                .child(text)
                .when_some(playing.page, |d, page| {
                    d.track_focus(&self.focus_source.clone().tab_stop(true))
                        .role(gpui::Role::Link)
                        .aria_label(playing.name.clone())
                        .cursor_pointer()
                        .on_click(move |event: &ClickEvent, window, cx| {
                            // Space is the global play/pause key.
                            if let ClickEvent::Keyboard(k) = event {
                                if k.button == KeyboardButton::Space {
                                    return;
                                }
                            }
                            open(page.clone(), window, cx);
                        })
                })
                .when(ring, |d| d.child(focus_ring(color::accent(accent), radius::SMALL)))
        });

        let player = self.player.clone();
        div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(SOURCE_ROW))
            .px(px(spacing::S4))
            .children(place)
            // Right-aligned: a radio keeps playing when the queue would end.
            .child(
                div()
                    .debug_selector(|| "queue-autoplay".into())
                    .flex_none()
                    .ml_auto()
                    .child(
                        LabeledSwitch::new(
                            "queue-autoplay",
                            &self.focus_autoplay,
                            "Autoplay",
                            player_view.autoplay,
                            accent,
                        )
                        .enabled(ready)
                        .on_toggle(move |_, cx| {
                            player.update(cx, |m, cx| {
                                let on = !m.autoplay();
                                m.set_autoplay(on, cx)
                            });
                        }),
                    ),
            )
    }
}

impl Focusable for QueuePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_list.clone()
    }
}

impl Render for QueuePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (player_view, view) = {
            let p = self.player.read(cx);
            (p.view(), p.queue_view())
        };
        self.accent = player_view.accent;
        self.items = flatten(&view);
        self.view = view;
        let ready = player_view.status == Status::Ready;
        let has_upcoming = !self.view.upcoming.is_empty();
        let accent = player_view.accent;

        let (repeat_icon, repeat_label, repeat_on) = match player_view.repeat {
            Repeat::Off => (Icon::RepeatAllRegular, "Repeat off", false),
            Repeat::All => (Icon::RepeatAllFilled, "Repeat all", true),
            Repeat::One => (Icon::RepeatOneFilled, "Repeat one", true),
        };
        let player = self.player.clone();
        let tabs = self
            .tabs
            .clone()
            .filter(|_| self.tabs_visible)
            .map(|(focus, on_pick)| PaneTabs::new(Pane::UpNext, &focus, accent, on_pick));
        let titled = tabs.is_none();
        let header = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .child(if titled {
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child("Queue")
                    .into_any_element()
            } else {
                div().into_any_element()
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing::S1))
                    .child(
                        IconButton::new(
                            "queue-shuffle",
                            &self.focus_shuffle,
                            if player_view.shuffle {
                                Icon::ShuffleFilled
                            } else {
                                Icon::ShuffleRegular
                            },
                            "Shuffle",
                            accent,
                        )
                        .enabled(ready)
                        .icon_color(on_off(player_view.shuffle))
                        .toggled(player_view.shuffle)
                        .on_click({
                            let p = player.clone();
                            move |_, cx| {
                                p.update(cx, |m, cx| m.toggle_shuffle(cx));
                            }
                        }),
                    )
                    .child(
                        IconButton::new(
                            "queue-repeat",
                            &self.focus_repeat,
                            repeat_icon,
                            repeat_label,
                            accent,
                        )
                        .enabled(ready)
                        .icon_color(on_off(repeat_on))
                        .on_click({
                            let p = player.clone();
                            move |_, cx| {
                                p.update(cx, |m, cx| m.cycle_repeat(cx));
                            }
                        }),
                    )
                    .child(
                        div().debug_selector(|| "queue-clear".into()).child(
                            IconButton::new(
                                "queue-clear",
                                &self.focus_clear,
                                Icon::Delete,
                                "Clear up next",
                                accent,
                            )
                            .enabled(ready && has_upcoming)
                            .on_click({
                                let p = player.clone();
                                move |_, cx| {
                                    p.update(cx, |m, cx| m.clear_upcoming(cx));
                                }
                            }),
                        ),
                    )
            );

        let source = self.source_row(&player_view, window, cx);

        let list = div()
            .id("queue-list")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus_list)
            .role(gpui::Role::List)
            .aria_label("Queue")
            .on_action(cx.listener(|this, _: &CursorUp, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &CursorDown, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &PlayCursor, _, cx| this.play_cursor(cx)))
            .on_action(cx.listener(|this, _: &RemoveCursor, _, cx| this.remove_cursor(cx)))
            .on_action(cx.listener(|this, _: &LikeCursor, _, cx| this.like_cursor(cx)))
            .on_action(cx.listener(|this, _: &MoveCursorUp, _, cx| this.move_cursor(-1, cx)))
            .on_action(cx.listener(|this, _: &MoveCursorDown, _, cx| this.move_cursor(1, cx)))
            .flex_1()
            .min_h_0()
            .w_full()
            .child(
                uniform_list(
                    "queue-rows",
                    self.items.len(),
                    cx.processor(|this, range: Range<usize>, window, cx| {
                        this.rows(range, window, cx)
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            );

        div()
            .id("queue-panel")
            .debug_selector(|| "queue-panel".into())
            .flex()
            .flex_col()
            .flex_none()
            .w(px(layout::QUEUE_PANEL_WIDTH))
            .h_full()
            .bg(color::bg_raised())
            .border_l_1()
            .border_color(color::border_subtle())
            .children(tabs)
            .child(header)
            .child(source)
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use apollo_domain::model::QueueEntryId;

    use super::*;

    fn row(n: u64) -> QueueRow {
        QueueRow {
            id: QueueEntryId::from_raw(n),
            track: apollo_domain::model::Track::new(format!("v{n}"), format!("Track {n}")),
            video_id: format!("v{n}"),
            title: format!("Track {n}").into(),
            artist: "Artist".into(),
            duration_ms: Some(180_000),
        }
    }

    fn view(history: u64, current: bool, upcoming: u64) -> QueueView {
        QueueView {
            history: (0..history).map(row).collect(),
            current: current.then(|| row(100)),
            upcoming: (0..upcoming).map(|i| row(200 + i)).collect(),
            heading: None,
        }
    }

    #[test]
    fn lines_run_history_current_then_up_next_with_headers() {
        assert_eq!(
            flatten(&view(2, true, 2)),
            [
                Item::Header(Section::History),
                Item::History(0),
                Item::History(1),
                Item::Current,
                Item::Header(Section::UpNext),
                Item::Upcoming(0),
                Item::Upcoming(1),
            ]
        );
    }

    #[test]
    fn no_history_means_no_history_header() {
        assert_eq!(
            flatten(&view(0, true, 1)),
            [
                Item::Current,
                Item::Header(Section::UpNext),
                Item::Upcoming(0)
            ]
        );
    }

    #[test]
    fn an_empty_up_next_shows_the_empty_line_and_an_empty_queue_only_that() {
        assert_eq!(
            flatten(&view(0, true, 0)),
            [Item::Current, Item::Header(Section::UpNext), Item::Empty]
        );
        assert_eq!(flatten(&view(0, false, 0)), [Item::Empty]);
    }

    #[test]
    fn the_cursor_visits_history_current_then_upcoming() {
        let ids = view(1, true, 1).ordered_ids();
        assert_eq!(
            ids,
            [
                QueueEntryId::from_raw(0),
                QueueEntryId::from_raw(100),
                QueueEntryId::from_raw(200)
            ]
        );
    }

    #[test]
    fn after_a_removal_the_cursor_takes_the_next_row_else_the_previous() {
        let ids = [
            QueueEntryId::from_raw(1),
            QueueEntryId::from_raw(2),
            QueueEntryId::from_raw(3),
        ];
        assert_eq!(
            cursor_after_removal(&ids, QueueEntryId::from_raw(2)),
            Some(QueueEntryId::from_raw(3))
        );
        assert_eq!(
            cursor_after_removal(&ids, QueueEntryId::from_raw(3)),
            Some(QueueEntryId::from_raw(2))
        );
        assert_eq!(
            cursor_after_removal(&[QueueEntryId::from_raw(1)], QueueEntryId::from_raw(1)),
            None
        );
        assert_eq!(cursor_after_removal(&ids, QueueEntryId::from_raw(9)), None);
    }

    #[test]
    fn rows_are_56px_with_a_ring_sized_inset() {
        // The focus ring reaches offset + width = 4px, exactly the row inset.
        assert_eq!(
            ROW_INSET,
            layout::FOCUS_RING_OFFSET + layout::FOCUS_RING_WIDTH
        );
        assert_eq!(ROW_HEIGHT, 56.0);
    }
}
