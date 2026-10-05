//! Multi-select for song lists: the shared part every list drops in.
//!
//! A list keeps a [`Selection`] beside its keyboard cursor. The cursor is the row the arrow keys
//! are on (one row, drawn with the focus ring as before); the selection is the set of rows the
//! bulk verbs act on. They are independent: Ctrl+click picks rows without moving the cursor's
//! meaning, Shift+Arrow moves the cursor and stretches the selection to it.
//!
//! * Shift+click / Shift+Up / Shift+Down: the rows from the anchor to the pointer or cursor.
//! * Ctrl+click: add or take out one row. The row becomes the anchor for the next Shift.
//! * Ctrl+A: every row of the list. Esc: clear (then Esc does what it did before).
//! * A plain click does what it always did (play) and clears the selection.
//!
//! Rows are named by a key the list chooses (a video id, a playlist row's token), so the
//! selection survives rows moving, loading in and dropping out; [`Selection::keep`] forgets keys
//! that left the list. The verbs act from 2 rows up ([`Selection::is_bulk`]); one picked row is
//! a plain row.
//!
//! The look (coordinator, DIRECTION.md): a selected row is tinted with the accent at 16% alpha
//! ([`row_tint`]); the list's header shows "N selected" in caption text.secondary at its right
//! end while 2 or more are picked ([`count_label`]). No floating bar.

use std::collections::HashSet;
use std::hash::Hash;

use apollo_theme::tokens::typography;
use apollo_theme::Accent;
use gpui::{div, prelude::*, px, ClickEvent, Hsla, Modifiers, SharedString};

use crate::style::{self, color};

/// Accent alpha of a selected row's background.
pub const TINT_ALPHA: f32 = 0.16;
/// Width of the "N selected" slot in a header that reserves one, so the header does not shift
/// when the label comes and goes.
pub const COUNT_SLOT_WIDTH: f32 = 96.0;

/// What a click on a row means for the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// No modifier: the row's own action (play), and the selection is cleared.
    Plain,
    /// Ctrl (or Cmd): add or take out this row.
    Toggle,
    /// Shift: the range from the anchor to this row. `add` (Shift and Ctrl together) keeps the
    /// rows already picked instead of replacing them.
    Range { add: bool },
}

impl Click {
    pub fn of(modifiers: Modifiers) -> Self {
        let toggle = modifiers.control || modifiers.platform;
        if modifiers.shift {
            Click::Range { add: toggle }
        } else if toggle {
            Click::Toggle
        } else {
            Click::Plain
        }
    }

    /// The click on a row, read from the event gpui hands a click listener.
    pub fn of_event(event: &ClickEvent) -> Self {
        // A keyboard click (Enter or Space on a control) never carries modifiers.
        Self::of(event.modifiers())
    }

    /// Whether this click is a selection gesture (the row's own action then stays quiet).
    pub fn is_selecting(self) -> bool {
        self != Click::Plain
    }
}

/// The rows picked in one list, named by keys of the list's choosing.
#[derive(Clone, Debug)]
pub struct Selection<K> {
    picked: HashSet<K>,
    /// Where the next Shift gesture starts from.
    anchor: Option<K>,
}

impl<K> Default for Selection<K> {
    fn default() -> Self {
        Self {
            picked: HashSet::new(),
            anchor: None,
        }
    }
}

