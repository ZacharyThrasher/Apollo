//! The lyrics pane of Now Playing: the right side of the page where the queue would be.
//!
//! Synced lyrics are a column of lines; the current one is text.primary, the others
//! text.secondary at reduced opacity. The current line sits a third of the way down and the
//! column follows it (200 ms, instant under reduced motion). Clicking a line seeks to it; a
//! manual scroll pauses following until a line is clicked or 4 s pass. Plain lyrics are static,
//! scrollable text. No lyrics is one centred line; loading is the shared skeleton pulse.

use std::sync::Arc;
use std::time::Instant;

use apollo_lyrics::{Line, Lyrics};
use apollo_theme::tokens::{lyrics as tokens, motion, radius, spacing, typography};
use gpui::{
    div, point, prelude::*, px, relative, Context, Entity, IntoElement, ParentElement, Render,
    ScrollHandle, SharedString, Styled, Window,
};

use crate::lyrics::{current_line, LyricsModel, LyricsState};
use crate::player::PlayerModel;
use crate::settings::LyricsOpenStore;
use crate::style::{self, color};
use crate::ui::skeleton;

/// Line height as a multiple of the 22px type: generous, because lines are read at a glance.
const LINE_HEIGHT: f32 = 1.3;
/// Space above and below each line (the gap between two lines is twice this).
const LINE_PAD: f32 = spacing::S2;
/// How often a pause after manual scrolling checks whether it is over.
const HOLD_TICK: std::time::Duration = std::time::Duration::from_millis(500);
/// Skeleton bars shown while loading, as a share of the pane's width.
const SKELETON_WIDTHS: [f32; 6] = [0.9, 0.7, 0.85, 0.6, 0.8, 0.5];

/// A scroll in progress: from one offset to another over the lyrics scroll duration.
struct Glide {
    from: f32,
    to: f32,
    started: Instant,
}

pub struct LyricsPane {
    player: Entity<PlayerModel>,
    lyrics: Entity<LyricsModel>,
    store: Option<Arc<dyn LyricsOpenStore>>,
    /// Lyrics take the right side of Now Playing.
    open: bool,
    scroll: ScrollHandle,
    /// The track and line the column was last steered to.
    followed: Option<(String, Option<usize>)>,
    /// The column should move to the current line when its layout is known.
    need_scroll: bool,
    /// ... and jump there rather than glide (a new track, or the pane just appeared).
    snap: bool,
    glide: Option<Glide>,
    /// Ticks since the last manual scroll while following is paused.
    hold_ticks: Option<u32>,
    hold_task: bool,
}

