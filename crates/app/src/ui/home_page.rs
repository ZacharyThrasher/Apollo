//! The Home page: the account's shelves, each a titled row you scroll sideways.
//!
//! A shelf of nothing but songs ("Quick picks") is a grid of compact song rows, four tall and
//! paging by column; every other shelf is a row of cards. Hovering a row shows previous/next
//! chevrons at its edges; each shelf is one Tab stop and Left and Right walk its items (roving
//! focus), and the row and page scroll the focused item into view. A card opens its album, artist or playlist
//! page; a song plays with its shelf as the queue.
//!
//! Tab order: one stop per shelf, at index 6 after the nav rail, in reading order.

use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{HomeItem, HomeShelf};
use apollo_theme::tokens::{layout, motion, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, point, prelude::*, px, AnyElement, App, Context, ElementId, Entity, FocusHandle,
    IntoElement, KeyBinding, ParentElement, Render, ScrollHandle, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::home::{HomeModel, More, Phase};
use crate::library::model::{ArtistTarget, Target};
use crate::queue_source;
use crate::motion::HoverFade;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::card::{self, heading, tile, Card};
use crate::ui::playing_row;
use crate::ui::search_page::OpenTarget;
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::row_actions::{more_slot, ACTION_SIZE};
use crate::ui::tooltip::Tooltip;
use crate::ui::track_menu::{self, Subject};
use crate::url_thumbs::{Thumb, ThumbStore};

actions!(home, [ShelfNext, ShelfPrevious]);

/// Key context of a shelf: Left and Right move to the neighbouring item.
pub const SHELF_CONTEXT: &str = "HomeShelf";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("right", ShelfNext, Some(SHELF_CONTEXT)),
        KeyBinding::new("left", ShelfPrevious, Some(SHELF_CONTEXT)),
    ]);
}

/// Every stop on the page: after the nav rail (1-5), in the slots Now Playing uses (they never
/// show together).
const TAB_INDEX: isize = 6;
/// Space between cards, and between the columns of a song grid.
const GAP: f32 = spacing::S4;
/// What one card advances a row by.
const CARD_STEP: f32 = card::WIDTH + GAP;
/// Compact song rows: 40px art with 4px above and below.
const SONG_ROW: f32 = 48.0;
const SONG_ART: f32 = 40.0;
/// Rows in a column of the song grid.
const COLUMN_ROWS: usize = 4;
const COLUMN_WIDTH: f32 = 320.0;
const COLUMN_STEP: f32 = COLUMN_WIDTH + GAP;
/// Room inside a scrolling row for the focus ring, which sits outside what it rings.
const RING_ROOM: f32 = layout::FOCUS_RING_OFFSET + layout::FOCUS_RING_WIDTH;
/// Scrim over a song's art while it is the play target (the same as Search's rows).
const ART_SCRIM: f32 = 0.6;
/// The next batch of shelves is asked for when less than this much page is left below.
const LOAD_MORE_MARGIN: f32 = 600.0;
/// Artwork is fetched for rows this far above and below the window.
const ART_MARGIN: f32 = 400.0;
/// Before a row has been measured, this many columns are assumed to be on screen.
const ASSUMED_VISIBLE: f32 = 6.0;
const SKELETON_CARDS: usize = 6;

/// The pixel step of a row: one card, or one column of songs.
fn step_of(songs: bool) -> f32 {
    if songs {
        COLUMN_STEP
    } else {
        CARD_STEP
    }
}

/// Where a chevron click sends a row: a whole number of steps, as many as fit on screen (at
/// least one), kept inside the row. `direction` is +1 for next and -1 for previous; offsets are
/// zero at the start and negative as the row scrolls.
pub fn page_target(offset: f32, max: f32, viewport: f32, step: f32, direction: f32) -> f32 {
    let steps = (viewport / step).floor().max(1.0);
    let raw = (offset - direction * steps * step).clamp(-max, 0.0);
    // Land on a step, except at the far end where the last stretch is what is left.
    let snapped = (raw / step).round() * step;
    snapped.clamp(-max, 0.0).min(0.0)
}

/// The items of a row to fetch artwork for: those on screen and one step either side.
pub fn visible_range(offset: f32, viewport: f32, step: f32, count: usize) -> Range<usize> {
    let viewport = if viewport <= 0.0 {
        step * ASSUMED_VISIBLE
    } else {
        viewport
    };
    let left = -offset.min(0.0);
    let first = (left / step).floor().max(0.0) as usize;
    let last = ((left + viewport) / step).ceil() as usize + 1;
    first.saturating_sub(1)..last.min(count)
}

