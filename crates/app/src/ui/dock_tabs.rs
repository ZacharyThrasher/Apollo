//! The three-segment control at the top of the docked right panel of the main window:
//! Queue · Crate · Recent. The queue panel and the Crate panel share that slot, one at a time;
//! Recent is the Crate panel showing the recently played list instead of the Crate. Picking
//! another segment swaps them.
//!
//! Same look as the Now Playing pane control (`ui::pane_tabs`): the active segment is
//! body-strong in text.primary with a 2px accent underline, the other is body in text.secondary.

use std::rc::Rc;

use apollo_theme::tokens::{layout, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, App, ClickEvent, FocusHandle, IntoElement, KeyboardButton, Window,
};

use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused};

/// What the docked panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Queue,
    Crate,
    /// The Crate panel, showing the recently played songs.
    Recent,
}

impl Dock {
    pub const ALL: [Dock; 3] = [Dock::Queue, Dock::Crate, Dock::Recent];

    pub fn label(self) -> &'static str {
        match self {
            Dock::Queue => "Queue",
            Dock::Crate => "Crate",
            Dock::Recent => "Recent",
        }
    }

    fn index(self) -> usize {
        match self {
            Dock::Queue => 0,
            Dock::Crate => 1,
            Dock::Recent => 2,
        }
    }
}

/// Height of the strip: 32px segments with 8px above and below (the pane control's height).
pub const HEIGHT: f32 = layout::MIN_HIT_TARGET + 2.0 * spacing::S2;
const UNDERLINE: f32 = 2.0;

/// One tab stop per segment, shared by the panels (they never show together).
#[derive(Clone)]
pub struct DockFocus(Rc<[FocusHandle; 3]>);

impl DockFocus {
    /// The pane control's slot (they never show together), ahead of the panels' own controls.
    pub const TAB_INDEX: isize = 12;

    pub fn new(cx: &mut App) -> Self {
        let stop = |cx: &mut App| cx.focus_handle().tab_index(Self::TAB_INDEX).tab_stop(true);
        Self(Rc::new([stop(cx), stop(cx), stop(cx)]))
    }

    /// Whether the keyboard is on either segment.
    pub fn any_focused(&self, window: &Window) -> bool {
        self.0.iter().any(|h| h.is_focused(window))
    }

    fn of(&self, dock: Dock) -> &FocusHandle {
        &self.0[dock.index()]
    }
}

/// Called with the segment the owner picked.
pub type OnPick = Rc<dyn Fn(Dock, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct DockTabs {
    active: Dock,
    focus: DockFocus,
    accent: Accent,
    on_pick: OnPick,
}

impl DockTabs {
    pub fn new(active: Dock, focus: &DockFocus, accent: Accent, on_pick: OnPick) -> Self {
        Self {
            active,
            focus: focus.clone(),
            accent,
            on_pick,
        }
    }
}

impl RenderOnce for DockTabs {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let segments = Dock::ALL.map(|dock| {
            let active = dock == self.active;
            let focus = self.focus.of(dock).clone();
            let ring = keyboard_focused(&focus, window);
            let accent = color::accent(self.accent);
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
            .id(dock.label())
            .debug_selector(move || format!("dock-tab-{}", dock.label()))
            .relative()
            .flex()
            .items_center()
            .h(px(layout::MIN_HIT_TARGET))
            .px(px(spacing::S2))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .track_focus(&focus)
            .role(gpui::Role::Tab)
            .aria_label(dock.label())
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
                if !active {
                    on_pick(dock, window, cx);
                }
            })
            .child(dock.label())
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
            .when(ring, move |d| d.child(focus_ring(accent, radius::SMALL)))
        });
        div()
            .id("dock-tabs")
            .role(gpui::Role::TabList)
            .aria_label("Side panel")
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
