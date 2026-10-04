//! The Top result card on Search's All view: the service's best match for the query, on a wide
//! bg.raised 8px card. Artwork 96px (a circle for an artist), the name at 22px semibold, the
//! kind and artist in text.secondary, and a Play button.
//!
//! The artwork and text are one control (Enter or a click opens an album, artist or playlist
//! page and plays a song); the Play button is a second, separate stop.

use std::rc::Rc;
use std::time::Instant;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{div, prelude::*, px, App, FocusHandle, SharedString, Window};

use crate::assets::Icon;
use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::card::{shaped_tile, OpenCard, KEY_CONTEXT};
use crate::ui::tooltip::Tooltip;
use crate::ui::track_menu::{self, Subject};
use crate::url_thumbs::Thumb;

/// The card's width: wide enough for a long name, narrow enough that the song list beside it
/// keeps most of a minimum-width window.
pub const WIDTH: f32 = 340.0;
/// The artwork square.
pub const ART: f32 = 96.0;

type Action = Rc<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct TopCard {
    title: SharedString,
    /// "Song • Artist": the kind, then what the service says after it.
    detail: SharedString,
    thumb: Thumb,
    circle: bool,
    focus: FocusHandle,
    play_focus: FocusHandle,
    accent: Accent,
    on_open: Action,
    on_play: Action,
    on_intent: Option<Rc<dyn Fn(&mut App) + 'static>>,
    menu: Option<Subject>,
}

impl TopCard {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        thumb: Thumb,
        focus: &FocusHandle,
        play_focus: &FocusHandle,
        accent: Accent,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
        on_play: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            thumb,
            circle: false,
            focus: focus.clone(),
            play_focus: play_focus.clone(),
            accent,
            on_open: Rc::new(on_open),
            on_play: Rc::new(on_play),
            on_intent: None,
            menu: None,
        }
    }

    pub fn circular(mut self, circle: bool) -> Self {
        self.circle = circle;
        self
    }

    pub fn menu(mut self, subject: Option<Subject>) -> Self {
        self.menu = subject;
        self
    }

    /// Runs when the pointer arrives on the card (see `PlayerModel::hint_play`).
    pub fn on_intent(mut self, f: impl Fn(&mut App) + 'static) -> Self {
        self.on_intent = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for TopCard {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state("top-card", cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        let play_fade = window.use_keyed_state("top-card-play", cx, |_, _| HoverFade::default());
        let (play_level, play_moving) = play_fade.read(cx).level(Instant::now());
        if moving || play_moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let play_ring = keyboard_focused(&self.play_focus, window);
        let accent = color::accent(self.accent);
        let accent_fg = color::accent_fg(self.accent);
        let (open, open_key, play) = (self.on_open.clone(), self.on_open, self.on_play);
        let on_intent = self.on_intent;
        let (title, detail) = (self.title, self.detail);
        let label: SharedString = format!("{title}, {detail}").into();
        let menu = self.menu;

        let info = div()
            .id("top-card")
            .debug_selector(|| "top-card".into())
            .relative()
            .flex()
            .flex_col()
            .gap(px(spacing::S1))
            .p(px(spacing::S2))
            .rounded(px(radius::MEDIUM))
            .bg(color::state_hover().opacity(level))
            .cursor_pointer()
            .track_focus(&self.focus)
            .role(gpui::Role::Button)
            .aria_label(label)
            .on_hover(move |hovered, _, cx| {
                if *hovered {
                    if let Some(f) = &on_intent {
                        f(cx);
                    }
                }
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animations);
                    cx.notify();
                });
            })
            .key_context(KEY_CONTEXT)
            .on_action(move |_: &OpenCard, window, cx| open_key(window, cx))
            .on_click(move |event, window, cx| {
                // The keyboard goes through `OpenCard`; Space is the play/pause key.
                if !matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    open(window, cx);
                }
            })
            .child(
                div()
                    .size(px(ART))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| {
                        if self.circle {
                            d.rounded_full()
                        } else {
                            d.rounded(px(radius::MEDIUM))
                        }
                    })
                    .overflow_hidden()
                    .bg(color::bg_overlay())
                    .child(shaped_tile(&self.thumb, self.circle)),
            )
            .child(
                style::text_style(div(), typography::TOP_RESULT_TITLE, cx)
                    .id("top-card-title")
                    .mt(px(spacing::S2))
                    .truncate()
                    .text_color(color::text_primary())
                    .tooltip({
                        let full = title.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(title),
            )
            .child(
                style::text_style(div(), typography::BODY, cx)
                    .id("top-card-detail")
                    .truncate()
                    .text_color(color::text_secondary())
                    .tooltip({
                        let full = detail.clone();
                        move |_, cx| Tooltip::view(full.clone(), cx)
                    })
                    .child(detail),
            )
            .when(ring, move |d| d.child(focus_ring(accent, radius::MEDIUM)))
            .map(move |d| match menu {
                Some(subject) => track_menu::on_context_menu(d, subject),
                None => d,
            });

        let play_button = style::text_style(div(), typography::BODY_STRONG, cx)
            .id("top-card-play")
            .debug_selector(|| "top-card-play".into())
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S1))
            .h(px(layout::MIN_HIT_TARGET))
            .pl(px(spacing::S3))
            .pr(px(spacing::S4))
            .rounded_full()
            .bg(accent)
            .text_color(accent_fg)
            .cursor_pointer()
            .track_focus(&self.play_focus)
            .role(gpui::Role::Button)
            .aria_label("Play")
            .on_hover(move |hovered, _, cx| {
                play_fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animations);
                    cx.notify();
                });
            })
            .on_click(move |event, window, cx| {
                // Space is the global play/pause key, so Enter alone presses it.
                if let gpui::ClickEvent::Keyboard(k) = event {
                    if k.button == gpui::KeyboardButton::Space {
                        return;
                    }
                }
                play(window, cx);
            })
            .child(
                div()
                    .absolute()
                    .size_full()
                    .top_0()
                    .left_0()
                    .rounded_full()
                    .bg(color::state_hover().opacity(play_level)),
            )
            .child(
                gpui::svg()
                    .path(Icon::Play.path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(accent_fg),
            )
            .child("Play")
            .when(play_ring, move |d| {
                d.child(focus_ring(accent, layout::MIN_HIT_TARGET / 2.0))
            });

        div()
            .w(px(WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(spacing::S2))
            .p(px(spacing::S2))
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_raised())
            .child(info)
            .child(
                div()
                    .pl(px(spacing::S2))
                    .pb(px(spacing::S2))
                    .child(play_button),
            )
    }
}
