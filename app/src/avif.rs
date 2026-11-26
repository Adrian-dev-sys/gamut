//! AVIF support for egui's image loader (Steam store art), decoded by Windows (WIC + AV1 extension).

use image::error::{DecodingError, ImageFormatHint};
use image::{ColorType, ImageDecoder, ImageError, ImageResult};
use std::io::Read;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;

/// Registers the "avif" format with the `image` crate; egui_extras then picks it up.
pub fn register() {
    // ISO-BMFF: 4-byte box size, then "ftypavif" / "ftypavis".
    let mask: &[u8] = &[0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
    image::hooks::register_format_detection_hook("avif".into(), b"\0\0\0\0ftypavif", Some(mask));
    image::hooks::register_format_detection_hook("avif".into(), b"\0\0\0\0ftypavis", Some(mask));
    image::hooks::register_decoding_hook(
        "avif".into(),
        Box::new(|mut r| {
            let mut bytes = Vec::new();
            r.read_to_end(&mut bytes)?;
            let (w, h, mut rgba) = decode(&bytes).map_err(|e| {
                ImageError::Decoding(DecodingError::new(ImageFormatHint::Name("AVIF".into()), e.message()))
            })?;
            apply_alpha(&bytes, w, h, &mut rgba);
            Ok(Box::new(Decoded { w, h, rgba }))
        }),
    );
}

fn decode(bytes: &[u8]) -> windows::core::Result<(u32, u32, Vec<u8>)> {
    unsafe {
        // Decoder threads may not have COM yet; an "already initialized" result is fine.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = f.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = dec.GetFrame(0)?;
        let conv = f.CreateFormatConverter()?;
        conv.Initialize(&frame, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let (mut w, mut h) = (0, 0);
        conv.GetSize(&mut w, &mut h)?;
        let mut rgba = vec![0u8; w as usize * h as usize * 4];
        conv.CopyPixels(std::ptr::null(), w * 4, &mut rgba)?;
        Ok((w, h, rgba))
    }
}

/// WIC ignores AVIF's alpha (a separate monochrome AV1 item), so decode that item on its own
/// by wrapping it in a minimal AVIF, and copy its luma into the alpha channel.
fn apply_alpha(bytes: &[u8], w: u32, h: u32, rgba: &mut [u8]) {
    let Ok(d) = avif_parse::read_avif(&mut &bytes[..]) else { return };
    let Some(a) = d.alpha_item.as_deref() else { return };
    let Ok(m) = avif_parse::AV1Metadata::parse_av1_bitstream(a) else { return };
    let mut av = avif_serialize::Aviffy::new();
    av.set_monochrome(m.monochrome)
        .set_chroma_subsampling(m.chroma_subsampling)
        .set_seq_profile(m.seq_profile)
        .set_full_color_range(true);
    let file = av.to_vec(a, None, m.max_frame_width.get(), m.max_frame_height.get(), m.bit_depth);
    match decode(&file) {
        Ok((aw, ah, mask)) if (aw, ah) == (w, h) => {
            for (px, m) in rgba.chunks_exact_mut(4).zip(mask.chunks_exact(4)) {
                px[3] = m[0];
                if d.premultiplied_alpha && m[0] > 0 {
                    for c in &mut px[..3] {
                        *c = (*c as u32 * 255 / m[0] as u32).min(255) as u8;
                    }
                }
            }
        }
        Ok(_) => log::warn!("AVIF alpha size differs from the image; ignoring it"),
        Err(e) => log::warn!("AVIF alpha decode failed: {e}"),
    }
}

struct Decoded {
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

impl ImageDecoder for Decoded {
    fn dimensions(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn color_type(&self) -> ColorType {
        ColorType::Rgba8
    }
    fn read_image(self, buf: &mut [u8]) -> ImageResult<()> {
        buf.copy_from_slice(&self.rgba);
        Ok(())
    }
    fn read_image_boxed(self: Box<Self>, buf: &mut [u8]) -> ImageResult<()> {
        (*self).read_image(buf)
    }
}

#[cfg(test)]
mod tests {
    /// `GAMUT_AVIF=<file.avif> cargo test -p gamut -- --ignored avif` (writes <file>.png next to it)
    #[test]
    #[ignore]
    fn decodes_avif_file() {
        super::register();
        let path = std::env::var("GAMUT_AVIF").unwrap();
        let img = image::load_from_memory(&std::fs::read(&path).unwrap()).unwrap().to_rgba8();
        let alpha_min = img.pixels().map(|p| p[3]).min().unwrap();
        let alpha_max = img.pixels().map(|p| p[3]).max().unwrap();
        println!("{}x{} alpha {alpha_min}..{alpha_max}, centre {:?}", img.width(), img.height(), img.get_pixel(img.width() / 2, img.height() / 2));
        img.save(std::path::Path::new(&path).with_extension("png")).unwrap();
    }
    /// Full egui path: http fetch -> ImageCrateLoader -> hook. Needs network.
    #[test]
    #[ignore]
    fn egui_loads_steam_avif() {
        super::register();
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let url = "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/3265700/extras/a299aff2fbc72e320fc259435c5330ef.poster.avif?t=1776925935";
        for _ in 0..200 {
            match ctx.try_load_image(url, egui::SizeHint::default()) {
                Ok(egui::load::ImagePoll::Ready { image }) => return assert!(image.width() > 0),
                Ok(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(e) => panic!("{e}"),
            }
        }
        panic!("timed out");
    }
}
