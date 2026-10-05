//! The Library page: Playlists and Albums lists, and the pages opened from them and from
//! Search: an album or playlist's track list and an artist's top songs and album rows. Pages
//! stack, and Back unwinds one at a time.
//!
//! Lists are virtualized (`uniform_list`: only the visible rows exist), with one keyboard
//! selection per list rather than a focus handle per row. Everything visual comes from
//! `crates/theme` through `style`; the patterns follow the nav rail (selected tint, accent
//! bar) and the player bar (error row, no-art tile).

mod stats;
mod tabs;

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use apollo_domain::library::{Collection, CollectionKind};
use apollo_domain::listing::LoadFailure;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, uniform_list, AnyElement, App, Context, ElementId, Entity,
    FocusHandle, Focusable, IntoElement, KeyBinding, ParentElement, Render, ScrollHandle,
    SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::assets::Icon;
use crate::library::model::{
    ArtSize, ArtistDetail, ArtistTarget, Detail, LibraryModel, Load, PlayRequest, Tab, Target,
};
use apollo_domain::model::Track;
use crate::library::edits::{EditEvent, LibraryEdits};
use crate::library::source::{ArtistContent, TrackRow};
use crate::library::writes::LibraryWrites;
use crate::more_like::MoreLikeModel;
use crate::motion::HoverFade;
use crate::player::{format_time, PlayerModel};
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::art_tile::art_tile_shaped;
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::card::{heading, Card};
use crate::ui::playing_row;
use crate::ui::playlist_dialog::{self, Mode};
use crate::ui::playlist_picker::{PickerSong, PlaylistPicker};
use crate::ui::row_actions::{heart_action, more_slot, playlist_action, ACTION_SIZE};
use crate::ui::selection::{self, Click, Selection};
use crate::ui::track_menu::{self, Command, Subject, TrackSubject};
use crate::ui::skeleton;
use crate::ui::text_button::TextButton;
use crate::ui::text_field::{TextField, TextFieldEvent};
use crate::ui::tooltip::Tooltip;
use crate::library::filter::Visible;

mod artist;
mod more_like;

/// Where the add-to-playlist popover of track row `ix` is anchored.
fn playlist_anchor(ix: usize) -> String {
    format!("track-playlist-{ix}")
}

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
        LikeRow,
        CrateRow,
        AddRowToPlaylist,
        AddRowToTarget,
        ExtendUp,
        ExtendDown,
        SelectAllRows,
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
        KeyBinding::new("l", LikeRow, Some(LIST_CONTEXT)),
        KeyBinding::new("c", CrateRow, Some(LIST_CONTEXT)),
        KeyBinding::new("p", AddRowToPlaylist, Some(LIST_CONTEXT)),
        KeyBinding::new("a", AddRowToTarget, Some(LIST_CONTEXT)),
        KeyBinding::new("shift-up", ExtendUp, Some(LIST_CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(LIST_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAllRows, Some(LIST_CONTEXT)),
        KeyBinding::new("alt-left", Back, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Back, Some(KEY_CONTEXT)),
    ]);
    tabs::bind_keys(cx);
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
/// Lines of a playlist's description the page header shows.
const DESCRIPTION_LINES: usize = 2;
/// The album column needs room; below this window width it is dropped.
const ALBUM_COLUMN_MIN_WINDOW: f32 = 1000.0;
/// Skeleton rows drawn while loading: enough to fill a tall window.
const SKELETON_ROWS: usize = 12;
/// An artist page opens on its top songs, so its skeleton is about that many rows.
const ARTIST_SKELETON_ROWS: usize = 5;
/// The ghost that follows the pointer while a row is dragged.
const DRAG_GHOST_WIDTH: f32 = 360.0;
/// The insertion line a dragged row shows where it would land.
const DROP_LINE: f32 = 2.0;

/// Width of the filter field in a playlist or album page's header.
const FILTER_WIDTH: f32 = 240.0;

