//! Short store clips (Steam description videos) decoded with Windows Media Foundation.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;

/// Frame-by-frame RGBA reader for an in-memory video (video stream only).
pub struct Reader {
    r: IMFSourceReader,
    pub w: u32,
    pub h: u32,
    buf_h: u32,
    stride: i32,
}

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;

impl Reader {
    /// `mime` (e.g. "video/webm") tells Media Foundation the container, since there's no file name.
    pub fn open(bytes: &[u8], mime: &str) -> Result<Self> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, MFSTARTUP_LITE).context("MFStartup")?;
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 1)?;
            let attrs = attrs.context("no attributes")?;
            // Lets the reader convert decoder output (NV12 etc.) to RGB32 for us.
            attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
            let stream = windows::Win32::UI::Shell::SHCreateMemStream(Some(bytes)).context("SHCreateMemStream")?;
            let bs = MFCreateMFByteStreamOnStream(&stream)?;
            let bs_attrs: IMFAttributes = windows::core::Interface::cast(&bs)?;
            bs_attrs.SetString(&MF_BYTESTREAM_CONTENT_TYPE, &windows::core::HSTRING::from(mime))?;
            let r = MFCreateSourceReaderFromByteStream(&bs, &attrs).with_context(|| format!("opening {mime}"))?;
            r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            r.SetStreamSelection(VIDEO, true)?;
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            r.SetCurrentMediaType(VIDEO, None, &t).context("no decoder for this video")?;
            let cur = r.GetCurrentMediaType(VIDEO)?;
            let fs = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
            let (bw, bh) = ((fs >> 32) as u32, fs as u32);
            let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(bw as i32 * 4);
            // Decoders may pad the buffer (360 -> 368); the aperture is the visible part.
            let mut area = MFVideoArea::default();
            let blob = std::slice::from_raw_parts_mut(&mut area as *mut _ as *mut u8, std::mem::size_of::<MFVideoArea>());
            let (w, h) = match cur.GetBlob(&MF_MT_MINIMUM_DISPLAY_APERTURE, blob, None) {
                Ok(()) if area.Area.cx > 0 && area.Area.cy > 0 => (area.Area.cx as u32, area.Area.cy as u32),
                _ => (bw, bh),
            };
            Ok(Self { r, w: w.min(bw), h: h.min(bh), buf_h: bh, stride })
        }
    }

    /// Next frame as (timestamp in seconds, RGBA pixels); None at the end of the clip.
    pub fn next(&mut self) -> Result<Option<(f64, Vec<u8>)>> {
        unsafe {
            loop {
                let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
                self.r.ReadSample(VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    return Ok(None);
                }
                let Some(sample) = sample else { continue };
                let buf = sample.ConvertToContiguousBuffer()?;
                let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                buf.Lock(&mut p, None, Some(&mut len))?;
                let data = std::slice::from_raw_parts(p, len as usize);
                let out = self.to_rgba(data);
                buf.Unlock()?;
                return Ok(out.map(|px| (ts as f64 / 1e7, px)));
            }
        }
    }

    /// BGRX rows (top-down or bottom-up) -> tightly packed RGBA.
    fn to_rgba(&self, data: &[u8]) -> Option<Vec<u8>> {
        let row = self.stride.unsigned_abs() as usize;
        if data.len() < row * self.buf_h as usize {
            return None;
        }
        let mut out = Vec::with_capacity(self.w as usize * self.h as usize * 4);
        for y in 0..self.h as usize {
            let sy = if self.stride < 0 { self.buf_h as usize - 1 - y } else { y };
            for px in data[sy * row..][..self.w as usize * 4].chunks_exact(4) {
                out.extend_from_slice(&[px[2], px[1], px[0], 255]);
            }
        }
        Some(out)
    }
}

/// Steam keeps a clip next to each `.poster.avif` still; WebM (VP9) first since HEVC needs a paid extension.
fn clip_urls(poster: &str) -> Vec<String> {
    let (path, query) = poster.split_once('?').map_or((poster, ""), |(p, q)| (p, q));
    let Some(base) = path.strip_suffix(".poster.avif") else { return Vec::new() };
    let q = if query.is_empty() { String::new() } else { format!("?{query}") };
    ["webm", "mp4"].iter().map(|e| format!("{base}.{e}{q}")).collect()
}

pub fn is_clip_poster(url: &str) -> bool {
    !clip_urls(url).is_empty()
}

const MAX_PLAYING: usize = 3;
/// A clip stops decoding this long after it was last on screen.
const IDLE: Duration = Duration::from_millis(1500);

#[derive(Default)]
struct Clip {
    seen: Mutex<Option<Instant>>,
    frame: Mutex<Option<egui::ColorImage>>,
    tex: Mutex<Option<egui::TextureHandle>>,
    running: AtomicBool,
    failed: AtomicBool,
}

static PLAYING: AtomicUsize = AtomicUsize::new(0);

