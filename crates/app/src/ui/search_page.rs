//! The Search page: a field, a row of kind chips, then results.
//!
//! "All" is the mixed answer: a Top result card beside the top songs, then albums, artists and
//! playlists, each group with a "Show all" that jumps to its chip. The other chips are the
//! service's filtered searches: one kind of row, paged by scrolling.
//!
//! Songs play or queue; album, artist and playlist cards open their pages. Keyboard:
//! Ctrl+K or Ctrl+F opens the page with the field focused; Down moves from the field into the
//! songs, Up and Down move between them, Enter plays, Ctrl+Enter queues, Escape goes back to the
//! field (and out of it when the field is empty). Cards are tab stops after the list; Enter opens.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use apollo_domain::library::{Collection, CollectionKind};
use apollo_provider::{Hit, LinkTarget, ListedTrack, SearchResults, TopResult};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, deferred, div, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, ScrollHandle, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::library::model::{ArtistTarget, LibraryModel, Target};
use crate::library::source::TrackRow;
use crate::queue_source;
use crate::library::writes::LibraryWrites;
use crate::motion::HoverFade;
use crate::player::{format_time, PlayerModel};
use crate::search::local::{LocalHit, LocalResults, CHIP_CAP};
use crate::search::{
    as_link, failure_text, normalise, More, SearchChip, SearchEvent, SearchModel, SearchState,
    BAD_LINK_TEXT, HIDE_MUSIC_VIDEOS, SAVE_RECENT_SEARCHES,
};
use crate::settings::SettingsModel;
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::button::{focus_ring, keyboard_focused, IconButton};
use crate::ui::card::{self, grid, tile, Card};
use crate::ui::playlist_picker::PlaylistPicker;
use crate::ui::row_actions::{heart_action, more_slot, playlist_action, ACTION_SIZE};
use crate::ui::search_chips::ChipRow;
use crate::ui::search_library::LibraryRow;
use crate::ui::search_popover::{self, SearchPopover};
use crate::ui::top_result::{self, TopCard};
use crate::ui::selection::{self, Click, Selection};
use crate::ui::track_menu::{self, Command, Subject, TrackSubject};
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
        LikeSelected,
        CrateSelected,
        PlaylistSelected,
        BackToField,
        PreviousSuggestion,
        ExtendNext,
        ExtendPrevious,
        SelectAllSongs,
    ]
);

