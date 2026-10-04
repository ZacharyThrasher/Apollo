//! The equalizer's editor, opened from Settings > Audio > Equalizer > Edit: preset chips and
//! Reset on top, then ten vertical sliders, 31 Hz to 16 kHz, each +-12 dB with its frequency
//! under it in text.tertiary. Changes reach the sound while they are made (the panel edits the
//! player; the settings are written when a change is final, see `audio`). A band's value shows
//! above its slider while it is pointed at, dragged or focused, and takes no room otherwise.
//! Editing a flat equalizer that is off turns it on, so the change is heard.

use std::time::Instant;

use apollo_domain::audio::{band_label, Equalizer, Preset, BANDS, BAND_HZ};
use apollo_theme::tokens::{radius, spacing, typography};
use apollo_theme::Accent;
use gpui::{
    div, prelude::*, px, App, Context, ElementId, Entity, FocusHandle, IntoElement, KeyboardButton,
    ParentElement, Render, SharedString, Styled, Window,
};

use crate::audio::{after_edit, fraction_of, gain_at, gain_text, save_equalizer, with_band};
use crate::motion::HoverFade;
use crate::player::PlayerModel;
use crate::settings::SettingsModel;
use crate::style::{self, color};
use crate::ui::account_page::TAB_INDEX;
use crate::ui::button::{focus_ring, keyboard_focused};
use crate::ui::text_button::TextButton;
use crate::ui::vslider::{VSlider, VSliderState};

/// Height of a band's slider.
pub const SLIDER_HEIGHT: f32 = 160.0;
/// Height of the line above a slider that shows its value, and of the one below with its
/// frequency: held open so nothing shifts when a value appears.
const CAPTION_HEIGHT: f32 = 16.0;
/// Chips and the Reset button are as tall as every hit target.
const CHIP_HEIGHT: f32 = 32.0;
/// Whole dB per arrow key press, as a fraction of the slider.
const STEP: f32 = 1.0 / 24.0;

pub struct EqualizerPanel {
    player: Entity<PlayerModel>,
    settings: Entity<SettingsModel>,
    sliders: Vec<Entity<VSliderState>>,
    focus_sliders: Vec<FocusHandle>,
    focus_presets: Vec<FocusHandle>,
    focus_reset: FocusHandle,
}

impl EqualizerPanel {
    pub fn new(
        player: Entity<PlayerModel>,
        settings: Entity<SettingsModel>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&player, |_, _, cx| cx.notify()).detach();
        let stop = |cx: &mut Context<Self>| cx.focus_handle().tab_index(TAB_INDEX).tab_stop(true);
        let sliders: Vec<_> = (0..BANDS)
            .map(|_| cx.new(|_| VSliderState::default()))
            .collect();
        for state in &sliders {
            cx.observe(state, |_, _, cx| cx.notify()).detach();
        }
        Self {
            focus_sliders: (0..BANDS).map(|_| stop(cx)).collect(),
            focus_presets: Preset::ALL.iter().map(|_| stop(cx)).collect(),
            focus_reset: stop(cx),
            player,
            settings,
            sliders,
        }
    }

    /// Apply `eq` to the sound now; `save` also writes it to the settings (a final change).
    fn apply(&self, eq: Equalizer, save: bool, cx: &mut App) {
        self.player.update(cx, |p, cx| p.set_equalizer(eq, cx));
        if save {
            save_equalizer(&self.settings, eq, cx);
        }
    }
}

