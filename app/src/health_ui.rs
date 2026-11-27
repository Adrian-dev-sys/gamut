//! Library health dialog: broken games and likely duplicates, checked off-thread.
use crate::theme::{self, TEXT_DIM};
use crate::ui::dialog_header;
use crossbeam_channel::Receiver;
use egui::{RichText, Ui};
use gamut_core::health::{self, Issue, Problem};
use gamut_core::{Game, Source};

pub struct HealthDialog {
    /// Working copy of the settings; saved when changed.
    pub settings: health::Settings,
    issues: Vec<Issue>,
    rx: Option<Receiver<Vec<Issue>>>,
    /// The library changed since the last check (re-run once other dialogs close).
    pub stale: bool,
    skip_input: String,
}

pub enum HealthOut {
    Edit(i64),
    Remove(i64),
    /// Settings changed (ignore list, skipped platforms).
    Save,
    Close,
}

impl HealthDialog {
    pub fn new(settings: health::Settings, games: &[Game]) -> Self {
        let mut d = HealthDialog { settings, issues: Vec::new(), rx: None, stale: false, skip_input: String::new() };
        d.run(games);
        d
    }

    pub fn run(&mut self, games: &[Game]) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (games, s) = (games.to_vec(), self.settings.clone());
        // File checks can stall on a sleeping or network drive.
        std::thread::spawn(move || {
            let _ = tx.send(health::check(&games, &s));
        });
        (self.rx, self.stale) = (Some(rx), false);
    }

    pub fn show(&mut self, ctx: &egui::Context, games: &[Game], platforms: &[(String, usize)]) -> Option<HealthOut> {
        if let Some(r) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.issues, self.rx) = (r, None);
        }
        if self.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let (mut out, mut open) = (None, true);
        let name = |id: i64| games.iter().find(|g| g.id == id);
        let screen = ctx.content_rect();
        let size = egui::vec2(820.0_f32.min(screen.width() - 80.0), 700.0_f32.min(screen.height() - 100.0));
        let resp = theme::dialog(ctx, "library_health", false, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            dialog_header(ui, "Library health", &mut open);
            ui.horizontal(|ui| {
                let n = self.issues.len();
                ui.label(if self.rx.is_some() {
                    "Checking…".to_string()
                } else if n == 0 {
                    "Nothing wrong found.".to_string()
                } else {
                    format!("{n} problem{}", if n == 1 { "" } else { "s" })
                });
                if ui.add_enabled(self.rx.is_none(), egui::Button::new("Check again")).clicked() {
                    self.run(games);
                }
            });
            ui.collapsing("Settings", |ui| {
                ui.label("Skip platforms").on_hover_text("Games you play on the console itself, for example.");
                let before = self.settings.skip_platforms.clone();
                crate::editor::chips(ui, "health_skip", &mut self.settings.skip_platforms, &mut self.skip_input, platforms, "add a platform");
                if self.settings.skip_platforms != before {
                    out = Some(HealthOut::Save);
                }
                let n = self.settings.ignored.len();
                if n > 0 && ui.button(format!("Show {n} ignored again")).clicked() {
                    self.settings.ignored.clear();
                    out = Some(HealthOut::Save);
                }
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
                let mut ignore = None;
                for (i, issue) in self.issues.iter().enumerate() {
                    let gs: Vec<&Game> = issue.ids.iter().filter_map(|&id| name(id)).collect();
                    let Some(first) = gs.first() else { continue };
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&first.name).strong());
                            ui.label(RichText::new(health::platform(first)).color(TEXT_DIM));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // A disconnected drive fixes itself; nothing to dismiss.
                                if !matches!(issue.problem, Problem::DriveMissing(_))
                                    && ui.button("Ignore").on_hover_text("Don't report this again").clicked()
                                {
                                    ignore = Some(i);
                                }
                                if gs.len() == 1 {
                                    row_buttons(ui, first.id, &mut out);
                                }
                            });
                        });
                        ui.label(theme::hint(&issue.problem.describe()));
                        if gs.len() > 1 {
                            for g in &gs {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(copy_label(g)).color(TEXT_DIM));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| row_buttons(ui, g.id, &mut out));
                                });
                            }
                        }
                    });
                }
                if let Some(i) = ignore {
                    let issue = self.issues.remove(i);
                    self.settings.ignored.push(issue.key);
                    out = Some(HealthOut::Save);
                }
            });
        });
        if !open || resp.should_close() {
            out = Some(HealthOut::Close);
        }
        out
    }
}

fn row_buttons(ui: &mut Ui, id: i64, out: &mut Option<HealthOut>) {
    if ui.button("Remove…").clicked() {
        *out = Some(HealthOut::Remove(id));
    }
    if ui.button("Edit…").clicked() {
        *out = Some(HealthOut::Edit(id));
    }
}

/// One copy of a duplicate: where it's from, how it starts, time played.
fn copy_label(g: &Game) -> String {
    let from = match g.source {
        Source::Steam => "Steam".to_string(),
        Source::Custom => g.store.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Custom".into()),
    };
    let how = g.exe.as_deref().map(|e| format!(" · {}", short(e))).unwrap_or_default();
    let played = if g.playtime_min > 0 { format!(" · {}h played", (g.playtime_min + 30) / 60) } else { String::new() };
    format!("{from}{how}{played}")
}

/// The last two path parts ("Hades\Hades.exe").
fn short(p: &str) -> String {
    let parts: Vec<&str> = p.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 { p.to_string() } else { format!("…\\{}", parts[parts.len() - 2..].join("\\")) }
}
