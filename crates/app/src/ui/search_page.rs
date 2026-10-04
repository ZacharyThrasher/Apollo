//! The Search page: a field, then results grouped as songs, albums, artists and playlists.
//!
//! Songs play or queue; album, artist and playlist cards open their pages. Keyboard:
//! Ctrl+K or Ctrl+F opens the page with the field focused; Down moves from the field into the
//! songs, Up and Down move between them, Enter plays, Ctrl+Enter queues, Escape goes back to the
//! field (and out of it when the field is empty). Cards are tab stops after the list; Enter opens.

use std::rc::Rc;
use std::time::Instant;

use apollo_domain::library::{Collection, CollectionKind};
use apollo_provider::{Hit, LinkTarget, ListedTrack, SearchResults};
use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, deferred, div, prelude::*, px, AnyElement, App, Context, Entity, FocusHandle,
    Focusable, IntoElement, ParentElement, Render, ScrollHandle, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::library::model::{ArtistTarget, Target};
use crate::library::writes::LibraryWrites;
use crate::motion::HoverFade;
use crate::player::{format_time, PlayerModel};
use crate::search::{
    as_link, failure_text, normalise, SearchEvent, SearchModel, SearchState, BAD_LINK_TEXT,
    HIDE_MUSIC_VIDEOS, SAVE_RECENT_SEARCHES,
};
use crate::settings::SettingsModel;
use crate::style::{self, color};
use crate::ui::anchor;
use crate::ui::button::{focus_ring, keyboard_focused, IconButton};
use crate::ui::card::{grid, heading, tile, Card};
use crate::ui::playlist_picker::PlaylistPicker;
use crate::ui::row_actions::{heart_action, more_slot, playlist_action, ACTION_SIZE};
use crate::ui::search_popover::{self, SearchPopover};
use crate::ui::track_menu::{self, Subject};
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
        PlaylistSelected,
        BackToField,
        PreviousSuggestion,
    ]
);

/// Key context of the results list.
pub const RESULTS_CONTEXT: &str = "SearchResults";