/// A sideways scroll in progress.
pub(crate) struct Slide {
    pub(crate) from: f32,
    pub(crate) to: f32,
    pub(crate) started: Instant,
    /// The offset last written, to tell the page's own movement from a wheel or a focus jump.
    pub(crate) written: f32,
}

struct Row {
    scroll: ScrollHandle,
    slide: Option<Slide>,
    /// The one item of the shelf that Tab stops on: the one last focused (roving focus).
    tab: usize,
}

impl Row {
    fn new() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            slide: None,
            tab: 0,
        }
    }
}

pub struct HomeView {
    player: Entity<PlayerModel>,
    model: Entity<HomeModel>,
    thumbs: Entity<ThumbStore>,
    on_open: OpenTarget,
    on_sign_in: Rc<dyn Fn(&mut Window, &mut App)>,
    /// The whole page scrolls; its children are the title, then one per shelf.
    page: ScrollHandle,
    rows: Vec<Row>,
    /// The tab stops of the items, grown to the most items the page ever held.
    stops: Vec<FocusHandle>,
    action_focus: FocusHandle,
    /// The stop that last took keyboard focus and was scrolled into view.
    scrolled_to: Option<usize>,
}

impl HomeView {
    pub fn new(
        player: Entity<PlayerModel>,
        model: Entity<HomeModel>,
        thumbs: Entity<ThumbStore>,
        on_open: impl Fn(Target, &mut Window, &mut App) + 'static,
        on_sign_in: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            model,
            thumbs,
            on_open: Rc::new(on_open),
            on_sign_in: Rc::new(on_sign_in),
            page: ScrollHandle::new(),
            rows: Vec::new(),
            stops: Vec::new(),
            action_focus: cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true),
            scrolled_to: None,
        }
    }

    /// How far row `row` is scrolled sideways: zero at its start, negative as it scrolls (tests).
    pub fn row_offset(&self, row: usize) -> f32 {
        self.rows
            .get(row)
            .map_or(0.0, |r| f32::from(r.scroll.offset().x))
    }

    /// Every stop of the page by index, in reading order (tests).
    pub fn item_stops(&self) -> &[FocusHandle] {
        &self.stops
    }

    /// The Retry / Sign in stop (tests).
    pub fn action_stop(&self) -> &FocusHandle {
        &self.action_focus
    }

    /// Left / Right: move focus to the next or previous item of the shelf whose items are stops
    /// `first..first + count`, staying inside it.
    fn step_focus(
        &mut self,
        first: usize,
        count: usize,
        by: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = (0..count).find(|k| self.stops[first + k].is_focused(window)) else {
            return;
        };
        let to = at as isize + by;
        if (0..count as isize).contains(&to) {
            window.focus(&self.stops[first + to as usize], cx);
        }
    }

    /// Back to the top, every row at its first column (a different account's feed).
    pub fn rewind(&mut self, cx: &mut Context<Self>) {
        self.rows.clear();
        self.scrolled_to = None;
        self.page.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// A chevron was pressed: slide row `row` one page toward `direction`.
    fn slide_row(
        &mut self,
        row: usize,
        songs: bool,
        direction: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.rows.get_mut(row) else {
            return;
        };
        let offset = f32::from(r.scroll.offset().x);
        let target = page_target(
            offset,
            f32::from(r.scroll.max_offset().x),
            f32::from(r.scroll.bounds().size.width),
            step_of(songs),
            direction,
        );
        if (target - offset).abs() < 0.5 {
            return;
        }
        if cx.reduce_motion() {
            r.slide = None;
            r.scroll.set_offset(point(px(target), r.scroll.offset().y));
        } else {
            r.slide = Some(Slide {
                from: offset,
                to: target,
                started: Instant::now(),
                written: offset,
            });
        }
        window.refresh();
        cx.notify();
    }

    /// Advances every slide to now. A row scrolled by something else (wheel, focus) in the
    /// meantime is left where that put it.
    fn tick_slides(&mut self, window: &mut Window) {
        let now = Instant::now();
        for r in &mut self.rows {
            let Some(slide) = r.slide.as_mut() else {
                continue;
            };
            let here = f32::from(r.scroll.offset().x);
            if (here - slide.written).abs() > 1.0 {
                r.slide = None;
                continue;
            }
            let t = now.saturating_duration_since(slide.started).as_secs_f32()
                / motion::PANEL.as_secs_f32();
            let x = slide.from + (slide.to - slide.from) * motion::ease_out_cubic(t);
            r.scroll.set_offset(point(px(x), r.scroll.offset().y));
            slide.written = x;
            if t >= 1.0 {
                r.slide = None;
            } else {
                window.request_animation_frame();
            }
        }
    }
}

