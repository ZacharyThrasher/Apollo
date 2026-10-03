//! Left nav rail: Home, Search, Library, with Settings pinned at the bottom. 64px with icons and
//! tooltips, 220px with labels.

use apollo_theme::tokens::{layout, motion, radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, Animation, AnimationExt, App, ElementId, FocusHandle, IntoElement,
    ParentElement, RenderOnce, SharedString, Styled, Window,
};

use crate::assets::Icon;
use crate::style::{self, color};
use crate::ui::button::{focus_ring, keyboard_focused, Handler, IconButton};
use crate::ui::tooltip::Tooltip;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Route {
    Home,
    Search,
    Library,
    Settings,
}

impl Route {
    /// The three destinations at the top of the rail.
    pub const MAIN: [Route; 3] = [Route::Home, Route::Search, Route::Library];
    pub const ALL: [Route; 4] = [Route::Home, Route::Search, Route::Library, Route::Settings];

    pub fn title(self) -> &'static str {
        match self {
            Route::Home => "Home",
            Route::Search => "Search",
            Route::Library => "Library",
            Route::Settings => "Settings",
        }
    }

    fn icon(self, active: bool) -> Icon {
        match (self, active) {
            (Route::Home, false) => Icon::HomeRegular,
            (Route::Home, true) => Icon::HomeFilled,
            (Route::Search, false) => Icon::SearchRegular,
            (Route::Search, true) => Icon::SearchFilled,
            (Route::Library, false) => Icon::LibraryRegular,
            (Route::Library, true) => Icon::LibraryFilled,
            (Route::Settings, false) => Icon::SettingsRegular,
            (Route::Settings, true) => Icon::SettingsFilled,
        }
    }

    fn id(self) -> &'static str {
        match self {
            Route::Home => "nav-home",
            Route::Search => "nav-search",
            Route::Library => "nav-library",
            Route::Settings => "nav-settings",
        }
    }
}

/// Width of the indicator bar on the active item.
const INDICATOR_WIDTH: f32 = 3.0;
const ITEM_HEIGHT: f32 = 40.0;
const BADGE_SIZE: f32 = 8.0;
/// Centred on the top right corner of the 20px icon (which sits at 14px, 10px down).
const BADGE_LEFT: f32 = 30.0;
const BADGE_TOP: f32 = 6.0;

type RouteHandler = Box<dyn Fn(Route, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct NavRail {
    expanded: bool,
    /// Animate width changes only after the user has toggled once (never on first paint).
    animate: bool,
    route: Route,
    accent: Accent,
    toggle_focus: FocusHandle,
    item_focus: [FocusHandle; 4],
    /// Warning dot on Settings: the account needs the owner.
    attention: bool,
    on_toggle: Option<Handler>,
    on_route: Option<RouteHandler>,
}

impl NavRail {
    pub fn new(
        expanded: bool,
        animate: bool,
        route: Route,
        accent: Accent,
        toggle_focus: &FocusHandle,
        item_focus: &[FocusHandle; 4],
    ) -> Self {
        Self {
            expanded,
            animate,
            route,
            accent,
            toggle_focus: toggle_focus.clone(),
            item_focus: item_focus.clone(),
            attention: false,
            on_toggle: None,
            on_route: None,
        }
    }

    pub fn attention(mut self, attention: bool) -> Self {
        self.attention = attention;
        self
    }

    pub fn on_toggle(mut self, f: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }

    pub fn on_route(mut self, f: impl Fn(Route, &mut Window, &mut App) + 'static) -> Self {
        self.on_route = Some(Box::new(f));
        self
    }
}

fn rail_width(expanded: bool) -> f32 {
    if expanded {
        layout::NAV_RAIL_EXPANDED
    } else {
        layout::NAV_RAIL_COLLAPSED
    }
}

impl RenderOnce for NavRail {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (expanded, accent, current) = (self.expanded, self.accent, self.route);
        let on_route = self.on_route.map(std::rc::Rc::new);
        let on_toggle = self.on_toggle;
        let mut rail = div()
            .id("nav-rail")
            .flex()
            .flex_col()
            .flex_none()
            .h_full()
            .overflow_hidden()
            .gap(px(spacing::S1))
            .p(px(spacing::S2))
            .child(
                // Same icon column as the items below: centred in the collapsed rail.
                div()
                    .flex()
                    .flex_none()
                    .ml(px((layout::NAV_RAIL_COLLAPSED
                        - 2.0 * spacing::S2
                        - ITEM_HEIGHT)
                        / 2.0))
                    .child(
                        IconButton::new(
                            "nav-toggle",
                            &self.toggle_focus,
                            Icon::Navigation,
                            if expanded { "Collapse" } else { "Expand" },
                            accent,
                        )
                        .size(ITEM_HEIGHT)
                        .icon_color(color::text_secondary())
                        .when_some(on_toggle, |b, f| b.on_click(move |w, cx| f(w, cx))),
                    ),
            );
        for (route, focus) in Route::ALL.into_iter().zip(self.item_focus.iter()) {
            let on_route = on_route.clone();
            if route == Route::Settings {
                // Settings sits at the bottom of the rail.
                rail = rail.child(div().flex_1());
            }
            rail = rail.child(NavItem {
                route,
                active: route == current,
                expanded,
                accent,
                badge: route == Route::Settings && self.attention,
                focus: focus.clone(),
                on_click: Box::new(move |w, cx| {
                    if let Some(f) = &on_route {
                        f(route, w, cx);
                    }
                }),
            });
        }

        let (from, to) = if expanded {
            (rail_width(false), rail_width(true))
        } else {
            (rail_width(true), rail_width(false))
        };
        // The animation id includes the state, so each toggle starts a fresh 200 ms slide.
        let id = ElementId::from(("nav-rail-width", expanded as usize));
        if self.animate {
            rail.with_animation(
                id,
                Animation::new(motion::PANEL).with_easing(motion::ease_out_cubic),
                move |el, t| el.w(px(from + (to - from) * t)),
            )
            .into_any_element()
        } else {
            rail.w(px(to)).into_any_element()
        }
    }
}

