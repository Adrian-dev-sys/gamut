//! Add/Edit game dialog: tabs for details, launch settings and art.
use crate::picker::ArtKind;
use crate::theme::{self, ACCENT, CARD, CARD_HI, TEXT_DIM};
use crate::util::{self, opt};
use egui::{vec2, Color32, CornerRadius, RichText, Stroke, Ui};
use gamut_core::{Game, Source};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    General,
    Launch,
    Media,
}

/// Completion statuses offered besides the ones already in the library.
const STATUSES: [&str; 8] = ["Not Played", "Plan to Play", "Playing", "Played", "Beaten", "Completed", "On Hold", "Abandoned"];

pub struct Editor {
    pub id: i64,
    pub source: Source,
    tab: Tab,
    pub name: String,
    pub exe: String,
    pub args: String,
    pub work_dir: String,
    pub description: String,
    developer: String,
    publisher: String,
    release: String,
    series: String,
    platform: String,
    store: String,
    completion: String,
    critic: String,
    community: String,
    genres: Vec<String>,
    tags: Vec<String>,
    genre_input: String,
    tag_input: String,
    pub cover_file: Option<std::path::PathBuf>,
}

/// Suggestion lists taken from the library (value, count).
pub struct Lists<'a> {
    pub platforms: &'a [(String, usize)],
    pub tags: &'a [(String, usize)],
    pub genres: &'a [(String, usize)],
    pub completions: &'a [(String, usize)],
}

pub enum EditorOutcome {
    Open,
    Cancel,
    Save,
    PickArt(i64, ArtKind),
}

