//! Now Playing: the expanded view of the current track, opened from the player bar. Large art
//! over the ambient glow (blurred artwork, the only place the direction allows it), the title
//! at 40px, the artist, then the progress row and transport.

use std::rc::Rc;

use apollo_theme::tokens::{ambient, layout, lyrics, spacing, typography};
use gpui::{
    div, img, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, ImageSource,
    IntoElement, ObjectFit, ParentElement, Render, Styled, Window,
};

use crate::assets::Icon;
use crate::library::writes::LibraryWrites;
use crate::player::PlayerModel;
use crate::style::color;
use crate::ui::anchor;
use crate::ui::art_tile::art_tile_fluid;
use crate::ui::button::{Handler, IconButton};
use crate::ui::lyrics_pane::LyricsPane;
use crate::ui::player_bar::text_line;
use crate::ui::playlist_picker::PlaylistPicker;
use crate::ui::row_actions::{heart_icon, heart_label};
use crate::ui::slider::SliderState;
use crate::ui::transport::{self, on_off, TransportFocus};

/// Tab order: after the nav rail, before the queue panel and the player bar.
/// Where the add-to-playlist popover of Now Playing is anchored.
const PLAYLIST_ANCHOR: &str = "np-playlist-anchor";
const TAB_CLOSE: isize = 6;
const TAB_TRANSPORT: isize = 7;

/// The glow sits in the middle of the direction's 12-18% range.
pub const AMBIENT_OPACITY: f32 = (ambient::OPACITY_MIN + ambient::OPACITY_MAX) / 2.0;

/// Line heights of the title and artist lines. Both are single lines, so a tight height costs
/// nothing visually and saves what the default (1.6x) leading would take at the smallest window.
const TITLE_LINE_HEIGHT: f32 = 1.25;
const ARTIST_LINE_HEIGHT: f32 = 1.4;
/// The text and controls column is at least this wide, even over a small cover.
const MIN_COLUMN: f32 = 320.0;
/// Everything on the page except the art: the page's top and bottom padding (2 x 8), the gap
/// under the art (16), the title (50), the 8px gap and the artist line (25.2), the 8px gap
/// above the controls and the progress row with the transport (80). The layout test keeps the
/// transport inside the page at the smallest window.
const BELOW_ART: f32 = 204.0;

/// Art is `min(50vh, 480px)`, or the height left under the title bar and above the player bar
/// once the title, artist and controls have theirs if that is less, and never below 160px: the
/// transport and progress row stay on screen without scrolling.
pub fn art_side(viewport_height: f32) -> f32 {
    let page = viewport_height - layout::TITLE_BAR_HEIGHT - layout::PLAYER_BAR_HEIGHT;
    (viewport_height * 0.5)
        .min(layout::NOW_PLAYING_ART_MAX)
        .min(page - BELOW_ART)
        .max(layout::NOW_PLAYING_ART_MIN)
}

pub struct NowPlaying {
    player: Entity<PlayerModel>,
    writes: Entity<LibraryWrites>,
    picker: Entity<PlaylistPicker>,
    /// Takes the right side of the page when it is open.
    lyrics: Entity<LyricsPane>,
    focus_close: FocusHandle,
    focus_like: FocusHandle,
    focus_playlist: FocusHandle,
    focus: TransportFocus,
    progress: Entity<SliderState>,
    on_close: Rc<Handler>,
}

impl NowPlaying {
    pub fn new(
        player: Entity<PlayerModel>,
        writes: Entity<LibraryWrites>,
        picker: Entity<PlaylistPicker>,
        lyrics: Entity<LyricsPane>,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        let progress = cx.new(|_| SliderState::default());
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&writes, |_, _, cx| cx.notify()).detach();
        cx.observe(&progress, |_, _, cx| cx.notify()).detach();
        cx.observe(&lyrics, |_, _, cx| cx.notify()).detach();
        Self {
            focus_close: stop(cx, TAB_CLOSE),
            // Same slot as the transport: they sit above it, so they come first.
            focus_like: stop(cx, TAB_TRANSPORT),
            focus_playlist: stop(cx, TAB_TRANSPORT),
            focus: TransportFocus {
                shuffle: stop(cx, TAB_TRANSPORT),
                previous: stop(cx, TAB_TRANSPORT + 1),
                play: stop(cx, TAB_TRANSPORT + 2),
                next: stop(cx, TAB_TRANSPORT + 3),
                repeat: stop(cx, TAB_TRANSPORT + 4),
                progress: stop(cx, TAB_TRANSPORT + 5),
            },
            player,
            writes,
            picker,
            lyrics,
            progress,
            on_close: Rc::new(Box::new(on_close)),
        }
    }
}

impl Focusable for NowPlaying {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_close.clone()
    }
}

impl Render for NowPlaying {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.player.read(cx).view();
        let side = art_side(f32::from(window.viewport_size().height));

        let glow = v.ambient.clone().map(|image| {
            // Absolute and drawn first, so it sits behind everything else.
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .opacity(AMBIENT_OPACITY)
                .child(
                    img(ImageSource::Render(image))
                        .size_full()
                        .object_fit(ObjectFit::Cover),
                )
        });

