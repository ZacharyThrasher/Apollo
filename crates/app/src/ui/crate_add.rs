//! The Crate's "Add songs" box: a text field above the list (on the Crate page and in the docked
//! panel) and the popover under it. The state and the lookups are `crate_add`; this draws them.
//!
//! * Typing lists up to six songs under the field, styled like Search's suggestions popover
//!   (bg.overlay, 8px radius, 32px rows, 4px padding). Up / Down move through them, Enter (or a
//!   click) adds the one the keyboard is on, the field empties and keeps its focus for the next.
//!   Esc closes the popover; with nothing open another Esc hands the keyboard back to the list.
//! * Ctrl+V into the field, or onto the Crate, reads links and lines. One song link, or one
//!   line pasted onto the list, is added at once; a longer paste opens a review inside the popover:
//!   each line with its match and a checkbox (Space ticks, on by default), "Add 14", Cancel.
//!   Lines that match nothing say "No match" and are skipped. Enter adds, Esc cancels.
//!
//! Rows are picked on mouse-down so the field keeps its focus.

use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    deferred, div, prelude::*, px, svg, AnyElement, App, Context, Entity, FocusHandle, Focusable,
    MouseButton, ParentElement, Render, ScrollHandle, Styled, Window,
};

use crate::assets::Icon;
use crate::crate_add::{
    self, CrateAddModel, LineState, PasteStart, Review, Status, BUSY_TEXT, NO_RESULTS_TEXT,
};
use crate::search::failure_text;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::search_page::{FocusResults, PreviousSuggestion};
use crate::ui::text_field::{self, TextField, TextFieldEvent};

/// Width of the box on the Crate page (the docked panel's fills its width).
const PAGE_WIDTH: f32 = 420.0;
/// Height of a song row in the popover.
const ROW_HEIGHT: f32 = layout::MIN_HIT_TARGET;
/// Height of a row of the review list: the match, and the line it came from under it.
const REVIEW_ROW_HEIGHT: f32 = spacing::S12;
/// The review list scrolls past this height.
const REVIEW_MAX_HEIGHT: f32 = 360.0;
const ICON: f32 = 16.0;
const PAD: f32 = spacing::S1;

/// What Esc and Down hand the keyboard to when the box has nothing open: the Crate's list.
pub type Leave = Rc<dyn Fn(&mut Window, &mut App)>;

pub struct CrateAddBox {
    model: Entity<CrateAddModel>,
    field: Option<Entity<TextField>>,
    /// The field's tab slot (the page's and the dock's differ).
    tab_index: isize,
    docked: bool,
    accent: Accent,
    /// Esc closed the popover; typing or taking focus again brings it back.
    dismissed: bool,
    field_was_focused: bool,
    review_scroll: ScrollHandle,
    on_leave: Leave,
}

impl CrateAddBox {
    pub fn new(
        model: Entity<CrateAddModel>,
        tab_index: isize,
        docked: bool,
        on_leave: Leave,
        cx: &mut Context<Self>,
    ) -> Self {
        // The field follows what the model holds: an add (or leaving the page) empties it.
        cx.observe(&model, |this, model, cx| {
            let typed = model.read(cx).query().to_owned();
            if let Some(field) = this.field.clone() {
                let shown = crate::search::normalise(field.read(cx).text());
                if shown != typed {
                    field.update(cx, |f, cx| f.set_text(&typed, cx));
                }
            }
            cx.notify();
        })
        .detach();
        Self {
            model,
            field: None,
            tab_index,
            docked,
            accent: Accent::fallback(),
            dismissed: false,
            field_was_focused: false,
            review_scroll: ScrollHandle::new(),
            on_leave,
        }
    }

    pub fn set_accent(&mut self, accent: Accent) {
        self.accent = accent;
    }

    pub fn model(&self) -> &Entity<CrateAddModel> {
        &self.model
    }