/// Key context of the results list.
pub const RESULTS_CONTEXT: &str = "SearchResults";

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    crate::ui::search_chips::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("down", FocusResults, Some(text_field::KEY_CONTEXT)),
        KeyBinding::new("up", PreviousSuggestion, Some(text_field::KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(RESULTS_CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(RESULTS_CONTEXT)),
        KeyBinding::new("enter", PlaySelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("ctrl-enter", QueueSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("l", LikeSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("c", CrateSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("p", PlaylistSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("escape", BackToField, Some(RESULTS_CONTEXT)),
        KeyBinding::new("shift-down", ExtendNext, Some(RESULTS_CONTEXT)),
        KeyBinding::new("shift-up", ExtendPrevious, Some(RESULTS_CONTEXT)),
        KeyBinding::new("ctrl-a", SelectAllSongs, Some(RESULTS_CONTEXT)),
    ]);
}

const ROW_HEIGHT: f32 = 56.0;
const ROW_ART: f32 = 40.0;
const SKELETON_ROWS: usize = 6;
/// Songs the All view lists (beside the Top result card); the rest are behind "Show all".
const ALL_SONGS: usize = 4;
/// Rows of skeleton under a filtered list while its next page loads.
const MORE_SKELETON_ROWS: usize = 3;
const SKELETON_CARDS: usize = 4;
/// Distance from the end of a filtered list at which the next page is asked for.
const LOAD_MORE_MARGIN: f32 = 600.0;
/// Tab stops the All view's own controls take beyond its cards: the Top result card and its
/// Play button, one "Show all" per group, and the "In your library" group's "Show all" and rows.
const EXTRA_STOPS: usize = 2 + 4 + 1 + crate::search::local::LEAD_ROWS;
/// The All view's group of the owner's own saved things.
const LIBRARY_GROUP: &str = "In your library";
const SELECTED_MARKER_WIDTH: f32 = 3.0;
/// Scrim over a row's artwork while it is the play target.
const ART_SCRIM: f32 = 0.6;

/// What a card does when opened: show that album, artist or playlist.
pub type OpenTarget = Rc<dyn Fn(Target, &mut Window, &mut App) + 'static>;

pub struct SearchPage {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    picker: Entity<PlaylistPicker>,
    model: Entity<SearchModel>,
    /// Where the owner's saved songs, albums, playlists and artists come from.
    library: Entity<LibraryModel>,
    /// The library index last handed to the search model.
    index_rev: Option<u64>,
    /// The field held text on the last change: a search starting from empty makes sure the
    /// library's lists are loaded.
    searching: bool,
    thumbs: Entity<ThumbStore>,
    field: Entity<TextField>,
    /// Tab stop of the results list (rows are not tab stops: Up and Down move between them).
    results_focus: FocusHandle,
    /// Tab stop of the message action (Retry, Clear).
    action_focus: FocusHandle,
    root_focus: FocusHandle,
    /// Tab stops of the cards, grown to the most cards any result had.
    card_focus: Vec<FocusHandle>,
    /// One tab stop per chip, in [`SearchChip::ROW`] order.
    chip_focus: Vec<FocusHandle>,
    /// What the list on show was for: when it changes the list starts again at the top.
    shown: (String, SearchChip),
    on_open: OpenTarget,
    scroll: ScrollHandle,
    /// The keyboard-selected song, shown only while the list holds focus.
    selected: Option<usize>,
    /// The songs picked for bulk verbs (Shift/Ctrl+click, Shift+Arrows, Ctrl+A), by video id.
    selection: Selection<String>,
    /// What a picked row's More button and right-click open: the menu of the whole selection.
    bulk_menu: Option<Subject>,
    /// This page, for the closures of rows (which are drawn from `&self`).
    this: gpui::WeakEntity<Self>,
    /// The popover under the field (recents or suggestions): the row under the keyboard or
    /// pointer, whether it was closed since the text last changed, and whether the field had
    /// focus when last drawn (gaining it brings the popover back).
    active: Option<usize>,
    dismissed: bool,
    field_was_focused: bool,
    /// The text a picked row just put in the field, so its change event does not reopen the
    /// popover.
    picked: Option<String>,
}

impl SearchPage {
    /// `root_focus` is where focus goes when the field is left with Escape; `on_open` is what a
    /// card does.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        model: Entity<SearchModel>,
        library: Entity<LibraryModel>,
        thumbs: Entity<ThumbStore>,
        settings: Entity<SettingsModel>,
        root_focus: FocusHandle,
        on_open: impl Fn(Target, &mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let on_open: OpenTarget = Rc::new(on_open);
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
                    this.selection.clear();
                    this.active = None;
                    // Typing brings the popover back; a row picked a moment ago does not.
                    if this.picked.take().as_deref() != Some(text.as_str()) {
                        this.dismissed = false;
                    }
                    this.start_searching(!text.trim().is_empty(), cx);
                    this.model.update(cx, |m, cx| m.set_query(&text, cx));
                }
                TextFieldEvent::Submit => this.submit(cx),
            },
        )
        .detach();
        {
            let on_open = on_open.clone();
            let player = player.clone();
            cx.subscribe(&model, move |_, _, event: &SearchEvent, cx| {
                let SearchEvent::OpenLink(target) = event;
                open_link(target.clone(), &player, &on_open, cx);
            })
            .detach();
        }
        cx.observe(&settings, |this, settings, cx| {
            this.sync_options(&settings, cx)
        })
        .detach();
        let initial = settings.clone();
        cx.observe(&model, |this, model, cx| {
            this.selected = None;
            let view = {
                let m = model.read(cx);
                (m.query().to_owned(), m.chip())
            };
            if view != this.shown {
                // Another query or kind: the list starts at its top, with nothing picked.
                this.shown = view;
                this.selection.clear();
                this.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
            }
            this.request_thumbs(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        cx.observe(&library, |this, _, cx| this.sync_library(cx)).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        let mut page = Self {
            player,
            writes,
            picker,
            model,
            library,
            index_rev: None,
            searching: false,
            thumbs,
            field,
            results_focus,
            action_focus,
            root_focus,
            card_focus: Vec::new(),
            chip_focus: (0..SearchChip::ROW.len())
                .map(|_| cx.focus_handle().tab_index(6).tab_stop(true))
                .collect(),
            shown: (String::new(), SearchChip::All),
            on_open,
            scroll: ScrollHandle::new(),
            selected: None,
            selection: Selection::new(),
            bulk_menu: None,
            this: cx.weak_entity(),
            active: None,
            dismissed: false,
            field_was_focused: false,
            picked: None,
        };
        page.sync_options(&initial, cx);
        page.sync_library(cx);
        page
    }

    /// Hand the library's lists to the search model when they have changed.
    fn sync_library(&mut self, cx: &mut Context<Self>) {
        let (rev, index) = {
            let library = self.library.read(cx);
            let (rev, index) = library.search_index();
            (rev, index.clone())
        };
        if self.index_rev != Some(rev) {
            self.index_rev = Some(rev);
            self.model.update(cx, |m, cx| m.set_library(index, cx));
        }
    }

    /// Typing from an empty field starts a search: the library loads whatever of its lists the
    /// screens have not (the artists, the liked songs), once.
    fn start_searching(&mut self, has_text: bool, cx: &mut Context<Self>) {
        if has_text && !self.searching {
            self.library.update(cx, |l, cx| l.ensure_searchable(cx));
        }
        self.searching = has_text;
    }

    /// Hand the Settings > Search switches to the model.
    fn sync_options(&mut self, settings: &Entity<SettingsModel>, cx: &mut Context<Self>) {
        let (save, hide) = {
            let s = settings.read(cx);
            (s.get(&SAVE_RECENT_SEARCHES), s.get(&HIDE_MUSIC_VIDEOS))
        };
        self.model.update(cx, |m, cx| m.set_options(save, hide, cx));
    }

    /// Back from a page opened from here: focus returns to the results (or the field when
    /// there are none), where the cards and songs are.
    pub fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let shown = model.results().is_some() || model.local_shown().is_some_and(|l| !l.is_empty());
        if shown {
            window.focus(&self.results_focus, cx);
        } else {
            text_field::focus(&self.field, window, cx);
        }
    }

    pub fn field(&self) -> &Entity<TextField> {
        &self.field
    }

    /// The chip holding keyboard focus, if any (tests and diagnostics).
    pub fn focused_chip(&self, window: &Window) -> Option<SearchChip> {
        SearchChip::ROW
            .into_iter()
            .zip(&self.chip_focus)
            .find(|(_, focus)| focus.is_focused(window))
            .map(|(chip, _)| chip)
    }

    /// How many songs are picked for bulk verbs (tests and diagnostics).
    pub fn picked(&self) -> usize {
        self.selection.len()
    }

    /// The index of the keyboard-selected song, if any (tests and diagnostics).
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// The songs on show (and so the keyboard's), in the order they are drawn. On All: the first
    /// few beside the Top result, then the library's own in its group. On Library: every saved
    /// song that matched. Otherwise every one of a filtered Songs list.
    fn songs(&self, cx: &App) -> Vec<ListedTrack> {
        let model = self.model.read(cx);
        match model.chip() {
            SearchChip::Library => model
                .local_shown()
                .map(|l| l.songs.iter().take(CHIP_CAP).map(listed).collect())
                .unwrap_or_default(),
            chip => {
                let cap = if chip == SearchChip::All {
                    ALL_SONGS
                } else {
                    usize::MAX
                };
                let mut songs: Vec<ListedTrack> = model
                    .results()
                    .map(|r| r.songs.iter().take(cap).cloned().collect())
                    .unwrap_or_default();
                if chip == SearchChip::All {
                    songs.extend(model.local_shown().into_iter().flat_map(|l| l.lead_songs()).map(listed));
                }
                songs
            }
        }
    }

    /// How many of [`Self::songs`] are the service's (the rest are the library's).
    fn remote_songs(&self, cx: &App) -> usize {
        let model = self.model.read(cx);
        match model.chip() {
            SearchChip::Library => 0,
            SearchChip::All => model
                .results()
                .map_or(0, |r| r.songs.len().min(ALL_SONGS)),
            _ => usize::MAX,
        }
    }

    /// The scroll container's child that holds song `index`: All lists its songs in its first
    /// row and the library's in the group under it, a Songs list has one child per song, and
    /// the Library chip puts the Songs heading first.
    fn song_child(&self, index: usize, cx: &App) -> usize {
        let model = self.model.read(cx);
        match model.chip() {
            SearchChip::All if index < self.remote_songs(cx) => 0,
            SearchChip::All => self.lead_child(cx),
            SearchChip::Library => index + 1,
            _ => index,
        }
    }

    /// The scroll container's child that holds the "In your library" group on All: the first,
    /// or the one after the Top result row when the page draws one.
    fn lead_child(&self, cx: &App) -> usize {
        let model = self.model.read(cx);
        match (model.state(), model.results()) {
            (SearchState::Ready | SearchState::Loading, Some(r)) if !model.is_confirmed_empty() => {
                usize::from(r.top.is_some() || !r.songs.is_empty())
            }
            _ => 0,
        }
    }

    /// A chip was picked: its list starts at the top with nothing selected.
    fn select_chip(&mut self, chip: SearchChip, cx: &mut Context<Self>) {
        self.selected = None;
        self.model.update(cx, |m, cx| m.set_chip(chip, cx));
    }

    fn request_thumbs(&mut self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let mut urls: Vec<String> = model
            .results()
            .map(|r| thumbnails(r).collect())
            .unwrap_or_default();
        if let Some(local) = model.local_shown() {
            urls.extend(local_thumbnails(local, model.chip()));
        }
        if urls.is_empty() {
            return;
        }
        self.thumbs.update(cx, |store, cx| {
            for url in &urls {
                store.request(url, cx);
            }
        });
    }

    /// Enter in the field: a highlighted popover row is picked, anything else is searched for.
    fn submit(&mut self, cx: &mut Context<Self>) {
        if let Some(row) = self
            .active
            .and_then(|i| self.popover_rows(cx).get(i).cloned())
        {
            if self.popover_visible(cx) {
                self.pick_text(row.to_string(), cx);
                return;
            }
        }
        self.dismissed = true;
        self.active = None;
        self.model.update(cx, |m, cx| m.submit(cx));
        cx.notify();
    }

    /// A popover row was chosen (Enter or a click): the field takes its text and it is
    /// searched for at once.
    fn pick_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.dismissed = true;
        self.active = None;
        self.picked = Some(normalise(&text));
        self.field.update(cx, |f, cx| f.set_text(&text, cx));
        self.model.update(cx, |m, cx| m.search_for(&text, cx));
        cx.notify();
    }

    /// What the popover would list: the recents while the field is empty, the completions while
    /// it holds text (none for a pasted link).
    fn popover_rows(&self, cx: &App) -> Vec<SharedString> {
        let typed = normalise(self.field.read(cx).text());
        let model = self.model.read(cx);
        let rows: Vec<String> = if typed.is_empty() {
            model.recents().to_vec()
        } else if as_link(&typed).is_some() {
            Vec::new()
        } else {
            model.suggestions().to_vec()
        };
        rows.into_iter().map(Into::into).collect()
    }

    fn popover_kind(&self, cx: &App) -> search_popover::Kind {
        if self.field.read(cx).text().trim().is_empty() {
            search_popover::Kind::Recents
        } else {
            search_popover::Kind::Suggestions
        }
    }

    /// The popover shows while the field has focus, was not closed since the text last
    /// changed, and there is something to list.
    fn popover_visible(&self, cx: &App) -> bool {
        self.field_was_focused && !self.dismissed && !self.popover_rows(cx).is_empty()
    }

    fn step_popover(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.popover_rows(cx).len() as isize;
        let next = match (self.active, delta) {
            (None, d) if d > 0 => Some(0),
            (None, _) => None,
            (Some(i), d) => {
                let i = i as isize + d;
                (i >= 0).then(|| i.min(count - 1) as usize)
            }
        };
        self.active = next;
        cx.notify();
    }

    fn previous_suggestion(
        &mut self,
        _: &PreviousSuggestion,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.popover_visible(cx) {
            self.step_popover(-1, cx);
        } else {
            cx.propagate();
        }
    }

    /// Escape closes the popover before it does anything else.
    fn close_popover(&mut self, _: &text_field::Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.popover_visible(cx) {
            self.dismissed = true;
            self.active = None;
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn focus_results(&mut self, _: &FocusResults, window: &mut Window, cx: &mut Context<Self>) {
        if self.popover_visible(cx) {
            self.step_popover(1, cx);
            return;
        }
        if self.songs(cx).is_empty() {
            return;
        }
        self.selected = Some(0);
        self.hint_selected(cx);
        window.focus(&self.results_focus, cx);
        self.scroll.scroll_to_item(0);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.songs(cx).len();
        if count == 0 {
            return;
        }
        // The plain arrows only move the cursor: a picked set does not follow it.
        self.selection.clear();
        let next = self.selected.map_or(0, |i| (i + 1).min(count - 1));
        self.select(next, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.clear();
        match self.selected {
            Some(i) if i > 0 => self.select(i - 1, cx),
            _ => self.leave_list(window, cx),
        }
    }

    // ---- picking songs ---------------------------------------------------------------------

    /// The key of every song on the list, top to bottom (a song is named by its video id).
    fn song_keys(&self, cx: &App) -> Vec<String> {
        self.songs(cx)
            .iter()
            .map(|s| s.track.video_id.as_str().to_owned())
            .collect()
    }

    /// The picked songs in list order, as the track menu takes them.
    fn picked_songs(&self, cx: &App) -> Vec<TrackSubject> {
        let mut seen = std::collections::HashSet::new();
        self.songs(cx)
            .into_iter()
            .filter(|s| {
                let key = s.track.video_id.as_str().to_owned();
                self.selection.contains(&key) && seen.insert(key)
            })
            .map(|s| TrackSubject::of_row(s.track, s.thumbnail, None))
            .collect()
    }

    /// Drop picks that left the list and work out the selection's menu for this frame.
    fn refresh_selection(&mut self, cx: &App) {
        let keys = self.song_keys(cx);
        self.selection.keep(&keys);
        self.bulk_menu = if self.selection.is_bulk() {
            Subject::tracks(self.picked_songs(cx))
        } else {
            None
        };
    }

    /// A click on song `index` (Ctrl, Shift or plain; the row's own play is the art's and the
    /// text's business).
    fn row_clicked(&mut self, index: usize, how: Click, window: &mut Window, cx: &mut Context<Self>) {
        let keys = self.song_keys(cx);
        let Some(key) = keys.get(index) else {
            return;
        };
        let cursor = self.selected.and_then(|i| keys.get(i));
        if self.selection.click(how, key, cursor, &keys) {
            // The keyboard carries on from the clicked row: Shift+Arrows, Ctrl+A and Esc work.
            self.selected = Some(index);
            window.focus(&self.results_focus, cx);
            self.hint_selected(cx);
        }
        cx.notify();
    }

    fn extend_next(&mut self, _: &ExtendNext, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_by(1, cx);
    }

    fn extend_previous(&mut self, _: &ExtendPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.extend_by(-1, cx);
    }

    /// Shift+Up / Shift+Down: the cursor moves and the picked rows stretch (or shrink) to it.
    fn extend_by(&mut self, delta: isize, cx: &mut Context<Self>) {
        let keys = self.song_keys(cx);
        if keys.is_empty() {
            return;
        }
        let to = match self.selected {
            Some(i) => (i as isize + delta).clamp(0, keys.len() as isize - 1) as usize,
            None => 0,
        };
        let from = self.selected.and_then(|i| keys.get(i));
        self.selection.extend(from, &keys[to], &keys);
        self.select(to, cx);
    }

    /// Ctrl+A: every song on the list.
    fn select_all(&mut self, _: &SelectAllSongs, _: &mut Window, cx: &mut Context<Self>) {
        let keys = self.song_keys(cx);
        self.selection.select_all(&keys);
        cx.notify();
    }

    /// Run a menu verb on the picked songs, when two or more are picked. Whether it did.
    fn run_bulk(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.selection.is_bulk() {
            return false;
        }
        if let Some(subject) = Subject::tracks(self.picked_songs(cx)) {
            track_menu::run_now(command, subject, window, cx);
        }
        true
    }

    /// The keyboard selection is resting on a song: have it ready in case it is played.
    fn hint_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.player.update(cx, |m, cx| m.hint_play(&song.track, cx));
        }
    }

    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        self.selected = Some(index);
        self.hint_selected(cx);
        self.scroll.scroll_to_item(self.song_child(index, cx));
        cx.notify();
    }

    /// Esc: picked songs go first; with none picked it leaves the list.
    fn back_to_field(&mut self, _: &BackToField, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selection.is_empty() {
            self.selection.clear();
            cx.notify();
            return;
        }
        self.leave_list(window, cx);
    }

    fn leave_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = None;
        text_field::focus(&self.field, window, cx);
        cx.notify();
    }

    fn play_selected(&mut self, _: &PlaySelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.model.update(cx, |m, cx| m.remember(cx));
            self.player.update(cx, |m, cx| {
                m.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
                m.play_now(song.track, cx)
            });
        }
    }

    fn queue_selected(&mut self, _: &QueueSelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_bulk(Command::AddToQueue, window, cx) {
            return;
        }
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.player.update(cx, |m, cx| {
                m.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
                m.add_to_queue(song.track, cx)
            });
        }
    }

    fn like_selected(&mut self, _: &LikeSelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_bulk(Command::ToggleLike, window, cx) {
            return;
        }
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.writes
                .update(cx, |w, cx| w.toggle_like(song.track.video_id.as_str(), cx));
        }
    }

    fn crate_selected(&mut self, _: &CrateSelected, window: &mut Window, cx: &mut Context<Self>) {
        if self.run_bulk(Command::AddToCrate, window, cx) {
            return;
        }
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            crate::crate_list::add_track(&self.player, song.track, song.thumbnail, cx);
        }
    }

    fn playlist_selected(
        &mut self,
        _: &PlaylistSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.picker.read(cx).key_enabled(cx) {
            return;
        }
        if self.selection.is_bulk() {
            // The picker opens at the cursor row, for every picked song.
            let ids: Vec<String> = self
                .picked_songs(cx)
                .iter()
                .map(|s| s.track.video_id.as_str().to_owned())
                .collect();
            let title = format!("{} songs", ids.len());
            let at = anchor::bounds_of(&playlist_anchor(self.selected.unwrap_or_default()), cx);
            self.picker
                .update(cx, |p, cx| p.open_many(ids, title, at, window, cx));
            return;
        }
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            let at = playlist_anchor(self.selected.unwrap_or_default());
            self.picker.update(cx, |p, cx| {
                p.open_at(
                    song.track.video_id.as_str(),
                    song.track.title.clone(),
                    &at,
                    window,
                    cx,
                )
            });
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

/// The artwork of the library rows on show: the lead rows on All, every capped list on Library.
fn local_thumbnails(local: &LocalResults, chip: SearchChip) -> Vec<String> {
    match chip {
        SearchChip::All => local
            .lead
            .iter()
            .filter_map(|hit| match hit {
                LocalHit::Song(row) => row.thumbnail.clone(),
                LocalHit::Album(c) | LocalHit::Playlist(c) => c.thumbnail.clone(),
                LocalHit::Artist(h) => h.thumbnail.clone(),
            })
            .collect(),
        SearchChip::Library => local
            .songs
            .iter()
            .take(CHIP_CAP)
            .filter_map(|r| r.thumbnail.clone())
            .chain(
                local
                    .albums
                    .iter()
                    .chain(&local.playlists)
                    .take(CHIP_CAP)
                    .filter_map(|c| c.thumbnail.clone()),
            )
            .chain(
                local
                    .artists
                    .iter()
                    .take(CHIP_CAP)
                    .filter_map(|h| h.thumbnail.clone()),
            )
            .collect(),
        _ => Vec::new(),
    }
}

/// A library row as the lists of search results carry songs.
fn listed(row: &TrackRow) -> ListedTrack {
    ListedTrack {
        track: row.track.clone(),
        occurrence: row.occurrence.clone(),
        thumbnail: row.thumbnail.clone(),
        explicit: row.explicit,
        liked: row.liked,
    }
}

/// A saved album, playlist or artist as the cards of search results carry them.
fn hit_of(collection: &Collection) -> Hit {
    Hit {
        id: collection.id.clone(),
        title: collection.title.clone(),
        subtitle: collection.subtitle.clone(),
        thumbnail: collection.thumbnail.clone(),
        explicit: false,
    }
}

/// A pasted link was looked up: a track plays, a page opens.
fn open_link(target: LinkTarget, player: &Entity<PlayerModel>, on_open: &OpenTarget, cx: &mut App) {
    match target {
        LinkTarget::Track(song) => player.update(cx, |m, cx| {
            m.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
            m.play_now(song.track, cx)
        }),
        LinkTarget::Collection(collection) => {
            open_page(Target::Collection(collection), on_open, cx)
        }
        LinkTarget::Artist {
            id,
            name,
            thumbnail,
        } => open_page(
            Target::Artist(ArtistTarget {
                id,
                name,
                thumbnail,
            }),
            on_open,
            cx,
        ),
    }
}

/// Open a page from outside a window callback: through the app's window.
fn open_page(target: Target, on_open: &OpenTarget, cx: &mut App) {
    let Some(window) = cx.windows().into_iter().next() else {
        return;
    };
    let on_open = on_open.clone();
    let _ = window.update(cx, move |_, window, cx| on_open(target, window, cx));
}

impl Render for SearchPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let accent = self.player.read(cx).view().accent;
        let model = self.model.read(cx);
        let state = model.state();
        let chip = model.chip();
        let query: SharedString = model.query().to_owned().into();
        let results = model.results().cloned();
        let confirmed_empty = model.is_confirmed_empty();
        let local: Option<Arc<LocalResults>> = model.local_shown().cloned();
        let local_empty = model.is_local_empty();
        let more = model.more();
        let show_chips = matches!(
            state,
            SearchState::Loading | SearchState::Ready | SearchState::Failed(_)
        ) && as_link(&query).is_none();

        let cards = results.as_ref().map_or(0, |r| {
            r.albums.len() + r.artists.len() + r.playlists.len()
        }) + local.as_ref().map_or(0, |l| {
            l.albums.len().min(CHIP_CAP) + l.playlists.len().min(CHIP_CAP) + l.artists.len().min(CHIP_CAP)
        }) + EXTRA_STOPS;
        while self.card_focus.len() < cards {
            self.card_focus
                .push(cx.focus_handle().tab_index(7).tab_stop(true));
        }

        let field_focus = self.field.read(cx).focus_handle(cx);
        let field_ring = keyboard_focused(&field_focus, window);
        let field_focused = field_focus.is_focused(window);
        if field_focused && !self.field_was_focused {
            // Back in the field: the recents show again, suggestions wait for typing.
            self.dismissed = !self.field.read(cx).text().is_empty();
            self.active = None;
        }
        self.field_was_focused = field_focused;
        let list_focused = self.results_focus.is_focused(window);
        let popover = self.popover(cx);
        if chip.is_remote() && chip != SearchChip::All && state == SearchState::Ready {
            self.ask_for_more(cx);
        }
        // The library rows of the All view: its best few, once the page has any.
        let lead: Option<Arc<LocalResults>> = local
            .clone()
            .filter(|l| chip == SearchChip::All && !l.lead.is_empty());

        self.refresh_selection(cx);

        // Every item is its own child of the scroll container, so a keyboard selection can
        // scroll to it.
        let body: Vec<AnyElement> = match (state, results) {
            (SearchState::Idle, _) => Vec::new(),
            (SearchState::BadLink, _) => {
                let field = self.field.clone();
                vec![self.message(
                    BAD_LINK_TEXT.into(),
                    "Clear",
                    accent,
                    move |window, cx| {
                        field.update(cx, |f, cx| f.set_text("", cx));
                        text_field::focus(&field, window, cx);
                    },
                    cx,
                )]
            }
            // The Library chip never asks the service: its list is the local answer.
            (SearchState::Ready, _) if chip == SearchChip::Library => {
                if local_empty {
                    vec![self.no_results(&query, accent, cx)]
                } else {
                    local.as_ref().map_or_else(Vec::new, |l| {
                        self.library_view(l, list_focused, accent, cx)
                    })
                }
            }
            (SearchState::Failed(failure), _) => {
                let model = self.model.clone();
                // What the library holds is still there when the service is not.
                let mut items = lead
                    .as_ref()
                    .map(|l| self.lead_group(l, false, 0, 0, list_focused, accent, cx))
                    .into_iter()
                    .collect::<Vec<_>>();
                items.push(self.message(
                    failure_text(failure).into(),
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.retry(cx)),
                    cx,
                ));
                items
            }
            (SearchState::Ready, Some(_)) if confirmed_empty => match &lead {
                // The service found nothing, the library did: the group says so.
                Some(l) => vec![self.lead_group(l, false, 0, 0, list_focused, accent, cx)],
                None => vec![self.no_results(&query, accent, cx)],
            },
            (_, Some(results)) => self.results_list(
                &results,
                lead.as_deref(),
                chip,
                more,
                list_focused,
                accent,
                cx,
            ),
            (SearchState::Loading, None) => {
                // The library answers before the service does.
                let mut items = lead
                    .as_ref()
                    .map(|l| self.lead_group(l, false, 0, 0, list_focused, accent, cx))
                    .into_iter()
                    .collect::<Vec<_>>();
                items.push(skeletons(chip, cx));
                items
            }
            // Ready without results cannot happen; show nothing rather than guess.
            (SearchState::Ready, None) => Vec::new(),
        };
        let chips = show_chips.then(|| {
            let page = cx.entity();
            ChipRow::new(chip, accent, &self.chip_focus, move |chip, _, cx| {
                page.update(cx, |p, cx| p.select_chip(chip, cx))
            })
        });

        let picked = self.selection.len();
        let fields_focus = self.results_focus.clone();
        div()
            .id("search-page")
            .on_action(cx.listener(Self::focus_results))
            .on_action(cx.listener(Self::previous_suggestion))
            .capture_action(cx.listener(Self::close_popover))
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
                    .debug_selector(|| "search-field".into())
                    .child(self.field.clone())
                    .when(field_ring, |d| {
                        d.child(focus_ring(color::accent(accent), radius::SMALL))
                    })
                    .children(popover.map(|popover| {
                        // Deferred so it paints over the results below the field.
                        deferred(
                            div()
                                .absolute()
                                .top_full()
                                .mt(px(spacing::S1))
                                .left_0()
                                .right_0()
                                .child(popover),
                        )
                        .with_priority(1)
                    })),
            )
            .children(chips.map(|chips| {
                // "N selected" sits at the right end of the chip row while 2 or more are picked.
                div()
                    .flex_none()
                    .mt(px(spacing::S4))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(chips)
                    .children(selection::count_label(picked, cx).map(|label| {
                        label.debug_selector(|| "search-selected-count".into())
                    }))
            }))
            .child(
                div()
                    .id("search-results")
                    .key_context(RESULTS_CONTEXT)
                    .track_focus(&fields_focus)
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_previous))
                    .on_action(cx.listener(Self::play_selected))
                    .on_action(cx.listener(Self::queue_selected))
                    .on_action(cx.listener(Self::like_selected))
                    .on_action(cx.listener(Self::crate_selected))
                    .on_action(cx.listener(Self::playlist_selected))
                    .on_action(cx.listener(Self::back_to_field))
                    .on_action(cx.listener(Self::extend_next))
                    .on_action(cx.listener(Self::extend_previous))
                    .on_action(cx.listener(Self::select_all))
                    .role(gpui::Role::ListBox)
                    .aria_label("Search results")
                    .flex_1()
                    .min_h_0()
                    .mt(px(spacing::S4))
                    .pb(px(spacing::S6))
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(body.into_iter().map(|item| div().flex_none().child(item))),
            )
    }
}

