//! Folder scanners: manage saved scanners, run one off-thread, then review what it found before adding.
use crate::theme::{self, ACCENT, TEXT_DIM};
use crate::ui::dialog_header;
use crossbeam_channel::Receiver;
use egui::{vec2, Color32, RichText, Ui};
use gamut_core::scanner::{self, Candidate, Scanner};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct ScanDialog {
    pub scanners: Vec<Scanner>,
    sel: usize,
    busy: Option<Receiver<anyhow::Result<Vec<Candidate>>>>,
    review: Option<Review>,
    error: Option<String>,
}

struct Review {
    scanner: Scanner,
    rows: Vec<Row>,
    show_existing: bool,
}

struct Row {
    c: Candidate,
    on: bool,
    name: String,
    exe: usize,
}

pub enum ScanOut {
    Open,
    /// Closed; save the scanners.
    Close,
    /// Add these (name, exe) with the scanner's platform/tags; the dialog closes.
    Import(Scanner, Vec<(String, PathBuf)>),
}

impl ScanDialog {
    pub fn new(mut scanners: Vec<Scanner>) -> Self {
        if scanners.is_empty() {
            scanners.push(Scanner::default());
        }
        ScanDialog { scanners, sel: 0, busy: None, review: None, error: None }
    }

    pub fn show(&mut self, ctx: &egui::Context, known: &HashSet<String>) -> ScanOut {
        if let Some(rx) = &self.busy {
            if let Ok(r) = rx.try_recv() {
                self.busy = None;
                match r {
                    Ok(found) => {
                        let rows = found
                            .into_iter()
                            .map(|c| Row { on: !c.existing, name: c.name.clone(), exe: 0, c })
                            .collect();
                        self.review = Some(Review { scanner: self.scanners[self.sel].clone(), rows, show_existing: false });
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let mut out = ScanOut::Open;
        let mut open = true;
        let screen = ctx.content_rect();
        let size = vec2(860.0_f32.min(screen.width() - 80.0), 620.0_f32.min(screen.height() - 100.0));
        theme::dialog(ctx, "Scanners", false, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            match self.review.as_mut() {
                Some(r) => {
                    dialog_header(ui, "Review found games", &mut open);
                    match review(ui, r) {
                        Some(true) => {
                            let picked = r.rows.iter().filter(|w| w.on && !w.c.existing && !w.name.trim().is_empty()).map(|w| (w.name.trim().to_string(), w.c.exes[w.exe].clone())).collect();
                            out = ScanOut::Import(r.scanner.clone(), picked);
                        }
                        Some(false) => self.review = None,
                        None => {}
                    }
                }
                None => {
                    dialog_header(ui, "Scan folders for games", &mut open);
                    if self.editor(ui, known) {
                        out = ScanOut::Close;
                    }
                }
            }
        });
        if !open {
            out = ScanOut::Close;
        }
        out
    }

    /// Scanner list + the selected one's settings. True = Close.
    fn editor(&mut self, ui: &mut Ui, known: &HashSet<String>) -> bool {
        let mut close = false;
        let body_h = ui.available_height() - 52.0;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(190.0);
                ui.set_min_height(body_h);
                for (i, s) in self.scanners.iter().enumerate() {
                    let label = if s.name.trim().is_empty() { "(unnamed)" } else { s.name.as_str() };
                    if ui.selectable_label(self.sel == i, label).clicked() {
                        self.sel = i;
                        self.error = None;
                    }
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("+ New").clicked() {
                        self.scanners.push(Scanner { name: format!("Scanner {}", self.scanners.len() + 1), ..Default::default() });
                        self.sel = self.scanners.len() - 1;
                    }
                    if self.scanners.len() > 1 && ui.button("Delete").clicked() {
                        self.scanners.remove(self.sel);
                        self.sel = self.sel.min(self.scanners.len() - 1);
                    }
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                let s = &mut self.scanners[self.sel];
                let w = 420.0;
                egui::Grid::new("scanner_fields").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut s.name).desired_width(w));
                    ui.end_row();
                    ui.label("Folder");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut s.dir).hint_text("D:\\Games").desired_width(w - 90.0));
                        if ui.button("Browse…").clicked() {
                            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                s.dir = p.display().to_string();
                            }
                        }
                    });
                    ui.end_row();
                    ui.label("File types");
                    ui.add(egui::TextEdit::singleline(&mut s.extensions).hint_text("exe").desired_width(160.0));
                    ui.end_row();
                    ui.label("Depth");
                    ui.add(egui::DragValue::new(&mut s.depth).range(0..=8).suffix(" folders deep"));
                    ui.end_row();
                    ui.label("Platform");
                    ui.add(egui::TextEdit::singleline(&mut s.platform).hint_text("PC (Windows)").desired_width(w));
                    ui.end_row();
                    ui.label("Tags");
                    ui.add(egui::TextEdit::singleline(&mut s.tags).hint_text("comma separated, optional").desired_width(w));
                    ui.end_row();
                    ui.label("Exclude");
                    ui.add(
                        egui::TextEdit::multiline(&mut s.exclude)
                            .hint_text("one per line: a folder or file name, or a path\n*Demo*\nTools\\Mods")
                            .desired_rows(4)
                            .desired_width(w),
                    );
                    ui.end_row();
                });
                ui.label(theme::hint(
                    "Each folder inside counts as one game; its main file is guessed (installers, redistributables and \
                     crash reporters are skipped). You review everything before it's added.",
                ));
                if let Some(e) = &self.error {
                    ui.colored_label(theme::DANGER, e);
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            let busy = self.busy.is_some();
            let s = self.scanners[self.sel].clone();
            let label = if busy { "Scanning…" } else { "Scan" };
            let scan = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(ACCENT);
            if ui.add_enabled(!busy && !s.dir.trim().is_empty(), scan).clicked() {
                self.error = None;
                let (tx, rx) = crossbeam_channel::bounded(1);
                let (known, ctx) = (known.clone(), ui.ctx().clone());
                std::thread::spawn(move || {
                    let _ = tx.send(scanner::scan(&s, &known));
                    ctx.request_repaint();
                });
                self.busy = Some(rx);
            }
            if busy {
                ui.spinner();
            }
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        close
    }
}

