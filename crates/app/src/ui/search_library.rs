//! One album, playlist or artist from the owner's library as a list row, for Search's "In your
//! library" group: artwork (a circle for an artist), the title, and the kind with what the
//! library says after it in text.secondary. A click or Enter opens its page. Songs use the same
//! row as the Songs group; this is the row for the kinds that open pages.

use std::rc::Rc;
use std::time::Instant;

use apollo_theme::tokens::{radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{div, prelude::*, px, App, FocusHandle, SharedString, Window};

use crate::motion::HoverFade;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::card::{shaped_tile, OpenCard, KEY_CONTEXT};
use crate::ui::tooltip::Tooltip;
use crate::ui::track_menu::{self, Subject};
use crate::url_thumbs::Thumb;

pub const ROW_HEIGHT: f32 = 56.0;
pub const ART: f32 = 40.0;

#[derive(IntoElement)]
pub struct LibraryRow {
    /// Unique on its page: names the hover state and the text's tooltips.
    id: SharedString,
    title: SharedString,
    /// "Album \u{2022} Artist \u{2022} 2013": the kind first.
    detail: SharedString,
    thumb: Thumb,
    circle: bool,
    focus: FocusHandle,
    accent: Accent,
    on_open: Rc<dyn Fn(&mut Window, &mut App) + 'static>,
    menu: Option<Subject>,
}

impl LibraryRow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        detail: impl Into<SharedString>,
        thumb: Thumb,
        focus: &FocusHandle,
        accent: Accent,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: detail.into(),
            thumb,
            circle: false,
            focus: focus.clone(),
            accent,
            on_open: Rc::new(on_open),
            menu: None,
        }
    }

    /// An artist's artwork is a circle.
    pub fn circular(mut self, circle: bool) -> Self {
        self.circle = circle;
        self
    }

    /// What a right-click on the row offers (nothing for an artist).
    pub fn menu(mut self, subject: Option<Subject>) -> Self {
        self.menu = subject;
        self
    }
}

impl RenderOnce for LibraryRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let fade = window.use_keyed_state(self.id.clone(), cx, |_, _| HoverFade::default());
        let (level, moving) = fade.read(cx).level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let animations = !cx.reduce_motion();
        let ring = keyboard_focused(&self.focus, window);
        let ring_color = color::accent(self.accent);
        let open = self.on_open;
        let open_key = open.clone();
        let (id, title, detail) = (self.id.clone(), self.title.clone(), self.detail.clone());
        let menu = self.menu.clone();

        div()
            .debug_selector({
                let id = id.to_string();
                move || id.clone()
            })
            .id(id.clone())
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(spacing::S3))
            .h(px(ROW_HEIGHT))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .bg(color::state_hover().opacity(level))
            .cursor_pointer()
            .track_focus(&self.focus)
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("{title}, {detail}")))
            .key_context(KEY_CONTEXT)
            .on_action(move |_: &OpenCard, window, cx| open_key(window, cx))
            .on_hover(move |hovered, _, cx| {
                fade.update(cx, |s, cx| {
                    s.set(*hovered, Instant::now(), animations);
                    cx.notify();
                });
            })
            .on_click(move |event, window, cx| {
                // The keyboard opens a row through `OpenCard`; Space is the play/pause key.
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
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(
                        style::text_style(div(), typography::LIST_TITLE, cx)
                            .id((id.clone(), 0usize))
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
                            .id((id, 1usize))
                            .truncate()
                            .text_color(color::text_secondary())
                            .tooltip({
                                let full = detail.clone();
                                move |_, cx| Tooltip::view(full.clone(), cx)
                            })
                            .child(detail),
                    ),
            )
            .when(ring, move |d| d.child(focus_ring(ring_color, radius::SMALL)))
            .map(move |d| match menu {
                Some(subject) => track_menu::on_context_menu(d, subject),
                None => d,
            })
    }
}