impl SearchPage {
    /// The popover under the field, when it should show.
    fn popover(&self, cx: &mut Context<Self>) -> Option<SearchPopover> {
        if !self.popover_visible(cx) {
            return None;
        }
        let rows = self.popover_rows(cx);
        let kind = self.popover_kind(cx);
        let page = cx.entity();
        let pick = {
            let (page, rows) = (page.clone(), rows.clone());
            Rc::new(move |ix: usize, _: &mut Window, cx: &mut App| {
                if let Some(row) = rows.get(ix) {
                    let text = row.to_string();
                    page.update(cx, |p, cx| p.pick_text(text, cx));
                }
            })
        };
        let hover = {
            let page = page.clone();
            Rc::new(move |ix: usize, _: &mut Window, cx: &mut App| {
                page.update(cx, |p, cx| {
                    if p.active != Some(ix) {
                        p.active = Some(ix);
                        cx.notify();
                    }
                });
            })
        };
        let remove = {
            let (page, rows) = (page.clone(), rows.clone());
            Rc::new(move |ix: usize, _: &mut Window, cx: &mut App| {
                if let Some(row) = rows.get(ix) {
                    let text = row.to_string();
                    page.update(cx, |p, cx| {
                        p.active = None;
                        p.model.update(cx, |m, cx| m.remove_recent(&text, cx));
                    });
                }
            })
        };
        let clear = Rc::new(move |_: &mut Window, cx: &mut App| {
            page.update(cx, |p, cx| {
                p.active = None;
                p.model.update(cx, |m, cx| m.clear_recents(cx));
            });
        });
        Some(SearchPopover {
            kind,
            rows,
            active: self.active,
            on_pick: pick,
            on_hover: hover,
            on_remove: remove,
            on_clear: clear,
        })
    }

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

/// Where the add-to-playlist popover of song row `index` is anchored.
fn playlist_anchor(index: usize) -> String {
    format!("song-playlist-{index}")
}

impl SearchPage {
    /// Asks for the next page of a filtered list once it is nearly scrolled to its end (or does
    /// not fill the window yet).
    fn ask_for_more(&self, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        if !model.has_more() || model.more() != More::Idle {
            return;
        }
        let left = f32::from(self.scroll.max_offset().y) + f32::from(self.scroll.offset().y);
        if left < LOAD_MORE_MARGIN {
            self.model.update(cx, |m, cx| m.load_more(cx));
        }
    }

