//! The transport row (shuffle, previous, play/pause, next, repeat) and the progress row (times
//! around the seek slider), shared by the player bar and the Now Playing view.

use apollo_domain::queue::Repeat;
use apollo_theme::tokens::{layout, spacing, typography};
use gpui::{div, prelude::*, px, App, Entity, FocusHandle, IntoElement, SharedString, Styled};

use crate::assets::Icon;
use crate::player::{format_time, PlayerModel, PlayerView, Status};
use crate::style::{self, color};
use crate::ui::button::IconButton;
use crate::ui::slider::{Slider, SliderState};

/// Seek step for the keyboard on the progress bar.
const SEEK_STEP_MS: f64 = 5_000.0;
const TIME_WIDTH: f32 = spacing::S12;

#[derive(Clone)]
pub struct TransportFocus {
    pub shuffle: FocusHandle,
    pub previous: FocusHandle,
    pub play: FocusHandle,
    pub next: FocusHandle,
    pub repeat: FocusHandle,
    pub progress: FocusHandle,
}

fn id(prefix: &str, name: &str) -> SharedString {
    format!("{prefix}{name}").into()
}

pub fn on_off(on: bool) -> gpui::Hsla {
    if on {
        color::text_primary()
    } else {
        color::text_secondary()
    }
}

/// Shuffle, previous, play/pause, next, repeat. `prefix` keeps element ids unique when the row
/// is on screen twice.
pub fn buttons(
    prefix: &str,
    player: &Entity<PlayerModel>,
    v: &PlayerView,
    focus: &TransportFocus,
) -> gpui::Div {
    let accent = v.accent;
    let ready = v.status == Status::Ready;
    let controls = ready && v.has_track;
    let accent_hsla = color::accent(accent);

    let shuffle_icon = if v.shuffle {
        Icon::ShuffleFilled
    } else {
        Icon::ShuffleRegular
    };
    let (repeat_icon, repeat_label, repeat_on) = match v.repeat {
        Repeat::Off => (Icon::RepeatAllRegular, "Repeat off", false),
        Repeat::All => (Icon::RepeatAllFilled, "Repeat all", true),
        Repeat::One => (Icon::RepeatOneFilled, "Repeat one", true),
    };
    let (play_icon, play_label) = if v.busy {
        (Icon::Spinner, "Pause")
    } else if v.playing {
        (Icon::Pause, "Pause")
    } else {
        (Icon::Play, "Play")
    };

    let button = |name: &str, focus: &FocusHandle, icon: Icon, label: &str, enabled: bool| {
        IconButton::new(id(prefix, name), focus, icon, label.to_owned(), accent).enabled(enabled)
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S2))
        .h(px(layout::PLAY_BUTTON))
        .child(
            button("shuffle", &focus.shuffle, shuffle_icon, "Shuffle", ready)
                .icon_color(on_off(v.shuffle))
                .toggled(v.shuffle)
                .on_click({
                    let p = player.clone();
                    move |_, cx| {
                        p.update(cx, |m, cx| m.toggle_shuffle(cx));
                    }
                }),
        )
        .child(
            button(
                "previous",
                &focus.previous,
                Icon::Previous,
                "Previous",
                controls,
            )
            .icon_color(color::text_primary())
            .on_click({
                let p = player.clone();
                move |_, cx| {
                    p.update(cx, |m, cx| m.previous(cx));
                }
            }),
        )
        .child(
            button("play", &focus.play, play_icon, play_label, controls)
                .size(layout::PLAY_BUTTON)
                .circle()
                .filled(if controls {
                    accent_hsla
                } else {
                    color::bg_overlay()
                })
                .icon_color(color::accent_fg(accent))
                .spinning(v.busy)
                .on_click({
                    let p = player.clone();
                    move |_, cx| {
                        p.update(cx, |m, cx| m.toggle_play(cx));
                    }
                }),
        )
        .child(
            button("next", &focus.next, Icon::Next, "Next", controls)
                .icon_color(color::text_primary())
                .on_click({
                    let p = player.clone();
                    move |_, cx| {
                        p.update(cx, |m, cx| m.next(cx));
                    }
                }),
        )
        .child(
            button("repeat", &focus.repeat, repeat_icon, repeat_label, ready)
                .icon_color(on_off(repeat_on))
                .on_click({
                    let p = player.clone();
                    move |_, cx| {
                        p.update(cx, |m, cx| m.cycle_repeat(cx));
                    }
                }),
        )
}

fn time(prefix: &str, name: &str, text: String, right: bool, cx: &App) -> impl IntoElement {
    style::tabular(style::text_style(div(), typography::CAPTION, cx))
        .id(id(prefix, name))
        .w(px(TIME_WIDTH))
        .flex_none()
        .text_color(color::text_tertiary())
        .when(right, |d| d.text_right())
        .child(text)
}

/// Elapsed time, the seek slider and the length.
pub fn progress(
    prefix: &str,
    player: &Entity<PlayerModel>,
    v: &PlayerView,
    focus: &FocusHandle,
    state: &Entity<SliderState>,
    cx: &App,
) -> gpui::Div {
    let controls = v.status == Status::Ready && v.has_track;
    let duration = v.duration_ms;
    let seekable = controls && duration.is_some_and(|d| d > 0);
    let scrub = state.read(cx).scrubbing();
    let shown_ms = match (scrub, duration) {
        (Some(f), Some(d)) => (f64::from(f) * d as f64) as u64,
        _ => v.position_ms,
    };
    let fraction = duration
        .filter(|d| *d > 0)
        .map_or(0.0, |d| (v.position_ms as f64 / d as f64).min(1.0) as f32);
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(spacing::S2))
        .w_full()
        .child(time(
            prefix,
            "time-position",
            format_time(shown_ms),
            true,
            cx,
        ))
        .child(
            div().flex_1().min_w_0().child(
                Slider::new(id(prefix, "progress"), state, focus, "Seek", v.accent)
                    .value(fraction)
                    .value_text(match duration {
                        Some(d) if seekable => {
                            format!("{} of {}", format_time(shown_ms), format_time(d))
                        }
                        _ => format_time(shown_ms),
                    })
                    .step(
                        duration
                            .filter(|d| *d > 0)
                            .map_or(0.0, |d| (SEEK_STEP_MS / d as f64) as f32),
                    )
                    .enabled(seekable)
                    .on_change({
                        let p = player.clone();
                        move |frac, committed, _, cx| {
                            if committed {
                                if let Some(d) = duration {
                                    let ms = (f64::from(frac) * d as f64).round() as u64;
                                    p.update(cx, |m, cx| m.seek(ms, cx));
                                }
                            }
                        }
                    }),
            ),
        )
        .child(time(
            prefix,
            "time-duration",
            format_time(duration.unwrap_or(0)),
            false,
            cx,
        ))
}
