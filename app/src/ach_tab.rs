//! The Achievements tab: overall progress across games, read from the cache.
use crate::theme::{self, ACCENT, CARD, TEXT, TEXT_DIM};
use egui::{vec2, Color32, CornerRadius, RichText, Sense, Ui};
use gamut_core::achievements::Achievement;
use gamut_core::{Game, Library};
use std::collections::HashMap;

const GOLD: Color32 = Color32::from_rgb(232, 186, 82);
const SORTS: [&str; 3] = ["Completion", "Recent unlock", "Name"];

/// A game with achievements: (game id, total, unlocked, newest unlock, points earned).
type Row = (i64, i64, i64, i64, i64);

pub struct Overview {
    games: Vec<Row>,
    recent: Vec<(i64, Achievement)>,
    rarest: Vec<(i64, Achievement)>,
    sort: usize,
    show_unstarted: bool,
    /// When the numbers were read (egui time), and whether newer ones exist.
    pub loaded: f64,
    pub dirty: bool,
}

impl Overview {
    pub fn load(lib: &Library, now: f64) -> Self {
        Overview {
            games: lib.achievement_progress().unwrap_or_default(),
            recent: lib.unlocked_achievements(false, 12).unwrap_or_default(),
            rarest: lib.unlocked_achievements(true, 12).unwrap_or_default(),
            sort: 0,
            show_unstarted: false,
            loaded: now,
            dirty: false,
        }
    }

    /// Reloads the numbers, keeping the view settings.
    pub fn refresh(&mut self, lib: &Library, now: f64) {
        let (sort, show_unstarted) = (self.sort, self.show_unstarted);
        *self = Overview { sort, show_unstarted, ..Overview::load(lib, now) };
    }
}

fn pct(r: &Row) -> f32 {
    r.2 as f32 / r.1.max(1) as f32
}

/// Draws the tab; returns a game the user clicked.
pub fn show(ui: &mut Ui, ov: &mut Overview, games: &[Game], thumbs: &mut crate::thumbs::ThumbCache, busy: bool) -> Option<i64> {
    let by_id: HashMap<i64, &Game> = games.iter().filter(|g| !g.hidden).map(|g| (g.id, g)).collect();
    let rows: Vec<Row> = ov.games.iter().filter(|r| by_id.contains_key(&r.0)).copied().collect();
    if rows.is_empty() {
        ui.add_space(60.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("No achievements yet").size(18.0).color(TEXT));
            ui.add_space(6.0);
            ui.label(
                RichText::new("Add a Steam Web API key or a RetroAchievements login in Settings. Played games are checked in the background.")
                    .color(TEXT_DIM),
            );
            if busy {
                ui.add_space(10.0);
                ui.add(egui::Spinner::new());
            }
        });
        return None;
    }
    let started = rows.iter().filter(|r| r.2 > 0).count();
    let unlocked: i64 = rows.iter().map(|r| r.2).sum();
    let perfect = rows.iter().filter(|r| r.2 == r.1).count();
    let avg = rows.iter().filter(|r| r.2 > 0).map(pct).sum::<f32>() / started.max(1) as f32;
    let points: i64 = rows.iter().map(|r| r.4).sum();
    let mut open = None;

    egui::ScrollArea::vertical().id_salt("ach_tab").auto_shrink([false, false]).show(ui, |ui| {
        page(ui, |ui| body(ui, ov, &rows, &by_id, thumbs, busy, (unlocked, perfect, avg, started, points), &mut open));
    });
    open
}

/// Centers `add` in a column at most 1200 px wide, so wide windows don't stretch rows apart.
pub fn page(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let full = ui.available_width();
    let w = full.min(1200.0) - 16.0;
    ui.horizontal_top(|ui| {
        ui.add_space(((full - w) / 2.0).max(0.0));
        ui.allocate_ui_with_layout(vec2(w, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(w);
            add(ui);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn body(
    ui: &mut Ui,
    ov: &mut Overview,
    rows: &[Row],
    by_id: &HashMap<i64, &Game>,
    thumbs: &mut crate::thumbs::ThumbCache,
    busy: bool,
    (unlocked, perfect, avg, started, points): (i64, usize, f32, usize, i64),
    open: &mut Option<i64>,
) {
    {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            tile(ui, "UNLOCKED", &unlocked.to_string());
            tile(ui, "PERFECT GAMES", &perfect.to_string());
            tile(ui, "AVERAGE COMPLETION", &format!("{:.0}%", avg * 100.0)).on_hover_text("Across games with at least one unlock");
            tile(ui, "GAMES STARTED", &format!("{started} / {}", rows.len()));
            if points > 0 {
                tile(ui, "RA POINTS", &points.to_string());
            }
            if busy {
                ui.add(egui::Spinner::new().size(14.0)).on_hover_text("Checking achievements…");
            }
        });
        ui.add_space(20.0);

        let gap = 32.0;
        let col = (ui.available_width() - gap) / 2.0;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (i, (title, list, rare)) in [("Recent unlocks", &ov.recent, false), ("Rarest unlocks", &ov.rarest, true)].into_iter().enumerate() {
                if i > 0 {
                    ui.add_space(gap);
                }
                ui.allocate_ui_with_layout(vec2(col, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                    ui.set_width(col);
                    theme::section(ui, title);
                    for (gid, a) in list.iter().filter(|(g, _)| by_id.contains_key(g)).take(8) {
                        let extra = if rare {
                            a.percent.map_or(String::new(), |p| format!("{p:.1}% of players"))
                        } else {
                            crate::util::date(a.unlocked)
                        };
                        if unlock_row(ui, a, &by_id[gid].name, &extra) {
                            *open = Some(*gid);
                        }
                    }
                });
            }
        });

        let mut near: Vec<&Row> = rows.iter().filter(|r| r.2 < r.1 && pct(r) >= 0.6).collect();
        near.sort_by(|a, b| pct(b).total_cmp(&pct(a)));
        if !near.is_empty() {
            ui.add_space(16.0);
            theme::section(ui, "Almost there");
            for r in near.into_iter().take(6) {
                if game_row(ui, by_id[&r.0], r, thumbs) {
                    *open = Some(r.0);
                }
            }
        }

        ui.add_space(16.0);
        ui.horizontal(|ui| {
            theme::section(ui, "All games");
            ui.add_space(16.0);
            for (i, s) in SORTS.iter().enumerate() {
                ui.selectable_value(&mut ov.sort, i, *s);
            }
            ui.add_space(16.0);
            ui.checkbox(&mut ov.show_unstarted, "Include games with no unlocks");
        });
        let mut list: Vec<&Row> = rows.iter().filter(|r| ov.show_unstarted || r.2 > 0).collect();
        match ov.sort {
            0 => list.sort_by(|a, b| pct(b).total_cmp(&pct(a)).then(b.2.cmp(&a.2))),
            1 => list.sort_by_key(|r| std::cmp::Reverse(r.3)),
            _ => list.sort_by_cached_key(|r| by_id[&r.0].name.to_lowercase()),
        }
        for r in list {
            if game_row(ui, by_id[&r.0], r, thumbs) {
                *open = Some(r.0);
            }
        }
        ui.add_space(20.0);
    }
}

fn tile(ui: &mut Ui, label: &str, value: &str) -> egui::Response {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(theme::caption(label));
                ui.label(RichText::new(value).size(20.0).strong());
            });
        })
        .response
}

