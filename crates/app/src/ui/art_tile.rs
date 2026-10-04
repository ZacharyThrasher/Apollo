//! Album art in a rounded square, or the quiet music-note tile when a track has none.

use std::sync::Arc;

use apollo_theme::tokens::{layout, radius};
use gpui::{div, img, prelude::*, px, Div, ImageSource, ObjectFit, RenderImage, Styled};

use crate::assets::Icon;
use crate::style::color;

/// The no-art glyph is 20px on small tiles (DIRECTION.md, Shell decisions 3) and grows with
/// larger ones, so a 480px tile does not hold a speck.
fn glyph_size(tile: f32) -> f32 {
    if tile <= 56.0 {
        layout::ICON_SIZE
    } else {
        tile / 4.0
    }
}

pub fn art_tile(size: f32, image: Option<Arc<RenderImage>>) -> Div {
    art_tile_fluid(size, image).size(px(size)).flex_none()
}

/// Like [`art_tile`] but the caller sets the width (and a square aspect); `nominal` only picks
/// the glyph size for the no-art tile.
pub fn art_tile_fluid(nominal: f32, image: Option<Arc<RenderImage>>) -> Div {
    art_tile_shaped(nominal, image, false)
}

/// [`art_tile`] as a circle (an artist) or a rounded square.
pub fn art_tile_shaped(nominal: f32, image: Option<Arc<RenderImage>>, circle: bool) -> Div {
    art_tile_fluid_shaped(nominal, image, circle)
        .size(px(nominal))
        .flex_none()
}

/// [`art_tile`] with its own corner radius (the HUD's art has smaller corners than a card's).
pub fn art_tile_rounded(size: f32, image: Option<Arc<RenderImage>>, corner: f32) -> Div {
    art_tile_corners(size, image, false, corner)
        .size(px(size))
        .flex_none()
}

fn art_tile_fluid_shaped(nominal: f32, image: Option<Arc<RenderImage>>, circle: bool) -> Div {
    art_tile_corners(nominal, image, circle, radius::MEDIUM)
}

fn art_tile_corners(
    nominal: f32,
    image: Option<Arc<RenderImage>>,
    circle: bool,
    corner: f32,
) -> Div {
    let size = nominal;
    div()
        .flex()
        .items_center()
        .justify_center()
        .map(|d| {
            if circle {
                d.rounded_full()
            } else {
                d.rounded(px(corner))
            }
        })
        .overflow_hidden()
        .bg(color::bg_overlay())
        .map(|d| match image {
            Some(image) => d.child(
                // The image carries the corners itself: GPUI clips children to the parent's
                // rectangle, not its rounded corners.
                img(ImageSource::Render(image))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .map(|i| {
                        if circle {
                            i.rounded_full()
                        } else {
                            i.rounded(px(corner))
                        }
                    }),
            ),
            // A decorative glyph, not text, so text.tertiary on bg.overlay (4.19:1) clears the
            // 3:1 non-text minimum.
            None => d.child(
                gpui::svg()
                    .path(Icon::MusicNote.path())
                    .size(px(glyph_size(size)))
                    .flex_none()
                    .text_color(color::text_tertiary()),
            ),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_glyph_stays_20px_on_small_tiles_and_scales_on_large_ones() {
        assert_eq!(glyph_size(40.0), 20.0);
        assert_eq!(glyph_size(56.0), 20.0);
        assert_eq!(glyph_size(480.0), 120.0);
    }
}
