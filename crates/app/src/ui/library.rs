//! The Library page: Playlists and Albums lists, and a collection's track list.
//!
//! Lists are virtualized (`uniform_list`: only the visible rows exist), with one keyboard
//! selection per list rather than a focus handle per row. Everything visual comes from
//! `crates/theme` through `style`; the patterns follow the nav rail (selected tint, accent
//! bar) and the player bar (error row, no-art tile).

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::{Duration, Instant};

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::LoadFailure;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, uniform_list, Animation, AnimationExt, AnyElement, App, Context,
    ElementId, Entity, FocusHandle, Focusable, IntoElement, KeyBinding, ParentElement, Render,
    SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::assets::Icon;
use crate::library::model::{ArtSize, Detail, LibraryModel, Load, PlayRequest, Tab};
use crate::library::source::TrackRow;
use crate::motion::HoverFade;
use crate::player::{format_time, PlayerModel};
use crate::style::{self, color};
use crate::ui::art_tile::art_tile;
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::text_button::TextButton;
use crate::ui::tooltip::Tooltip;

actions!(
    library,
    [
        ListUp,
        ListDown,
        ListPageUp,
        ListPageDown,
        ListFirst,
        ListLast,
        ListActivate,
        Back
    ]
);

/// Key context of the whole page (Back works from any control on it).
pub const KEY_CONTEXT: &str = "Library";
/// Key context of a list: arrows, paging and Enter apply only while it has focus.
pub const LIST_CONTEXT: &str = "LibraryList";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", ListUp, Some(LIST_CONTEXT)),
        KeyBinding::new("down", ListDown, Some(LIST_CONTEXT)),
        KeyBinding::new("pageup", ListPageUp, Some(LIST_CONTEXT)),
        KeyBinding::new("pagedown", ListPageDown, Some(LIST_CONTEXT)),
        KeyBinding::new("home", ListFirst, Some(LIST_CONTEXT)),
        KeyBinding::new("end", ListLast, Some(LIST_CONTEXT)),
        KeyBinding::new("enter", ListActivate, Some(LIST_CONTEXT)),
        KeyBinding::new("alt-left", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Back, Some(KEY_CONTEXT)),
    ]);
}

/// Playlist and album rows: 48px art plus 8px above and below.
const COLLECTION_ROW: f32 = 64.0;
const COLLECTION_ART: f32 = 48.0;
/// Track rows: 40px art plus 8px above and below.
const TRACK_ROW: f32 = 56.0;
const TRACK_ART: f32 = 40.0;
const INDEX_WIDTH: f32 = spacing::S8;
const DURATION_WIDTH: f32 = spacing::S12;
/// The detail header's cover.
const HEADER_ART: f32 = 96.0;
/// The album column needs room; below this window width it is dropped.
const ALBUM_COLUMN_MIN_WINDOW: f32 = 1000.0;
/// Width of the accent bar on the row that is playing.
const PLAYING_BAR: f32 = 3.0;
/// Skeleton rows drawn while loading: enough to fill a tall window.
const SKELETON_ROWS: usize = 12;
/// One pulse of the skeleton shimmer.
const SHIMMER: Duration = Duration::from_millis(1600);

#[derive(Clone)]
struct Focus {
    back: FocusHandle,
    tab_playlists: FocusHandle,
    tab_albums: FocusHandle,
    play: FocusHandle,
    shuffle: FocusHandle,
    list: FocusHandle,
    retry: FocusHandle,
}

/// 120 ms hover fades for list rows: only rows that are hovered or still fading keep an entry.
#[derive(Default)]
struct RowHover {
    fades: HashMap<usize, HoverFade>,
}

impl RowHover {
    fn set(&mut self, row: usize, hovered: bool, animations: bool) {
        self.fades
            .entry(row)
            .or_default()
            .set(hovered, Instant::now(), animations);
    }

    /// Fade level of a row; `moving` if any fade is still running.
    fn level(&mut self, row: usize, moving: &mut bool) -> f32 {
        let now = Instant::now();
        match self.fades.get(&row) {
            Some(f) => {
                let (level, m) = f.level(now);
                *moving |= m;
                level
            }
            None => 0.0,
        }
    }

    fn prune(&mut self) {
        let now = Instant::now();
        self.fades.retain(|_, f| f.is_hovered() || f.level(now).1);
    }

