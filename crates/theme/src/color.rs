//! 8-bit sRGB color with straight (non-premultiplied) alpha, plus WCAG contrast.

/// sRGB color, straight alpha (`a` 255 = opaque).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// `0xRRGGBB`, opaque.
    pub const fn hex(rgb: u32) -> Self {
        Self::rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }

    /// White at `pct` percent opacity (0-100).
    pub const fn white_pct(pct: u8) -> Self {
        Self::rgb(255, 255, 255).with_alpha_pct(pct)
    }

    /// Same color at `pct` percent opacity (0-100), rounded to the nearest alpha step.
    pub const fn with_alpha_pct(self, pct: u8) -> Self {
        Self {
            a: ((pct as u32 * 255 + 50) / 100) as u8,
            ..self
        }
    }

    /// Alpha as 0.0-1.0.
    pub fn alpha(self) -> f32 {
        self.a as f32 / 255.0
    }

    /// Composite `self` over an opaque `backdrop`.
    pub fn over(self, backdrop: Rgba) -> Rgba {
        let a = self.alpha();
        let mix = |f: u8, b: u8| (f as f32 * a + b as f32 * (1.0 - a)).round() as u8;
        Rgba::rgb(
            mix(self.r, backdrop.r),
            mix(self.g, backdrop.g),
            mix(self.b, backdrop.b),
        )
    }

    /// WCAG relative luminance of the color's RGB (alpha ignored).
    pub fn relative_luminance(self) -> f32 {
        let lin = |c: u8| crate::oklch::srgb_to_linear(c as f32 / 255.0);
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }
}

/// WCAG contrast ratio of `fg` (composited over `bg` if translucent) against opaque `bg`.
pub fn contrast_ratio(fg: Rgba, bg: Rgba) -> f32 {
    let l1 = fg.over(bg).relative_luminance();
    let l2 = bg.relative_luminance();
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}