pub fn bind_keys(cx: &mut App) {
    use gpui::KeyBinding;
    cx.bind_keys([
        KeyBinding::new("down", FocusResults, Some(text_field::KEY_CONTEXT)),
        KeyBinding::new("up", PreviousSuggestion, Some(text_field::KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(RESULTS_CONTEXT)),
        KeyBinding::new("up", SelectPrevious, Some(RESULTS_CONTEXT)),
        KeyBinding::new("enter", PlaySelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("ctrl-enter", QueueSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("l", LikeSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("p", PlaylistSelected, Some(RESULTS_CONTEXT)),
        KeyBinding::new("escape", BackToField, Some(RESULTS_CONTEXT)),
    ]);
}

const ROW_HEIGHT: f32 = 56.0;
const ROW_ART: f32 = 40.0;
const SKELETON_ROWS: usize = 6;
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
    thumbs: Entity<ThumbStore>,
    field: Entity<TextField>,
    /// Tab stop of the results list (rows are not tab stops: Up and Down move between them).
    results_focus: FocusHandle,
    /// Tab stop of the message action (Retry, Clear).
    action_focus: FocusHandle,
    root_focus: FocusHandle,
    /// Tab stops of the cards, grown to the most cards any result had.
    card_focus: Vec<FocusHandle>,
    on_open: OpenTarget,
    scroll: ScrollHandle,
    /// The keyboard-selected song, shown only while the list holds focus.
    selected: Option<usize>,
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
                    this.active = None;
                    // Typing brings the popover back; a row picked a moment ago does not.
                    if this.picked.take().as_deref() != Some(text.as_str()) {
                        this.dismissed = false;
                    }
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
        cx.observe(&model, |this, _, cx| {
            this.selected = None;
            this.request_thumbs(cx);
            cx.notify();
        })
        .detach();
        cx.observe(&thumbs, |_, _, cx| cx.notify()).detach();
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        let mut page = Self {
            player,
            writes,
            picker,
            model,
            thumbs,
            field,
            results_focus,
            action_focus,
            root_focus,
            card_focus: Vec::new(),
            on_open,
            scroll: ScrollHandle::new(),
            selected: None,
            active: None,
            dismissed: false,
            field_was_focused: false,
            picked: None,
        };
        page.sync_options(&initial, cx);
        page
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
        if self.model.read(cx).results().is_some() {
            window.focus(&self.results_focus, cx);
        } else {
            text_field::focus(&self.field, window, cx);
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
        let next = self.selected.map_or(0, |i| (i + 1).min(count - 1));
        self.select(next, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected {
            Some(i) if i > 0 => self.select(i - 1, cx),
            _ => self.back_to_field(&BackToField, window, cx),
        }
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
            self.model.update(cx, |m, cx| m.remember(cx));
            self.player.update(cx, |m, cx| {
                m.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
                m.play_now(song.track, cx)
            });
        }
    }

    fn queue_selected(&mut self, _: &QueueSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.player.update(cx, |m, cx| {
                m.hint_art(song.track.video_id.as_str(), song.thumbnail.as_deref());
                m.add_to_queue(song.track, cx)
            });
        }
    }

    fn like_selected(&mut self, _: &LikeSelected, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(song) = self.selected.and_then(|i| self.songs(cx).get(i).cloned()) {
            self.writes
                .update(cx, |w, cx| w.toggle_like(song.track.video_id.as_str(), cx));
        }
    }

    fn playlist_selected(
        &mut self,
        _: &PlaylistSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        let query: SharedString = model.query().to_owned().into();
        let results = model.results().cloned();
        let confirmed_empty = model.is_confirmed_empty();

        let cards = results
            .as_ref()
            .map_or(0, |r| r.albums.len() + r.artists.len() + r.playlists.len());
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

        let body: AnyElement = match (state, results) {
            (SearchState::Idle, _) => div().into_any_element(),
            (SearchState::BadLink, _) => {
                let field = self.field.clone();
                self.message(
                    BAD_LINK_TEXT.into(),
                    "Clear",
                    accent,
                    move |window, cx| {
                        field.update(cx, |f, cx| f.set_text("", cx));
                        text_field::focus(&field, window, cx);
                    },
                    cx,
                )
            }
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
                    .on_action(cx.listener(Self::playlist_selected))
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
                        liked: self.writes.read(cx).is_liked(song.track.video_id.as_str()),
                        menu: Subject::track(song.track.clone(), song.thumbnail.clone()),
                        on_like: {
                            let (writes, video) =
                                (self.writes.clone(), song.track.video_id.as_str().to_owned());
                            Box::new(move |_, cx| {
                                writes.update(cx, |w, cx| w.toggle_like(&video, cx))
                            })
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
                            let (player, track, thumb) = (
                                self.player.clone(),
                                song.track.clone(),
                                song.thumbnail.clone(),
                            );
                            Box::new(move |_, cx| {
                                player.update(cx, |m, cx| {
                                    m.hint_art(track.video_id.as_str(), thumb.as_deref());
                                    m.play_now(track.clone(), cx)
                                })
                            })
                        },
                        on_queue: {
                            let (player, track, thumb) = (
                                self.player.clone(),
                                song.track.clone(),
                                song.thumbnail.clone(),
                            );
                            Box::new(move |_, cx| {
                                player.update(cx, |m, cx| {
                                    m.hint_art(track.video_id.as_str(), thumb.as_deref());
                                    m.add_to_queue(track.clone(), cx)
                                })
                            })
                        },
                        on_intent: {
                            let (player, track) = (self.player.clone(), song.track.clone());
                            Box::new(move |_, cx| {
                                player.update(cx, |m, cx| m.hint_play(&track, cx))
                            })
                        },
                    }
                    .into_any_element(),
                );
            }
        }
        let groups: [(&'static str, &'static str, &Vec<Hit>); 3] = [
            ("Albums", "album", &results.albums),
            ("Artists", "artist", &results.artists),
            ("Playlists", "playlist", &results.playlists),
        ];
        // Cards take their tab stops in reading order across the three groups.
        let mut stop = 0;
        for (title, kind, hits) in groups {
            if hits.is_empty() {
                continue;
            }
            children.push(heading(title, !children.is_empty(), cx));
            let cards: Vec<Card> = hits
                .iter()
                .enumerate()
                .map(|(index, hit)| {
                    let focus = &self.card_focus[stop];
                    stop += 1;
                    let (target, on_open) = (target_of(kind, hit), self.on_open.clone());
                    Card::new(
                        format!("search-{kind}-{index}"),
                        hit.title.clone(),
                        hit.subtitle.clone().map(Into::into),
                        thumb_of(&hit.thumbnail),
                        focus,
                        accent,
                        move |window, cx| on_open(target.clone(), window, cx),
                    )
                    .circular(kind == "artist")
                    .map_menu(Subject::of_target(&target_of(kind, hit)))
                })
                .collect();
            children.push(grid().children(cards).into_any_element());
        }
        div()
            .flex()
            .flex_col()
            .children(children)
            .into_any_element()
    }
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
    liked: bool,
    /// What the More button and a right-click offer.
    menu: Subject,
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
            .aria_selected(self.selected)
            // The art and the text play on click; assistive technology addresses the whole row.
            .on_a11y_action(gpui::AccessibleAction::Click, move |_, window, cx| {
                play_row(window, cx)
            })
            .bg(if self.selected {
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
