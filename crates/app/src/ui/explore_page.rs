//! The Explore page: new releases, the song chart, any other shelves the service sends, and the
//! mood and genre tiles.
//!
//! New releases and the card shelves are rows of the shared cards that scroll sideways, with
//! the chevrons Home's shelves have. The chart is a numbered list of compact song rows (twenty,
//! then "Show all"). The mood and genre tiles are pills in a wrapping grid; one opens a page of
//! playlist cards titled with the mood, and Back (Esc, Alt+Left, the button) returns to the
//! tiles. A playlist opened from there lands on the Library page and Back returns here.
//!
//! Tab order: one stop per row, list and tile group (roving focus, Left and Right inside a row
//! or the tiles, Up and Down inside the list), at the same index Home uses after the nav rail.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use apollo_domain::library::Collection;
use apollo_domain::listing::LoadFailure;
use apollo_domain::model::Track;
use apollo_provider::{ExplorePage, HomeItem, ListedTrack, MoodTile};
use apollo_theme::tokens::{layout, motion, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, point, prelude::*, px, AnyElement, App, Context, ElementId, Entity, FocusHandle,
    IntoElement, KeyBinding, ParentElement, Pixels, Point, Render, ScrollHandle, SharedString,
    Styled, Window,
};

use crate::assets::Icon;
use crate::explore::{ExploreModel, MoodOpen, MoodPhase, Phase};
use crate::library::model::{ArtistTarget, Target};
use crate::queue_source;
use crate::motion::HoverFade;
use crate::player::PlayerModel;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, IconButton};
use crate::ui::card::{self, heading, Card};
use crate::ui::home_page::{
    page_target, skeleton_shelf, visible_range, ScrollButton, Slide, SHELF_CONTEXT, ShelfNext,
    ShelfPrevious, SongTile,
};
use crate::ui::search_page::OpenTarget;
use crate::ui::text_button::TextButton;
use crate::ui::row_actions::more_slot;
use crate::ui::track_menu::{Subject, TrackSubject};
use crate::url_thumbs::{Thumb, ThumbStore};

actions!(explore, [Back, ListNext, ListPrevious]);

/// Key context of the page while a mood page is open: Back works from any control on it.
pub const KEY_CONTEXT: &str = "Explore";
/// Key context of the chart: Up and Down move to the neighbouring row.
pub const LIST_CONTEXT: &str = "ExploreList";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-left", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("down", ListNext, Some(LIST_CONTEXT)),
        KeyBinding::new("up", ListPrevious, Some(LIST_CONTEXT)),
        KeyBinding::new("left", crate::ui::shell::FocusSidebar, Some(LIST_CONTEXT)),
    ]);
}

/// Every stop on the page: after the nav rail, where Home's stops are (the two never show
/// together).
const TAB_INDEX: isize = 6;
/// Rows the chart shows before "Show all".
pub const TOP_SHOWN: usize = 20;
/// The chart's row width and its rank column.
const CHART_WIDTH: f32 = 560.0;
const RANK_WIDTH: f32 = 32.0;
const GAP: f32 = spacing::S4;
const CARD_STEP: f32 = card::WIDTH + GAP;
/// Room inside a scrolling row for the focus ring, which sits outside what it rings.
const RING_ROOM: f32 = layout::FOCUS_RING_OFFSET + layout::FOCUS_RING_WIDTH;
/// A mood page asks for this many covers at most.
const MOOD_ART: usize = 60;

/// One block of the page: a heading and what sits under it.
enum Block {
    Cards(Vec<HomeItem>),
    Chart,
    Moods,
}

struct Section {
    title: SharedString,
    block: Block,
}

/// The sections of a page, in the order they are shown.
fn sections(page: &ExplorePage) -> Vec<Section> {
    let mut out = Vec::new();
    if !page.new_releases.is_empty() {
        out.push(Section {
            title: "New releases".into(),
            block: Block::Cards(
                page.new_releases
                    .iter()
                    .cloned()
                    .map(HomeItem::Collection)
                    .collect(),
            ),
        });
    }
    if !page.top_songs.is_empty() {
        out.push(Section {
            title: "Top songs".into(),
            block: Block::Chart,
        });
    }
    for shelf in &page.shelves {
        out.push(Section {
            title: shelf.title.clone().into(),
            block: Block::Cards(shelf.items.clone()),
        });
    }
    if !page.moods.is_empty() {
        out.push(Section {
            title: "Moods & genres".into(),
            block: Block::Moods,
        });
    }
    out
}

