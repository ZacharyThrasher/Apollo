use crate::accent::{clamp_accent, MAX_C, MAX_L, MIN_C, MIN_L};
use crate::tokens::color::*;
use crate::{accent_from_artwork, contrast_ratio, Accent, Oklch, Rgba, FALLBACK_ACCENT};

const AA: f32 = 4.5;

// ---------- helpers ----------

fn solid(w: usize, h: usize, [r, g, b]: [u8; 3]) -> Vec<u8> {
    (0..w * h).flat_map(|_| [r, g, b, 255]).collect()
}

/// Left `frac` of each row is `a`, the rest `b`.
fn split(w: usize, h: usize, frac: f32, a: [u8; 3], b: [u8; 3]) -> Vec<u8> {
    let edge = (w as f32 * frac) as usize;
    (0..w * h)
        .flat_map(|i| {
            let [r, g, bl] = if i % w < edge { a } else { b };
            [r, g, bl, 255]
        })
        .collect()
}

fn assert_in_bounds(a: &Accent) {
    let o = a.oklch;
    assert!((MIN_L..=MAX_L).contains(&o.l), "L {} out of bounds", o.l);
    assert!((MIN_C..=MAX_C).contains(&o.c), "C {} out of bounds", o.c);
}

fn hue_dist(a: f32, b: f32) -> f32 {
    ((a - b + 540.0).rem_euclid(360.0) - 180.0).abs()
}

fn hue_of(rgb: [u8; 3]) -> f32 {
    Oklch::from_srgb(rgb[0], rgb[1], rgb[2]).h
}

// ---------- contrast ----------

#[test]
fn text_tokens_pass_aa_on_base_and_raised() {
    for (name, fg) in [
        ("text.primary", TEXT_PRIMARY),
        ("text.secondary", TEXT_SECONDARY),
        ("text.tertiary", TEXT_TERTIARY),
    ] {
        for (bg_name, bg) in [("bg.base", BG_BASE), ("bg.raised", BG_RAISED)] {
            let ratio = contrast_ratio(fg, bg);
            assert!(ratio >= AA, "{name} on {bg_name}: {ratio:.2}:1 < {AA}:1");
        }
    }
}

#[test]
fn primary_and_secondary_pass_aa_on_overlay() {
    // Tertiary is deliberately not asserted here: it is 4.19:1 on bg.overlay (see DIRECTION.md).
    for fg in [TEXT_PRIMARY, TEXT_SECONDARY] {
        assert!(contrast_ratio(fg, BG_OVERLAY) >= AA);
    }
}

#[test]
fn accent_fg_on_fallback_accent_is_readable() {
    let a = Accent::fallback();
    assert!(contrast_ratio(a.fg, a.rgb) >= AA);
}

#[test]
fn accent_fg_is_readable_across_the_whole_clamp_range() {
    // Lowest-lightness accent at every hue still carries bg.base text at AA.
    for h in (0..360).step_by(15) {
        let a = Accent::from_oklch(Oklch::new(MIN_L, MAX_C, h as f32), false);
        let ratio = contrast_ratio(a.fg, a.rgb);
        assert!(ratio >= AA, "hue {h}: {ratio:.2}:1");
    }
}

// ---------- color math ----------

#[test]
fn srgb_oklch_round_trip() {
    for rgb in [
        [200u8, 30, 30],
        [20, 160, 90],
        [60, 90, 220],
        [128, 128, 128],
    ] {
        let back = Oklch::from_srgb(rgb[0], rgb[1], rgb[2]).to_srgb();
        for (a, b) in [(back.r, rgb[0]), (back.g, rgb[1]), (back.b, rgb[2])] {
            assert!(a.abs_diff(b) <= 1, "{rgb:?} -> {back:?}");
        }
    }
}

#[test]
fn known_oklch_values() {
    let white = Oklch::from_srgb(255, 255, 255);
    assert!((white.l - 1.0).abs() < 1e-3 && white.c < 1e-3);
    let black = Oklch::from_srgb(0, 0, 0);
    assert!(black.l < 1e-3);
    // sRGB red is roughly OKLCH(0.628, 0.258, 29.2).
    let red = Oklch::from_srgb(255, 0, 0);
    assert!(
        (red.l - 0.628).abs() < 0.01 && (red.c - 0.258).abs() < 0.01 && (red.h - 29.2).abs() < 1.0
    );
}

#[test]
fn out_of_gamut_keeps_hue_and_lightness() {
    // High-chroma green at the top of the L range does not fit sRGB.
    let wild = Oklch::new(0.78, 0.18, 145.0);
    let rgb = wild.to_srgb();
    let back = Oklch::from_srgb(rgb.r, rgb.g, rgb.b);
    assert!(hue_dist(back.h, 145.0) < 3.0, "hue {}", back.h);
    assert!((back.l - 0.78).abs() < 0.01, "L {}", back.l);
}

#[test]
fn fallback_is_in_gamut() {
    assert!(FALLBACK_ACCENT.in_srgb_gamut());
}

// ---------- lerp ----------

