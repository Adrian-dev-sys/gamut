//! Toast notifications in the bottom-right corner; every message is also logged.
use crate::theme::{self, CARD, TEXT};
use egui::{vec2, Color32, CornerRadius, RichText, Stroke};

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Info,
    Success,
    Error,
}

struct Toast {
    level: Level,
    text: String,
    /// Seconds left; errors stay until dismissed.
    ttl: Option<f32>,
}

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    last: f64,
}

impl Toasts {
    /// Starts with an optional error already showing (e.g. a database repair notice).
    pub fn with(first: Option<String>) -> Self {
        let mut t = Self::default();
        if let Some(m) = first {
            t.error(m);
        }
        t
    }

    pub fn push(&mut self, level: Level, text: impl Into<String>) {
        let text = text.into();
        match level {
            Level::Error => log::error!("{text}"),
            _ => log::info!("{text}"),
        }
        // Same message again (e.g. a repeating failure): don't stack copies.
        self.items.retain(|t| t.text != text);
        let ttl = (level != Level::Error).then_some(5.0);
        self.items.push(Toast { level, text, ttl });
        if self.items.len() > 5 {
            self.items.remove(0);
        }
    }
    pub fn info(&mut self, t: impl Into<String>) {
        self.push(Level::Info, t)
    }
    pub fn success(&mut self, t: impl Into<String>) {
        self.push(Level::Success, t)
    }
    pub fn error(&mut self, t: impl Into<String>) {
        self.push(Level::Error, t)
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if self.items.is_empty() {
            self.last = 0.0;
            return;
        }
        // Real elapsed time: idle frames are far apart, so egui's predicted dt would undercount.
        let now = ctx.input(|i| i.time);
        let dt = if self.last > 0.0 { (now - self.last).min(1.0) as f32 } else { 0.0 };
        self.last = now;
        let mut dismiss = None;
        let mut hovered = false;
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0))
            .order(egui::Order::Tooltip)
            .show(ctx, |ui| {
                for (i, t) in self.items.iter().enumerate().rev() {
                    let color = match t.level {
                        Level::Info => theme::ACCENT,
                        Level::Success => theme::GOOD,
                        Level::Error => Color32::from_rgb(230, 85, 85),
                    };
                    let r = egui::Frame::new()
                        .fill(CARD)
                        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(egui::Margin { left: 14, right: 10, top: 10, bottom: 10 })
                        .shadow(ctx.global_style().visuals.popup_shadow)
                        .show(ui, |ui| {
                            ui.set_width(360.0);
                            ui.horizontal(|ui| {
                                let label = match t.level {
                                    Level::Info => "Info",
                                    Level::Success => "Done",
                                    Level::Error => "Error",
                                };
                                ui.label(RichText::new(label).strong().color(color));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if theme::x_button(ui, 20.0).on_hover_text("Dismiss").clicked() {
                                        dismiss = Some(i);
                                    }
                                    if t.level == Level::Error && ui.small_button("Copy").clicked() {
                                        ui.ctx().copy_text(t.text.clone());
                                    }
                                });
                            });
                            ui.add(egui::Label::new(RichText::new(&t.text).color(TEXT)).wrap());
                        });
                    hovered |= r.response.contains_pointer();
                    ui.add_space(6.0);
                }
            });
        if let Some(i) = dismiss {
            self.items.remove(i);
        }
        // Hovering pauses the countdown so a message can be read.
        if !hovered {
            for t in &mut self.items {
                if let Some(ttl) = &mut t.ttl {
                    *ttl -= dt;
                }
            }
            self.items.retain(|t| t.ttl.is_none_or(|s| s > 0.0));
        }
        if self.items.iter().any(|t| t.ttl.is_some()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
