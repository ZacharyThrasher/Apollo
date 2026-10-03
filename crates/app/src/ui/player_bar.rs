//! Bottom player bar (80px, bg.raised). Left: art and title/artist. Centre: shuffle, previous,
//! play/pause, next, repeat over the progress bar. Right: volume and the queue toggle.

use std::rc::Rc;

use apollo_domain::queue::Repeat;
use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    div, img, prelude::*, px, relative, App, Entity, FocusHandle, ImageSource, IntoElement,
    ObjectFit, ParentElement, RenderOnce, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::player::{failure_text, format_time, PlayerModel, PlayerView, Status};
use crate::style::{self, color};
use crate::ui::button::{Handler, IconButton};
use crate::ui::slider::{Slider, SliderState};
use crate::ui::tooltip::Tooltip;

/// Seek step for the keyboard on the progress bar.
const SEEK_STEP_MS: f64 = 5_000.0;
const VOLUME_WIDTH: f32 = 96.0;
const TIME_WIDTH: f32 = spacing::S12;
/// Widest the centre column grows; it takes 40% of the bar below that.
const CENTER_MAX: f32 = 640.0;
const CENTER_MIN: f32 = 320.0;

#[derive(Clone)]
pub struct BarFocus {
    pub shuffle: FocusHandle,
    pub previous: FocusHandle,
    pub play: FocusHandle,
    pub next: FocusHandle,
    pub repeat: FocusHandle,
    pub mute: FocusHandle,
    pub queue: FocusHandle,
    pub progress: FocusHandle,
    pub volume: FocusHandle,
}

#[derive(IntoElement)]
pub struct PlayerBar {
    pub player: Entity<PlayerModel>,
    pub view: PlayerView,
    pub focus: BarFocus,
    pub progress: Entity<SliderState>,
    pub volume: Entity<SliderState>,
    pub queue_open: bool,
    pub on_queue_toggle: Rc<Handler>,
}

fn volume_icon(volume: u8, muted: bool) -> Icon {
    if muted || volume == 0 {
        Icon::VolumeMute
    } else if volume <= 50 {
        Icon::VolumeLow
    } else {
        Icon::VolumeHigh
    }
}

