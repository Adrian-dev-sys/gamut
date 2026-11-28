//! Game page "Sessions" tab: one game's play sessions with a short summary.
use crate::theme::{self, CARD, TEXT, TEXT_DIM};
use crate::util;
use egui::{vec2, CornerRadius, RichText, Sense, Ui};
use gamut_core::Library;

const SHOWN: usize = 25;
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

pub struct GameSessions {
    pub id: i64,
    /// (start, minutes, kind), newest first; kind 1 = picked up from Steam's total.
    list: Vec<(i64, i64, i64)>,
    since: Option<i64>,
    show_all: bool,
}

impl GameSessions {
    pub fn load(lib: &Library, id: i64) -> Self {
        let list = lib.game_sessions(id).unwrap_or_else(|e| {
            log::warn!("sessions for game {id}: {e:#}");
            Vec::new()
        });
        Self { id, list, since: lib.stats_since().ok().flatten(), show_all: false }
    }

    pub fn count(&self) -> usize {
        self.list.len()
    }
}

pub fn section(ui: &mut Ui, s: &mut GameSessions) {
    let off = gamut_core::utc_offset();
    if s.list.is_empty() {
        theme::section(ui, "Play sessions");
        let from = s.since.map(|t| format!(" since {}", util::date(t + off))).unwrap_or_default();
        ui.label(RichText::new(format!(
            "No sessions recorded{from}. Games Gamut starts are timed; Steam play is picked up from library scans."
        ))
        .color(TEXT_DIM()));
        return;
    }
    let total: i64 = s.list.iter().map(|x| x.1).sum();
    let longest = s.list.iter().max_by_key(|x| x.1).unwrap();
    let month_ago = gamut_core::now() - 30 * 86_400;
    let recent: i64 = s.list.iter().filter(|x| x.0 >= month_ago).map(|x| x.1).sum();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        tile(ui, "SESSIONS", &s.list.len().to_string());
        tile(ui, "RECORDED", &util::playtime(total));
        tile(ui, "AVERAGE", &util::playtime((total + s.list.len() as i64 / 2) / s.list.len() as i64));
        tile(ui, "LONGEST", &util::playtime(longest.1));
        tile(ui, "LAST 30 DAYS", &if recent > 0 { util::playtime(recent) } else { "0m".into() });
    });
    ui.add_space(16.0);
    theme::section(ui, "Play sessions");
    let n = if s.show_all { s.list.len() } else { s.list.len().min(SHOWN) };
    for &(start, mins, kind) in &s.list[..n] {
        row(ui, start + off, mins, kind == 1);
    }
    if s.list.len() > SHOWN {
        ui.add_space(6.0);
        let label = if s.show_all { "Show fewer".to_string() } else { format!("Show all {}", s.list.len()) };
        if ui.button(label).clicked() {
            s.show_all = !s.show_all;
        }
    }
}

/// `local` is the start in local seconds.
fn row(ui: &mut Ui, local: i64, mins: i64, steam: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, CARD());
    }
    let p = ui.painter();
    let y = rect.center().y;
    let font = |s: f32| egui::FontId::proportional(s);
    let weekday = DAYS[(local.div_euclid(86_400) + 3).rem_euclid(7) as usize];
    p.text(egui::pos2(rect.left() + 10.0, y), egui::Align2::LEFT_CENTER, format!("{weekday}  {}", util::date(local)), font(14.0), TEXT());
    let hm = |t: i64| format!("{:02}:{:02}", t.rem_euclid(86_400) / 3600, t.rem_euclid(3600) / 60);
    let time = if steam { format!("~{}", hm(local)) } else { format!("{} – {}", hm(local), hm(local + mins * 60)) };
    p.text(egui::pos2(rect.left() + 190.0, y), egui::Align2::LEFT_CENTER, time, font(13.0), TEXT_DIM());
    let src = if steam { "Steam" } else { "Gamut" };
    p.text(egui::pos2(rect.left() + 340.0, y), egui::Align2::LEFT_CENTER, src, font(12.5), TEXT_DIM());
    let dur = if mins > 0 { util::playtime(mins) } else { "<1m".into() };
    p.text(egui::pos2(rect.right() - 10.0, y), egui::Align2::RIGHT_CENTER, dur, font(14.0), TEXT());
    if steam {
        resp.on_hover_text("Picked up from Steam's playtime total after the game closed; the start time is approximate.");
    }
}

fn tile(ui: &mut Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(CARD())
        .stroke(egui::Stroke::new(1.0, theme::BORDER()))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(theme::caption(label));
                ui.label(RichText::new(value).size(17.0).strong());
            });
        });
}