#[derive(Clone)]
struct Focus {
    back: FocusHandle,
    tab_playlists: FocusHandle,
    tab_albums: FocusHandle,
    tab_artists: FocusHandle,
    tab_history: FocusHandle,
    tab_songs: FocusHandle,
    tab_stats: FocusHandle,
    /// The sort button, and the popover it opens (not a tab stop).
    sort: FocusHandle,
    sort_menu: FocusHandle,
    play: FocusHandle,
    shuffle: FocusHandle,
    /// Skim, after Shuffle on a playlist, album or Songs header.
    skim: FocusHandle,
    /// A playlist page's description, when it opens the Edit playlist dialog.
    description: FocusHandle,
    list: FocusHandle,
    retry: FocusHandle,
    /// The Playlists tab's New playlist button.
    new_playlist: FocusHandle,
    /// "More like this" on a playlist page: its Refresh button.
    more_refresh: FocusHandle,
    /// The artist page's Radio and Subscribe buttons, "Show all" and the bio.
    radio: FocusHandle,
    subscribe: FocusHandle,
    show_all: FocusHandle,
    bio: FocusHandle,
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

/// Scroll positions of one open page (kept while another page is open over it).
struct PageScroll {
    /// A collection's virtualized track list.
    list: UniformListScrollHandle,
    /// An artist page's scrolling column.
    column: ScrollHandle,
    /// One per card shelf of an artist page, scrolling sideways.
    shelves: Vec<ScrollHandle>,
    /// The card that last took keyboard focus and was scrolled into view.
    scrolled_to: Option<usize>,
    /// A playlist or album's filter field, made when the page first has rows to filter.
    filter: Option<Entity<TextField>>,
}

impl PageScroll {
    fn new() -> Self {
        Self {
            list: UniformListScrollHandle::new(),
            column: ScrollHandle::new(),
            shelves: Vec::new(),
            scrolled_to: None,
            filter: None,
        }
    }
}

pub struct LibraryView {
    model: Entity<LibraryModel>,
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    picker: Entity<PlaylistPicker>,
    /// The suggestions under a playlist of your own.
    more: Entity<MoreLikeModel>,
    focus: Focus,
    playlists_scroll: UniformListScrollHandle,
    albums_scroll: UniformListScrollHandle,
    /// The Songs tab's virtualized list and its filter field (made when the tab first shows).
    songs_scroll: UniformListScrollHandle,
    songs_filter: Option<Entity<TextField>>,
    /// The Artists tab's and the History tab's scrolling columns.
    artists_scroll: ScrollHandle,
    history_scroll: ScrollHandle,
    /// The Stats tab's scrolling column.
    stats_scroll: ScrollHandle,
    /// The sort popover is open, and the option the keyboard is on.
    sort_open: bool,
    sort_cursor: usize,
    /// One per open page, in the model's stack order.
    pages: Vec<PageScroll>,
    /// Keyboard selection in the list on screen.
    selected: usize,
    /// The selection in each list beneath the page on screen (the overview's first).
    selected_below: Vec<usize>,
    /// Songs picked on a playlist page for bulk verbs (Shift/Ctrl+click, Shift+Arrows, Ctrl+A),
    /// named by the row's own token. Empty everywhere else.
    selection: Selection<String>,
    /// The playlist the picks belong to: another page starts with none.
    selection_page: Option<String>,
    /// What a picked row's More button and right-click open: the menu of the whole selection.
    bulk_menu: Option<Subject>,
    /// Tab stops of an artist page's cards, grown to the most cards a page had.
    card_focus: Vec<FocusHandle>,
    hover: RowHover,
    /// The empty state's action: go find music.
    on_search: Rc<Handler>,
    /// Back closed the last open page: a page opened from elsewhere (Search) returns there.
    on_closed: Rc<Handler>,
    /// Nobody is signed in: the page says so instead of listing (or failing to).
    signed_out: bool,
    /// The signed-out state's action: Settings > Account.
    on_sign_in: Rc<Handler>,
    /// Rows drawn last frame: with virtualization this stays a screenful however long the list.
    rendered: Range<usize>,
    /// A playlist was deleted: if its page is open it closes on the next frame (events carry no
    /// window).
    deleted: Option<String>,
}

/// The song row being dragged to a new place in an editable playlist.
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
            .h(px(TRACK_ROW - spacing::S2))
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

impl LibraryView {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        edits: Entity<LibraryEdits>,
        on_search: impl Fn(&mut Window, &mut App) + 'static,
        on_closed: impl Fn(&mut Window, &mut App) + 'static,
        on_sign_in: impl Fn(&mut Window, &mut App) + 'static,
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
            tab_albums: stop(cx, 6),
            tab_artists: stop(cx, 6),
            tab_history: stop(cx, 6),
            tab_songs: stop(cx, 6),
            tab_stats: stop(cx, 6),
            sort: stop(cx, 7),
            sort_menu: cx.focus_handle(),
            play: stop(cx, 7),
            shuffle: stop(cx, 8),
            skim: stop(cx, 8),
            description: stop(cx, 8),
            list: stop(cx, 9),
            retry: stop(cx, 10),
            more_refresh: stop(cx, 11),
            new_playlist: stop(cx, 8),
            // Same slot as Shuffle (they follow it in the tree), then the list's, then the cards'.
            radio: stop(cx, 8),
            subscribe: stop(cx, 8),
            show_all: stop(cx, 9),
            bio: stop(cx, 10),
        };
        let model = cx.new(|cx| LibraryModel::new(player.clone(), writes.clone(), edits.clone(), cx));
        cx.observe(&model, |_, _, cx| cx.notify()).detach();
        let more = cx.new(|cx| MoreLikeModel::new(player.clone(), model.clone(), writes.clone(), cx));
        cx.observe(&more, |_, _, cx| cx.notify()).detach();
        // Hearts follow the likes made anywhere.
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        // The lists follow what is made, renamed, deleted and saved; hearts of the menu too.
        cx.observe(&edits, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&edits, |this, _, event, cx| {
            if let EditEvent::Deleted(collection) = event {
                this.deleted = Some(collection.library_key().to_owned());
                cx.notify();
            }
        })
        .detach();
        // The accent and the playing row follow the player.
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        Self {
            model,
            player,
            writes,
            picker,
            more,
            focus,
            playlists_scroll: UniformListScrollHandle::new(),
            albums_scroll: UniformListScrollHandle::new(),
            songs_scroll: UniformListScrollHandle::new(),
            songs_filter: None,
            artists_scroll: ScrollHandle::new(),
            history_scroll: ScrollHandle::new(),
            stats_scroll: ScrollHandle::new(),
            sort_open: false,
            sort_cursor: 0,
            pages: Vec::new(),
            selected: 0,
            selected_below: Vec::new(),
            selection: Selection::new(),
            selection_page: None,
            bulk_menu: None,
            card_focus: Vec::new(),
            hover: RowHover::default(),
            on_search: Rc::new(Box::new(on_search)),
            on_closed: Rc::new(Box::new(on_closed)),
            signed_out: false,
            on_sign_in: Rc::new(Box::new(on_sign_in)),
            rendered: 0..0,
            deleted: None,
        }
    }

    /// Back to a blank overview (the account changed: nothing of the old one may show).
    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.model.update(cx, |m, cx| m.reset(cx));
        self.selected = 0;
        self.selected_below.clear();
        self.pages.clear();
        self.hover.clear();
        cx.notify();
    }

    /// Whether nobody is signed in. Unlike an expired session, a deliberate sign-out is not an
    /// error: the page shows one line and the way back in.
    pub fn set_signed_out(&mut self, signed_out: bool, cx: &mut Context<Self>) {
        if self.signed_out != signed_out {
            self.signed_out = signed_out;
            cx.notify();
        }
    }

    fn render_signed_out(&mut self, accent: Accent, cx: &mut Context<Self>) -> AnyElement {
        let on_sign_in = self.on_sign_in.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .child(
                style::text_style(div(), typography::PAGE_TITLE, cx)
                    .text_color(color::text_primary())
                    .child("Library"),
            )
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .text_color(color::text_secondary())
                    .child("Signed out"),
            )
            .child(
                div()
                    .debug_selector(|| "library-sign-in".into())
                    .flex()
                    .flex_row()
                    .child(
                        TextButton::new("library-sign-in", &self.focus.retry, "Sign in", accent)
                            .primary()
                            .on_click(move |window, cx| on_sign_in(window, cx)),
                    ),
            )
            .into_any_element()
    }

    pub fn rendered_rows(&self) -> Range<usize> {
        self.rendered.clone()
    }

    pub fn model(&self) -> &Entity<LibraryModel> {
        &self.model
    }

    /// How many songs are picked for bulk verbs (tests and diagnostics).
    pub fn picked(&self) -> usize {
        self.selection.len()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    /// Closes every open page without moving focus (the host is leaving the page).
    pub fn close_all_pages(&mut self, cx: &mut Context<Self>) {
        if self.model.read(cx).depth() == 0 {
            return;
        }
        self.model.update(cx, |m, cx| m.close_all(cx));
        self.selected = self.selected_below.first().copied().unwrap_or(0);
        self.selected_below.clear();
        self.pages.clear();
        self.hover.clear();
        cx.notify();
    }

    /// Back to the Playlists/Albums lists, closing every open page (also what clicking Library
    /// in the nav does). Does not tell the host: it is the host asking.
    pub fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).depth() == 0 {
            return;
        }
        self.model.update(cx, |m, cx| m.close_all(cx));
        self.selected = self.selected_below.first().copied().unwrap_or(0);
        self.selected_below.clear();
        self.pages.clear();
        self.hover.clear();
        window.focus(&self.focus.list, cx);
        cx.notify();
    }

    // ---- navigation ----------------------------------------------------------------------

    pub fn open(&mut self, collection: Collection, window: &mut Window, cx: &mut Context<Self>) {
        self.open_target(Target::Collection(collection), window, cx);
    }

    pub fn open_artist(
        &mut self,
        artist: ArtistTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_target(Target::Artist(artist), window, cx);
    }

    /// Opens an album, playlist or artist over whatever is showing.
    pub fn open_target(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_below.push(self.selected);
        self.selected = 0;
        self.pages.push(PageScroll::new());
        self.hover.clear();
        self.model.update(cx, |m, cx| m.open_target(target, cx));
        window.focus(&self.focus.list, cx);
        cx.notify();
    }

    /// Closes the page on screen, back to the one beneath it or the lists. Closing the last
    /// one tells the host.
    pub fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).depth() == 0 {
            return;
        }
        let left = self.model.update(cx, |m, cx| m.close(cx));
        self.pages.pop();
        self.selected = self.selected_below.pop().unwrap_or(0);
        self.hover.clear();
        window.focus(&self.focus.list, cx);
        cx.notify();
        if left == 0 {
            let on_closed = self.on_closed.clone();
            on_closed(window, cx);
        }
    }

    fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.model.read(cx).tab() != tab {
            self.model.update(cx, |m, cx| m.set_tab(tab, cx));
            self.selected = 0;
            self.sort_open = false;
            self.hover.clear();
        }
    }

    /// Whether the overview shows the history (its rows are track rows, not collections).
    fn history_open(&self, cx: &App) -> bool {
        let model = self.model.read(cx);
        model.depth() == 0 && model.tab() == Tab::History
    }

    /// Whether the overview shows the Stats tab.
    fn stats_open(&self, cx: &App) -> bool {
        let model = self.model.read(cx);
        model.depth() == 0 && model.tab() == Tab::Stats
    }

    /// The track list on screen's filter field (a playlist or album page's, or the Songs tab's),
    /// once it has one.
    pub fn filter_field(&self, cx: &App) -> Option<Entity<TextField>> {
        let model = self.model.read(cx);
        if model.songs_open() {
            return self.songs_filter.clone();
        }
        model.detail()?;
        self.pages.last()?.filter.clone()
    }

    /// How many rows of the playlist or album on screen the filter keeps.
    pub fn visible_count(&self, cx: &App) -> usize {
        self.visible(cx).len()
    }

    /// Whether the sort popover is open.
    pub fn sort_open(&self) -> bool {
        self.sort_open
    }

    /// What the page's filter says to keep, lowercased and trimmed; empty when there is none.
    fn filter_text(&self, cx: &App) -> String {
        self.filter_field(cx)
            .map(|f| f.read(cx).text().trim().to_lowercase())
            .unwrap_or_default()
    }

    /// The rows of the playlist or album on screen that the filter keeps.
    fn visible(&self, cx: &App) -> Visible {
        Visible::of(self.model.read(cx).list_rows(), &self.filter_text(cx))
    }

    /// Esc with a filter in use clears it before it closes the page. Whether it did.
    fn clear_filter(&mut self, cx: &mut Context<Self>) -> bool {
        match self.filter_field(cx) {
            Some(field) if !field.read(cx).text().is_empty() => {
                field.update(cx, |f, cx| f.set_text("", cx));
                true
            }
            _ => false,
        }
    }

    /// The page's filter field, made on first use. Typing starts the list over from its top;
    /// Enter hands the keyboard to the list.
    fn ensure_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Entity<TextField>> {
        if let Some(field) = self.filter_field(cx) {
            return Some(field);
        }
        self.model.read(cx).detail()?;
        let field = self.new_filter(window, cx);
        self.pages.last_mut()?.filter = Some(field.clone());
        Some(field)
    }

    /// The Songs tab's filter field, made on first use.
    pub(super) fn ensure_songs_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        if let Some(field) = self.songs_filter.clone() {
            return field;
        }
        let field = self.new_filter(window, cx);
        self.songs_filter = Some(field.clone());
        field
    }

    fn new_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextField> {
        let focus = cx.focus_handle().tab_index(9).tab_stop(true);
        let clear = cx.focus_handle().tab_index(9).tab_stop(true);
        let field = cx.new(|_| TextField::new("Filter", focus, clear));
        cx.subscribe_in(&field, window, |this, _, event, window, cx| match event {
            TextFieldEvent::Changed => {
                // The Songs tab's Play and Shuffle queue what the filter keeps.
                if this.model.read(cx).songs_open() {
                    let text = this.filter_text(cx);
                    this.model.update(cx, |m, _| m.set_songs_query(&text));
                }
                this.selected = 0;
                this.hover.clear();
                this.scroll_handle(cx)
                    .scroll_to_item(0, gpui::ScrollStrategy::Top);
                cx.notify();
            }
            TextFieldEvent::Submit => window.focus(&this.focus.list, cx),
        })
        .detach();
        field
    }

    fn scroll_handle(&self, cx: &App) -> &UniformListScrollHandle {
        let model = self.model.read(cx);
        if model.detail().is_some() {
            self.pages
                .last()
                .map_or(&self.playlists_scroll, |page| &page.list)
        } else if model.tab() == Tab::Playlists {
            &self.playlists_scroll
        } else if model.tab() == Tab::Songs {
            &self.songs_scroll
        } else {
            &self.albums_scroll
        }
    }

    /// Rows the keyboard can select in the list on screen.
    fn selectable_rows(&self, cx: &App) -> usize {
        let model = self.model.read(cx);
        if let Some(artist) = model.artist() {
            return artist.content().map_or(0, |c| c.top_songs.len());
        }
        match model.detail() {
            Some(_) => self.visible(cx).len(),
            None if model.tab() == Tab::History => model.history_count(),
            None if model.tab() == Tab::Songs => self.visible(cx).len(),
            None if model.tab() == Tab::Stats => model.stats_row_count(),
            None if model.tab() == Tab::Artists => 0,
            None => model.list(model.tab()).len(),
        }
    }

    fn move_selection(&mut self, target: isize, cx: &mut Context<Self>) {
        let rows = self.selectable_rows(cx);
        if rows == 0 {
            return;
        }
        self.selected = target.clamp(0, rows as isize - 1) as usize;
        if self.model.read(cx).artist().is_some() {
            // The top songs follow the banner, the buttons and the "Top songs" heading.
            if let Some(page) = self.pages.last() {
                page.column.scroll_to_item(self.selected + artist::SONGS_OFFSET);
            }
        } else if self.stats_open(cx) {
            let lens = self.model.read(cx).stats_lens();
            self.stats_scroll
                .scroll_to_item(stats::stats_child(lens, self.selected));
        } else if self.history_open(cx) {
            let child = match self.model.read(cx).history() {
                Load::Ready(groups) => crate::library::browse::history_child(groups, self.selected),
                _ => self.selected,
            };
            self.history_scroll.scroll_to_item(child);
        } else {
            self.scroll_handle(cx)
                .scroll_to_item(self.selected, gpui::ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn page_rows(&self, cx: &App) -> isize {
        let model = self.model.read(cx);
        let row = if model.depth() > 0 || matches!(model.tab(), Tab::History | Tab::Songs | Tab::Stats) {
            TRACK_ROW
        } else {
            COLLECTION_ROW
        };
        let height = if model.artist().is_some() {
            self.pages
                .last()
                .map_or(px(0.0), |p| p.column.bounds().size.height)
        } else if self.history_open(cx) {
            self.history_scroll.bounds().size.height
        } else if self.stats_open(cx) {
            self.stats_scroll.bounds().size.height
        } else {
            self.scroll_handle(cx)
                .0
                .borrow()
                .base_handle
                .bounds()
                .size
                .height
        };
        ((f32::from(height) / row) as isize - 1).max(1)
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        if model.artist().is_some() {
            if index < self.selectable_rows(cx) {
                self.model
                    .update(cx, |m, cx| m.request_play(PlayRequest::row(index), cx));
            }
            return;
        }
        match model.detail() {
            Some(_) => {
                if let Some(real) = self.visible(cx).get(index) {
                    self.model
                        .update(cx, |m, cx| m.request_play(PlayRequest::row(real), cx));
                }
            }
            None if model.tab() == Tab::History => {
                self.model.update(cx, |m, cx| m.play_history(index, cx));
            }
            None if model.tab() == Tab::Songs => {
                if let Some(real) = self.visible(cx).get(index) {
                    self.model
                        .update(cx, |m, cx| m.request_play(PlayRequest::row(real), cx));
                }
            }
            None if model.tab() == Tab::Stats => self.activate_stat(index, window, cx),
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
        self.art_tile_shaped(url, size, px_size, glyph, glyph_color, false, cx)
    }

    /// `circle` is the artist's shape: the one cue that tells an artist from an album.
    #[allow(clippy::too_many_arguments)]
    fn art_tile_shaped(
        &mut self,
        url: Option<&str>,
        size: ArtSize,
        px_size: f32,
        glyph: Icon,
        glyph_color: gpui::Hsla,
        circle: bool,
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
            return art_tile_shaped(px_size, image, circle).into_any_element();
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
        skeleton::pulse(column, "library-skeleton", animations)
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
            .child(self.tab_row(accent, window, cx));

        let body: AnyElement = match tab {
            Tab::Artists => self.render_artists_tab(accent, window, cx),
            Tab::History => self.render_history_tab(accent, window, cx),
            Tab::Songs => self.render_songs_tab(accent, window, cx),
            Tab::Stats => self.render_stats_tab(accent, window, cx),
            Tab::Playlists | Tab::Albums => {
                let (state, count) = {
                    let model = self.model.read(cx);
                    (model.collections().clone(), model.list(tab).len())
                };
                match state {
                    Load::Idle | Load::Loading => Self::skeleton_rows(
                        SKELETON_ROWS,
                        COLLECTION_ROW,
                        COLLECTION_ART,
                        animations,
                    ),
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
                }
            }
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
        let name = match kind {
            ListKind::Collections => match self.model.read(cx).tab() {
                Tab::Playlists => "Playlists",
                Tab::Albums => "Albums",
                Tab::Artists => "Artists",
                Tab::History => "History",
                Tab::Songs => "Songs",
                Tab::Stats => "Stats",
            },
            ListKind::Tracks => "Tracks",
        };
        let focused = keyboard_focused(&self.focus.list, window);
        let handle = self.scroll_handle(cx).clone();
        // The element the list sits in has to be tall (a bounded flex child), so the list only
        // draws the rows in view.
        let list = div()
            .id(id)
            .key_context(LIST_CONTEXT)
            .track_focus(&self.focus.list)
            .role(gpui::Role::List)
            .aria_label(name)
            .size_full()
            .min_h_0();
        Self::list_actions(list, cx)
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

    /// Arrow, paging, Home/End and Enter handling, for whichever list holds focus.
    fn list_actions(
        el: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        el.on_action(cx.listener(|this, _: &ListUp, _, cx| {
            // The plain arrows only move the cursor: picked songs do not follow it.
            this.selection.clear();
            this.move_selection(this.selected as isize - 1, cx)
        }))
        .on_action(cx.listener(|this, _: &ListDown, _, cx| {
            this.selection.clear();
            this.move_selection(this.selected as isize + 1, cx)
        }))
        .on_action(cx.listener(|this, _: &ExtendUp, _, cx| this.extend_selection(-1, cx)))
        .on_action(cx.listener(|this, _: &ExtendDown, _, cx| this.extend_selection(1, cx)))
        .on_action(cx.listener(|this, _: &SelectAllRows, _, cx| this.select_all_rows(cx)))
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
        .on_action(cx.listener(|this, _: &LikeRow, window, cx| {
            if this.run_bulk(Command::ToggleLike, window, cx) {
                return;
            }
            if let Some((video, _)) = this.selected_track(cx) {
                this.writes.update(cx, |w, cx| w.toggle_like(&video, cx));
            }
        }))
        .on_action(cx.listener(|this, _: &CrateRow, window, cx| {
            if this.run_bulk(Command::AddToCrate, window, cx) {
                return;
            }
            if let Some((track, thumbnail)) = this.selected_full_track(cx) {
                crate::crate_list::add_track(&this.player, track, thumbnail, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &AddRowToTarget, window, cx| {
            if !crate::target_playlist::key_enabled(cx) {
                return;
            }
            // Every picked song, or the cursor row's; the picker (with no target set) opens at
            // the cursor row.
            let songs: Vec<PickerSong> =
                if this.selection_enabled(cx) && this.selection.is_bulk() {
                    this.picked_songs(cx)
                        .iter()
                        .map(|s| PickerSong::new(s.track.video_id.as_str(), s.track.title.clone()))
                        .collect()
                } else {
                    this.selected_track(cx)
                        .map(|(video, title)| PickerSong::new(video, title))
                        .into_iter()
                        .collect()
                };
            crate::target_playlist::add_songs(
                songs,
                Some(&playlist_anchor(this.selected)),
                window,
                cx,
            );
        }))
        .on_action(cx.listener(|this, _: &AddRowToPlaylist, window, cx| {
            if !this.picker.read(cx).key_enabled(cx) {
                return;
            }
            if this.selection_enabled(cx) && this.selection.is_bulk() {
                // The picker opens at the cursor row, for every picked song.
                let ids: Vec<String> = this
                    .picked_songs(cx)
                    .iter()
                    .map(|s| s.track.video_id.as_str().to_owned())
                    .collect();
                let title = format!("{} songs", ids.len());
                let at = anchor::bounds_of(&playlist_anchor(this.selected), cx);
                this.picker
                    .update(cx, |p, cx| p.open_many(ids, title, at, window, cx));
                return;
            }
            if let Some((video, title)) = this.selected_track(cx) {
                let at = playlist_anchor(this.selected);
                this.picker
                    .update(cx, |p, cx| p.open_at(&video, title, &at, window, cx));
            }
        }))
    }

    // ---- picking songs ---------------------------------------------------------------------

    /// Songs can be picked on a playlist page (albums, the Songs tab and the rest come later).
    fn selection_enabled(&self, cx: &App) -> bool {
        let model = self.model.read(cx);
        model
            .detail()
            .is_some_and(|d| d.collection.kind == CollectionKind::Playlist)
    }

    /// A row's name in the selection: its own token, else its video id.
    fn row_key(row: &TrackRow) -> String {
        row.occurrence
            .clone()
            .unwrap_or_else(|| format!("v:{}", row.track.video_id.as_str()))
    }

    /// The key of every row shown, top to bottom (a filter leaves out the rows it hides).
    fn selection_keys(&self, cx: &App) -> Vec<String> {
        let rows = self.model.read(cx).list_rows();
        let visible = self.visible(cx);
        (0..visible.len())
            .filter_map(|n| visible.get(n).and_then(|real| rows.get(real)))
            .map(Self::row_key)
            .collect()
    }

    /// Per frame: forget picks that left the page, and work out which shown rows are picked and
    /// what the selection's menu is. Does nothing while nothing is picked.
    fn refresh_selection(&mut self, kind: ListKind, cx: &App) -> std::collections::HashSet<usize> {
        let enabled = kind == ListKind::Tracks && self.selection_enabled(cx);
        let page = enabled
            .then(|| self.model.read(cx).detail().map(|d| d.collection.library_key().to_owned()))
            .flatten();
        if page != self.selection_page {
            // Another playlist: nothing carries over.
            self.selection.clear();
            self.selection_page = page;
        }
        self.bulk_menu = None;
        if !enabled || self.selection.is_empty() {
            return Default::default();
        }
        let keys = self.selection_keys(cx);
        self.selection.keep(&keys);
        if self.selection.is_bulk() {
            self.bulk_menu = Subject::tracks(self.picked_songs(cx));
        }
        keys.iter()
            .enumerate()
            .filter(|(_, k)| self.selection.contains(k))
            .map(|(ix, _)| ix)
            .collect()
    }

    /// The picked songs in list order, as the track menu takes them.
    fn picked_songs(&self, cx: &App) -> Vec<TrackSubject> {
        let model = self.model.read(cx);
        let editable = model.detail().filter(|d| d.can_edit()).map(|d| d.collection.clone());
        let rows = model.list_rows();
        let visible = self.visible(cx);
        (0..visible.len())
            .filter_map(|n| visible.get(n).and_then(|real| rows.get(real)))
            .filter(|row| self.selection.contains(&Self::row_key(row)))
            .map(|row| {
                let playlist = editable
                    .clone()
                    .zip(row.occurrence.clone());
                TrackSubject::of_row(row.track.clone(), row.thumbnail.clone(), playlist)
            })
            .collect()
    }

    /// A click on shown row `ix`. Whether it was a Ctrl or Shift click (a pick), which the row's
    /// own action then leaves alone. A plain click clears the picks.
    fn pick_row(&mut self, ix: usize, how: Click, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.selection_enabled(cx) {
            return false;
        }
        let keys = self.selection_keys(cx);
        let Some(key) = keys.get(ix) else {
            return false;
        };
        let cursor = keys.get(self.selected);
        let picked = self.selection.click(how, key, cursor, &keys);
        if picked {
            // The keyboard carries on from the clicked row: Shift+Arrows, Ctrl+A and Esc work.
            window.focus(&self.focus.list, cx);
        }
        cx.notify();
        picked
    }

    /// Shift+Up / Shift+Down: the cursor moves and the picked rows stretch (or shrink) to it.
    fn extend_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if !self.selection_enabled(cx) {
            return;
        }
        let keys = self.selection_keys(cx);
        if keys.is_empty() {
            return;
        }
        let from = self.selected.min(keys.len() - 1);
        let to = (from as isize + delta).clamp(0, keys.len() as isize - 1) as usize;
        self.selection.extend(keys.get(from), &keys[to], &keys);
        self.move_selection(to as isize, cx);
    }

    /// Ctrl+A: every row shown.
    fn select_all_rows(&mut self, cx: &mut Context<Self>) {
        if !self.selection_enabled(cx) {
            return;
        }
        let keys = self.selection_keys(cx);
        self.selection.select_all(&keys);
        cx.notify();
    }

    /// Run a menu verb on the picked songs, when two or more are picked. Whether it did.
    fn run_bulk(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.selection_enabled(cx) || !self.selection.is_bulk() {
            return false;
        }
        if let Some(subject) = Subject::tracks(self.picked_songs(cx)) {
            track_menu::run_now(command, subject, window, cx);
        }
        true
    }

    /// The track row the keyboard is on, whole (none in the lists of playlists and albums).
    fn selected_full_track(&self, cx: &App) -> Option<(Track, Option<String>)> {
        let model = self.model.read(cx);
        let row = if let Some(artist) = model.artist() {
            artist.content()?.top_songs.get(self.selected)?.clone()
        } else if self.history_open(cx) {
            model.history_at(self.selected)?
        } else {
            let real = self.visible(cx).get(self.selected)?;
            model.list_rows().get(real)?.clone()
        };
        Some((row.track, row.thumbnail))
    }

    /// The video id and title of the track row the keyboard is on (none in the lists of
    /// playlists and albums, which are not tracks).
    fn selected_track(&self, cx: &App) -> Option<(String, SharedString)> {
        let model = self.model.read(cx);
        let row = if let Some(artist) = model.artist() {
            artist.content()?.top_songs.get(self.selected)?.clone()
        } else if self.history_open(cx) {
            model.history_at(self.selected)?
        } else if self.stats_open(cx) {
            // Only the Top tracks rows are songs.
            match model.stats() {
                Load::Ready(r) => {
                    let t = r.tracks.get(self.selected)?;
                    TrackRow {
                        track: t.track.clone(),
                        occurrence: None,
                        thumbnail: t.art.clone(),
                        explicit: false,
                        liked: None,
                    }
                }
                _ => return None,
            }
        } else {
            let real = self.visible(cx).get(self.selected)?;
            model.list_rows().get(real)?.clone()
        };
        Some((
            row.track.video_id.as_str().to_owned(),
            row.track.title.clone().into(),
        ))
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
        // A filtered track list draws the rows the filter keeps, each knowing its own place in
        // the loaded list.
        let visible = match kind {
            ListKind::Tracks => self.visible(cx),
            ListKind::Collections => Visible::All(usize::MAX),
        };
        let picked = self.refresh_selection(kind, cx);
        let rows: Vec<AnyElement> = range
            .map(|ix| {
                let level = self.hover.level(ix, &mut moving);
                let selected = list_focused && ix == self.selected;
                let frame = RowFrame {
                    ix,
                    real: visible.get(ix).unwrap_or(usize::MAX),
                    level,
                    selected,
                    picked: picked.contains(&ix),
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
    /// click, hover tracking. A track row passes its `track`: lingering on it gets the track
    /// resolved ahead of the click.
    fn row_shell(
        &self,
        frame: &RowFrame,
        height: f32,
        playing: bool,
        track: Option<Track>,
        menu: Subject,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        self.row_shell_with(frame, height, playing, track, Some(menu), on_click, cx)
    }

    /// [`row_shell`](Self::row_shell) for a row that may have no menu (an artist).
    #[allow(clippy::too_many_arguments)]
    fn row_shell_with(
        &self,
        frame: &RowFrame,
        height: f32,
        playing: bool,
        track: Option<Track>,
        menu: Option<Subject>,
        on_click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (ix, animations) = (frame.ix, frame.animations);
        let tint = if frame.picked {
            selection::row_tint(frame.accent)
        } else if frame.selected || playing {
            playing_row::tint()
        } else {
            color::state_hover().opacity(frame.level)
        };
        let row = div()
            .id(("row", ix))
            .debug_selector(move || format!("library-row-{ix}"))
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
                if let (true, Some(track)) = (*hovered, &track) {
                    this.player.update(cx, |p, cx| p.hint_play(track, cx));
                }
                cx.notify();
            }))
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                this.selected = ix;
                // Ctrl and Shift pick songs on a playlist page; a plain click clears the picks
                // and does what the row always did.
                if this.pick_row(ix, Click::of_event(event), window, cx) {
                    return;
                }
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
            });
        // Right-click opens the same menu as the More button.
        match menu {
            Some(menu) => track_menu::on_context_menu(row, menu),
            None => row,
        }
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
            false,
            None,
            Subject::Collection(c.clone()),
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

    /// Play (filled) and Shuffle, as a playlist, album or the Songs tab shows them. `enabled`
    /// is "there are rows, or they are on their way"; `pending` is the play command waiting
    /// for the rows, which keeps its own button busy.
    fn play_buttons(
        &self,
        enabled: bool,
        pending: Option<PlayRequest>,
        name: &'static str,
        accent: Accent,
        cx: &App,
    ) -> AnyElement {
        let play_busy = pending.is_some_and(|p| !p.skim && p.shuffle != Some(true));
        let shuffle_busy = pending.is_some_and(|p| !p.skim && p.shuffle == Some(true));
        let skim_busy = pending.is_some_and(|p| p.skim);
        let skim_shown = self.model.read(cx).skim_enabled(cx);
        let (model, model2, model3) = (
            self.model.clone(),
            self.model.clone(),
            self.model.clone(),
        );
        let (play_name, shuffle_name, skim_name) = match name {
            "songs" => ("songs-play", "songs-shuffle", "songs-skim"),
            _ => ("detail-play", "detail-shuffle", "detail-skim"),
        };
        div()
            .flex()
            .flex_row()
            .gap(px(spacing::S2))
            .mt(px(spacing::S2))
            .child(
                div().debug_selector(move || play_name.into()).child(
                    TextButton::new("library-play", &self.focus.play, "Play", accent)
                        .primary()
                        .enabled(enabled)
                        .busy(play_busy)
                        .on_click(move |_, cx| {
                            model.update(cx, |m, cx| m.request_play(PlayRequest::PLAY_ALL, cx))
                        }),
                ),
            )
            .child(
                div().debug_selector(move || shuffle_name.into()).child(
                    TextButton::new("library-shuffle", &self.focus.shuffle, "Shuffle", accent)
                        .enabled(enabled)
                        .busy(shuffle_busy)
                        .on_click(move |_, cx| {
                            model2.update(cx, |m, cx| m.request_play(PlayRequest::SHUFFLE, cx))
                        }),
                ),
            )
            .when(skim_shown, |d| {
                d.child(
                    div().debug_selector(move || skim_name.into()).child(
                        TextButton::new("library-skim", &self.focus.skim, "Skim", accent)
                            .enabled(enabled)
                            .busy(skim_busy)
                            .on_click(move |_, cx| {
                                model3.update(cx, |m, cx| m.request_play(PlayRequest::SKIM, cx))
                            }),
                    ),
                )
            })
            .into_any_element()
    }

    /// A playlist's description under its title and counts: two lines at most, and a click opens
    /// the Edit playlist dialog on a playlist of the account's own. Nothing when there is none.
    fn description_line(
        &self,
        collection: &Collection,
        text: Option<String>,
        editable: bool,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let text = text?;
        let el = style::text_style(div(), typography::BODY, cx)
            .id("detail-description")
            .debug_selector(|| "detail-description".into())
            .relative()
            .min_w_0()
            .rounded(px(radius::SMALL))
            .text_color(color::text_secondary())
            .line_clamp(DESCRIPTION_LINES)
            .child(SharedString::from(text));
        if !editable {
            return Some(el.into_any_element());
        }
        let focus = self.focus.description.clone();
        let ring = keyboard_focused(&focus, window);
        let collection = collection.clone();
        Some(
            el.cursor_pointer()
                .track_focus(&focus)
                .role(gpui::Role::Button)
                .aria_label("Edit description")
                .on_click(move |event, window, cx| {
                    // Space is the global play/pause key.
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == gpui::KeyboardButton::Space {
                            return;
                        }
                    }
                    playlist_dialog::open(Mode::Edit(collection.clone()), window, cx);
                })
                .when(ring, |d| d.child(focus_ring(color::accent(accent), radius::SMALL)))
                .into_any_element(),
        )
    }

    fn render_detail(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let animations = !cx.reduce_motion();
        let filter = self.ensure_filter(window, cx);
        let (collection, loading, failure, pending, items, skipped, confirmed_empty, editable, blurb) = {
            let d: &Detail = self.model.read(cx).detail().expect("detail is open");
            (
                d.collection.clone(),
                d.is_loading(),
                d.failure(),
                d.pending,
                d.listing.items().len(),
                d.listing.skipped(),
                d.listing.is_confirmed_empty(),
                d.can_edit(),
                d.meta.description.clone().filter(|t| !t.trim().is_empty()),
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
        // The count shows once every page has arrived; until then its line is held open, so the
        // header does not shift.
        let count = (!loading && failure.is_none())
            .then(|| track_count_text(collection.subtitle.as_deref(), items))
            .flatten();
        let blurb = self.description_line(&collection, blurb, editable, accent, window, cx);
        let buttons = self.play_buttons(can_play || loading, pending, "detail", accent, cx);

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
                        style::text_style(div(), typography::BODY, cx)
                            .id("detail-count")
                            .text_color(color::text_tertiary())
                            // A non-breaking space keeps the line's height while it is empty.
                            .child(count.unwrap_or_else(|| "\u{a0}".into())),
                    )
                    .children(blurb)
                    .child(buttons),
            )
            // "N selected" at the list header's right end while 2 or more songs are picked.
            .when(collection.kind == CollectionKind::Playlist, |d| {
                d.child(
                    selection::count_slot(self.selection.len(), cx)
                        .debug_selector(|| "library-selected-count".into()),
                )
            })
            // Right-aligned, once there are rows to filter.
            .children(filter.filter(|_| items > 0).map(|field| {
                div()
                    .debug_selector(|| "library-filter".into())
                    .w(px(FILTER_WIDTH))
                    .flex_none()
                    .child(field)
            }))
            .child(more_slot(
                "detail-more",
                true,
                Subject::Collection(collection.clone()),
            ));
        // Right-click on the header opens the same menu as the More button.
        let header = track_menu::on_context_menu(header, Subject::Collection(collection.clone()));
        let visible = self.visible(cx).len();
        let filtering = !self.filter_text(cx).is_empty();

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
        } else if filtering && visible == 0 && !loading {
            let this = cx.entity();
            self.notice(
                "No matches",
                false,
                "Clear",
                accent,
                move |_, cx| {
                    this.update(cx, |t, cx| {
                        t.clear_filter(cx);
                    })
                },
                cx,
            )
        } else {
            // One extra row at the end carries the loading / partial-failure / skipped note.
            // On a playlist of your own it is also where a row dropped last lands (a filter hides
            // some rows, so then rows cannot be dragged and there is no end to drop at).
            let footer = loading || failure.is_some() || skipped > 0 || (editable && !filtering);
            // A playlist of your own ends with its suggestions: more rows after the footer.
            let suggestions = self.rail_rows(cx);
            self.render_list(
                visible + usize::from(footer) + suggestions,
                ListKind::Tracks,
                window,
                cx,
            )
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
        let (ix, real) = (frame.ix, frame.real);
        // The History tab's rows are track rows with no page behind them.
        let history = self.history_open(cx);
        let songs = self.model.read(cx).songs_open();
        let (row, album_collection, loading, failure, skipped, editable): (
            Option<TrackRow>,
            bool,
            bool,
            Option<LoadFailure>,
            u32,
            Option<Collection>,
        ) = {
            let model = self.model.read(cx);
            if let Some(artist) = model.artist() {
                // Top songs: a plain list with no footer.
                let row = artist.content().and_then(|c| c.top_songs.get(real).cloned());
                (row, false, false, None, 0, None)
            } else if history {
                (model.history_at(real), false, false, None, 0, None)
            } else if songs {
                let page = model.songs();
                (
                    model.list_rows().get(real).cloned(),
                    false,
                    page.is_loading(),
                    page.failure(),
                    page.listing.skipped(),
                    None,
                )
            } else {
                let d = model.detail().expect("a page is open");
                (
                    d.listing.items().get(real).cloned(),
                    d.collection.kind == CollectionKind::Album,
                    d.is_loading(),
                    d.failure(),
                    d.listing.skipped(),
                    d.can_edit().then(|| d.collection.clone()),
                )
            }
        };
        // A filter hides some rows: a row's place in the list on screen is not its place in the
        // playlist, so nothing is dragged while one is in use.
        let filtering = !self.filter_text(cx).is_empty();
        let Some(row) = row else {
            if let Some(slot) = self.rail_slot(ix, cx) {
                return self.rail_row(slot, frame, window, cx);
            }
            return self.track_footer(
                ix,
                loading,
                failure,
                skipped,
                editable.is_some() && !filtering,
                frame.accent,
                cx,
            );
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
        let video: String = row.track.video_id.as_str().to_owned();
        let liked = self.writes.read(cx).is_liked(&video);
        // A row of a playlist of your own can be removed from it, and dragged to a new place.
        let subject = match (&editable, &row.occurrence) {
            // A picked row speaks for every picked row.
            _ if frame.picked && self.bulk_menu.is_some() => {
                self.bulk_menu.clone().expect("checked")
            }
            (Some(collection), Some(occurrence)) => Subject::playlist_track(
                row.track.clone(),
                row.thumbnail.clone(),
                collection.clone(),
                occurrence.clone(),
            ),
            _ => Subject::track(row.track.clone(), row.thumbnail.clone()),
        };
        let draggable = editable.is_some()
            && !filtering
            && self
                .model
                .read(cx)
                .detail()
                .is_some_and(|d| d.listing.items().iter().all(|r| r.occurrence.is_some()));
        let dragged = DraggedRow {
            from: real,
            title: row.track.title.clone().into(),
            artist: artists.clone(),
        };
        let actions = self.row_actions(
            ix,
            &video,
            row.track.title.clone().into(),
            liked,
            hovered || frame.selected,
            subject.clone(),
        );
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
            } else if history {
                // The history has no numbering: it is a log, not a list to count along.
                div().into_any_element()
            } else {
                // The row's place in the whole list, also while a filter hides some.
                style::tabular(style::text_style(div(), typography::BODY, cx))
                    .text_color(color::text_tertiary())
                    .child(format!("{}", real + 1))
                    .into_any_element()
            });

        self.row_shell(
            &frame,
            TRACK_ROW,
            playing,
            Some(row.track.clone()),
            subject,
            move |this, _, cx| {
                this.model.update(cx, |m, cx| {
                    if history {
                        m.play_history(real, cx)
                    } else {
                        m.request_play(PlayRequest::row(real), cx)
                    }
                })
            },
            cx,
        )
        .aria_label(row.track.title.clone())
        .when(playing, |d| {
            d.child(playing_row::marker(color::accent(accent)))
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
                    playing_row::title_style(typography::LIST_TITLE, playing),
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
        .child(actions)
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
        .when(draggable, |d| {
            let group: SharedString = format!("row-drop-{ix}").into();
            d.group(group.clone())
                .child(Self::drop_line(group))
                .on_drag(dragged, |dragged, _, _, cx| {
                    cx.new(|_| DragGhost(dragged.clone()))
                })
                .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
                    this.drop_row(dragged.from, ix, cx)
                }))
        })
        .into_any_element()
    }

    /// The thin accent line a dragged row shows above the row it would land before.
    fn drop_line(group: SharedString) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left(px(spacing::S2))
            .right(px(spacing::S2))
            .h(px(DROP_LINE))
            .rounded_full()
            .bg(color::text_secondary())
            .opacity(0.0)
            .group_drag_over::<DraggedRow>(group, |s| s.opacity(1.0))
    }

    /// A dragged row was let go before slot `slot`: the playlist reorders.
    fn drop_row(&mut self, from: usize, slot: usize, cx: &mut Context<Self>) {
        let moved = self.model.update(cx, |m, cx| m.move_row(from, slot, cx));
        if moved {
            // The row keeps the keyboard selection with it.
            self.selected = if slot > from { slot - 1 } else { slot };
        }
        cx.notify();
    }

    /// The like and add-to-playlist actions of a track row. Their room is always reserved, so
    /// nothing shifts when they appear: both show on hover or under the keyboard, and the heart
    /// stays once the track is liked.
    fn row_actions(
        &self,
        ix: usize,
        video: &str,
        title: SharedString,
        liked: bool,
        active: bool,
        subject: Subject,
    ) -> gpui::Div {
        let slot = || div().relative().size(px(ACTION_SIZE)).flex_none();
        let heart = slot()
            .debug_selector(move || format!("track-like-{ix}"))
            .when(active || liked, |d| {
                let (writes, video) = (self.writes.clone(), video.to_owned());
                d.child(heart_action(("track-like", ix), liked, move |_, cx| {
                    writes.update(cx, |w, cx| w.toggle_like(&video, cx))
                }))
            });
        let add = slot()
            .debug_selector(move || format!("track-playlist-{ix}"))
            // The popover opens here, also for the `p` key of the selected row.
            .child(anchor::probe(playlist_anchor(ix)))
            .when(active, |d| {
                let (picker, video) = (self.picker.clone(), video.to_owned());
                d.child(playlist_action(
                    ("track-playlist", ix),
                    move |window, cx| {
                        picker.update(cx, |p, cx| {
                            p.open_at(&video, title.clone(), &playlist_anchor(ix), window, cx)
                        })
                    },
                ))
            });
        let more = more_slot(format!("track-more-{ix}"), active, subject);
        div()
            .flex()
            .flex_row()
            .flex_none()
            .child(heart)
            .child(add)
            .child(more)
    }

    /// The row after the last track: still loading, a failure part-way, or unreadable rows.
    fn track_footer(
        &mut self,
        ix: usize,
        loading: bool,
        failure: Option<LoadFailure>,
        skipped: u32,
        editable: bool,
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
        let footer = div()
            .id(("track-footer", ix))
            .debug_selector(move || format!("library-row-{ix}"))
            .relative()
            .flex()
            .items_center()
            .h(px(TRACK_ROW))
            .px(px(spacing::S2))
            .child(body);
        if editable {
            // A row dropped here goes to the end of the playlist.
            let group: SharedString = format!("row-drop-{ix}").into();
            return footer
                .group(group.clone())
                .child(Self::drop_line(group))
                .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
                    this.drop_row(dragged.from, ix, cx)
                }))
                .into_any_element();
        }
        footer.into_any_element()
    }
}

