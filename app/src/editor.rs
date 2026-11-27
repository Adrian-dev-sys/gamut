//! Add/Edit game dialog: tabs for details, launch settings and art.
use crate::picker::ArtKind;
use crate::theme::{self, ACCENT, CARD, CARD_HI, TEXT, TEXT_DIM};
use crate::util::{self, opt};
use egui::{vec2, Color32, CornerRadius, RichText, Stroke, Ui};
use crossbeam_channel::{unbounded, Receiver};
use crate::lookup::{self, Compare, CompareOut, Hit, Src};
use gamut_core::igdb;
use gamut_core::vndb::VnDetails;
use gamut_core::model::{to_json, Link, PlayAction};
use gamut_core::{scrape, Config, Game, Source};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    General,
    Launch,
    Actions,
    Links,
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
    lookup: Option<Lookup>,
    compare: Option<Compare>,
    play_h: String,
    play_m: String,
    play_count: String,
    /// Cover and banner files, for the Media tab's previews.
    pub art_paths: Option<(std::path::PathBuf, std::path::PathBuf)>,
    art: Option<Art>,
    filled: Option<String>,
    sort_name: String,
    user_score: String,
    notes: String,
    links: Vec<Link>,
    actions: Vec<PlayAction>,
    script_before: String,
    script_after: String,
    install_dir: String,
    install_size: Option<i64>,
    size_rx: Option<Receiver<i64>>,
    save_dir: String,
    save_rx: Option<Receiver<Result<Option<std::path::PathBuf>, String>>>,
    save_msg: Option<String>,
    /// Icon file to copy in on save; Some(None) removes the icon.
    pub icon_file: Option<Option<std::path::PathBuf>>,
    /// The current icon, for the Media tab.
    pub icon_path: Option<std::path::PathBuf>,
    icon_tex: Option<Option<egui::TextureHandle>>,
    icon_rx: Option<Receiver<Option<egui::ColorImage>>>,
}

enum Msg {
    Results(Result<Vec<Hit>, String>),
    /// A picked Steam/GOG/VNDB entry's details (fetched after the pick).
    Details(Result<VnDetails, String>),
}

/// The "Fill from …" panel: a name search and its results.
struct Lookup {
    src: Src,
    query: String,
    results: Vec<Hit>,
    busy: bool,
    error: Option<String>,
    rx: Option<Receiver<Msg>>,
    /// The picked entry being previewed (and its IGDB id, saved as the match on fill).
    preview: Option<(VnDetails, Option<i64>)>,
}