/// What one shelf needs to be drawn, worked out before any element is built.
struct Plan {
    shelf: Arc<HomeShelf>,
    songs: bool,
    /// Items whose art is on screen or about to be.
    wanted: Range<usize>,
    /// How far the pointer's presence has faded the chevrons in.
    hover: Entity<HoverFade>,
    hover_level: f32,
    can_back: bool,
    can_forward: bool,
}

impl Render for HomeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let animate = !cx.reduce_motion();
        let (phase, more, has_more, shelves) = {
            let m = self.model.read(cx);
            (m.phase(), m.more(), m.has_more(), m.display_shelves())
        };

        let title = style::text_style(div(), typography::PAGE_TITLE, cx)
            .text_color(color::text_primary())
            .child("Home");
        let page = div()
            .id("home-page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            // A sideways swipe belongs to the row under the pointer, not to the page.
            .restrict_scroll_to_axis()
            .track_scroll(&self.page)
            .px(px(spacing::S8))
            .pt(px(spacing::S6))
            .child(title);

        match phase {
            Phase::SignedOut => page.child(self.signed_out(accent)),
            Phase::Loading => page.child(skeleton_page(animate, cx)),
            Phase::Failed(failure) => {
                let model = self.model.clone();
                page.child(self.notice(
                    failure_line(failure),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry(cx)),
                    cx,
                ))
            }
            Phase::Ready if shelves.is_empty() => {
                let model = self.model.clone();
                page.child(self.notice(
                    "Nothing to show".into(),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry(cx)),
                    cx,
                ))
            }
            Phase::Ready => {
                let body = self.shelves(&shelves, accent, window, cx);
                self.ask_for_more(has_more, more, cx);
                let footer = self.footer(more, accent, animate, cx);
                page.children(body).children(footer).child(
                    // Room under the last row.
                    div().h(px(spacing::S6)).flex_none(),
                )
            }
        }
    }
}

