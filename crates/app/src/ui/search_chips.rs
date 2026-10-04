//! The row of kind chips under Search's field: All, Songs, Albums, Artists, Playlists and
//! Community playlists. One row of pills on bg.raised; the active one is filled bg.overlay with
//! text.primary and an accent outline.
//!
//! The row is one tab stop (the active chip, as on Home's shelves); Left and Right move between
//! the chips and Enter picks the one in focus (Space stays the global play/pause key).

use std::rc::Rc;
use std::time::Instant;

use apollo_theme::tokens::{layout, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    actions, div, prelude::*, px, transparent_black, App, FocusHandle, KeyBinding, SharedString,
    Window,
};

use crate::motion::HoverFade;
use crate::search::SearchChip;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};

actions!(search_chips, [ChipLeft, ChipRight, ChipPick]);

/// Key context of the chip row.
pub const KEY_CONTEXT: &str = "SearchChips";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("left", ChipLeft, Some(KEY_CONTEXT)),
        KeyBinding::new("right", ChipRight, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", ChipPick, Some(KEY_CONTEXT)),
    ]);
}

/// The outline that marks the active chip. Every chip reserves it, so picking one moves nothing.
const OUTLINE: f32 = 1.0;

#[derive(IntoElement)]
pub struct ChipRow {
    row: PillRow,
}

impl ChipRow {
    pub fn new(
        active: SearchChip,
        accent: Accent,
        focus: &[FocusHandle],
        on_pick: impl Fn(SearchChip, &mut Window, &mut App) + 'static,
    ) -> Self {
        let labels = SearchChip::ROW.iter().map(|c| SharedString::from(c.label())).collect();
        let active = SearchChip::ROW.iter().position(|c| *c == active);
        Self {
            row: PillRow::new(
                "search-chip",
                labels,
                active,
                accent,
                focus,
                move |ix, window, cx| on_pick(SearchChip::ROW[ix], window, cx),
            ),
        }
    }
}

impl RenderOnce for ChipRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.row.render(window, cx)
    }
}

/// A row of pills, one of which may be active: the search chips and Home's mood chips are both
/// this. Left and Right move between the pills and Enter picks the one in focus; the row is one
/// tab stop (the active pill, or the first when none is).
#[derive(IntoElement)]
pub struct PillRow {
    /// Names the row's elements: `{prefix}-{label}` is each pill's id and test selector.
    prefix: &'static str,
    labels: Vec<SharedString>,
    active: Option<usize>,
    accent: Accent,
    /// One tab stop per pill, in label order.
    focus: Vec<FocusHandle>,
    on_pick: Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>,
}

impl PillRow {
    pub fn new(
        prefix: &'static str,
        labels: Vec<SharedString>,
        active: Option<usize>,
        accent: Accent,
        focus: &[FocusHandle],
        on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            prefix,
            labels,
            active,
            accent,
            focus: focus.to_vec(),
            on_pick: Rc::new(on_pick),
        }
    }
}

impl RenderOnce for PillRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let handles = self.focus.clone();
        let step = move |delta: isize| {
            let handles = handles.clone();
            move |window: &mut Window, cx: &mut App| {
                let Some(at) = handles.iter().position(|h| h.is_focused(window)) else {
                    return;
                };
                let to = (at as isize + delta).clamp(0, handles.len() as isize - 1) as usize;
                window.focus(&handles[to], cx);
            }
        };
        let (left, right) = (step(-1), step(1));
        let pick_handles = self.focus.clone();
        let pick_on = self.on_pick.clone();
        let pick = move |window: &mut Window, cx: &mut App| {
            if let Some(at) = pick_handles.iter().position(|h| h.is_focused(window)) {
                pick_on(at, window, cx);
            }
        };
        let stop = self.active.unwrap_or(0);
        let chips: Vec<_> = self
            .labels
            .into_iter()
            .zip(self.focus)
            .enumerate()
            .map(|(ix, (label, focus))| {
                let on_pick = self.on_pick.clone();
                chip_view(
                    self.prefix,
                    label,
                    Some(ix) == self.active,
                    ix == stop,
                    self.accent,
                    focus,
                    move |window, cx| on_pick(ix, window, cx),
                    window,
                    cx,
                )
            })
            .collect();
        div()
            .id(SharedString::from(format!("{}s", self.prefix)))
            .key_context(KEY_CONTEXT)
            .on_action(move |_: &ChipLeft, window, cx| left(window, cx))
            .on_action(move |_: &ChipRight, window, cx| right(window, cx))
            .on_action(move |_: &ChipPick, window, cx| pick(window, cx))
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(spacing::S2))
            .children(chips)
    }
}

#[allow(clippy::too_many_arguments)]
fn chip_view(
    prefix: &'static str,
    label: SharedString,
    active: bool,
    tab_stop: bool,
    accent: Accent,
    focus: FocusHandle,
    on_pick: impl Fn(&mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let id: SharedString = format!("{prefix}-{label}").into();
    let fade = window.use_keyed_state(id.clone(), cx, |_, _| HoverFade::default());
    let (level, moving) = fade.read(cx).level(Instant::now());
    if moving {
        window.request_animation_frame();
    }
    let animations = !cx.reduce_motion();
    let ring = keyboard_focused(&focus, window);
    let accent = color::accent(accent);
    style::text_style(div(), typography::BODY, cx)
        .debug_selector({
            let id = id.to_string();
            move || id.clone()
        })
        .id(id)
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .h(px(layout::MIN_HIT_TARGET))
        .px(px(spacing::S3))
        .rounded_full()
        .border(px(OUTLINE))
        .border_color(if active { accent } else { transparent_black() })
        .bg(if active {
            color::bg_overlay()
        } else {
            color::bg_raised()
        })
        .text_color(if active {
            color::text_primary()
        } else {
            color::text_secondary()
        })
        .whitespace_nowrap()
        .cursor_pointer()
        // One stop for the row: the active chip. The rest take focus from the arrows.
        .track_focus(&focus.clone().tab_stop(tab_stop))
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .on_hover(move |hovered, _, cx| {
            fade.update(cx, |s, cx| {
                s.set(*hovered, Instant::now(), animations);
                cx.notify();
            });
        })
        .on_click(move |event, window, cx| {
            // The keyboard picks through `ChipPick`; Space is the play/pause key.
            if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                on_pick(window, cx);
            }
        })
        .child(
            div()
                .absolute()
                .size_full()
                .top_0()
                .left_0()
                .rounded_full()
                .bg(color::state_hover().opacity(level)),
        )
        .child(label)
        .when(ring, move |d| {
            d.child(focus_ring(accent, layout::MIN_HIT_TARGET / 2.0))
        })
}
