//! "Status" lines in Settings: each service's key is checked live, shortly after it changes.
use crate::theme::{DANGER, GOOD, TEXT_DIM};
use egui::{RichText, Ui};
use gamut_core::Config;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
enum Check {
    #[default]
    NotSet,
    Checking,
    Ok(String),
    Err(String),
}

pub const STEAM: usize = 0;
pub const SGDB: usize = 1;
pub const RETRO: usize = 2;

#[derive(Default)]
pub struct KeyChecks {
    state: Arc<Mutex<[Check; 3]>>,
    /// Values last checked, and the values seen last frame with when they changed.
    checked: [String; 3],
    seen: [(String, f64); 3],
}

fn values(c: &Config, i: usize) -> String {
    match i {
        STEAM => format!("{}\n{}", c.steam_api_key.trim(), c.steam_id.trim()),
        SGDB => c.steamgriddb_key.trim().to_string(),
        _ => format!("{}\n{}", c.ra_user.trim(), c.ra_api_key.trim()),
    }
}

impl KeyChecks {
    /// Starts checks for keys that changed and then sat still for a second (immediately on first open).
    pub fn update(&mut self, c: &Config, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        for i in 0..3 {
            let v = values(c, i);
            if self.seen[i].0 != v {
                let first = self.checked[i].is_empty() && self.seen[i].0.is_empty();
                self.seen[i] = (v.clone(), if first { f64::MIN } else { now });
            }
            if v == self.checked[i] || now - self.seen[i].1 < 1.0 {
                if v != self.checked[i] {
                    ctx.request_repaint_after(std::time::Duration::from_millis(300));
                }
                continue;
            }
            self.checked[i] = v;
            let empty = match i {
                STEAM => c.steam_api_key.trim().is_empty(),
                SGDB => c.steamgriddb_key.trim().is_empty(),
                _ => c.ra_user.trim().is_empty() || c.ra_api_key.trim().is_empty(),
            };
            if empty {
                self.state.lock().unwrap()[i] = Check::NotSet;
                continue;
            }
            self.state.lock().unwrap()[i] = Check::Checking;
            let (state, c, ctx) = (self.state.clone(), c.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = match i {
                    STEAM => gamut_core::status::steam(&c),
                    SGDB => gamut_core::status::steamgriddb(&c.steamgriddb_key),
                    _ => gamut_core::status::retro(&c.ra_user, &c.ra_api_key),
                };
                // A newer check may have started meanwhile; only the latest values' result counts.
                let mut s = state.lock().unwrap();
                if matches!(s[i], Check::Checking) {
                    s[i] = match r {
                        Ok(m) => Check::Ok(m),
                        Err(e) => Check::Err(format!("{e:#}")),
                    };
                }
                ctx.request_repaint();
            });
        }
    }

    /// Forgets results so the next open checks again.
    pub fn reset(&mut self) {
        *self = KeyChecks::default();
    }

    /// One "Status" grid row.
    pub fn row(&self, ui: &mut Ui, i: usize) {
        ui.label("Status");
        let s = self.state.lock().unwrap()[i].clone();
        ui.horizontal(|ui| match s {
            Check::NotSet => {
                ui.label(RichText::new("○ Not set").color(TEXT_DIM));
            }
            Check::Checking => {
                ui.add(egui::Spinner::new().size(12.0));
                ui.label(RichText::new("Checking…").color(TEXT_DIM));
            }
            Check::Ok(m) => {
                ui.label(RichText::new("● ").color(GOOD));
                ui.label(m);
            }
            Check::Err(m) => {
                ui.label(RichText::new("● ").color(DANGER));
                ui.label(RichText::new(m).color(DANGER));
            }
        });
        ui.end_row();
    }
}
