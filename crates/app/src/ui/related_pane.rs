//! The Related pane of Now Playing: what YouTube Music shows on the playing track's Related
//! tab. A shelf of songs is a list of rows like the queue's (click plays it with the shelf as
//! the queue, More and right-click open the track menu); every other shelf is a row of compact
//! cards that open an album, artist or playlist page.
//!
//! It keeps the previous track's shelves on screen until the new ones arrive. Loading is the
//! shared skeleton, a failure is one line with Retry, and a track with nothing related says so.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{HomeItem, HomeShelf};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, FocusHandle, IntoElement,
    ParentElement, Render, SharedString, Styled, Window,
};

use crate::library::model::{ArtistTarget, Target};
use crate::queue_source;
use crate::motion::HoverFade;
use crate::player::{format_time, PlayerModel};
use crate::related::{Phase, RelatedModel};
use crate::settings::RelatedOpenStore;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::card::{self, shaped_tile};
use crate::ui::playing_row;
use crate::ui::row_actions::more_slot;
use crate::ui::search_page::OpenTarget;
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::tooltip::Tooltip;
use crate::ui::track_menu::{self, Subject};
use crate::url_thumbs::{Thumb, ThumbStore};

/// Tab order: where the queue panel's stops are (they never show together).
const TAB_INDEX: isize = 13;
/// Song rows are the queue's: 56px with a 4px inset, 40px art.
const ROW_HEIGHT: f32 = 56.0;
const ROW_ART: f32 = 40.0;
/// Width of the slot at a row's right end: the duration at rest, More under the pointer.
const END_SLOT: f32 = spacing::S12;
/// Compact cards: 96px artwork inside an 8px inset.
const CARD_ART: f32 = 96.0;
const CARD_WIDTH: f32 = CARD_ART + 2.0 * card::INSET;
const SKELETON_ROWS: usize = 7;
/// Scrim over a song's art while it is the play target (the same as Search's rows).
const ART_SCRIM: f32 = 0.6;

pub struct RelatedPane {
    player: Entity<PlayerModel>,
    model: Entity<RelatedModel>,
    thumbs: Entity<ThumbStore>,
    on_open: OpenTarget,
    store: Option<Arc<dyn RelatedOpenStore>>,
    /// Related takes the right side of Now Playing.
    open: bool,
    /// One tab stop per item, grown to the most items the pane ever held.
    stops: Vec<FocusHandle>,
    focus_retry: FocusHandle,
}

