//! Accent extraction from album art, per `docs/design/DIRECTION.md` (Color > Accent).
//!
//! Pipeline: box-downscale to at most 64x64, drop transparent / near-gray / near-black /
//! near-white pixels, median-cut the rest into 5 clusters in OKLab, take the most
//! populous cluster, convert its mean to OKLCH and clamp L and C (hue kept).

use crate::color::Rgba;
use crate::oklch::{srgb_to_oklab, Oklch};
use crate::tokens::color::BG_BASE;

pub const MIN_L: f32 = 0.68;
pub const MAX_L: f32 = 0.78;
pub const MIN_C: f32 = 0.08;
pub const MAX_C: f32 = 0.18;

/// Used when there is no art, or the art has no vivid pixels.
pub const FALLBACK_ACCENT: Oklch = Oklch::new(0.72, 0.12, 275.0);

/// Side length the artwork is downscaled to before analysis.
pub const SAMPLE_SIZE: usize = 64;
/// Median-cut cluster count.
pub const CLUSTERS: usize = 5;

// The direction says "ignoring near-gray, near-black and near-white" without numbers;
// these are the chosen cut-offs (OKLCH of the downscaled pixel).
pub const GRAY_CHROMA_BELOW: f32 = 0.04;
pub const BLACK_LIGHTNESS_BELOW: f32 = 0.18;
pub const WHITE_LIGHTNESS_ABOVE: f32 = 0.92;
/// Fewer vivid pixels than this share of the opaque pixels counts as "no vivid color".
pub const MIN_VIVID_SHARE: f32 = 0.01;

/// A resolved accent: the clamped OKLCH value, its sRGB rendering, and the on-accent color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Accent {
    pub oklch: Oklch,
    /// `oklch` as opaque sRGB (chroma reduced if needed to fit the gamut).
    pub rgb: Rgba,
    /// Text/icons drawn on the accent (`accent.fg` = bg.base).
    pub fg: Rgba,
    pub is_fallback: bool,
}

impl Accent {
    pub(crate) fn from_oklch(oklch: Oklch, is_fallback: bool) -> Self {
        Self {
            oklch,
            rgb: oklch.to_srgb(),
            fg: BG_BASE,
            is_fallback,
        }
    }

    pub fn fallback() -> Self {
        Self::from_oklch(FALLBACK_ACCENT, true)
    }

    /// Crossfade frame between two accents; `t` is eased progress 0-1, OKLCH interpolation
    /// along the shortest hue arc. The result is never flagged as fallback.
    pub fn lerp(self, to: Accent, t: f32) -> Accent {
        Self::from_oklch(self.oklch.lerp(to.oklch, t.clamp(0.0, 1.0)), false)
    }
}

impl Default for Accent {
    fn default() -> Self {
        Self::fallback()
    }
}

/// Extract the accent from straight-alpha RGBA8 pixels (`w * h * 4` bytes, row-major).
/// Malformed input (zero size, short buffer) yields the fallback.
pub fn accent_from_artwork(rgba: &[u8], w: usize, h: usize) -> Accent {
    let Some(pixels) = downscale(rgba, w, h) else {
        return Accent::fallback();
    };
    let opaque = pixels.len();
    let vivid: Vec<[f32; 3]> = pixels
        .iter()
        .map(|&[r, g, b]| srgb_to_oklab(r, g, b))
        .filter(|&lab| is_vivid(Oklch::from_oklab(lab)))
        .collect();
    if opaque == 0 || (vivid.len() as f32) < opaque as f32 * MIN_VIVID_SHARE {
        return Accent::fallback();
    }

    let dominant = median_cut(vivid, CLUSTERS)
        .into_iter()
        .max_by_key(|cluster| cluster.len())
        .expect("median_cut returns at least one cluster for non-empty input");
    let n = dominant.len() as f32;
    let mean = dominant.iter().fold([0.0; 3], |acc, p| {
        [acc[0] + p[0], acc[1] + p[1], acc[2] + p[2]]
    });
    let mean = Oklch::from_oklab([mean[0] / n, mean[1] / n, mean[2] / n]);

    Accent::from_oklch(clamp_accent(mean), false)
}

/// Clamp L to 0.68-0.78 and C to 0.08-0.18, keeping hue.
pub fn clamp_accent(c: Oklch) -> Oklch {
    Oklch::new(c.l.clamp(MIN_L, MAX_L), c.c.clamp(MIN_C, MAX_C), c.h)
}

fn is_vivid(c: Oklch) -> bool {
    c.c >= GRAY_CHROMA_BELOW && c.l >= BLACK_LIGHTNESS_BELOW && c.l <= WHITE_LIGHTNESS_ABOVE
}

/// Box-average to at most `SAMPLE_SIZE`^2 (never upscales). Pixels with alpha < 128 are
/// skipped; output holds one opaque sRGB triple per cell that had any opaque source pixel.
fn downscale(rgba: &[u8], w: usize, h: usize) -> Option<Vec<[u8; 3]>> {
    if w == 0 || h == 0 || rgba.len() < w.checked_mul(h)?.checked_mul(4)? {
        return None;
    }
    let (ow, oh) = (w.min(SAMPLE_SIZE), h.min(SAMPLE_SIZE));
    let mut out = Vec::with_capacity(ow * oh);
    for oy in 0..oh {
        let (y0, y1) = (oy * h / oh, ((oy + 1) * h / oh).max(oy * h / oh + 1));
        for ox in 0..ow {
            let (x0, x1) = (ox * w / ow, ((ox + 1) * w / ow).max(ox * w / ow + 1));
            let (mut sum, mut n) = ([0u32; 3], 0u32);
            for y in y0..y1 {
                for x in x0..x1 {
                    let p = &rgba[(y * w + x) * 4..][..4];
                    if p[3] >= 128 {
                        sum[0] += p[0] as u32;
                        sum[1] += p[1] as u32;
                        sum[2] += p[2] as u32;
                        n += 1;
                    }
                }
            }
            if let (Some(r), Some(g), Some(b)) = (
                sum[0].checked_div(n),
                sum[1].checked_div(n),
                sum[2].checked_div(n),
            ) {
                out.push([r as u8, g as u8, b as u8]);
            }
        }
    }
    Some(out)
}

/// Median-cut in OKLab: repeatedly split the box with the widest extent at its median
/// along that axis until `k` boxes exist or nothing can be split. Deterministic.
fn median_cut(points: Vec<[f32; 3]>, k: usize) -> Vec<Vec<[f32; 3]>> {
    fn widest_axis(points: &[[f32; 3]]) -> (usize, f32) {
        (0..3)
            .map(|axis| {
                let (lo, hi) = points.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
                    (lo.min(p[axis]), hi.max(p[axis]))
                });
                (axis, hi - lo)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("three axes")
    }

    let mut boxes = vec![points];
    while boxes.len() < k {
        let Some((idx, axis)) = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 1)
            .map(|(i, b)| (i, widest_axis(b)))
            .filter(|(_, (_, extent))| *extent > 0.0)
            .max_by(|a, b| a.1 .1.total_cmp(&b.1 .1))
            .map(|(i, (axis, _))| (i, axis))
        else {
            break;
        };
        let mut b = boxes.swap_remove(idx);
        b.sort_by(|p, q| p[axis].total_cmp(&q[axis]));
        let upper = b.split_off(b.len() / 2);
        boxes.push(b);
        boxes.push(upper);
    }
    boxes
}
