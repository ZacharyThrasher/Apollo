//! The add-to-playlist picker: a small popover listing the account's playlists, anchored to the
//! add-to-playlist action (or the selected row) that opened it. With no anchor it sits in the
//! middle of the window. There is no scrim: the page behind stays as it is, and a click on it
//! closes the popover.
//!
//! Choosing a playlist closes the panel at once; the add itself is optimistic and rolls back
//! with an error if the service refuses (see `library::writes`). A playlist that already holds
//! the track shows a check and does nothing. Keyboard: Up and Down move, Enter or Space adds,
//! Escape closes; Tab stays inside. Clicking outside closes.

use apollo_domain::library::PlaylistTarget;
use apollo_domain::listing::LoadFailure;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, AnyElement, App, Bounds, Context, Entity, FocusHandle,
    Focusable, IntoElement, KeyBinding, MouseButton, ParentElement, Pixels, Render, ScrollHandle,
    SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::library::model::Load;
use crate::library::writes::LibraryWrites;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::button::keyboard_focused;
use crate::ui::playlist_dialog::{self, Mode};
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::tooltip::Tooltip;

actions!(
    apollo_picker,
    [PickerUp, PickerDown, PickerChoose, PickerClose, PickerStay]
);

/// Key context of the picker.
pub const KEY_CONTEXT: &str = "PlaylistPicker";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", PickerUp, Some(KEY_CONTEXT)),
        KeyBinding::new("down", PickerDown, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", PickerChoose, Some(KEY_CONTEXT)),
        KeyBinding::new("space", PickerChoose, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", PickerClose, Some(KEY_CONTEXT)),
        // Focus stays in the panel while it is open.
        KeyBinding::new("tab", PickerStay, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-tab", PickerStay, Some(KEY_CONTEXT)),
    ]);
}

pub const PANEL_WIDTH: f32 = 280.0;
pub const PANEL_MAX_HEIGHT: f32 = 320.0;
const ROW_HEIGHT: f32 = 40.0;
/// The shortest the popover gets, however little room there is.
const MIN_HEIGHT: f32 = 160.0;
const SKELETON_ROWS: usize = 5;
/// Gap between the anchor and the popover, and the least it keeps from the window's edge.
const ANCHOR_GAP: f32 = spacing::S1;
const EDGE_MARGIN: f32 = spacing::S2;

struct Open {
    video_id: String,
    /// Every song being added when the panel was opened for several (the Crate's "Add all to…");
    /// empty for one song. `video_id` is then the first of them, which only the list's
    /// "already held" marks are read for, so those are not shown.
    all: Vec<String>,
    /// The track being added, shown under the panel title.
    title: SharedString,
    playlists: Load<Vec<PlaylistTarget>>,
    cursor: usize,
    /// The cursor is on the fixed "New playlist\u{2026}" row above the list.
    on_new: bool,
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
    focus: FocusHandle,
    retry_focus: FocusHandle,
    scroll: ScrollHandle,
    open: Option<Open>,
    next_request: u64,
}

impl PlaylistPicker {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            writes,
            focus: cx.focus_handle(),
            retry_focus: cx.focus_handle().tab_stop(true),
            scroll: ScrollHandle::new(),
            open: None,
            next_request: 1,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// The track the panel is open for.
    pub fn video(&self) -> Option<&str> {
        self.open.as_ref().map(|o| o.video_id.as_str())
    }

    pub fn playlists(&self) -> Option<&Load<Vec<PlaylistTarget>>> {
        self.open.as_ref().map(|o| &o.playlists)
    }

    /// The bounds the popover is anchored to.
    pub fn anchor(&self) -> Option<Bounds<Pixels>> {
        self.open.as_ref().and_then(|o| o.anchor)
    }

    pub fn cursor(&self) -> Option<usize> {
        self.open.as_ref().map(|o| o.cursor)
    }

