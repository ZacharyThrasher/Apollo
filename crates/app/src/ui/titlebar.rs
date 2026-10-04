//! Custom title bar: a 32px drag region over the Mica backdrop with native-style caption
//! buttons. Windows resolves the control areas itself (snap layouts, double-click maximize,
//! system menu), so there are no click handlers here. The buttons are also exposed to assistive
//! technology as named buttons whose accessible Click does the same thing; Windows draws no
//! system caption for a window like this, so they would otherwise be invisible to Narrator and
//! voice control.

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

#[derive(IntoElement, Clone, Copy, PartialEq, Eq)]
enum CaptionButton {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Closes like the native button: through the window's close request, which Close to tray
/// answers by hiding. Without a native window handle the window is simply removed.
fn close(window: &mut Window, cx: &mut App) {
    // Expanded from the mini player: it collapses back instead.
    if crate::mini::collapse_on_close(cx) {
        return;
    }
    #[cfg(windows)]
    if crate::tray_win::request_close(window) {
        return;
    }
    window.remove_window();
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

    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize",
            Self::Maximize => "Maximize",
            Self::Restore => "Restore",
            Self::Close => "Close",
        }
    }

    /// What an assistive-technology Click on the button does.
    fn activate(self, window: &mut Window, cx: &mut App) {
        match self {
            Self::Minimize => window.minimize_window(),
            Self::Maximize | Self::Restore => window.zoom_window(),
            Self::Close => close(window, cx),
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
            .when(enabled && self != Self::Close, |d| {
                d.hover(|s| s.bg(color::state_hover()))
                    .active(|s| s.bg(color::state_pressed()))
            })
            // Close keeps the native Windows 11 red with a white glyph.
            .when(self == Self::Close, |d| {
                d.hover(|s| s.bg(color::close_hover()).text_color(color::close_fg()))
                    .active(|s| s.bg(color::close_pressed()).text_color(color::close_fg()))
            })
            .window_control_area(self.area())
            .role(gpui::Role::Button)
            .aria_label(self.label())
            .when(enabled, |d| {
                d.on_a11y_action(gpui::AccessibleAction::Click, move |_, window, cx| {
                    self.activate(window, cx)
                })
            })
            .child(self.glyph())
    }
}