    fn clear(&mut self) {
        self.fades.clear();
    }
}

pub struct LibraryView {
    model: Entity<LibraryModel>,
    player: Entity<PlayerModel>,
    focus: Focus,
    playlists_scroll: UniformListScrollHandle,
    albums_scroll: UniformListScrollHandle,
    detail_scroll: UniformListScrollHandle,
    /// Keyboard selection in the list on screen.
    selected: usize,
    /// Where the overview's selection was when a collection was opened.
    overview_selected: usize,
    hover: RowHover,
    /// The empty state's action: go find music.
    on_search: Rc<Handler>,
    /// Rows drawn last frame: with virtualization this stays a screenful however long the list.
    rendered: Range<usize>,
}

impl LibraryView {
    pub fn new(
        player: Entity<PlayerModel>,
        on_search: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        // Tab order sits after the nav rail (1-5), in the slots Now Playing uses (they never
        // show together). Overview and detail never show at once, so they share indices.
        let focus = Focus {
            back: stop(cx, 6),
            tab_playlists: stop(cx, 6),
            tab_albums: stop(cx, 7),
            play: stop(cx, 7),
            shuffle: stop(cx, 8),
            list: stop(cx, 9),
            retry: stop(cx, 10),
        };
        let model = cx.new(|cx| LibraryModel::new(player.clone(), cx));
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        // The accent and the playing row follow the player.
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            player,
            focus,
            playlists_scroll: UniformListScrollHandle::new(),
            albums_scroll: UniformListScrollHandle::new(),
            detail_scroll: UniformListScrollHandle::new(),
            selected: 0,
            overview_selected: 0,
            hover: RowHover::default(),
            on_search: Rc::new(Box::new(on_search)),
            rendered: 0..0,
        }
    }

    /// Back to a blank overview (the account changed: nothing of the old one may show).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.reset(cx));
        self.selected = 0;
        self.overview_selected = 0;
        self.hover.clear();
        cx.notify();
    }

    pub fn rendered_rows(&self) -> Range<usize> {
        self.rendered.clone()
    }

    pub fn model(&self) -> &Entity<LibraryModel> {
        &self.model
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Back to the Playlists/Albums lists (also what clicking Library in the nav does).
    pub fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).detail().is_some() {
            self.back(window, cx);
        }
    }

    // ---- navigation ----------------------------------------------------------------------

    pub fn open(&mut self, collection: Collection, window: &mut Window, cx: &mut Context<Self>) {
        self.overview_selected = self.selected;
        self.selected = 0;
        self.detail_scroll = UniformListScrollHandle::new();
        self.hover.clear();
        self.model.update(cx, |m, cx| m.open(collection, cx));
        window.focus(&self.focus.list, cx);
        cx.notify();
    }

    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).detail().is_none() {
            return;
        }
        self.model.update(cx, |m, cx| m.close(cx));
        self.selected = self.overview_selected;
        self.hover.clear();
        window.focus(&self.focus.list, cx);
        cx.notify();
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.model.read(cx).tab() != tab {
            self.model.update(cx, |m, cx| m.set_tab(tab, cx));
            self.selected = 0;
            self.hover.clear();
        }
    }

    fn scroll_handle(&self, cx: &App) -> &UniformListScrollHandle {
        let model = self.model.read(cx);
        if model.detail().is_some() {
            &self.detail_scroll
        } else if model.tab() == Tab::Playlists {
            &self.playlists_scroll
        } else {
            &self.albums_scroll
        }
    }

    /// Rows the keyboard can select in the list on screen.
    fn selectable_rows(&self, cx: &App) -> usize {
        let model = self.model.read(cx);
        match model.detail() {
            Some(d) => d.listing.items().len(),
            None => model.list(model.tab()).len(),
        }
    }

    fn move_selection(&mut self, target: isize, cx: &mut Context<Self>) {
        let rows = self.selectable_rows(cx);
        if rows == 0 {
            return;
        }
        self.selected = target.clamp(0, rows as isize - 1) as usize;
        self.scroll_handle(cx)
            .scroll_to_item(self.selected, gpui::ScrollStrategy::Nearest);
        cx.notify();
    }

    fn page_rows(&self, cx: &App) -> isize {
        let row = if self.model.read(cx).detail().is_some() {
            TRACK_ROW
        } else {
            COLLECTION_ROW
        };
        let height = self
            .scroll_handle(cx)
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height;
        ((f32::from(height) / row) as isize - 1).max(1)
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        match model.detail() {
            Some(d) => {
                if index < d.listing.items().len() {
                    self.model
                        .update(cx, |m, cx| m.request_play(PlayRequest::row(index), cx));
                }
            }
            None => {
                if let Some(c) = model.list(model.tab()).get(index).cloned() {
                    self.open(c, window, cx);
                }
            }
        }
    }

    // ---- small pieces --------------------------------------------------------------------

    fn art_tile(
        &mut self,
        url: Option<&str>,
        size: ArtSize,
        px_size: f32,
        glyph: Icon,
        glyph_color: gpui::Hsla,
        cx: &mut App,
    ) -> AnyElement {
        let image = url.and_then(|url| {
            let model = self.model.read(cx);
            if !model.art_requested(url, size) {
                let url = url.to_owned();
                self.model.update(cx, |m, cx| m.want_art(&url, size, cx));
                return None;
            }
            self.model.read(cx).art(url, size)
        });
        // Everything but Liked Music (which has no cover) is the shell's art tile.
        if !matches!(glyph, Icon::HeartFilled) {
            return art_tile(px_size, image).into_any_element();
        }
        // A decorative glyph: the 3:1 non-text minimum applies, not 4.5:1.
        div()
            .size(px(px_size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .bg(color::bg_overlay())
            .child(
                gpui::svg()
                    .path(glyph.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(glyph_color),
            )
            .into_any_element()
    }

    /// One truncated line; the full text shows in a tooltip.
    fn line(
        id: impl Into<ElementId>,
        text: SharedString,
        style: typography::TextStyle,
        text_color: gpui::Hsla,
        tabular: bool,
        cx: &App,
    ) -> AnyElement {
        let tip = text.clone();
        let el = style::text_style(div(), style, cx)
            .id(id)
            .min_w_0()
            .truncate()
            .text_color(text_color)
            .child(text)
            .tooltip(move |_, cx| Tooltip::view(tip.clone(), cx));
        if tabular {
            style::tabular(el).into_any_element()
        } else {
            el.into_any_element()
        }
    }

    fn skeleton_rows(count: usize, row: f32, art: f32, animations: bool) -> AnyElement {
        let block = |w: Option<f32>, h: f32| {
            let el = div()
                .h(px(h))
                .rounded(px(radius::SMALL))
                .bg(color::skeleton());
            match w {
                Some(w) => el.w(px(w)),
                None => el.w_full(),
            }
        };
        let rows = (0..count).map(|i| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing::S3))
                .h(px(row))
                .child(
                    div()
                        .size(px(art))
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
                        .child(block(Some(220.0 - (i % 3) as f32 * 40.0), 12.0))
                        .child(block(Some(140.0), 10.0)),
                )
        });
        let column = div().flex().flex_col().w_full().children(rows);
        if animations {
            column
                .with_animation(
                    "skeleton-shimmer",
                    Animation::new(SHIMMER).repeat(),
                    |el, t| el.opacity(0.65 + 0.35 * (t * std::f32::consts::TAU).sin().abs()),
                )
                .into_any_element()
        } else {
            column.into_any_element()
        }
    }

    /// One short line and one action: the shape of every empty and error state.
    fn notice(
        &self,
        text: impl Into<SharedString>,
        warning: bool,
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
                    .child(text.into()),
            )
            .child(
                div()
                    .debug_selector(|| "library-notice-action".into())
                    .child(
                        TextButton::new(ElementId::from(action), &self.focus.retry, action, accent)
                            .on_click(on_action),
                    ),
            )
            .into_any_element()
    }

    fn failure_line(failure: LoadFailure, what: &'static str) -> SharedString {
        match failure {
            LoadFailure::Session => "Sign-in expired".into(),
            _ => format!("Couldn't load {what}").into(),
        }
    }

    // ---- overview ------------------------------------------------------------------------

    fn render_overview(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = self.model.read(cx).tab();
        let animations = !cx.reduce_motion();

        let tab_button = |label: &'static str,
                          this: Tab,
                          focus: &FocusHandle,
                          cx: &mut Context<Self>,
                          window: &Window| {
            let active = tab == this;
            let ring = keyboard_focused(focus, window);
            let ring_color = color::accent(accent);
            style::text_style(
                div(),
                if active {
                    typography::BODY_STRONG
                } else {
                    typography::BODY
                },
                cx,
            )
            .id(label)
            .relative()
            .flex()
            .items_center()
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S4))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .track_focus(focus)
            .role(gpui::Role::Tab)
            .aria_label(label)
            .aria_selected(active)
            .text_color(if active {
                color::text_primary()
            } else {
                color::text_secondary()
            })
            .when(active, |d| d.bg(color::state_selected()))
            .hover(|s| s.bg(color::state_hover()))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if let gpui::ClickEvent::Keyboard(k) = event {
                    if k.button == gpui::KeyboardButton::Space {
                        return;
                    }
                }
                this.set_tab(this_tab(label), cx);
            }))
            .child(label)
            .when(ring, move |d| {
                d.child(focus_ring(ring_color, radius::SMALL))
            })
        };
        fn this_tab(label: &str) -> Tab {
            if label == "Albums" {
                Tab::Albums
            } else {
                Tab::Playlists
            }
        }

        let header = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap(px(spacing::S4))
            .child(
                style::text_style(div(), typography::PAGE_TITLE, cx)
                    .text_color(color::text_primary())
                    .child("Library"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(spacing::S2))
                    .child(tab_button(
                        "Playlists",
                        Tab::Playlists,
                        &self.focus.tab_playlists.clone(),
                        cx,
                        window,
                    ))
                    .child(tab_button(
                        "Albums",
                        Tab::Albums,
                        &self.focus.tab_albums.clone(),
                        cx,
                        window,
                    )),
            );

        let (state, count) = {
            let model = self.model.read(cx);
            (model.collections().clone(), model.list(tab).len())
        };
        let body: AnyElement = match state {
            Load::Idle | Load::Loading => {
                Self::skeleton_rows(SKELETON_ROWS, COLLECTION_ROW, COLLECTION_ART, animations)
            }
            Load::Failed(failure) => {
                let model = self.model.clone();
                self.notice(
                    Self::failure_line(failure, "your library"),
                    true,
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.reload_collections(cx)),
                    cx,
                )
            }
            Load::Ready(_) if count == 0 => {
                let text = if tab == Tab::Playlists {
                    "No playlists yet"
                } else {
                    "No saved albums"
                };
                self.search_notice(text, accent, cx)
            }
            Load::Ready(_) => self.render_list(count, ListKind::Collections, window, cx),
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().mt(px(spacing::S3)).child(body))
            .into_any_element()
    }

    fn search_notice(
        &self,
        text: &'static str,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_search = self.on_search.clone();
        self.notice(
            text,
            false,
            "Search",
            accent,
            move |w, cx| on_search(w, cx),
            cx,
        )
    }

    // ---- shared list ---------------------------------------------------------------------

    fn render_list(
        &mut self,
        count: usize,
        kind: ListKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match kind {
            ListKind::Collections => "collections",
            ListKind::Tracks => "tracks",
        };
        let focused = keyboard_focused(&self.focus.list, window);
        let handle = self.scroll_handle(cx).clone();
        // The element the list sits in has to be tall (a bounded flex child), so the list only
        // draws the rows in view.
        div()
            .id(id)
            .key_context(LIST_CONTEXT)
            .track_focus(&self.focus.list)
            .role(gpui::Role::List)
            .size_full()
            .min_h_0()
            .on_action(cx.listener(|this, _: &ListUp, _, cx| {
                this.move_selection(this.selected as isize - 1, cx)
            }))
            .on_action(cx.listener(|this, _: &ListDown, _, cx| {
                this.move_selection(this.selected as isize + 1, cx)
            }))
            .on_action(cx.listener(|this, _: &ListPageUp, _, cx| {
                let step = this.page_rows(cx);
                this.move_selection(this.selected as isize - step, cx)
            }))
            .on_action(cx.listener(|this, _: &ListPageDown, _, cx| {
                let step = this.page_rows(cx);
                this.move_selection(this.selected as isize + step, cx)
            }))
            .on_action(cx.listener(|this, _: &ListFirst, _, cx| this.move_selection(0, cx)))
            .on_action(cx.listener(|this, _: &ListLast, _, cx| this.move_selection(isize::MAX, cx)))
            .on_action(cx.listener(|this, _: &ListActivate, window, cx| {
                let i = this.selected;
                this.activate(i, window, cx)
            }))
            .child(
                uniform_list(
                    id,
                    count,
                    cx.processor(move |this, range: Range<usize>, window, cx| {
                        this.render_rows(kind, range, focused, window, cx)
                    }),
                )
                .track_scroll(&handle)
                .size_full(),
            )
            .into_any_element()
    }

    fn render_rows(
        &mut self,
        kind: ListKind,
        range: Range<usize>,
        list_focused: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let animations = !cx.reduce_motion();
        let (accent, _) = self.player.read(cx).accent_now();
        self.rendered = range.clone();
        self.hover.prune();
        let mut moving = false;
        let rows: Vec<AnyElement> = range
            .map(|ix| {
                let level = self.hover.level(ix, &mut moving);
                let selected = list_focused && ix == self.selected;
                let frame = RowFrame {
                    ix,
                    level,
                    selected,
                    accent,
                    animations,
                };
                match kind {
                    ListKind::Collections => self.collection_row(frame, cx),
                    ListKind::Tracks => self.track_row(frame, window, cx),
                }
            })
            .collect();
        if moving {
            window.request_animation_frame();
        }
        rows
    }

    /// The row's outer box: hover tint (fading), keyboard selection (tint plus inset ring),
    /// click, hover tracking.
    fn row_shell(
        &self,
        frame: &RowFrame,
        height: f32,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (ix, animations) = (frame.ix, frame.animations);
        let tint = if frame.selected {
            color::state_selected()
        } else {
            color::state_hover().opacity(frame.level)
        };
        div()
            .id(("row", ix))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .h(px(height))
            .flex_none()
            .w_full()
            .px(px(spacing::S2))
            .rounded(px(radius::MEDIUM))
            .cursor_pointer()
            .role(gpui::Role::ListItem)
            .bg(tint)
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover.set(ix, *hovered, animations);
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                this.selected = ix;
                on_click(this, window, cx);
            }))
            .when(frame.selected, |d| {
                d.child(
                    div()
                        .absolute()
                        .size_full()
                        .rounded(px(radius::MEDIUM))
                        .border(px(layout::FOCUS_RING_WIDTH))
                        .border_color(color::accent(frame.accent)),
                )
            })
    }

    // ---- collection rows -----------------------------------------------------------------

    fn collection_row(&mut self, frame: RowFrame, cx: &mut Context<Self>) -> AnyElement {
        let tab = self.model.read(cx).tab();
        let Some(c) = self.model.read(cx).list(tab).get(frame.ix).cloned() else {
            return div().into_any_element();
        };
        let (glyph, glyph_color) = if c.is_liked_music() {
            (Icon::HeartFilled, color::text_secondary())
        } else {
            (Icon::MusicNote, color::text_tertiary())
        };
        let art = self.art_tile(
            c.thumbnail.as_deref(),
            ArtSize::List,
            COLLECTION_ART,
            glyph,
            glyph_color,
            cx,
        );
        let ix = frame.ix;
        let open = c.clone();
        self.row_shell(
            &frame,
            COLLECTION_ROW,
            move |this, window, cx| this.open(open.clone(), window, cx),
            cx,
        )
        .aria_label(c.title.clone())
        .child(art)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(Self::line(
                    ("collection-title", ix),
                    c.title.clone().into(),
                    typography::LIST_TITLE,
                    color::text_primary(),
                    false,
                    cx,
                ))
                .children(c.subtitle.clone().map(|s| {
                    Self::line(
                        ("collection-subtitle", ix),
                        s.into(),
                        typography::BODY,
                        color::text_secondary(),
                        false,
                        cx,
                    )
                })),
        )
        .into_any_element()
    }

    // ---- detail --------------------------------------------------------------------------

    fn render_detail(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let animations = !cx.reduce_motion();
        let (collection, loading, failure, pending, items, skipped, confirmed_empty) = {
            let d: &Detail = self.model.read(cx).detail().expect("detail is open");
            (
                d.collection.clone(),
                d.is_loading(),
                d.failure(),
                d.pending,
                d.listing.items().len(),
                d.listing.skipped(),
                d.listing.is_confirmed_empty(),
            )
        };
        let what = if collection.kind == CollectionKind::Album {
            "this album"
        } else {
            "this playlist"
        };

        let (glyph, glyph_color) = if collection.is_liked_music() {
            (Icon::HeartFilled, color::text_secondary())
        } else {
            (Icon::MusicNote, color::text_tertiary())
        };
        let art = self.art_tile(
            collection.thumbnail.as_deref(),
            ArtSize::Header,
            HEADER_ART,
            glyph,
            glyph_color,
            cx,
        );
        let can_play = items > 0;
        let play_busy = pending.is_some_and(|p| p.shuffle != Some(true));
        let shuffle_busy = pending.is_some_and(|p| p.shuffle == Some(true));
        let model = self.model.clone();
        let model2 = self.model.clone();

        let header = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S4))
            .child(
                IconButton::new(
                    "library-back",
                    &self.focus.back,
                    Icon::ArrowLeft,
                    "Back",
                    accent,
                )
                .on_click({
                    let this = cx.entity();
                    move |window, cx| this.update(cx, |t, cx| t.back(window, cx))
                }),
            )
            .child(art)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(px(spacing::S1))
                    .child(Self::line(
                        "detail-title",
                        collection.title.clone().into(),
                        typography::PAGE_TITLE,
                        color::text_primary(),
                        false,
                        cx,
                    ))
                    .children(collection.subtitle.clone().map(|s| {
                        Self::line(
                            "detail-subtitle",
                            s.into(),
                            typography::BODY,
                            color::text_secondary(),
                            false,
                            cx,
                        )
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(spacing::S2))
                            .mt(px(spacing::S2))
                            .child(
                                TextButton::new("library-play", &self.focus.play, "Play", accent)
                                    .primary()
                                    .enabled(can_play || loading)
                                    .busy(play_busy)
                                    .on_click(move |_, cx| {
                                        model.update(cx, |m, cx| {
                                            m.request_play(PlayRequest::PLAY_ALL, cx)
                                        })
                                    }),
                            )
                            .child(
                                TextButton::new(
                                    "library-shuffle",
                                    &self.focus.shuffle,
                                    "Shuffle",
                                    accent,
                                )
                                .enabled(can_play || loading)
                                .busy(shuffle_busy)
                                .on_click(move |_, cx| {
                                    model2.update(cx, |m, cx| {
                                        m.request_play(PlayRequest::SHUFFLE, cx)
                                    })
                                }),
                            ),
                    ),
            );

        let body: AnyElement = if items == 0 && loading {
            Self::skeleton_rows(SKELETON_ROWS, TRACK_ROW, TRACK_ART, animations)
        } else if let (0, Some(failure)) = (items, failure) {
            let model = self.model.clone();
            self.notice(
                Self::failure_line(failure, what),
                true,
                "Retry",
                accent,
                move |_, cx| model.update(cx, |m, cx| m.reload_detail(cx)),
                cx,
            )
        } else if confirmed_empty || items == 0 {
            let text = if collection.is_liked_music() {
                "No liked songs yet"
            } else if collection.kind == CollectionKind::Album {
                "No tracks"
            } else {
                "This playlist is empty"
            };
            self.search_notice(text, accent, cx)
        } else {
            // One extra row at the end carries the loading / partial-failure / skipped note.
            let footer = loading || failure.is_some() || skipped > 0;
            self.render_list(items + usize::from(footer), ListKind::Tracks, window, cx)
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .child(header)
            .child(div().flex_1().min_h_0().mt(px(spacing::S4)).child(body))
            .into_any_element()
    }

    fn track_row(
        &mut self,
        frame: RowFrame,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ix = frame.ix;
        let (row, album_collection, loading, failure, skipped): (
            Option<TrackRow>,
            bool,
            bool,
            Option<LoadFailure>,
            u32,
        ) = {
            let d = self.model.read(cx).detail().expect("detail is open");
            (
                d.listing.items().get(ix).cloned(),
                d.collection.kind == CollectionKind::Album,
                d.is_loading(),
                d.failure(),
                d.listing.skipped(),
            )
        };
        let Some(row) = row else {
            return self.track_footer(ix, loading, failure, skipped, frame.accent, cx);
        };

        let playing =
            self.player.read(cx).current_video().as_deref() == Some(row.track.video_id.as_str());
        let art = self.art_tile(
            row.thumbnail.as_deref(),
            ArtSize::List,
            TRACK_ART,
            Icon::MusicNote,
            color::text_tertiary(),
            cx,
        );
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
        let show_album =
            !album_collection && f32::from(window.viewport_size().width) >= ALBUM_COLUMN_MIN_WINDOW;
        let album: Option<SharedString> = row.track.album.as_ref().map(|a| a.title.clone().into());
        let hovered = frame.level > 0.0;
        let accent = frame.accent;
        let index_cell = div()
            .w(px(INDEX_WIDTH))
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .child(if hovered && !playing {
                gpui::svg()
                    .path(Icon::Play.path())
                    .size(px(layout::ICON_SIZE))
                    .text_color(color::text_primary())
                    .into_any_element()
            } else {
                style::tabular(style::text_style(div(), typography::BODY, cx))
                    .text_color(color::text_tertiary())
                    .child(format!("{}", ix + 1))
                    .into_any_element()
            });

        self.row_shell(
            &frame,
            TRACK_ROW,
            move |this, _, cx| {
                this.model
                    .update(cx, |m, cx| m.request_play(PlayRequest::row(ix), cx))
            },
            cx,
        )
        .aria_label(row.track.title.clone())
        .when(playing, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(spacing::S2))
                    .bottom(px(spacing::S2))
                    .w(px(PLAYING_BAR))
                    .rounded_full()
                    .bg(color::accent(accent)),
            )
        })
        .child(index_cell)
        .child(art)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(Self::line(
                    ("track-title", ix),
                    row.track.title.clone().into(),
                    typography::LIST_TITLE,
                    color::text_primary(),
                    false,
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
                                    .id(("explicit", ix))
                                    .flex_none()
                                    .text_color(color::text_tertiary())
                                    .aria_label("Explicit")
                                    .child("E"),
                            )
                        })
                        .child(Self::line(
                            ("track-artist", ix),
                            artists,
                            typography::BODY,
                            color::text_secondary(),
                            false,
                            cx,
                        )),
                ),
        )
        .when(show_album, |d| {
            d.child(div().flex_1().min_w_0().children(album.map(|a| {
                Self::line(
                    ("track-album", ix),
                    a,
                    typography::BODY,
                    color::text_secondary(),
                    false,
                    cx,
                )
            })))
        })
        .child(
            div()
                .w(px(DURATION_WIDTH))
                .flex_none()
                .flex()
                .justify_end()
                .child(Self::line(
                    ("track-duration", ix),
                    duration,
                    typography::CAPTION,
                    color::text_tertiary(),
                    true,
                    cx,
                )),
        )
        .into_any_element()
    }

    /// The row after the last track: still loading, a failure part-way, or unreadable rows.
    fn track_footer(
        &mut self,
        ix: usize,
        loading: bool,
        failure: Option<LoadFailure>,
        skipped: u32,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let body: AnyElement = if let Some(failure) = failure {
            let model = self.model.clone();
            self.notice(
                match failure {
                    LoadFailure::Session => "Sign-in expired".into(),
                    _ => SharedString::from("Couldn't load all tracks"),
                },
                true,
                "Retry",
                accent,
                move |_, cx| model.update(cx, |m, cx| m.reload_detail(cx)),
                cx,
            )
        } else if loading {
            Self::skeleton_rows(1, TRACK_ROW, TRACK_ART, !cx.reduce_motion())
        } else if skipped > 0 {
            style::text_style(div(), typography::BODY, cx)
                .text_color(color::text_secondary())
                .child(if skipped == 1 {
                    SharedString::from("1 track can't be shown")
                } else {
                    format!("{skipped} tracks can't be shown").into()
                })
                .into_any_element()
        } else {
            div().into_any_element()
        };
        div()
            .id(("track-footer", ix))
            .flex()
            .items_center()
            .h(px(TRACK_ROW))
            .px(px(spacing::S2))
            .child(body)
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
enum ListKind {
    Collections,
    Tracks,
}

struct RowFrame {
    ix: usize,
    /// Hover fade level, 0-1.
    level: f32,
    selected: bool,
    accent: Accent,
    animations: bool,
}

impl Focusable for LibraryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.list.clone()
    }
}

impl Render for LibraryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (accent, moving) = self.player.read(cx).accent_now();
        if moving {
            window.request_animation_frame();
        }
        let detail_open = self.model.read(cx).detail().is_some();
        let page = if detail_open {
            self.render_detail(accent, window, cx)
        } else {
            self.render_overview(accent, window, cx)
        };
        div()
            .id("library")
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &Back, window, cx| this.back(window, cx)))
            .size_full()
            .min_h_0()
            .child(page)
    }
}