/// The found-games table. Some(true) = add the ticked ones, Some(false) = back.
fn review(ui: &mut Ui, r: &mut Review) -> Option<bool> {
    let mut res = None;
    let existing = r.rows.iter().filter(|w| w.c.existing).count();
    let new = r.rows.len() - existing;
    ui.label(RichText::new(format!("{new} new in {}", r.scanner.dir)).color(TEXT_DIM));
    ui.horizontal(|ui| {
        if ui.button("Select all").clicked() {
            r.rows.iter_mut().filter(|w| !w.c.existing).for_each(|w| w.on = true);
        }
        if ui.button("Select none").clicked() {
            r.rows.iter_mut().for_each(|w| w.on = false);
        }
        if existing > 0 {
            ui.checkbox(&mut r.show_existing, format!("Show {existing} already in the library"));
        }
    });
    ui.add_space(4.0);
    let body_h = ui.available_height() - 52.0;
    egui::ScrollArea::vertical().max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
        if new == 0 && !r.show_existing {
            ui.label(theme::hint("Nothing new here. Everything found is already in the library."));
        }
        egui::Grid::new("scan_rows").num_columns(3).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
            for (i, w) in r.rows.iter_mut().enumerate() {
                if w.c.existing && !r.show_existing {
                    continue;
                }
                ui.add_enabled(!w.c.existing, egui::Checkbox::without_text(&mut w.on));
                ui.add_enabled(!w.c.existing, egui::TextEdit::singleline(&mut w.name).desired_width(300.0));
                ui.horizontal(|ui| {
                    if w.c.existing {
                        ui.label(theme::hint("In library"));
                    }
                    let rel = |p: &PathBuf| p.strip_prefix(&w.c.folder).ok().filter(|r| !r.as_os_str().is_empty()).unwrap_or(p).display().to_string();
                    if w.c.exes.len() > 1 && !w.c.existing {
                        egui::ComboBox::from_id_salt(("scan_exe", i))
                            .width(360.0)
                            .selected_text(rel(&w.c.exes[w.exe]))
                            .show_ui(ui, |ui| {
                                for (k, e) in w.c.exes.iter().enumerate() {
                                    ui.selectable_value(&mut w.exe, k, rel(e));
                                }
                            });
                    } else {
                        ui.label(RichText::new(rel(&w.c.exes[w.exe])).color(TEXT_DIM));
                    }
                });
                ui.end_row();
            }
        });
    });
    ui.separator();
    ui.horizontal(|ui| {
        let n = r.rows.iter().filter(|w| w.on && !w.c.existing && !w.name.trim().is_empty()).count();
        let add = egui::Button::new(RichText::new(format!("Add {n} games")).color(Color32::WHITE)).fill(ACCENT);
        if ui.add_enabled(n > 0, add).clicked() {
            res = Some(true);
        }
        if ui.button("Back").clicked() {
            res = Some(false);
        }
        ui.label(theme::hint("Metadata and covers are fetched after adding; fix any misses with Edit → Fill from…"));
    });
    res
}