impl LyricsPane {
    pub fn new(
        player: Entity<PlayerModel>,
        lyrics: Entity<LyricsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        cx.observe(&lyrics, |_, _, cx| cx.notify()).detach();
        Self {
            player,
            lyrics,
            store: None,
            open: false,
            scroll: ScrollHandle::new(),
            followed: None,
            need_scroll: true,
            snap: true,
            glide: None,
            hold_ticks: None,
            hold_task: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Where the choice is remembered between runs (none in tests).
    pub fn set_store(&mut self, store: Option<Arc<dyn LyricsOpenStore>>) {
        self.store = store;
    }

    /// Apply the remembered choice at startup. Does not save it again.
    pub fn restore_open(&mut self, saved: Option<bool>, cx: &mut Context<Self>) {
        if let Some(open) = saved {
            self.open = open;
            self.reset_follow();
            cx.notify();
        }
    }

    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.open == open {
            return;
        }
        self.open = open;
        self.reset_follow();
        if let Some(store) = self.store.clone() {
            std::thread::spawn(move || store.save(open));
        }
        cx.notify();
    }

    /// The column was off screen: when it is next drawn it starts on the current line.
    pub fn reset_follow(&mut self) {
        self.snap = true;
        self.need_scroll = true;
        self.glide = None;
        self.hold_ticks = None;
    }

    /// The pointer wheel moved the column: stop following for a while.
    fn user_scrolled(&mut self, cx: &mut Context<Self>) {
        self.glide = None;
        self.hold_ticks = Some(0);
        if self.hold_task {
            return;
        }
        self.hold_task = true;
        let ticks_to_release = (tokens::MANUAL_HOLD.as_millis() / HOLD_TICK.as_millis()) as u32;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(HOLD_TICK).await;
            let done = this.update(cx, |this, cx| match this.hold_ticks {
                Some(n) if n + 1 >= ticks_to_release => {
                    this.hold_ticks = None;
                    this.hold_task = false;
                    this.need_scroll = true;
                    cx.notify();
                    true
                }
                Some(n) => {
                    this.hold_ticks = Some(n + 1);
                    false
                }
                None => {
                    this.hold_task = false;
                    true
                }
            });
            if done.unwrap_or(true) {
                break;
            }
        })
        .detach();
    }

    /// A line was clicked: seek to it and follow again at once.
    fn seek_to(&mut self, start_ms: u64, cx: &mut Context<Self>) {
        self.hold_ticks = None;
        self.need_scroll = true;
        self.player.update(cx, |m, cx| m.seek(start_ms, cx));
        cx.notify();
    }

    /// Steer the column to `line`. Called while drawing, the only place the window is at hand;
    /// it moves the scroll offset (gliding, snapping, or waiting a frame for the layout).
    fn follow(
        &mut self,
        video: &str,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = Instant::now();
        let key = (video.to_owned(), line);
        if self.followed.as_ref() != Some(&key) {
            if self.followed.as_ref().map(|(v, _)| v.as_str()) != Some(video) {
                self.snap = true;
            }
            self.followed = Some(key);
            self.need_scroll = true;
        }
        let instant = self.snap || cx.reduce_motion();
        if self.need_scroll && self.hold_ticks.is_none() {
            match self.target_offset(line) {
                Some(to) if instant => {
                    self.scroll.set_offset(point(px(0.0), px(to)));
                    self.glide = None;
                    self.need_scroll = false;
                    self.snap = false;
                }
                Some(to) => {
                    let from = f32::from(self.scroll.offset().y);
                    self.glide = ((from - to).abs() > 0.5).then_some(Glide {
                        from,
                        to,
                        started: now,
                    });
                    self.need_scroll = false;
                }
                // Not laid out yet: try again next frame.
                None => window.request_animation_frame(),
            }
        }
        if let Some(glide) = &self.glide {
            let (y, moving) = glide_at(glide, now);
            self.scroll.set_offset(point(px(0.0), px(y)));
            if moving {
                window.request_animation_frame();
            } else {
                self.glide = None;
            }
        }
    }

    /// The scroll offset (negative: scrolled down) that puts `line` a third of the way down,
    /// from the layout of the last frame. `None` until the column has been laid out.
    fn target_offset(&self, line: Option<usize>) -> Option<f32> {
        let view = self.scroll.bounds();
        let height = f32::from(view.size.height);
        if height <= 0.0 {
            return None;
        }
        let Some(line) = line else { return Some(0.0) };
        // Child 0 is the spacer above the first line.
        let bounds = self.scroll.bounds_for_item(line + 1)?;
        // The children's bounds are where layout put them, before the scroll offset moves
        // them, so the distance from the column's top is the offset that would bring the line
        // to the top.
        let line_top = f32::from(bounds.top() - view.top());
        let line_height = f32::from(bounds.size.height);
        let want = line_top - (height * tokens::FOCUS_FRACTION - line_height / 2.0);
        let max = f32::from(self.scroll.max_offset().y).abs();
        Some((-want).clamp(-max, 0.0))
    }

    fn pane_height(&self, window: &Window) -> f32 {
        let measured = f32::from(self.scroll.bounds().size.height);
        if measured > 0.0 {
            measured
        } else {
            // Before the first layout: the page under the title bar and above the player bar.
            let viewport = f32::from(window.viewport_size().height);
            (viewport
                - apollo_theme::tokens::layout::TITLE_BAR_HEIGHT
                - apollo_theme::tokens::layout::PLAYER_BAR_HEIGHT)
                .max(0.0)
        }
    }
}

/// The offset a glide has reached after `now - started`, and whether it is still moving.
fn glide_at(glide: &Glide, now: Instant) -> (f32, bool) {
    let elapsed = now.saturating_duration_since(glide.started);
    if elapsed >= tokens::SCROLL {
        return (glide.to, false);
    }
    let t = motion::ease_out_cubic(elapsed.as_secs_f32() / tokens::SCROLL.as_secs_f32());
    (glide.from + (glide.to - glide.from) * t, true)
}

/// One line's text in the lyrics type.
fn lyric_text(text: impl Into<SharedString>, cx: &gpui::App) -> gpui::Div {
    style::text_style(div(), typography::LYRICS, cx)
        .line_height(relative(LINE_HEIGHT))
        .child(text.into())
}

