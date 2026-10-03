//! Custom title bar: a 32px drag region over the Mica backdrop with native-style caption
//! buttons. Windows resolves the control areas itself (snap layouts, double-click maximize,
//! system menu), so there are no click handlers here.

use apollo_theme::tokens::layout;
use gpui::{
    div, prelude::*, px, App, ElementId, IntoElement, ParentElement, RenderOnce, Styled, Window,
    WindowControlArea,
};

use crate::style::color;

/// Caption button width in the native Windows title bar.
const CAPTION_BUTTON_WIDTH: f32 = 46.0;
/// Glyph size inside a caption button.
const CAPTION_GLYPH_PX: f32 = 10.0;

#[derive(IntoElement)]
pub struct TitleBar;

impl RenderOnce for TitleBar {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let maximized = window.is_maximized();
        div()
            .id("title-bar")
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .flex_row()
            .flex_none()
            .w_full()
            .h(px(layout::TITLE_BAR_HEIGHT))
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .h_full()
                    .font_family(caption_font())
                    .child(CaptionButton::Minimize)
                    .child(if maximized {
                        CaptionButton::Restore
                    } else {
                        CaptionButton::Maximize
                    })
                    .child(CaptionButton::Close),
            )
    }
}

/// Segoe Fluent Icons on Windows 11 (build 22000+), which is what the glyph codes below belong
/// to; Segoe MDL2 Assets carries the same codes on older builds.
fn caption_font() -> &'static str {
    "Segoe Fluent Icons"
}

#[derive(IntoElement, Clone, Copy)]
enum CaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl CaptionButton {
    fn id(self) -> &'static str {
        match self {
            Self::Minimize => "caption-minimize",
            Self::Maximize => "caption-maximize",
            Self::Restore => "caption-restore",
            Self::Close => "caption-close",
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Minimize => "\u{e921}",
            Self::Restore => "\u{e923}",
            Self::Maximize => "\u{e922}",
            Self::Close => "\u{e8bb}",
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize | Self::Restore => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
}

impl RenderOnce for CaptionButton {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let enabled = match self {
            Self::Minimize => window.is_minimizable(),
            Self::Maximize | Self::Restore => window.is_resizable(),
            Self::Close => true,
        };
        div()
            .id(ElementId::from(self.id()))
            .flex()
            .items_center()
            .justify_center()
            .occlude()
            .w(px(CAPTION_BUTTON_WIDTH))
            .h_full()
            .text_size(px(CAPTION_GLYPH_PX))
            .text_color(if enabled {
                color::text_primary()
            } else {
                color::text_disabled()
            })
            .when(enabled, |d| {
                d.hover(|s| s.bg(color::state_hover()))
                    .active(|s| s.bg(color::state_pressed()))
            })
            .window_control_area(self.area())
            .child(self.glyph())
    }
}
