//! OKLab / OKLCH math (Björn Ottosson's matrices) and sRGB conversion.

use crate::color::Rgba;

/// OKLCH color: lightness 0-1, chroma ~0-0.4, hue in degrees `[0, 360)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

pub(crate) fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB bytes to OKLab `[L, a, b]`.
pub fn srgb_to_oklab(r: u8, g: u8, b: u8) -> [f32; 3] {
    let (r, g, b) = (
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
    );
    let l = (0.412_221_47 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab to *linear* sRGB (may fall outside 0-1 when out of gamut).
fn oklab_to_linear_srgb([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l, m, s) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_93 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_4 * s,
        -0.004_196_086 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

impl Oklch {
    pub const fn new(l: f32, c: f32, h: f32) -> Self {
        Self { l, c, h }
    }

    pub fn from_oklab([l, a, b]: [f32; 3]) -> Self {
        Self {
            l,
            c: a.hypot(b),
            h: b.atan2(a).to_degrees().rem_euclid(360.0),
        }
    }

    pub fn to_oklab(self) -> [f32; 3] {
        let h = self.h.to_radians();
        [self.l, self.c * h.cos(), self.c * h.sin()]
    }

    pub fn from_srgb(r: u8, g: u8, b: u8) -> Self {
        Self::from_oklab(srgb_to_oklab(r, g, b))
    }

    fn linear_srgb(self) -> [f32; 3] {
        oklab_to_linear_srgb(self.to_oklab())
    }

    /// True when the color is representable in sRGB (small tolerance for rounding).
    pub fn in_srgb_gamut(self) -> bool {
        self.linear_srgb()
            .iter()
            .all(|c| (-1e-4..=1.0 + 1e-4).contains(c))
    }

    /// Convert to opaque sRGB. Out-of-gamut colors lose chroma (hue and lightness kept)
    /// until they fit, so the displayed color never shifts hue.
    pub fn to_srgb(self) -> Rgba {
        let mut color = self;
        if !color.in_srgb_gamut() {
            let (mut lo, mut hi) = (0.0_f32, color.c);
            for _ in 0..24 {
                let mid = (lo + hi) / 2.0;
                if (Oklch { c: mid, ..color }).in_srgb_gamut() {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            color.c = lo;
        }
        let [r, g, b] = color.linear_srgb();
        let byte = |c: f32| (linear_to_srgb(c.clamp(0.0, 1.0)) * 255.0).round() as u8;
        Rgba::rgb(byte(r), byte(g), byte(b))
    }

    /// Interpolate toward `to` at `t` (0-1): L and C linearly, hue along the shortest arc.
    pub fn lerp(self, to: Oklch, t: f32) -> Oklch {
        let dh = (to.h - self.h + 540.0).rem_euclid(360.0) - 180.0;
        Oklch {
            l: self.l + (to.l - self.l) * t,
            c: self.c + (to.c - self.c) * t,
            h: (self.h + dh * t).rem_euclid(360.0),
        }
    }
}