fn list(v: &Option<String>) -> Vec<String> {
    v.as_deref().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

impl Editor {
    pub fn new_custom() -> Self {
        Self::from_game(&Game::new_custom(String::new(), None))
    }

    pub fn from_game(g: &Game) -> Self {
        let s = |v: &Option<String>| v.clone().unwrap_or_default();
        let n = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_default();
        Editor {
            id: g.id,
            source: g.source,
            tab: Tab::General,
            name: g.name.clone(),
            exe: s(&g.exe),
            args: s(&g.args),
            work_dir: s(&g.work_dir),
            description: s(&g.description),
            developer: s(&g.developer),
            publisher: s(&g.publisher),
            release: g.release_date.as_deref().map(util::release).unwrap_or_default(),
            series: s(&g.series),
            platform: s(&g.platform),
            store: s(&g.store),
            completion: s(&g.completion),
            critic: n(g.critic_score),
            community: n(g.community_score),
            genres: list(&g.genres),
            tags: list(&g.tags),
            genre_input: String::new(),
            tag_input: String::new(),
            cover_file: None,
        }
    }

    /// Copies the edited values into `g` (empty fields become NULL).
    pub fn apply_to(&self, g: &mut Game) {
        let score = |s: &str| s.trim().parse::<i64>().ok().map(|v| v.clamp(0, 100));
        let join = |v: &[String]| (!v.is_empty()).then(|| v.join(", "));
        g.name = self.name.trim().into();
        g.exe = opt(&self.exe);
        g.args = opt(&self.args);
        g.work_dir = opt(&self.work_dir);
        g.description = opt(&self.description);
        g.developer = opt(&self.developer);
        g.publisher = opt(&self.publisher);
        g.release_date = opt(&util::release_input(&self.release));
        g.series = opt(&self.series);
        g.platform = opt(&self.platform);
        g.store = opt(&self.store);
        g.completion = opt(&self.completion);
        g.critic_score = score(&self.critic);
        g.community_score = score(&self.community);
        g.genres = join(&self.genres);
        g.tags = join(&self.tags);
    }

    pub fn show(&mut self, ctx: &egui::Context, lists: &Lists) -> EditorOutcome {
        let screen = ctx.content_rect();
        let size = vec2((screen.width() * 0.8).min(1150.0), (screen.height() * 0.86).min(900.0));
        let mut open = true;
        let mut out = EditorOutcome::Open;
        let title = if self.id == 0 { "Add game" } else { "Edit game" };
        egui::Window::new(title)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .fixed_size(size)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                crate::ui::dialog_header(ui, title, &mut open);
                ui.horizontal(|ui| {
                    for (t, label) in [(Tab::General, "General"), (Tab::Launch, "Launch"), (Tab::Media, "Media")] {
                        ui.selectable_value(&mut self.tab, t, RichText::new(label).size(14.5));
                    }
                });
                ui.separator();
                // Body takes everything above the footer.
                let body_h = ui.available_height() - 48.0;
                egui::ScrollArea::vertical().id_salt("ed_body").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                    match self.tab {
                        Tab::General => self.general(ui, lists, body_h),
                        Tab::Launch => self.launch(ui),
                        Tab::Media => {
                            if let Some(o) = self.media(ui) {
                                out = o;
                            }
                        }
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            open = false;
                        }
                        let ok = !self.name.trim().is_empty();
                        let save = egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(ACCENT).min_size(vec2(90.0, 0.0));
                        if ui.add_enabled(ok, save).on_disabled_hover_text("A name is required").clicked() {
                            out = EditorOutcome::Save;
                        }
                    });
                });
            });
        if !open {
            EditorOutcome::Cancel
        } else {
            out
        }
    }

    fn general(&mut self, ui: &mut Ui, lists: &Lists, body_h: f32) {
        let col_w = (ui.available_width() - 40.0) / 2.0;
        let field_w = col_w - 130.0;
        ui.add_space(4.0);
        ui.columns(2, |cols| {
            let grid = |ui: &mut Ui, id: &str, f: &mut dyn FnMut(&mut Ui)| {
                egui::Grid::new(id).num_columns(2).spacing([14.0, 10.0]).min_col_width(110.0).show(ui, |ui| f(ui));
            };
            grid(&mut cols[0], "ed_left", &mut |ui| {
                row(ui, "Name", |ui| text(ui, &mut self.name, field_w, ""));
                row(ui, "Platform", |ui| suggest(ui, "ed_platform", &mut self.platform, lists.platforms, field_w, "PC (Windows)"));
                row(ui, "Store", |ui| text(ui, &mut self.store, field_w, "e.g. GOG, PlayStation"));
                row(ui, "Developer", |ui| text(ui, &mut self.developer, field_w, ""));
                row(ui, "Publisher", |ui| text(ui, &mut self.publisher, field_w, ""));
            });
            grid(&mut cols[1], "ed_right", &mut |ui| {
                row(ui, "Release date", |ui| text(ui, &mut self.release, field_w, "DD/MM/YYYY"));
                row(ui, "Series", |ui| text(ui, &mut self.series, field_w, ""));
                row(ui, "Completion", |ui| completion(ui, &mut self.completion, lists.completions, field_w));
                row(ui, "Critic score", |ui| text(ui, &mut self.critic, 60.0, "0–100"));
                row(ui, "Community score", |ui| text(ui, &mut self.community, 60.0, "0–100"));
            });
        });
        ui.add_space(10.0);
        theme::section(ui, "Genres");
        chips(ui, "ed_genres", &mut self.genres, &mut self.genre_input, lists.genres, "Add genre…");
        ui.add_space(6.0);
        theme::section(ui, "Tags");
        chips(ui, "ed_tags", &mut self.tags, &mut self.tag_input, lists.tags, "Add tag…");
        ui.add_space(6.0);
        theme::section(ui, "Description");
        let h = (body_h - (ui.cursor().top() - ui.min_rect().top()) - 12.0).max(220.0);
        egui::ScrollArea::vertical().id_salt("ed_desc").min_scrolled_height(h).max_height(h).show(ui, |ui| {
            ui.add_sized(
                vec2(ui.available_width(), h),
                egui::TextEdit::multiline(&mut self.description).hint_text("No description"),
            );
        });
    }

    fn launch(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        if self.source == Source::Steam {
            ui.label(theme::hint("Steam games start through Steam; there is nothing to set here."));
            return;
        }
        let w = (ui.available_width() - 160.0).min(700.0);
        egui::Grid::new("ed_launch").num_columns(2).spacing([14.0, 10.0]).min_col_width(110.0).show(ui, |ui| {
            row(ui, "Executable", |ui| {
                ui.horizontal(|ui| {
                    text(ui, &mut self.exe, w - 90.0, "exe, shortcut or URL");
                    if ui.button("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new()
                            .add_filter("Programs", &["exe", "bat", "cmd", "lnk", "url"])
                            .add_filter("All files", &["*"])
                            .pick_file()
                        {
                            if self.name.trim().is_empty() {
                                if let Some(stem) = p.file_stem() {
                                    self.name = stem.to_string_lossy().into();
                                }
                            }
                            self.exe = p.to_string_lossy().into();
                        }
                    }
                });
            });
            row(ui, "Arguments", |ui| text(ui, &mut self.args, w, ""));
            row(ui, "Working folder", |ui| text(ui, &mut self.work_dir, w, "defaults to the exe's folder"));
        });
    }

    fn media(&mut self, ui: &mut Ui) -> Option<EditorOutcome> {
        ui.add_space(4.0);
        let mut out = None;
        if self.id == 0 {
            ui.label(theme::hint("Pick a cover file now, or search for art after saving."));
            ui.horizontal(|ui| {
                if ui.button("Choose cover file…").clicked() {
                    self.cover_file = rfd::FileDialog::new().add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file();
                }
                let t = self.cover_file.as_ref().map_or("none".into(), |p| p.file_name().unwrap_or_default().to_string_lossy().to_string());
                ui.label(RichText::new(t).color(TEXT_DIM));
            });
            return None;
        }
        ui.label(theme::hint("Search SteamGridDB, VNDB and Steam, or use a file from your PC."));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Change cover…").clicked() {
                out = Some(EditorOutcome::PickArt(self.id, ArtKind::Cover));
            }
            if ui.button("Change banner…").clicked() {
                out = Some(EditorOutcome::PickArt(self.id, ArtKind::Banner));
            }
        });
        out
    }
}

fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.label(RichText::new(label).color(TEXT_DIM));
    add(ui);
    ui.end_row();
}

fn text(ui: &mut Ui, v: &mut String, w: f32, hint: &str) {
    ui.add(egui::TextEdit::singleline(v).desired_width(w).hint_text(hint));
}

/// Text field with a dropdown of known values that match what's typed.
fn suggest(ui: &mut Ui, id: &str, v: &mut String, items: &[(String, usize)], w: f32, hint: &str) {
    use egui::containers::{Popup, PopupCloseBehavior, SetOpenCommand};
    let r = ui.add(egui::TextEdit::singleline(v).desired_width(w).hint_text(hint));
    let q = v.trim().to_lowercase();
    let hits: Vec<&String> = items.iter().map(|(s, _)| s).filter(|s| s.to_lowercase().contains(&q) && **s != *v).take(30).collect();
    let open = (r.gained_focus() || r.changed()).then_some(SetOpenCommand::Bool(true));
    let pid = ui.make_persistent_id(id).with("pop");
    let mut pick = None;
    if !hits.is_empty() {
        Popup::from_response(&r)
            .id(pid)
            .open_memory(open)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(w)
            .show(|ui| {
                egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                    for h in &hits {
                        if ui.selectable_label(false, h.as_str()).clicked() {
                            pick = Some((*h).clone());
                        }
                    }
                });
            });
    }
    if let Some(p) = pick {
        *v = p;
        Popup::close_id(ui.ctx(), pid);
    }
}

fn completion(ui: &mut Ui, v: &mut String, known: &[(String, usize)], w: f32) {
    let mut all: Vec<String> = STATUSES.iter().map(|s| s.to_string()).collect();
    for (k, _) in known {
        if !all.contains(k) {
            all.push(k.clone());
        }
    }
    egui::ComboBox::from_id_salt("ed_completion")
        .width(w)
        .selected_text(if v.is_empty() { "—" } else { v.as_str() })
        .show_ui(ui, |ui| {
            ui.selectable_value(v, String::new(), "—");
            for s in all {
                let label = s.clone();
                ui.selectable_value(v, s, label);
            }
        });
}

/// Removable chips plus an input that adds on Enter/comma, with suggestions from the library.
fn chips(ui: &mut Ui, id: &str, items: &mut Vec<String>, input: &mut String, known: &[(String, usize)], hint: &str) {
    use egui::containers::{Popup, PopupCloseBehavior, SetOpenCommand};
    let mut remove = None;
    let mut add: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (i, t) in items.iter().enumerate() {
            egui::Frame::new()
                .fill(CARD)
                .stroke(Stroke::new(1.0, CARD_HI))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin { left: 10, right: 4, top: 2, bottom: 2 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.label(t);
                        let x = ui.add(egui::Button::new(RichText::new("✕").size(11.0).color(TEXT_DIM)).frame(false));
                        if x.on_hover_text("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                });
        }
        let r = ui.add(egui::TextEdit::singleline(input).desired_width(170.0).hint_text(hint));
        if input.contains(',') {
            add = Some(input.replace(',', ""));
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            add = Some(input.clone());
            r.request_focus();
        }
        let q = input.trim().to_lowercase();
        let hits: Vec<&String> = known
            .iter()
            .map(|(s, _)| s)
            .filter(|s| !q.is_empty() && s.to_lowercase().contains(&q) && !items.contains(s))
            .take(20)
            .collect();
        let pid = ui.make_persistent_id(id).with("pop");
        let open = r.changed().then_some(SetOpenCommand::Bool(true));
        if !hits.is_empty() {
            Popup::from_response(&r)
                .id(pid)
                .open_memory(open)
                .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
                .width(240.0)
                .show(|ui| {
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for h in &hits {
                            if ui.selectable_label(false, h.as_str()).clicked() {
                                add = Some((*h).clone());
                            }
                        }
                    });
                });
        }
        if add.is_some() {
            Popup::close_id(ui.ctx(), pid);
        }
    });
    if let Some(i) = remove {
        items.remove(i);
    }
    if let Some(a) = add {
        let a = a.trim().to_string();
        if !a.is_empty() && !items.iter().any(|t| t.eq_ignore_ascii_case(&a)) {
            items.push(a);
        }
        input.clear();
    }
}