impl HomeView {
    fn signed_out(&self, accent: Accent) -> AnyElement {
        let on_sign_in = self.on_sign_in.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .mt(px(spacing::S4))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .debug_selector(|| "home-sign-in".into())
                    .child(
                        TextButton::new("home-sign-in", &self.action_focus, "Sign in", accent)
                            .primary()
                            .on_click(move |window, cx| on_sign_in(window, cx)),
                    ),
            )
            .into_any_element()
    }

    /// One short line and one action: the error and empty states (the row the Library uses).
    fn notice(
        &self,
        text: SharedString,
        action: &'static str,
        accent: Accent,
        on_action: impl Fn(&mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        let warning = text.as_ref() != "Nothing to show";
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .mt(px(spacing::S4))
            .h(px(layout::MIN_HIT_TARGET))
            .when(warning, |d| {
                d.child(
                    gpui::svg()
                        .path(Icon::Warning.path())
                        .size(px(layout::ICON_SIZE))
                        .flex_none()
                        .text_color(color::warning()),
                )
            })
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child(text),
            )
            .child(
                div().debug_selector(|| "home-notice-action".into()).child(
                    TextButton::new(ElementId::from(action), &self.action_focus, action, accent)
                        .on_click(on_action),
                ),
            )
            .into_any_element()
    }

    /// Below the last shelf: the next batch loading, or what went wrong with it.
    fn footer(&self, more: More, accent: Accent, animate: bool, cx: &App) -> Option<AnyElement> {
        match more {
            More::Idle => None,
            More::Loading => Some(skeleton_shelf(true, animate, cx)),
            More::Failed(_) => {
                let model = self.model.clone();
                Some(self.notice(
                    "Couldn't load more".into(),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry_more(cx)),
                    cx,
                ))
            }
        }
    }

    /// Asks for the next batch of shelves once the page is nearly scrolled to its end (or does
    /// not fill the window yet).
    fn ask_for_more(&self, has_more: bool, more: More, cx: &mut Context<Self>) {
        if !has_more || more != More::Idle {
            return;
        }
        let left = f32::from(self.page.max_offset().y) + f32::from(self.page.offset().y);
        if left < LOAD_MORE_MARGIN {
            self.model.update(cx, |m, cx| m.load_more(cx));
        }
    }

    /// Whether shelf `child` of the page (the title is child 0) is near the window.
    fn near_window(&self, child: usize) -> bool {
        let Some(bounds) = self.page.bounds_for_item(child) else {
            // Not measured yet: the first few shelves are what a window shows.
            return child <= 4;
        };
        let (top, bottom) = (self.page.bounds().top(), self.page.bounds().bottom());
        let offset = self.page.offset().y;
        let margin = px(ART_MARGIN);
        bounds.top() + offset < bottom + margin && bounds.bottom() + offset > top - margin
    }

    fn shelves(
        &mut self,
        shelves: &[Arc<HomeShelf>],
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        // ---- plan: everything that needs the window or a mutable app ----------------------
        while self.rows.len() < shelves.len() {
            self.rows.push(Row::new());
        }
        let total: usize = shelves.iter().map(|s| s.items.len()).sum();
        while self.stops.len() < total {
            self.stops
                .push(cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true));
        }
        self.tick_slides(window);
        let animate = !cx.reduce_motion();
        let plans: Vec<Plan> = shelves
            .iter()
            .enumerate()
            .map(|(i, shelf)| {
                let songs = shelf.is_songs();
                let row = &self.rows[i];
                let hover =
                    window.use_keyed_state(("home-row", i), cx, |_, _| HoverFade::default());
                let (hover_level, moving) = hover.read(cx).level(Instant::now());
                if moving {
                    window.request_animation_frame();
                }
                let offset = f32::from(row.scroll.offset().x);
                let max = f32::from(row.scroll.max_offset().x);
                let step = step_of(songs);
                let columns = if songs {
                    shelf.items.len().div_ceil(COLUMN_ROWS)
                } else {
                    shelf.items.len()
                };
                let cols = visible_range(
                    offset,
                    f32::from(row.scroll.bounds().size.width),
                    step,
                    columns,
                );
                let wanted = if !self.near_window(1 + i) {
                    0..0
                } else if songs {
                    (cols.start * COLUMN_ROWS)..(cols.end * COLUMN_ROWS).min(shelf.items.len())
                } else {
                    cols
                };
                Plan {
                    shelf: shelf.clone(),
                    songs,
                    wanted,
                    hover,
                    hover_level,
                    can_back: offset < -1.0,
                    can_forward: offset > -max + 1.0,
                }
            })
            .collect();

        // Keyboard focus moves the row and the page to the focused item.
        let mut focused = None;
        let mut stop = 0;
        for (i, plan) in plans.iter().enumerate() {
            for j in 0..plan.shelf.items.len() {
                if self.stops[stop].is_focused(window) {
                    self.rows[i].tab = j;
                }
                if keyboard_focused(&self.stops[stop], window) {
                    focused = Some((stop, i, if plan.songs { j / COLUMN_ROWS } else { j }));
                }
                stop += 1;
            }
        }
        match focused {
            Some((stop, row, column)) if self.scrolled_to != Some(stop) => {
                self.scrolled_to = Some(stop);
                self.rows[row].scroll.scroll_to_item(column);
                self.page.scroll_to_item(1 + row);
            }
            Some(_) => {}
            None => self.scrolled_to = None,
        }

        // Artwork for what is on screen; the store keeps a bounded number.
        let urls: Vec<String> = plans
            .iter()
            .flat_map(|p| {
                p.shelf.items[p.wanted.clone()]
                    .iter()
                    .filter_map(|i| i.thumbnail().map(str::to_owned))
            })
            .collect();
        self.thumbs.update(cx, |store, cx| {
            for url in &urls {
                store.request(url, cx);
            }
        });

        // ---- build ---------------------------------------------------------------------
        let playing = self.player.read(cx).current_video();
        let view = cx.entity();
        let mut out = Vec::with_capacity(plans.len());
        let mut stop = 0;
        for (i, plan) in plans.into_iter().enumerate() {
            let count = plan.shelf.items.len();
            let tab = self.rows[i].tab.min(count.saturating_sub(1));
            let handles: Vec<FocusHandle> = self.stops[stop..stop + count]
                .iter()
                .enumerate()
                .map(|(j, h)| h.clone().tab_stop(j == tab))
                .collect();
            let first = stop;
            stop += count;
            let thumb_of = |item: &HomeItem| -> Thumb {
                match item.thumbnail() {
                    Some(url) => self.thumbs.read(cx).get(url),
                    None => Thumb::Missing,
                }
            };
            let thumbs: Vec<Thumb> = plan.shelf.items.iter().map(thumb_of).collect();
            let content = if plan.songs {
                self.song_grid(i, &plan, &handles, thumbs, playing.as_deref(), accent)
            } else {
                self.card_row(i, &plan, &handles, thumbs, accent)
            };
            let row = &self.rows[i];
            let songs = plan.songs;
            let (hover, level) = (plan.hover.clone(), plan.hover_level);
            let chevron_top = if songs {
                (SONG_ROW * COLUMN_ROWS as f32) / 2.0 - layout::MIN_HIT_TARGET / 2.0
            } else {
                card::INSET + card::ART / 2.0 - layout::MIN_HIT_TARGET / 2.0
            };
            let button = |direction: f32, icon: Icon, label: &'static str| {
                let view = view.clone();
                let name = if direction < 0.0 {
                    "home-previous"
                } else {
                    "home-next"
                };
                ScrollButton {
                    id: (name, i).into(),
                    selector: format!("{name}-{i}"),
                    icon,
                    label,
                    on_click: Box::new(move |window, cx| {
                        view.update(cx, |this, cx| {
                            this.slide_row(i, songs, direction, window, cx)
                        })
                    }),
                }
            };
            let wrapper = div()
                .id(("home-row", i))
                .relative()
                .on_hover(move |hovered, _, cx| {
                    hover.update(cx, |fade, cx| {
                        fade.set(*hovered, Instant::now(), animate);
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .id(("home-shelf", i))
                        .role(gpui::Role::List)
                        .aria_label(SharedString::from(plan.shelf.title.clone()))
                        .key_context(SHELF_CONTEXT)
                        .on_action(cx.listener(move |this, _: &ShelfNext, window, cx| {
                            this.step_focus(first, count, 1, window, cx)
                        }))
                        .on_action(cx.listener(move |this, _: &ShelfPrevious, window, cx| {
                            this.step_focus(first, count, -1, window, cx)
                        }))
                        .flex()
                        .flex_row()
                        .gap(px(GAP))
                        // Room for the focus ring, taken back with a negative margin so the art
                        // still lines up with the heading.
                        .px(px(RING_ROOM))
                        .py(px(RING_ROOM))
                        .mx(px(-(card::INSET + RING_ROOM)))
                        .my(px(-RING_ROOM))
                        .overflow_x_scroll()
                        .restrict_scroll_to_axis()
                        .track_scroll(&row.scroll)
                        .children(content),
                )
                .when(level > 0.0 && plan.can_back, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(spacing::S1))
                            .top(px(chevron_top))
                            .opacity(level)
                            .child(button(-1.0, Icon::ChevronLeft, "Previous")),
                    )
                })
                .when(level > 0.0 && plan.can_forward, |d| {
                    d.child(
                        div()
                            .absolute()
                            .right(px(spacing::S1))
                            .top(px(chevron_top))
                            .opacity(level)
                            .child(button(1.0, Icon::ChevronRight, "Next")),
                    )
                });
            out.push(
                div()
                    .child(heading(plan.shelf.title.clone(), true, cx))
                    .child(wrapper)
                    .into_any_element(),
            );
        }
        out
    }

    /// A shelf of cards: albums, playlists and artists open their pages, a song plays.
    fn card_row(
        &self,
        shelf_ix: usize,
        plan: &Plan,
        handles: &[FocusHandle],
        thumbs: Vec<Thumb>,
        accent: Accent,
    ) -> Vec<AnyElement> {
        let mut song_ix = 0usize;
        plan.shelf
            .items
            .iter()
            .zip(handles)
            .zip(thumbs)
            .enumerate()
            .map(|(j, ((item, focus), thumb))| {
                let id = format!("home-{shelf_ix}-{j}");
                match item {
                    HomeItem::Song(song) => {
                        let start = song_ix;
                        song_ix += 1;
                        let (player, shelf) = (self.player.clone(), plan.shelf.clone());
                        let (hint_player, track) = (self.player.clone(), song.track.clone());
                        Card::new(
                            id,
                            song.track.title.clone(),
                            artists_line(&song.track),
                            thumb,
                            focus,
                            accent,
                            move |_, cx| play_from(&player, &shelf, start, cx),
                        )
                        .on_intent(move |cx| {
                            hint_player.update(cx, |p, cx| p.hint_play(&track, cx));
                        })
                        .menu(Subject::track(song.track.clone(), song.thumbnail.clone()))
                        .into_any_element()
                    }
                    HomeItem::Collection(c) => {
                        let (target, on_open) =
                            (Target::Collection(c.clone()), self.on_open.clone());
                        Card::new(
                            id,
                            c.title.clone(),
                            c.subtitle.clone().map(Into::into),
                            thumb,
                            focus,
                            accent,
                            move |window, cx| on_open(target.clone(), window, cx),
                        )
                        .menu(Subject::Collection(c.clone()))
                        .into_any_element()
                    }
                    HomeItem::Artist(h) => {
                        let target = Target::Artist(ArtistTarget {
                            id: h.id.clone(),
                            name: h.title.clone(),
                            thumbnail: h.thumbnail.clone(),
                        });
                        let on_open = self.on_open.clone();
                        Card::new(
                            id,
                            h.title.clone(),
                            h.subtitle.clone().map(Into::into),
                            thumb,
                            focus,
                            accent,
                            move |window, cx| on_open(target.clone(), window, cx),
                        )
                        .circular(true)
                        .into_any_element()
                    }
                }
            })
            .collect()
    }

    /// A shelf of songs: columns of four compact rows, filled top to bottom.
    fn song_grid(
        &self,
        shelf_ix: usize,
        plan: &Plan,
        handles: &[FocusHandle],
        thumbs: Vec<Thumb>,
        playing: Option<&str>,
        accent: Accent,
    ) -> Vec<AnyElement> {
        let rows: Vec<AnyElement> = plan
            .shelf
            .items
            .iter()
            .zip(handles)
            .zip(thumbs)
            .enumerate()
            .filter_map(|(j, ((item, focus), thumb))| {
                let HomeItem::Song(song) = item else {
                    return None;
                };
                let (player, shelf) = (self.player.clone(), plan.shelf.clone());
                Some(
                    SongTile {
                        id: format!("home-{shelf_ix}-{j}").into(),
                        title: song.track.title.clone().into(),
                        artists: artists_line(&song.track).unwrap_or_default(),
                        thumb,
                        playing: playing == Some(song.track.video_id.as_str()),
                        menu: Subject::track(song.track.clone(), song.thumbnail.clone()),
                        focus: focus.clone(),
                        accent,
                        width: COLUMN_WIDTH,
                        on_play: Rc::new(move |_, cx| play_from(&player, &shelf, j, cx)),
                        on_intent: Some(Rc::new({
                            let (hint_player, track) = (self.player.clone(), song.track.clone());
                            move |cx: &mut App| {
                                hint_player.update(cx, |p, cx| p.hint_play(&track, cx));
                            }
                        })),
                    }
                    .into_any_element(),
                )
            })
            .collect();
        let mut columns = Vec::new();
        let mut rows = rows.into_iter().peekable();
        while rows.peek().is_some() {
            columns.push(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .w(px(COLUMN_WIDTH))
                    .children(rows.by_ref().take(COLUMN_ROWS))
                    .into_any_element(),
            );
        }
        columns
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

/// What went wrong in six words or fewer (DIRECTION.md, States and copy).
fn failure_line(failure: LoadFailure) -> SharedString {
    match failure {
        LoadFailure::Session => "Sign-in expired".into(),
        LoadFailure::Throttled => "Too many requests".into(),
        _ => "Couldn't load Home".into(),
    }
}

// ---- song rows ----------------------------------------------------------------------------

/// One compact song: art that plays on click, then title and artists. The whole row is the
/// control, like a card.
#[derive(IntoElement)]
pub(crate) struct SongTile {
    pub(crate) id: SharedString,
    pub(crate) title: SharedString,
    pub(crate) artists: SharedString,
    pub(crate) thumb: Thumb,
    pub(crate) playing: bool,
    /// What the More button and a right-click offer.
    pub(crate) menu: Subject,
    pub(crate) focus: FocusHandle,
    pub(crate) accent: Accent,
    /// The row's width: a column of Home's grid, or wider where a list stands alone.
    pub(crate) width: f32,
    pub(crate) on_play: Rc<dyn Fn(&mut Window, &mut App) + 'static>,
    /// The pointer arrived: let the player start resolving the song before the click.
    pub(crate) on_intent: Option<Rc<dyn Fn(&mut App) + 'static>>,
}

impl RenderOnce for SongTile {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animate = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let ring_color = color::accent(self.accent);
        let label: SharedString = if self.artists.is_empty() {
            self.title.clone()
        } else {
            format!("{}, {}", self.title, self.artists).into()
        };
        let (play, play_key) = (self.on_play.clone(), self.on_play.clone());
        let on_intent = self.on_intent.clone();
        let (id, title, artists) = (self.id.clone(), self.title.clone(), self.artists.clone());
        let menu = self.menu.clone();

        let art = div()
            .relative()
            .size(px(SONG_ART))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .bg(color::bg_overlay())
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
        let title_style = playing_row::title_style(typography::LIST_TITLE, self.playing);
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
            .when(!artists.is_empty(), |d| {
                d.child(
                    style::text_style(div(), typography::BODY, cx)
                        .id((id.clone(), 1usize))
                        .truncate()
                        .text_color(color::text_secondary())
                        .tooltip({
                            let full = artists.clone();
                            move |_, cx| Tooltip::view(full.clone(), cx)
                        })
                        .child(artists),
                )
            });

        // The More button sits over the row's right end while the pointer is on it.
        let more = more_slot(format!("{id}-more"), level > 0.0, menu.clone())
            .absolute()
            .right(px(spacing::S1))
            .top(px((SONG_ROW - ACTION_SIZE) / 2.0));

        let tile = div()
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
            .w(px(self.width))
            .h(px(SONG_ROW))
            .flex_none()
            .px(px(spacing::S1))
            .rounded(px(radius::SMALL))
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
                if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    play(window, cx);
                }
            })
            .when(self.playing, |d| {
                d.child(playing_row::marker(color::accent(self.accent)))
            })
            .child(art)
            .child(info)
            .child(more)
            .when(ring, move |d| {
                d.child(focus_ring(ring_color, radius::SMALL))
            });
        track_menu::on_context_menu(tile, menu)
    }
}

