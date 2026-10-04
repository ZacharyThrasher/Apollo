//! The three-segment control at the top of Now Playing's right pane: Up next · Lyrics · Related.
//!
//! Text segments: the active one is body-strong in text.primary with a 2px accent underline, the
//! others body in text.secondary. It sits above whichever of the queue, the lyrics and the
//! related songs the pane shows; picking a segment swaps the pane.

use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, App, ClickEvent, FocusHandle, IntoElement, KeyboardButton, Window,
};

use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};

/// What the right pane of Now Playing shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pane {
    UpNext,
    Lyrics,
    Related,
}

impl Pane {
    pub const ALL: [Pane; 3] = [Pane::UpNext, Pane::Lyrics, Pane::Related];

    pub fn label(self) -> &'static str {
        match self {
            Pane::UpNext => "Up next",
            Pane::Lyrics => "Lyrics",
            Pane::Related => "Related",
        }
    }

    fn index(self) -> usize {
        match self {
            Pane::UpNext => 0,
            Pane::Lyrics => 1,
            Pane::Related => 2,
        }
    }
}

/// Height of the strip: the segments are 32px hit targets with 8px above and below.
pub const HEIGHT: f32 = layout::MIN_HIT_TARGET + 2.0 * spacing::S2;
/// The underline under the active segment.
const UNDERLINE: f32 = 2.0;

/// One tab stop per segment, shared by every place the strip is drawn (they never show together).
#[derive(Clone)]
pub struct PaneFocus(Rc<[FocusHandle; 3]>);

impl PaneFocus {
    /// After the nav rail and Now Playing, ahead of the queue panel's controls.
    pub const TAB_INDEX: isize = 12;

    pub fn new(cx: &mut App) -> Self {
        let stop = |cx: &mut App| cx.focus_handle().tab_index(Self::TAB_INDEX).tab_stop(true);
        Self(Rc::new([stop(cx), stop(cx), stop(cx)]))
    }

    fn of(&self, pane: Pane) -> &FocusHandle {
        &self.0[pane.index()]
    }
}

/// Called with the segment the owner picked.
pub type OnPick = Rc<dyn Fn(Pane, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct PaneTabs {
    active: Pane,
    focus: PaneFocus,
    accent: Accent,
    on_pick: OnPick,
}

impl PaneTabs {
    pub fn new(active: Pane, focus: &PaneFocus, accent: Accent, on_pick: OnPick) -> Self {
        Self {
            active,
            focus: focus.clone(),
            accent,
            on_pick,
        }
    }
}

impl RenderOnce for PaneTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let segments = Pane::ALL.map(|pane| {
            let active = pane == self.active;
            let focus = self.focus.of(pane).clone();
            let ring = keyboard_focused(&focus, window);
            let accent = color::accent(self.accent);
            let ring_color = accent;
            let on_pick = self.on_pick.clone();
            style::text_style(
                div(),
                if active {
                    typography::BODY_STRONG
                } else {
                    typography::BODY
                },
                cx,
            )
            .id(pane.label())
            .debug_selector(move || format!("pane-tab-{}", pane.label()))
            .relative()
            .flex()
            .items_center()
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .track_focus(&focus)
            .role(gpui::Role::Tab)
            .aria_label(pane.label())
            .aria_selected(active)
            .text_color(if active {
                color::text_primary()
            } else {
                color::text_secondary()
            })
            .hover(|s| s.bg(color::state_hover()))
            .on_click(move |event: &ClickEvent, window, cx| {
                // Space is the play/pause key.
                if let ClickEvent::Keyboard(k) = event {
                    if k.button == KeyboardButton::Space {
                        return;
                    }
                }
                on_pick(pane, window, cx);
            })
            .child(pane.label())
            .when(active, |d| {
                d.child(
                    div()
                        .absolute()
                        .left(px(spacing::S2))
                        .right(px(spacing::S2))
                        .bottom(px(0.0))
                        .h(px(UNDERLINE))
                        .rounded_full()
                        .bg(accent),
                )
            })
            .when(ring, move |d| d.child(focus_ring(ring_color, radius::SMALL)))
        });
        div()
            .id("pane-tabs")
            .role(gpui::Role::TabList)
            .aria_label("Now Playing")
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S2))
            .h(px(HEIGHT))
            .w_full()
            .px(px(spacing::S3))
            .children(segments)
    }
}
