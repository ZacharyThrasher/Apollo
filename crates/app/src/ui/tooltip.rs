use std::time::{Duration, Instant};

use apollo_theme::tokens::{radius, spacing, typography};
use gpui::{
    div, px, AnyView, App, AppContext, Context, Entity, Global, IntoElement, ParentElement,
    Render, SharedString, Styled, Window,
};

use crate::style::{self, color};

/// Counts the times every tooltip on screen was told to go away. A tooltip that was built
/// before the latest count renders as nothing.
///
/// GPUI hides a tooltip when the mouse moves off its element, but not when the element scrolls
/// away under a still mouse, when a drag starts or ends, or when the window loses focus; in a
/// virtualized list the element may not even be drawn any more. The shell calls [`dismiss_all`]
/// for those events.
///
/// A tooltip that was still waiting out GPUI's 500ms show delay when the event came would
/// otherwise appear afterwards, over whatever scrolled under the mouse. So a tooltip built
/// within [`QUIET`] of a dismissal is hidden too; one for a hover that began after the event
/// cannot be built that early.
#[derive(Default)]
struct Epoch {
    count: u64,
    last: Option<Instant>,
}

impl Global for Epoch {}

/// A little longer than GPUI's tooltip show delay.
const QUIET: Duration = Duration::from_millis(600);

fn epoch(cx: &App) -> u64 {
    cx.try_global::<Epoch>().map_or(0, |e| e.count)
}

fn in_quiet_period(now: Instant, cx: &App) -> bool {
    cx.try_global::<Epoch>()
        .and_then(|e| e.last)
        .is_some_and(|last| now.saturating_duration_since(last) < QUIET)
}

/// Hide every tooltip currently showing or about to show.
pub fn dismiss_all(cx: &mut App) {
    let epoch = cx.default_global::<Epoch>();
    epoch.count += 1;
    epoch.last = Some(Instant::now());
}

/// Menus, popovers and tooltips sit on bg.overlay and use text.secondary as their lowest text
/// tier (text.tertiary fails AA there).
pub struct Tooltip {
    text: SharedString,
    built_at: u64,
    /// Built so soon after a dismissal that it belongs to a hover from before it.
    stale: bool,
}

impl Tooltip {
    pub fn view(text: impl Into<SharedString>, cx: &mut App) -> AnyView {
        Self::entity(text, cx).into()
    }

    pub fn entity(text: impl Into<SharedString>, cx: &mut App) -> Entity<Self> {
        Self::entity_at(text, Instant::now(), cx)
    }

    fn entity_at(text: impl Into<SharedString>, now: Instant, cx: &mut App) -> Entity<Self> {
        let text = text.into();
        let (built_at, stale) = (epoch(cx), in_quiet_period(now, cx));
        cx.new(|_| Tooltip {
            text,
            built_at,
            stale,
        })
    }

    /// Whether [`dismiss_all`] was called since this tooltip was built, or it was built right
    /// after one.
    pub fn is_dismissed(&self, cx: &App) -> bool {
        self.stale || self.built_at != epoch(cx)
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.is_dismissed(cx) {
            return div().into_any_element();
        }
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
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn dismissing_hides_the_tooltips_built_before_and_not_after(cx: &mut TestAppContext) {
        let first = cx.update(|cx| Tooltip::entity("a", cx));
        assert!(!cx.read(|cx| first.read(cx).is_dismissed(cx)));
        cx.update(dismiss_all);
        assert!(cx.read(|cx| first.read(cx).is_dismissed(cx)));
        // One built right after belongs to a hover that began before: hidden as well.
        let straggler = cx.update(|cx| Tooltip::entity("b", cx));
        assert!(cx.read(|cx| straggler.read(cx).is_dismissed(cx)));
        // One for a hover that began after the event is built later and shows.
        let later = Instant::now() + QUIET + Duration::from_millis(1);
        let second = cx.update(|cx| Tooltip::entity_at("c", later, cx));
        assert!(!cx.read(|cx| second.read(cx).is_dismissed(cx)));
    }
}
