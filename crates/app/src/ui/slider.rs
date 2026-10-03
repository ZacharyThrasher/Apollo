//! Horizontal slider for seek and volume: 32px hit target around a 4px track, accent fill, a
//! thumb that appears on hover / drag / keyboard focus. Drag state lives in a [`SliderState`]
//! entity owned by the shell.

use std::rc::Rc;

use apollo_theme::Accent;
use gpui::{
    actions, canvas, div, prelude::*, px, relative, App, Bounds, DispatchPhase, ElementId,
    FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Window,
};

use crate::style::color;
use crate::ui::button::{focus_ring, keyboard_focused};

actions!(apollo_slider, [StepBack, StepForward]);

pub const HIT_HEIGHT: f32 = 32.0;
const TRACK_HEIGHT: f32 = 4.0;
const THUMB: f32 = 12.0;

/// Key binding context for the slider's focus handle.
pub const KEY_CONTEXT: &str = "Slider";

#[derive(Default)]
pub struct SliderState {
    bounds: Bounds<Pixels>,
    dragging: bool,
    frac: f32,
}

impl SliderState {
    /// The scrub position while dragging (0-1), else `None`.
    pub fn scrubbing(&self) -> Option<f32> {
        self.dragging.then_some(self.frac)
    }
}

/// Position of `x` along a track starting at `left` and `width` wide, clamped to 0-1.
pub fn fraction_at(x: f32, left: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 0.0;
    }
    ((x - left) / width).clamp(0.0, 1.0)
}

/// `value` moved by `delta`, clamped to 0-1.
pub fn stepped(value: f32, delta: f32) -> f32 {
    (value + delta).clamp(0.0, 1.0)
}

type ChangeHandler = Rc<dyn Fn(f32, bool, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    state: gpui::Entity<SliderState>,
    focus: FocusHandle,
    label: gpui::SharedString,
    value: f32,
    /// The value in words for assistive technology ("1:23 of 3:12", "70%").
    value_text: Option<gpui::SharedString>,
    step: f32,
    enabled: bool,
    accent: Accent,
    fill: Option<gpui::Hsla>,
    on_change: Option<ChangeHandler>,
}

