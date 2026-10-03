//! Static tokens from `docs/design/DIRECTION.md`. Dark only for v1.

use std::time::Duration;

pub mod color {
    use crate::Rgba;

    // Surfaces
    pub const BG_BASE: Rgba = Rgba::hex(0x0E0E10);
    /// Opacity of bg.base when Mica is active. 80% is the direction's value; the contrast
    /// test confirms it passes AA against the worst-case backdrop (no raise was needed).
    pub const MICA_BASE_ALPHA_PCT: u8 = 80;
    /// bg.base when Mica is active.
    pub const BG_BASE_MICA: Rgba = BG_BASE.with_alpha_pct(MICA_BASE_ALPHA_PCT);
    /// Worst plausible dark-mode Mica backdrop, used only to test contrast.
    pub const MICA_WORST_CASE_BACKDROP: Rgba = Rgba::hex(0x3A3A3A);
    pub const BG_RAISED: Rgba = Rgba::hex(0x17171A);
    pub const BG_OVERLAY: Rgba = Rgba::hex(0x1F1F23);

    // Interaction states and borders (white overlays)
    pub const STATE_HOVER: Rgba = Rgba::white_pct(6);
    pub const STATE_SELECTED: Rgba = Rgba::white_pct(8);
    pub const STATE_PRESSED: Rgba = Rgba::white_pct(10);
    pub const BORDER_SUBTLE: Rgba = Rgba::white_pct(6);
    pub const BORDER_STRONG: Rgba = Rgba::white_pct(12);

    // Text
    pub const TEXT_PRIMARY: Rgba = Rgba::hex(0xF2F2F3);
    pub const TEXT_SECONDARY: Rgba = Rgba::hex(0xA1A1A8);
    /// Durations, counts; never essential info. Nudged from #7C7C84 (4.32:1 on bg.raised) to pass AA.
    pub const TEXT_TERTIARY: Rgba = Rgba::hex(0x808088);
    pub const TEXT_DISABLED: Rgba = Rgba::white_pct(30);
    /// Menus, popovers and tooltips (bg.overlay) use text.secondary as their lowest text
    /// tier; text.tertiary is only ever drawn on bg.base and bg.raised.
    pub const TEXT_LOWEST_ON_OVERLAY: Rgba = TEXT_SECONDARY;

    // Skeleton blocks
    pub const SKELETON: Rgba = Rgba::white_pct(4);

    // Semantic (sparingly; there is no success color)
    pub const ERROR: Rgba = Rgba::hex(0xFF6B6B);
    pub const WARNING: Rgba = Rgba::hex(0xF5C26B);

    // Close caption button: native Windows 11 red, white glyph; pressed is 90% of it.
    pub const CLOSE_HOVER: Rgba = Rgba::hex(0xC42B1C);
    pub const CLOSE_PRESSED: Rgba = CLOSE_HOVER.with_alpha_pct(90);
    pub const CLOSE_FG: Rgba = Rgba::hex(0xFFFFFF);

    /// accent.fg: text/icons on the accent.
    pub const ACCENT_FG: Rgba = BG_BASE;
}

pub mod typography {
    pub const FONT_FAMILY: &str = "Segoe UI Variable";
    pub const FONT_FALLBACK: &str = "Segoe UI";
    /// OpenType feature for tabular numerals (times, counts).
    pub const TABULAR_NUMERALS: &str = "tnum";

