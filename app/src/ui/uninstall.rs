//! Uninstall: Steam's dialog for Steam games, a confirmed Windows uninstaller for others.
use super::*;
use gamut_core::uninstall::{self, Uninstaller};

/// Whether the menu offers Uninstall (the registry is only read on click).
pub(super) fn offered(g: &Game) -> bool {
    match g.source {
        Source::Steam => g.installed,
        _ => g.rom().is_none() && g.folder().is_some_and(|d| d.is_dir()),
    }
}

impl GamutApp {
    pub(super) fn uninstall(&mut self, id: i64) -> anyhow::Result<()> {
        let Some(g) = self.game(id).cloned() else { return Ok(()) };
        // An exe several games start is an emulator or launcher: its uninstaller isn't this game's.
        let shared = |e: &str| self.games.iter().filter(|o| o.exe.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(e))).count() > 1;
        if g.source != Source::Steam && g.install_dir.is_none() && g.exe.as_deref().is_some_and(shared) {
            anyhow::bail!("{} starts through a program other games use too, so Gamut won't uninstall it; set its install folder in Edit first.", g.name);
        }
        match uninstall::find(&g) {
            Some(u @ Uninstaller::Steam(_)) => {
                uninstall::run(&u)?;
                self.toasts.info(format!("Steam will ask to confirm uninstalling {}.", g.name));
            }
            Some(u) => self.confirm_uninstall = Some((id, u, false)),
            None => anyhow::bail!("Windows lists no uninstaller for {}'s folder; delete the folder yourself (Open install folder).", g.name),
        }
        Ok(())
    }

    pub(super) fn uninstall_window(&mut self, ctx: &egui::Context) {
        let Some((id, u, mut remove)) = self.confirm_uninstall.clone() else { return };
        let Uninstaller::App { name: app, command } = &u else { return };
        let name = self.game(id).map(|g| g.name.clone()).unwrap_or_default();
        let (mut close, mut open, mut go) = (false, true, false);
        theme::dialog(ctx, "Uninstall game", false, |ui| {
            ui.set_max_width(520.0);
            dialog_header(ui, "Uninstall game", &mut open);
            ui.label(format!("Uninstall \"{name}\"? This runs \"{app}\"'s uninstaller from Windows' installed apps:"));
            ui.label(theme::hint(command));
            ui.checkbox(&mut remove, "Also remove it from the library");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Uninstall").color(Color32::WHITE)).fill(theme::DANGER())).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if go {
            close = true;
            match uninstall::run(&u) {
                Ok(()) => {
                    self.toasts.info(format!("Started {app}'s uninstaller."));
                    if remove {
                        if let Err(e) = self.remove_games(&[id]) {
                            self.toasts.error(format!("{e:#}"));
                        }
                    }
                }
                Err(e) => self.toasts.error(format!("Couldn't start the uninstaller: {e:#}")),
            }
        }
        match close || !open {
            true => self.confirm_uninstall = None,
            false => self.confirm_uninstall = Some((id, u, remove)),
        }
    }
}
