//! The Library overview's tab row and sort control, and the Artists and History tabs.
//!
//! The tab row is the nav's selected-tint pattern. The sort control is a compact text button at
//! the row's right end that opens an anchored popover in the track menu's style (bg.overlay,
//! 220px, 32px rows, no scrim); the choice is remembered per tab. Artists are the shared round
//! cards in the shared grid; History is the song rows under YouTube Music's own date headings.

use apollo_provider::Hit;
use gpui::{deferred, MouseButton};

use super::*;
use crate::library::browse::history_len;
use crate::stats::report::Period;
use crate::ui::card::{self, Card};
use crate::ui::track_menu::{ITEM_HEIGHT, MENU_WIDTH};

actions!(
    library_sort,
    [SortUp, SortDown, SortChoose, SortClose, SortStay]
);

/// Key context of the sort popover.
pub const SORT_CONTEXT: &str = "LibrarySort";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SortUp, Some(SORT_CONTEXT)),
        KeyBinding::new("down", SortDown, Some(SORT_CONTEXT)),
        KeyBinding::new("enter", SortChoose, Some(SORT_CONTEXT)),
        KeyBinding::new("space", SortChoose, Some(SORT_CONTEXT)),
        KeyBinding::new("escape", SortClose, Some(SORT_CONTEXT)),
        // Focus stays in the popover while it is open.
        KeyBinding::new("tab", SortStay, Some(SORT_CONTEXT)),
        KeyBinding::new("shift-tab", SortStay, Some(SORT_CONTEXT)),
    ]);
}

/// Cards drawn as placeholders while the artists load.
const SKELETON_CARDS: usize = 8;

impl LibraryView {
    fn tab_focus(&self, tab: Tab) -> &FocusHandle {
        match tab {
            Tab::Playlists => &self.focus.tab_playlists,
            Tab::Albums => &self.focus.tab_albums,
            Tab::Artists => &self.focus.tab_artists,
            Tab::History => &self.focus.tab_history,
            Tab::Stats => &self.focus.tab_stats,
        }
    }

    // ---- tab row ---------------------------------------------------------------------------