/// The scroll and focus state of one block.
struct Group {
    scroll: ScrollHandle,
    slide: Option<Slide>,
    /// The one item of the group that Tab stops on: the one last focused (roving focus).
    tab: usize,
}

impl Group {
    fn new() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            slide: None,
            tab: 0,
        }
    }
}

pub struct ExploreView {
    player: Entity<PlayerModel>,
    model: Entity<ExploreModel>,
    thumbs: Entity<ThumbStore>,
    on_open: OpenTarget,
    on_sign_in: Rc<dyn Fn(&mut Window, &mut App)>,
    /// The whole page scrolls; its children are the title, then one per section.
    page: ScrollHandle,
    groups: Vec<Group>,
    /// The tab stops of the items, grown to the most items the page ever held.
    stops: Vec<FocusHandle>,
    action_focus: FocusHandle,
    show_all_focus: FocusHandle,
    /// The chart's Skim button.
    skim_focus: FocusHandle,
    back_focus: FocusHandle,
    /// The stop that last took keyboard focus and was scrolled into view.
    scrolled_to: Option<usize>,
    /// The chart is open in place (no page of its own to go to).
    expanded: bool,
    /// Where the page was scrolled when a mood page opened over it.
    saved_offset: Option<Point<Pixels>>,
    /// The first stop of the mood tiles on the page last drawn, to put focus back after Back.
    moods_stop: Option<(usize, usize)>,
}