// ---- chevrons -----------------------------------------------------------------------------

/// A row's previous/next button: a 32px circle on bg.overlay that shows while the pointer is
/// over the row. Not a tab stop: the keyboard moves through the items instead.
#[derive(IntoElement)]
pub(crate) struct ScrollButton {
    pub(crate) id: ElementId,
    /// Names the button for the headless tests (`home-next-<shelf>`).
    pub(crate) selector: String,
    pub(crate) icon: Icon,
    pub(crate) label: &'static str,
    pub(crate) on_click: Box<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl RenderOnce for ScrollButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animate = !cx.reduce_motion();
        let on_click = self.on_click;
        let selector = self.selector;
        div()
            .debug_selector(move || selector.clone())
            .id(self.id)
            .relative()
            .size(px(layout::MIN_HIT_TARGET))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_strong())
            .cursor_pointer()
            .role(gpui::Role::Button)
            .aria_label(self.label)
            .on_hover(move |hovered, _, cx| {
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
            })
            .on_click(move |_, window, cx| on_click(window, cx))
            .child(
                div()
                    .id("tint")
                    .absolute()
                    .size_full()
                    .rounded_full()
                    .bg(color::state_hover().opacity(level))
                    .active(|s| s.bg(color::state_pressed())),
            )
            .child(
                gpui::svg()
                    .path(self.icon.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(color::text_secondary()),
            )
    }
}