    /// The cursor is on the "New playlist\u{2026}" row.
    pub fn on_new(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.on_new)
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
        self.open_songs(video_id, Vec::new(), title, anchor, window, cx);
    }

    /// Open the panel to add all of `video_ids` (in order) to one playlist, anchored like
    /// [`Self::open_with`]. Nothing opens for an empty list.
    pub fn open_many(
        &mut self,
        video_ids: Vec<String>,
        title: impl Into<SharedString>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(first) = video_ids.first().cloned() else {
            return;
        };
        self.open_songs(&first, video_ids, title, anchor, window, cx);
    }

    fn open_songs(
        &mut self,
        video_id: &str,
        all: Vec<String>,
        title: impl Into<SharedString>,
        anchor: Option<Bounds<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let return_focus = self
            .open
            .as_ref()
            .and_then(|o| o.return_focus.clone())
            .or_else(|| window.focused(cx));
        self.open = Some(Open {
            video_id: video_id.to_owned(),
            all,
            title: title.into(),
            playlists: Load::Loading,
            cursor: 0,
            on_new: false,
            request: 0,
            return_focus,
            anchor,
        });
        window.focus(&self.focus, cx);
        self.load(cx);
        cx.notify();
    }

    /// Close without touching focus: the account changed under the panel.
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
        let video = open.video_id.clone();
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
        // With no playlist to land on, the cursor rests on the way to make one.
        open.on_new = !matches!(&result, Ok(list) if !list.is_empty());
        open.playlists = match result {
            Ok(list) => Load::Ready(list),
            Err(failure) => Load::Failed(failure),
        };
        open.cursor = 0;
        cx.notify();
    }

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let len = match &open.playlists {
            Load::Ready(list) => list.len(),
            _ => 0,
        };
        if open.on_new {
            // Down leaves the New playlist row for the first playlist, if there is one.
            if delta > 0 && len > 0 {
                open.on_new = false;
                open.cursor = 0;
                self.scroll.scroll_to_item(0);
                cx.notify();
            }
            return;
        }
        if delta < 0 && (len == 0 || open.cursor == 0) {
            open.on_new = true;
            cx.notify();
            return;
        }
        if len == 0 {
            return;
        }
        open.cursor = (open.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        self.scroll.scroll_to_item(open.cursor);
        cx.notify();
    }

    /// Close the picker and ask for a name for a new playlist that will hold the song.
    pub fn new_playlist(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let video = open.video_id.clone();
        let many = open.all.clone();
        self.close(window, cx);
        if many.is_empty() {
            playlist_dialog::open(
                Mode::Create {
                    add_video: Some(video),
                },
                window,
                cx,
            );
        } else {
            playlist_dialog::open(
                Mode::FromCrate {
                    name: playlist_dialog::CRATE_PLAYLIST_NAME.to_owned(),
                    video_ids: many,
                },
                window,
                cx,
            );
        }
    }

    /// Add the track to the playlist at `index` and close. A playlist that already holds it
    /// does nothing.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.on_new() {
            self.new_playlist(window, cx);
            return;
        }
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let Load::Ready(list) = &open.playlists else {
            return;
        };
        let Some(target) = list.get(index).cloned() else {
            return;
        };
        if !open.all.is_empty() {
            let videos = open.all.clone();
            self.writes
                .update(cx, |w, cx| w.add_many_to_playlist(&target, &videos, cx));
            self.close(window, cx);
            return;
        }
        let video = open.video_id.clone();
        if self.writes.read(cx).holds(&target, &video) {
            return;
        }
        self.writes
            .update(cx, |w, cx| w.add_to_playlist(&target, &video, cx));
        self.close(window, cx);
    }
}

impl Focusable for PlaylistPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// What the panel says when the list could not be loaded.
pub const LOAD_FAILED: &str = "Couldn't load playlists";
pub const NO_PLAYLISTS: &str = "No playlists yet";
/// The fixed first row.
pub const NEW_PLAYLIST: &str = "New playlist\u{2026}";

