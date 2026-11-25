use anyhow::Result;
use image::imageops::FilterType;
use std::path::Path;

/// Grid thumbnails are pre-scaled to this size (2:3), sharp enough for ~200px cards at 1.5x DPI.
pub const THUMB_W: u32 = 300;
pub const THUMB_H: u32 = 450;

pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Stores the original cover and writes its grid thumbnail.
pub fn store_cover(bytes: &[u8], cover: &Path, thumb: &Path) -> Result<()> {
    let img = image::load_from_memory(bytes)?;
    let t = img.resize_to_fill(THUMB_W, THUMB_H, FilterType::Triangle).to_rgb8();
    let mut out = std::io::BufWriter::new(std::fs::File::create(thumb)?);
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88).encode_image(&t)?;
    std::fs::write(cover, bytes)?;
    Ok(())
}

/// Validates and stores a hero/background image.
pub fn store_hero(bytes: &[u8], hero: &Path) -> Result<()> {
    image::guess_format(bytes)?;
    std::fs::write(hero, bytes)?;
    Ok(())
}

/// Decodes an image file to RGBA, downscaling so it fits within `max_w` wide.
pub fn decode(path: &Path, max_w: u32) -> Result<Rgba> {
    // Sniff format from content (cover/hero files use a neutral .img extension).
    let mut img = image::load_from_memory(&std::fs::read(path)?)?;
    if img.width() > max_w {
        let h = (img.height() as u64 * max_w as u64 / img.width() as u64) as u32;
        img = img.resize_exact(max_w, h.max(1), FilterType::Triangle);
    }
    let rgba = img.to_rgba8();
    Ok(Rgba { width: rgba.width(), height: rgba.height(), pixels: rgba.into_raw() })
}