// ---- skeletons ----------------------------------------------------------------------------

fn block(width: gpui::Length, height: f32) -> gpui::Div {
    div()
        .w(width)
        .h(px(height))
        .rounded(px(radius::SMALL))
        .bg(color::skeleton())
}

/// What a card is while it loads: the same inset, art square and two text lines, so nothing
/// moves when the real cards arrive.
fn skeleton_card(cx: &App) -> AnyElement {
    let line = |style| {
        style::text_style(div(), style, cx)
            .w(px(card::ART * 0.7))
            .child("\u{00A0}")
    };
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(px(spacing::S1))
        .w(px(card::WIDTH))
        .p(px(card::INSET))
        .child(
            div()
                .size(px(card::ART))
                .flex_none()
                .rounded(px(radius::MEDIUM))
                .bg(color::skeleton()),
        )
        .child(
            line(typography::BODY_STRONG)
                .rounded(px(radius::SMALL))
                .bg(color::skeleton()),
        )
        .child(
            line(typography::BODY)
                .w(px(card::ART * 0.45))
                .rounded(px(radius::SMALL))
                .bg(color::skeleton()),
        )
        .into_any_element()
}

fn skeleton_song(cx: &App) -> AnyElement {
    let line = |style, width: f32| {
        style::text_style(div(), style, cx)
            .w(px(width))
            .rounded(px(radius::SMALL))
            .bg(color::skeleton())
            .child("\u{00A0}")
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S3))
        .w(px(COLUMN_WIDTH))
        .h(px(SONG_ROW))
        .flex_none()
        .px(px(spacing::S1))
        .child(
            div()
                .size(px(SONG_ART))
                .flex_none()
                .rounded(px(radius::MEDIUM))
                .bg(color::skeleton()),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(spacing::S1))
                .child(line(typography::LIST_TITLE, COLUMN_WIDTH * 0.5))
                .child(line(typography::BODY, COLUMN_WIDTH * 0.3)),
        )
        .into_any_element()
}

