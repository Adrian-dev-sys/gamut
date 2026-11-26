//! Frame trace for chasing visual glitches: keeps the last few seconds of frames in memory and
//! writes them to the log when a popup isn't drawn, the screen behind it brightens, or on F12.
use eframe::{egui_glow, glow};
use egui::{Context, Id, LayerId, Order};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Popups by the id their modal is created with.
const POPUPS: [&str; 6] = ["details_modal", "editor", "Settings", "Remove game", "Import from Playnite", "art_picker"];
/// About 12 s at 165 Hz.
const KEEP: usize = 2000;
/// Points sampled from the finished frame: window corner (top bar) and the left edge of the grid.
const SAMPLES: [(f32, f32); 2] = [(6.0, 6.0), (12.0, 0.5)];
const MAX_AUTO_DUMPS: u32 = 5;

#[derive(Default)]
pub struct FrameTrace {
    frames: VecDeque<String>,
    nr: u64,
    last_time: f64,
    /// Popups the app meant to show last frame.
    expected: Vec<&'static str>,
    /// ...and the frame before that: a popup's first frame is always invisible (egui sizing pass).
    older: Vec<&'static str>,
    backdrop: bool,
    /// Pixels read back from the GPU at the end of the last paint.
    pixels: Arc<Mutex<Vec<[u8; 4]>>>,
    /// Corner pixel of the previous frame, while a popup was steady.
    last_corner: Option<[u8; 4]>,
    /// Frames left to record after a dump, so the log shows what followed.
    tail: usize,
    auto_dumps: u32,
}

impl FrameTrace {
    /// Call at the start of a frame: checks last frame's popups against what egui drew.
    pub fn begin(&mut self, ctx: &Context) {
        self.nr += 1;
        let (now, events, moves, rect, focused) = ctx.input(|i| {
            let mut moves = 0;
            let ev: Vec<String> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        Some(format!("{}{:?}@{:.0},{:.0}", if *pressed { "down:" } else { "up:" }, button, pos.x, pos.y))
                    }
                    egui::Event::Key { key, pressed: true, .. } => Some(format!("key:{key:?}")),
                    egui::Event::WindowFocused(f) => Some(format!("focus:{f}")),
                    egui::Event::PointerMoved(_) | egui::Event::MouseMoved(_) => {
                        moves += 1;
                        None
                    }
                    _ => None,
                })
                .collect();
            (i.time, ev, moves, i.content_rect(), i.focused)
        });
        let dt = (now - self.last_time) * 1000.0;
        self.last_time = now;
        let drawn: Vec<&str> = POPUPS
            .iter()
            .copied()
            .filter(|p| {
                let layer = LayerId::new(Order::Foreground, Id::unique(*p));
                ctx.memory(|m| m.areas().visible_last_frame(&layer))
            })
            .collect();
        let missing: Vec<&str> = self.expected.iter().copied().filter(|p| !drawn.contains(p) && self.older.contains(p)).collect();
        let causes: Vec<String> = ctx
            .repaint_causes()
            .iter()
            .take(3)
            .map(|c| {
                let s = c.to_string();
                s.rsplit(['\\', '/']).next().unwrap_or(&s).trim().to_string()
            })
            .collect();
        let px = self.pixels.lock().map(|p| p.clone()).unwrap_or_default();
        let px_txt: Vec<String> = px.iter().map(|p| format!("{:02x}{:02x}{:02x}", p[0], p[1], p[2])).collect();
        let line = format!(
            "#{} +{dt:.0}ms want={:?} drawn={drawn:?} backdrop={} px={px_txt:?} size={:.0}x{:.0} focused={focused} moves={moves} events={events:?} repaint={causes:?}",
            self.nr - 1,
            self.expected,
            self.backdrop,
            rect.width(),
            rect.height(),
        );
        if self.tail > 0 {
            log::info!(target: "gamut::frames", "{line}");
            self.tail -= 1;
        }
        self.frames.push_back(line);
        if self.frames.len() > KEEP {
            self.frames.pop_front();
        }

        // A popup that stayed open for 3 frames should leave the dimmed corner unchanged.
        let steady = !self.expected.is_empty() && self.expected == self.older;
        let corner = px.first().copied();
        let mut brightened = None;
        if let (true, Some(c)) = (steady, corner) {
            if let Some(prev) = self.last_corner {
                let lum = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
                if lum(c) > lum(prev) + 24 {
                    brightened = Some(format!("{prev:02x?} -> {c:02x?}"));
                }
            }
            self.last_corner = Some(c);
        } else {
            self.last_corner = None;
        }

        if !missing.is_empty() {
            self.auto_dump(&format!("popup {missing:?} was expected but not drawn in frame #{}", self.nr - 1));
        } else if let Some(b) = brightened {
            self.auto_dump(&format!("screen behind popup brightened in frame #{}: {b}", self.nr - 1));
        } else if ctx.input(|i| i.key_pressed(egui::Key::F12)) {
            self.dump("F12 pressed (user saw a glitch)");
        }
    }

    /// Call after the app has queued its popups: records what should show and samples the screen.
    pub fn end(&mut self, ctx: &Context, expected: Vec<&'static str>, backdrop: bool) {
        self.older = std::mem::replace(&mut self.expected, expected);
        self.backdrop = backdrop;
        let screen = ctx.content_rect();
        let ppp = ctx.pixels_per_point();
        let out = self.pixels.clone();
        // Topmost layer, so the readback sees everything else already drawn this frame.
        let cb = egui_glow::CallbackFn::new(move |info, painter| {
            let gl = painter.gl();
            let h = info.screen_size_px[1] as i32;
            let mut got = Vec::with_capacity(SAMPLES.len());
            for (x, y) in SAMPLES {
                let y = if y <= 1.0 { screen.height() * y } else { y };
                let (px, py) = ((x * ppp) as i32, (y * ppp) as i32);
                let mut buf = [0u8; 4];
                unsafe {
                    use glow::HasContext as _;
                    gl.read_pixels(px, h - py - 1, 1, 1, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
                }
                got.push(buf);
            }
            if let Ok(mut p) = out.lock() {
                *p = got;
            }
        });
        let layer = LayerId::new(Order::Debug, Id::unique("gamut_probe"));
        ctx.layer_painter(layer).add(egui::PaintCallback { rect: screen, callback: Arc::new(cb) });
    }

    fn auto_dump(&mut self, why: &str) {
        if self.tail > 0 || self.auto_dumps >= MAX_AUTO_DUMPS {
            return;
        }
        self.auto_dumps += 1;
        self.dump(why);
    }

    fn dump(&mut self, why: &str) {
        log::warn!(target: "gamut::frames", "frame trace: {why}; last {} frames:", self.frames.len());
        for f in self.frames.drain(..) {
            log::info!(target: "gamut::frames", "{f}");
        }
        self.tail = 30;
    }
}