impl Slider {
    pub fn new(
        id: impl Into<ElementId>,
        state: &gpui::Entity<SliderState>,
        focus: &FocusHandle,
        label: impl Into<gpui::SharedString>,
        accent: Accent,
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            focus: focus.clone(),
            label: label.into(),
            value: 0.0,
            value_text: None,
            step: 0.05,
            enabled: true,
            accent,
            fill: None,
            on_change: None,
        }
    }

    pub fn value(mut self, value: f32) -> Self {
        self.value = value.clamp(0.0, 1.0);
        self
    }

    /// What the value means in words, for screen readers; the fraction alone says little.
    pub fn value_text(mut self, text: impl Into<gpui::SharedString>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// Fill color of the played part of the track; the accent when unset.
    pub fn fill(mut self, fill: gpui::Hsla) -> Self {
        self.fill = Some(fill);
        self
    }

    /// Arrow-key step as a fraction of the full range.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// `(fraction, committed)`: `committed` is false while dragging and true when the user lets
    /// go, clicks or uses the keyboard.
    pub fn on_change(mut self, f: impl Fn(f32, bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (dragging, drag_frac) = {
            let s = self.state.read(cx);
            (s.dragging, s.frac)
        };
        let shown = if dragging { drag_frac } else { self.value };
        let enabled = self.enabled;
        let kb_focus = enabled && keyboard_focused(&self.focus, window);
        let accent = color::accent(self.accent);
        let fill = self.fill.unwrap_or(accent);
        let on_change = self.on_change.clone().filter(|_| enabled);
        let step = self.step;
        let value = self.value;
        let group: gpui::SharedString = format!("slider-{}", self.id).into();

        let thumb_visible = dragging || kb_focus;
        let thumb = div()
            .absolute()
            .top(px(-(THUMB - TRACK_HEIGHT) / 2.0))
            .left(relative(shown))
            .ml(px(-THUMB / 2.0))
            .size(px(THUMB))
            .rounded_full()
            .bg(color::text_primary())
            .opacity(if thumb_visible { 1.0 } else { 0.0 })
            .group_hover(group.clone(), |s| s.opacity(1.0))
            .when(kb_focus, |d| d.child(focus_ring(accent, THUMB / 2.0)));

        // Records the track's bounds, and while a drag is live listens for the pointer
        // anywhere in the window so the thumb keeps following outside the track.
        let probe = {
            let state = self.state.clone();
            let on_change = on_change.clone();
            canvas(
                {
                    let state = state.clone();
                    move |bounds, _, cx| {
                        state.update(cx, |s, _| s.bounds = bounds);
                    }
                },
                move |_, _, window, _| {
                    if !dragging {
                        return;
                    }
                    let on_move = {
                        let state = state.clone();
                        let on_change = on_change.clone();
                        move |ev: &MouseMoveEvent,
                              phase: DispatchPhase,
                              window: &mut Window,
                              cx: &mut App| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let frac = state.update(cx, |s, cx| {
                                s.frac = fraction_at(
                                    f32::from(ev.position.x),
                                    f32::from(s.bounds.left()),
                                    f32::from(s.bounds.size.width),
                                );
                                cx.notify();
                                s.frac
                            });
                            if let Some(f) = &on_change {
                                f(frac, false, window, cx);
                            }
                        }
                    };
                    let on_up = {
                        let state = state.clone();
                        let on_change = on_change.clone();
                        move |ev: &MouseUpEvent,
                              phase: DispatchPhase,
                              window: &mut Window,
                              cx: &mut App| {
                            if phase != DispatchPhase::Capture || ev.button != MouseButton::Left {
                                return;
                            }
                            let frac = state.update(cx, |s, cx| {
                                s.dragging = false;
                                s.frac = fraction_at(
                                    f32::from(ev.position.x),
                                    f32::from(s.bounds.left()),
                                    f32::from(s.bounds.size.width),
                                );
                                cx.notify();
                                s.frac
                            });
                            if let Some(f) = &on_change {
                                f(frac, true, window, cx);
                            }
                        }
                    };
                    window.on_mouse_event(on_move);
                    window.on_mouse_event(on_up);
                },
            )
            .absolute()
            .size_full()
        };

        let state_down = self.state.clone();
        let on_change_down = on_change.clone();
        let on_change_back = on_change.clone();
        let on_change_fwd = on_change.clone();
        let on_change_a11y = on_change.clone();
        let value_text = self.value_text.clone();

        div()
            .id(self.id.clone())
            .group(group)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus.clone().tab_stop(enabled))
            .role(gpui::Role::Slider)
            .aria_label(self.label.clone())
            .aria_numeric_value(f64::from(value))
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(1.0)
            .aria_numeric_value_step(f64::from(step))
            .when_some(value_text, |d, text| d.aria_value(text))
            // Assistive technology moves the slider the way the arrow keys and the mouse do.
            .when_some(on_change_a11y, |d, f| {
                use gpui::accesskit::ActionData;
                use gpui::AccessibleAction as A;
                let (up, down, set) = (f.clone(), f.clone(), f);
                d.on_a11y_action(A::Increment, move |_, window, cx| {
                    up(stepped(value, step), true, window, cx)
                })
                .on_a11y_action(A::Decrement, move |_, window, cx| {
                    down(stepped(value, -step), true, window, cx)
                })
                .on_a11y_action(A::SetValue, move |data, window, cx| {
                    if let Some(ActionData::NumericValue(v)) = data {
                        set((*v as f32).clamp(0.0, 1.0), true, window, cx)
                    }
                })
            })
            .w_full()
            .h(px(HIT_HEIGHT))
            .flex()
            .items_center()
            .when(enabled, |d| d.cursor_pointer())
            .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, cx| {
                let frac = state_down.update(cx, |s, cx| {
                    s.dragging = true;
                    s.frac = fraction_at(
                        f32::from(ev.position.x),
                        f32::from(s.bounds.left()),
                        f32::from(s.bounds.size.width),
                    );
                    cx.notify();
                    s.frac
                });
                if let Some(f) = &on_change_down {
                    f(frac, false, window, cx);
                }
            })
            .on_action(move |_: &StepBack, window, cx| {
                if let Some(f) = &on_change_back {
                    f(stepped(value, -step), true, window, cx);
                }
            })
            .on_action(move |_: &StepForward, window, cx| {
                if let Some(f) = &on_change_fwd {
                    f(stepped(value, step), true, window, cx);
                }
            })
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(TRACK_HEIGHT))
                    .rounded_full()
                    .bg(color::border_strong())
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .h_full()
                            .w(relative(shown))
                            .rounded_full()
                            .bg(if enabled {
                                fill
                            } else {
                                color::text_disabled()
                            }),
                    )
                    .child(thumb)
                    .child(probe),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fraction_is_clamped_to_the_track() {
        assert_eq!(fraction_at(50.0, 100.0, 200.0), 0.0);
        assert_eq!(fraction_at(100.0, 100.0, 200.0), 0.0);
        assert_eq!(fraction_at(200.0, 100.0, 200.0), 0.5);
        assert_eq!(fraction_at(300.0, 100.0, 200.0), 1.0);
        assert_eq!(fraction_at(900.0, 100.0, 200.0), 1.0);
        assert_eq!(fraction_at(10.0, 0.0, 0.0), 0.0);
    }

    #[test]
    fn stepping_stays_in_range() {
        assert_eq!(stepped(0.97, 0.05), 1.0);
        assert_eq!(stepped(0.02, -0.05), 0.0);
        assert!((stepped(0.5, 0.05) - 0.55).abs() < 1e-6);
    }
}