/// A heading block and a row at the final size of a shelf of cards (or of songs).
pub(crate) fn skeleton_shelf(songs: bool, animate: bool, cx: &App) -> AnyElement {
    let heading = style::text_style(div(), typography::SECTION, cx)
        .mt(px(spacing::S6))
        .mb(px(spacing::S2))
        .child(block(px(card::ART).into(), 20.0).my(px(3.0)));
    let row = if songs {
        div()
            .flex()
            .flex_row()
            .gap(px(GAP))
            .children((0..2).map(|_| {
                div()
                    .flex()
                    .flex_col()
                    .children((0..COLUMN_ROWS).map(|_| skeleton_song(cx)))
            }))
    } else {
        div()
            .flex()
            .flex_row()
            .gap(px(GAP))
            .ml(px(-card::INSET))
            .children((0..SKELETON_CARDS).map(|_| skeleton_card(cx)))
    };
    let shelf = div().child(heading).child(row);
    skeleton::pulse(
        shelf,
        if songs {
            "home-skeleton-songs"
        } else {
            "home-skeleton-cards"
        },
        animate,
    )
}

/// The page while the first batch loads: a song shelf, then a row of cards.
fn skeleton_page(animate: bool, cx: &App) -> AnyElement {
    div()
        .child(skeleton_shelf(true, animate, cx))
        .child(skeleton_shelf(false, animate, cx))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_moves_a_whole_number_of_steps_and_stays_inside_the_row() {
        // 970px window, 176px cards: five fit, so a page is five cards.
        let (step, viewport, max) = (176.0, 970.0, 1_500.0);
        assert_eq!(page_target(0.0, max, viewport, step, 1.0), -880.0);
        assert_eq!(page_target(-880.0, max, viewport, step, 1.0), -1_500.0);
        assert_eq!(page_target(-1_500.0, max, viewport, step, 1.0), -1_500.0);
        assert_eq!(page_target(-1_500.0, max, viewport, step, -1.0), -704.0);
        assert_eq!(page_target(-100.0, max, viewport, step, -1.0), 0.0);
        // A window narrower than one step still moves one.
        assert_eq!(page_target(0.0, max, 100.0, step, 1.0), -176.0);
        // A row that does not scroll does not move.
        assert_eq!(page_target(0.0, 0.0, viewport, step, 1.0), 0.0);
    }

    #[test]
    fn art_is_wanted_for_the_visible_items_and_a_step_either_side() {
        let step = 176.0;
        assert_eq!(visible_range(0.0, 970.0, step, 20), 0..7);
        assert_eq!(visible_range(-880.0, 970.0, step, 20), 4..12);
        assert_eq!(visible_range(-880.0, 970.0, step, 8), 4..8);
        // Unmeasured: assume a window's worth.
        assert_eq!(visible_range(0.0, 0.0, step, 20), 0..7);
        assert_eq!(visible_range(0.0, 970.0, step, 3), 0..3);
    }
}