impl<K: Clone + Eq + Hash> Selection<K> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.picked.clear();
        self.anchor = None;
    }

    pub fn is_empty(&self) -> bool {
        self.picked.is_empty()
    }

    pub fn len(&self) -> usize {
        self.picked.len()
    }

    /// Two rows or more: the verbs act on the selection, not on the row under the pointer.
    pub fn is_bulk(&self) -> bool {
        self.picked.len() >= 2
    }

    pub fn contains(&self, key: &K) -> bool {
        self.picked.contains(key)
    }

    /// Whether `key` is picked as part of a bulk selection: what a row's menu acts on.
    pub fn in_bulk(&self, key: &K) -> bool {
        self.is_bulk() && self.picked.contains(key)
    }

    /// The picked keys in the list's order. `order` is every key of the list, top to bottom.
    pub fn members<'a>(&self, order: &'a [K]) -> Vec<&'a K> {
        order.iter().filter(|k| self.picked.contains(*k)).collect()
    }

    /// Rows that took another key (`(old, new)`: a song swapped for another version of itself):
    /// whatever was picked, or the anchor, under the old key is under the new one.
    pub fn remap(&mut self, pairs: &[(K, K)]) {
        for (old, new) in pairs {
            if self.picked.remove(old) {
                self.picked.insert(new.clone());
            }
            if self.anchor.as_ref() == Some(old) {
                self.anchor = Some(new.clone());
            }
        }
    }

    /// Forget keys that are no longer in the list.
    pub fn keep(&mut self, order: &[K]) {
        if self.picked.is_empty() {
            self.anchor = None;
            return;
        }
        let present: HashSet<&K> = order.iter().collect();
        self.picked.retain(|k| present.contains(k));
        if self.anchor.as_ref().is_some_and(|a| !present.contains(a)) {
            self.anchor = None;
        }
    }

    /// Apply a click on `key`. Returns whether it was a selection gesture; a plain click clears
    /// the selection (and makes the row the anchor) and leaves the row's own action to the caller.
    /// `cursor` is the row the keyboard is on: with no anchor yet, a Shift-click stretches from
    /// there.
    pub fn click(&mut self, how: Click, key: &K, cursor: Option<&K>, order: &[K]) -> bool {
        match how {
            Click::Plain => {
                self.picked.clear();
                self.anchor = Some(key.clone());
                false
            }
            Click::Toggle => {
                if !self.picked.remove(key) {
                    self.picked.insert(key.clone());
                }
                self.anchor = Some(key.clone());
                true
            }
            Click::Range { add } => {
                if self.anchor.is_none() {
                    self.anchor = cursor.cloned().or_else(|| Some(key.clone()));
                }
                self.stretch_to(key, order, add);
                true
            }
        }
    }

    /// Shift+Up / Shift+Down: the cursor moved to `to`; the selection becomes the rows from the
    /// anchor (the row the cursor started from, if there is none yet) to it.
    pub fn extend(&mut self, from: Option<&K>, to: &K, order: &[K]) {
        if self.anchor.is_none() {
            self.anchor = from.cloned().or_else(|| Some(to.clone()));
        }
        self.stretch_to(to, order, false);
    }

    /// Ctrl+A.
    pub fn select_all(&mut self, order: &[K]) {
        self.picked = order.iter().cloned().collect();
        if self.anchor.is_none() {
            self.anchor = order.first().cloned();
        }
    }

    /// Pick the rows between the anchor and `to`, inclusive, in `order`.
    fn stretch_to(&mut self, to: &K, order: &[K], add: bool) {
        let ends = (
            self.anchor
                .as_ref()
                .and_then(|a| order.iter().position(|k| k == a)),
            order.iter().position(|k| k == to),
        );
        let (Some(a), Some(b)) = ends else {
            // The anchor left the list: start over from this row.
            self.picked.insert(to.clone());
            self.anchor = Some(to.clone());
            return;
        };
        let (lo, hi) = (a.min(b), a.max(b));
        if !add {
            self.picked.clear();
        }
        self.picked.extend(order[lo..=hi].iter().cloned());
    }
}

/// A selected row's background: the accent at 16% alpha.
pub fn row_tint(accent: Accent) -> Hsla {
    color::accent(accent).opacity(TINT_ALPHA)
}

/// "N selected": caption text.secondary, for the right end of a list's header. `None` unless 2
/// or more are picked.
pub fn count_label(count: usize, cx: &gpui::App) -> Option<gpui::Div> {
    (count >= 2).then(|| {
        style::text_style(div(), typography::CAPTION, cx)
            .text_color(color::text_secondary())
            .child(count_text(count))
    })
}

/// A header's fixed slot for the label: its width stays when the label goes.
pub fn count_slot(count: usize, cx: &gpui::App) -> gpui::Div {
    div()
        .w(px(COUNT_SLOT_WIDTH))
        .flex_none()
        .flex()
        .justify_end()
        .children(count_label(count, cx))
}

