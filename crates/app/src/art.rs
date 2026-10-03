//! Album artwork: decode, square-crop and accent extraction. Everything here is plain pixels
//! (no GPUI types), so it runs on a background thread and is testable anywhere.

use std::sync::Arc;

use apollo_theme::{accent_from_artwork, Accent};
use gpui::RenderImage;
use image::{imageops, Frame, RgbaImage};

/// Longest side kept for display. The player bar shows 56px art; Now Playing needs up to 480.
pub const MAX_SIDE: u32 = 512;

/// A decoded, square artwork image and the accent taken from it.
#[derive(Clone)]
pub struct DecodedArt {
    pub pixels: RgbaImage,
    pub accent: Accent,
}

impl DecodedArt {
    pub fn from_rgba(pixels: RgbaImage) -> Self {
        let pixels = square(pixels);
        let accent = accent_from_artwork(
            pixels.as_raw(),
            pixels.width() as usize,
            pixels.height() as usize,
        );
        Self { pixels, accent }
    }

    /// `None` when the bytes are not a PNG/JPEG image.
    pub fn from_encoded(bytes: &[u8]) -> Option<Self> {
        let img = image::load_from_memory(bytes).ok()?;
        Some(Self::from_rgba(img.to_rgba8()))
    }

    /// GPUI draws BGRA, so swap the channels once here.
    pub fn render_image(&self) -> Arc<RenderImage> {
        let mut bgra = self.pixels.clone();
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        Arc::new(RenderImage::new(vec![Frame::new(bgra)]))
    }
}

/// Centre-crop to a square (video thumbnails are 16:9 with the cover in the middle) and cap the
/// size.
pub fn square(img: RgbaImage) -> RgbaImage {
    let (w, h) = img.dimensions();
    let side = w.min(h);
    let cropped = if w == h {
        img
    } else {
        imageops::crop_imm(&img, (w - side) / 2, (h - side) / 2, side, side).to_image()
    };
    if side > MAX_SIDE {
        imageops::resize(&cropped, MAX_SIDE, MAX_SIDE, imageops::FilterType::Triangle)
    } else {
        cropped
    }
}

/// Source of artwork for a track. Blocking; callers run it off the UI thread.
pub trait ArtSource: Send + Sync {
    fn fetch(&self, video_id: &str) -> Option<DecodedArt>;
}

/// Fixture artwork for the simulated backend: no network, deterministic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SyntheticArt {
    /// Diagonal two-tone gradient around a hue (degrees).
    Hue(f32),
    /// Grayscale only, so the accent falls back.
    Gray,
}

impl SyntheticArt {
    pub fn render(self, side: u32) -> RgbaImage {
        use apollo_theme::Oklch;
        let mut img = RgbaImage::new(side, side);
        for (x, y, px) in img.enumerate_pixels_mut() {
            let t = (x + y) as f32 / (2 * side) as f32;
            let [r, g, b] = match self {
                SyntheticArt::Hue(h) => {
                    let c = Oklch::new(0.55 + 0.25 * t, 0.15, h + 40.0 * t).to_srgb();
                    [c.r, c.g, c.b]
                }
                SyntheticArt::Gray => {
                    let v = (60.0 + 140.0 * t) as u8;
                    [v, v, v]
                }
            };
            *px = image::Rgba([r, g, b, 255]);
        }
        img
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_wide_images_to_a_centred_square() {
        let mut img = RgbaImage::new(160, 90);
        for (x, _, px) in img.enumerate_pixels_mut() {
            *px = if (35..125).contains(&x) {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 255])
            };
        }
        let sq = square(img);
        assert_eq!(sq.dimensions(), (90, 90));
        assert!(sq.pixels().all(|p| p.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn large_art_is_capped() {
        let sq = square(RgbaImage::new(1200, 1200));
        assert_eq!(sq.dimensions(), (MAX_SIDE, MAX_SIDE));
    }

    #[test]
    fn colorful_art_gets_a_non_fallback_accent_near_its_hue() {
        let art = DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(128));
        assert!(!art.accent.is_fallback);
        let dh = ((art.accent.oklch.h - 220.0 + 540.0).rem_euclid(360.0) - 180.0).abs();
        assert!(dh < 40.0, "hue {}", art.accent.oklch.h);
    }

    #[test]
    fn gray_art_falls_back() {
        let art = DecodedArt::from_rgba(SyntheticArt::Gray.render(128));
        assert!(art.accent.is_fallback);
    }

    #[test]
    fn encoded_round_trip() {
        let src = SyntheticArt::Hue(20.0).render(64);
        let mut png = Vec::new();
        src.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let art = DecodedArt::from_encoded(&png).expect("png decodes");
        assert_eq!(art.pixels.dimensions(), (64, 64));
        assert!(DecodedArt::from_encoded(b"not an image").is_none());
    }

    #[test]
    fn render_image_is_bgra() {
        let mut img = RgbaImage::new(2, 2);
        for px in img.pixels_mut() {
            *px = image::Rgba([10, 20, 30, 255]);
        }
        let ri = DecodedArt::from_rgba(img).render_image();
        assert_eq!(&ri.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
    }
}
