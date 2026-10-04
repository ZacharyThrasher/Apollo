//! Vertical slider for the equalizer's bands: a 32px-wide hit target around a 4px track, a fill
//! that runs from the middle (0 dB) to the thumb, and the thumb of the horizontal slider (12px,
//! text.primary, shown on hover, drag or keyboard focus). Drag state lives in a
//! [`VSliderState`] entity the owner keeps, one per slider.

use std::rc::Rc;

use gpui::{
    actions, canvas, div, prelude::*, px, relative, App, Bounds, DispatchPhase, ElementId,
    FocusHandle, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Window,
};

use crate::style::color;
use crate::ui::button::{focus_ring, keyboard_focused};

actions!(apollo_vslider, [StepDown, StepUp, PageDown, PageUp]);

/// Width of the hit target; its height is the slider's.
pub const HIT_WIDTH: f32 = 32.0;
const TRACK_WIDTH: f32 = 4.0;
const THUMB: f32 = 12.0;

/// Key binding context for the slider's focus handle.
pub const KEY_CONTEXT: &str = "VSlider";

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", StepDown, Some(KEY_CONTEXT)),
        KeyBinding::new("up", StepUp, Some(KEY_CONTEXT)),
        KeyBinding::new("pagedown", PageDown, Some(KEY_CONTEXT)),
        KeyBinding::new("pageup", PageUp, Some(KEY_CONTEXT)),
    ]);
}

#[derive(Default)]
pub struct VSliderState {
    bounds: Bounds<Pixels>,
    dragging: bool,
    frac: f32,
}

impl VSliderState {
    /// Whether a drag is in progress.
    pub fn is_dragging(&self) -> bool {
        self.dragging
    }
}

/// Position of `y` along a track starting at `top` and `height` tall, as 0 (bottom) to 1 (top).
pub fn fraction_at(y: f32, top: f32, height: f32) -> f32 {
    if height <= 0.0 {
        return 0.0;
    }
    (1.0 - (y - top) / height).clamp(0.0, 1.0)
}