/// One unlocked achievement with its game; true when clicked.
fn unlock_row(ui: &mut Ui, a: &Achievement, game: &str, extra: &str) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 72.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return false;
    }
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, CARD);
    }
    let icon = egui::Rect::from_min_size(rect.min + vec2(4.0, 4.0), vec2(64.0, 64.0));
    if a.icon.starts_with("http") {
        let img = egui::Image::new(a.icon.as_str()).corner_radius(6);
        img.texture_options(crate::achievements::ICON_TEX).paint_at(ui, icon);
    } else {
        ui.painter().rect_filled(icon, 4.0, theme::CARD_HI);
    }
    let x = icon.right() + 12.0;
    let max = rect.right() - x - 4.0;
    let p = ui.painter();
    let line = |text: String, size: f32, color: Color32, y: f32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text, egui::FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(max);
        p.galley(egui::pos2(x, rect.top() + y), p.layout_job(job), color);
    };
    line(a.name.clone(), 15.0, TEXT, 15.0);
    line(format!("{game} · {extra}"), 12.5, TEXT_DIM, 38.0);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = if a.description.is_empty() { resp } else { resp.on_hover_text(&a.description) };
    resp.clicked()
}

/// A game's cover, name and progress bar; true when clicked.
fn game_row(ui: &mut Ui, g: &Game, r: &Row, thumbs: &mut crate::thumbs::ThumbCache) -> bool {
    let (outer, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0), Sense::click());
    if !ui.is_rect_visible(outer) {
        return false;
    }
    let rect = outer.shrink2(vec2(8.0, 3.0));
    if resp.hovered() {
        ui.painter().rect_filled(rect, 6.0, CARD);
    }
    let cover = egui::Rect::from_min_size(rect.min + vec2(6.0, 3.0), vec2(60.0, 80.0));
    match thumbs.get(g.id, g.has_cover) {
        Some(t) => {
            egui::Image::from_texture(t).corner_radius(4).paint_at(ui, cover);
        }
        None => {
            ui.painter().rect_filled(cover, 3.0, theme::CARD_HI);
        }
    }
    let (done, frac) = (r.2 == r.1, pct(r));
    let bar_w = (rect.width() * 0.28).clamp(120.0, 320.0);
    let bar = egui::Rect::from_min_size(egui::pos2(rect.right() - bar_w - 120.0, rect.center().y - 3.0), vec2(bar_w, 6.0));
    let x = cover.right() + 14.0;
    let p = ui.painter();
    let text = |s: String, size: f32, color: Color32, y: f32| {
        let mut job = egui::text::LayoutJob::simple_singleline(s, egui::FontId::proportional(size), color);
        job.wrap = egui::text::TextWrapping::truncate_at_width(bar.left() - x - 20.0);
        p.galley(egui::pos2(x, rect.top() + y), p.layout_job(job), color);
    };
    text(g.name.clone(), 15.0, TEXT, 24.0);
    let sub = match g.source {
        gamut_core::Source::Steam => "Steam".to_string(),
        _ => g.platform.clone().unwrap_or_default(),
    };
    text(sub, 12.5, TEXT_DIM, 46.0);
    p.rect_filled(bar, 3.0, theme::CARD_HI);
    let mut fill = bar;
    fill.set_width(bar.width() * frac);
    p.rect_filled(fill, 3.0, if done { GOLD } else { ACCENT });
    p.text(
        egui::pos2(rect.right() - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{} / {}   {:.0}%", r.2, r.1, frac * 100.0),
        egui::FontId::proportional(12.5),
        if done { GOLD } else { TEXT_DIM },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