#[derive(IntoElement)]
struct NavItem {
    route: Route,
    active: bool,
    expanded: bool,
    accent: Accent,
    /// A warning dot over the icon; the item's label says why.
    badge: bool,
    focus: FocusHandle,
    on_click: Handler,
}

impl RenderOnce for NavItem {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let label: SharedString = if self.badge {
            format!("{}, needs attention", self.route.title()).into()
        } else {
            self.route.title().into()
        };
        let ring = keyboard_focused(&self.focus, window);
        let icon_color = if self.active {
            color::text_primary()
        } else {
            color::text_secondary()
        };
        let accent = color::accent(self.accent);
        let item = div()
            .id(self.route.id())
            .relative()
            .flex()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(spacing::S3))
            .h(px(ITEM_HEIGHT))
            // Icon centred in the collapsed 48px item: (48 - 20) / 2.
            .pl(px((layout::NAV_RAIL_COLLAPSED
                - 2.0 * spacing::S2
                - layout::ICON_SIZE)
                / 2.0))
            .rounded(px(radius::SMALL))
            .cursor_pointer()
            .track_focus(&self.focus)
            .aria_label(label.clone())
            .role(gpui::Role::Link)
            .aria_selected(self.active)
            .when(self.active, |d| d.bg(color::state_selected()))
            .hover(|s| s.bg(color::state_hover()))
            .on_click({
                let f = self.on_click;
                move |event, window, cx| {
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == gpui::KeyboardButton::Space {
                            return;
                        }
                    }
                    f(window, cx)
                }
            })
            .when(self.active, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(spacing::S2))
                        .bottom(px(spacing::S2))
                        .w(px(INDICATOR_WIDTH))
                        .rounded_full()
                        .bg(accent),
                )
            })
            .child(
                gpui::svg()
                    .path(self.route.icon(self.active).path())
                    .size(px(layout::ICON_SIZE))
                    .flex_none()
                    .text_color(icon_color),
            )
            .when(self.badge, |d| {
                d.child(
                    div()
                        .absolute()
                        .top(px(BADGE_TOP))
                        .left(px(BADGE_LEFT))
                        .size(px(BADGE_SIZE))
                        .rounded_full()
                        .bg(color::warning()),
                )
            })
            .when(self.expanded, |d| {
                // Selected: body-strong in text.primary. Unselected: body in text.secondary.
                let label_style = if self.active {
                    typography::BODY_STRONG
                } else {
                    typography::BODY
                };
                d.child(
                    style::text_style(div(), label_style, cx)
                        .whitespace_nowrap()
                        .text_color(icon_color)
                        .child(label.clone()),
                )
            })
            .when(ring, |d| d.child(focus_ring(accent, radius::SMALL)));
        if self.expanded {
            item.into_any_element()
        } else {
            item.tooltip(move |_, cx| Tooltip::view(label.clone(), cx))
                .into_any_element()
        }
    }
}
