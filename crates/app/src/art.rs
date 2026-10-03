//! Album artwork: decode, square-crop and accent extraction. Everything here is plain pixels
//! (no GPUI types), so it runs on a background thread and is testable anywhere.

use std::sync::Arc;

use apollo_theme::{accent_from_artwork, Accent};
use gpui::RenderImage;
use image::{imageops, Frame, RgbaImage};

/// Longest side kept for display. The player bar shows 56px art; Now Playing needs up to 480.
pub const MAX_SIDE: u32 = 512;
/// Side of the queue-row thumbnails.
pub const THUMB_SIDE: u32 = 96;

/// Side of the image the ambient glow is blurred at, and the side it is smoothed up to.
const AMBIENT_BLUR_SIDE: u32 = 32;
pub const AMBIENT_SIDE: u32 = 128;
/// Blur sigma as a fraction of the image side. The ambient image is drawn "cover" over the Now
/// Playing area, so on screen the radius is this fraction of the area's longer side. DIRECTION
/// asks for at least 80px; at 0.25 that holds whenever the longer side is 320px or more (the
/// smallest area is 324px: minimum window, expanded nav rail and open queue panel).
pub const AMBIENT_SIGMA_FRACTION: f32 = 0.25;

/// A decoded, square artwork image, the accent taken from it and the blurred ambient version
/// for the Now Playing background.
#[derive(Clone)]
pub struct DecodedArt {
    pub pixels: RgbaImage,
    pub ambient: RgbaImage,
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
        let ambient = ambient(&pixels);
        Self {
            pixels,
            ambient,
            accent,
        }
    }

    /// `None` when the bytes are not a PNG/JPEG image.
    pub fn from_encoded(bytes: &[u8]) -> Option<Self> {
        let img = image::load_from_memory(bytes).ok()?;
        Some(Self::from_rgba(img.to_rgba8()))
    }

    /// GPUI draws BGRA, so swap the channels once here.
    pub fn render_image(&self) -> Arc<RenderImage> {
        bgra_image(&self.pixels)
    }

    pub fn ambient_image(&self) -> Arc<RenderImage> {
        bgra_image(&self.ambient)
    }

    /// Queue-row artwork: [`THUMB_SIDE`] pixels, twice a 40px row at 2x scaling.
    pub fn thumb_image(&self) -> Arc<RenderImage> {
        bgra_image(&imageops::resize(
            &self.pixels,
            THUMB_SIDE,
            THUMB_SIDE,
            imageops::FilterType::Triangle,
        ))
    }
}

fn bgra_image(pixels: &RgbaImage) -> Arc<RenderImage> {
    let mut bgra = pixels.clone();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(bgra)]))
}

/// Heavily blurred, small copy of the artwork for the ambient glow. Blurred at a small size,
/// then smoothed up so the GPU's bilinear stretch shows no blocks.
pub fn ambient(pixels: &RgbaImage) -> RgbaImage {
    let small = imageops::resize(
        pixels,
        AMBIENT_BLUR_SIDE,
        AMBIENT_BLUR_SIDE,
        imageops::FilterType::Triangle,
    );
    let blurred = imageops::blur(&small, AMBIENT_SIGMA_FRACTION * AMBIENT_BLUR_SIDE as f32);
    imageops::resize(
        &blurred,
        AMBIENT_SIDE,
        AMBIENT_SIDE,
        imageops::FilterType::CatmullRom,
    )
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

    /// The thumbnail a listing (library, search) gave for a track this source was not told
    /// about at start-up. Without it a source can only guess the video's own frame, which is
    /// letterboxed for songs.
    fn hint(&self, _video_id: &str, _url: &str) {}
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
    fn ambient_is_small_smooth_and_keeps_the_hue() {
        let art = DecodedArt::from_rgba(SyntheticArt::Hue(200.0).render(128));
        assert_eq!(art.ambient.dimensions(), (AMBIENT_SIDE, AMBIENT_SIDE));
        // Smooth: neighbouring pixels differ by almost nothing.
        let max_step = (0..AMBIENT_SIDE - 1)
            .flat_map(|x| (0..AMBIENT_SIDE).map(move |y| (x, y)))
            .map(|(x, y)| {
                let (a, b) = (art.ambient.get_pixel(x, y), art.ambient.get_pixel(x + 1, y));
                (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap()
            })
            .max()
            .unwrap();
        assert!(max_step <= 6, "step {max_step}");
        // Blue-ish art stays blue-ish (blue channel beats red).
        let mid = art.ambient.get_pixel(AMBIENT_SIDE / 2, AMBIENT_SIDE / 2);
        assert!(mid[2] > mid[0], "{mid:?}");
    }

    #[test]
    fn ambient_blur_meets_the_direction_minimum_on_the_smallest_area() {
        // On-screen sigma = fraction x the longer side of the area the image covers.
        let smallest_longer_side = 324.0_f32;
        assert!(
            AMBIENT_SIGMA_FRACTION * smallest_longer_side
                >= apollo_theme::tokens::ambient::BLUR_RADIUS_MIN_PX
        );
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
