//! The Search page: a field, then results grouped as songs, albums, artists and playlists.
//!
//! Songs play or queue; the other groups are shown only (their pages come later). Keyboard:
//! Ctrl+K or Ctrl+F opens the page with the field focused; Down moves from the field into the
//! songs, Up and Down move between them, Enter plays, Ctrl+Enter queues, Escape goes back to the
//! field (and out of it when the field is empty).

use std::time::Instant;

use apollo_provider::{Hit, ListedTrack, SearchResults};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, img, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle, Focusable,
    ImageSource, IntoElement, ObjectFit, ParentElement, Render, ScrollHandle, SharedString, Styled,
    Window,
};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::player::{format_time, PlayerModel};
use crate::search::{failure_text, SearchModel, SearchState};
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, IconButton};
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::text_field::{self, TextField, TextFieldEvent};
use crate::ui::tooltip::Tooltip;
use crate::url_thumbs::{Thumb, ThumbStore};

actions!(
    apollo_search,
    [
        FocusResults,
        SelectNext,
        SelectPrevious,
        PlaySelected,
        QueueSelected,
        BackToField,
    ]
);

/// Key context of the results list.
pub const RESULTS_CONTEXT: &str = "SearchResults";

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("down", FocusResults, Some(text_field::KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(RESULTS_CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(RESULTS_CONTEXT)),
        KeyBinding::new("enter", PlaySelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("ctrl-enter", QueueSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("escape", BackToField, Some(RESULTS_CONTEXT)),
    ]);
}

const ROW_HEIGHT: f32 = 56.0;
const ROW_ART: f32 = 40.0;
const CARD_WIDTH: f32 = 144.0;
const SKELETON_ROWS: usize = 6;
const SELECTED_MARKER_WIDTH: f32 = 3.0;
/// Scrim over a row's artwork while it is the play target.
const ART_SCRIM: f32 = 0.6;

pub struct SearchPage {
    player: Entity<PlayerModel>,
    model: Entity<SearchModel>,
    thumbs: Entity<ThumbStore>,
    field: Entity<TextField>,
    /// Tab stop of the results list (rows are not tab stops: Up and Down move between them).
    results_focus: FocusHandle,
    /// Tab stop of the message action (Retry, Clear).
    action_focus: FocusHandle,
    root_focus: FocusHandle,
    scroll: ScrollHandle,
    /// The keyboard-selected song, shown only while the list holds focus.
    selected: Option<usize>,
}

impl SearchPage {
    /// `root_focus` is where focus goes when the field is left with Escape.
    pub fn new(
        player: Entity<PlayerModel>,
        model: Entity<SearchModel>,
        thumbs: Entity<ThumbStore>,
        root_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        // Tab order: nav (1-4), the field and its clear button (5), the list (6), the player bar.
        let field_focus = cx.focus_handle().tab_index(5).tab_stop(true);
        let clear_focus = cx.focus_handle().tab_index(5).tab_stop(true);
        let results_focus = cx.focus_handle().tab_index(6).tab_stop(true);
        let action_focus = cx.focus_handle().tab_index(6).tab_stop(true);
        let field = cx.new(|_| TextField::new("Search", field_focus, clear_focus));
        cx.subscribe(
            &field,
            |this, field, event: &TextFieldEvent, cx| match event {
                TextFieldEvent::Changed => {
                    let text = field.read(cx).text().to_owned();
                    this.selected = None;
                    this.model.update(cx, |m, cx| m.set_query(&text, cx));
                }
                TextFieldEvent::Submit => this.model.update(cx, |m, cx| m.submit(cx)),
            },
        )
        .detach();
        cx.observe(&model, |this, _, cx| {
            this.selected = None;
            this.request_thumbs(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            model,
            thumbs,
            field,
            results_focus,
            action_focus,
            root_focus,
            scroll: ScrollHandle::new(),
            selected: None,
        }
    }

    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The index of the keyboard-selected song, if any (tests and diagnostics).
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    fn songs(&self, cx: &App) -> Vec<ListedTrack> {
        self.model
            .read(cx)
            .results()
            .map(|r| r.songs.clone())
            .unwrap_or_default()
    }

    fn request_thumbs(&mut self, cx: &mut Context<Self>) {
        let Some(results) = self.model.read(cx).results().cloned() else {
            return;
        };
        let urls: Vec<String> = thumbnails(&results).collect();
        self.thumbs.update(cx, |store, cx| {
            for url in &urls {
                store.request(url, cx);
            }
        });
    }

    fn focus_results(&mut self, _: &FocusResults, window: &mut Window, cx: &mut Context<Self>) {
        if self.songs(cx).is_empty() {
            return;
        }
        self.selected = Some(0);
        window.focus(&self.results_focus, cx);
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.songs(cx).len();
        if count == 0 {
            return;
        }
        let next = self.selected.map_or(0, |i| (i + 1).min(count - 1));
        self.select(next, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Some(i) if i > 0 => self.select(i - 1, cx),
            _ => self.back_to_field(&BackToField, window, cx),
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = Some(index);
        // The songs' heading is child 0 of the list, so song `i` is child `i + 1`.
        self.scroll.scroll_to_item(index + 1);
        cx.notify();
    }

    fn back_to_field(&mut self, _: &BackToField, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = None;
        text_field::focus(&self.field, window, cx);
        cx.notify();
    }

    fn play_selected(&mut self, _: &PlaySelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.player.update(cx, |m, cx| m.play_now(song.track, cx));
        }
    }

    fn queue_selected(&mut self, _: &QueueSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.player
                .update(cx, |m, cx| m.add_to_queue(song.track, cx));
        }
    }

    /// Escape in an empty field leaves it: focus goes back to the window.
    fn leave_field(&mut self, _: &text_field::Escape, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.root_focus, cx);
    }
}

impl Focusable for SearchPage {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.field.read(cx).focus_handle(cx)
    }
}