impl RelatedPane {
    pub fn new(
        player: Entity<PlayerModel>,
        model: Entity<RelatedModel>,
        thumbs: Entity<ThumbStore>,
        on_open: impl Fn(Target, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            model,
            thumbs,
            on_open: Rc::new(on_open),
            store: None,
            open: false,
            stops: Vec::new(),
            focus_retry: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Where the choice is remembered between runs (none in tests).
    pub fn set_store(&mut self, store: Option<Arc<dyn RelatedOpenStore>>) {
        self.store = store;
    }

    /// Apply the remembered choice at startup. Does not save it again.
    pub fn restore_open(&mut self, saved: Option<bool>, cx: &mut Context<Self>) {
        if let Some(open) = saved {
            self.open = open;
            cx.notify();
        }
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        if let Some(store) = self.store.clone() {
            std::thread::spawn(move || store.save(open));
        }
        cx.notify();
    }

    fn message(&self, text: impl Into<SharedString>, cx: &App) -> AnyElement {
        style::text_style(div(), typography::BODY, cx)
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .flex()
            .items_center()
            .text_color(color::text_secondary())
            .child(text.into())
            .into_any_element()
    }

    fn failure(&self, failure: LoadFailure, accent: Accent, cx: &mut Context<Self>) -> AnyElement {
        let text = match failure {
            LoadFailure::Session => "Sign-in expired",
            LoadFailure::Network => "Connection problem",
            LoadFailure::Throttled => "Too many requests",
            _ => "Couldn't load related",
        };
        let model = self.model.clone();
        div()
            .id("related-error")
            .debug_selector(|| "related-error".into())
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S4))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child(text),
            )
            .child(
                div().debug_selector(|| "related-retry".into()).child(
                    TextButton::new("related-retry", &self.focus_retry, "Retry", accent)
                        .strong()
                        .on_click(move |_, cx| model.update(cx, |m, cx| m.retry(cx))),
                ),
            )
            .into_any_element()
    }

    fn skeleton(&self, animate: bool, cx: &App) -> AnyElement {
        let block = |w: gpui::Length, h: f32| {
            div()
                .w(w)
                .h(px(h))
                .rounded(px(radius::SMALL))
                .bg(color::state_hover().opacity(0.66))
        };
        let rows = (0..SKELETON_ROWS).map(|_| {
            div()
                .h(px(ROW_HEIGHT))
                .px(px(spacing::S4))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing::S3))
                .child(
                    div()
                        .size(px(ROW_ART))
                        .flex_none()
                        .rounded(px(radius::MEDIUM))
                        .bg(color::state_hover().opacity(0.66)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(spacing::S1))
                        .child(block(gpui::relative(0.7).into(), 12.0))
                        .child(block(gpui::relative(0.45).into(), 10.0)),
                )
        });
        let _ = cx;
        skeleton::pulse(
            div().flex().flex_col().children(rows),
            "related-skeleton",
            animate,
        )
    }

    /// The shelves, each a heading then its songs or cards.
    fn shelves(
        &mut self,
        shelves: &[Arc<HomeShelf>],
        accent: Accent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let total: usize = shelves.iter().map(|s| s.items.len()).sum();
        while self.stops.len() < total {
            self.stops
                .push(cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true));
        }
        let urls: Vec<String> = shelves
            .iter()
            .flat_map(|s| s.items.iter().filter_map(|i| i.thumbnail().map(str::to_owned)))
            .collect();
        self.thumbs.update(cx, |store, cx| {
            for url in &urls {
                store.request(url, cx);
            }
        });
        let playing = self.player.read(cx).current_video();
        let thumb_of = |item: &HomeItem, cx: &App| match item.thumbnail() {
            Some(url) => self.thumbs.read(cx).get(url),
            None => Thumb::Missing,
        };

        let mut stop = 0;
        let mut out = Vec::with_capacity(shelves.len());
        for (i, shelf) in shelves.iter().enumerate() {
            let heading = style::text_style(div(), typography::BODY_STRONG, cx)
                .px(px(spacing::S4))
                .pt(px(spacing::S4))
                .pb(px(spacing::S2))
                .text_color(color::text_secondary())
                .child(SharedString::from(shelf.title.clone()));
            let body: AnyElement = if shelf.is_songs() {
                let rows = shelf.items.iter().enumerate().filter_map(|(j, item)| {
                    let HomeItem::Song(song) = item else {
                        return None;
                    };
                    let focus = self.stops[stop + j].clone();
                    let (player, shelf) = (self.player.clone(), shelf.clone());
                    Some(
                        SongRow {
                            id: format!("related-{i}-{j}").into(),
                            title: song.track.title.clone().into(),
                            artists: artists_line(&song.track),
                            duration: song
                                .track
                                .duration
                                .map(|d| format_time(d.as_millis() as u64).into()),
                            thumb: thumb_of(item, cx),
                            playing: playing.as_deref() == Some(song.track.video_id.as_str()),
                            menu: Subject::track(song.track.clone(), song.thumbnail.clone()),
                            focus,
                            accent,
                            on_play: Rc::new(move |_, cx| play_from(&player, &shelf, j, cx)),
                            on_intent: Some(Rc::new({
                                let (player, track) = (self.player.clone(), song.track.clone());
                                move |cx: &mut App| {
                                    player.update(cx, |p, cx| p.hint_play(&track, cx));
                                }
                            })),
                        }
                        .into_any_element(),
                    )
                });
                div().flex().flex_col().children(rows).into_any_element()
            } else {
                let mut song_ix = 0usize;
                let cards: Vec<AnyElement> = shelf
                    .items
                    .iter()
                    .enumerate()
                    .map(|(j, item)| {
                        let focus = self.stops[stop + j].clone();
                        let id: SharedString = format!("related-{i}-{j}").into();
                        let thumb = thumb_of(item, cx);
                        self.card(id, item, &mut song_ix, shelf, thumb, focus, accent)
                    })
                    .collect();
                div()
                    .id(("related-shelf", i))
                    .role(gpui::Role::List)
                    .aria_label(SharedString::from(shelf.title.clone()))
                    .flex()
                    .flex_row()
                    .gap(px(spacing::S1))
                    .px(px(spacing::S4 - card::INSET))
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .children(cards)
                    .into_any_element()
            };
            stop += shelf.items.len();
            out.push(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(heading)
                    .child(body)
                    .into_any_element(),
            );
        }
        let _ = window;
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn card(
        &self,
        id: SharedString,
        item: &HomeItem,
        song_ix: &mut usize,
        shelf: &Arc<HomeShelf>,
        thumb: Thumb,
        focus: FocusHandle,
        accent: Accent,
    ) -> AnyElement {
        match item {
            HomeItem::Song(song) => {
                let start = *song_ix;
                *song_ix += 1;
                let (player, shelf) = (self.player.clone(), shelf.clone());
                let (hint, track) = (self.player.clone(), song.track.clone());
                MiniCard {
                    id,
                    title: song.track.title.clone().into(),
                    subtitle: artists_line(&song.track),
                    thumb,
                    circle: false,
                    menu: Some(Subject::track(song.track.clone(), song.thumbnail.clone())),
                    focus,
                    accent,
                    on_open: Rc::new(move |_, cx| play_from(&player, &shelf, start, cx)),
                    on_intent: Some(Rc::new(move |cx: &mut App| {
                        hint.update(cx, |p, cx| p.hint_play(&track, cx));
                    })),
                }
                .into_any_element()
            }
            HomeItem::Collection(c) => {
                let (target, on_open) = (Target::Collection(c.clone()), self.on_open.clone());
                MiniCard {
                    id,
                    title: c.title.clone().into(),
                    subtitle: c.subtitle.clone().map(Into::into),
                    thumb,
                    circle: false,
                    menu: Some(Subject::Collection(c.clone())),
                    focus,
                    accent,
                    on_open: Rc::new(move |window, cx| on_open(target.clone(), window, cx)),
                    on_intent: None,
                }
                .into_any_element()
            }
            HomeItem::Artist(h) => {
                let target = Target::Artist(ArtistTarget {
                    id: h.id.clone(),
                    name: h.title.clone(),
                    thumbnail: h.thumbnail.clone(),
                });
                let on_open = self.on_open.clone();
                MiniCard {
                    id,
                    title: h.title.clone().into(),
                    subtitle: h.subtitle.clone().map(Into::into),
                    thumb,
                    circle: true,
                    menu: None,
                    focus,
                    accent,
                    on_open: Rc::new(move |window, cx| on_open(target.clone(), window, cx)),
                    on_intent: None,
                }
                .into_any_element()
            }
        }
    }
}