impl ExploreView {
    pub fn new(
        player: Entity<PlayerModel>,
        model: Entity<ExploreModel>,
        thumbs: Entity<ThumbStore>,
        on_open: impl Fn(Target, &mut Window, &mut App) + 'static,
        on_sign_in: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let stop = |cx: &mut Context<Self>| cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true);
        Self {
            player,
            model,
            thumbs,
            on_open: Rc::new(on_open),
            on_sign_in: Rc::new(on_sign_in),
            page: ScrollHandle::new(),
            groups: Vec::new(),
            stops: Vec::new(),
            action_focus: stop(cx),
            show_all_focus: stop(cx),
            skim_focus: stop(cx),
            back_focus: stop(cx),
            scrolled_to: None,
            expanded: false,
            saved_offset: None,
            moods_stop: None,
        }
    }

    /// Every item stop of the page by index, in reading order (tests).
    pub fn item_stops(&self) -> &[FocusHandle] {
        &self.stops
    }

    /// The Retry / Sign in stop (tests).
    pub fn action_stop(&self) -> &FocusHandle {
        &self.action_focus
    }

    /// The Back button's stop on a mood page (tests).
    pub fn back_stop(&self) -> &FocusHandle {
        &self.back_focus
    }

    /// How far group `group` is scrolled sideways: zero at its start, negative as it scrolls
    /// (tests).
    pub fn row_offset(&self, group: usize) -> f32 {
        self.groups
            .get(group)
            .map_or(0.0, |g| f32::from(g.scroll.offset().x))
    }

    /// How far the page is scrolled; the window's history keeps it. Nothing while a mood page is
    /// open (the tiles' own position is kept for its Back).
    pub fn scroll_offset(&self, cx: &App) -> Option<Point<Pixels>> {
        self.model
            .read(cx)
            .mood()
            .is_none()
            .then(|| self.page.offset())
    }

    pub fn set_scroll_offset(&self, offset: Point<Pixels>) {
        self.page.set_offset(offset);
    }

    /// Back to the top, nothing open (a different account's page).
    pub fn rewind(&mut self, cx: &mut Context<Self>) {
        self.groups.clear();
        self.scrolled_to = None;
        self.expanded = false;
        self.saved_offset = None;
        self.page.set_offset(point(px(0.0), px(0.0)));
        cx.notify();
    }

    /// Opens a mood tile's page of playlists, from the top, with focus on its Back button.
    fn open_mood(&mut self, tile: &MoodTile, window: &mut Window, cx: &mut Context<Self>) {
        self.saved_offset = Some(self.page.offset());
        self.page.set_offset(point(px(0.0), px(0.0)));
        self.scrolled_to = None;
        self.model.update(cx, |m, cx| m.open_mood(tile, cx));
        window.focus(&self.back_focus, cx);
        cx.notify();
    }

    /// Back from a mood page to the tiles, where they were.
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).mood().is_none() {
            return;
        }
        self.model.update(cx, |m, cx| m.close_mood(cx));
        self.scrolled_to = None;
        if let Some(offset) = self.saved_offset.take() {
            self.page.set_offset(offset);
        }
        if let Some((first, tab)) = self.moods_stop {
            if let Some(stop) = self.stops.get(first + tab) {
                window.focus(stop, cx);
            }
        }
        cx.notify();
    }

    /// Where the first arrow press lands: the first item on the page (the Back button on a mood).
    pub fn main_focus(&self, cx: &App) -> Option<FocusHandle> {
        if self.model.read(cx).mood().is_some() {
            return Some(self.back_focus.clone());
        }
        self.stops.first().cloned()
    }

    /// Left / Right / Up / Down: move focus to the next or previous item of the group whose
    /// items are stops `first..first + count`, staying inside it.
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

    /// A chevron was pressed: slide group `group` one page toward `direction`.
    fn slide_row(
        &mut self,
        group: usize,
        direction: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(g) = self.groups.get_mut(group) else {
            return;
        };
        let offset = f32::from(g.scroll.offset().x);
        let target = page_target(
            offset,
            f32::from(g.scroll.max_offset().x),
            f32::from(g.scroll.bounds().size.width),
            CARD_STEP,
            direction,
        );
        if (target - offset).abs() < 0.5 {
            return;
        }
        if cx.reduce_motion() {
            g.slide = None;
            g.scroll.set_offset(point(px(target), g.scroll.offset().y));
        } else {
            g.slide = Some(Slide {
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
        for g in &mut self.groups {
            let Some(slide) = g.slide.as_mut() else {
                continue;
            };
            let here = f32::from(g.scroll.offset().x);
            if (here - slide.written).abs() > 1.0 {
                g.slide = None;
                continue;
            }
            let t = now.saturating_duration_since(slide.started).as_secs_f32()
                / motion::PANEL.as_secs_f32();
            let x = slide.from + (slide.to - slide.from) * motion::ease_out_cubic(t);
            g.scroll.set_offset(point(px(x), g.scroll.offset().y));
            slide.written = x;
            if t >= 1.0 {
                g.slide = None;
            } else {
                window.request_animation_frame();
            }
        }
    }
}

impl Render for ExploreView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let animate = !cx.reduce_motion();
        let (phase, page, mood) = {
            let m = self.model.read(cx);
            (m.phase(), m.page().cloned(), m.mood().cloned())
        };
        let mood = mood.filter(|_| phase == Phase::Ready);
        let in_mood = mood.is_some();

        let root = div()
            .id("explore-page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            // A sideways swipe belongs to the row under the pointer, not to the page.
            .restrict_scroll_to_axis()
            .track_scroll(&self.page)
            .px(px(spacing::S8))
            .pt(px(spacing::S6))
            .when(in_mood, |d| {
                d.key_context(KEY_CONTEXT)
                    .on_action(cx.listener(|this, _: &Back, window, cx| {
                        // While skimming, Esc is the skim's.
                        if this.player.read(cx).skimming() {
                            cx.propagate();
                        } else {
                            this.back(window, cx)
                        }
                    }))
            });

        if let Some(open) = mood {
            return root.child(self.mood_page(&open, accent, animate, window, cx));
        }

        let root = root.child(
            style::text_style(div(), typography::PAGE_TITLE, cx)
                .text_color(color::text_primary())
                .child("Explore"),
        );
        match phase {
            Phase::SignedOut => root.child(self.signed_out(accent)),
            Phase::Loading => root.child(skeleton_page(animate, cx)),
            Phase::Failed(failure) => {
                let model = self.model.clone();
                root.child(self.notice(
                    failure_line(failure),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry(cx)),
                    cx,
                ))
            }
            Phase::Ready => match page.filter(|p| !p.is_empty()) {
                None => {
                    let model = self.model.clone();
                    root.child(self.notice(
                        "Nothing to show".into(),
                        "Retry",
                        accent,
                        move |_, cx| model.update(cx, |m, cx| m.retry(cx)),
                        cx,
                    ))
                }
                Some(page) => {
                    let body = self.sections(&page, accent, window, cx);
                    root.children(body)
                        .child(div().h(px(spacing::S6)).flex_none())
                }
            },
        }
    }
}

