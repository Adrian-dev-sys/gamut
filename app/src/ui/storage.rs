//! Settings → General → Storage: what Gamut's data uses, with buttons for what can go.
use super::*;
use gamut_core::storage::{self, Kind, Usage};
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(super) struct StorageView {
    /// Measured on a worker thread; None while measuring.
    usage: Arc<Mutex<Option<Vec<Usage>>>>,
    started: bool,
    /// Asked once to delete this; the next click deletes.
    armed: Option<Kind>,
    /// Bytes freed by the last clear, shown until the next.
    freed: Arc<Mutex<Option<(Kind, u64)>>>,
}

fn mb(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b => format!("{:.0} MB", (b as f64 / (1u64 << 20) as f64).max(if b > 0 { 1.0 } else { 0.0 })),
    }
}

impl StorageView {
    /// Measures again (after clearing, or when Settings opens).
    fn refresh(&mut self, ctx: &egui::Context, paths: &gamut_core::Paths, backups: std::path::PathBuf) {
        self.started = true;
        *self.usage.lock().unwrap() = None;
        let (out, paths, ctx) = (self.usage.clone(), paths.clone(), ctx.clone());
        std::thread::spawn(move || {
            *out.lock().unwrap() = Some(storage::measure(&paths, &backups));
            ctx.request_repaint();
        });
    }

    pub(super) fn reset(&mut self) {
        (self.started, self.armed) = (false, None);
    }
}

pub(super) fn section(ui: &mut Ui, v: &mut StorageView, paths: &gamut_core::Paths, backups: std::path::PathBuf) -> bool {
    theme::section(ui, "Storage");
    if !v.started {
        v.refresh(ui.ctx(), paths, backups.clone());
    }
    let usage = v.usage.lock().unwrap().clone();
    let Some(usage) = usage else {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(theme::hint("Measuring…"));
        });
        return false;
    };
    let total: u64 = usage.iter().map(|u| u.bytes).sum();
    ui.label(format!("Gamut uses {} in its data folder (backups included).", mb(total)));
    let freed = *v.freed.lock().unwrap();
    let mut clear = None;
    egui::Grid::new("storage").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
        for u in usage.iter().filter(|u| u.bytes > 0 || u.clearable) {
            ui.label(u.label).on_hover_text(u.hint);
            ui.label(theme::hint(&format!("{}  ·  {} files", mb(u.bytes), u.files)));
            if u.clearable {
                let armed = v.armed == Some(u.kind);
                let label = match (armed, u.kind) {
                    (true, _) => "Click again to delete",
                    (_, Kind::Thumbs) => "Clear",
                    _ => "Delete…",
                };
                let mut b = egui::Button::new(label);
                if armed {
                    b = b.fill(theme::DANGER());
                }
                if ui.add_enabled(u.bytes > 0, b).on_hover_text(u.hint).clicked() {
                    if armed || u.kind == Kind::Thumbs {
                        clear = Some(u.kind);
                    } else {
                        v.armed = Some(u.kind);
                    }
                }
            } else {
                ui.label("");
            }
            ui.end_row();
        }
    });
    if let Some((k, b)) = freed {
        let what = usage.iter().find(|u| u.kind == k).map_or("", |u| u.label);
        ui.label(theme::hint(&format!("{what}: freed {}.", mb(b))));
    }
    if let Some(kind) = clear {
        v.armed = None;
        let (paths2, out, freed, ctx) = (paths.clone(), v.usage.clone(), v.freed.clone(), ui.ctx().clone());
        *out.lock().unwrap() = None;
        std::thread::spawn(move || {
            match storage::clear(&paths2, kind) {
                Ok(b) => *freed.lock().unwrap() = Some((kind, b)),
                Err(e) => log::warn!("clearing {kind:?}: {e:#}"),
            }
            *out.lock().unwrap() = Some(storage::measure(&paths2, &backups));
            ctx.request_repaint();
        });
    }
    // The caller drops loaded thumbnails too.
    clear == Some(Kind::Thumbs)
}