/// Plays song `start` of `shelf` with the shelf's songs as the queue. `start` counts the shelf's
/// songs only (the cards of a mixed shelf do not take a place in it).
fn play_from(player: &Entity<PlayerModel>, shelf: &HomeShelf, start: usize, cx: &mut App) {
    let source = queue_source::named(shelf.title.clone());
    let songs = shelf.songs();
    let tracks: Vec<Track> = songs.iter().map(|s| s.track.clone()).collect();
    player.update(cx, |p, cx| {
        for song in &songs {
            p.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
        }
        p.play_tracks_from(tracks, Some(start), None, Some(source), cx);
    });
}

fn artists_line(track: &Track) -> Option<SharedString> {
    let line = track
        .artists
        .iter()
        .map(|a| a.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    (!line.is_empty()).then(|| line.into())
}

impl Render for RelatedPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let (phase, shelves) = {
            let m = self.model.read(cx);
            (m.phase(), m.shelves().to_vec())
        };
        let animate = !cx.reduce_motion();
        let body: Vec<AnyElement> = match phase {
            Phase::Idle => vec![self.message("Nothing playing", cx)],
            Phase::Loading => vec![self.skeleton(animate, cx)],
            Phase::Failed(f) => vec![self.failure(f, accent, cx)],
            Phase::Ready if shelves.is_empty() => vec![self.message("Nothing related", cx)],
            Phase::Ready => self.shelves(&shelves, accent, window, cx),
        };
        div()
            .id("related-pane")
            .debug_selector(|| "related-pane".into())
            .role(gpui::Role::Group)
            .aria_label("Related")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pb(px(spacing::S4))
            .children(body)
    }
}