    /// The field, made on first use (it needs the window to listen for its keys).
    fn ensure_field(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<TextField> {
        if let Some(field) = self.field.clone() {
            return field;
        }
        let focus = cx.focus_handle().tab_index(self.tab_index).tab_stop(true);
        let clear = cx.focus_handle().tab_index(self.tab_index).tab_stop(true);
        let hook: text_field::PasteHook = {
            let this = cx.weak_entity();
            Rc::new(move |text, window, cx| {
                this.update(cx, |b, cx| b.paste(text, true, window, cx) != PasteStart::Typed)
                    .unwrap_or(false)
            })
        };
        let field = cx.new(|_| {
            TextField::new("Add songs", focus, clear)
                .space_to_owner_when_empty()
                .on_paste(hook)
        });
        cx.subscribe_in(&field, window, |this, field, event, window, cx| match event {
            TextFieldEvent::Changed => {
                this.dismissed = false;
                let text = field.read(cx).text().to_owned();
                this.model.update(cx, |m, cx| m.set_query(&text, cx));
            }
            TextFieldEvent::Submit => this.submit(window, cx),
        })
        .detach();
        self.field = Some(field.clone());
        field
    }

    /// The keyboard goes to the field (the `/` key).
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let field = self.ensure_field(window, cx);
        text_field::focus(&field, window, cx);
        self.dismissed = false;
        cx.notify();
    }

    /// Whether the keyboard is in the field.
    pub fn has_focus(&self, window: &Window, cx: &App) -> bool {
        self.field
            .as_ref()
            .is_some_and(|f| f.read(cx).focus_handle(cx).is_focused(window))
    }

    /// Ctrl+V: `text` is what the clipboard held. Into the field, one plain line is only typed
    /// (the field does that when this says [`PasteStart::Typed`]).
    pub fn paste(
        &mut self,
        text: &str,
        into_field: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PasteStart {
        let start = self.model.update(cx, |m, cx| m.paste(text, into_field, cx));
        if start == PasteStart::Started {
            // The field may be the one pasting (it is being updated now): what touches it waits
            // for a moment.
            cx.defer_in(window, |this, window, cx| {
                // A list to review takes the keys: the field holds nothing to type over.
                if this.model.read(cx).review().is_some_and(|r| r.confirm) {
                    let field = this.ensure_field(window, cx);
                    if !field.read(cx).text().is_empty() {
                        field.update(cx, |f, cx| f.set_text("", cx));
                    }
                    text_field::focus(&field, window, cx);
                }
                this.dismissed = false;
                cx.notify();
            });
        }
        start
    }

    /// Enter in the field.
    fn submit(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let reviewing = self.model.read(cx).review().is_some();
        if reviewing {
            self.model.update(cx, |m, cx| m.confirm_review(cx));
        } else if !self.dismissed {
            self.model.update(cx, |m, cx| m.add_active(cx));
        }
        cx.notify();
    }

    /// Whether the popover has something to show.
    fn popover_visible(&self, focused: bool, cx: &App) -> bool {
        let model = self.model.read(cx);
        if model.review().is_some() {
            return true;
        }
        if !focused || self.dismissed || model.query().is_empty() {
            return false;
        }
        !model.results().is_empty()
            || matches!(model.status(), Status::NoResults | Status::Failed(_) | Status::Paused)
    }

    fn down(&mut self, _: &FocusResults, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.has_focus(window, cx);
        if self.model.read(cx).review().is_some() {
            self.model.update(cx, |m, cx| m.move_cursor(1, cx));
            self.scroll_review(cx);
        } else if self.popover_visible(focused, cx) && !self.model.read(cx).results().is_empty() {
            self.model.update(cx, |m, cx| m.step(1, cx));
        } else {
            // Nothing to step through: on to the list.
            (self.on_leave.clone())(window, cx);
        }
    }

    fn up(&mut self, _: &PreviousSuggestion, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.has_focus(window, cx);
        if self.model.read(cx).review().is_some() {
            self.model.update(cx, |m, cx| m.move_cursor(-1, cx));
            self.scroll_review(cx);
        } else if self.popover_visible(focused, cx) && !self.model.read(cx).results().is_empty() {
            self.model.update(cx, |m, cx| m.step(-1, cx));
        } else {
            cx.propagate();
        }
    }

    /// Space reaches here only while the field is empty: it ticks a line of the review.
    fn space(&mut self, _: &text_field::InsertSpace, _: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).review().is_some() {
            self.model.update(cx, |m, cx| m.toggle_cursor(cx));
        }
    }