type ChangeHandler = Rc<dyn Fn(f32, bool, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct VSlider {
    id: ElementId,
    state: gpui::Entity<VSliderState>,
    focus: FocusHandle,
    label: gpui::SharedString,
    value: f32,
    value_text: Option<gpui::SharedString>,
    height: f32,
    origin: f32,
    step: f32,
    fill: gpui::Hsla,
    on_change: Option<ChangeHandler>,
}

impl VSlider {
    pub fn new(
        id: impl Into<ElementId>,
        state: &gpui::Entity<VSliderState>,
        focus: &FocusHandle,
        label: impl Into<gpui::SharedString>,
        height: f32,
    ) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
            focus: focus.clone(),
            label: label.into(),
            value: 0.5,
            value_text: None,
            height,
            origin: 0.5,
            step: 0.05,
            fill: color::text_secondary(),
            on_change: None,
        }
    }

    /// The position of the thumb, 0 (bottom) to 1 (top).
    pub fn value(mut self, value: f32) -> Self {
        self.value = value.clamp(0.0, 1.0);
        self
    }

    /// What the value means in words, for screen readers.
    pub fn value_text(mut self, text: impl Into<gpui::SharedString>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// Where the fill starts (0.5, the middle, by default).
    pub fn origin(mut self, origin: f32) -> Self {
        self.origin = origin.clamp(0.0, 1.0);
        self
    }

    /// Arrow-key step as a fraction of the full range.
    pub fn step(mut self, step: f32) -> Self {
        self.step = step;
        self
    }

    /// `(fraction, committed)`: `committed` is false while dragging and true when the owner lets
    /// go, clicks or uses the keyboard.
    pub fn on_change(mut self, f: impl Fn(f32, bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for VSlider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (dragging, drag_frac) = {
            let s = self.state.read(cx);
            (s.dragging, s.frac)
        };
        let shown = if dragging { drag_frac } else { self.value };
        let kb_focus = keyboard_focused(&self.focus, window);
        let on_change = self.on_change.clone();
        let (step, value, origin, height) = (self.step, self.value, self.origin, self.height);
        let group: gpui::SharedString = format!("vslider-{}", self.id).into();

        // The fill runs between the origin and the thumb, whichever is higher.
        let (low, high) = (shown.min(origin), shown.max(origin));
        let fill = div()
            .absolute()
            .left_0()
            .w_full()
            .bottom(relative(low))
            .h(relative(high - low))
            .rounded_full()
            .bg(self.fill);

        let thumb_visible = dragging || kb_focus;
        let thumb = div()
            .absolute()
            .left(px(-(THUMB - TRACK_WIDTH) / 2.0))
            .bottom(relative(shown))
            .mb(px(-THUMB / 2.0))
            .size(px(THUMB))
            .rounded_full()
            .bg(color::text_primary())
            .opacity(if thumb_visible { 1.0 } else { 0.0 })
            .group_hover(group.clone(), |s| s.opacity(1.0))
            .when(kb_focus, |d| {
                d.child(focus_ring(color::text_primary(), THUMB / 2.0))
            });

        // Records the track's bounds, and while a drag is live listens for the pointer anywhere
        // in the window so the thumb keeps following outside the track.
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
                                    f32::from(ev.position.y),
                                    f32::from(s.bounds.top()),
                                    f32::from(s.bounds.size.height),
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
                                    f32::from(ev.position.y),
                                    f32::from(s.bounds.top()),
                                    f32::from(s.bounds.size.height),
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
        let (down, up, page_down, page_up) = (
            on_change.clone(),
            on_change.clone(),
            on_change.clone(),
            on_change.clone(),
        );
        let value_text = self.value_text.clone();

        div()
            .id(self.id.clone())
            .group(group)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus.clone().tab_stop(true))
            .role(gpui::Role::Slider)
            .aria_label(self.label.clone())
            .aria_numeric_value(f64::from(value))
            .aria_min_numeric_value(0.0)
            .aria_max_numeric_value(1.0)
            .aria_numeric_value_step(f64::from(step))
            .when_some(value_text, |d, text| d.aria_value(text))
            .w(px(HIT_WIDTH))
            .h(px(height))
            .flex_none()
            .flex()
            .justify_center()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |ev: &MouseDownEvent, window, cx| {
                let frac = state_down.update(cx, |s, cx| {
                    s.dragging = true;
                    s.frac = fraction_at(
                        f32::from(ev.position.y),
                        f32::from(s.bounds.top()),
                        f32::from(s.bounds.size.height),
                    );
                    cx.notify();
                    s.frac
                });
                if let Some(f) = &on_change_down {
                    f(frac, false, window, cx);
                }
            })
            .on_action(move |_: &StepDown, window, cx| {
                if let Some(f) = &down {
                    f((value - step).clamp(0.0, 1.0), true, window, cx);
                }
            })
            .on_action(move |_: &StepUp, window, cx| {
                if let Some(f) = &up {
                    f((value + step).clamp(0.0, 1.0), true, window, cx);
                }
            })
            .on_action(move |_: &PageDown, window, cx| {
                if let Some(f) = &page_down {
                    f((value - 3.0 * step).clamp(0.0, 1.0), true, window, cx);
                }
            })
            .on_action(move |_: &PageUp, window, cx| {
                if let Some(f) = &page_up {
                    f((value + 3.0 * step).clamp(0.0, 1.0), true, window, cx);
                }
            })
            .child(
                div()
                    .relative()
                    .h_full()
                    .w(px(TRACK_WIDTH))
                    .rounded_full()
                    .bg(color::border_strong())
                    .child(fill)
                    .child(thumb)
                    .child(probe),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_of_the_track_is_one_and_the_bottom_is_zero() {
        assert_eq!(fraction_at(100.0, 100.0, 200.0), 1.0);
        assert_eq!(fraction_at(300.0, 100.0, 200.0), 0.0);
        assert_eq!(fraction_at(200.0, 100.0, 200.0), 0.5);
    }

    #[test]
    fn the_pointer_outside_the_track_is_clamped_to_it() {
        assert_eq!(fraction_at(0.0, 100.0, 200.0), 1.0);
        assert_eq!(fraction_at(900.0, 100.0, 200.0), 0.0);
        assert_eq!(fraction_at(10.0, 0.0, 0.0), 0.0);
    }
}
