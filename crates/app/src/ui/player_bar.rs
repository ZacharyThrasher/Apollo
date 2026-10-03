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
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::slider::{Slider, SliderState};
use crate::ui::tooltip::Tooltip;

/// Seek step for the keyboard on the progress bar.
const SEEK_STEP_MS: f64 = 5_000.0;
const VOLUME_WIDTH: f32 = 96.0;
const TIME_WIDTH: f32 = spacing::S12;
/// The centre column takes 40% of the bar, between these bounds.
const CENTER_MAX: f32 = 640.0;
const CENTER_MIN: f32 = 320.0;
/// Smallest window width at which both side blocks keep their minimum and the centre fits:
/// two sides, the centre, the bar's side padding and the gaps between the columns.
pub const MIN_BAR_WIDTH: f32 =
    2.0 * layout::PLAYER_BAR_SIDE_MIN + CENTER_MIN + 2.0 * spacing::S4 + 2.0 * spacing::S4;

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
    /// Replaces the progress row while an error shows; shares its place in the tab order.
    pub retry: FocusHandle,
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
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius::MEDIUM))
            .overflow_hidden()
            .bg(color::bg_overlay())
            .map(|d| match v.art.clone() {
                Some(image) => d.child(
                    img(ImageSource::Render(image))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                ),
                // No artwork: a quiet music note. A decorative glyph, not text, so
                // text.tertiary on bg.overlay (4.19:1) clears the 3:1 non-text minimum.
                None => d.child(
                    gpui::svg()
                        .path(Icon::MusicNote.path())
                        .size(px(layout::ICON_SIZE))
                        .flex_none()
                        .text_color(color::text_tertiary()),
                ),
            });

        let player = self.player.clone();
        let artist_line = text_line(
            "artist",
            v.artist.clone(),
            typography::BODY,
            color::text_secondary(),
            cx,
        );
        let left = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .flex_none()
            .w(relative(layout::PLAYER_BAR_SIDE_FRACTION))
            .min_w(px(layout::PLAYER_BAR_SIDE_MIN))
            .max_w(px(layout::PLAYER_BAR_SIDE_MAX))
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
        let error_text: Option<SharedString> = match (&v.status, v.error) {
            (Status::Unavailable(text), _) => Some(text.clone()),
            (_, Some(failure)) => Some(SharedString::from(failure_text(failure))),
            _ => None,
        };
        // While an error shows, one row replaces the times and the slider: a warning icon,
        // the short reason, and Retry when retrying makes sense.
        let error_row = error_text.clone().map(|text| {
            let retry = v.error.is_some().then(|| {
                let focus = self.focus.retry.clone();
                let ring = keyboard_focused(&focus, window);
                let player = player.clone();
                style::text_style(div(), typography::BODY_STRONG, cx)
                    .id("retry")
                    .relative()
                    .flex_none()
                    .px(px(spacing::S2))
                    .rounded(px(radius::SMALL))
                    .cursor_pointer()
                    .track_focus(&focus)
                    .role(gpui::Role::Button)
                    .aria_label("Retry")
                    .text_color(color::text_primary())
                    .hover(|s| s.underline())
                    .on_click(move |event, _, cx| {
                        // Space is the global play/pause key.
                        if let gpui::ClickEvent::Keyboard(k) = event {
                            if k.button == gpui::KeyboardButton::Space {
                                return;
                            }
                        }
                        player.update(cx, |m, cx| m.retry(cx));
                    })
                    .child("Retry")
                    .when(ring, |d| d.child(focus_ring(accent_hsla, radius::SMALL)))
            });
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .gap(px(spacing::S2))
                .w_full()
                .h(px(crate::ui::slider::HIT_HEIGHT))
                .child(
                    gpui::svg()
                        .path(Icon::Warning.path())
                        .size(px(layout::ICON_SIZE))
                        .flex_none()
                        .text_color(color::warning()),
                )
                .child(
                    style::text_style(div(), typography::BODY, cx)
                        .id("error-text")
                        .truncate()
                        .text_color(color::text_secondary())
                        .child(text),
                )
                .children(retry)
        });
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
            .child(match error_row {
                Some(row) => row.into_any_element(),
                None => progress.into_any_element(),
            });

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
            .flex_none()
            .w(relative(layout::PLAYER_BAR_SIDE_FRACTION))
            .min_w(px(layout::PLAYER_BAR_SIDE_MIN))
            .max_w(px(layout::PLAYER_BAR_SIDE_MAX))
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
                        .fill(color::text_secondary())
                        .enabled(ready)
                        .on_change({
                            let p = player.clone();
                            move |frac, committed, _, cx| {
                                let percent = (frac * 100.0).round() as u8;
                                p.update(cx, |m, cx| {
                                    m.set_volume(percent, cx);
                                    if committed {
                                        m.save_volume();
                                    }
                                });
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

        div()
            .id("player-bar")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            // Equal side blocks keep the transport centred on the window.
            .justify_between()
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