    /// Esc: the review is cancelled, else the popover closes, else text in the field is cleared
    /// (the field does that), else the keyboard goes back to the list.
    fn escape(&mut self, _: &text_field::Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.model.read(cx).review().is_some() {
            self.model.update(cx, |m, cx| m.cancel_review(cx));
            cx.stop_propagation();
            return;
        }
        let focused = self.has_focus(window, cx);
        if self.popover_visible(focused, cx) {
            self.dismissed = true;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let empty = self.field.as_ref().is_none_or(|f| f.read(cx).text().is_empty());
        if empty {
            cx.stop_propagation();
            (self.on_leave.clone())(window, cx);
        }
    }

    fn scroll_review(&self, cx: &App) {
        if let Some(review) = self.model.read(cx).review() {
            self.review_scroll.scroll_to_item(review.cursor);
        }
    }

    // ---- drawing ----------------------------------------------------------------------------

    fn popover(&self, focused: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.popover_visible(focused, cx) {
            return None;
        }
        let model = self.model.read(cx);
        let body = if let Some(review) = model.review() {
            if review.confirm {
                self.review_panel(review, model.addable(cx), cx.entity(), cx)
            } else {
                self.message("Looking it up\u{2026}".to_owned(), cx)
            }
        } else if !model.results().is_empty() {
            self.results_list(cx)
        } else {
            let text = match model.status() {
                Status::NoResults => NO_RESULTS_TEXT.to_owned(),
                Status::Paused => BUSY_TEXT.to_owned(),
                Status::Failed(f) => failure_text(f).to_owned(),
                Status::Idle | Status::Searching => return None,
            };
            self.message(text, cx)
        };
        Some(
            div()
                .id("crate-add-popover")
                .debug_selector(|| "crate-add-popover".into())
                .flex()
                .flex_col()
                .w_full()
                .p(px(PAD))
                .rounded(px(radius::MEDIUM))
                .bg(color::bg_overlay())
                .border_1()
                .border_color(color::border_subtle())
                // Nothing behind the popover reacts to the pointer.
                .occlude()
                .child(body)
                .into_any_element(),
        )
    }

    /// One line of text in the popover's row height.
    fn message(&self, text: String, cx: &App) -> AnyElement {
        style::text_style(div(), typography::BODY, cx)
            .debug_selector(|| "crate-add-message".into())
            .flex()
            .items_center()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S3))
            .text_color(color::text_secondary())
            .child(text)
            .into_any_element()
    }

    /// The songs of the typed search, one 32px row each.
    fn results_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        let active = model.active();
        let rows: Vec<_> = model
            .results()
            .iter()
            .enumerate()
            .map(|(ix, listed)| {
                let held = model.holds(listed.track.video_id.as_str(), cx);
                let artists = listed
                    .track
                    .artists
                    .iter()
                    .map(|a| a.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                (ix, listed.track.title.clone(), artists, held)
            })
            .collect();
        let this = cx.entity();
        let rows = rows.into_iter().map(|(ix, title, artists, held)| {
            let (pick, hover) = (this.clone(), this.clone());
            style::text_style(div(), typography::BODY, cx)
                .id(("crate-add-result", ix))
                .debug_selector(move || format!("crate-add-result-{ix}"))
                .flex()
                .flex_row()
                .flex_none()
                .items_center()
                .gap(px(spacing::S3))
                .h(px(ROW_HEIGHT))
                .px(px(spacing::S3))
                .rounded(px(radius::SMALL))
                .cursor_pointer()
                .text_color(color::text_primary())
                .bg(if active == Some(ix) {
                    color::state_hover()
                } else {
                    gpui::transparent_black()
                })
                .on_hover(move |hovered, _, cx| {
                    if *hovered {
                        hover.update(cx, |b, cx| b.model.update(cx, |m, cx| m.set_active(ix, cx)));
                    }
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    pick.update(cx, |b, cx| {
                        b.model.update(cx, |m, cx| m.add_result(ix, cx));
                    });
                })
                .child(
                    svg()
                        .path(if held { Icon::Checkmark } else { Icon::Add }.path())
                        .size(px(ICON))
                        .flex_none()
                        .text_color(color::text_secondary()),
                )
                .child(div().min_w_0().truncate().child(title))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(color::text_secondary())
                        .child(artists),
                )
                .children(held.then(|| {
                    style::text_style(div(), typography::CAPTION, cx)
                        .flex_none()
                        .text_color(color::text_secondary())
                        .child("In Crate")
                }))
        });
        div().flex().flex_col().children(rows).into_any_element()
    }

    /// The review of a multi-line paste: progress, one row per line, "Add 14" and Cancel.
    fn review_panel(
        &self,
        review: &Review,
        addable: usize,
        this: Entity<Self>,
        cx: &App,
    ) -> AnyElement {
        let accent = self.accent;
        let header = style::text_style(div(), typography::BODY_STRONG, cx)
            .debug_selector(|| "crate-add-progress".into())
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S3))
            .text_color(color::text_secondary())
            .child(crate_add::progress_text(review));
        let note = crate_add::left_out_text(review.left_out).map(|text| {
            style::text_style(div(), typography::CAPTION, cx)
                .px(px(spacing::S3))
                .pb(px(spacing::S1))
                .text_color(color::text_secondary())
                .child(text)
        });
        let rows: Vec<AnyElement> = review
            .lines
            .iter()
            .enumerate()
            .map(|(ix, line)| {
                let (toggle, hover) = (this.clone(), this.clone());
                let cursor = review.cursor == ix;
                let found = line.state == LineState::Found;
                let (title, artists) = (line.title(), line.artists());
                let heading = if artists.is_empty() {
                    title
                } else {
                    format!("{title} \u{b7} {artists}")
                };
                let can_pick = line.can_pick();
                let picked = line.picked;
                style::text_style(div(), typography::BODY, cx)
                    .id(("crate-add-line", ix))
                    .debug_selector(move || format!("crate-add-line-{ix}"))
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(px(spacing::S3))
                    .h(px(REVIEW_ROW_HEIGHT))
                    .px(px(spacing::S3))
                    .rounded(px(radius::SMALL))
                    .when(can_pick, |d| d.cursor_pointer())
                    .bg(if cursor { color::state_hover() } else { gpui::transparent_black() })
                    .on_hover(move |hovered, _, cx| {
                        if *hovered {
                            hover.update(cx, |b, cx| b.model.update(cx, |m, cx| m.set_cursor(ix, cx)));
                        }
                    })
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        toggle.update(cx, |b, cx| b.model.update(cx, |m, cx| m.toggle(ix, cx)));
                    })
                    .child(checkbox(can_pick, picked, accent, ix))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_color(if found {
                                        color::text_primary()
                                    } else {
                                        color::text_secondary()
                                    })
                                    .child(heading),
                            )
                            .child(
                                style::text_style(div(), typography::CAPTION, cx)
                                    .truncate()
                                    .text_color(color::text_secondary())
                                    .child(line.raw.clone()),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let can_add = !review.working() && addable > 0;
        let (add, cancel) = (this.clone(), this);
        let footer = div()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .justify_end()
            .gap(px(spacing::S2))
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S2))
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .id("crate-add-cancel")
                    .debug_selector(|| "crate-add-cancel".into())
                    .flex()
                    .items_center()
                    .px(px(spacing::S2))
                    .h(px(spacing::S6))
                    .rounded(px(radius::SMALL))
                    .cursor_pointer()
                    .text_color(color::text_secondary())
                    .hover(|s| s.text_color(color::text_primary()).bg(color::state_hover()))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        cancel.update(cx, |b, cx| b.model.update(cx, |m, cx| m.cancel_review(cx)));
                    })
                    .child("Cancel"),
            )
            .child(
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id("crate-add-confirm")
                    .debug_selector(|| "crate-add-confirm".into())
                    .flex()
                    .items_center()
                    .px(px(spacing::S2))
                    .h(px(spacing::S6))
                    .rounded(px(radius::SMALL))
                    .when(can_add, |d| {
                        d.cursor_pointer()
                            .text_color(color::text_primary())
                            .hover(|s| s.bg(color::state_hover()))
                            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                cx.stop_propagation();
                                add.update(cx, |b, cx| {
                                    b.model.update(cx, |m, cx| m.confirm_review(cx));
                                });
                            })
                    })
                    .when(!can_add, |d| d.text_color(color::text_secondary()))
                    .child(add_text(addable)),
            );
        div()
            .flex()
            .flex_col()
            .child(header)
            .children(note)
            .child(
                div()
                    .id("crate-add-lines")
                    .debug_selector(|| "crate-add-lines".into())
                    .flex()
                    .flex_col()
                    .max_h(px(REVIEW_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.review_scroll)
                    .children(rows),
            )
            .child(footer)
            .into_any_element()
    }
}

