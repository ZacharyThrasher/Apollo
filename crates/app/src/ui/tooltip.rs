use apollo_theme::tokens::{radius, spacing, typography};
use gpui::{
    div, px, AnyView, App, AppContext, Context, IntoElement, ParentElement, Render, SharedString,
    Styled, Window,
};

use crate::style::{self, color};

/// Menus, popovers and tooltips sit on bg.overlay and use text.secondary as their lowest text
/// tier (text.tertiary fails AA there).
pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn view(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
        let text = text.into();
        cx.new(|_| Tooltip { text }).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        style::text_style(div(), typography::BODY, cx)
            .px(px(spacing::S2))
            .py(px(spacing::S1))
            .rounded(px(radius::SMALL))
            .bg(color::bg_overlay())
            .border_1()
            .border_color(color::border_subtle())
            .text_color(color::text_secondary())
            .max_w(px(360.))
            .child(self.text.clone())
    }
}