    /// The chip a group's "Show all" opens. Search's mixed answer does not say which of its
    /// playlists are the service's own and which are the community's, so the first one's byline
    /// decides.
    fn playlists_chip(hits: &[Hit]) -> SearchChip {
        let featured = hits.first().is_some_and(|h| {
            h.subtitle
                .as_deref()
                .is_some_and(|s| s.contains("YouTube Music"))
        });
        if featured {
            SearchChip::Playlists
        } else {
            SearchChip::CommunityPlaylists
        }
    }

    fn results_list(
        &self,
        results: &SearchResults,
        lead: Option<&LocalResults>,
        chip: SearchChip,
        more: More,
        list_focused: bool,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut items: Vec<AnyElement> = Vec::new();
        let page = cx.entity();
        let show_all = |id: &'static str, to: SearchChip, stop: usize| {
            let page = page.clone();
            div()
                .debug_selector(move || id.into())
                .child(
                    TextButton::new(id, &self.card_focus[stop], "Show all", accent)
                        .on_click(move |_, cx| page.update(cx, |p, cx| p.select_chip(to, cx))),
                )
                .into_any_element()
        };
        // Tab stops are taken in reading order across the Top result, the "Show all" buttons
        // and the cards.
        let mut stop = 0;
        let mut next_stop = || {
            stop += 1;
            stop - 1
        };

