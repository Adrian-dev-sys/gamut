//! Settings → Backups, and the restore confirmation (applied on restart).
use super::*;
use gamut_core::backup::{self, Backup, Settings};
use std::path::PathBuf;

/// Backups found on disk, re-read every few seconds while the tab is open.
#[derive(Default)]
pub(super) struct BackupList {
    read_at: f64,
    dir: PathBuf,
    items: Vec<Backup>,
}

/// The Backups tab of Settings; `restore` is set when one is picked.
pub(super) fn tab(ui: &mut Ui, s: &mut Settings, paths: &gamut_core::Paths, workers: &crate::workers::Workers, list: &mut BackupList, restore: &mut Option<PathBuf>) {
    theme::section(ui, "Backups");
    ui.checkbox(&mut s.auto, "Back up automatically");
    ui.add_enabled_ui(s.auto, |ui| {
        ui.horizontal(|ui| {
            ui.label("Every");
            ui.add(egui::DragValue::new(&mut s.days).range(1..=90).suffix(" days"));
        });
    });
    ui.horizontal(|ui| {
        ui.label("Keep the newest");
        ui.add(egui::DragValue::new(&mut s.keep).range(1..=100));
    });
    ui.checkbox(&mut s.art, "Include covers, banners and icons (copied once, then only what changed)");
    let dir = backup::dir(paths, s);
    ui.horizontal(|ui| {
        ui.label("Folder");
        ui.add(egui::TextEdit::singleline(&mut s.dir).hint_text(dir.display().to_string()).desired_width(390.0));
        if ui.button("Browse…").clicked() {
            if let Some(d) = rfd::FileDialog::new().set_title("Backup folder").pick_folder() {
                s.dir = d.display().to_string();
            }
        }
    });
    ui.label(theme::hint(
        "The library, settings (including your API keys) and local achievements go into a dated zip. \
         A folder on another drive also survives a failing disk. Screenshots (Pictures\\Gamut) aren't included.",
    ));
    ui.add_space(8.0);
    let busy = workers.backing_up.load(Ordering::Relaxed);
    ui.horizontal(|ui| {
        if ui.add_enabled(!busy, egui::Button::new("Back up now")).clicked() {
            workers.backup(Some(s.clone()));
        }
        if busy {
            ui.spinner();
            ui.label(theme::hint("Backing up…"));
        }
        if ui.add_enabled(dir.is_dir(), egui::Button::new("Open folder")).clicked() {
            let _ = open::that(&dir);
        }
    });

    let now = ui.input(|i| i.time);
    if list.dir != dir || now - list.read_at > 2.0 {
        (list.dir, list.read_at, list.items) = (dir.clone(), now, backup::list(&dir));
    }
    ui.add_space(12.0);
    ui.separator();
    theme::section(ui, "Restore");
    if list.items.is_empty() {
        ui.label(theme::hint("No backups in this folder yet."));
        return;
    }
    egui::Grid::new("backups").num_columns(3).spacing([18.0, 6.0]).show(ui, |ui| {
        for b in &list.items {
            ui.label(label(b));
            ui.label(theme::hint(&format!("{:.1} MB", b.size as f64 / 1_048_576.0)));
            if ui.button("Restore…").clicked() {
                *restore = Some(b.path.clone());
            }
            ui.end_row();
        }
    });
    ui.label(theme::hint("Restoring restarts Gamut. The current library and settings are kept next to the restored ones."));
}

/// "2026-11-28 15:30" from the file name.
fn label(b: &Backup) -> String {
    let stem = b.path.file_stem().unwrap_or_default().to_string_lossy();
    let t = stem.trim_start_matches("gamut-backup-");
    match (t.get(..10), t.get(11..13), t.get(13..15)) {
        (Some(d), Some(h), Some(m)) => format!("{d} {h}:{m}"),
        _ => t.to_string(),
    }
}

impl GamutApp {
    pub(super) fn restore_window(&mut self, ctx: &egui::Context) {
        let Some(path) = self.confirm_restore.clone() else { return };
        let (mut close, mut open) = (false, true);
        theme::dialog(ctx, "Restore backup", false, |ui| {
            ui.set_max_width(480.0);
            dialog_header(ui, "Restore backup", &mut open);
            let when = label(&Backup { path: path.clone(), time: std::time::SystemTime::now(), size: 0 });
            ui.label(format!("Restore the backup from {when}?"));
            ui.label(theme::hint(
                "Gamut restarts and its library, settings and local achievements become the backup's. \
                 Games added or changed since then are lost; the current files are kept as *.pre-restore-<time>. \
                 Art that's missing is put back.",
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Restore and restart").color(Color32::WHITE)).fill(ACCENT())).clicked() {
                    match backup::stage(&self.paths, &path) {
                        Ok(()) => self.restart(ctx),
                        Err(e) => self.toasts.error(format!("Can't restore: {e:#}")),
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || !open {
            self.confirm_restore = None;
        }
    }

    /// Starts a new Gamut that waits for this one to exit, then exits.
    fn restart(&mut self, ctx: &egui::Context) {
        let started = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--restarting").spawn());
        if let Err(e) = started {
            self.toasts.error(format!("Couldn't restart ({e}); the restore happens the next time Gamut starts."));
            return;
        }
        log::info!("restarting to restore a backup");
        self.settings = None;
        self.quitting = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}