#[test]
fn lerp_endpoints() {
    let (a, b) = (Oklch::new(0.70, 0.10, 20.0), Oklch::new(0.75, 0.15, 200.0));
    assert_eq!(a.lerp(b, 0.0), a);
    let end = a.lerp(b, 1.0);
    assert!(
        (end.l - b.l).abs() < 1e-6 && (end.c - b.c).abs() < 1e-6 && hue_dist(end.h, b.h) < 1e-3
    );
}

#[test]
fn lerp_takes_shortest_hue_arc_across_zero() {
    let mid = Oklch::new(0.7, 0.1, 350.0).lerp(Oklch::new(0.7, 0.1, 10.0), 0.5);
    assert!(hue_dist(mid.h, 0.0) < 1e-3, "got {}", mid.h);
    let back = Oklch::new(0.7, 0.1, 10.0).lerp(Oklch::new(0.7, 0.1, 350.0), 0.25);
    assert!(hue_dist(back.h, 5.0) < 1e-3, "got {}", back.h);
}

#[test]
fn lerp_midpoint_l_and_c() {
    let mid = Oklch::new(0.68, 0.08, 100.0).lerp(Oklch::new(0.78, 0.18, 120.0), 0.5);
    assert!(
        (mid.l - 0.73).abs() < 1e-6 && (mid.c - 0.13).abs() < 1e-6 && hue_dist(mid.h, 110.0) < 1e-3
    );
}

#[test]
fn accent_lerp_stays_within_clamp_bounds() {
    let from = accent_from_artwork(&solid(64, 64, [200, 30, 30]), 64, 64);
    let to = accent_from_artwork(&solid(64, 64, [30, 60, 220]), 64, 64);
    for i in 0..=10 {
        assert_in_bounds(&from.lerp(to, i as f32 / 10.0));
    }
}

// ---------- extraction ----------

#[test]
fn vivid_art_keeps_hue() {
    for rgb in [
        [200u8, 30, 30],
        [30, 160, 70],
        [40, 80, 230],
        [230, 140, 20],
    ] {
        let a = accent_from_artwork(&solid(64, 64, rgb), 64, 64);
        assert!(!a.is_fallback, "{rgb:?}");
        assert_in_bounds(&a);
        assert!(
            hue_dist(a.oklch.h, hue_of(rgb)) < 2.0,
            "{rgb:?}: {} vs {}",
            a.oklch.h,
            hue_of(rgb)
        );
    }
}

#[test]
fn dominant_vivid_cluster_wins() {
    // 70% blue, 30% red.
    let px = split(64, 64, 0.7, [30, 60, 220], [210, 30, 30]);
    let a = accent_from_artwork(&px, 64, 64);
    assert!(
        hue_dist(a.oklch.h, hue_of([30, 60, 220])) < 5.0,
        "hue {}",
        a.oklch.h
    );
}

#[test]
fn vivid_accent_ignores_gray_majority() {
    // 90% mid-gray, black and white borders, 10% orange: the orange is the accent.
    let mut px = split(64, 64, 0.9, [128, 128, 128], [230, 140, 20]);
    for p in px.chunks_mut(4).take(64 * 4) {
        p[..3].copy_from_slice(&[0, 0, 0]);
    }
    for p in px.chunks_mut(4).skip(64 * 60) {
        p[..3].copy_from_slice(&[255, 255, 255]);
    }
    let a = accent_from_artwork(&px, 64, 64);
    assert!(!a.is_fallback);
    assert!(
        hue_dist(a.oklch.h, hue_of([230, 140, 20])) < 5.0,
        "hue {}",
        a.oklch.h
    );
}

#[test]
fn grayscale_art_falls_back() {
    let gradient: Vec<u8> = (0..64 * 64)
        .flat_map(|i| {
            let v = (i % 64 * 4) as u8;
            [v, v, v, 255]
        })
        .collect();
    let a = accent_from_artwork(&gradient, 64, 64);
    assert!(a.is_fallback);
    assert_eq!(a.oklch, FALLBACK_ACCENT);
}

#[test]
fn near_gray_tint_falls_back() {
    // Slightly warm gray: chroma below the near-gray cut-off.
    let a = accent_from_artwork(&solid(64, 64, [128, 125, 122]), 64, 64);
    assert!(a.is_fallback);
}

#[test]
fn near_black_and_near_white_art_fall_back() {
    for rgb in [
        [0u8, 0, 0],
        [12, 10, 16],
        [10, 0, 2],
        [255, 255, 255],
        [250, 246, 248],
    ] {
        let a = accent_from_artwork(&solid(64, 64, rgb), 64, 64);
        assert!(a.is_fallback, "{rgb:?} should fall back, got {:?}", a.oklch);
    }
}

#[test]
fn tiny_vivid_speck_falls_back() {
    // One vivid pixel in 4096 is below MIN_VIVID_SHARE.
    let mut px = solid(64, 64, [128, 128, 128]);
    px[..3].copy_from_slice(&[255, 0, 0]);
    assert!(accent_from_artwork(&px, 64, 64).is_fallback);
}