/// Every thumbnail URL the results carry, once each.
fn thumbnails(results: &SearchResults) -> impl Iterator<Item = String> + '_ {
    results
        .songs
        .iter()
        .filter_map(|s| s.thumbnail.clone())
        .chain(
            results
                .albums
                .iter()
                .chain(&results.artists)
                .chain(&results.playlists)
                .filter_map(|h| h.thumbnail.clone()),
        )
}

impl Render for SearchPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let model = self.model.read(cx);
        let state = model.state();
        let query: SharedString = model.query().to_owned().into();
        let results = model.results().cloned();
        let confirmed_empty = model.is_confirmed_empty();

        let field_ring = keyboard_focused(&self.field.read(cx).focus_handle(cx), window);
        let list_focused = self.results_focus.is_focused(window);

        let body: AnyElement = match (state, results) {
            (SearchState::Idle, _) => div().into_any_element(),
            (SearchState::Failed(failure), _) => {
                let model = self.model.clone();
                self.message(
                    failure_text(failure).into(),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry(cx)),
                    cx,
                )
            }
            (SearchState::Ready, Some(_)) if confirmed_empty => {
                let text: SharedString = format!("No results for \u{201C}{query}\u{201D}").into();
                let field = self.field.clone();
                self.message(
                    text,
                    "Clear",
                    accent,
                    move |window, cx| {
                        field.update(cx, |f, cx| f.set_text("", cx));
                        text_field::focus(&field, window, cx);
                    },
                    cx,
                )
            }
            (_, Some(results)) => self.results_list(&results, list_focused, accent, cx),
            (SearchState::Loading, None) => skeletons(cx),
            // Ready without results cannot happen; show nothing rather than guess.
            (SearchState::Ready, None) => div().into_any_element(),
        };

        let fields_focus = self.results_focus.clone();
        div()
            .id("search-page")
            .on_action(cx.listener(Self::focus_results))
            .on_action(cx.listener(Self::leave_field))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .px(px(spacing::S8))
            .pt(px(spacing::S6))
            .child(
                style::text_style(div(), typography::PAGE_TITLE, cx)
                    .text_color(color::text_primary())
                    .child("Search"),
            )
            .child(
                div()
                    .relative()
                    .flex_none()
                    .mt(px(spacing::S4))
                    .child(self.field.clone())
                    .when(field_ring, |d| {
                        d.child(focus_ring(color::accent(accent), radius::SMALL))
                    }),
            )
            .child(
                div()
                    .id("search-results")
                    .key_context(RESULTS_CONTEXT)
                    .track_focus(&fields_focus)
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_previous))
                    .on_action(cx.listener(Self::play_selected))
                    .on_action(cx.listener(Self::queue_selected))
                    .on_action(cx.listener(Self::back_to_field))
                    .role(gpui::Role::ListBox)
                    .aria_label("Search results")
                    .flex_1()
                    .min_h_0()
                    .mt(px(spacing::S4))
                    .pb(px(spacing::S6))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(body),
            )
    }
}

impl SearchPage {
    /// One short line and one action: the empty and error states.
    fn message(
        &self,
        text: SharedString,
        action: &'static str,
        accent: Accent,
        on_action: impl Fn(&mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(layout::MIN_HIT_TARGET))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .truncate()
                    .text_color(color::text_secondary())
                    .child(text),
            )
            .child(
                TextButton::new("search-message-action", &self.action_focus, action, accent)
                    .strong()
                    .on_click(on_action),
            )
            .into_any_element()
    }
}