    /// Display optical size applies from this size up; Text below.
    pub const DISPLAY_OPTICAL_MIN_PX: f32 = 20.0;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum OpticalSize {
        Text,
        Display,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct TextStyle {
        pub size_px: f32,
        pub weight: u16,
    }

    impl TextStyle {
        pub const fn new(size_px: f32, weight: u16) -> Self {
            Self { size_px, weight }
        }

        pub fn optical_size(self) -> OpticalSize {
            if self.size_px >= DISPLAY_OPTICAL_MIN_PX {
                OpticalSize::Display
            } else {
                OpticalSize::Text
            }
        }
    }

    pub const CAPTION: TextStyle = TextStyle::new(11.0, 400);
    pub const BODY: TextStyle = TextStyle::new(13.0, 400);
    pub const BODY_STRONG: TextStyle = TextStyle::new(13.0, 600);
    pub const LIST_TITLE: TextStyle = TextStyle::new(15.0, 400);
    pub const SECTION: TextStyle = TextStyle::new(20.0, 600);
    pub const PAGE_TITLE: TextStyle = TextStyle::new(28.0, 600);
    pub const NOW_PLAYING_TITLE: TextStyle = TextStyle::new(40.0, 600);
}

/// 4px grid, in logical pixels.
pub mod spacing {
    pub const S1: f32 = 4.0;
    pub const S2: f32 = 8.0;
    pub const S3: f32 = 12.0;
    pub const S4: f32 = 16.0;
    pub const S6: f32 = 24.0;
    pub const S8: f32 = 32.0;
    pub const S12: f32 = 48.0;
    pub const ALL: [f32; 7] = [S1, S2, S3, S4, S6, S8, S12];
}

pub mod radius {
    /// Small controls.
    pub const SMALL: f32 = 4.0;
    /// Artwork, cards.
    pub const MEDIUM: f32 = 8.0;
    /// Panels, popovers.
    pub const LARGE: f32 = 12.0;

    /// Corner radius for a shape of the given size; `Full` is a circle (play button).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum Radius {
        Px(f32),
        Full,
    }

    impl Radius {
        pub fn resolve(self, size_px: f32) -> f32 {
            match self {
                Radius::Px(px) => px,
                Radius::Full => size_px / 2.0,
            }
        }
    }
}

pub mod motion {
    use super::Duration;

    /// Hover and press.
    pub const HOVER_PRESS: Duration = Duration::from_millis(120);
    /// Panels (queue slide-in).
    pub const PANEL: Duration = Duration::from_millis(200);
    pub const ACCENT_CROSSFADE: Duration = Duration::from_millis(400);

    /// Ease-out cubic, `t` clamped to 0-1.
    pub fn ease_out_cubic(t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// Duration to use given the Windows "Animation effects" setting: instant when off.
    pub fn effective(duration: Duration, animations_enabled: bool) -> Duration {
        if animations_enabled {
            duration
        } else {
            Duration::ZERO
        }
    }
}

/// Fixed dimensions from the Layout / Spacing sections, in logical pixels.
pub mod layout {
    pub const TITLE_BAR_HEIGHT: f32 = 32.0;
    pub const NAV_RAIL_COLLAPSED: f32 = 64.0;
    pub const NAV_RAIL_EXPANDED: f32 = 220.0;
    pub const PLAYER_BAR_HEIGHT: f32 = 80.0;
    pub const PLAYER_BAR_ART: f32 = 56.0;
    /// Player bar left block: 30% of the window, clamped.
    pub const PLAYER_BAR_SIDE_FRACTION: f32 = 0.3;
    pub const PLAYER_BAR_SIDE_MIN: f32 = 240.0;
    pub const PLAYER_BAR_SIDE_MAX: f32 = 360.0;
    pub const PLAY_BUTTON: f32 = 40.0;
    pub const QUEUE_PANEL_WIDTH: f32 = 320.0;
    /// Now Playing art is `min(50vh, NOW_PLAYING_ART_MAX)`.
    pub const NOW_PLAYING_ART_MAX: f32 = 480.0;
    pub const ICON_SIZE: f32 = 20.0;
    pub const MIN_HIT_TARGET: f32 = 32.0;
    pub const FOCUS_RING_WIDTH: f32 = 2.0;
    pub const FOCUS_RING_OFFSET: f32 = 2.0;
}

/// Ambient glow behind the now-playing area only.
pub mod ambient {
    pub const BLUR_RADIUS_MIN_PX: f32 = 80.0;
    pub const OPACITY_MIN: f32 = 0.12;
    pub const OPACITY_MAX: f32 = 0.18;
}
