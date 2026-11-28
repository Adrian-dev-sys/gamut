//! Settings → Stores, and the review dialog that matches a store's games to the library before importing.
use super::*;
use gamut_core::stores::sync::{self, Decision, Match, Planned};
use gamut_core::stores::Store;
use std::collections::HashMap;

pub(super) struct Review {
    store: Store,
    plan: Vec<Planned>,
    /// Doubts and new games: what to do with each (by store id).
    choice: HashMap<String, Decision>,
}

impl Review {
    pub(super) fn new(store: Store, plan: Vec<Planned>) -> Self {
        let choice = plan
            .iter()
            .filter_map(|p| match &p.m {
                Match::Doubt(c) => Some((p.game.id.clone(), Decision::Link(c[0]))),
                Match::New => Some((p.game.id.clone(), Decision::Import)),
                _ => None,
            })
            .collect();
        Review { store, plan, choice }
    }
}

/// The Stores tab; returns a store whose review was asked for.
pub(super) fn tab(ui: &mut Ui, s: &mut gamut_core::Config, lib: &gamut_core::Library, busy: bool) -> Option<Store> {
    let mut review = None;
    for store in Store::ALL {
        theme::section(ui, store.name());
        let on = s.stores.iter().any(|k| k == store.key());
        let linked = lib.store_links(store.name()).map(|l| l.len()).unwrap_or(0);
        let skipped = lib.store_skipped(store.name()).map(|l| l.len()).unwrap_or(0);
        ui.label(theme::hint(&match store {
            Store::Gog => "Owned games come from GOG Galaxy; without it, only games installed with GOG's installers are found. \
                           Games that aren't installed get Install, which opens them in GOG Galaxy.",
        }));
        ui.horizontal(|ui| {
            let label = if on { "Review…" } else { "Import games…" };
            if ui.add_enabled(!busy, egui::Button::new(label)).on_hover_text("Shows what's new and what may already be in your library before anything changes").clicked() {
                review = Some(store);
            }
            if busy {
                ui.spinner();
            }
            if on {
                let mut keep = true;
                if ui.checkbox(&mut keep, "Keep in sync").on_hover_text("On start, hourly and with Refresh").changed() && !keep {
                    s.stores.retain(|k| k != store.key());
                }
            }
        });
        let mut status = format!("{linked} games linked");
        if skipped > 0 {
            status += &format!(", {skipped} not imported");
        }
        ui.horizontal(|ui| {
            ui.label(theme::hint(&status));
            if skipped > 0 && ui.small_button("Offer them again").clicked() {
                let _ = lib.unskip_store(store.name());
            }
        });
        ui.add_space(10.0);
    }
    review
}