        match chip {
            SearchChip::All => {
                let songs: Vec<&ListedTrack> = results.songs.iter().take(ALL_SONGS).collect();
                if results.top.is_some() || !songs.is_empty() {
                    let mut row = div().flex().flex_row().items_start().gap(px(spacing::S6));
                    if let Some(top) = &results.top {
                        let (card_stop, play_stop) = (next_stop(), next_stop());
                        row = row.child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_none()
                                .w(px(top_result::WIDTH))
                                .child(group_header("Top result", false, None, cx))
                                .child(self.top_card(
                                    top,
                                    results,
                                    &songs,
                                    card_stop,
                                    play_stop,
                                    accent,
                                    cx,
                                )),
                        );
                    }
                    if !songs.is_empty() {
                        let all = show_all("show-all-songs", SearchChip::Songs, next_stop());
                        let mut column = div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(group_header("Songs", false, Some(all), cx));
                        for (index, song) in songs.iter().enumerate() {
                            column = column.child(self.song_row(
                                index,
                                song,
                                None,
                                list_focused,
                                accent,
                                cx,
                            ));
                        }
                        row = row.child(column);
                    }
                    items.push(row.into_any_element());
                }
                // The library's own matches sit directly under that row.
                if let Some(lead) = lead {
                    let first_stop = next_stop();
                    for _ in lead.lead.iter() {
                        next_stop();
                    }
                    items.push(self.lead_group(
                        lead,
                        true,
                        first_stop,
                        songs.len(),
                        list_focused,
                        accent,
                        cx,
                    ));
                }
                // The Top result is also the first row of its own group; it is not shown twice.
                let top_hit = match &results.top {
                    Some(TopResult::Album(h) | TopResult::Artist(h) | TopResult::Playlist(h)) => {
                        Some(&h.id)
                    }
                    _ => None,
                };
                let playlists_to = Self::playlists_chip(&results.playlists);
                let groups: [(&'static str, &'static str, &Vec<Hit>, SearchChip, &'static str); 3] = [
                    ("Albums", "album", &results.albums, SearchChip::Albums, "show-all-albums"),
                    ("Artists", "artist", &results.artists, SearchChip::Artists, "show-all-artists"),
                    ("Playlists", "playlist", &results.playlists, playlists_to, "show-all-playlists"),
                ];
                for (title, kind, hits, to, id) in groups {
                    let shown: Vec<&Hit> = hits.iter().filter(|h| Some(&h.id) != top_hit).collect();
                    if shown.is_empty() {
                        continue;
                    }
                    let all = show_all(id, to, next_stop());
                    items.push(group_header(title, !items.is_empty(), Some(all), cx));
                    let cards: Vec<Card> = shown
                        .into_iter()
                        .enumerate()
                        .map(|(index, hit)| {
                            self.hit_card(kind, index, hit, next_stop(), accent, cx)
                        })
                        .collect();
                    items.push(grid().children(cards).into_any_element());
                }
            }
            SearchChip::Songs => {
                for (index, song) in results.songs.iter().enumerate() {
                    items.push(self.song_row(index, song, None, list_focused, accent, cx));
                }
                if more == More::Loading {
                    items.push(row_skeletons(MORE_SKELETON_ROWS, "search-more-skeleton", cx));
                }
            }
            // The Library chip has its own view (`library_view`); it never reaches here.
            SearchChip::Library => {}
            SearchChip::Albums | SearchChip::Artists | SearchChip::Playlists
            | SearchChip::CommunityPlaylists => {
                let (kind, hits) = match chip {
                    SearchChip::Albums => ("album", &results.albums),
                    SearchChip::Artists => ("artist", &results.artists),
                    _ => ("playlist", &results.playlists),
                };
                let cards: Vec<Card> = hits
                    .iter()
                    .enumerate()
                    .map(|(index, hit)| self.hit_card(kind, index, hit, next_stop(), accent, cx))
                    .collect();
                items.push(grid().children(cards).into_any_element());
                if more == More::Loading {
                    items.push(card_skeletons("search-more-skeleton", cx));
                }
            }
        }
        if let More::Failed(_) = more {
            let model = self.model.clone();
            items.push(self.message(
                "Couldn't load more".into(),
                "Retry",
                accent,
                move |_, cx| model.update(cx, |m, cx| m.retry_more(cx)),
                cx,
            ));
        }
        items
    }

