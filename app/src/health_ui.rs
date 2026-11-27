//! Library health dialog: broken games and likely duplicates, checked off-thread.
use crate::theme::{self, ACCENT, TEXT_DIM};
use crate::ui::dialog_header;
use crate::util;
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
    /// "Keep only this" awaiting confirmation: issue key, kept game, move playtime.
    keep: Option<(String, i64, bool)>,
}

pub enum HealthOut {
    Edit(i64),
    Remove(i64),
    /// Remove the other copies of a duplicate; optionally add their playtime to the kept one.
    KeepOnly { keep: i64, remove: Vec<i64>, move_playtime: bool },
    /// Settings changed (ignore list, skipped platforms).
    Save,
    Close,
}

impl HealthDialog {
    pub fn new(settings: health::Settings, games: &[Game]) -> Self {
        let mut d = HealthDialog { settings, issues: Vec::new(), rx: None, stale: false, skip_input: String::new(), keep: None };
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
        let screen = ctx.content_rect();
        let size = egui::vec2(820.0_f32.min(screen.width() - 80.0), 700.0_f32.min(screen.height() - 100.0));
        let resp = theme::dialog(ctx, "library_health", false, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            dialog_header(ui, "Library health", &mut open);
            let issues = self.issues.clone();
            let (dups, rest): (Vec<&Issue>, Vec<&Issue>) = issues.iter().partition(|i| i.problem == Problem::Duplicate);
            let (absent, broken): (Vec<&Issue>, Vec<&Issue>) = rest.into_iter().partition(|i| i.problem == Problem::NotInstalled);
            ui.horizontal(|ui| {
                ui.label(if self.rx.is_some() {
                    "Checking…".to_string()
                } else if self.issues.is_empty() {
                    "Nothing wrong found.".to_string()
                } else {
                    format!("{} can't start · {} possible duplicates · {} not installed", broken.len(), dups.len(), absent.len())
                });
                if ui.add_enabled(self.rx.is_none(), egui::Button::new("Check again")).clicked() {
                    self.run(games);
                }
            });
            ui.collapsing("Settings", |ui| {
                ui.label("Don't check these platforms").on_hover_text("Games you play on the console itself, for example.");
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
            let mut ignore = None;
            egui::ScrollArea::vertical().max_height(ui.available_height()).auto_shrink([false, false]).show(ui, |ui| {
                let find = |id: i64| games.iter().find(|g| g.id == id);
                let mut single = |ui: &mut Ui, list: &[&Issue]| {
                    for issue in list {
                        let Some(g) = issue.ids.first().and_then(|&id| find(id)) else { continue };
                        card(ui, |ui| {
                            ui.horizontal(|ui| {
                                title(ui, g);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    // A disconnected drive fixes itself; nothing to dismiss.
                                    if !matches!(issue.problem, Problem::DriveMissing(_))
                                        && ui.button("Ignore").on_hover_text("It's fine, don't report it again").clicked()
                                    {
                                        ignore = Some(issue.key.clone());
                                    }
                                    if ui.button("Remove…").clicked() {
                                        out = Some(HealthOut::Remove(g.id));
                                    }
                                    if ui.button("Edit…").clicked() {
                                        out = Some(HealthOut::Edit(g.id));
                                    }
                                });
                            });
                            ui.label(theme::hint(&issue.problem.describe()));
                        });
                    }
                };
                if !broken.is_empty() {
                    theme::section(ui, &format!("CAN'T START ({})", broken.len()));
                    ui.label(theme::hint("Nothing to launch, or the files moved. Edit to point at the new location, or Remove if it's gone for good."));
                    single(ui, &broken);
                    ui.add_space(8.0);
                }
                if !absent.is_empty() {
                    egui::CollapsingHeader::new(theme::caption(&format!("NOT INSTALLED ({})", absent.len()))).id_salt("health_absent").show(ui, |ui| {
                        ui.label(theme::hint(
                            "Imported as not installed, with nothing to start. Edit to add the exe if you have it, Remove it, or Ignore it to keep it as a wishlist/owned entry.",
                        ));
                        single(ui, &absent);
                    });
                    ui.add_space(8.0);
                }
                if !dups.is_empty() {
                    theme::section(ui, &format!("POSSIBLE DUPLICATES ({})", dups.len()));
                    ui.label(theme::hint(
                        "The same game was added more than once for one platform. Pick the copy to keep (the one marked \"suggested\" \
                         it can start and has the most playtime and details). \"Not a duplicate\" hides the group.",
                    ));
                    for issue in &dups {
                        let gs: Vec<&Game> = issue.ids.iter().filter_map(|&id| find(id)).collect();
                        if gs.len() < 2 {
                            continue;
                        }
                        let best = gs.iter().max_by_key(|g| keep_score(g)).map(|g| g.id);
                        let mut cancel = false;
                        card(ui, |ui| {
                            ui.horizontal(|ui| {
                                title(ui, gs[0]);
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button("Not a duplicate").on_hover_text("Keep all copies and don't report them again").clicked() {
                                        ignore = Some(issue.key.clone());
                                    }
                                });
                            });
                            for g in &gs {
                                ui.separator();
                                copy_row(ui, g, best == Some(g.id), &mut out, &issue.key, &mut self.keep);
                            }
                            if let Some((_, keep, mv)) = self.keep.as_mut().filter(|(k, ..)| *k == issue.key) {
                                let others: Vec<&Game> = gs.iter().copied().filter(|g| g.id != *keep).collect();
                                let mins: i64 = others.iter().map(|g| g.playtime_min).sum();
                                ui.separator();
                                ui.label(format!(
                                    "Remove the other {} cop{} from the library? Their files aren't touched.",
                                    others.len(),
                                    if others.len() == 1 { "y" } else { "ies" }
                                ));
                                if mins > 0 {
                                    ui.checkbox(mv, format!("Add their playtime ({}) to the kept copy", util::playtime(mins)));
                                }
                                ui.horizontal(|ui| {
                                    if ui.button(RichText::new("Remove the others").color(ACCENT)).clicked() {
                                        out = Some(HealthOut::KeepOnly { keep: *keep, remove: others.iter().map(|g| g.id).collect(), move_playtime: *mv && mins > 0 });
                                    }
                                    if ui.button("Cancel").clicked() {
                                        cancel = true;
                                    }
                                });
                            }
                            if cancel {
                                self.keep = None;
                            }
                        });
                    }
                }
            });
            if let Some(key) = ignore {
                self.issues.retain(|i| i.key != key);
                self.settings.ignored.push(key);
                out = Some(HealthOut::Save);
            }
        });
        if matches!(out, Some(HealthOut::KeepOnly { .. })) {
            self.keep = None;
        }
        if !open || resp.should_close() {
            out = Some(HealthOut::Close);
        }
        out
    }
}

fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
    ui.add_space(4.0);
}

fn title(ui: &mut Ui, g: &Game) {
    ui.label(RichText::new(&g.name).strong().size(15.0));
    ui.label(RichText::new(health::platform(g)).color(TEXT_DIM));
}

/// One copy of a duplicate: how it starts on the first line, where it came from and what it has below.
fn copy_row(ui: &mut Ui, g: &Game, suggested: bool, out: &mut Option<HealthOut>, key: &str, keep: &mut Option<(String, i64, bool)>) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            let head = if suggested { format!("{}  (suggested)", launcher(g)) } else { launcher(g) };
            ui.label(if suggested { RichText::new(head).color(ACCENT) } else { RichText::new(head) });
            ui.label(theme::hint(&details(g)));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Keep only this").clicked() {
                *keep = Some((key.to_string(), g.id, true));
            }
            if ui.button("Edit…").clicked() {
                *out = Some(HealthOut::Edit(g.id));
            }
        });
    });
}

/// "Steam", "Azahar · Fire Emblem Awakening.3ds", "…\Game\game.exe", "No launcher (GOG)".
fn launcher(g: &Game) -> String {
    if g.source == Source::Steam {
        return "Steam".into();
    }
    let Some(exe) = g.exe.as_deref().filter(|e| !e.trim().is_empty()) else {
        return match g.store.as_deref().filter(|s| !s.is_empty()) {
            Some(s) => format!("No launcher ({s})"),
            None => "No launcher".into(),
        };
    };
    let file = |p: &str| std::path::Path::new(p).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string());
    // Emulators: the ROM tells copies apart, not the shared emulator exe.
    let rom = g.args.as_deref().unwrap_or("").split('"').skip(1).step_by(2).find(|a| a.contains(['\\', '/'])).map(file);
    let stem = std::path::Path::new(exe).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    match rom {
        Some(r) => format!("{stem} · {r}"),
        None => short(exe),
    }
}

fn details(g: &Game) -> String {
    let from = if g.playnite_id.is_some() {
        "Playnite import".to_string()
    } else {
        g.store.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "Added in Gamut".into())
    };
    let played = if g.playtime_min > 0 { format!("{} played", util::playtime(g.playtime_min)) } else { "never played".into() };
    let cover = if g.has_cover { "has cover" } else { "no cover" };
    format!("{from} · added {} · {played} · {cover}", util::date(g.added))
}

/// Higher = better copy to keep: can start, played, has art and details, added earlier.
fn keep_score(g: &Game) -> (bool, i64, bool, bool, i64) {
    let starts = g.source == Source::Steam || g.exe.as_deref().is_some_and(|e| !e.trim().is_empty());
    (starts, g.playtime_min, g.has_cover, g.description.is_some(), -g.added)
}

/// The last two path parts ("…\Hades\Hades.exe").
fn short(p: &str) -> String {
    let parts: Vec<&str> = p.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 { p.to_string() } else { format!("…\\{}", parts[parts.len() - 2..].join("\\")) }
}
