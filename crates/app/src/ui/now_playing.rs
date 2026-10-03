//! Now Playing: the expanded view of the current track, opened from the player bar. Large art
//! over the ambient glow (blurred artwork, the only place the direction allows it), the title
//! at 40px, the artist, then the progress row and transport.

use std::rc::Rc;

use apollo_theme::tokens::{ambient, layout, spacing, typography};
use gpui::{
    div, img, prelude::*, px, App, Context, Entity, FocusHandle, Focusable, ImageSource,
    IntoElement, ObjectFit, ParentElement, Render, Styled, Window,
};

use crate::assets::Icon;
use crate::player::PlayerModel;
use crate::style::color;
use crate::ui::art_tile::art_tile_fluid;
use crate::ui::button::{Handler, IconButton};
use crate::ui::player_bar::text_line;
use crate::ui::slider::SliderState;
use crate::ui::transport::{self, TransportFocus};

/// Tab order: after the nav rail, before the queue panel and the player bar.
const TAB_CLOSE: isize = 6;
const TAB_TRANSPORT: isize = 7;

/// The glow sits in the middle of the direction's 12-18% range.
pub const AMBIENT_OPACITY: f32 = (ambient::OPACITY_MIN + ambient::OPACITY_MAX) / 2.0;

/// Art is `min(50vh, 480px)`, and never wider than the room it has.
pub fn art_side(viewport_height: f32) -> f32 {
    (viewport_height * 0.5).min(layout::NOW_PLAYING_ART_MAX)
}

pub struct NowPlaying {
    player: Entity<PlayerModel>,
    focus_close: FocusHandle,
    focus: TransportFocus,
    progress: Entity<SliderState>,
    on_close: Rc<Handler>,
}

impl NowPlaying {
    pub fn new(
        player: Entity<PlayerModel>,
        on_close: impl Fn(&mut Window, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        let stop = |cx: &mut Context<Self>, index: isize| {
            cx.focus_handle().tab_index(index).tab_stop(true)
        };
        let progress = cx.new(|_| SliderState::default());
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&progress, |_, _, cx| cx.notify()).detach();
        Self {
            focus_close: stop(cx, TAB_CLOSE),
            focus: TransportFocus {
                shuffle: stop(cx, TAB_TRANSPORT),
                previous: stop(cx, TAB_TRANSPORT + 1),
                play: stop(cx, TAB_TRANSPORT + 2),
                next: stop(cx, TAB_TRANSPORT + 3),
                repeat: stop(cx, TAB_TRANSPORT + 4),
                progress: stop(cx, TAB_TRANSPORT + 5),
            },
            player,
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

        let text = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .child(text_line(
                "np-title",
                v.title.clone(),
                typography::NOW_PLAYING_TITLE,
                color::text_primary(),
                cx,
            ))
            .child(text_line(
                "np-artist",
                v.artist.clone(),
                typography::SECTION,
                color::text_secondary(),
                cx,
            ));

        let controls = div()
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
            .on_click(move |window, cx| on_close(window, cx))
        };

        div()
            .id("now-playing")
            .relative()
            .size_full()
            .overflow_hidden()
            .children(glow)
            .child(
                div()
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
                            .py(px(spacing::S4))
                            .child(art)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(spacing::S2))
                                    .w_full()
                                    .max_w(px(side))
                                    .child(text)
                                    .child(controls),
                            ),
                    ),
            )
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
    fn art_is_half_the_window_height_capped_at_480() {
        assert_eq!(art_side(480.0), 240.0);
        assert_eq!(art_side(720.0), 360.0);
        assert_eq!(art_side(960.0), 480.0);
        assert_eq!(art_side(1400.0), 480.0);
    }

    #[test]
    fn the_glow_opacity_is_inside_the_direction_range() {
        assert!((ambient::OPACITY_MIN..=ambient::OPACITY_MAX).contains(&AMBIENT_OPACITY));
    }
}