    /// "No results for “query”" with a Clear action.
    fn no_results(&self, query: &SharedString, accent: Accent, cx: &App) -> AnyElement {
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

    /// The "In your library" group: the library's best few matches of any kind, as rows, under
    /// a heading with a "Show all" that opens the Library chip. Song rows are numbered from
    /// `base` (after the service's songs on the page); the group's tab stops start at
    /// `first_stop`; `spaced` leaves room above it (it follows something).
    fn lead_group(
        &self,
        local: &LocalResults,
        spaced: bool,
        first_stop: usize,
        base: usize,
        list_focused: bool,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let page = cx.entity();
        let all = div()
            .debug_selector(|| "show-all-library".into())
            .child(
                TextButton::new(
                    "show-all-library",
                    &self.card_focus[first_stop],
                    "Show all",
                    accent,
                )
                .on_click(move |_, cx| {
                    page.update(cx, |p, cx| p.select_chip(SearchChip::Library, cx))
                }),
            )
            .into_any_element();
        let mut song_index = base;
        let mut column = div()
            .flex()
            .flex_col()
            .debug_selector(|| "library-group".into())
            .child(group_header(LIBRARY_GROUP, spaced, Some(all), cx));
        for (n, hit) in local.lead.iter().enumerate() {
            let stop = first_stop + 1 + n;
            column = column.child(match hit {
                LocalHit::Song(row) => {
                    let song = listed(row);
                    let element = self.song_row(
                        song_index,
                        &song,
                        Some("Song"),
                        list_focused,
                        accent,
                        cx,
                    );
                    song_index += 1;
                    element
                }
                LocalHit::Album(c) => self.library_row("album", n, &hit_of(c), stop, accent, cx),
                LocalHit::Playlist(c) => {
                    self.library_row("playlist", n, &hit_of(c), stop, accent, cx)
                }
                LocalHit::Artist(h) => self.library_row("artist", n, h, stop, accent, cx),
            });
        }
        column.into_any_element()
    }

    /// An album, playlist or artist of the library as a row (tab stop `stop`).
    fn library_row(
        &self,
        kind: &'static str,
        index: usize,
        hit: &Hit,
        stop: usize,
        accent: Accent,
        cx: &App,
    ) -> AnyElement {
        let thumb = match &hit.thumbnail {
            Some(url) => self.thumbs.read(cx).get(url),
            None => Thumb::Missing,
        };
        let label = match kind {
            "album" => "Album",
            "playlist" => "Playlist",
            _ => "Artist",
        };
        let detail = library_detail(label, hit.subtitle.as_deref());
        let (target, on_open) = (target_of(kind, hit), self.on_open.clone());
        LibraryRow::new(
            format!("library-row-{index}"),
            hit.title.clone(),
            detail,
            thumb,
            &self.card_focus[stop],
            accent,
            move |window, cx| on_open(target.clone(), window, cx),
        )
        .circular(kind == "artist")
        .menu(Subject::of_target(&target_of(kind, hit)))
        .into_any_element()
    }

    /// The Library chip's list: everything the library holds for the query, grouped Songs,
    /// Albums, Playlists, Artists, at most [`CHIP_CAP`] of each.
    fn library_view(
        &self,
        local: &LocalResults,
        list_focused: bool,
        accent: Accent,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut items: Vec<AnyElement> = Vec::new();
        let mut stop = 0;
        if !local.songs.is_empty() {
            items.push(group_header("Songs", false, None, cx));
            for (index, row) in local.songs.iter().take(CHIP_CAP).enumerate() {
                items.push(self.song_row(index, &listed(row), None, list_focused, accent, cx));
            }
        }
        let albums: Vec<Hit> = local.albums.iter().take(CHIP_CAP).map(hit_of).collect();
        let playlists: Vec<Hit> = local.playlists.iter().take(CHIP_CAP).map(hit_of).collect();
        let artists: Vec<Hit> = local.artists.iter().take(CHIP_CAP).cloned().collect();
        for (title, kind, hits) in [
            ("Albums", "album", &albums),
            ("Playlists", "playlist", &playlists),
            ("Artists", "artist", &artists),
        ] {
            if hits.is_empty() {
                continue;
            }
            items.push(group_header(title, !items.is_empty(), None, cx));
            let cards: Vec<Card> = hits
                .iter()
                .enumerate()
                .map(|(index, hit)| {
                    stop += 1;
                    self.hit_card(kind, index, hit, stop - 1, accent, cx)
                })
                .collect();
            items.push(grid().children(cards).into_any_element());
        }
        items
    }

    /// One album, artist or playlist card (the focus stop is `stop` of the card handles).
    fn hit_card(
        &self,
        kind: &'static str,
        index: usize,
        hit: &Hit,
        stop: usize,
        accent: Accent,
        cx: &App,
    ) -> Card {
        let thumb = match &hit.thumbnail {
            Some(url) => self.thumbs.read(cx).get(url),
            None => Thumb::Missing,
        };
        let (target, on_open) = (target_of(kind, hit), self.on_open.clone());
        Card::new(
            format!("search-{kind}-{index}"),
            hit.title.clone(),
            hit.subtitle.clone().map(Into::into),
            thumb,
            &self.card_focus[stop],
            accent,
            move |window, cx| on_open(target.clone(), window, cx),
        )
        .circular(kind == "artist")
        .map_menu(Subject::of_target(&target_of(kind, hit)))
    }

    /// The Top result card for `top`. Play does what the kind implies: a song plays, an album
    /// or playlist plays through, an artist plays the songs listed beside the card.
    #[allow(clippy::too_many_arguments)]
    fn top_card(
        &self,
        top: &TopResult,
        results: &SearchResults,
        songs: &[&ListedTrack],
        card_stop: usize,
        play_stop: usize,
        accent: Accent,
        cx: &App,
    ) -> TopCard {
        let thumbs = self.thumbs.read(cx);
        let thumb_of = |url: &Option<String>| match url {
            Some(url) => thumbs.get(url),
            None => Thumb::Missing,
        };
        let detail = |kind: &str, rest: Option<&str>| -> SharedString {
            match rest.filter(|r| !r.trim().is_empty()) {
                Some(rest) => format!("{kind} \u{2022} {rest}").into(),
                None => kind.to_owned().into(),
            }
        };
        let (card_focus, play_focus) = (&self.card_focus[card_stop], &self.card_focus[play_stop]);
        match top {
            TopResult::Song(song) => {
                let kind = if results.video_ids.contains(song.track.video_id.as_str()) {
                    "Video"
                } else {
                    "Song"
                };
                let artists = artists_line(song);
                let play = {
                    let (player, track, thumb) =
                        (self.player.clone(), song.track.clone(), song.thumbnail.clone());
                    Rc::new(move |_: &mut Window, cx: &mut App| {
                        player.update(cx, |m, cx| {
                            m.hint_art(track.video_id.as_str(), thumb.as_deref());
                            m.play_now(track.clone(), cx)
                        })
                    })
                };
                let (open, play_button) = (play.clone(), play);
                let intent = {
                    let (player, track) = (self.player.clone(), song.track.clone());
                    move |cx: &mut App| player.update(cx, |m, cx| m.hint_play(&track, cx))
                };
                TopCard::new(
                    song.track.title.clone(),
                    detail(kind, Some(&artists)),
                    thumb_of(&song.thumbnail),
                    card_focus,
                    play_focus,
                    accent,
                    move |w, cx| open(w, cx),
                    move |w, cx| play_button(w, cx),
                )
                .on_intent(intent)
                .menu(Some(Subject::track(song.track.clone(), song.thumbnail.clone())))
            }
            TopResult::Album(hit) | TopResult::Playlist(hit) => {
                let kind = if matches!(top, TopResult::Album(_)) {
                    "album"
                } else {
                    "playlist"
                };
                let target = target_of(kind, hit);
                let on_open = self.on_open.clone();
                let open_target = target.clone();
                let subject = Subject::of_target(&target);
                let play_subject = subject.clone();
                TopCard::new(
                    hit.title.clone(),
                    detail(
                        if kind == "album" { "Album" } else { "Playlist" },
                        hit.subtitle.as_deref(),
                    ),
                    thumb_of(&hit.thumbnail),
                    card_focus,
                    play_focus,
                    accent,
                    move |w, cx| on_open(open_target.clone(), w, cx),
                    move |w, cx| {
                        if let Some(subject) = play_subject.clone() {
                            track_menu::run_now(Command::Play, subject, w, cx);
                        }
                    },
                )
                .menu(subject)
            }
            TopResult::Artist(hit) => {
                let target = target_of("artist", hit);
                let on_open = self.on_open.clone();
                let tracks: Vec<_> = songs.iter().map(|s| s.track.clone()).collect();
                let source = match &target {
                    Target::Artist(a) => Some(queue_source::of_artist(a)),
                    _ => None,
                };
                let player = self.player.clone();
                TopCard::new(
                    hit.title.clone(),
                    detail("Artist", hit.subtitle.as_deref()),
                    thumb_of(&hit.thumbnail),
                    card_focus,
                    play_focus,
                    accent,
                    move |w, cx| on_open(target.clone(), w, cx),
                    move |_, cx| {
                        let (tracks, source) = (tracks.clone(), source.clone());
                        player.update(cx, |m, cx| {
                            m.play_tracks_from(tracks, Some(0), None, source, cx)
                        })
                    },
                )
                .circular(true)
            }
        }
    }

    /// One song. `kind` leads its second line ("Song \u{2022} Artist") where the row sits among
    /// other kinds.
    fn song_row(
        &self,
        index: usize,
        song: &ListedTrack,
        kind: Option<&'static str>,
        list_focused: bool,
        accent: Accent,
        cx: &App,
    ) -> AnyElement {
        let thumb = match &song.thumbnail {
            Some(url) => self.thumbs.read(cx).get(url),
            None => Thumb::Missing,
        };
        SongRow {
            index,
            title: song.track.title.clone().into(),
            artists: match kind {
                Some(kind) => {
                    let artists = artists_line(song);
                    if artists.is_empty() {
                        kind.into()
                    } else {
                        format!("{kind} \u{2022} {artists}").into()
                    }
                }
                None => artists_line(song),
            },
            duration: song
                .track
                .duration
                .map(|d| format_time(d.as_millis() as u64)),
            thumb,
            selected: list_focused && self.selected == Some(index),
            picked: self.selection.contains(&song.track.video_id.as_str().to_owned()),
            accent,
            liked: self.writes.read(cx).is_liked(song.track.video_id.as_str()),
            menu: match &self.bulk_menu {
                // A picked row speaks for every picked row.
                Some(bulk) if self.selection.contains(&song.track.video_id.as_str().to_owned()) => {
                    bulk.clone()
                }
                _ => Subject::track(song.track.clone(), song.thumbnail.clone()),
            },
            on_select: {
                let this = self.this.clone();
                Box::new(move |how, window, cx| {
                    let _ = this.update(cx, |p, cx| p.row_clicked(index, how, window, cx));
                })
            },
            on_like: {
                let (writes, video) = (self.writes.clone(), song.track.video_id.as_str().to_owned());
                Box::new(move |_, cx| writes.update(cx, |w, cx| w.toggle_like(&video, cx)))
            },
            on_playlist: {
                let (picker, track) = (self.picker.clone(), song.track.clone());
                Box::new(move |window, cx| {
                    picker.update(cx, |p, cx| {
                        p.open_at(
                            track.video_id.as_str(),
                            track.title.clone(),
                            &playlist_anchor(index),
                            window,
                            cx,
                        )
                    })
                })
            },
            on_play: {
                let (player, track, thumb) =
                    (self.player.clone(), song.track.clone(), song.thumbnail.clone());
                Box::new(move |_, cx| {
                    player.update(cx, |m, cx| {
                        m.hint_art(track.video_id.as_str(), thumb.as_deref());
                        m.play_now(track.clone(), cx)
                    })
                })
            },
            on_queue: {
                let (player, track, thumb) =
                    (self.player.clone(), song.track.clone(), song.thumbnail.clone());
                Box::new(move |_, cx| {
                    player.update(cx, |m, cx| {
                        m.hint_art(track.video_id.as_str(), thumb.as_deref());
                        m.add_to_queue(track.clone(), cx)
                    })
                })
            },
            on_intent: {
                let (player, track) = (self.player.clone(), song.track.clone());
                Box::new(move |_, cx| player.update(cx, |m, cx| m.hint_play(&track, cx)))
            },
        }
        .into_any_element()
    }
}

/// A group's heading with, on the right, its "Show all".
fn group_header(
    title: &'static str,
    spaced: bool,
    show_all: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(layout::MIN_HIT_TARGET))
        .when(spaced, |d| d.mt(px(spacing::S6)))
        .mb(px(spacing::S2))
        .child(
            style::text_style(div(), typography::SECTION, cx)
                .text_color(color::text_primary())
                .child(title),
        )
        .children(show_all)
        .into_any_element()
}