impl SearchPage {
    fn results_list(
        &self,
        results: &SearchResults,
        list_focused: bool,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let thumbs = self.thumbs.read(cx);
        let thumb_of = |url: &Option<String>| match url {
            Some(url) => thumbs.get(url),
            None => Thumb::Missing,
        };
        // The scroll container's direct children are what `ScrollHandle` can scroll to:
        // the songs' heading first, then one child per song row.
        let mut children: Vec<AnyElement> = Vec::new();
        if !results.songs.is_empty() {
            children.push(heading("Songs", false, cx));
            for (index, song) in results.songs.iter().enumerate() {
                children.push(
                    SongRow {
                        index,
                        title: song.track.title.clone().into(),
                        artists: artists_line(song),
                        duration: song
                            .track
                            .duration
                            .map(|d| format_time(d.as_millis() as u64)),
                        thumb: thumb_of(&song.thumbnail),
                        selected: list_focused && self.selected == Some(index),
                        accent,
                        on_play: {
                            let (player, track) = (self.player.clone(), song.track.clone());
                            Box::new(move |_, cx| {
                                player.update(cx, |m, cx| m.play_now(track.clone(), cx))
                            })
                        },
                        on_queue: {
                            let (player, track) = (self.player.clone(), song.track.clone());
                            Box::new(move |_, cx| {
                                player.update(cx, |m, cx| m.add_to_queue(track.clone(), cx))
                            })
                        },
                    }
                    .into_any_element(),
                );
            }
        }
        let groups: [(&'static str, &Vec<Hit>); 3] = [
            ("Albums", &results.albums),
            ("Artists", &results.artists),
            ("Playlists", &results.playlists),
        ];
        for (title, hits) in groups {
            if hits.is_empty() {
                continue;
            }
            children.push(heading(title, !children.is_empty(), cx));
            children.push(
                div()
                    .flex()
                    .flex_row()
                    .flex_wrap()
                    .gap(px(spacing::S4))
                    .children(hits.iter().enumerate().map(|(index, hit)| Card {
                        id: (title, index),
                        title: hit.title.clone().into(),
                        subtitle: hit.subtitle.clone().map(Into::into),
                        thumb: thumb_of(&hit.thumbnail),
                    }))
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .children(children)
            .into_any_element()
    }
}

fn artists_line(song: &ListedTrack) -> SharedString {
    song.track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}

/// Section heading: `section` type, with room above unless it opens the list.
fn heading(title: &'static str, spaced: bool, cx: &App) -> AnyElement {
    style::text_style(div(), typography::SECTION, cx)
        .flex_none()
        .when(spaced, |d| d.mt(px(spacing::S6)))
        .mb(px(spacing::S2))
        .text_color(color::text_primary())
        .child(title)
        .into_any_element()
}

/// Skeleton blocks at the final size of a song row, so nothing shifts when results arrive.
fn skeletons(cx: &App) -> AnyElement {
    let block = |w: gpui::Length, h: f32| {
        div()
            .w(w)
            .h(px(h))
            .rounded(px(radius::SMALL))
            .bg(color::skeleton())
    };
    let column = div()
        .flex()
        .flex_col()
        .children((0..SKELETON_ROWS).map(|_| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing::S3))
                .h(px(ROW_HEIGHT))
                .px(px(spacing::S2))
                .child(
                    div()
                        .size(px(ROW_ART))
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
                        .child(block(gpui::relative(0.4).into(), 12.0))
                        .child(block(gpui::relative(0.25).into(), 10.0)),
                )
        }));
    skeleton::pulse(column, "search-skeleton", !cx.reduce_motion())
}

type RowAction = Box<dyn Fn(&mut Window, &mut App) + 'static>;

/// One song: artwork that plays on click, title and artists (also play on click), duration, and
/// an add-to-queue button. The actions appear on hover and for the keyboard-selected row.
#[derive(IntoElement)]
struct SongRow {
    index: usize,
    title: SharedString,
    artists: SharedString,
    duration: Option<String>,
    thumb: Thumb,
    selected: bool,
    accent: Accent,
    on_play: RowAction,
    on_queue: RowAction,
}

