//! "More like the Crate": the last rows of the Crate's list, on the page (ten songs) and in the
//! docked panel (five). A title row (the shelf title with a Refresh button), then songs from the
//! radio of one of the Crate's songs (see [`crate::crate_more`] for what is fetched and when).
//! The rows look like More like this's on a playlist page: the title and artist are
//! text.secondary until the pointer is on the row, and a + sets the song aside too (the Crate
//! row appears, the pill says so, and the song leaves the rail).
//!
//! They are extra rows of the page's virtual list, after the slot a row can be dropped into at
//! the end, so they scroll with the Crate. Nothing here is drawn while a filter narrows the list.

use apollo_theme::tokens::{radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle, ParentElement,
    ScrollStrategy, SharedString, Styled, Window,
};

use super::{art_tile, text_line, CratePage, ACTION_SIZE, ART, DURATION_WIDTH, ROW_HEIGHT, ROW_INSET};
use crate::assets::Icon;
use crate::crate_more::{self, CrateMoreModel, Phase};
use crate::library::source::TrackRow;
use crate::player::format_time;
use crate::queue_source;
use crate::style::{self, color};
use crate::ui::playing_row;
use crate::ui::row_actions::{heart_action, row_action};
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::track_menu::{self, Subject};

/// Skeleton rows under the title while the first set is on its way.
const SKELETON: usize = 3;

/// Tab stop of Refresh, after the list (the page's and the panel's slots end at 17).
const TAB_REFRESH: isize = 18;

/// What a Crate page (or the docked panel) keeps of its rail.
pub(super) struct Rail {
    model: Option<Entity<CrateMoreModel>>,
    refresh: FocusHandle,
    /// Copied at each render, for the list's row callback.
    phase: Phase,
    songs: Vec<TrackRow>,
    refreshing: bool,
}

impl Rail {
    pub(super) fn new(cx: &mut Context<CratePage>) -> Self {
        let model = crate_more::handle(cx);
        if let Some(model) = &model {
            cx.observe(model, |_, _, cx| cx.notify()).detach();
        }
        Self {
            model,
            refresh: cx.focus_handle().tab_index(TAB_REFRESH).tab_stop(true),
            phase: Phase::Hidden,
            songs: Vec::new(),
            refreshing: false,
        }
    }
}

impl CratePage {
    /// How many songs this layout's rail shows.
    fn rail_limit(&self) -> usize {
        if self.docked() {
            crate_more::DOCK_ROWS
        } else {
            crate_more::PAGE_ROWS
        }
    }

    /// Runs at each render: tells the model its rail is on screen (so it may fetch, once) and
    /// copies what it shows. `filtering`: a filter narrows the list, which hides the rail.
    pub(super) fn sync_rail(&mut self, filtering: bool, cx: &mut Context<Self>) {
        let Some(model) = self.rail.model.clone() else {
            return;
        };
        if !model.read(cx).is_open() {
            // From inside a draw: the model is told right after it.
            let open = model.clone();
            cx.defer(move |cx| open.update(cx, |m, cx| m.open(cx)));
        }
        let limit = self.rail_limit();
        let m = model.read(cx);
        let phase = if filtering { Phase::Hidden } else { m.phase(cx) };
        self.rail.songs = if phase == Phase::Ready {
            m.visible(limit, cx)
        } else {
            Vec::new()
        };
        self.rail.refreshing = m.is_refreshing();
        self.rail.phase = phase;
    }

    /// How many rows the rail adds after the list's end slot.
    pub(super) fn rail_rows(&self) -> usize {
        match self.rail.phase {
            Phase::Hidden => 0,
            Phase::Loading => 1 + SKELETON,
            Phase::Ready => 1 + self.rail.songs.len(),
            Phase::Idle => 1,
            Phase::Failed(_) => 2,
        }
    }

    /// Artwork for the rail's songs that are on screen (a song's cover URL is known from the
    /// radio, so this loads only the picture, never the song).
    pub(super) fn rail_art(&mut self, range: &std::ops::Range<usize>, cx: &mut Context<Self>) {
        let first = self.rows.len() + 2;
        let mut ids = Vec::new();
        for ix in range.clone().filter(|ix| *ix >= first) {
            if let Some(song) = self.rail.songs.get(ix - first) {
                let id = song.track.video_id.as_str().to_owned();
                self.player
                    .read(cx)
                    .hint_art(&id, song.thumbnail.as_deref());
                ids.push(id);
            }
        }
        if !ids.is_empty() {
            self.player.update(cx, |m, cx| m.request_thumbs(ids, cx));
        }
    }