/// The page a search card opens.
fn target_of(kind: &str, hit: &Hit) -> Target {
    let collection = |kind| {
        Target::Collection(Collection {
            kind,
            id: hit.id.clone(),
            title: hit.title.clone(),
            subtitle: hit.subtitle.clone(),
            thumbnail: hit.thumbnail.clone(),
        })
    };
    match kind {
        "album" => collection(CollectionKind::Album),
        "playlist" => collection(CollectionKind::Playlist),
        _ => Target::Artist(ArtistTarget {
            id: hit.id.clone(),
            name: hit.title.clone(),
            thumbnail: hit.thumbnail.clone(),
        }),
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

/// Skeleton blocks at the final size of what the chip on show lists, so nothing shifts when
/// results arrive.
fn skeletons(chip: SearchChip, cx: &App) -> AnyElement {
    match chip {
        SearchChip::All | SearchChip::Songs => row_skeletons(SKELETON_ROWS, "search-skeleton", cx),
        _ => card_skeletons("search-skeleton", cx),
    }
}

fn skeleton_block(w: gpui::Length, h: f32) -> gpui::Div {
    div()
        .w(w)
        .h(px(h))
        .rounded(px(radius::SMALL))
        .bg(color::skeleton())
}

fn row_skeletons(rows: usize, id: &'static str, cx: &App) -> AnyElement {
    let column = div().flex().flex_col().children((0..rows).map(|_| {
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
                    .child(skeleton_block(gpui::relative(0.4).into(), 12.0))
                    .child(skeleton_block(gpui::relative(0.25).into(), 10.0)),
            )
    }));
    skeleton::pulse(column, id, !cx.reduce_motion())
}

/// A row of card-sized blocks: the artwork square and two lines.
fn card_skeletons(id: &'static str, cx: &App) -> AnyElement {
    let row = grid().children((0..SKELETON_CARDS).map(|_| {
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
                    .rounded(px(radius::MEDIUM))
                    .bg(color::skeleton()),
            )
            .child(skeleton_block(gpui::relative(0.7).into(), 12.0))
            .child(skeleton_block(gpui::relative(0.45).into(), 10.0))
    }));
    skeleton::pulse(row, id, !cx.reduce_motion())
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
    /// The song is one of the picked ones (drawn tinted).
    picked: bool,
    accent: Accent,
    liked: bool,
    /// What the More button and a right-click offer.
    menu: Subject,
    /// A click on the row: Ctrl and Shift pick, a plain click clears the picks.
    on_select: Box<dyn Fn(Click, &mut Window, &mut App) + 'static>,
    on_like: RowAction,
    on_playlist: RowAction,
    on_play: RowAction,
    on_queue: RowAction,
    /// The pointer is resting on the row: the track is likely to be played next.
    on_intent: RowAction,
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
        let play_row = play.clone();
        let play_info = play;
        let queue = self.on_queue;
        let intent = self.on_intent;
        let (on_like, on_playlist) = (self.on_like, self.on_playlist);
        let on_select = self.on_select;
        let menu = self.menu;
        let liked = self.liked;
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
            .on_click(move |event, window, cx| {
                // Ctrl and Shift pick the row; only a plain click plays it.
                if !Click::of_event(event).is_selecting() {
                    play_art(window, cx)
                }
            })
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
            .on_click(move |event, window, cx| {
                if !Click::of_event(event).is_selecting() {
                    play_info(window, cx)
                }
            })
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

        // Like and add to playlist: their room is always there too; the heart stays once liked.
        let slot = || div().relative().size(px(ACTION_SIZE)).flex_none();
        let like = slot()
            .debug_selector(move || format!("song-like-{id}"))
            .when(active || liked, |d| {
                d.child(heart_action(("song-like", id), liked, move |w, cx| {
                    on_like(w, cx)
                }))
            });
        let playlist = slot()
            .debug_selector(move || format!("song-playlist-{id}"))
            .child(anchor::probe(playlist_anchor(id)))
            .when(active, |d| {
                d.child(playlist_action(("song-playlist", id), move |w, cx| {
                    on_playlist(w, cx)
                }))
            });

        let more = more_slot(format!("song-more-{id}"), active, menu.clone());

        let row_fade = fade.clone();
        let row = div()
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
            .aria_selected(self.selected || self.picked)
            .on_click(move |event, window, cx| on_select(Click::of_event(event), window, cx))
            // The art and the text play on click; assistive technology addresses the whole row.
            .on_a11y_action(gpui::AccessibleAction::Click, move |_, window, cx| {
                play_row(window, cx)
            })
            .bg(if self.picked {
                selection::row_tint(self.accent)
            } else if self.selected {
                color::state_selected()
            } else {
                color::state_hover().opacity(level)
            })
            .on_hover(move |hovered, window, cx| {
                row_fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animate);
                    cx.notify();
                });
                if *hovered {
                    intent(window, cx);
                }
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
            .child(like)
            .child(playlist)
            .child(add)
            .child(more);
        track_menu::on_context_menu(row, menu)
    }
}