impl RenderOnce for SongRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade =
            window.use_keyed_state(("song-row", self.index), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animate = !cx.reduce_motion();
        let active = level > 0.0 || self.selected;
        let play = std::rc::Rc::new(self.on_play);
        let play_art = play.clone();
        let play_info = play;
        let queue = self.on_queue;
        let accent = color::accent(self.accent);
        let id = self.index;

        let art = div()
            .id(("song-art", id))
            .relative()
            .size(px(ROW_ART))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .bg(color::bg_overlay())
            .cursor_pointer()
            .on_click(move |_, window, cx| play_art(window, cx))
            .child(tile(&self.thumb))
            .child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(color::bg_base().opacity(ART_SCRIM * level))
                    .child(
                        gpui::svg()
                            .path(Icon::Play.path())
                            .size(px(layout::ICON_SIZE))
                            .flex_none()
                            .text_color(color::text_primary().opacity(level)),
                    ),
            );

        let title = self.title.clone();
        let artists = self.artists.clone();
        let info = div()
            .id(("song-info", id))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .cursor_pointer()
            .on_click(move |_, window, cx| play_info(window, cx))
            .child(
                style::text_style(div(), typography::LIST_TITLE, cx)
                    .id(("song-title", id))
                    .truncate()
                    .text_color(color::text_primary())
                    .tooltip({
                        let full = title.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(title),
            )
            .when(!artists.is_empty(), |d| {
                d.child(
                    style::text_style(div(), typography::BODY, cx)
                        .id(("song-artists", id))
                        .truncate()
                        .text_color(color::text_secondary())
                        .tooltip({
                            let full = artists.clone();
                            move |_, cx| Tooltip::view(full.clone(), cx)
                        })
                        .child(artists),
                )
            });

        let duration = self.duration.map(|text| {
            style::tabular(style::text_style(div(), typography::BODY, cx))
                .flex_none()
                .text_color(color::text_tertiary())
                .child(text)
        });

        // The button's slot is always there, so nothing shifts when it appears.
        let add_focus = cx.focus_handle();
        let add = div()
            .size(px(layout::MIN_HIT_TARGET))
            .flex_none()
            .when(active, |d| {
                d.child(
                    IconButton::new(
                        ("song-queue", id),
                        &add_focus,
                        Icon::Add,
                        "Add to queue",
                        self.accent,
                    )
                    .on_click(move |window, cx| queue(window, cx)),
                )
            });

        let row_fade = fade.clone();
        div()
            .id(("song-row", id))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .role(gpui::Role::ListBoxOption)
            .aria_label(SharedString::from(format!(
                "{}, {}",
                self.title, self.artists
            )))
            .aria_selected(self.selected)
            .bg(if self.selected {
                color::state_selected()
            } else {
                color::state_hover().opacity(level)
            })
            .on_hover(move |hovered, _, cx| {
                row_fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
            })
            .when(self.selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(spacing::S2))
                        .bottom(px(spacing::S2))
                        .w(px(SELECTED_MARKER_WIDTH))
                        .rounded_full()
                        .bg(accent),
                )
            })
            .child(art)
            .child(info)
            .children(duration)
            .child(add)
    }
}

/// Artwork or the no-art tile: bg.overlay with a quiet music note (Shell decisions 3).
fn tile(thumb: &Thumb) -> AnyElement {
    match thumb {
        Thumb::Image(image) => img(ImageSource::Render(image.clone()))
            .size_full()
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        // Still downloading: the plain tile, nothing to animate.
        Thumb::Pending => div().into_any_element(),
        Thumb::Missing => gpui::svg()
            .path(Icon::MusicNote.path())
            .size(px(layout::ICON_SIZE))
            .flex_none()
            .text_color(color::text_tertiary())
            .into_any_element(),
    }
}

/// An album, artist or playlist: artwork over a title and a quiet second line. Shown only; there
/// is nothing to open yet.
#[derive(IntoElement)]
struct Card {
    id: (&'static str, usize),
    title: SharedString,
    subtitle: Option<SharedString>,
    thumb: Thumb,
}

impl RenderOnce for Card {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (group, index) = self.id;
        let title = self.title.clone();
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(spacing::S1))
            .w(px(CARD_WIDTH))
            .child(
                div()
                    .size(px(CARD_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(radius::MEDIUM))
                    .overflow_hidden()
                    .bg(color::bg_overlay())
                    .child(tile(&self.thumb)),
            )
            .child(
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id((group, index))
                    .truncate()
                    .text_color(color::text_primary())
                    .tooltip({
                        let full = title.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(title),
            )
            .children(self.subtitle.map(|text| {
                style::text_style(div(), typography::BODY, cx)
                    .id((group, index + 10_000))
                    .truncate()
                    .text_color(color::text_secondary())
                    .tooltip({
                        let full = text.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(text)
            }))
    }
}
