//! Bottom player bar (80px, bg.raised). Left: art and title/artist. Centre: shuffle, previous,
//! play/pause, next, repeat over the progress bar. Right: volume, the mini player button, the
//! lyrics toggle and the queue toggle.

use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use gpui::{
    div, prelude::*, px, relative, App, Entity, FocusHandle, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::player::{failure_text, PlayerModel, PlayerView, Status};
use crate::style::{self, color};
use crate::ui::art_tile::art_tile;
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::row_actions::{heart_icon, heart_label};
use crate::ui::slider::{Slider, SliderState};
use crate::ui::tooltip::Tooltip;
use crate::ui::transport::{self, TransportFocus};

const VOLUME_WIDTH: f32 = 96.0;
/// The slider narrows to this when the right block is at its minimum width and holds four
/// buttons beside it (mini player, lyrics, queue, heart and mute at 32px, with their gaps).
const VOLUME_MIN_WIDTH: f32 = 40.0;
/// The centre column takes what the side blocks leave, between these bounds. (A fixed 40% plus
/// two 30% sides plus the gaps overflowed the bar by the gaps' width.)
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
    pub lyrics: FocusHandle,
    pub mini: FocusHandle,
    pub like: FocusHandle,
    pub progress: FocusHandle,
    pub volume: FocusHandle,
    /// Replaces the progress row while an error shows; shares its place in the tab order.
    pub retry: FocusHandle,
    /// The art and title block, which opens Now Playing.
    pub now_playing: FocusHandle,
}

impl BarFocus {
    pub fn transport(&self) -> TransportFocus {
        TransportFocus {
            shuffle: self.shuffle.clone(),
            previous: self.previous.clone(),
            play: self.play.clone(),
            next: self.next.clone(),
            repeat: self.repeat.clone(),
            progress: self.progress.clone(),
        }
    }
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
    /// Lyrics are on screen (Now Playing is open with lyrics chosen).
    pub lyrics_open: bool,
    pub on_lyrics_toggle: Rc<Handler>,
    /// Opens the mini player window.
    pub on_mini_player: Rc<Handler>,
    pub now_playing_open: bool,
    pub on_now_playing_toggle: Rc<Handler>,
    /// The playing track is liked (what the heart shows).
    pub liked: bool,
    pub on_like: Rc<Handler>,
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
        let art = art_tile(layout::PLAYER_BAR_ART, v.art.clone());

        let player = self.player.clone();
        let artist_line = text_line(
            "artist",
            v.artist.clone(),
            typography::BODY,
            color::text_secondary(),
            cx,
        );
        // The block opens (and closes) Now Playing. Inert while nothing is queued.
        let ring = controls && keyboard_focused(&self.focus.now_playing, window);
        let left = div()
            .id("now-playing-toggle")
            .debug_selector(|| "now-playing-toggle".into())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .w(relative(layout::PLAYER_BAR_SIDE_FRACTION))
            .min_w(px(layout::PLAYER_BAR_SIDE_MIN))
            .max_w(px(layout::PLAYER_BAR_SIDE_MAX))
            .rounded(px(radius::MEDIUM))
            .track_focus(&self.focus.now_playing.clone().tab_stop(controls))
            .role(gpui::Role::Button)
            .aria_label("Now playing")
            .aria_toggled(if self.now_playing_open {
                gpui::Toggled::True
            } else {
                gpui::Toggled::False
            })
            .when(controls, |d| {
                let toggle = self.on_now_playing_toggle.clone();
                d.cursor_pointer()
                    .hover(|s| s.bg(color::state_hover()))
                    .on_click(move |event, window, cx| {
                        // Space is the global play/pause key.
                        if let gpui::ClickEvent::Keyboard(k) = event {
                            if k.button == gpui::KeyboardButton::Space {
                                return;
                            }
                        }
                        toggle(window, cx)
                    })
            })
            .when(ring, |d| d.child(focus_ring(accent_hsla, radius::MEDIUM)))
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
        let transport_row = transport::buttons("", &player, &v, &self.focus.transport());

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
        let progress =
            transport::progress("", &player, &v, &self.focus.progress, &self.progress, cx);

        let center = div()
            .debug_selector(|| "player-bar-center".into())
            .flex()
            .flex_col()
            .items_center()
            .flex_1()
            .min_w(px(CENTER_MIN))
            .max_w(px(CENTER_MAX))
            .child(transport_row)
            .child(match error_row {
                Some(row) => row.into_any_element(),
                None => progress.into_any_element(),
            });

        // ---- right: volume, queue toggle --------------------------------------------
        let button =
            |id: &'static str, focus: &FocusHandle, icon: Icon, label: &str, enabled: bool| {
                IconButton::new(id, focus, icon, label.to_owned(), accent).enabled(enabled)
            };
        let volume_value = if v.muted {
            0.0
        } else {
            f32::from(v.volume) / 100.0
        };
        let right = div()
            .debug_selector(|| "player-bar-right".into())
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(spacing::S2))
            .w(relative(layout::PLAYER_BAR_SIDE_FRACTION))
            .min_w(px(layout::PLAYER_BAR_SIDE_MIN))
            .max_w(px(layout::PLAYER_BAR_SIDE_MAX))
            .child(
                div().debug_selector(|| "bar-like".into()).child(
                    button(
                        "like",
                        &self.focus.like,
                        heart_icon(self.liked),
                        heart_label(self.liked),
                        controls,
                    )
                    .icon_color(transport::on_off(self.liked))
                    .accessible_name("Like")
                    .toggled(self.liked)
                    .on_click({
                        let f = self.on_like.clone();
                        move |w, cx| f(w, cx)
                    }),
                ),
            )
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
                // Gives up to 32px to the buttons when the block is at its minimum width.
                div().w(px(VOLUME_WIDTH)).min_w(px(VOLUME_MIN_WIDTH)).flex_shrink_1().child(
                    Slider::new("volume", &self.volume, &self.focus.volume, "Volume", accent)
                        .value(volume_value)
                        .value_text(format!("{}%", (volume_value * 100.0).round() as u32))
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
                    "mini-player",
                    &self.focus.mini,
                    Icon::PictureInPicture,
                    "Mini player",
                    ready,
                )
                .on_click({
                    let f = self.on_mini_player.clone();
                    move |w, cx| f(w, cx)
                }),
            )
            .child(
                button(
                    "lyrics",
                    &self.focus.lyrics,
                    if self.lyrics_open {
                        Icon::LyricsFilled
                    } else {
                        Icon::LyricsRegular
                    },
                    "Lyrics",
                    ready,
                )
                .icon_color(transport::on_off(self.lyrics_open))
                .toggled(self.lyrics_open)
                .on_click({
                    let f = self.on_lyrics_toggle.clone();
                    move |w, cx| f(w, cx)
                }),
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
                .icon_color(transport::on_off(self.queue_open))
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
pub(crate) fn text_line(
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
