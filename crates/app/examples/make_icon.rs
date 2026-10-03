//! Writes `assets/apollo.ico`: the tray mark (`tray::icon_rgba`, the fixed fallback accent) at
//! the sizes Windows asks for, as PNG frames inside an ICO container. Run from the repo root
//! after changing the mark, and commit the result:
//!
//!     cargo run -p apollo-app --example make_icon

use apollo_app::tray::icon_rgba;
use apollo_theme::Accent;

const SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];

fn png(size: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_raw(size, size, icon_rgba(size, Accent::fallback()))
        .expect("icon buffer matches its size");
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode PNG");
    out.into_inner()
}

fn main() {
    let frames: Vec<(u32, Vec<u8>)> = SIZES.iter().map(|&s| (s, png(s))).collect();
    let mut ico = Vec::new();
    // ICONDIR: reserved, type 1 (icon), image count.
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, data) in &frames {
        // ICONDIRENTRY: width, height (0 means 256), colours, reserved, planes, bpp, size, offset.
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &frames {
        ico.extend_from_slice(data);
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/apollo.ico");
    std::fs::write(&path, ico).expect("write apollo.ico");
    println!("wrote {}", path.display());
}