pub fn count_text(count: usize) -> SharedString {
    format!("{count} selected").into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("k{i}")).collect()
    }

    fn picked(sel: &Selection<String>, order: &[String]) -> Vec<String> {
        sel.members(order).into_iter().cloned().collect()
    }

    fn k(i: usize) -> String {
        format!("k{i}")
    }

    #[test]
    fn modifiers_mean_the_usual_things() {
        assert_eq!(Click::of(Modifiers::none()), Click::Plain);
        assert_eq!(Click::of(Modifiers::control()), Click::Toggle);
        assert_eq!(Click::of(Modifiers::shift()), Click::Range { add: false });
        let both = Modifiers {
            shift: true,
            control: true,
            ..Modifiers::none()
        };
        assert_eq!(Click::of(both), Click::Range { add: true });
        assert!(!Click::Plain.is_selecting());
        assert!(Click::Toggle.is_selecting());
    }

    #[test]
    fn ctrl_click_toggles_rows_one_at_a_time() {
        let order = keys(5);
        let mut sel = Selection::new();
        assert!(sel.click(Click::Toggle, &k(1), None, &order));
        assert!(!sel.is_bulk(), "one row is a plain row");
        assert!(sel.click(Click::Toggle, &k(3), None, &order));
        assert!(sel.is_bulk());
        assert_eq!(picked(&sel, &order), [k(1), k(3)]);
        sel.click(Click::Toggle, &k(1), None, &order);
        assert_eq!(picked(&sel, &order), [k(3)]);
        assert!(!sel.is_bulk());
    }

    #[test]
    fn shift_click_takes_the_range_from_the_anchor_and_replaces_the_last_one() {
        let order = keys(8);
        let mut sel = Selection::new();
        sel.click(Click::Toggle, &k(2), None, &order);
        sel.click(Click::Range { add: false }, &k(5), None, &order);
        assert_eq!(picked(&sel, &order), [k(2), k(3), k(4), k(5)]);
        // The anchor stays: a second Shift-click re-stretches from the same row.
        sel.click(Click::Range { add: false }, &k(0), None, &order);
        assert_eq!(picked(&sel, &order), [k(0), k(1), k(2)]);
    }

    #[test]
    fn ctrl_shift_click_adds_a_range_to_what_is_there() {
        let order = keys(10);
        let mut sel = Selection::new();
        sel.click(Click::Toggle, &k(0), None, &order);
        sel.click(Click::Toggle, &k(6), None, &order);
        sel.click(Click::Range { add: true }, &k(8), None, &order);
        assert_eq!(picked(&sel, &order), [k(0), k(6), k(7), k(8)]);
    }

    #[test]
    fn shift_click_with_no_anchor_starts_at_the_cursor() {
        let order = keys(6);
        let mut sel = Selection::new();
        sel.click(Click::Range { add: false }, &k(4), Some(&k(1)), &order);
        assert_eq!(picked(&sel, &order), [k(1), k(2), k(3), k(4)]);
        let mut sel = Selection::new();
        sel.click(Click::Range { add: false }, &k(4), None, &order);
        assert_eq!(picked(&sel, &order), [k(4)]);
    }

    #[test]
    fn a_plain_click_clears_and_is_left_to_the_row() {
        let order = keys(4);
        let mut sel = Selection::new();
        sel.select_all(&order);
        assert!(!sel.click(Click::Plain, &k(2), None, &order));
        assert!(sel.is_empty());
        // And it is where a following Shift-click starts.
        sel.click(Click::Range { add: false }, &k(3), None, &order);
        assert_eq!(picked(&sel, &order), [k(2), k(3)]);
    }

    #[test]
    fn shift_arrows_stretch_from_where_the_cursor_started_and_shrink_back() {
        let order = keys(6);
        let mut sel = Selection::new();
        // Cursor on 2; Shift+Down twice.
        sel.extend(Some(&k(2)), &k(3), &order);
        sel.extend(Some(&k(3)), &k(4), &order);
        assert_eq!(picked(&sel, &order), [k(2), k(3), k(4)]);
        // Shift+Up: shrinks.
        sel.extend(Some(&k(4)), &k(3), &order);
        assert_eq!(picked(&sel, &order), [k(2), k(3)]);
        // Past the anchor the range flips to the other side.
        sel.extend(Some(&k(3)), &k(2), &order);
        sel.extend(Some(&k(2)), &k(1), &order);
        assert_eq!(picked(&sel, &order), [k(1), k(2)]);
    }

    #[test]
    fn select_all_and_clear() {
        let order = keys(3);
        let mut sel = Selection::new();
        sel.select_all(&order);
        assert_eq!(sel.len(), 3);
        assert!(sel.in_bulk(&k(1)));
        sel.clear();
        assert!(sel.is_empty());
        assert!(!sel.in_bulk(&k(1)));
    }

    #[test]
    fn one_picked_row_is_not_drawn_or_acted_on_as_a_selection() {
        let order = keys(3);
        let mut sel = Selection::new();
        sel.click(Click::Toggle, &k(1), None, &order);
        assert!(sel.contains(&k(1)));
        assert!(!sel.in_bulk(&k(1)));
    }

    #[test]
    fn rows_that_left_the_list_are_forgotten() {
        let order = keys(5);
        let mut sel = Selection::new();
        sel.click(Click::Toggle, &k(0), None, &order);
        sel.click(Click::Toggle, &k(2), None, &order);
        sel.click(Click::Toggle, &k(4), None, &order);
        let after: Vec<String> = vec![k(0), k(4), k(9)];
        sel.keep(&after);
        assert_eq!(picked(&sel, &after), [k(0), k(4)]);
        assert!(!sel.contains(&k(2)));
        // The anchor (4) is still there; losing it makes the next Shift start at the row clicked.
        sel.keep(&[k(0)]);
        sel.click(Click::Range { add: false }, &k(0), None, &[k(0)]);
        assert_eq!(picked(&sel, &[k(0)]), [k(0)]);
    }

    #[test]
    fn members_come_in_the_lists_order_not_the_order_they_were_picked() {
        let order = keys(5);
        let mut sel = Selection::new();
        for i in [3, 0, 2] {
            sel.click(Click::Toggle, &k(i), None, &order);
        }
        assert_eq!(picked(&sel, &order), [k(0), k(2), k(3)]);
    }

    #[test]
    fn the_label_shows_from_two_up() {
        assert_eq!(count_text(3), "3 selected");
    }
}