impl GamutApp {
    pub(super) fn review_window(&mut self, ctx: &egui::Context) {
        let Some(r) = self.store_review.as_mut() else { return };
        let (mut close, mut open, mut apply) = (false, true, false);
        let name = |id: i64| self.games.iter().find(|g| g.id == id).map_or(format!("#{id}"), |g| match &g.platform {
            Some(p) if !p.starts_with("PC") => format!("{} ({p})", g.name),
            _ => g.name.clone(),
        });
        let (mut known, mut doubts, mut new) = (Vec::new(), Vec::new(), Vec::new());
        for p in &r.plan {
            match &p.m {
                Match::Linked(id) | Match::Exact(id) => known.push((p, *id)),
                Match::Doubt(_) => doubts.push(p),
                Match::New => new.push(p),
            }
        }
        let screen = ctx.content_rect();
        let size = vec2(720.0_f32.min(screen.width() - 80.0), 620.0_f32.min(screen.height() - 120.0));
        let title = format!("Import from {}", r.store.name());
        theme::dialog(ctx, &title, false, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            dialog_header(ui, &title, &mut open);
            ui.label(format!(
                "{} games: {} already in your library, {} to check, {} new.",
                r.plan.len(),
                known.len(),
                doubts.len(),
                new.len()
            ));
            ui.add_space(6.0);
            ScrollArea::vertical().max_height(ui.available_height() - 52.0).auto_shrink([false, false]).show(ui, |ui| {
                if !doubts.is_empty() {
                    theme::section(ui, "May already be in your library");
                    ui.label(theme::hint("Same name or folder as a game you have. Pick what each one is."));
                    egui::Grid::new("doubts").num_columns(2).spacing([14.0, 6.0]).show(ui, |ui| {
                        for p in &doubts {
                            let Match::Doubt(cands) = &p.m else { continue };
                            ui.label(&p.game.name);
                            let c = r.choice.entry(p.game.id.clone()).or_insert(Decision::Link(cands[0]));
                            let text = |d: &Decision| match d {
                                Decision::Link(id) => format!("Same as {}", name(*id)),
                                Decision::Import => "Import as a new game".into(),
                                Decision::Skip => "Don't import".into(),
                            };
                            egui::ComboBox::from_id_salt(("doubt", &p.game.id)).width(340.0).selected_text(text(c)).show_ui(ui, |ui| {
                                for d in cands.iter().map(|id| Decision::Link(*id)).chain([Decision::Import, Decision::Skip]) {
                                    let t = text(&d);
                                    ui.selectable_value(c, d, t);
                                }
                            });
                            ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                }
                if !new.is_empty() {
                    ui.horizontal(|ui| {
                        theme::section(ui, "New");
                        if ui.small_button("All").clicked() {
                            new.iter().for_each(|p| _ = r.choice.insert(p.game.id.clone(), Decision::Import));
                        }
                        if ui.small_button("None").clicked() {
                            new.iter().for_each(|p| _ = r.choice.insert(p.game.id.clone(), Decision::Skip));
                        }
                    });
                    for p in &new {
                        let c = r.choice.entry(p.game.id.clone()).or_insert(Decision::Import);
                        let mut on = *c == Decision::Import;
                        let label = if p.game.installed { format!("{}  (installed)", p.game.name) } else { p.game.name.clone() };
                        if ui.checkbox(&mut on, label).changed() {
                            *c = if on { Decision::Import } else { Decision::Skip };
                        }
                    }
                    ui.label(theme::hint("Unticked games aren't imported or offered again (Settings → Stores can bring them back)."));
                    ui.add_space(10.0);
                }
                if !known.is_empty() {
                    egui::CollapsingHeader::new(format!("Already in your library ({})", known.len())).show(ui, |ui| {
                        ui.label(theme::hint("Linked by their store id: their installed state and launch follow the store from now on."));
                        for (p, id) in &known {
                            let lib_name = name(*id);
                            let same = sync::norm(&lib_name) == sync::norm(&p.game.name);
                            ui.label(if same { p.game.name.clone() } else { format!("{}  →  {lib_name}", p.game.name) });
                        }
                    });
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Import").color(Color32::WHITE)).fill(ACCENT())).clicked() {
                    apply = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                ui.label(theme::hint("Then kept in sync on start, hourly and with Refresh."));
            });
        });
        if apply {
            let r = self.store_review.take().unwrap();
            match sync::apply(&mut self.lib, r.store, &r.plan, sync::decided(&r.choice)) {
                Ok(a) => {
                    self.toasts.success(format!("{}: {} imported, {} updated, {} not imported.", r.store.name(), a.added, a.updated, a.skipped));
                    let mut cfg = self.cfg.write().unwrap();
                    if !cfg.stores.iter().any(|k| k == r.store.key()) {
                        cfg.stores.push(r.store.key().into());
                        if let Some(s) = self.settings.as_mut() {
                            s.stores = cfg.stores.clone();
                        }
                        if let Err(e) = cfg.save(&self.paths.config) {
                            self.toasts.error(format!("Saving settings failed: {e:#}"));
                        }
                    }
                    drop(cfg);
                    self.reload_all();
                    self.workers.kick();
                }
                Err(e) => self.toasts.error(format!("{} import failed: {e:#}", r.store.name())),
            }
        } else if close || !open {
            self.store_review = None;
        }
    }
}