// ---- song rows ----------------------------------------------------------------------------

/// One related song: art that plays on click, title over artists, then the duration (More under
/// the pointer). The whole row is the control.
#[derive(IntoElement)]
struct SongRow {
    id: SharedString,
    title: SharedString,
    artists: Option<SharedString>,
    duration: Option<SharedString>,
    thumb: Thumb,
    playing: bool,
    menu: Subject,
    focus: FocusHandle,
    accent: Accent,
    on_play: Rc<dyn Fn(&mut Window, &mut App) + 'static>,
    on_intent: Option<Rc<dyn Fn(&mut App) + 'static>>,
}

impl RenderOnce for SongRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animate = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let ring_color = color::accent(self.accent);
        let label: SharedString = match &self.artists {
            Some(a) => format!("{}, {a}", self.title).into(),
            None => self.title.clone(),
        };
        let (play, play_key) = (self.on_play.clone(), self.on_play.clone());
        let on_intent = self.on_intent.clone();
        let (id, title) = (self.id.clone(), self.title.clone());
        let menu = self.menu.clone();
        let hovered = level > 0.0;

        let art = div()
            .relative()
            .size(px(ROW_ART))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .bg(color::bg_overlay())
            .child(card::tile(&self.thumb))
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
                            .path(crate::assets::Icon::Play.path())
                            .size(px(layout::ICON_SIZE))
                            .flex_none()
                            .text_color(color::text_primary().opacity(level)),
                    ),
            );
        let title_style = playing_row::title_style(typography::BODY, self.playing);
        let info = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                style::text_style(div(), title_style, cx)
                    .id((id.clone(), 0usize))
                    .truncate()
                    .text_color(color::text_primary())
                    .tooltip({
                        let full = title.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(title),
            )
            .children(self.artists.map(|artists| {
                style::text_style(div(), typography::BODY, cx)
                    .id((id.clone(), 1usize))
                    .truncate()
                    .text_color(color::text_secondary())
                    .tooltip({
                        let full = artists.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(artists)
            }));
        // Always the same width, so the title keeps its room whichever one shows.
        let end = div()
            .w(px(END_SLOT))
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .map(|d| {
                if hovered {
                    d.child(more_slot(format!("{id}-more"), true, menu.clone()))
                } else {
                    d.children(self.duration.map(|text| {
                        style::text_style(div(), typography::CAPTION, cx)
                            .text_color(color::text_tertiary())
                            .child(text)
                    }))
                }
            });

        let row = div()
            .debug_selector({
                let id = id.to_string();
                move || id.clone()
            })
            .id(id)
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .w_full()
            .h(px(ROW_HEIGHT))
            .flex_none()
            .px(px(spacing::S4))
            .cursor_pointer()
            .bg(if self.playing {
                playing_row::tint()
            } else {
                color::state_hover().opacity(level)
            })
            .track_focus(&self.focus)
            .role(gpui::Role::Button)
            .aria_label(label)
            .on_hover(move |hovered, _, cx| {
                if *hovered {
                    if let Some(f) = &on_intent {
                        f(cx);
                    }
                }
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
            })
            .key_context(card::KEY_CONTEXT)
            .on_action(move |_: &card::OpenCard, window, cx| play_key(window, cx))
            .on_click(move |event, window, cx| {
                // The keyboard plays through `OpenCard`; Space is the play/pause key.
                if !matches!(event, ClickEvent::Keyboard(_)) {
                    play(window, cx);
                }
            })
            .when(self.playing, |d| {
                d.child(playing_row::marker(color::accent(self.accent)))
            })
            .child(art)
            .child(info)
            .child(end)
            .when(ring, move |d| {
                d.child(focus_ring(ring_color, radius::SMALL))
            });
        track_menu::on_context_menu(row, menu)
    }
}

// ---- compact cards ------------------------------------------------------------------------

/// A card of a shelf: 96px artwork over a title and a quiet second line.
#[derive(IntoElement)]
struct MiniCard {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    thumb: Thumb,
    /// An artist's artwork is a circle.
    circle: bool,
    menu: Option<Subject>,
    focus: FocusHandle,
    accent: Accent,
    on_open: Rc<dyn Fn(&mut Window, &mut App) + 'static>,
    on_intent: Option<Rc<dyn Fn(&mut App) + 'static>>,
}

impl RenderOnce for MiniCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animate = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let ring_color = color::accent(self.accent);
        let label: SharedString = match &self.subtitle {
            Some(s) => format!("{}, {s}", self.title).into(),
            None => self.title.clone(),
        };
        let (open, open_key) = (self.on_open.clone(), self.on_open.clone());
        let on_intent = self.on_intent.clone();
        let (id, title) = (self.id.clone(), self.title.clone());
        let menu = self.menu.clone();
        let circle = self.circle;
        let card = div()
            .debug_selector({
                let id = id.to_string();
                move || id.clone()
            })
            .id(id.clone())
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(spacing::S1))
            .w(px(CARD_WIDTH))
            .p(px(card::INSET))
            .rounded(px(radius::MEDIUM))
            .bg(color::state_hover().opacity(level))
            .cursor_pointer()
            .track_focus(&self.focus)
            .role(gpui::Role::Button)
            .aria_label(label)
            .on_hover(move |hovered, _, cx| {
                if *hovered {
                    if let Some(f) = &on_intent {
                        f(cx);
                    }
                }
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
            })
            .key_context(card::KEY_CONTEXT)
            .on_action(move |_: &card::OpenCard, window, cx| open_key(window, cx))
            .on_click(move |event, window, cx| {
                if !matches!(event, ClickEvent::Keyboard(_)) {
                    open(window, cx);
                }
            })
            .child(
                div()
                    .size(px(CARD_ART))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| {
                        if circle {
                            d.rounded_full()
                        } else {
                            d.rounded(px(radius::MEDIUM))
                        }
                    })
                    .overflow_hidden()
                    .bg(color::bg_overlay())
                    .child(shaped_tile(&self.thumb, circle)),
            )
            .child(
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id((id.clone(), 0usize))
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
                    .id((id.clone(), 1usize))
                    .truncate()
                    .text_color(color::text_secondary())
                    .tooltip({
                        let full = text.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(text)
            }))
            .when(ring, move |d| d.child(focus_ring(ring_color, radius::MEDIUM)));
        match menu {
            Some(subject) => track_menu::on_context_menu(card, subject).into_any_element(),
            None => card.into_any_element(),
        }
    }
}
