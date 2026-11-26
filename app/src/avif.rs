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
            let (w, h, rgba) = decode(&bytes).map_err(|e| {
                ImageError::Decoding(DecodingError::new(ImageFormatHint::Name("AVIF".into()), e.message()))
            })?;
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
    /// `GAMUT_AVIF=<file.avif> cargo test -p gamut -- --ignored avif`
    #[test]
    #[ignore]
    fn decodes_avif_file() {
        super::register();
        let bytes = std::fs::read(std::env::var("GAMUT_AVIF").unwrap()).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        assert!(img.width() > 0 && img.height() > 0);
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
