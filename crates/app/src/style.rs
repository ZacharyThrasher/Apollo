//! Turns `apollo-theme` tokens into GPUI values. The only place theme types meet GPUI types, so
//! no view carries a literal color, size or duration of its own.

use apollo_theme::tokens::typography::{self, OpticalSize, TextStyle};
use apollo_theme::Rgba;
use gpui::{px, App, FontFeatures, FontWeight, Global, Hsla, SharedString, Styled};

/// Token color to GPUI color (straight alpha).
pub fn hsla(c: Rgba) -> Hsla {
    gpui::Rgba {
        r: f32::from(c.r) / 255.0,
        g: f32::from(c.g) / 255.0,
        b: f32::from(c.b) / 255.0,
        a: c.alpha(),
    }
    .into()
}

/// Font families that exist on this machine, chosen once at startup.
///
/// The direction asks for Segoe UI Variable (Display optical size from 20px, Text below) with
/// Segoe UI as the fallback. DirectWrite lists the optical sizes as separate families, so the
/// first installed candidate of each list wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fonts {
    pub text: SharedString,
    pub display: SharedString,
}

impl Global for Fonts {}

impl Fonts {
    pub fn detect(installed: &[SharedString]) -> Self {
        let pick = |candidates: &[String]| -> SharedString {
            candidates
                .iter()
                .find(|c| installed.iter().any(|i| i.as_ref() == c.as_str()))
                .cloned()
                .unwrap_or_else(|| typography::FONT_FALLBACK.to_string())
                .into()
        };
        let base = typography::FONT_FAMILY;
        Self {
            text: pick(&[
                format!("{base} Text"),
                base.to_string(),
                typography::FONT_FALLBACK.into(),
            ]),
            display: pick(&[
                format!("{base} Display"),
                format!("{base} Text"),
                base.to_string(),
                typography::FONT_FALLBACK.into(),
            ]),
        }
    }

    pub fn install(cx: &mut App) {
        let installed = cx.text_system().all_font_names();
        let fonts = Self::detect(
            &installed
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        );
        #[cfg(debug_assertions)]
        eprintln!("fonts: text={} display={}", fonts.text, fonts.display);
        cx.set_global(fonts);
    }

    pub fn family_for(&self, style: TextStyle) -> SharedString {
        match style.optical_size() {
            OpticalSize::Display => self.display.clone(),
            OpticalSize::Text => self.text.clone(),
        }
    }
}

/// Apply a type-scale entry: family (by optical size), size and weight.
pub fn text_style<E: Styled>(el: E, style: TextStyle, cx: &App) -> E {
    el.font_family(cx.global::<Fonts>().family_for(style))
        .text_size(px(style.size_px))
        .font_weight(FontWeight(f32::from(style.weight)))
}

/// Tabular numerals for times and counts.
pub fn tabular<E: Styled>(el: E) -> E {
    el.font_features(FontFeatures(std::sync::Arc::new(vec![(
        typography::TABULAR_NUMERALS.to_string(),
        1,
    )])))
}

/// Token colors as GPUI colors. `bg_base` already includes the Mica alpha, because the window
/// always runs on the Mica backdrop.
pub mod color {
    use apollo_theme::tokens::color as t;
    use gpui::Hsla;

    use super::hsla;

    pub fn bg_base() -> Hsla {
        hsla(t::BG_BASE_MICA)
    }
    pub fn bg_raised() -> Hsla {
        hsla(t::BG_RAISED)
    }
    pub fn bg_overlay() -> Hsla {
        hsla(t::BG_OVERLAY)
    }
    pub fn state_hover() -> Hsla {
        hsla(t::STATE_HOVER)
    }
    pub fn state_selected() -> Hsla {
        hsla(t::STATE_SELECTED)
    }
    pub fn state_pressed() -> Hsla {
        hsla(t::STATE_PRESSED)
    }
    pub fn border_subtle() -> Hsla {
        hsla(t::BORDER_SUBTLE)
    }
    pub fn border_strong() -> Hsla {
        hsla(t::BORDER_STRONG)
    }
    pub fn text_primary() -> Hsla {
        hsla(t::TEXT_PRIMARY)
    }
    pub fn text_secondary() -> Hsla {
        hsla(t::TEXT_SECONDARY)
    }
    pub fn text_tertiary() -> Hsla {
        hsla(t::TEXT_TERTIARY)
    }
    pub fn text_disabled() -> Hsla {
        hsla(t::TEXT_DISABLED)
    }
    pub fn error() -> Hsla {
        hsla(t::ERROR)
    }
    pub fn warning() -> Hsla {
        hsla(t::WARNING)
    }
    pub fn close_hover() -> Hsla {
        hsla(t::CLOSE_HOVER)
    }
    pub fn close_pressed() -> Hsla {
        hsla(t::CLOSE_PRESSED)
    }
    pub fn close_fg() -> Hsla {
        hsla(t::CLOSE_FG)
    }
    pub fn accent(a: apollo_theme::Accent) -> Hsla {
        hsla(a.rgb)
    }
    pub fn accent_fg(a: apollo_theme::Accent) -> Hsla {
        hsla(a.fg)
    }
}