impl Render for LyricsPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let video = self.player.read(cx).current_video();
        let state = self.lyrics.read(cx).state(video.as_deref());
        let height = self.pane_height(window);
        let top = height * tokens::FOCUS_FRACTION;

        let body = match (&state, video.as_deref()) {
            (LyricsState::Ready(lyrics), Some(video)) => match lyrics.as_ref() {
                Lyrics::Synced(lines) => {
                    let position = self.player.read(cx).view().position_ms;
                    let current = current_line(lines, position);
                    self.follow(video, current, window, cx);
                    self.synced(lines, current, height - top, top, cx)
                }
                Lyrics::Plain(text) => {
                    self.followed = None;
                    self.plain(text, cx)
                }
            },
            (LyricsState::Loading, _) => {
                self.followed = None;
                self.loading(top, !cx.reduce_motion())
            }
            (LyricsState::Missing, _) => {
                self.followed = None;
                div()
                    .debug_selector(|| "lyrics-none".into())
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color::text_secondary())
                    .child(lyric_text("No lyrics", cx))
                    .into_any_element()
            }
            _ => {
                self.followed = None;
                div().size_full().into_any_element()
            }
        };
        div()
            .id("lyrics-pane")
            .debug_selector(|| "lyrics-pane".into())
            .size_full()
            .min_w_0()
            .child(body)
    }
}

impl LyricsPane {
    fn synced(
        &self,
        lines: &[Line],
        current: Option<usize>,
        below: f32,
        above: f32,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let dim = color::text_secondary().opacity(f32::from(tokens::DIM_OPACITY_PCT) / 100.0);
        let rows = lines.iter().enumerate().map(|(i, line)| {
            let is_current = current == Some(i);
            let start_ms = line.start_ms;
            lyric_text(line.text.clone(), cx)
                .id(("lyric", i))
                .debug_selector(move || format!("lyric-{i}"))
                .w_full()
                .py(px(LINE_PAD))
                .rounded(px(radius::SMALL))
                .cursor_pointer()
                .text_color(if is_current {
                    color::text_primary()
                } else {
                    dim
                })
                .when(!is_current, |d| {
                    d.hover(|s| s.text_color(color::text_secondary()))
                })
                .on_click(cx.listener(move |this, _, _, cx| this.seek_to(start_ms, cx)))
        });
        div()
            .id("lyrics-scroll")
            .debug_selector(|| "lyrics-scroll".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .px(px(spacing::S8))
            .on_scroll_wheel(cx.listener(|this, _, _, cx| this.user_scrolled(cx)))
            // Space above the first line and below the last, so either can reach the third.
            .child(div().flex_none().h(px(above)))
            .children(rows)
            .child(div().flex_none().h(px(below)))
            .into_any_element()
    }

    fn plain(&self, text: &str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let blank = typography::LYRICS.size_px * LINE_HEIGHT;
        div()
            .id("lyrics-plain")
            .debug_selector(|| "lyrics-plain".into())
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .px(px(spacing::S8))
            .py(px(spacing::S8))
            .text_color(color::text_secondary())
            .children(text.lines().map(|line| {
                if line.trim().is_empty() {
                    div().flex_none().h(px(blank)).into_any_element()
                } else {
                    lyric_text(line.trim().to_owned(), cx).into_any_element()
                }
            }))
            .into_any_element()
    }

    fn loading(&self, top: f32, animate: bool) -> gpui::AnyElement {
        let bars = SKELETON_WIDTHS.iter().map(|share| {
            div()
                .h(px(typography::LYRICS.size_px))
                .w(relative(*share))
                .rounded(px(radius::SMALL))
                .bg(color::skeleton())
        });
        let column = div()
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .size_full()
            .px(px(spacing::S8))
            .pt(px(top))
            .children(bars);
        skeleton::pulse(
            column.debug_selector(|| "lyrics-skeleton".into()),
            "lyrics-skeleton",
            animate,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_glide_runs_from_start_to_target_over_200_ms_and_eases_out() {
        let started = Instant::now();
        let glide = Glide {
            from: 0.0,
            to: -100.0,
            started,
        };
        assert_eq!(glide_at(&glide, started), (0.0, true));
        let (mid, moving) = glide_at(&glide, started + Duration::from_millis(100));
        assert!(
            moving && mid < -50.0,
            "ease-out covers most of it early: {mid}"
        );
        assert_eq!(glide_at(&glide, started + tokens::SCROLL), (-100.0, false));
        assert_eq!(
            glide_at(&glide, started + Duration::from_secs(5)),
            (-100.0, false)
        );
    }

    #[test]
    fn the_scroll_lasts_the_panel_duration() {
        assert_eq!(tokens::SCROLL, Duration::from_millis(200));
    }
}