#[test]
fn degenerate_inputs_fall_back() {
    assert!(accent_from_artwork(&[], 0, 0).is_fallback);
    assert!(accent_from_artwork(&[255, 0, 0, 255], 0, 1).is_fallback);
    assert!(accent_from_artwork(&[255, 0, 0, 255], 2, 2).is_fallback); // short buffer
    assert!(accent_from_artwork(&[], usize::MAX, usize::MAX).is_fallback);
    // fully transparent
    let clear: Vec<u8> = (0..64 * 64).flat_map(|_| [255, 0, 0, 0]).collect();
    assert!(accent_from_artwork(&clear, 64, 64).is_fallback);
}

#[test]
fn clamp_raises_dark_and_mutes_pale() {
    // Deep red: below the L floor.
    let dark = accent_from_artwork(&solid(64, 64, [90, 0, 10]), 64, 64);
    assert!(!dark.is_fallback);
    assert_eq!(dark.oklch.l, MIN_L);
    // Pale pink: above the L ceiling, below the C floor.
    let pale = accent_from_artwork(&solid(64, 64, [250, 215, 225]), 64, 64);
    assert!(!pale.is_fallback);
    assert_eq!(pale.oklch.l, MAX_L);
    assert_eq!(pale.oklch.c, MIN_C);
}

#[test]
fn clamp_caps_chroma_and_keeps_hue() {
    let neon = accent_from_artwork(&solid(64, 64, [0, 255, 0]), 64, 64);
    assert_eq!(neon.oklch.c, MAX_C);
    assert!(hue_dist(neon.oklch.h, hue_of([0, 255, 0])) < 2.0);
}

#[test]
fn clamp_accent_bounds_exact() {
    let lo = clamp_accent(Oklch::new(0.0, 0.0, 33.0));
    let hi = clamp_accent(Oklch::new(1.0, 0.4, 33.0));
    assert_eq!((lo.l, lo.c, lo.h), (MIN_L, MIN_C, 33.0));
    assert_eq!((hi.l, hi.c, hi.h), (MAX_L, MAX_C, 33.0));
    let mid = Oklch::new(0.72, 0.12, 275.0);
    assert_eq!(clamp_accent(mid), mid);
}

#[test]
fn large_art_is_downscaled_and_deterministic() {
    let (w, h) = (517, 389); // non-multiples of 64
    let px = split(w, h, 0.6, [30, 160, 70], [200, 30, 30]);
    let a = accent_from_artwork(&px, w, h);
    assert_eq!(a, accent_from_artwork(&px, w, h));
    assert!(hue_dist(a.oklch.h, hue_of([30, 160, 70])) < 5.0);
}

#[test]
fn small_art_is_not_upscaled_but_works() {
    let a = accent_from_artwork(&solid(3, 2, [40, 80, 230]), 3, 2);
    assert!(!a.is_fallback);
    assert_in_bounds(&a);
}

#[test]
fn accent_rgb_matches_oklch_and_fg_is_bg_base() {
    let a = accent_from_artwork(&solid(64, 64, [200, 30, 30]), 64, 64);
    assert_eq!(a.fg, BG_BASE);
    assert_eq!(a.rgb, a.oklch.to_srgb());
}

// ---------- tokens ----------

#[test]
fn alpha_percentages() {
    assert_eq!(STATE_HOVER.a, 15); // 6%
    assert_eq!(STATE_SELECTED.a, 20); // 8%
    assert_eq!(STATE_PRESSED.a, 26); // 10%
    assert_eq!(BORDER_STRONG.a, 31); // 12%
    assert_eq!(TEXT_DISABLED.a, 77); // 30%
    assert_eq!(BG_BASE_MICA.a, 204); // 80%
    assert_eq!(BG_BASE_MICA.with_alpha_pct(100), BG_BASE);
}

#[test]
fn hex_values() {
    assert_eq!(BG_BASE, Rgba::rgb(0x0E, 0x0E, 0x10));
    assert_eq!(ERROR, Rgba::rgb(0xFF, 0x6B, 0x6B));
}

#[test]
fn spacing_is_on_the_4px_grid() {
    use crate::tokens::spacing::ALL;
    assert_eq!(ALL, [4.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0]);
    assert!(ALL.iter().all(|s| s % 4.0 == 0.0));
}

#[test]
fn optical_size_switches_at_20px() {
    use crate::tokens::typography::*;
    assert_eq!(LIST_TITLE.optical_size(), OpticalSize::Text);
    assert_eq!(SECTION.optical_size(), OpticalSize::Display);
    assert_eq!(NOW_PLAYING_TITLE.optical_size(), OpticalSize::Display);
}

#[test]
fn motion() {
    use crate::tokens::motion::*;
    use std::time::Duration;
    assert_eq!(effective(PANEL, true), Duration::from_millis(200));
    assert_eq!(effective(ACCENT_CROSSFADE, false), Duration::ZERO);
    assert_eq!((ease_out_cubic(0.0), ease_out_cubic(1.0)), (0.0, 1.0));
    assert!(ease_out_cubic(0.5) > 0.5);
    assert_eq!(ease_out_cubic(2.0), 1.0);
}
