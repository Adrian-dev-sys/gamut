//! Grid cover textures: decoded off-thread, only for what's on screen, with a bounded cache.
use crossbeam_channel::{unbounded, Receiver};
use egui::{Color32, ColorImage, Context, TextureHandle, TextureOptions};
use gamut_core::{images, Paths};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

/// Max textures kept on the GPU (~540 KB each).
const CAP: usize = 320;

#[derive(Default)]
struct Queue {
    wanted: Vec<i64>,
    in_flight: HashSet<i64>,
}

pub struct ThumbCache {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    rx: Receiver<(i64, Option<(ColorImage, u32)>)>,
    tex: HashMap<i64, (TextureHandle, u64)>,
    /// Each cover's main color (for the grid glow); kept after the texture is evicted.
    colors: HashMap<i64, Color32>,
    missing: HashSet<i64>,
    wanted: Vec<i64>,
    frame: u64,
    /// Decode width in pixels; small lists set their exact on-screen size so the GPU never shrinks them.
    width: Arc<AtomicU32>,
}

impl ThumbCache {
    pub fn new(ctx: &Context, paths: Paths, threads: usize) -> Self {
        let width = Arc::new(AtomicU32::new(images::THUMB_W));
        let queue: Arc<(Mutex<Queue>, Condvar)> = Default::default();
        let (tx, rx) = unbounded();
        for _ in 0..threads {
            let (queue, tx, ctx, paths, width) = (queue.clone(), tx.clone(), ctx.clone(), paths.clone(), width.clone());
            std::thread::spawn(move || loop {
                let id = {
                    let (m, cv) = &*queue;
                    let mut q = m.lock().unwrap();
                    while q.wanted.is_empty() {
                        q = cv.wait(q).unwrap();
                    }
                    let id = q.wanted.remove(0);
                    q.in_flight.insert(id);
                    id
                };
                let w = width.load(Ordering::Relaxed);
                let img = load(&paths, id, w);
                queue.0.lock().unwrap().in_flight.remove(&id);
                if tx.send((id, img.map(|i| (i, w)))).is_err() {
                    break;
                }
                ctx.request_repaint();
            });
        }
        ThumbCache { queue, rx, colors: HashMap::new(), tex: HashMap::new(), missing: HashSet::new(), wanted: Vec::new(), frame: 0, width }
    }

    pub fn begin_frame(&mut self, ctx: &Context) {
        self.frame += 1;
        self.wanted.clear();
        for (id, img) in self.rx.try_iter() {
            match img {
                Some((_, w)) if w != self.width.load(Ordering::Relaxed) => {} // decoded before a size change
                Some((img, _)) => {
                    self.colors.insert(id, main_color(&img));
                    let t = ctx.load_texture(format!("thumb{id}"), img, TextureOptions::LINEAR);
                    self.tex.insert(id, (t, self.frame));
                }
                None => {
                    self.missing.insert(id);
                }
            }
        }
    }

    /// Texture for a game's cover, queuing a load if needed.
    pub fn get(&mut self, id: i64, has_cover: bool) -> Option<&TextureHandle> {
        if !has_cover || self.missing.contains(&id) {
            return None;
        }
        match self.tex.get_mut(&id) {
            Some((t, used)) => {
                *used = self.frame;
                Some(t)
            }
            None => {
                self.wanted.push(id);
                None
            }
        }
    }

    /// Hands this frame's visible misses to the loaders (dropping stale requests) and evicts.
    pub fn end_frame(&mut self) {
        {
            let (m, cv) = &*self.queue;
            let mut q = m.lock().unwrap();
            let in_flight = std::mem::take(&mut q.in_flight);
            q.wanted = self.wanted.iter().copied().filter(|id| !in_flight.contains(id)).collect();
            q.in_flight = in_flight;
            if !q.wanted.is_empty() {
                cv.notify_all();
            }
        }
        if self.tex.len() > CAP {
            let mut by_age: Vec<(u64, i64)> = self.tex.iter().map(|(id, (_, f))| (*f, *id)).collect();
            by_age.sort_unstable();
            for (f, id) in by_age.into_iter().take(self.tex.len() - CAP) {
                if f + 1 < self.frame {
                    self.tex.remove(&id);
                }
            }
        }
    }

    /// Drops every cached texture (after bulk cover changes).
    pub fn clear(&mut self) {
        self.tex.clear();
        self.missing.clear();
    }

    /// Sets the decode width (pixels), dropping textures made at another width.
    pub fn set_width(&mut self, w: u32) {
        if self.width.swap(w.clamp(16, images::THUMB_W), Ordering::Relaxed) != w.clamp(16, images::THUMB_W) {
            self.clear();
        }
    }

    /// The cover's main color, once its thumbnail has loaded.
    pub fn color(&self, id: i64) -> Option<Color32> {
        self.colors.get(&id).copied()
    }

    pub fn invalidate(&mut self, id: i64) {
        self.colors.remove(&id);
        self.tex.remove(&id);
        self.missing.remove(&id);
    }
}

/// Average of the cover's colorful pixels, made bright and saturated enough to glow.
fn main_color(img: &ColorImage) -> Color32 {
    let (mut sum, mut w) = ([0f32; 3], 0f32);
    for p in img.pixels.iter().step_by(5) {
        let [r, g, b, _] = p.to_array();
        let (mx, mn) = (r.max(g).max(b) as f32, r.min(g).min(b) as f32);
        let k = ((mx - mn) / 255.0).powi(2) * (mx / 255.0) + 0.002;
        for (s, c) in sum.iter_mut().zip([r, g, b]) {
            *s += c as f32 * k;
        }
        w += k;
    }
    let avg = sum.map(|s| (s / w).round() as u8);
    let mut hsv = egui::ecolor::Hsva::from(Color32::from_rgb(avg[0], avg[1], avg[2]));
    if hsv.s < 0.12 {
        return crate::theme::ACCENT; // grey cover: no color of its own
    }
    (hsv.s, hsv.v) = (hsv.s.clamp(0.45, 0.9), hsv.v.max(0.85));
    Color32::from(hsv)
}

fn load(paths: &Paths, id: i64, w: u32) -> Option<ColorImage> {
    let thumb = paths.thumb(id);
    if !thumb.exists() {
        // Rebuild a deleted thumbnail from the stored cover.
        let bytes = std::fs::read(paths.cover(id)).ok()?;
        images::store_cover(&bytes, &paths.cover(id), &thumb).ok()?;
    }
    let img = images::decode(&thumb, w).ok()?;
    Some(ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels))
}

/// One-off decode for large images (detail panel banner).
pub fn load_large(path: &std::path::Path, max_w: u32) -> Option<ColorImage> {
    let img = images::decode(path, max_w).ok()?;
    Some(ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels))
}