impl RenderOnce for PlayerBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let v = self.view;
        let accent = v.accent;
        let ready = v.status == Status::Ready;
        let controls = ready && v.has_track;
        let accent_hsla = color::accent(accent);

        // ---- left: art, title, artist / error ---------------------------------------
        let art = div()
            .size(px(layout::PLAYER_BAR_ART))
            .flex_none()
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .bg(color::bg_overlay())
            .when_some(v.art.clone(), |d, image| {
                d.child(
                    img(ImageSource::Render(image))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
            });

        let player = self.player.clone();
        let second_line = match (&v.status, v.error) {
            (Status::Unavailable(text), _) => Some((text.clone(), true)),
            (_, Some(failure)) => Some((SharedString::from(failure_text(failure)), true)),
            _ => None,
        };
        let artist_line = match second_line {
            Some((text, _)) => div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(spacing::S2))
                .child(
                    style::text_style(div(), typography::BODY, cx)
                        .truncate()
                        .text_color(color::error())
                        .child(text),
                )
                .when(v.error.is_some(), |d| {
                    let player = player.clone();
                    d.child(
                        style::text_style(div(), typography::BODY_STRONG, cx)
                            .id("retry")
                            .flex_none()
                            .px(px(spacing::S2))
                            .rounded(px(radius::SMALL))
                            .cursor_pointer()
                            .text_color(color::text_primary())
                            .hover(|s| s.bg(color::state_hover()))
                            .on_click(move |_, _, cx| {
                                player.update(cx, |m, cx| m.retry(cx));
                            })
                            .child("Retry"),
                    )
                })
                .into_any_element(),
            None => text_line(
                "artist",
                v.artist.clone(),
                typography::BODY,
                color::text_secondary(),
                cx,
            )
            .into_any_element(),
        };
        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .flex_1()
            .min_w_0()
            .child(art)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(text_line(
                        "title",
                        v.title.clone(),
                        typography::BODY_STRONG,
                        color::text_primary(),
                        cx,
                    ))
                    .child(artist_line),
            );

        // ---- centre: transport + progress -------------------------------------------
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
        let on_off = |on: bool| {
            if on {
                color::text_primary()
            } else {
                color::text_secondary()
            }
        };
        let (play_icon, play_label) = if v.busy {
            (Icon::Spinner, "Pause")
        } else if v.playing {
            (Icon::Pause, "Pause")
        } else {
            (Icon::Play, "Play")
        };

        let button =
            |id: &'static str, focus: &FocusHandle, icon: Icon, label: &str, enabled: bool| {
                IconButton::new(id, focus, icon, label.to_owned(), accent).enabled(enabled)
            };
        let transport = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(layout::PLAY_BUTTON))
            .child(
                button(
                    "shuffle",
                    &self.focus.shuffle,
                    shuffle_icon,
                    "Shuffle",
                    ready,
                )
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
                    &self.focus.previous,
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
                button("play", &self.focus.play, play_icon, play_label, controls)
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
                button("next", &self.focus.next, Icon::Next, "Next", controls)
                    .icon_color(color::text_primary())
                    .on_click({
                        let p = player.clone();
                        move |_, cx| {
                            p.update(cx, |m, cx| m.next(cx));
                        }
                    }),
            )
            .child(
                button(
                    "repeat",
                    &self.focus.repeat,
                    repeat_icon,
                    repeat_label,
                    ready,
                )
                .icon_color(on_off(repeat_on))
                .on_click({
                    let p = player.clone();
                    move |_, cx| {
                        p.update(cx, |m, cx| m.cycle_repeat(cx));
                    }
                }),
            );

        let duration = v.duration_ms;
        let seekable = controls && duration.is_some_and(|d| d > 0);
        let scrub = self.progress.read(cx).scrubbing();
        let shown_ms = match (scrub, duration) {
            (Some(f), Some(d)) => (f64::from(f) * d as f64) as u64,
            _ => v.position_ms,
        };
        let fraction = duration
            .filter(|d| *d > 0)
            .map_or(0.0, |d| (v.position_ms as f64 / d as f64).min(1.0) as f32);
        let time = |id: &'static str, text: String, right: bool, cx: &App| {
            style::tabular(style::text_style(div(), typography::CAPTION, cx))
                .id(id)
                .w(px(TIME_WIDTH))
                .flex_none()
                .text_color(color::text_tertiary())
                .when(right, |d| d.text_right())
                .child(text)
        };
        let progress = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S2))
            .w_full()
            .child(time("time-position", format_time(shown_ms), true, cx))
            .child(
                div().flex_1().min_w_0().child(
                    Slider::new(
                        "progress",
                        &self.progress,
                        &self.focus.progress,
                        "Seek",
                        accent,
                    )
                    .value(fraction)
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
                "time-duration",
                format_time(duration.unwrap_or(0)),
                false,
                cx,
            ));

        let center = div()
            .flex()
            .flex_col()
            .items_center()
            .flex_none()
            .w(relative(0.4))
            .min_w(px(CENTER_MIN))
            .max_w(px(CENTER_MAX))
            .child(transport)
            .child(progress);

        // ---- right: volume, queue toggle --------------------------------------------
        let volume_value = if v.muted {
            0.0
        } else {
            f32::from(v.volume) / 100.0
        };
        let right = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(spacing::S2))
            .flex_1()
            .min_w_0()
            .child(
                button(
                    "mute",
                    &self.focus.mute,
                    volume_icon(v.volume, v.muted),
                    if v.muted { "Unmute" } else { "Mute" },
                    ready,
                )
                .on_click({
                    let p = player.clone();
                    move |_, cx| {
                        p.update(cx, |m, cx| m.toggle_mute(cx));
                    }
                }),
            )
            .child(
                div().w(px(VOLUME_WIDTH)).flex_none().child(
                    Slider::new("volume", &self.volume, &self.focus.volume, "Volume", accent)
                        .value(volume_value)
                        .enabled(ready)
                        .on_change({
                            let p = player.clone();
                            move |frac, _, _, cx| {
                                let percent = (frac * 100.0).round() as u8;
                                p.update(cx, |m, cx| m.set_volume(percent, cx));
                            }
                        }),
                ),
            )
            .child(
                button(
                    "queue",
                    &self.focus.queue,
                    if self.queue_open {
                        Icon::QueueFilled
                    } else {
                        Icon::QueueRegular
                    },
                    "Queue",
                    ready,
                )
                .icon_color(on_off(self.queue_open))
                .toggled(self.queue_open)
                .on_click({
                    let f = self.on_queue_toggle.clone();
                    move |w, cx| f(w, cx)
                }),
            );

        let _ = window;
        div()
            .id("player-bar")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S4))
            .w_full()
            .h(px(layout::PLAYER_BAR_HEIGHT))
            .px(px(spacing::S4))
            .py(px(spacing::S1))
            .bg(color::bg_raised())
            .child(left)
            .child(center)
            .child(right)
    }
}

/// One truncated line of text; the full text shows in a tooltip.
fn text_line(
    id: &'static str,
    text: SharedString,
    style_: typography::TextStyle,
    text_color: gpui::Hsla,
    cx: &App,
) -> impl IntoElement {
    let full = text.clone();
    style::text_style(div(), style_, cx)
        .id(id)
        .truncate()
        .text_color(text_color)
        .when(!text.is_empty(), move |d| {
            d.tooltip(move |_, cx| Tooltip::view(full.clone(), cx))
        })
        .child(text)
}