impl Render for EqualizerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (eq, accent) = {
            let p = self.player.read(cx);
            (p.equalizer(), p.view().accent)
        };
        let selected = eq.preset();
        let panel = cx.entity();

        let chips = Preset::ALL.iter().enumerate().map(|(i, preset)| {
            let panel = panel.clone();
            let preset = *preset;
            Chip::new(
                ("eq-preset", i),
                &self.focus_presets[i],
                preset.name(),
                selected == Some(preset),
                accent,
            )
            .on_click(move |_, cx| {
                panel.update(cx, |this, cx| {
                    let current = this.player.read(cx).equalizer();
                    this.apply(after_edit(current, preset.gains()), true, cx);
                });
            })
        });

        let reset = {
            let panel = panel.clone();
            TextButton::new("eq-reset", &self.focus_reset, "Reset", accent)
                .enabled(eq.gains_db.iter().any(|g| *g != 0.0))
                .on_click(move |_, cx| {
                    panel.update(cx, |this, cx| {
                        let current = this.player.read(cx).equalizer();
                        this.apply(after_edit(current, Preset::Flat.gains()), true, cx);
                    });
                })
        };

        let bands = (0..BANDS).map(|band| {
            let gain = eq.gains_db[band];
            let state = self.sliders[band].clone();
            let dragging = state.read(cx).is_dragging();
            let label = band_label(BAND_HZ[band]);
            let group: SharedString = format!("eq-band-group-{band}").into();
            let focused = self.focus_sliders[band].is_focused(window);
            let panel = panel.clone();
            div()
                .id(("eq-band", band))
                .debug_selector(move || format!("eq-band-{band}"))
                .group(group.clone())
                .flex()
                .flex_col()
                .flex_1()
                .items_center()
                .gap(px(spacing::S1))
                .child(
                    style::text_style(div(), typography::CAPTION, cx)
                        .h(px(CAPTION_HEIGHT))
                        .text_color(color::text_secondary())
                        .opacity(if dragging || focused { 1.0 } else { 0.0 })
                        .group_hover(group, |s| s.opacity(1.0))
                        .child(gain_text(gain)),
                )
                .child(
                    div().relative().child(zero_mark()).child(
                        VSlider::new(
                            ("eq-slider", band),
                            &state,
                            &self.focus_sliders[band],
                            format!("{label} Hz"),
                            SLIDER_HEIGHT,
                        )
                        .value(fraction_of(gain))
                        .value_text(format!("{} dB", gain_text(gain)))
                        .step(STEP)
                        .on_change(move |fraction, committed, _, cx| {
                            panel.update(cx, |this, cx| {
                                let current = this.player.read(cx).equalizer();
                                let gains = with_band(current.gains_db, band, gain_at(fraction));
                                this.apply(after_edit(current, gains), committed, cx);
                            });
                        }),
                    ),
                )
                .child(
                    style::text_style(div(), typography::CAPTION, cx)
                        .h(px(CAPTION_HEIGHT))
                        .text_color(color::text_tertiary())
                        .child(label),
                )
        });

        div()
            .id("equalizer-panel")
            .debug_selector(|| "equalizer-panel".into())
            .flex()
            .flex_col()
            .gap(px(spacing::S4))
            .p(px(spacing::S4))
            .w_full()
            .rounded(px(radius::MEDIUM))
            .bg(color::bg_raised())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(px(spacing::S4))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .gap(px(spacing::S2))
                            .children(chips),
                    )
                    .child(div().debug_selector(|| "eq-reset".into()).child(reset)),
            )
            .child(div().flex().flex_row().w_full().children(bands))
    }
}

/// The 0 dB line behind a slider: a hairline a little wider than the track, level with the
/// middle of it.
fn zero_mark() -> impl IntoElement {
    div()
        .absolute()
        .top(px(SLIDER_HEIGHT / 2.0))
        .left(px(10.0))
        .w(px(12.0))
        .h(px(1.0))
        .bg(color::border_strong())
}

// ---- a preset chip ---------------------------------------------------------------------------

#[derive(Default)]
struct ChipState {
    hover: HoverFade,
}

/// A preset chip: text on a 4px-radius pill, on the selected tint when it is the curve in force.
#[derive(IntoElement)]
struct Chip {
    id: ElementId,
    focus: FocusHandle,
    label: SharedString,
    selected: bool,
    ring: gpui::Hsla,
    on_click: Option<Box<dyn Fn(&mut Window, &mut App) + 'static>>,
}

impl Chip {
    fn new(
        id: impl Into<ElementId>,
        focus: &FocusHandle,
        label: impl Into<SharedString>,
        selected: bool,
        accent: Accent,
    ) -> Self {
        Self {
            id: id.into(),
            focus: focus.clone(),
            label: label.into(),
            selected,
            ring: color::accent(accent),
            on_click: None,
        }
    }

    fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Chip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let animations = !cx.reduce_motion();
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ChipState::default());
        let (level, moving) = state.read(cx).hover.level(Instant::now());
        if moving {
            window.request_animation_frame();
        }
        let ring = keyboard_focused(&self.focus, window);
        let handler = self.on_click;
        let selected = self.selected;
        let label = self.label.clone();
        let hover_state = state.clone();
        style::text_style(div(), typography::BODY, cx)
            .id(self.id)
            .debug_selector({
                let label = label.clone();
                move || format!("eq-preset-{label}")
            })
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(px(CHIP_HEIGHT))
            .px(px(spacing::S3))
            .rounded(px(radius::SMALL))
            .whitespace_nowrap()
            .cursor_pointer()
            .track_focus(&self.focus.clone().tab_stop(true))
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .text_color(if selected {
                color::text_primary()
            } else {
                color::text_secondary()
            })
            .bg(if selected {
                color::state_selected()
            } else {
                color::state_hover().opacity(level)
            })
            .on_hover(move |hovered, _, cx| {
                hover_state.update(cx, |s, cx| {
                    s.hover.set(*hovered, Instant::now(), animations);
                    cx.notify();
                });
            })
            .when_some(handler, |d, handler| {
                d.on_click(move |event, window, cx| {
                    // Space is the global play/pause key.
                    if let gpui::ClickEvent::Keyboard(k) = event {
                        if k.button == KeyboardButton::Space {
                            return;
                        }
                    }
                    handler(window, cx);
                })
            })
            .child(label)
            .when(ring, |d| d.child(focus_ring(self.ring, radius::SMALL)))
    }
}
