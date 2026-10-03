//! The add-to-playlist picker: a small panel listing the account's playlists, opened over the
//! window from the add-to-playlist action of a row, the player bar's track or Now Playing.
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
    actions, div, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle, Focusable,
    IntoElement, KeyBinding, MouseButton, ParentElement, Render, ScrollHandle, SharedString,
    Styled, Window,
};

use crate::assets::Icon;
use crate::library::model::Load;
use crate::library::writes::LibraryWrites;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::button::keyboard_focused;
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

const PANEL_WIDTH: f32 = 360.0;
const PANEL_MAX_HEIGHT: f32 = 440.0;
const ROW_HEIGHT: f32 = 40.0;
const SKELETON_ROWS: usize = 5;
/// The scrim over the page behind the panel: the base surface, mostly opaque.
const SCRIM: f32 = 0.6;

struct Open {
    video_id: String,
    /// The track being added, shown under the panel title.
    title: SharedString,
    playlists: Load<Vec<PlaylistTarget>>,
    cursor: usize,
    /// Answers for an older request (Retry, reopen) are dropped.
    request: u64,
    /// Where keyboard focus returns to when the panel closes.
    return_focus: Option<FocusHandle>,
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

    pub fn cursor(&self) -> Option<usize> {
        self.open.as_ref().map(|o| o.cursor)
    }

    /// Open the panel for a track. Focus moves into it and comes back on close.
    pub fn open(
        &mut self,
        video_id: &str,
        title: impl Into<SharedString>,
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
            title: title.into(),
            playlists: Load::Loading,
            cursor: 0,
            request: 0,
            return_focus,
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
        let Load::Ready(list) = &open.playlists else {
            return;
        };
        if list.is_empty() {
            return;
        }
        open.cursor = (open.cursor as isize + delta).clamp(0, list.len() as isize - 1) as usize;
        self.scroll.scroll_to_item(open.cursor);
        cx.notify();
    }

    /// Add the track to the playlist at `index` and close. A playlist that already holds it
    /// does nothing.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_ref() else {
            return;
        };
        let Load::Ready(list) = &open.playlists else {
            return;
        };
        let Some(target) = list.get(index).cloned() else {
            return;
        };
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
                        .map(|t| writes.holds(t, &open.video_id))
                        .collect()
                };
                let rows: Vec<AnyElement> = list
                    .iter()
                    .enumerate()
                    .map(|(ix, target)| {
                        playlist_row(ix, target, held[ix], ix == open.cursor, ring, accent, cx)
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
            .rounded(px(radius::LARGE))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            // Clicks inside the panel are not clicks on the scrim.
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
            .child(body);

        div()
            .id("playlist-picker-scrim")
            .debug_selector(|| "playlist-picker-scrim".into())
            .absolute()
            .top(px(layout::TITLE_BAR_HEIGHT))
            .left_0()
            .right_0()
            .bottom_0()
            .flex()
            .items_center()
            .justify_center()
            .p(px(spacing::S4))
            .bg(color::bg_base().opacity(SCRIM))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|m, _, window, cx| m.close(window, cx)),
            )
            .child(panel)
            .into_any_element()
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