fn clip_for(poster: &str) -> Arc<Clip> {
    static CLIPS: OnceLock<Mutex<HashMap<String, Arc<Clip>>>> = OnceLock::new();
    CLIPS.get_or_init(Default::default).lock().unwrap().entry(poster.to_string()).or_default().clone()
}

/// Draws a description clip at `size`: the poster until frames arrive, then the looping video.
pub fn show(ui: &mut egui::Ui, poster_url: &str, poster: egui::Image, size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let c = clip_for(poster_url);
    if ui.is_rect_visible(rect) {
        *c.seen.lock().unwrap() = Some(Instant::now());
        if !c.failed.load(Relaxed) && !c.running.load(Relaxed) && PLAYING.load(Relaxed) < MAX_PLAYING {
            start(c.clone(), poster_url.to_string(), ui.ctx().clone());
        }
    }
    if let Some(img) = c.frame.lock().unwrap().take() {
        let mut t = c.tex.lock().unwrap();
        match t.as_mut() {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => *t = Some(ui.ctx().load_texture(format!("clip {poster_url}"), img, egui::TextureOptions::LINEAR)),
        }
    }
    match c.tex.lock().unwrap().as_ref() {
        Some(t) => egui::Image::from_texture(egui::load::SizedTexture::from_handle(t)).corner_radius(6).paint_at(ui, rect),
        None => poster.paint_at(ui, rect),
    }
}

fn start(c: Arc<Clip>, poster: String, ctx: egui::Context) {
    c.running.store(true, Relaxed);
    PLAYING.fetch_add(1, Relaxed);
    std::thread::spawn(move || {
        if let Err(e) = play(&c, &poster, &ctx) {
            log::warn!("Description clip for {poster} can't play: {e:#}");
            c.failed.store(true, Relaxed);
        }
        PLAYING.fetch_sub(1, Relaxed);
        c.running.store(false, Relaxed);
        ctx.request_repaint();
    });
}

/// Downloads (cached) and plays the clip in a loop, paced by frame timestamps, until it's off screen.
fn play(c: &Clip, poster: &str, ctx: &egui::Context) -> Result<()> {
    let idle = || c.seen.lock().unwrap().is_none_or(|t| t.elapsed() > IDLE);
    let mut errors = Vec::new();
    for url in clip_urls(poster) {
        let mime = if url.split('?').next().is_some_and(|p| p.ends_with(".webm")) { "video/webm" } else { "video/mp4" };
        // Streamed into memory only while the clip plays; nothing is kept on disk.
        let bytes = match gamut_core::http::get_bytes(&url) {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{e:#}"));
                continue;
            }
        };
        let mut r = match Reader::open(&bytes, mime) {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("{e:#}"));
                continue;
            }
        };
        loop {
            let t0 = Instant::now();
            let mut frames = 0;
            while let Some((ts, px)) = r.next()? {
                if idle() {
                    return Ok(());
                }
                let due = t0 + Duration::from_secs_f64(ts.max(0.0));
                if let Some(wait) = due.checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                *c.frame.lock().unwrap() = Some(egui::ColorImage::from_rgba_unmultiplied([r.w as usize, r.h as usize], &px));
                ctx.request_repaint();
                frames += 1;
            }
            anyhow::ensure!(frames > 0, "no frames decoded");
            r = Reader::open(&bytes, mime)?;
        }
    }
    anyhow::bail!("{}", errors.join("; "))
}

/// Older builds cached clips in %TEMP%\gamut-clips; clips are streamed now, so drop that folder.
pub fn remove_old_cache() {
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("gamut-clips"));
}

#[cfg(test)]
mod tests {
    #[test]
    fn clip_urls_from_poster() {
        assert_eq!(super::clip_urls("https://x/a/1.poster.avif?t=5"), ["https://x/a/1.webm?t=5", "https://x/a/1.mp4?t=5"]);
        assert!(super::clip_urls("https://x/a/1.avif").is_empty());
    }

    /// `GAMUT_VIDEO=<file> cargo test -p gamut -- --ignored video` (writes <file>.png of frame 30)
    #[test]
    #[ignore]
    fn reads_frames() {
        let path = std::path::PathBuf::from(std::env::var("GAMUT_VIDEO").unwrap());
        let mime = if path.extension().is_some_and(|e| e == "webm") { "video/webm" } else { "video/mp4" };
        let mut r = super::Reader::open(&std::fs::read(&path).unwrap(), mime).unwrap();
        let (mut n, mut last) = (0, 0.0);
        let t = std::time::Instant::now();
        while let Some((ts, px)) = r.next().unwrap() {
            n += 1;
            last = ts;
            if n == 30 {
                image::RgbaImage::from_raw(r.w, r.h, px).unwrap().save(path.with_extension("png")).unwrap();
            }
        }
        println!("{}x{}: {n} frames, {last:.2}s, decoded in {:?}", r.w, r.h, t.elapsed());
        assert!(n > 0);
    }
}