impl Render for PlaylistPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(open) = self.open.as_ref() else {
            return div().into_any_element();
        };
        let (accent, _) = self.player.read(cx).accent_now();
        let ring = keyboard_focused(&self.focus, window);

        let body: AnyElement = match &open.playlists {
            Load::Idle | Load::Loading => skeleton_rows(!cx.reduce_motion()),
            Load::Failed(_) => {
                let entity = cx.entity();
                message(
                    LOAD_FAILED,
                    Some(
                        TextButton::new("picker-retry", &self.retry_focus, "Retry", accent)
                            .strong()
                            .on_click(move |_, cx| entity.update(cx, |m, cx| m.load(cx))),
                    ),
                    cx,
                )
            }
            Load::Ready(list) if list.is_empty() => message(NO_PLAYLISTS, None, cx),
            Load::Ready(list) => {
                let held: Vec<bool> = {
                    let writes = self.writes.read(cx);
                    list.iter()
                        // For several songs the marks would speak of the first one only.
                        .map(|t| open.all.is_empty() && writes.holds(t, &open.video_id))
                        .collect()
                };
                let rows: Vec<AnyElement> = list
                    .iter()
                    .enumerate()
                    .map(|(ix, target)| {
                        let cursor = !open.on_new && ix == open.cursor;
                        playlist_row(ix, target, held[ix], cursor, ring, accent, cx)
                    })
                    .collect();
                div()
                    .id("picker-list")
                    .role(gpui::Role::ListBox)
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows)
                    .into_any_element()
            }
        };

        let panel = div()
            .id("playlist-picker")
            .debug_selector(|| "playlist-picker".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .role(gpui::Role::Dialog)
            .aria_label("Add to playlist")
            .flex()
            .flex_col()
            .w(px(PANEL_WIDTH))
            .max_w_full()
            .max_h(px(PANEL_MAX_HEIGHT))
            .p(px(spacing::S2))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the panel are not clicks on the page behind it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|m, _: &PickerUp, _, cx| m.move_cursor(-1, cx)))
            .on_action(cx.listener(|m, _: &PickerDown, _, cx| m.move_cursor(1, cx)))
            .on_action(cx.listener(|m, _: &PickerChoose, window, cx| {
                let at = m.open.as_ref().map(|o| o.cursor);
                if let Some(ix) = at {
                    m.choose(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|m, _: &PickerClose, window, cx| m.close(window, cx)))
            .on_action(cx.listener(|_, _: &PickerStay, _, _| {}))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .px(px(spacing::S2))
                    .pt(px(spacing::S2))
                    .pb(px(spacing::S3))
                    .child(
                        style::text_style(div(), typography::SECTION, cx)
                            .text_color(color::text_primary())
                            .child("Add to playlist"),
                    )
                    .child(
                        style::text_style(div(), typography::BODY, cx)
                            .id("picker-track")
                            .truncate()
                            .text_color(color::text_secondary())
                            .tooltip({
                                let full = open.title.clone();
                                move |_, cx| Tooltip::view(full.clone(), cx)
                            })
                            .child(open.title.clone()),
                    ),
            )
            .child(new_row(open.on_new, ring, accent, cx))
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

fn playlist_row(
    ix: usize,
    target: &PlaylistTarget,
    holds: bool,
    cursor: bool,
    ring: bool,
    accent: Accent,
    cx: &mut Context<PlaylistPicker>,
) -> AnyElement {
    let title: SharedString = target.title.clone().into();
    let title_color = if holds {
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
        .rounded(px(radius::MEDIUM))
        .role(gpui::Role::ListBoxOption)
        .aria_label(title.clone())
        .aria_selected(cursor)
        .when(!holds, |d| d.cursor_pointer())
        .bg(if cursor {
            color::state_selected()
        } else {
            gpui::transparent_black()
        })
        .hover(|s| s.bg(color::state_hover()))
        .on_click(cx.listener(move |m, _: &gpui::ClickEvent, window, cx| {
            if let Some(open) = m.open.as_mut() {
                open.cursor = ix;
                open.on_new = false;
            }
            m.choose(ix, window, cx);
        }))
        .child(
            style::text_style(div(), typography::BODY, cx)
                .id(("picker-title", ix))
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(title_color)
                .tooltip({
                    let full = title.clone();
                    move |_, cx| Tooltip::view(full.clone(), cx)
                })
                .child(title),
        )
        .when(holds, |d| {
            d.child(
                gpui::svg()
                    .path(Icon::Checkmark.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(color::text_secondary()),
            )
        })
        .when(cursor && ring, |d| d.child(focus_ring_inset(accent)))
        .into_any_element()
}

/// The fixed first row: a way to make a playlist for the song.
fn new_row(
    cursor: bool,
    ring: bool,
    accent: Accent,
    cx: &mut Context<PlaylistPicker>,
) -> AnyElement {
    div()
        .id("picker-new")
        .debug_selector(|| "picker-new".into())
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(spacing::S3))
        .h(px(ROW_HEIGHT))
        .px(px(spacing::S3))
        .rounded(px(radius::MEDIUM))
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label(NEW_PLAYLIST)
        .bg(if cursor {
            color::state_selected()
        } else {
            gpui::transparent_black()
        })
        .hover(|s| s.bg(color::state_hover()))
        .on_click(cx.listener(|m, _: &gpui::ClickEvent, window, cx| m.new_playlist(window, cx)))
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
                .child(NEW_PLAYLIST),
        )
        .when(cursor && ring, |d| d.child(focus_ring_inset(accent)))
        .into_any_element()
}

/// The keyboard cursor's ring, drawn inside the row so neighbours do not clip it.
fn focus_ring_inset(accent: Accent) -> impl IntoElement {
    div()
        .absolute()
        .size_full()
        .rounded(px(radius::MEDIUM))
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