/// "Add 14", "Add" when there is nothing to add.
pub fn add_text(count: usize) -> String {
    if count == 0 {
        "Add".to_owned()
    } else {
        format!("Add {count}")
    }
}

/// The 16px box of a review row: ticked with the accent, empty, or (a line with nothing to add)
/// left blank.
fn checkbox(pickable: bool, picked: bool, accent: Accent, ix: usize) -> impl IntoElement {
    div()
        .debug_selector(move || format!("crate-add-check-{ix}"))
        .size(px(ICON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .when(pickable, |d| {
            d.rounded(px(radius::SMALL))
                .border_1()
                .border_color(color::border_strong())
                .when(picked, |d| {
                    d.child(
                        svg()
                            .path(Icon::Checkmark.path())
                            .size(px(ICON - 4.0))
                            .flex_none()
                            .text_color(color::accent(accent)),
                    )
                })
        })
}

impl Focusable for CrateAddBox {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.field {
            Some(field) => field.read(cx).focus_handle(cx),
            None => cx.focus_handle(),
        }
    }
}

impl Render for CrateAddBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let field = self.ensure_field(window, cx);
        let handle = field.read(cx).focus_handle(cx);
        let focused = handle.is_focused(window);
        // Taking the keyboard brings a closed popover back.
        if focused && !self.field_was_focused {
            self.dismissed = false;
        }
        self.field_was_focused = focused;
        let ring = keyboard_focused(&handle, window);
        let popover = self.popover(focused, cx);
        let docked = self.docked;
        div()
            .id(if docked { "crate-dock-add" } else { "crate-add" })
            .debug_selector(move || if docked { "dock-add" } else { "crate-add" }.into())
            .relative()
            .flex_none()
            .when(docked, |d| d.w_full())
            .when(!docked, |d| d.w(px(PAGE_WIDTH)).max_w_full())
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::space))
            .capture_action(cx.listener(Self::escape))
            .child(field)
            .when(ring, |d| d.child(focus_ring(color::accent(self.accent), radius::SMALL)))
            .children(popover.map(|popover| {
                // Deferred so it paints over the list below the field.
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
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_add_button_counts_what_it_adds() {
        assert_eq!(add_text(0), "Add");
        assert_eq!(add_text(1), "Add 1");
        assert_eq!(add_text(14), "Add 14");
    }
}
