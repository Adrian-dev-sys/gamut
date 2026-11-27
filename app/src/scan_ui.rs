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
    /// The scanner being run (index and settings bound at click) and its result.
    busy: Option<(usize, Scanner, Receiver<anyhow::Result<Vec<Candidate>>>)>,
    tag_input: String,
    review: Option<Review>,
    error: Option<String>,
}

struct Review {
    idx: usize,
    scanner: Scanner,
    rows: Vec<Row>,
    show_existing: bool,
    show_ignored: bool,
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
    /// Name, exe, tags per picked game.
    Import(Scanner, Vec<(String, PathBuf, String)>),
}

impl ScanDialog {
    pub fn new(mut scanners: Vec<Scanner>) -> Self {
        if scanners.is_empty() {
            scanners.push(Scanner::default());
        }
        ScanDialog { scanners, sel: 0, busy: None, tag_input: String::new(), review: None, error: None }
    }

    /// `known`: lowercased exe/install paths already in the library, built only when a scan starts.
    /// `platforms`/`tags`: the library's, for the pickers.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        known: impl FnOnce() -> HashSet<String>,
        platforms: &[(String, usize)],
        tags: &[(String, usize)],
    ) -> ScanOut {
        if let Some((idx, scanner, rx)) = &self.busy {
            if let Ok(r) = rx.try_recv() {
                let (idx, scanner) = (*idx, scanner.clone());
                self.busy = None;
                match r {
                    Ok(found) => {
                        let rows = found
                            .into_iter()
                            .map(|c| Row { on: !c.existing && !c.ignored, name: c.name.clone(), exe: 0, c })
                            .collect();
                        self.review = Some(Review { idx, scanner, rows, show_existing: false, show_ignored: false });
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
                            let picked = r.rows.iter().filter(|w| w.on && !w.c.existing && !w.name.trim().is_empty()).map(|w| (w.name.trim().to_string(), w.c.exes[w.exe].clone(), w.c.tags.clone())).collect();
                            // Unticked new games aren't offered again; ticked ones leave the list.
                            if let Some(s) = self.scanners.get_mut(r.idx) {
                                for w in r.rows.iter().filter(|w| !w.c.existing) {
                                    let k = scanner::key(&w.c.folder);
                                    s.ignored.retain(|i| *i != k);
                                    if !w.on {
                                        s.ignored.push(k);
                                    }
                                }
                                s.ignored.sort();
                            }
                            out = ScanOut::Import(r.scanner.clone(), picked);
                        }
                        Some(false) => self.review = None,
                        None => {}
                    }
                }
                None => {
                    dialog_header(ui, "Scan folders for games", &mut open);
                    if self.editor(ui, known, platforms, tags) {
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
    fn editor(&mut self, ui: &mut Ui, known: impl FnOnce() -> HashSet<String>, platforms: &[(String, usize)], tags: &[(String, usize)]) -> bool {
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
                    ui.label("Platform");
                    platform_pick(ui, &mut s.platform, platforms, w);
                    ui.end_row();
                    ui.label("Tags");
                    ui.vertical(|ui| {
                        ui.set_max_width(w);
                        let mut list: Vec<String> = s.tags.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
                        crate::editor::chips(ui, "scan_tags", &mut list, &mut self.tag_input, tags, "Add tag…");
                        s.tags = list.join(", ");
                    });
                    ui.end_row();
                    ui.label("Tag rules").on_hover_text("Extra tags for games whose folder name contains a word.");
                    ui.add(
                        egui::TextEdit::multiline(&mut s.tag_rules)
                            .hint_text("one per line: word in the folder name = tags\nrecomp = Recompilation\npc port = PC Port")
                            .desired_rows(3)
                            .desired_width(w),
                    );
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
                    "Each folder inside counts as one game, however deep its files are; its main file is guessed \
                     (installers, redistributables and crash reporters are skipped). You review everything before it's added.",
                ));
                let nested = scanner::nested_dirs(&self.scanners[self.sel].dir.clone(), &self.scanners);
                if !nested.is_empty() {
                    let names: Vec<&str> = self.scanners.iter().filter(|o| nested.contains(&scanner::key(std::path::Path::new(o.dir.trim())).trim_end_matches('\\').to_string())).map(|o| o.name.as_str()).collect();
                    ui.label(theme::hint(&format!("Skips folders with their own scanner: {}.", names.join(", "))));
                }
                if let Some(e) = &self.error {
                    ui.colored_label(theme::DANGER, e);
                }
            });
        });
        ui.separator();
        ui.horizontal(|ui| {
            let busy = self.busy.is_some();
            let (sel, mut s) = (self.sel, self.scanners[self.sel].clone());
            s.nested = scanner::nested_dirs(&s.dir, &self.scanners);
            let label = if busy { "Scanning…" } else { "Scan" };
            let scan = egui::Button::new(RichText::new(label).color(Color32::WHITE)).fill(ACCENT);
            if ui.add_enabled(!busy && !s.dir.trim().is_empty(), scan).clicked() {
                self.error = None;
                let (tx, rx) = crossbeam_channel::bounded(1);
                let (known, ctx, bound) = (known(), ui.ctx().clone(), s.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(scanner::scan(&s, &known));
                    ctx.request_repaint();
                });
                self.busy = Some((sel, bound, rx));
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

/// Platform dropdown: none, the library's platforms, and PC (Windows).
fn platform_pick(ui: &mut Ui, v: &mut String, platforms: &[(String, usize)], w: f32) {
    let mut all: Vec<&str> = vec!["PC (Windows)"];
    all.extend(platforms.iter().map(|(p, _)| p.as_str()).filter(|p| *p != "PC (Windows)"));
    egui::ComboBox::from_id_salt("scan_platform")
        .width(w)
        .selected_text(if v.is_empty() { "— (none)" } else { v.as_str() })
        .show_ui(ui, |ui| {
            ui.selectable_value(v, String::new(), "— (none)");
            for p in all {
                ui.selectable_value(v, p.to_string(), p);
            }
        });
}

/// The found-games table. Some(true) = add the ticked ones, Some(false) = back.
fn review(ui: &mut Ui, r: &mut Review) -> Option<bool> {
    let mut res = None;
    let existing = r.rows.iter().filter(|w| w.c.existing).count();
    let ignored = r.rows.iter().filter(|w| w.c.ignored && !w.c.existing).count();
    let new = r.rows.len() - existing - ignored;
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
        if ignored > 0 {
            ui.checkbox(&mut r.show_ignored, format!("Show {ignored} skipped before"))
                .on_hover_text("Games you left unticked last time. Tick one to add it after all.");
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
                if w.c.existing && !r.show_existing || w.c.ignored && !w.c.existing && !r.show_ignored {
                    continue;
                }
                ui.add_enabled(!w.c.existing, egui::Checkbox::without_text(&mut w.on));
                ui.add_enabled_ui(!w.c.existing, |ui| ui.add_sized([300.0, 22.0], egui::TextEdit::singleline(&mut w.name)));
                ui.horizontal(|ui| {
                    if w.c.existing {
                        ui.label(theme::hint("In library"));
                    } else if w.c.ignored {
                        ui.label(theme::hint("Skipped before"));
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
                    if !w.c.tags.is_empty() {
                        ui.label(theme::hint(&w.c.tags));
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
        ui.label(theme::hint("Unticked games won't be offered again. Details and covers are fetched after adding."));
    });
    res
}
