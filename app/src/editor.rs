//! Add/Edit game dialog: tabs for details, launch settings and art.
use crate::picker::ArtKind;
use crate::theme::{self, ACCENT, CARD, CARD_HI, TEXT, TEXT_DIM};
use crate::util::{self, opt};
use egui::{vec2, Color32, CornerRadius, RichText, Stroke, Ui};
use crossbeam_channel::{unbounded, Receiver};
use gamut_core::igdb::{self, IgdbGame};
use gamut_core::{Config, Game, Source};

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
    specialk: Option<bool>,
    reshade: Option<bool>,
    auto_hdr: bool,
    /// The game as opened (for ReShade's folder); and where Gamut installed ReShade, once looked up.
    orig: Game,
    rs_dir: Option<Option<std::path::PathBuf>>,
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
    /// IGDB entry picked with "Fill from IGDB", saved as the game's match.
    pub igdb_pick: Option<i64>,
    igdb: Option<IgdbSearch>,
    filled: Option<String>,
}

/// The "Fill from IGDB" panel: a name search and its results.
struct IgdbSearch {
    query: String,
    results: Vec<IgdbGame>,
    busy: bool,
    error: Option<String>,
    rx: Option<Receiver<Result<Vec<IgdbGame>, String>>>,
}

/// Suggestion lists taken from the library (value, count).
pub struct Lists<'a> {
    pub platforms: &'a [(String, usize)],
    pub tags: &'a [(String, usize)],
    pub genres: &'a [(String, usize)],
    pub completions: &'a [(String, usize)],
    /// Set when IGDB credentials are configured.
    pub igdb: Option<&'a Config>,
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
            specialk: g.specialk,
            reshade: g.reshade,
            auto_hdr: g.auto_hdr,
            orig: g.clone(),
            rs_dir: None,
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
            igdb_pick: None,
            igdb: None,
            filled: None,
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
        g.specialk = self.specialk;
        g.reshade = self.reshade;
        g.auto_hdr = self.auto_hdr;
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
        theme::dialog(ctx, "editor", false, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
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
        self.igdb_panel(ui, lists.igdb);
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

    /// "Fill from IGDB…": search by name, pick an entry, and its details replace the form's fields.
    fn igdb_panel(&mut self, ui: &mut Ui, cfg: Option<&Config>) {
        if let Some(s) = self.igdb.as_mut() {
            if let Some(r) = s.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                s.busy = false;
                s.rx = None;
                match r {
                    Ok(v) => (s.results, s.error) = (v, None),
                    Err(e) => (s.results, s.error) = (Vec::new(), Some(e)),
                }
            }
        }
        let Some(cfg) = cfg else {
            ui.add_enabled(false, egui::Button::new("Fill from IGDB…"))
                .on_disabled_hover_text("Add IGDB credentials in Settings to look games up there.");
            ui.add_space(8.0);
            return;
        };
        let mut search = false;
        match self.igdb.as_mut() {
            None => {
                ui.horizontal(|ui| {
                    if ui.button("Fill from IGDB…").on_hover_text("Look this game up on IGDB and fill in its details").clicked() {
                        self.igdb = Some(IgdbSearch { query: self.name.clone(), results: Vec::new(), busy: false, error: None, rx: None });
                        search = true;
                    }
                    if let Some(f) = &self.filled {
                        ui.label(RichText::new(f).color(TEXT_DIM));
                    }
                });
            }
            Some(s) => {
                let mut picked = None;
                let mut close = false;
                egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let r = ui.add(egui::TextEdit::singleline(&mut s.query).desired_width(360.0).hint_text("Game name"));
                        if ui.button("Search").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            search = true;
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                        if s.busy {
                            ui.add(egui::Spinner::new().size(14.0));
                        }
                    });
                    if let Some(e) = &s.error {
                        ui.label(RichText::new(e).color(theme::DANGER));
                    } else if !s.busy && s.results.is_empty() {
                        ui.label(RichText::new("Nothing found. Try a shorter or different name.").color(TEXT_DIM));
                    }
                    egui::ScrollArea::vertical().id_salt("ed_igdb").max_height(220.0).show(ui, |ui| {
                        for (i, g) in s.results.iter().enumerate() {
                            let year = g.year.map(|y| format!(" ({y})")).unwrap_or_default();
                            let plat = g.platforms.join(", ");
                            let text = format!("{}{year}{}{plat}", g.name, if plat.is_empty() { "" } else { " · " });
                            if ui.selectable_label(false, text).on_hover_text("Fill the form with this entry").clicked() {
                                picked = Some(i);
                            }
                        }
                    });
                });
                if let Some(i) = picked {
                    let g = s.results[i].clone();
                    self.fill_from(&g);
                    close = true;
                }
                if close {
                    self.igdb = None;
                }
            }
        }
        if let (true, Some(s)) = (search, self.igdb.as_mut()) {
            let (tx, rx) = unbounded();
            let (cfg, q, ctx) = (cfg.clone(), s.query.trim().to_string(), ui.ctx().clone());
            (s.busy, s.error, s.rx) = (true, None, Some(rx));
            std::thread::spawn(move || {
                let _ = tx.send(igdb::search(&cfg, &q).map_err(|e| format!("{e:#}")));
                ctx.request_repaint();
            });
        }
        ui.add_space(8.0);
    }

    /// Replaces the form's details with an IGDB entry's (fields IGDB lacks stay as they are).
    fn fill_from(&mut self, g: &IgdbGame) {
        let m = &g.meta;
        let set = |field: &mut String, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                *field = v.to_string();
            }
        };
        if self.name.trim().is_empty() {
            self.name = g.name.clone();
        }
        set(&mut self.description, &m.description);
        set(&mut self.developer, &m.developer);
        set(&mut self.publisher, &m.publisher);
        set(&mut self.series, &m.series);
        if let Some(d) = &m.release_date {
            self.release = util::release(d);
        }
        if let Some(c) = m.critic_score {
            self.critic = c.to_string();
        }
        if let Some(c) = m.community_score {
            self.community = c.to_string();
        }
        if m.genres.is_some() {
            self.genres = list(&m.genres);
        }
        self.igdb_pick = Some(g.id);
        self.filled = Some(format!("Filled from IGDB: {}. Review and Save.", g.name));
    }

    fn launch(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        let w = (ui.available_width() - 160.0).min(700.0);
        egui::Grid::new("ed_launch").num_columns(2).spacing([14.0, 10.0]).min_col_width(110.0).show(ui, |ui| {
            row(ui, "Special K", |ui| {
                const OPTS: [(Option<bool>, &str); 3] =
                    [(None, "Use the global setting"), (Some(true), "Always inject"), (Some(false), "Never inject")];
                egui::ComboBox::from_id_salt("ed_sk")
                    .width(220.0)
                    .selected_text(OPTS.iter().find(|o| o.0 == self.specialk).map_or("", |o| o.1))
                    .show_ui(ui, |ui| {
                        for (v, l) in OPTS {
                            ui.selectable_value(&mut self.specialk, v, l);
                        }
                    });
            });
            row(ui, "ReShade", |ui| {
                const OPTS: [(Option<bool>, &str); 3] =
                    [(None, "Use the global setting"), (Some(true), "Install on first start"), (Some(false), "Never install")];
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("ed_rs")
                        .width(220.0)
                        .selected_text(OPTS.iter().find(|o| o.0 == self.reshade).map_or("", |o| o.1))
                        .show_ui(ui, |ui| {
                            for (v, l) in OPTS {
                                ui.selectable_value(&mut self.reshade, v, l);
                            }
                        });
                    let dir = self.rs_dir.get_or_insert_with(|| gamut_core::reshade::installed_dir(&self.orig)).clone();
                    if let Some(d) = dir {
                        if ui.button("Remove ReShade").on_hover_text(format!("Deletes the ReShade files Gamut added in {}", d.display())).clicked() {
                            match gamut_core::reshade::uninstall(&self.orig) {
                                Ok(_) => self.rs_dir = Some(None),
                                Err(e) => log::warn!("removing ReShade failed: {e:#}"),
                            }
                        }
                    }
                });
            });
            row(ui, "HDR", |ui| {
                ui.checkbox(&mut self.auto_hdr, "Turn Windows HDR on while this game runs");
            });
            if self.source == Source::Steam {
                return;
            }
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
        if self.source == Source::Steam {
            ui.add_space(6.0);
            ui.label(theme::hint("Steam games start through Steam; executable and arguments are set there."));
        }
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

/// One removable chip; long text is cut with "…" (full text on hover). True when ✕ was clicked.
fn chip(ui: &mut Ui, text: &str, id: egui::Id) -> bool {
    let mut job = egui::text::LayoutJob::simple_singleline(text.into(), egui::FontId::proportional(13.0), TEXT);
    job.wrap = egui::text::TextWrapping::truncate_at_width(220.0);
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    let (h, x) = (24.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(10.0 + galley.size().x + 6.0 + x + 3.0, h), egui::Sense::hover());
    ui.painter().rect(rect, CornerRadius::same(12), CARD, Stroke::new(1.0, CARD_HI), egui::StrokeKind::Inside);
    ui.painter().galley(egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
    if cut {
        resp.on_hover_text(text);
    }
    let xr = egui::Rect::from_center_size(egui::pos2(rect.right() - 3.0 - x / 2.0, rect.center().y), vec2(x, x));
    theme::x_at(ui, xr, id).on_hover_text("Remove").clicked()
}

/// Removable chips plus an input that adds on Enter/comma, with suggestions from the library.
fn chips(ui: &mut Ui, id: &str, items: &mut Vec<String>, input: &mut String, known: &[(String, usize)], hint: &str) {
    use egui::containers::{Popup, PopupCloseBehavior, SetOpenCommand};
    let mut remove = None;
    let mut add: Option<String> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for (i, t) in items.iter().enumerate() {
            if chip(ui, t, ui.make_persistent_id((id, i))) {
                remove = Some(i);
            }
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
