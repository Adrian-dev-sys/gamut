//! Grid cover textures: decoded off-thread, only for what's on screen, with a bounded cache.
use crossbeam_channel::{unbounded, Receiver};
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use gamut_core::{images, Paths};
use std::collections::{HashMap, HashSet};
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
    rx: Receiver<(i64, Option<ColorImage>)>,
    tex: HashMap<i64, (TextureHandle, u64)>,
    missing: HashSet<i64>,
    wanted: Vec<i64>,
    frame: u64,
}

impl ThumbCache {
    pub fn new(ctx: &Context, paths: Paths, threads: usize) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Default::default();
        let (tx, rx) = unbounded();
        for _ in 0..threads {
            let (queue, tx, ctx, paths) = (queue.clone(), tx.clone(), ctx.clone(), paths.clone());
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
                let img = load(&paths, id);
                queue.0.lock().unwrap().in_flight.remove(&id);
                if tx.send((id, img)).is_err() {
                    break;
                }
                ctx.request_repaint();
            });
        }
        ThumbCache { queue, rx, tex: HashMap::new(), missing: HashSet::new(), wanted: Vec::new(), frame: 0 }
    }

    pub fn begin_frame(&mut self, ctx: &Context) {
        self.frame += 1;
        self.wanted.clear();
        for (id, img) in self.rx.try_iter() {
            match img {
                Some(img) => {
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

    pub fn invalidate(&mut self, id: i64) {
        self.tex.remove(&id);
        self.missing.remove(&id);
    }
}

fn load(paths: &Paths, id: i64) -> Option<ColorImage> {
    let thumb = paths.thumb(id);
    if !thumb.exists() {
        // Rebuild a deleted thumbnail from the stored cover.
        let bytes = std::fs::read(paths.cover(id)).ok()?;
        images::store_cover(&bytes, &paths.cover(id), &thumb).ok()?;
    }
    let img = images::decode(&thumb, images::THUMB_W).ok()?;
    Some(ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels))
}

/// One-off decode for large images (detail panel banner).
pub fn load_large(path: &std::path::Path, max_w: u32) -> Option<ColorImage> {
    let img = images::decode(path, max_w).ok()?;
    Some(ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.pixels))
}
