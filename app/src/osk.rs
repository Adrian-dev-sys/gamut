//! On-screen keyboard for the controller: types into whichever text field has focus (see `raw_input_hook`).
use crate::gamepad::Pad;
use crate::theme::{ACCENT, CARD, CARD_HI, PANEL, TEXT, TEXT_DIM};
use crate::ui::Nav;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Stroke, StrokeKind};

pub const ROWS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL'", "ZXCVBNM-:."];
/// Bottom row: label and the columns (of 10) it spans.
const SPECIAL: [(&str, usize, usize); 5] = [("Shift", 0, 2), ("Space", 2, 5), ("Delete", 5, 7), ("Clear", 7, 8), ("Done", 8, 10)];

pub struct Osk {
    row: usize,
    col: usize,
    shift: bool,
}

/// What a controller press does to the text field.
pub enum Out {
    None,
    Text(String),
    Backspace,
    Clear,
    Close,
}

impl Osk {
    pub fn new() -> Self {
        Osk { row: 1, col: 0, shift: false }
    }

    pub fn pad(&mut self, p: Pad) -> Out {
        let n = ROWS.len() + 1;
        match p {
            Pad::Nav(Nav::Left) => self.col = (self.col + 9) % 10,
            Pad::Nav(Nav::Right) => self.col = (self.col + 1) % 10,
            Pad::Nav(Nav::Up) => self.row = (self.row + n - 1) % n,
            Pad::Nav(Nav::Down) => self.row = (self.row + 1) % n,
            Pad::Confirm => return self.press(),
            Pad::Favorite => return Out::Backspace,
            Pad::Search => return Out::Text(" ".into()),
            Pad::PrevTab | Pad::NextTab => self.shift = !self.shift,
            Pad::Back | Pad::Menu => return Out::Close,
            _ => {}
        }
        Out::None
    }

    fn press(&mut self) -> Out {
        if self.row < ROWS.len() {
            let c = ROWS[self.row].chars().nth(self.col).unwrap_or(' ');
            let s = if self.shift { c.to_string() } else { c.to_lowercase().collect() };
            return Out::Text(s);
        }
        match SPECIAL.iter().position(|s| (s.1..s.2).contains(&self.col)) {
            Some(0) => {
                self.shift = !self.shift;
                Out::None
            }
            Some(1) => Out::Text(" ".into()),
            Some(2) => Out::Backspace,
            Some(3) => Out::Clear,
            _ => Out::Close,
        }
    }

    /// Drawn above everything, without taking focus from the text field.
    pub fn show(&self, ctx: &egui::Context) {
        let (key_w, key_h, gap) = (52.0, 44.0, 6.0);
        let t = crate::theme::appear(ctx, egui::Id::unique("osk"), 0.14);
        egui::Area::new(egui::Id::unique("osk"))
            .order(egui::Order::Tooltip)
            .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -24.0 + 20.0 * (1.0 - t)))
            .interactable(false)
            .show(ctx, |ui| {
                ui.multiply_opacity(t);
                egui::Frame::new()
                    .fill(PANEL())
                    .stroke(Stroke::new(1.0, crate::theme::BORDER()))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(14))
                    .shadow(ctx.global_style().visuals.window_shadow)
                    .show(ui, |ui| {
                        let w = 10.0 * key_w + 9.0 * gap;
                        let (rect, _) = ui.allocate_exact_size(vec2(w, 5.0 * key_h + 4.0 * gap + 26.0), egui::Sense::hover());
                        let p = ui.painter();
                        let key = |r: Rect, label: &str, on: bool, lit: bool| {
                            let fill = if on { ACCENT().gamma_multiply(0.45) } else if lit { ACCENT().gamma_multiply(0.2) } else { CARD_HI() };
                            p.rect_filled(r, CornerRadius::same(6), fill);
                            let s = if on { Stroke::new(2.5, Color32::WHITE) } else { Stroke::new(1.0, CARD()) };
                            p.rect_stroke(r, CornerRadius::same(6), s, StrokeKind::Inside);
                            p.text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(18.0), TEXT());
                        };
                        let at = |row: usize, from: usize, to: usize| {
                            let min = rect.min + vec2(from as f32 * (key_w + gap), row as f32 * (key_h + gap));
                            Rect::from_min_size(min, vec2((to - from) as f32 * (key_w + gap) - gap, key_h))
                        };
                        for (r, row) in ROWS.iter().enumerate() {
                            for (c, ch) in row.chars().enumerate() {
                                let label = if self.shift { ch.to_string() } else { ch.to_lowercase().collect() };
                                key(at(r, c, c + 1), &label, self.row == r && self.col == c, false);
                            }
                        }
                        for (label, from, to) in SPECIAL {
                            let on = self.row == ROWS.len() && (from..to).contains(&self.col);
                            key(at(ROWS.len(), from, to), label, on, label == "Shift" && self.shift);
                        }
                        p.text(
                            pos2(rect.center().x, rect.bottom() - 9.0),
                            Align2::CENTER_CENTER,
                            "A type · X delete · Y space · LB/RB shift · B done",
                            FontId::proportional(13.5),
                            TEXT_DIM(),
                        );
                    });
            });
    }
}
