//! Big screen search: a centered popup with an on-screen keyboard driven by the controller.
use crate::gamepad::Pad;
use crate::theme::{self, ACCENT, CARD, CARD_HI, TEXT_DIM};
use crate::ui::Nav;
use egui::{vec2, Color32, CornerRadius, RichText, Stroke};

const ROWS: [&str; 4] = ["1234567890", "QWERTYUIOP", "ASDFGHJKL'", "ZXCVBNM-:."];
/// Bottom row: label and the columns (of 10) it spans.
const SPECIAL: [(&str, usize, usize); 4] = [("Space", 0, 3), ("Delete", 3, 6), ("Clear", 6, 8), ("Done", 8, 10)];

#[derive(Default)]
pub struct SearchPop {
    row: usize,
    col: usize,
    focus_field: bool,
}

enum Key {
    Char(char),
    Space,
    Delete,
    Clear,
    Done,
}

impl SearchPop {
    pub fn new() -> Self {
        SearchPop { row: 1, col: 0, focus_field: true }
    }

    fn key_at(&self, row: usize, col: usize) -> Key {
        if row < ROWS.len() {
            return Key::Char(ROWS[row].chars().nth(col).unwrap_or(' '));
        }
        match SPECIAL.iter().position(|s| col >= s.1 && col < s.2) {
            Some(0) => Key::Space,
            Some(1) => Key::Delete,
            Some(2) => Key::Clear,
            _ => Key::Done,
        }
    }

    /// Applies a key; true when the popup should close.
    fn press(k: Key, q: &mut String) -> bool {
        match k {
            Key::Char(c) => q.extend(c.to_lowercase()),
            Key::Space => q.push(' '),
            Key::Delete => {
                q.pop();
            }
            Key::Clear => q.clear(),
            Key::Done => return true,
        }
        false
    }

    /// Controller input; true when the popup should close.
    pub fn pad(&mut self, p: Pad, q: &mut String) -> bool {
        match p {
            Pad::Nav(Nav::Left) => self.col = (self.col + 9) % 10,
            Pad::Nav(Nav::Right) => self.col = (self.col + 1) % 10,
            Pad::Nav(Nav::Up) => self.row = (self.row + ROWS.len()) % (ROWS.len() + 1),
            Pad::Nav(Nav::Down) => self.row = (self.row + 1) % (ROWS.len() + 1),
            Pad::Confirm => return Self::press(self.key_at(self.row, self.col), q),
            Pad::Favorite => {
                q.pop();
            }
            Pad::Back | Pad::Search | Pad::Start => return true,
            _ => {}
        }
        false
    }

    /// Draws the popup; true when it should close (Done, Enter, Esc or a click outside).
    pub fn show(&mut self, ctx: &egui::Context, q: &mut String, results: usize) -> bool {
        let mut close = false;
        let resp = egui::Modal::new(egui::Id::unique("search_pop"))
            .area(theme::no_fade(egui::Id::unique("search_pop")))
            .frame(egui::Frame::new().fill(theme::PANEL).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(20)))
            .backdrop_color(Color32::TRANSPARENT)
            .show(ctx, |ui| {
                ui.set_width(640.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search").size(22.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{results} games")).color(TEXT_DIM).size(15.0));
                    });
                });
                ui.add_space(8.0);
                let field = ui.add(
                    egui::TextEdit::singleline(q).font(egui::FontId::proportional(20.0)).hint_text("Type a name").desired_width(f32::INFINITY),
                );
                if std::mem::take(&mut self.focus_field) {
                    field.request_focus();
                }
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    close = true;
                }
                ui.add_space(12.0);
                let key_w = (ui.available_width() - 9.0 * 6.0) / 10.0;
                let key = |ui: &mut egui::Ui, label: &str, w: f32, on: bool| -> bool {
                    let b = egui::Button::new(RichText::new(label).size(18.0))
                        .min_size(vec2(w, 46.0))
                        .fill(if on { ACCENT.gamma_multiply(0.45) } else { CARD_HI })
                        .stroke(if on { Stroke::new(2.5, Color32::WHITE) } else { Stroke::new(1.0, CARD) });
                    ui.add(b).clicked()
                };
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (r, row) in ROWS.iter().enumerate() {
                    ui.horizontal(|ui| {
                        for (c, ch) in row.chars().enumerate() {
                            if key(ui, &ch.to_string(), key_w, self.row == r && self.col == c) {
                                close |= Self::press(Key::Char(ch), q);
                            }
                        }
                    });
                }
                ui.horizontal(|ui| {
                    for (label, from, to) in SPECIAL {
                        let w = key_w * (to - from) as f32 + 6.0 * (to - from - 1) as f32;
                        let on = self.row == ROWS.len() && (from..to).contains(&self.col);
                        if key(ui, label, w, on) {
                            close |= Self::press(self.key_at(ROWS.len(), from), q);
                        }
                    }
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new("A type · X delete · B / Y close · Results update behind this popup").color(TEXT_DIM).size(14.0),
                );
            });
        close || resp.should_close()
    }
}