impl ExploreView {
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
                    .debug_selector(|| "explore-sign-in".into())
                    .child(
                        TextButton::new("explore-sign-in", &self.action_focus, "Sign in", accent)
                            .primary()
                            .on_click(move |window, cx| on_sign_in(window, cx)),
                    ),
            )
            .into_any_element()
    }

    /// One short line and one action: the error and empty states (the row Home and the Library
    /// use).
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
                div()
                    .debug_selector(|| "explore-notice-action".into())
                    .child(
                        TextButton::new(
                            ElementId::from(action),
                            &self.action_focus,
                            action,
                            accent,
                        )
                        .on_click(on_action),
                    ),
            )
            .into_any_element()
    }

    /// The sections of a loaded page: a heading and a row, list or grid each.
    fn sections(
        &mut self,
        page: &Arc<ExplorePage>,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let plan = sections(page);
        self.groups.resize_with(plan.len(), Group::new);
        let shown = if self.expanded {
            page.top_songs.len()
        } else {
            page.top_songs.len().min(TOP_SHOWN)
        };
        let count_of = |block: &Block| match block {
            Block::Cards(items) => items.len(),
            Block::Chart => shown,
            Block::Moods => page.moods.len(),
        };
        let total: usize = plan.iter().map(|s| count_of(&s.block)).sum();
        while self.stops.len() < total {
            self.stops
                .push(cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true));
        }
        self.tick_slides(window);
        let animate = !cx.reduce_motion();

        // Keyboard focus moves the row and the page to the focused item.
        let mut focused = None;
        let mut stop = 0;
        for (i, section) in plan.iter().enumerate() {
            let count = count_of(&section.block);
            for j in 0..count {
                if self.stops[stop].is_focused(window) {
                    self.groups[i].tab = j;
                }
                if keyboard_focused(&self.stops[stop], window) {
                    focused = Some((stop, i, j, matches!(section.block, Block::Cards(_))));
                }
                stop += 1;
            }
        }
        match focused {
            Some((stop, group, item, is_row)) if self.scrolled_to != Some(stop) => {
                self.scrolled_to = Some(stop);
                if is_row {
                    self.groups[group].scroll.scroll_to_item(item);
                }
                self.page.scroll_to_item(1 + group);
            }
            Some(_) => {}
            None => self.scrolled_to = None,
        }

        // Artwork for what is on screen; the store keeps a bounded number.
        let mut urls: Vec<String> = Vec::new();
        for (i, section) in plan.iter().enumerate() {
            match &section.block {
                Block::Cards(items) => {
                    let g = &self.groups[i];
                    let range = visible_range(
                        f32::from(g.scroll.offset().x),
                        f32::from(g.scroll.bounds().size.width),
                        CARD_STEP,
                        items.len(),
                    );
                    urls.extend(
                        items[range]
                            .iter()
                            .filter_map(|i| i.thumbnail().map(str::to_owned)),
                    );
                }
                Block::Chart => urls.extend(
                    page.top_songs[..shown]
                        .iter()
                        .filter_map(|s| s.thumbnail.clone()),
                ),
                Block::Moods => {}
            }
        }
        self.thumbs.update(cx, |store, cx| {
            for url in &urls {
                store.request(url, cx);
            }
        });

        // ---- build ---------------------------------------------------------------------
        let playing = self.player.read(cx).current_video();
        let view = cx.entity();
        let mut out = Vec::with_capacity(plan.len());
        let mut stop = 0;
        self.moods_stop = None;
        for (i, section) in plan.iter().enumerate() {
            let count = count_of(&section.block);
            let tab = self.groups[i].tab.min(count.saturating_sub(1));
            let handles: Vec<FocusHandle> = self.stops[stop..stop + count]
                .iter()
                .enumerate()
                .map(|(j, h)| h.clone().tab_stop(j == tab))
                .collect();
            let first = stop;
            stop += count;
            let body = match &section.block {
                Block::Cards(items) => {
                    self.card_row(
                        i,
                        first,
                        items,
                        &handles,
                        accent,
                        animate,
                        &view,
                        section.title.clone(),
                        window,
                        cx,
                    )
                }
                Block::Chart => self.chart(
                    page,
                    shown,
                    first,
                    &handles,
                    playing.as_deref(),
                    accent,
                    cx,
                ),
                Block::Moods => {
                    self.moods_stop = Some((first, tab));
                    self.mood_tiles(&page.moods, first, &handles, accent, window, cx)
                }
            };
            out.push(
                div()
                    .child(heading(section.title.clone(), true, cx))
                    .child(body)
                    .into_any_element(),
            );
        }
        out
    }

    /// A row of cards: albums, playlists and artists open their pages, a song plays.
    #[allow(clippy::too_many_arguments)]
    fn card_row(
        &mut self,
        group: usize,
        first: usize,
        items: &[HomeItem],
        handles: &[FocusHandle],
        accent: Accent,
        animate: bool,
        view: &Entity<Self>,
        name: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = items.len();
        let hover = window.use_keyed_state(("explore-row", group), cx, |_, _| HoverFade::default());
        let (level, moving) = hover.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let g = &self.groups[group];
        let offset = f32::from(g.scroll.offset().x);
        let max = f32::from(g.scroll.max_offset().x);
        let (can_back, can_forward) = (offset < -1.0, offset > -max + 1.0);

        let songs: Vec<ListedTrack> = items
            .iter()
            .filter_map(|i| match i {
                HomeItem::Song(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        let mut song_ix = 0usize;
        let cards: Vec<AnyElement> = items
            .iter()
            .zip(handles)
            .enumerate()
            .map(|(j, (item, focus))| {
                let thumb = match item.thumbnail() {
                    Some(url) => self.thumbs.read(cx).get(url),
                    None => Thumb::Missing,
                };
                let id = format!("explore-{group}-{j}");
                match item {
                    HomeItem::Song(song) => {
                        let start = song_ix;
                        song_ix += 1;
                        let (player, queue) = (self.player.clone(), songs.clone());
                        let name = name.clone();
                        let (hint_player, track) = (self.player.clone(), song.track.clone());
                        Card::new(
                            id,
                            song.track.title.clone(),
                            artists_line(&song.track),
                            thumb,
                            focus,
                            accent,
                            move |_, cx| play_from(&player, &queue, start, &name, cx),
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
            .collect();

        let chevron_top = card::INSET + card::ART / 2.0 - layout::MIN_HIT_TARGET / 2.0;
        let button = |direction: f32, icon: Icon, label: &'static str| {
            let view = view.clone();
            let name = if direction < 0.0 {
                "explore-previous"
            } else {
                "explore-next"
            };
            ScrollButton {
                id: (name, group).into(),
                selector: format!("{name}-{group}"),
                icon,
                label,
                on_click: Box::new(move |window, cx| {
                    view.update(cx, |this, cx| this.slide_row(group, direction, window, cx))
                }),
            }
        };
        div()
            .id(("explore-row", group))
            .relative()
            .on_hover(move |hovered, _, cx| {
                hover.update(cx, |fade, cx| {
                    fade.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
            })
            .child(
                div()
                    .id(("explore-shelf", group))
                    .role(gpui::Role::List)
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
                    // Room for the focus ring, taken back with a negative margin so the art still
                    // lines up with the heading.
                    .px(px(RING_ROOM))
                    .py(px(RING_ROOM))
                    .mx(px(-(card::INSET + RING_ROOM)))
                    .my(px(-RING_ROOM))
                    .overflow_x_scroll()
                    .restrict_scroll_to_axis()
                    .track_scroll(&g.scroll)
                    .children(cards),
            )
            .when(level > 0.0 && can_back, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(spacing::S1))
                        .top(px(chevron_top))
                        .opacity(level)
                        .child(button(-1.0, Icon::ChevronLeft, "Previous")),
                )
            })
            .when(level > 0.0 && can_forward, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(spacing::S1))
                        .top(px(chevron_top))
                        .opacity(level)
                        .child(button(1.0, Icon::ChevronRight, "Next")),
                )
            })
            .into_any_element()
    }

    /// The song chart: a rank, then the compact song row, then "Show all" under the list.
    #[allow(clippy::too_many_arguments)]
    fn chart(
        &self,
        page: &Arc<ExplorePage>,
        shown: usize,
        first: usize,
        handles: &[FocusHandle],
        playing: Option<&str>,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let queue: Arc<Vec<ListedTrack>> = Arc::new(page.top_songs.clone());
        let rows: Vec<AnyElement> = page.top_songs[..shown]
            .iter()
            .zip(handles)
            .enumerate()
            .map(|(j, (song, focus))| {
                let thumb = match song.thumbnail.as_deref() {
                    Some(url) => self.thumbs.read(cx).get(url),
                    None => Thumb::Missing,
                };
                let (player, queue) = (self.player.clone(), queue.clone());
                let (hint_player, track) = (self.player.clone(), song.track.clone());
                let rank = style::tabular(style::text_style(div(), typography::BODY, cx))
                    .w(px(RANK_WIDTH))
                    .flex_none()
                    .text_color(color::text_tertiary())
                    .child(format!("{}", j + 1));
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(rank)
                    .child(SongTile {
                        id: format!("explore-chart-{j}").into(),
                        title: song.track.title.clone().into(),
                        artists: artists_line(&song.track).unwrap_or_default(),
                        thumb,
                        playing: playing == Some(song.track.video_id.as_str()),
                        menu: Subject::track(song.track.clone(), song.thumbnail.clone()),
                        focus: focus.clone(),
                        accent,
                        width: CHART_WIDTH,
                        on_play: Rc::new(move |_, cx| play_from(&player, &queue, j, "Top songs", cx)),
                        on_intent: Some(Rc::new(move |cx: &mut App| {
                            hint_player.update(cx, |p, cx| p.hint_play(&track, cx));
                        })),
                    })
                    .into_any_element()
            })
            .collect();

        let count = handles.len();
        let wants_more =
            !self.expanded && (page.top_songs_all.is_some() || page.top_songs.len() > TOP_SHOWN);
        let show_all = wants_more.then(|| {
            let (view, all) = (cx.entity(), page.top_songs_all.clone());
            let on_open = self.on_open.clone();
            div()
                .flex()
                .flex_row()
                .mt(px(spacing::S2))
                .ml(px(RANK_WIDTH))
                .child(
                    div().debug_selector(|| "explore-show-all".into()).child(
                        TextButton::new(
                            "explore-show-all",
                            &self.show_all_focus,
                            "Show all",
                            accent,
                        )
                        .on_click(move |window, cx| match &all {
                            Some(chart) => on_open(Target::Collection(chart.clone()), window, cx),
                            None => view.update(cx, |this, cx| {
                                this.expanded = true;
                                cx.notify();
                            }),
                        }),
                    ),
                )
        });
        // Skim: audition the chart, a few seconds of each song. More (at the right end) holds
        // Add all to Crate.
        let skim = self.player.read(cx).skim_enabled().then(|| {
            let (player, songs) = (self.player.clone(), queue.clone());
            div().debug_selector(|| "explore-skim".into()).child(
                TextButton::new("explore-skim", &self.skim_focus, "Skim", accent)
                    .on_click(move |_, cx| skim_from(&player, &songs, "Top songs", cx)),
            )
        });
        let more = Subject::Loaded(Arc::new(
            page.top_songs
                .iter()
                .map(|song| TrackSubject::of_row(song.track.clone(), song.thumbnail.clone(), None))
                .collect(),
        ));
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .mb(px(spacing::S2))
            .child(div().flex_1().children(skim))
            .child(more_slot("explore-more", true, more));
        div()
            .child(header)
            .child(
                div()
                    .id("explore-chart")
                    .role(gpui::Role::List)
                    .key_context(LIST_CONTEXT)
                    .on_action(cx.listener(move |this, _: &ListNext, window, cx| {
                        this.step_focus(first, count, 1, window, cx)
                    }))
                    .on_action(cx.listener(move |this, _: &ListPrevious, window, cx| {
                        this.step_focus(first, count, -1, window, cx)
                    }))
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .children(show_all)
            .into_any_element()
    }

    /// The mood and genre tiles: pills that wrap.
    fn mood_tiles(
        &mut self,
        tiles: &[MoodTile],
        first: usize,
        handles: &[FocusHandle],
        accent: Accent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let count = tiles.len();
        let pills: Vec<AnyElement> = tiles
            .iter()
            .zip(handles)
            .enumerate()
            .map(|(j, (tile, focus))| {
                let (view, opened) = (cx.entity(), tile.clone());
                MoodChip {
                    id: format!("explore-mood-{j}").into(),
                    label: tile.title.clone().into(),
                    focus: focus.clone(),
                    accent,
                    on_open: Rc::new(move |window, cx| {
                        view.update(cx, |this, cx| this.open_mood(&opened, window, cx))
                    }),
                }
                .into_any_element()
            })
            .collect();
        div()
            .id("explore-moods")
            .role(gpui::Role::List)
            .key_context(SHELF_CONTEXT)
            .on_action(cx.listener(move |this, _: &ShelfNext, window, cx| {
                this.step_focus(first, count, 1, window, cx)
            }))
            .on_action(cx.listener(move |this, _: &ShelfPrevious, window, cx| {
                this.step_focus(first, count, -1, window, cx)
            }))
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(spacing::S3))
            .children(pills)
            .into_any_element()
    }

    /// A mood page: Back and the mood's name, then its playlists as cards.
    fn mood_page(
        &mut self,
        open: &MoodOpen,
        accent: Accent,
        animate: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity();
        let header = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S4))
            .child(
                IconButton::new("explore-back", &self.back_focus, Icon::ArrowLeft, "Back", accent)
                    .on_click(move |window, cx| this.update(cx, |t, cx| t.back(window, cx))),
            )
            .child(
                style::text_style(div(), typography::PAGE_TITLE, cx)
                    .min_w_0()
                    .truncate()
                    .text_color(color::text_primary())
                    .child(SharedString::from(open.title.clone())),
            );
        let body = match &open.phase {
            MoodPhase::Loading => skeleton_shelf(false, animate, cx),
            MoodPhase::Failed(failure) => {
                let model = self.model.clone();
                self.notice(
                    mood_failure_line(*failure),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry_mood(cx)),
                    cx,
                )
            }
            MoodPhase::Ready(found) if found.is_empty() => {
                let this = cx.entity();
                self.notice(
                    "Nothing to show".into(),
                    "Back",
                    accent,
                    move |window, cx| this.update(cx, |t, cx| t.back(window, cx)),
                    cx,
                )
            }
            MoodPhase::Ready(found) => self.mood_cards(found, accent, window, cx),
        };
        div()
            .child(header)
            .child(div().mt(px(spacing::S6)).child(body))
            .child(div().h(px(spacing::S6)).flex_none())
            .into_any_element()
    }

    fn mood_cards(
        &mut self,
        found: &Arc<Vec<Collection>>,
        accent: Accent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        while self.stops.len() < found.len() {
            self.stops
                .push(cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true));
        }
        let urls: Vec<String> = found
            .iter()
            .take(MOOD_ART)
            .filter_map(|c| c.thumbnail.clone())
            .collect();
        self.thumbs.update(cx, |store, cx| {
            for url in &urls {
                store.request(url, cx);
            }
        });
        let cards: Vec<AnyElement> = found
            .iter()
            .zip(&self.stops)
            .enumerate()
            .map(|(j, (c, focus))| {
                let thumb = match c.thumbnail.as_deref() {
                    Some(url) => self.thumbs.read(cx).get(url),
                    None => Thumb::Missing,
                };
                let (target, on_open) = (Target::Collection(c.clone()), self.on_open.clone());
                Card::new(
                    format!("explore-playlist-{j}"),
                    c.title.clone(),
                    c.subtitle.clone().map(Into::into),
                    thumb,
                    focus,
                    accent,
                    move |window, cx| on_open(target.clone(), window, cx),
                )
                .menu(Subject::Collection(c.clone()))
                .into_any_element()
            })
            .collect();
        card::grid().children(cards).into_any_element()
    }
}