/// Current cover/banner textures, decoded off the UI thread.
struct Art {
    rx: Receiver<(bool, Option<egui::ColorImage>)>,
    cover: Option<Option<egui::TextureHandle>>,
    hero: Option<Option<egui::TextureHandle>>,
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

/// What an entry would fill, before it touches the form. Some(true) = Fill, Some(false) = Back.
fn preview(ui: &mut Ui, d: &VnDetails, src: &str) -> Option<bool> {
    let m = &d.meta;
    let desc = m.description.as_deref().unwrap_or("");
    let images = desc.lines().filter(|l| l.trim_start().starts_with(scrape::IMG_MARKER)).count();
    let text = scrape::text_only(desc);
    let date = m.release_date.as_deref().map(util::release);
    let score = |v: Option<i64>| v.map(|v| v.to_string());
    let tags = (!d.tags.is_empty()).then(|| d.tags.join(", "));
    let rows: [(&str, Option<String>); 8] = [
        ("Developer", m.developer.clone()),
        ("Publisher", m.publisher.clone()),
        ("Release date", date),
        ("Genres", m.genres.clone()),
        ("Series", m.series.clone()),
        ("Critic score", score(m.critic_score)),
        ("Community score", score(m.community_score)),
        ("Tags (added)", tags),
    ];
    let have = rows.iter().filter(|r| r.1.is_some()).count() + usize::from(!text.is_empty());
    let mut out = None;
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&d.title).strong().size(15.0));
        ui.label(RichText::new(format!("· {src} has {have} of 9 fields")).color(TEXT_DIM));
    });
    ui.add_space(4.0);
    egui::Grid::new("ed_preview").num_columns(2).spacing([14.0, 6.0]).min_col_width(110.0).show(ui, |ui| {
        for (label, v) in rows {
            ui.label(RichText::new(label).color(TEXT_DIM));
            match v {
                Some(v) => ui.add(egui::Label::new(v).wrap()),
                None => ui.label(RichText::new("—").color(TEXT_DIM)),
            };
            ui.end_row();
        }
        ui.label(RichText::new("Description").color(TEXT_DIM));
        if text.is_empty() {
            ui.label(RichText::new("—").color(TEXT_DIM));
        } else {
            let pics = if images > 0 { format!(", {images} images") } else { String::new() };
            ui.label(RichText::new(format!("{} words{pics}", text.split_whitespace().count())).color(TEXT_DIM));
        }
        ui.end_row();
    });
    if !text.is_empty() {
        egui::ScrollArea::vertical().id_salt("ed_preview_desc").max_height(120.0).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(&text).color(TEXT)).wrap());
        });
    }
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        let fill = egui::Button::new(RichText::new("Fill form").color(Color32::WHITE)).fill(ACCENT);
        if ui.add(fill).on_hover_text("Replace the form's fields with these (nothing is saved until Save)").clicked() {
            out = Some(true);
        }
        if ui.button("Back to results").clicked() {
            out = Some(false);
        }
    });
    out
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
            lookup: None,
            compare: None,
            play_h: (g.playtime_min / 60).to_string(),
            play_m: (g.playtime_min % 60).to_string(),
            play_count: g.play_count.to_string(),
            art_paths: None,
            art: None,
            filled: None,
            sort_name: s(&g.sort_name),
            user_score: n(g.user_score),
            notes: s(&g.notes),
            links: g.links(),
            actions: g.actions(),
            script_before: s(&g.script_before),
            script_after: s(&g.script_after),
            install_dir: s(&g.install_dir),
            install_size: g.install_size,
            size_rx: None,
            save_dir: s(&g.save_dir),
            save_rx: None,
            save_msg: None,
            icon_file: None,
            icon_path: None,
            icon_tex: None,
            icon_rx: None,
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
        g.sort_name = opt(&self.sort_name).filter(|s| *s != g.name);
        g.user_score = score(&self.user_score);
        g.notes = opt(&self.notes);
        let links: Vec<Link> = self
            .links
            .iter()
            .filter(|l| !l.url.trim().is_empty())
            .map(|l| Link { name: if l.name.trim().is_empty() { l.url.trim().into() } else { l.name.trim().into() }, url: l.url.trim().into() })
            .collect();
        g.links = to_json(&links);
        let actions: Vec<PlayAction> = self
            .actions
            .iter()
            .filter(|a| !a.path.trim().is_empty())
            .map(|a| PlayAction { name: if a.name.trim().is_empty() { "Action".into() } else { a.name.trim().into() }, ..a.clone() })
            .collect();
        g.actions = to_json(&actions);
        g.script_before = opt(&self.script_before);
        g.script_after = opt(&self.script_after);
        g.install_dir = opt(&self.install_dir);
        g.install_size = self.install_size;
        g.save_dir = opt(&self.save_dir);
    }

    /// Edited (playtime minutes, play count), when they differ from the game as opened.
    /// Steam reports its own playtime, so only the count is kept for Steam games.
    pub fn play_stats(&self) -> Option<(i64, i64)> {
        let n = |s: &str| s.trim().parse::<i64>().ok().filter(|v| *v >= 0);
        let mins = match self.source {
            Source::Steam => self.orig.playtime_min,
            _ => match (n(&self.play_h), n(&self.play_m)) {
                (Some(h), Some(m)) => h * 60 + m,
                _ => self.orig.playtime_min,
            },
        };
        let count = n(&self.play_count).unwrap_or(self.orig.play_count);
        (mins != self.orig.playtime_min || count != self.orig.play_count).then_some((mins, count))
    }

    pub fn reload_art(&mut self) {
        self.art = None;
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
                    let tabs =
                        [(Tab::General, "General"), (Tab::Launch, "Launch"), (Tab::Actions, "Actions & scripts"), (Tab::Links, "Links & notes"), (Tab::Media, "Media")];
                    for (t, label) in tabs {
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
                        Tab::Actions => self.actions_tab(ui),
                        Tab::Links => self.links_tab(ui, body_h),
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
        self.lookup_panel(ui, lists.igdb);
        ui.columns(2, |cols| {
            let grid = |ui: &mut Ui, id: &str, f: &mut dyn FnMut(&mut Ui)| {
                egui::Grid::new(id).num_columns(2).spacing([14.0, 10.0]).min_col_width(110.0).show(ui, |ui| f(ui));
            };
            grid(&mut cols[0], "ed_left", &mut |ui| {
                row(ui, "Name", |ui| text(ui, &mut self.name, field_w, ""));
                row(ui, "Sorting name", |ui| text(ui, &mut self.sort_name, field_w, "sorts by the name when empty"));
                row(ui, "Platform", |ui| suggest(ui, "ed_platform", &mut self.platform, lists.platforms, field_w, "PC (Windows)"));
                row(ui, "Store", |ui| text(ui, &mut self.store, field_w, "e.g. GOG, PlayStation"));
                row(ui, "Developer", |ui| text(ui, &mut self.developer, field_w, ""));
                row(ui, "Publisher", |ui| text(ui, &mut self.publisher, field_w, ""));
                let steam = self.source == Source::Steam;
                row(ui, "Time played", |ui| {
                    ui.add_enabled_ui(!steam, |ui| {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut self.play_h).desired_width(50.0));
                            ui.label(RichText::new("h").color(TEXT_DIM));
                            ui.add(egui::TextEdit::singleline(&mut self.play_m).desired_width(36.0));
                            ui.label(RichText::new("m").color(TEXT_DIM));
                        });
                    })
                    .response
                    .on_disabled_hover_text("Steam keeps this game's playtime; Gamut takes it from Steam.");
                });
            });
            grid(&mut cols[1], "ed_right", &mut |ui| {
                row(ui, "Release date", |ui| text(ui, &mut self.release, field_w, "DD/MM/YYYY"));
                row(ui, "Series", |ui| text(ui, &mut self.series, field_w, ""));
                row(ui, "Completion", |ui| completion(ui, &mut self.completion, lists.completions, field_w));
                row(ui, "Critic score", |ui| text(ui, &mut self.critic, 60.0, "0–100"));
                row(ui, "Community score", |ui| text(ui, &mut self.community, 60.0, "0–100"));
                row(ui, "Your score", |ui| text(ui, &mut self.user_score, 60.0, "0–100"));
                row(ui, "Play count", |ui| text(ui, &mut self.play_count, 60.0, "0"));
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

    /// "Fill from Steam/GOG/IGDB/VNDB…": search by name, preview an entry, then fill the form with it.
    fn lookup_panel(&mut self, ui: &mut Ui, cfg: Option<&Config>) {
        if let Some(c) = self.compare.as_mut() {
            let mut out = None;
            egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                theme::raised_widgets(ui);
                out = c.show(ui, cfg);
            });
            match out {
                Some(CompareOut::Apply(p)) => {
                    self.apply_picked(p);
                    self.compare = None;
                }
                Some(CompareOut::Close) => self.compare = None,
                None => {}
            }
            ui.add_space(8.0);
            return;
        }
        if let Some(s) = self.lookup.as_mut() {
            if let Some(m) = s.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                s.busy = false;
                s.rx = None;
                match m {
                    Msg::Results(Ok(v)) => (s.results, s.error) = (v, None),
                    Msg::Results(Err(e)) => (s.results, s.error) = (Vec::new(), Some(e)),
                    Msg::Details(Ok(d)) => s.preview = Some((d, None)),
                    Msg::Details(Err(e)) => s.error = Some(e),
                }
            }
        }
        let mut search = false;
        let mut fetch = None;
        let mut fill = None;
        let srcs = Src::ALL;
        let hint = Src::hint;
        match self.lookup.as_mut() {
            None => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Fill from").color(TEXT_DIM));
                    for src in srcs {
                        let on = src != Src::Igdb || cfg.is_some();
                        let r = ui
                            .add_enabled(on, egui::Button::new(format!("{}…", src.name())))
                            .on_hover_text(format!("Search {} and preview what it has", hint(src)))
                            .on_disabled_hover_text("Add IGDB credentials in Settings to look games up there.");
                        if r.clicked() {
                            let query = igdb::search_name(&self.name);
                            self.lookup = Some(Lookup { src, query, results: Vec::new(), busy: false, error: None, rx: None, preview: None });
                            search = true;
                        }
                    }
                    ui.separator();
                    let all = egui::Button::new(RichText::new("Compare all…").color(Color32::WHITE)).fill(ACCENT);
                    if ui.add(all).on_hover_text("Search every source at once, side by side, and mix fields between them").clicked() {
                        let year = util::release_input(&self.release).get(..4).and_then(|y| y.parse().ok());
                        self.compare = Some(Compare::new(ui.ctx(), cfg, &self.name, &self.platform, year));
                    }
                    if let Some(f) = &self.filled {
                        ui.label(RichText::new(f).color(TEXT_DIM));
                    }
                });
            }
            Some(s) => {
                let mut close = false;
                egui::Frame::new().fill(CARD).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                    theme::raised_widgets(ui);
                    ui.horizontal(|ui| {
                        // Switching source keeps the query, so the same game can be compared across sources.
                        for src in srcs {
                            let on = src != Src::Igdb || cfg.is_some();
                            let r = ui.add_enabled(on && !s.busy, egui::Button::selectable(s.src == src, src.name()));
                            if r.on_hover_text(hint(src)).clicked() && s.src != src {
                                (s.src, s.results, s.preview) = (src, Vec::new(), None);
                                search = true;
                            }
                        }
                        ui.separator();
                        let r = ui.add(egui::TextEdit::singleline(&mut s.query).desired_width(300.0).hint_text("Game name"));
                        if ui.button("Search").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                            s.preview = None;
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
                    }
                    if let Some((d, igdb_id)) = &s.preview {
                        match preview(ui, d, s.src.name()) {
                            Some(true) => fill = Some((d.clone(), *igdb_id, s.src.name())),
                            Some(false) => s.preview = None,
                            None => {}
                        }
                        return;
                    }
                    if s.error.is_none() && !s.busy && s.results.is_empty() {
                        ui.label(RichText::new("Nothing found. Try a shorter or different name.").color(TEXT_DIM));
                    }
                    let mut picked = None;
                    egui::ScrollArea::vertical().id_salt("ed_lookup").max_height(220.0).show(ui, |ui| {
                        for (i, h) in s.results.iter().enumerate() {
                            let r = ui.add_enabled(!s.busy, egui::Button::selectable(false, h.label()));
                            if r.on_hover_text("Preview this entry").clicked() {
                                picked = Some(i);
                            }
                        }
                    });
                    match picked.map(|i| s.results[i].clone()) {
                        // IGDB hits already carry their details.
                        Some(Hit::Igdb(g)) => {
                            let id = g.id;
                            s.preview = Some((VnDetails { title: g.name, meta: g.meta, tags: Vec::new() }, Some(id)));
                        }
                        Some(h) => fetch = Some(h),
                        None => {}
                    }
                });
                if close {
                    self.lookup = None;
                }
            }
        }
        if let Some((d, igdb_id, src)) = fill {
            self.fill_details(&d, src);
            if igdb_id.is_some() {
                self.igdb_pick = igdb_id;
            }
            self.lookup = None;
        }
        if let Some(s) = self.lookup.as_mut().filter(|_| search || fetch.is_some()) {
            let (tx, rx) = unbounded();
            let (cfg, q, src, ctx) = (cfg.cloned(), s.query.trim().to_string(), s.src, ui.ctx().clone());
            (s.busy, s.error, s.rx) = (true, None, Some(rx));
            std::thread::spawn(move || {
                let err = |e: anyhow::Error| format!("{e:#}");
                let msg = match fetch {
                    Some(h) => Msg::Details(lookup::fetch_details(h).map_err(err)),
                    None => Msg::Results(lookup::search(src, cfg.as_ref(), &q).map_err(err)),
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        }
        ui.add_space(8.0);
    }

    /// Replaces the form's details with an entry's (fields it lacks stay); its tags are added.
    fn fill_details(&mut self, d: &VnDetails, src: &str) {
        if self.name.trim().is_empty() {
            self.name = d.title.clone();
        }
        self.fill_meta(&d.meta);
        self.add_tags(&d.tags);
        self.filled = Some(format!("Filled from {src}: {}. Review and Save.", d.title));
    }

    fn apply_picked(&mut self, p: lookup::Picked) {
        self.fill_meta(&p.meta);
        self.add_tags(&p.tags);
        if p.igdb_id.is_some() {
            self.igdb_pick = p.igdb_id;
        }
        self.filled = Some(format!("Filled from {}. Review and Save.", p.label));
    }

    fn add_tags(&mut self, tags: &[String]) {
        for t in tags {
            if !self.tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                self.tags.push(t.clone());
            }
        }
    }

    fn fill_meta(&mut self, m: &gamut_core::model::Metadata) {
        let set = |field: &mut String, v: &Option<String>| {
            if let Some(v) = v.as_deref().filter(|v| !v.trim().is_empty()) {
                *field = v.to_string();
            }
        };
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
        ui.add_space(14.0);
        self.install_section(ui, w);
    }

    /// Install folder and size, and the save folder.
    fn install_section(&mut self, ui: &mut Ui, w: f32) {
        if let Some(b) = self.size_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            (self.install_size, self.size_rx) = (Some(b), None);
        }
        if let Some(r) = self.save_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.save_rx = None;
            self.save_msg = match r {
                Ok(Some(p)) => {
                    self.save_dir = p.display().to_string();
                    None
                }
                Ok(None) => Some("PCGamingWiki lists save locations, but none exist on this PC.".into()),
                Err(e) => Some(format!("Not found on PCGamingWiki ({e}).")),
            };
        }
        let steam = self.source == Source::Steam;
        egui::Grid::new("ed_install").num_columns(2).spacing([14.0, 10.0]).min_col_width(110.0).show(ui, |ui| {
            row(ui, "Install folder", |ui| {
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(!steam, |ui| text(ui, &mut self.install_dir, w - 90.0, "the game's folder"))
                        .response
                        .on_disabled_hover_text("Steam keeps this up to date.");
                    if !steam && ui.button("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.install_dir = p.display().to_string();
                        }
                    }
                });
            });
            row(ui, "Size", |ui| {
                ui.horizontal(|ui| {
                    let dir = self.folder();
                    ui.label(self.install_size.map(gamut_core::format_size).unwrap_or_else(|| "—".into()));
                    if self.size_rx.is_some() {
                        ui.spinner();
                    } else if ui.add_enabled(dir.is_some(), egui::Button::new("Calculate")).clicked() {
                        let (tx, rx) = unbounded();
                        let ctx = ui.ctx().clone();
                        let dir = dir.unwrap_or_default();
                        std::thread::spawn(move || {
                            let _ = tx.send(gamut_core::dir_size(&dir) as i64);
                            ctx.request_repaint();
                        });
                        self.size_rx = Some(rx);
                    }
                });
            });
            row(ui, "Save folder", |ui| {
                ui.horizontal(|ui| {
                    text(ui, &mut self.save_dir, w - 220.0, "where the game keeps its saves");
                    if ui.button("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_folder() {
                            self.save_dir = p.display().to_string();
                        }
                    }
                    if self.save_rx.is_some() {
                        ui.spinner();
                    } else if ui.button("Find").on_hover_text("Look up the save location on PCGamingWiki").clicked() {
                        let (tx, rx) = unbounded();
                        let ctx = ui.ctx().clone();
                        let mut g = self.orig.clone();
                        g.install_dir = opt(&self.install_dir).or(g.install_dir);
                        std::thread::spawn(move || {
                            let _ = tx.send(gamut_core::pcgw::find_save_dir(&g).map_err(|e| format!("{e:#}")));
                            ctx.request_repaint();
                        });
                        (self.save_rx, self.save_msg) = (Some(rx), None);
                    }
                });
            });
        });
        if let Some(m) = &self.save_msg {
            ui.label(theme::hint(m));
        }
    }

    /// The folder the size is measured in.
    fn folder(&self) -> Option<std::path::PathBuf> {
        let mut g = self.orig.clone();
        g.install_dir = opt(&self.install_dir).or(g.install_dir);
        g.exe = opt(&self.exe).or(g.exe);
        g.folder().filter(|d| d.is_dir())
    }

    fn actions_tab(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        theme::section(ui, "Other ways to start it");
        ui.label(theme::hint(
            "Show up next to Play (mod managers, config tools, other versions). \"Counts as playing\" tracks time and applies Special K, ReShade and HDR.",
        ));
        ui.add_space(4.0);
        let w = ui.available_width();
        let mut remove = None;
        for (i, a) in self.actions.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(w - 16.0);
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut a.name).hint_text("Name").desired_width(180.0));
                    ui.add(egui::TextEdit::singleline(&mut a.path).hint_text("exe, file or URL").desired_width(w - 420.0));
                    if ui.button("Browse…").clicked() {
                        if let Some(p) = rfd::FileDialog::new().pick_file() {
                            if a.name.trim().is_empty() {
                                a.name = p.file_stem().unwrap_or_default().to_string_lossy().into();
                            }
                            a.path = p.display().to_string();
                        }
                    }
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut a.args).hint_text("Arguments").desired_width(300.0));
                    ui.add(egui::TextEdit::singleline(&mut a.work_dir).hint_text("Working folder (default: its folder)").desired_width(260.0));
                    ui.checkbox(&mut a.track, "Counts as playing");
                });
            });
        }
        if let Some(i) = remove {
            self.actions.remove(i);
        }
        if ui.button("Add action").clicked() {
            self.actions.push(PlayAction::default());
        }
        ui.add_space(14.0);
        theme::section(ui, "Scripts");
        ui.label(theme::hint(
            "PowerShell. Before: runs and is waited for (up to 2 minutes) before the game starts. After: runs when the game exits. \
             $env:GAMUT_GAME_NAME, GAMUT_GAME_DIR and GAMUT_GAME_EXE are set.",
        ));
        ui.add_space(4.0);
        for (label, s) in [("Before starting", &mut self.script_before), ("After exiting", &mut self.script_after)] {
            ui.label(label);
            ui.add(egui::TextEdit::multiline(s).code_editor().desired_rows(5).desired_width(f32::INFINITY).hint_text("e.g. Start-Process \"C:\\Tools\\DSX.exe\""));
            ui.add_space(6.0);
        }
    }

    fn links_tab(&mut self, ui: &mut Ui, body_h: f32) {
        ui.add_space(4.0);
        theme::section(ui, "Links");
        ui.label(theme::hint("Shown in the details view, after Steam store, PCGamingWiki, HowLongToBeat and the other automatic ones."));
        ui.add_space(4.0);
        let w = ui.available_width();
        let (mut remove, mut swap) = (None, None);
        let n = self.links.len();
        for (i, l) in self.links.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut l.name).hint_text("Name").desired_width(180.0));
                ui.add(egui::TextEdit::singleline(&mut l.url).hint_text("https://…").desired_width(w - 400.0));
                if ui.add_enabled(i > 0, egui::Button::new("Up")).clicked() {
                    swap = Some(i - 1);
                }
                if ui.add_enabled(i + 1 < n, egui::Button::new("Down")).clicked() {
                    swap = Some(i);
                }
                if ui.button("Remove").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = swap {
            self.links.swap(i, i + 1);
        }
        if let Some(i) = remove {
            self.links.remove(i);
        }
        if ui.button("Add link").clicked() {
            self.links.push(Link::default());
        }
        ui.add_space(14.0);
        theme::section(ui, "Notes");
        let h = (body_h - (ui.cursor().top() - ui.min_rect().top()) - 12.0).max(180.0);
        ui.add_sized(vec2(ui.available_width(), h), egui::TextEdit::multiline(&mut self.notes).hint_text("Anything you want to remember about this game"));
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
        ui.label(theme::hint("Search SteamGridDB, IGDB, VNDB and Steam, or use a file from your PC."));
        ui.add_space(8.0);
        self.icon_section(ui);
        ui.add_space(10.0);
        self.poll_art(ui.ctx());
        let art = self.art.as_ref();
        let cover_h = 300.0;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                theme::section(ui, "Cover");
                art_box(ui, art.and_then(|a| a.cover.as_ref()), vec2(cover_h * 2.0 / 3.0, cover_h), "No cover yet");
                ui.add_space(6.0);
                if ui.button("Change cover…").clicked() {
                    out = Some(EditorOutcome::PickArt(self.id, ArtKind::Cover));
                }
            });
            ui.add_space(24.0);
            ui.vertical(|ui| {
                theme::section(ui, "Banner");
                let w = (ui.available_width() - 8.0).clamp(320.0, 760.0);
                art_box(ui, art.and_then(|a| a.hero.as_ref()), vec2(w, w * 620.0 / 1920.0), "No banner yet (the cover is used)");
                ui.add_space(6.0);
                if ui.button("Change banner…").clicked() {
                    out = Some(EditorOutcome::PickArt(self.id, ArtKind::Banner));
                }
            });
        });
        out
    }

    /// Small square icon (for lists); replaced or removed on save.
    fn icon_section(&mut self, ui: &mut Ui) {
        if self.icon_tex.is_none() && self.icon_rx.is_none() {
            let path = match &self.icon_file {
                Some(p) => p.clone(),
                None => self.icon_path.clone().filter(|p| p.exists()),
            };
            let (tx, rx) = unbounded();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send(path.and_then(|p| crate::thumbs::load_large(&p, 128)));
                ctx.request_repaint();
            });
            self.icon_rx = Some(rx);
        }
        if let Some(img) = self.icon_rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.icon_tex = Some(img.map(|i| ui.ctx().load_texture("ed_icon", i, egui::TextureOptions::LINEAR)));
            self.icon_rx = None;
        }
        theme::section(ui, "Icon");
        ui.horizontal(|ui| {
            art_box(ui, self.icon_tex.as_ref(), vec2(64.0, 64.0), "");
            ui.add_space(8.0);
            ui.vertical(|ui| {
                if ui.button("Choose file…").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]).pick_file() {
                        (self.icon_file, self.icon_tex) = (Some(Some(p)), None);
                    }
                }
                let has = self.icon_tex.as_ref().is_some_and(|t| t.is_some());
                if has && ui.button("Remove").clicked() {
                    (self.icon_file, self.icon_tex) = (Some(None), Some(None));
                }
            });
        });
    }

    /// Starts decoding the art files once, then turns finished images into textures.
    fn poll_art(&mut self, ctx: &egui::Context) {
        if self.art.is_none() {
            let Some((cover, hero)) = self.art_paths.clone() else { return };
            let (tx, rx) = unbounded();
            let c = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send((true, cover.exists().then(|| crate::thumbs::load_large(&cover, 600)).flatten()));
                let _ = tx.send((false, hero.exists().then(|| crate::thumbs::load_large(&hero, 1600)).flatten()));
                c.request_repaint();
            });
            self.art = Some(Art { rx, cover: None, hero: None });
        }
        let Some(a) = self.art.as_mut() else { return };
        while let Ok((is_cover, img)) = a.rx.try_recv() {
            let name = if is_cover { "ed_cover" } else { "ed_hero" };
            let tex = img.map(|i| ctx.load_texture(name, i, egui::TextureOptions::LINEAR));
            if is_cover {
                a.cover = Some(tex);
            } else {
                a.hero = Some(tex);
            }
        }
    }
}

/// An art preview: the image fitted into `size`, a spinner while it loads, or a placeholder.
fn art_box(ui: &mut Ui, tex: Option<&Option<egui::TextureHandle>>, size: egui::Vec2, empty: &str) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(8), CARD);
    match tex {
        Some(Some(t)) => {
            // Fit inside the box, keeping the image's aspect ratio.
            let s = t.size_vec2();
            let k = (size.x / s.x).min(size.y / s.y);
            let r = egui::Rect::from_center_size(rect.center(), s * k);
            egui::Image::new(t).corner_radius(CornerRadius::same(8)).paint_at(ui, r);
        }
        Some(None) => {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, empty, egui::FontId::proportional(13.0), TEXT_DIM);
        }
        None => {
            ui.put(egui::Rect::from_center_size(rect.center(), vec2(18.0, 18.0)), egui::Spinner::new());
        }
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
pub(crate) fn chips(ui: &mut Ui, id: &str, items: &mut Vec<String>, input: &mut String, known: &[(String, usize)], hint: &str) {
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