/// The second line of a library row: its kind, then what the library says about it. The
/// library's own albums already start with their kind ("Album \u{2022} Daft Punk \u{2022} 2005",
/// "EP \u{2022} ..."), which must not be put in front a second time.
fn library_detail(label: &str, subtitle: Option<&str>) -> SharedString {
    const KINDS: [&str; 6] = ["Album", "EP", "Single", "Playlist", "Artist", "Song"];
    match subtitle.map(str::trim).filter(|s| !s.is_empty()) {
        Some(rest) => {
            let first = rest.split('\u{2022}').next().unwrap_or("").trim();
            if KINDS.contains(&first) {
                rest.to_owned().into()
            } else {
                format!("{label} \u{2022} {rest}").into()
            }
        }
        None => label.to_owned().into(),
    }
}

#[cfg(test)]
mod library_detail_tests {
    use super::library_detail;

    #[test]
    fn a_kind_is_never_said_twice() {
        assert_eq!(
            library_detail("Album", Some("Album \u{2022} Daft Punk \u{2022} 2005")),
            "Album \u{2022} Daft Punk \u{2022} 2005"
        );
        assert_eq!(
            library_detail("Album", Some("EP \u{2022} Ludo \u{2022} 2005")),
            "EP \u{2022} Ludo \u{2022} 2005"
        );
        assert_eq!(
            library_detail("Album", Some("Daft Punk \u{2022} 2013")),
            "Album \u{2022} Daft Punk \u{2022} 2013"
        );
        assert_eq!(
            library_detail("Playlist", Some("Zach \u{2022} 23 tracks")),
            "Playlist \u{2022} Zach \u{2022} 23 tracks"
        );
        assert_eq!(library_detail("Artist", Some("  ")), "Artist");
        assert_eq!(library_detail("Artist", None), "Artist");
    }
}