        let art = art_tile_fluid(side, v.art.clone())
            .id("now-playing-art")
            .debug_selector(|| "now-playing-art".into())
            .w(px(side))
            .max_w_full()
            .aspect_square()
            .flex_none();

        // Like and add to playlist sit at the end of the title line, so the page keeps its
        // height. Inert while nothing is queued.
        let video = self.player.read(cx).current_video();
        let liked = video
            .as_deref()
            .is_some_and(|id| self.writes.read(cx).is_liked(id));
        let actions = video.clone().filter(|_| v.has_track).map(|video| {
            let like = IconButton::new(
                "np-like",
                &self.focus_like,
                heart_icon(liked),
                heart_label(liked),
                v.accent,
            )
            .icon_color(on_off(liked))
            .accessible_name("Like")
            .toggled(liked)
            .on_click({
                let (writes, video) = (self.writes.clone(), video.clone());
                move |_, cx| writes.update(cx, |w, cx| w.toggle_like(&video, cx))
            });
            let add = IconButton::new(
                "np-playlist",
                &self.focus_playlist,
                Icon::PlaylistAdd,
                "Add to playlist",
                v.accent,
            )
            .on_click({
                let (picker, title) = (self.picker.clone(), v.title.clone());
                move |window, cx| {
                    picker.update(cx, |p, cx| {
                        p.open_at(&video, title.clone(), PLAYLIST_ANCHOR, window, cx)
                    })
                }
            });
            let add = anchor::anchored(PLAYLIST_ANCHOR, add);
            div()
                .flex()
                .flex_row()
                .flex_none()
                .gap(px(spacing::S1))
                .child(div().debug_selector(|| "np-like".into()).child(like))
                .child(div().debug_selector(|| "np-playlist".into()).child(add))
        });

        let text = div()
            .debug_selector(|| "np-text".into())
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(spacing::S3))
                    .line_height(gpui::relative(TITLE_LINE_HEIGHT))
                    .child(div().flex_1().min_w_0().child(text_line(
                        "np-title",
                        v.title.clone(),
                        typography::NOW_PLAYING_TITLE,
                        color::text_primary(),
                        cx,
                    )))
                    .children(actions),
            )
            .child(
                div()
                    .mt(px(spacing::S2))
                    .line_height(gpui::relative(ARTIST_LINE_HEIGHT))
                    .child(text_line(
                        "np-artist",
                        v.subtitle.clone(),
                        typography::NOW_PLAYING_ARTIST,
                        color::text_secondary(),
                        cx,
                    )),
            );

        let controls = div()
            .debug_selector(|| "np-controls".into())
            .flex()
            .flex_col()
            .items_center()
            .gap(px(spacing::S2))
            .w_full()
            .child(transport::progress(
                "np-",
                &self.player,
                &v,
                &self.focus.progress,
                &self.progress,
                cx,
            ))
            .child(transport::buttons("np-", &self.player, &v, &self.focus));

        let close = {
            let on_close = self.on_close.clone();
            IconButton::new(
                "np-close",
                &self.focus_close,
                Icon::ChevronDown,
                "Close",
                v.accent,
            )
            // The window's own Close button is also on screen.
            .accessible_name("Close Now Playing")
            .on_click(move |window, cx| on_close(window, cx))
        };

        // The cover, title and controls, centred in their column.
        let column = div()
            .id("now-playing-scroll")
            .size_full()
            .overflow_y_scroll()
            .child(
                div()
                    .min_h_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(spacing::S4))
                    .px(px(spacing::S8))
                    .py(px(spacing::S2))
                    .child(art)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(spacing::S2))
                            .w_full()
                            .max_w(px(side.max(MIN_COLUMN)))
                            .child(text)
                            .child(controls),
                    ),
            );
        // With lyrics open the page is two halves: the column on the left, the lyrics where the
        // queue would be on the right.
        let page = if self.lyrics.read(cx).is_open() {
            div()
                .size_full()
                .flex()
                .flex_row()
                .child(div().flex_1().min_w_0().h_full().child(column))
                .child(
                    div()
                        .debug_selector(|| "np-lyrics".into())
                        .flex_1()
                        .min_w(px(lyrics::MIN_PANE_WIDTH))
                        .h_full()
                        .child(self.lyrics.clone()),
                )
                .into_any_element()
        } else {
            column.into_any_element()
        };

        div()
            .id("now-playing")
            .relative()
            .size_full()
            .overflow_hidden()
            .children(glow)
            .child(page)
            .child(
                div()
                    .absolute()
                    .top(px(spacing::S4))
                    .left(px(spacing::S4))
                    .child(close),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn art_is_half_the_window_height_capped_at_480_and_at_160_or_more() {
        assert_eq!(art_side(720.0), 360.0);
        assert_eq!(art_side(960.0), 480.0);
        assert_eq!(art_side(1400.0), 480.0);
        // A short window gives the art what is left under the text and controls, down to 160.
        assert_eq!(art_side(480.0), 164.0);
        assert!(art_side(560.0) > 160.0 && art_side(560.0) < 280.0);
    }

    #[test]
    fn the_glow_opacity_is_inside_the_direction_range() {
        assert!((ambient::OPACITY_MIN..=ambient::OPACITY_MAX).contains(&AMBIENT_OPACITY));
    }
}