    /// Row `slot` of the rail (0 is the title row).
    pub(super) fn rail_row(
        &self,
        slot: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if slot == 0 {
            return self.rail_title(cx);
        }
        match self.rail.phase {
            Phase::Ready => self.rail_song(slot - 1, window, cx),
            Phase::Loading => self.rail_skeleton(slot - 1, cx),
            _ => self.rail_failed(cx),
        }
    }

    fn rail_wrapper(&self) -> gpui::Div {
        div()
            .relative()
            .h(px(ROW_HEIGHT))
            .w_full()
            .flex_none()
            .py(px(ROW_INSET))
            .when(self.docked(), |d| d.px(px(spacing::S2)))
    }

    /// The shelf title, and Refresh (Retry after a failure) at the right.
    fn rail_title(&self, cx: &mut Context<Self>) -> AnyElement {
        let phase = self.rail.phase;
        let limit = self.rail_limit();
        let refreshing = self.rail.refreshing;
        let accent: Accent = self.accent;
        let button = self.rail.model.clone().map(|model| {
            let failed = matches!(phase, Phase::Failed(_));
            let label = if failed { "Retry" } else { "Refresh" };
            div().debug_selector(|| "crate-rail-refresh".into()).child(
                TextButton::new("crate-rail-refresh", &self.rail.refresh, label, accent)
                    .busy(refreshing)
                    .on_click(move |_, cx| {
                        model.update(cx, |m, cx| {
                            if failed {
                                m.retry(cx)
                            } else {
                                m.refresh(limit, cx)
                            }
                        })
                    }),
            )
        });
        div()
            .id("crate-rail-title")
            .debug_selector(|| "crate-rail-title".into())
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S2))
            .child(
                style::text_style(div(), typography::SECTION, cx)
                    .text_color(color::text_primary())
                    .child("More like the Crate"),
            )
            .children(button)
            .into_any_element()
    }

    fn rail_skeleton(&self, pos: usize, _cx: &mut Context<Self>) -> AnyElement {
        let block = |w: f32, h: f32| {
            div()
                .w(px(w))
                .h(px(h))
                .rounded(px(radius::SMALL))
                .bg(color::skeleton())
        };
        let row = self
            .rail_wrapper()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .child(
                div()
                    .size(px(ART))
                    .flex_none()
                    .rounded(px(radius::MEDIUM))
                    .bg(color::skeleton()),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(spacing::S2))
                    .flex_1()
                    .min_w_0()
                    .child(block(200.0 - (pos % 3) as f32 * 40.0, 12.0))
                    .child(block(120.0, 10.0)),
            );
        skeleton::pulse(row, "crate-rail-skeleton", true)
    }

    fn rail_failed(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S2))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child("Couldn't load suggestions"),
            )
            .into_any_element()
    }

    /// One suggested song: a Crate row with a + where the Crate's rows have Remove.
    fn rail_song(&self, pos: usize, _window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rail.songs.get(pos).cloned() else {
            return div().into_any_element();
        };
        let docked = self.docked();
        let name = if docked { "dock" } else { "crate" };
        let ix = pos;
        let video: String = row.track.video_id.as_str().to_owned();
        let playing = self.player.read(cx).current_video().as_deref() == Some(video.as_str());
        let group: SharedString = format!("{name}-rail-row-{ix}").into();
        let thumb = self.player.read(cx).thumb(&video);
        let title: SharedString = row.track.title.clone().into();
        let artists: SharedString = row
            .track
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
            .into();
        let duration: SharedString = row
            .track
            .duration
            .map(|d| format_time(d.as_millis() as u64))
            .unwrap_or_default()
            .into();
        let accent = color::accent(self.accent);
        let subject = Subject::track(row.track.clone(), row.thumbnail.clone());
        // Quiet until the pointer is on the row (or it is the song playing).
        let text = |d: gpui::Div| {
            if playing {
                d.text_color(color::text_primary())
            } else {
                d.text_color(color::text_secondary())
                    .group_hover(group.clone(), |s| s.text_color(color::text_primary()))
            }
        };

        let heart = div()
            .flex_none()
            .size(px(ACTION_SIZE))
            .opacity(0.0)
            .group_hover(group.clone(), |s| s.opacity(1.0))
            .child(heart_action((SharedString::from(format!("{name}-rail-like")), ix), false, {
                let (writes, video) = (self.writes.clone(), video.clone());
                move |_, cx| writes.update(cx, |w, cx| w.toggle_like(&video, cx))
            }));
        let plus = div()
            .flex_none()
            .size(px(ACTION_SIZE))
            .debug_selector(move || format!("{name}-rail-add-{ix}"))
            .child(row_action(
                (SharedString::from(format!("{name}-rail-add")), ix),
                Icon::Add,
                "Add to Crate",
                color::text_secondary(),
                {
                    let (page, video) = (cx.entity(), video.clone());
                    move |_, cx| page.update(cx, |p, cx| p.rail_add(&video, cx))
                },
            ));

        let surface = div()
            .id((SharedString::from(format!("{name}-rail-row")), ix))
            .debug_selector(move || format!("{name}-rail-row-{ix}"))
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
            .when(playing, |d| d.bg(playing_row::tint()))
            .hover(move |s| s.bg(if playing { playing_row::tint() } else { color::state_hover() }))
            .role(gpui::Role::ListItem)
            .aria_label(title.clone())
            .on_click(cx.listener({
                let video = video.clone();
                move |this, event: &gpui::ClickEvent, _, cx| {
                    // Space is the global play/pause key.
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == gpui::KeyboardButton::Space {
                            return;
                        }
                    }
                    this.rail_play(&video, cx);
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
                    .child(rail_line(
                        title,
                        playing_row::title_style(typography::LIST_TITLE, playing),
                        &text,
                        cx,
                    ))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(spacing::S1))
                            .min_w_0()
                            .when(row.explicit, |d| {
                                d.child(
                                    style::text_style(div(), typography::CAPTION, cx)
                                        .id((SharedString::from(format!("{name}-rail-explicit")), ix))
                                        .flex_none()
                                        .text_color(color::text_tertiary())
                                        .aria_label("Explicit")
                                        .child("E"),
                                )
                            })
                            .child(div().min_w_0().overflow_hidden().child(rail_line(
                                artists,
                                typography::BODY,
                                &text,
                                cx,
                            ))),
                    ),
            )
            .child(heart)
            // The panel is too narrow for the length; the + is the point.
            .when(!docked, |d| {
                d.child(
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
            })
            .child(plus);

        track_menu::on_context_menu(self.rail_wrapper().child(surface), subject).into_any_element()
    }

    /// The + of a suggested song: it goes in the Crate at the bottom, with the usual pill, and the
    /// list scrolls to keep it in view.
    pub(super) fn rail_add(&mut self, video: &str, cx: &mut Context<Self>) {
        let Some(model) = self.rail.model.clone() else {
            return;
        };
        let at = self.crate_.read(cx).len();
        if model.update(cx, |m, cx| m.add(video, cx)) {
            self.scroll.scroll_to_item(at, ScrollStrategy::Nearest);
            cx.notify();
        }
    }

    /// A click on a suggested song: it plays, with the suggestions on screen as the queue after
    /// it. Nothing is looked up before this.
    fn rail_play(&mut self, video: &str, cx: &mut Context<Self>) {
        if self.player.read(cx).current_video().as_deref() == Some(video) {
            self.player.update(cx, |m, cx| m.toggle_play(cx));
            return;
        }
        let songs = self.rail.songs.clone();
        let Some(start) = songs.iter().position(|r| r.track.video_id.as_str() == video) else {
            return;
        };
        let tracks = songs.iter().map(|r| r.track.clone()).collect();
        self.player.update(cx, |p, cx| {
            for song in &songs {
                p.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
            }
            p.play_tracks_from(
                tracks,
                Some(start),
                None,
                Some(queue_source::named("More like the Crate")),
                cx,
            );
        });
    }
}

/// One truncated line; the row sets its colour (and changes it under the pointer).
fn rail_line(
    text: SharedString,
    style_: typography::TextStyle,
    colour: &dyn Fn(gpui::Div) -> gpui::Div,
    cx: &App,
) -> impl IntoElement {
    colour(style::text_style(div(), style_, cx).truncate()).child(text)
}
