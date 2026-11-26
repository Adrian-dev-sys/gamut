//! Frame trace for chasing visual glitches: keeps the last few seconds of frames in memory and
//! writes them to the log when a popup that should be on screen wasn't drawn, or on F12.
use egui::{Context, Id, LayerId, Order};
use std::collections::VecDeque;

/// Popups by the id their modal is created with.
const POPUPS: [&str; 6] = ["details_modal", "editor", "Settings", "Remove game", "Import from Playnite", "art_picker"];
const KEEP: usize = 240;

#[derive(Default)]
pub struct FrameTrace {
    frames: VecDeque<String>,
    nr: u64,
    last_time: f64,
    /// Popups the app meant to show last frame.
    expected: Vec<&'static str>,
    /// ...and the frame before that: a popup's first frame is always invisible (egui sizing pass).
    older: Vec<&'static str>,
    /// Frames left to record after a dump, so the log shows what followed.
    tail: usize,
}

impl FrameTrace {
    /// Call at the start of a frame: checks last frame's popups against what egui drew.
    pub fn begin(&mut self, ctx: &Context) {
        self.nr += 1;
        let (now, events, rect, focused) = ctx.input(|i| {
            let ev: Vec<String> = i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::PointerButton { pos, button, pressed, .. } => {
                        Some(format!("{}{:?}@{:.0},{:.0}", if *pressed { "down:" } else { "up:" }, button, pos.x, pos.y))
                    }
                    egui::Event::Key { key, pressed: true, .. } => Some(format!("key:{key:?}")),
                    egui::Event::WindowFocused(f) => Some(format!("focus:{f}")),
                    _ => None,
                })
                .collect();
            (i.time, ev, i.content_rect(), i.focused)
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
        let causes: Vec<String> = ctx.repaint_causes().iter().take(3).map(|c| c.to_string()).collect();
        let line = format!(
            "#{} +{dt:.0}ms want={:?} drawn={drawn:?} size={:.0}x{:.0} focused={focused} events={events:?} repaint={causes:?}",
            self.nr - 1,
            self.expected,
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
        if !missing.is_empty() {
            self.dump(&format!("popup {missing:?} was expected but not drawn in frame #{}", self.nr - 1));
        } else if ctx.input(|i| i.key_pressed(egui::Key::F12)) {
            self.dump("F12 pressed (user saw a glitch)");
        }
    }

    /// Call at the end of a frame with the popups the app is showing.
    pub fn end(&mut self, expected: Vec<&'static str>) {
        self.older = std::mem::replace(&mut self.expected, expected);
    }

    fn dump(&mut self, why: &str) {
        log::warn!(target: "gamut::frames", "frame trace: {why}; last {} frames:", self.frames.len());
        for f in self.frames.drain(..) {
            log::info!(target: "gamut::frames", "{f}");
        }
        self.tail = 30;
    }
}
