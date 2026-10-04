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
    active: SearchChip,
    accent: Accent,
    /// One tab stop per chip, in [`SearchChip::ROW`] order.
    focus: Vec<FocusHandle>,
    on_pick: Rc<dyn Fn(SearchChip, &mut Window, &mut App) + 'static>,
}

impl ChipRow {
    pub fn new(
        active: SearchChip,
        accent: Accent,
        focus: &[FocusHandle],
        on_pick: impl Fn(SearchChip, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            active,
            accent,
            focus: focus.to_vec(),
            on_pick: Rc::new(on_pick),
        }
    }
}

impl RenderOnce for ChipRow {
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
                pick_on(SearchChip::ROW[at], window, cx);
            }
        };
        let chips: Vec<_> = SearchChip::ROW
            .into_iter()
            .zip(self.focus)
            .map(|(chip, focus)| {
                chip_view(
                    chip,
                    chip == self.active,
                    self.accent,
                    focus,
                    self.on_pick.clone(),
                    window,
                    cx,
                )
            })
            .collect();
        div()
            .id("search-chips")
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

fn chip_view(
    chip: SearchChip,
    active: bool,
    accent: Accent,
    focus: FocusHandle,
    on_pick: Rc<dyn Fn(SearchChip, &mut Window, &mut App) + 'static>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let id: SharedString = format!("search-chip-{}", chip.label()).into();
    let fade = window.use_keyed_state(id.clone(), cx, |_, _| HoverFade::default());
    let (level, moving) = fade.read(cx).level(Instant::now());
    if moving {
        window.request_animation_frame();
    }
    let animations = !cx.reduce_motion();
    let ring = keyboard_focused(&focus, window);
    let accent = color::accent(accent);
    let label = chip.label();
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
        .track_focus(&focus.clone().tab_stop(active))
        .role(gpui::Role::Button)
        .aria_label(label)
        .on_hover(move |hovered, _, cx| {
            fade.update(cx, |s, cx| {
                s.set(*hovered, Instant::now(), animations);
                cx.notify();
            });
        })
        .on_click(move |event, window, cx| {
            // The keyboard picks through `ChipPick`; Space is the play/pause key.
            if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                on_pick(chip, window, cx);
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