    /// The tabs on the left and, when the tab has orders to choose from, the sort control on
    /// the right.
    pub(super) fn tab_row(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.model.read(cx).tab();
        let buttons: Vec<AnyElement> = Tab::ALL
            .into_iter()
            .map(|tab| self.tab_button(tab, tab == current, accent, window, cx))
            .collect();
        let sort = (current.sorts().len() > 1 || current == Tab::Stats)
            .then(|| self.sort_control(current, accent, window, cx));
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .id("library-tabs")
                    .role(gpui::Role::TabList)
                    .aria_label("Library")
                    .flex()
                    .flex_row()
                    .gap(px(spacing::S2))
                    .children(buttons),
            )
            .children(sort)
            .into_any_element()
    }

    fn tab_button(
        &mut self,
        tab: Tab,
        active: bool,
        accent: Accent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = self.tab_focus(tab).clone();
        let ring = keyboard_focused(&focus, window);
        let ring_color = color::accent(accent);
        let label = tab.label();
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
        .debug_selector(move || format!("library-tab-{}", label.to_lowercase()))
        .relative()
        .flex()
        .items_center()
        .h(px(layout::MIN_HIT_TARGET))
        .px(px(spacing::S4))
        .rounded(px(radius::SMALL))
        .cursor_pointer()
        .track_focus(&focus)
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
            this.set_tab(tab, cx);
        }))
        .child(label)
        .when(ring, move |d| {
            d.child(focus_ring(ring_color, radius::SMALL))
        })
        .into_any_element()
    }

    // ---- sort ------------------------------------------------------------------------------

    /// What the popover at the right end of the tab row offers: the labels of its options and
    /// the one in use. The Stats tab chooses a period; the other tabs choose an order.
    fn sort_choices(&self, tab: Tab, cx: &App) -> (Vec<&'static str>, usize) {
        let model = self.model.read(cx);
        if tab == Tab::Stats {
            let current = model.stats_period();
            let at = Period::ALL.iter().position(|p| *p == current).unwrap_or(0);
            return (Period::ALL.iter().map(|p| p.label()).collect(), at);
        }
        let current = model.sort(tab);
        let at = tab.sorts().iter().position(|k| *k == current).unwrap_or(0);
        (tab.sorts().iter().map(|k| k.label()).collect(), at)
    }

    fn sort_control(
        &mut self,
        tab: Tab,
        accent: Accent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (options, current_ix) = self.sort_choices(tab, cx);
        let current_label = options.get(current_ix).copied().unwrap_or_default();
        let focus = self.focus.sort.clone();
        let ring = keyboard_focused(&focus, window);
        let ring_color = color::accent(accent);
        let button = style::text_style(div(), typography::BODY, cx)
            .id("library-sort")
            .debug_selector(|| "library-sort".into())
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S1))
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .whitespace_nowrap()
            .cursor_pointer()
            .track_focus(&focus)
            .role(gpui::Role::Button)
            .aria_label(format!(
                "{}: {}",
                if tab == Tab::Stats { "Period" } else { "Sort" },
                current_label
            ))
            .text_color(color::text_secondary())
            .hover(|s| s.text_color(color::text_primary()))
            .on_click(cx.listener(|this, event: &gpui::ClickEvent, window, cx| {
                if let gpui::ClickEvent::Keyboard(k) = event {
                    if k.button == gpui::KeyboardButton::Space {
                        return;
                    }
                }
                this.toggle_sort(window, cx);
            }))
            .child(current_label)
            .child(
                gpui::svg()
                    .path(Icon::ChevronDown.path())
                    .size(px(16.0))
                    .flex_none()
                    .text_color(color::text_secondary()),
            )
            .when(ring, move |d| {
                d.child(focus_ring(ring_color, radius::SMALL))
            });

        let open = self.sort_open;
        let panel = open.then(|| self.sort_menu(tab, cx));
        div()
            .relative()
            .flex_none()
            .child(button)
            .when_some(panel, |d, panel| {
                // A transparent layer, far larger than the window, catches the click that
                // closes the popover (no scrim); the popover paints above the list below.
                let backdrop = div()
                    .id("library-sort-backdrop")
                    .absolute()
                    .top(px(-3000.0))
                    .left(px(-3000.0))
                    .w(px(6000.0))
                    .h(px(6000.0))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| this.close_sort(window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, window, cx| this.close_sort(window, cx)),
                    );
                d.child(deferred(backdrop))
                    .child(deferred(panel).with_priority(1))
            })
            .into_any_element()
    }

    fn sort_menu(&mut self, tab: Tab, cx: &mut Context<Self>) -> AnyElement {
        let (options, current_ix) = self.sort_choices(tab, cx);
        let cursor = self.sort_cursor.min(options.len().saturating_sub(1));
        let items: Vec<AnyElement> = options
            .iter()
            .enumerate()
            .map(|(ix, label)| self.sort_item(ix, label, ix == current_ix, ix == cursor, cx))
            .collect();
        div()
            .id("library-sort-menu")
            .debug_selector(|| "library-sort-menu".into())
            .key_context(SORT_CONTEXT)
            .track_focus(&self.focus.sort_menu)
            .role(gpui::Role::Menu)
            .absolute()
            .top(px(layout::MIN_HIT_TARGET + spacing::S1))
            .right_0()
            .flex()
            .flex_col()
            .w(px(MENU_WIDTH))
            .p(px(spacing::S1))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &SortUp, _, cx| this.step_sort(-1, cx)))
            .on_action(cx.listener(|this, _: &SortDown, _, cx| this.step_sort(1, cx)))
            .on_action(cx.listener(|this, _: &SortChoose, window, cx| {
                let at = this.sort_cursor;
                this.choose_sort(at, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SortClose, window, cx| this.close_sort(window, cx)))
            .on_action(cx.listener(|_, _: &SortStay, _, _| {}))
            .children(items)
            .into_any_element()
    }

    fn sort_item(
        &mut self,
        ix: usize,
        label: &'static str,
        chosen: bool,
        cursor: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        style::text_style(div(), typography::BODY, cx)
            .id(("library-sort-item", ix))
            .debug_selector(move || format!("library-sort-item-{ix}"))
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S3))
            .h(px(ITEM_HEIGHT))
            .px(px(spacing::S3))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .role(gpui::Role::MenuItem)
            .aria_label(label)
            .text_color(color::text_primary())
            .bg(if cursor {
                color::state_hover()
            } else {
                gpui::transparent_black()
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.sort_cursor != ix {
                    this.sort_cursor = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, window, cx| {
                this.choose_sort(ix, window, cx)
            }))
            // The chosen order carries a check, like a playlist that already holds the track.
            .child(div().size(px(16.0)).flex_none().when(chosen, |d| {
                d.child(
                    gpui::svg()
                        .path(Icon::Checkmark.path())
                        .size(px(16.0))
                        .text_color(color::text_secondary()),
                )
            }))
            .child(label)
            .into_any_element()
    }

    pub(super) fn toggle_sort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sort_open {
            self.close_sort(window, cx);
            return;
        }
        let tab = self.model.read(cx).tab();
        self.sort_cursor = self.sort_choices(tab, cx).1;
        self.sort_open = true;
        window.focus(&self.focus.sort_menu, cx);
        cx.notify();
    }

    pub(super) fn close_sort(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sort_open {
            self.sort_open = false;
            window.focus(&self.focus.sort, cx);
            cx.notify();
        }
    }

    fn step_sort(&mut self, delta: isize, cx: &mut Context<Self>) {
        let tab = self.model.read(cx).tab();
        let len = self.sort_choices(tab, cx).0.len() as isize;
        if len > 0 {
            self.sort_cursor = (self.sort_cursor as isize + delta).clamp(0, len - 1) as usize;
            cx.notify();
        }
    }

    fn choose_sort(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.model.read(cx).tab();
        if tab == Tab::Stats {
            let Some(period) = Period::ALL.get(ix).copied() else {
                return;
            };
            self.model.update(cx, |m, cx| m.set_stats_period(period, cx));
            self.selected = 0;
            self.hover.clear();
            self.stats_scroll.scroll_to_item(0);
            self.close_sort(window, cx);
            return;
        }
        let Some(key) = tab.sorts().get(ix).copied() else {
            return;
        };
        self.model.update(cx, |m, cx| m.set_sort(tab, key, cx));
        self.selected = 0;
        self.hover.clear();
        // A new order starts at the top.
        match tab {
            Tab::Artists => self.artists_scroll.scroll_to_item(0),
            _ => self
                .scroll_handle(cx)
                .scroll_to_item(0, gpui::ScrollStrategy::Top),
        }
        self.close_sort(window, cx);
    }

    // ---- artists ---------------------------------------------------------------------------

    /// Placeholder cards at the real card's size, so nothing shifts when the artists arrive.
    fn skeleton_cards(animations: bool) -> AnyElement {
        let block = |w: f32, h: f32| {
            div()
                .h(px(h))
                .w(px(w))
                .rounded(px(radius::SMALL))
                .bg(color::skeleton())
        };
        let cards = (0..SKELETON_CARDS).map(|_| {
            div()
                .w(px(card::WIDTH))
                .p(px(card::INSET))
                .flex()
                .flex_col()
                .gap(px(spacing::S2))
                .child(
                    div()
                        .size(px(card::ART))
                        .rounded_full()
                        .bg(color::skeleton()),
                )
                .child(block(110.0, 12.0))
                .child(block(70.0, 10.0))
        });
        skeleton::pulse(
            card::grid().children(cards),
            "library-artists-skeleton",
            animations,
        )
    }

    pub(super) fn render_artists_tab(
        &mut self,
        accent: Accent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        enum S {
            Pending,
            Failed(LoadFailure),
            Empty,
            Ready,
        }
        let state = match self.model.read(cx).artists() {
            Load::Idle | Load::Loading => S::Pending,
            Load::Failed(f) => S::Failed(*f),
            Load::Ready(list) if list.is_empty() => S::Empty,
            Load::Ready(_) => S::Ready,
        };
        match state {
            S::Pending => Self::skeleton_cards(!cx.reduce_motion()),
            S::Failed(failure) => {
                let model = self.model.clone();
                self.notice(
                    Self::failure_line(failure, "your artists"),
                    true,
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.reload_artists(cx)),
                    cx,
                )
            }
            S::Empty => self.search_notice("No artists yet", accent, cx),
            S::Ready => {
                let hits: Vec<Hit> = self.model.read(cx).artist_list().to_vec();
                while self.card_focus.len() < hits.len() {
                    self.card_focus
                        .push(cx.focus_handle().tab_index(10).tab_stop(true));
                }
                let this = cx.entity();
                let mut cards = Vec::with_capacity(hits.len());
                for (ix, hit) in hits.iter().enumerate() {
                    if let Some(url) = hit.thumbnail.as_deref() {
                        if !self.model.read(cx).art_requested(url, ArtSize::Card) {
                            let url = url.to_owned();
                            self.model
                                .update(cx, |m, cx| m.want_art(&url, ArtSize::Card, cx));
                        }
                    }
                    let thumb = self
                        .model
                        .read(cx)
                        .thumb(hit.thumbnail.as_deref(), ArtSize::Card);
                    let (this, target) = (
                        this.clone(),
                        ArtistTarget {
                            id: hit.id.clone(),
                            name: hit.title.clone(),
                            thumbnail: hit.thumbnail.clone(),
                        },
                    );
                    cards.push(
                        Card::new(
                            format!("library-artist-{ix}"),
                            hit.title.clone(),
                            hit.subtitle.clone().map(Into::into),
                            thumb,
                            &self.card_focus[ix],
                            accent,
                            move |window, cx| {
                                this.update(cx, |t, cx| t.open_artist(target.clone(), window, cx))
                            },
                        )
                        .circular(true),
                    );
                }
                div()
                    .id("library-artists")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.artists_scroll)
                    .pb(px(spacing::S6))
                    .child(card::grid().children(cards))
                    .into_any_element()
            }
        }
    }

    // ---- history ---------------------------------------------------------------------------

    pub(super) fn render_history_tab(
        &mut self,
        accent: Accent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        enum S {
            Pending,
            Failed(LoadFailure),
            Empty,
            Ready(Vec<(String, usize)>),
        }
        let state = match self.model.read(cx).history() {
            Load::Idle | Load::Loading => S::Pending,
            Load::Failed(f) => S::Failed(*f),
            Load::Ready(groups) if history_len(groups) == 0 => S::Empty,
            Load::Ready(groups) => S::Ready(
                groups
                    .iter()
                    .map(|g| (g.title.clone(), g.rows.len()))
                    .collect(),
            ),
        };
        match state {
            S::Pending => {
                Self::skeleton_rows(SKELETON_ROWS, TRACK_ROW, TRACK_ART, !cx.reduce_motion())
            }
            S::Failed(failure) => {
                let model = self.model.clone();
                self.notice(
                    Self::failure_line(failure, "your history"),
                    true,
                    "Retry",
                    accent,
                    move |_, cx| model.update(cx, |m, cx| m.reload_history(cx)),
                    cx,
                )
            }
            S::Empty => self.search_notice("No history yet", accent, cx),
            S::Ready(groups) => {
                let animations = !cx.reduce_motion();
                let list_focused = keyboard_focused(&self.focus.list, window);
                self.hover.prune();
                let mut moving = false;
                // One heading per bucket, then its rows: the same children the keyboard scrolls
                // to (see `history_child`).
                let mut children: Vec<AnyElement> = Vec::new();
                let mut ix = 0;
                for (g, (title, rows)) in groups.into_iter().enumerate() {
                    children.push(
                        style::text_style(div(), typography::BODY_STRONG, cx)
                            .id(("history-heading", g))
                            .flex_none()
                            .px(px(spacing::S2))
                            .mt(px(if g == 0 { 0.0 } else { spacing::S4 }))
                            .mb(px(spacing::S1))
                            .text_color(color::text_secondary())
                            .child(title)
                            .into_any_element(),
                    );
                    for _ in 0..rows {
                        let frame = RowFrame {
                            ix,
                            real: ix,
                            level: self.hover.level(ix, &mut moving),
                            selected: list_focused && ix == self.selected,
                            accent,
                            animations,
                        };
                        children.push(self.track_row(frame, window, cx));
                        ix += 1;
                    }
                }
                if moving {
                    window.request_animation_frame();
                }
                self.rendered = 0..ix;
                let list = div()
                    .id("library-history")
                    .key_context(LIST_CONTEXT)
                    .track_focus(&self.focus.list)
                    .role(gpui::Role::List)
                    .aria_label("History")
                    .size_full()
                    .min_h_0();
                Self::list_actions(list, cx)
                    .child(
                        div()
                            .id("history-scroll")
                            .size_full()
                            .flex()
                            .flex_col()
                            .overflow_y_scroll()
                            .track_scroll(&self.history_scroll)
                            .pb(px(spacing::S6))
                            .children(children),
                    )
                    .into_any_element()
            }
        }
    }
}