/// Plays song `start` of `songs` with all of them as the queue, from the section called `name`.
fn play_from(
    player: &Entity<PlayerModel>,
    songs: &[ListedTrack],
    start: usize,
    name: &str,
    cx: &mut App,
) {
    let tracks: Vec<Track> = songs.iter().map(|s| s.track.clone()).collect();
    player.update(cx, |p, cx| {
        for song in songs {
            p.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
        }
        p.play_tracks_from(tracks, Some(start), None, Some(queue_source::named(name)), cx);
    });
}

/// Skims `songs` from the first, from the section called `name`.
fn skim_from(player: &Entity<PlayerModel>, songs: &[ListedTrack], name: &str, cx: &mut App) {
    let tracks: Vec<Track> = songs.iter().map(|s| s.track.clone()).collect();
    player.update(cx, |p, cx| {
        for song in songs {
            p.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
        }
        p.start_skim(tracks, Some(queue_source::named(name)), cx);
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
        _ => "Couldn't load Explore".into(),
    }
}

fn mood_failure_line(failure: LoadFailure) -> SharedString {
    match failure {
        LoadFailure::Session => "Sign-in expired".into(),
        LoadFailure::Throttled => "Too many requests".into(),
        _ => "Couldn't load playlists".into(),
    }
}

// ---- mood tiles ---------------------------------------------------------------------------

/// A mood or genre pill: bg.raised, the label in text.primary, a hover tint that fades over
/// 120 ms and the keyboard-only accent ring.
#[derive(IntoElement)]
struct MoodChip {
    id: SharedString,
    label: SharedString,
    focus: FocusHandle,
    accent: Accent,
    on_open: Rc<dyn Fn(&mut Window, &mut App) + 'static>,
}

impl RenderOnce for MoodChip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animate = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let ring_color = color::accent(self.accent);
        let (open, open_key) = (self.on_open.clone(), self.on_open.clone());
        div()
            .debug_selector({
                let id = self.id.to_string();
                move || id.clone()
            })
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S4))
            .rounded_full()
            .bg(color::bg_raised())
            .cursor_pointer()
            .track_focus(&self.focus)
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .on_hover(move |hovered, _, cx| {
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
            })
            .key_context(card::KEY_CONTEXT)
            .on_action(move |_: &card::OpenCard, window, cx| open_key(window, cx))
            .on_click(move |event, window, cx| {
                // The keyboard opens through `OpenCard`; Space is the play/pause key.
                if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    open(window, cx);
                }
            })
            .child(
                div()
                    .absolute()
                    .size_full()
                    .rounded_full()
                    .bg(color::state_hover().opacity(level)),
            )
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .whitespace_nowrap()
                    .text_color(color::text_primary())
                    .child(self.label),
            )
            .when(ring, move |d| d.child(focus_ring(ring_color, layout::MIN_HIT_TARGET / 2.0)))
    }
}

// ---- skeleton -----------------------------------------------------------------------------

/// The page while it loads: a row of cards, then a chart.
fn skeleton_page(animate: bool, cx: &App) -> AnyElement {
    div()
        .child(skeleton_shelf(false, animate, cx))
        .child(skeleton_shelf(true, animate, cx))
        .into_any_element()
}