/// "12 songs", for a collection whose own second line does not already say it. `None` for an
/// empty list.
fn track_count_text(subtitle: Option<&str>, loaded: usize) -> Option<SharedString> {
    let says_count = subtitle.is_some_and(|s| {
        let s = s.to_lowercase();
        s.contains("song") || s.contains("track")
    });
    if loaded == 0 || says_count {
        return None;
    }
    Some(if loaded == 1 {
        "1 song".into()
    } else {
        format!("{loaded} songs").into()
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Collections,
    Tracks,
}

struct RowFrame {
    /// The row's place on screen: names its hover state and its controls.
    ix: usize,
    /// Its place in the loaded list, which differs from `ix` while a filter hides rows.
    real: usize,
    /// Hover fade level, 0-1.
    level: f32,
    selected: bool,
    /// The row is one of the songs picked for bulk verbs.
    picked: bool,
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
        let (artist_open, detail_open, depth) = {
            let model = self.model.read(cx);
            (
                model.artist().is_some(),
                model.detail().is_some(),
                model.depth(),
            )
        };
        // Pages the model opened directly (not through this view) still get scroll state.
        self.pages.resize_with(depth, PageScroll::new);
        // A playlist deleted while its page is open: the page has nothing left to show.
        if let Some(key) = self.deleted.take() {
            let open_here = self
                .model
                .read(cx)
                .detail()
                .is_some_and(|d| d.collection.library_key() == key);
            if open_here {
                let this = cx.entity();
                window.defer(cx, move |window, cx| {
                    this.update(cx, |t, cx| t.back(window, cx));
                });
            }
        }
        let page = if self.signed_out {
            self.render_signed_out(accent, cx)
        } else if artist_open {
            self.render_artist(accent, window, cx)
        } else if detail_open {
            self.render_detail(accent, window, cx)
        } else {
            self.render_overview(accent, window, cx)
        };
        div()
            .id("library")
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|this, _: &Back, window, cx| {
                // Esc clears picked songs first, then empties a filter in use, then closes the page.
                if !this.selection.is_empty() {
                    this.selection.clear();
                    cx.notify();
                    return;
                }
                if !this.clear_filter(cx) {
                    this.back(window, cx)
                }
            }))
            .size_full()
            .min_h_0()
            .child(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_is_ours_only_when_the_second_line_does_not_say_it() {
        assert_eq!(
            track_count_text(Some("Neon Arcade • 2024"), 12).as_deref(),
            Some("12 songs")
        );
        assert_eq!(track_count_text(None, 1).as_deref(), Some("1 song"));
        assert_eq!(track_count_text(Some("Playlist • 3 songs"), 3), None);
        assert_eq!(track_count_text(Some("Album • 14 tracks"), 14), None);
        assert_eq!(
            track_count_text(Some("Someone"), 0),
            None,
            "nothing to count"
        );
    }
}
