//! Apollo design tokens ("Art-led dark") and album-art accent extraction.
//!
//! Source of truth for the look is `docs/design/DIRECTION.md`. This crate has no GPUI
//! dependency; the app converts these plain values into GPUI types at the edge.

pub mod accent;
pub mod color;
pub mod oklch;
pub mod tokens;

pub use accent::{accent_from_artwork, Accent, FALLBACK_ACCENT};
pub use color::{contrast_ratio, Rgba};
pub use oklch::Oklch;

#[cfg(test)]
mod tests;
