use crate::theme::{self, ACCENT, CARD, TEXT_DIM};
use crate::thumbs::{self, ThumbCache};
use crate::util::{self, opt};
use crate::workers::{Event, Workers};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{
    pos2, vec2, Align, Color32, ColorImage, CornerRadius, FontId, Key, Layout, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
};
use gamut_core::model::state;
use gamut_core::{images, launch, Config, Game, Library, Paths, Source};
use std::sync::atomic::Ordering;
use std::sync::{Arc, RwLock};

#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Installed,
    Favorites,
    Steam,
    Custom,
    Hidden,
}

#[derive(Clone, Copy, PartialEq)]
enum Sort {
    Name,
    LastPlayed,
    Playtime,
    Added,
}

enum Action {
    Play(i64),
    Edit(i64),
    ToggleFav(i64),
    ToggleHidden(i64),
    Refresh(i64),
    Remove(i64),
}

struct Editor {
    id: i64,
    source: Source,
    name: String,
    exe: String,
    args: String,
    work_dir: String,
    description: String,
    cover_file: Option<std::path::PathBuf>,
}

pub struct GamutApp {
    paths: Paths,
    cfg: Arc<RwLock<Config>>,
    lib: Library,
    games: Vec<Game>,
    view: Vec<usize>,
    view_key: Option<(String, Filter, Sort, usize)>,
    search: String,
    filter: Filter,
    sort: Sort,
    selected: Option<i64>,
    thumbs: ThumbCache,
    workers: Workers,
    hero: Option<(i64, Option<TextureHandle>)>,
    hero_tx: Sender<(i64, Option<ColorImage>)>,
    hero_rx: Receiver<(i64, Option<ColorImage>)>,
    editor: Option<Editor>,
    settings: Option<Config>,
    confirm_remove: Option<i64>,
    error: Option<String>,
    focus_search: bool,
}

impl GamutApp {
    pub fn new(cc: &eframe::CreationContext, paths: Paths) -> anyhow::Result<Self> {
        theme::apply(&cc.egui_ctx);
        let cfg = Arc::new(RwLock::new(Config::load(&paths.config)));
        let (lib, repair_notice) = Library::open_checked(&paths.db)?;
        let games = lib.all()?;
        let workers = Workers::start(&cc.egui_ctx, paths.clone(), cfg.clone());
        workers.rescan();
        let (hero_tx, hero_rx) = unbounded();
        Ok(GamutApp {
            thumbs: ThumbCache::new(&cc.egui_ctx, paths.clone(), 3),
            paths,
            cfg,
            lib,
            games,
            view: Vec::new(),
            view_key: None,
            search: String::new(),
            filter: Filter::All,
            sort: Sort::Name,
            selected: None,
            workers,
            hero: None,
            hero_tx,
            hero_rx,
            editor: None,
            settings: None,
            confirm_remove: None,
            error: repair_notice,
            focus_search: false,
        })
    }

    fn game(&self, id: i64) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }

    fn reload_all(&mut self) {
        match self.lib.all() {
            Ok(g) => self.games = g,
            Err(e) => self.error = Some(format!("{e:#}")),
        }
        self.view_key = None;
    }

    fn reload_one(&mut self, id: i64) {
        if let Ok(Some(g)) = self.lib.get(id) {
            if let Some(slot) = self.games.iter_mut().find(|x| x.id == id) {
                *slot = g;
            } else {
                self.games.push(g);
            }
            self.view_key = None;
        }
        self.thumbs.invalidate(id);
        if self.hero.as_ref().is_some_and(|(h, t)| *h == id && t.is_none()) {
            self.hero = None;
        }
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.workers.rx.try_recv() {
            match ev {
                Event::LibraryChanged => self.reload_all(),
                Event::GameUpdated(id) => self.reload_one(id),
                Event::Error(e) => self.error = Some(e),
            }
        }
    }

    /// Rebuilds the filtered + sorted index list when inputs change.
    fn update_view(&mut self) {
        let key = (self.search.clone(), self.filter, self.sort, self.games.len());
        if self.view_key.as_ref() == Some(&key) {
            return;
        }
        let q = self.search.to_lowercase();
        let mut v: Vec<usize> = (0..self.games.len())
            .filter(|&i| {
                let g = &self.games[i];
                let pass = match self.filter {
                    Filter::Hidden => g.hidden,
                    _ if g.hidden => false,
                    Filter::All => true,
                    Filter::Installed => g.installed,
                    Filter::Favorites => g.favorite,
                    Filter::Steam => g.source == Source::Steam,
                    Filter::Custom => g.source == Source::Custom,
                };
                pass && (q.is_empty() || g.name.to_lowercase().contains(&q))
            })
            .collect();
        let gs = &self.games;
        match self.sort {
            Sort::Name => v.sort_by_cached_key(|&i| gs[i].name.to_lowercase()),
            Sort::LastPlayed => v.sort_by_key(|&i| std::cmp::Reverse(gs[i].last_played)),
            Sort::Playtime => v.sort_by_key(|&i| std::cmp::Reverse(gs[i].playtime_min)),
            Sort::Added => v.sort_by_key(|&i| std::cmp::Reverse((gs[i].added, gs[i].id))),
        }
        self.view = v;
        self.view_key = Some(key);
    }

    fn apply(&mut self, a: Action) {
        let r: anyhow::Result<()> = (|| {
            match a {
                Action::Play(id) => {
                    let Some(g) = self.game(id).cloned() else { return Ok(()) };
                    let l = launch::launch(&g)?;
                    self.workers.track(id, l);
                }
                Action::Edit(id) => {
                    if let Some(g) = self.game(id) {
                        self.editor = Some(Editor {
                            id,
                            source: g.source,
                            name: g.name.clone(),
                            exe: g.exe.clone().unwrap_or_default(),
                            args: g.args.clone().unwrap_or_default(),
                            work_dir: g.work_dir.clone().unwrap_or_default(),
                            description: g.description.clone().unwrap_or_default(),
                            cover_file: None,
                        });
                    }
                }
                Action::ToggleFav(id) | Action::ToggleHidden(id) => {
                    let Some(g) = self.game(id) else { return Ok(()) };
                    if matches!(a, Action::ToggleFav(_)) {
                        self.lib.set_favorite(id, !g.favorite)?;
                    } else {
                        self.lib.set_hidden(id, !g.hidden)?;
                    }
                    self.reload_one(id);
                }
                Action::Refresh(id) => {
                    self.lib.requeue(id, true)?;
                    self.hero = None;
                    self.reload_one(id);
                    self.workers.kick();
                }
                Action::Remove(id) => self.confirm_remove = Some(id),
            }
            Ok(())
        })();
        if let Err(e) = r {
            self.error = Some(format!("{e:#}"));
        }
    }

    // ---------- panels ----------

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("GAMUT").strong().color(ACCENT).size(18.0));
            ui.add_space(12.0);
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search  (Ctrl+F)")
                    .desired_width(260.0),
            );
            if std::mem::take(&mut self.focus_search) {
                search.request_focus();
            }
            egui::ComboBox::from_id_salt("filter")
                .selected_text(match self.filter {
                    Filter::All => "All games",
                    Filter::Installed => "Installed",
                    Filter::Favorites => "Favorites",
                    Filter::Steam => "Steam",
                    Filter::Custom => "Custom",
                    Filter::Hidden => "Hidden",
                })
                .show_ui(ui, |ui| {
                    for (f, l) in [
                        (Filter::All, "All games"),
                        (Filter::Installed, "Installed"),
                        (Filter::Favorites, "Favorites"),
                        (Filter::Steam, "Steam"),
                        (Filter::Custom, "Custom"),
                        (Filter::Hidden, "Hidden"),
                    ] {
                        ui.selectable_value(&mut self.filter, f, l);
                    }
                })
                .response
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            egui::ComboBox::from_id_salt("sort")
                .selected_text(match self.sort {
                    Sort::Name => "Name",
                    Sort::LastPlayed => "Last played",
                    Sort::Playtime => "Playtime",
                    Sort::Added => "Recently added",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.sort, Sort::Name, "Name");
                    ui.selectable_value(&mut self.sort, Sort::LastPlayed, "Last played");
                    ui.selectable_value(&mut self.sort, Sort::Playtime, "Playtime");
                    ui.selectable_value(&mut self.sort, Sort::Added, "Recently added");
                })
                .response
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let mut w = self.cfg.read().unwrap().card_width;
            if ui
                .add(egui::Slider::new(&mut w, 110.0..=280.0).show_value(false))
                .on_hover_text("Cover size")
                .changed()
            {
                self.cfg.write().unwrap().card_width = w;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings = Some(self.cfg.read().unwrap().clone());
                }
                let scanning = self.workers.scanning.load(Ordering::Relaxed);
                if ui.add_enabled(!scanning, egui::Button::new("Refresh")).clicked() {
                    self.workers.rescan();
                }
                if ui.button("Add game").clicked() {
                    self.editor = Some(Editor {
                        id: 0,
                        source: Source::Custom,
                        name: String::new(),
                        exe: String::new(),
                        args: String::new(),
                        work_dir: String::new(),
                        description: String::new(),
                        cover_file: None,
                    });
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let visible = self.games.iter().filter(|g| !g.hidden).count();
            let installed = self.games.iter().filter(|g| !g.hidden && g.installed).count();
            ui.label(RichText::new(format!("{visible} games · {installed} installed")).color(TEXT_DIM));
            if self.workers.scanning.load(Ordering::Relaxed) {
                ui.spinner();
                ui.label(RichText::new("Scanning Steam…").color(TEXT_DIM));
            }
            let c = self.workers.pending_cover.load(Ordering::Relaxed);
            let m = self.workers.pending_meta.load(Ordering::Relaxed);
            if c + m > 0 {
                ui.spinner();
                ui.label(RichText::new(format!("Fetching: {c} covers, {m} metadata")).color(TEXT_DIM));
            }
            let cfg = self.cfg.read().unwrap();
            if cfg.steam_api_key.trim().is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("Installed Steam games only — add a Steam API key in Settings for your full library")
                            .color(TEXT_DIM),
                    );
                });
            }
        });
    }

    fn grid(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let card_w = self.cfg.read().unwrap().card_width;
        let card_h = (card_w * 1.5).round();
        let title_h = 24.0;
        let gap = 16.0;
        let row_h = card_h + title_h;
        let avail = ui.available_width() - 8.0;
        let cols = (((avail + gap) / (card_w + gap)).floor() as usize).max(1);
        let rows = self.view.len().div_ceil(cols);
        let used = cols as f32 * card_w + (cols - 1) as f32 * gap;
        let pad = ((avail - used) / 2.0).max(0.0);

        if self.view.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No games here. Try Refresh, or + Add game.").color(TEXT_DIM));
            });
            return;
        }

        let games = &self.games;
        let view = &self.view;
        let thumbs = &mut self.thumbs;
        let selected = &mut self.selected;
        ui.spacing_mut().item_spacing.y = gap;
        ScrollArea::vertical().auto_shrink([false; 2]).show_rows(ui, row_h, rows, |ui, range| {
            ui.add_space(4.0);
            for row in range {
                let (row_rect, _) = ui.allocate_exact_size(vec2(avail, row_h), Sense::hover());
                for c in 0..cols {
                    let Some(&gi) = view.get(row * cols + c) else { break };
                    let g = &games[gi];
                    let x = row_rect.left() + pad + c as f32 * (card_w + gap);
                    let cover = Rect::from_min_size(pos2(x, row_rect.top()), vec2(card_w, card_h));
                    let full = Rect::from_min_size(cover.min, vec2(card_w, row_h));
                    let resp = ui.interact(full, ui.scope_id().with(("card", g.id)), Sense::click());
                    let hovered = resp.hovered();
                    if hovered {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    let is_sel = *selected == Some(g.id);
                    let painter = ui.painter();
                    let r = CornerRadius::same(6);

                    // Lift hovered cards slightly.
                    let cover = if hovered { cover.translate(vec2(0.0, -3.0)) } else { cover };
                    match thumbs.get(g.id, g.has_cover) {
                        Some(t) => {
                            egui::Image::from_texture(t).corner_radius(r).paint_at(ui, cover);
                        }
                        None => {
                            painter.rect_filled(cover, r, CARD);
                            let galley = painter.layout(
                                g.name.clone(),
                                FontId::proportional(15.0),
                                Color32::from_gray(190),
                                card_w - 20.0,
                            );
                            let p = cover.center() - galley.size() / 2.0;
                            painter.galley(p, galley, Color32::WHITE);
                        }
                    }
                    if g.favorite {
                        let c = cover.right_top() + vec2(-16.0, 16.0);
                        painter.circle_filled(c, 13.0, Color32::from_black_alpha(140));
                        paint_star(painter, c, 9.0, true);
                    }
                    if is_sel || hovered {
                        let s = if is_sel { Stroke::new(2.5, ACCENT) } else { Stroke::new(1.5, Color32::from_gray(200)) };
                        painter.rect_stroke(cover, r, s, StrokeKind::Outside);
                    }
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        g.name.clone(),
                        FontId::proportional(13.5),
                        if is_sel { Color32::WHITE } else { Color32::from_gray(200) },
                    );
                    job.wrap = egui::text::TextWrapping::truncate_at_width(card_w);
                    let galley = painter.layout_job(job);
                    painter.galley(pos2(x, cover.bottom() + 6.0), galley, Color32::WHITE);

                    if resp.clicked() {
                        *selected = Some(g.id);
                    }
                    if resp.double_clicked() {
                        actions.push(Action::Play(g.id));
                    }
                    let id = g.id;
                    resp.context_menu(|ui| {
                        *selected = Some(id);
                        context_menu(ui, g, actions);
                    });
                }
            }
        });
    }

    fn details(&mut self, ui: &mut Ui, id: i64, actions: &mut Vec<Action>) {
        let Some(g) = self.game(id).cloned() else { return };

        // Load banner off-thread on selection change.
        if self.hero.as_ref().map(|(h, _)| *h) != Some(id) {
            self.hero = Some((id, None));
            let path = if g.has_hero { self.paths.hero(id) } else { self.paths.cover(id) };
            let tx = self.hero_tx.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send((id, thumbs::load_large(&path, 1600)));
                ctx.request_repaint();
            });
        }
        for (hid, img) in self.hero_rx.try_iter() {
            if let (Some((cur, slot)), Some(img)) = (self.hero.as_mut(), img) {
                if *cur == hid {
                    *slot = Some(ui.ctx().load_texture(format!("hero{hid}"), img, TextureOptions::LINEAR));
                }
            }
        }

        let w = ui.available_width();
        let banner = Rect::from_min_size(ui.cursor().min, vec2(w, (w * 0.42).round()));
        ui.allocate_rect(banner, Sense::hover());
        match self.hero.as_ref().and_then(|(_, t)| t.as_ref()) {
            Some(t) => {
                let [iw, ih] = t.size();
                egui::Image::from_texture(t)
                    .uv(cover_uv(iw as f32 / ih as f32, banner.aspect_ratio()))
                    .corner_radius(CornerRadius::same(8))
                    .paint_at(ui, banner);
            }
            None => {
                ui.painter().rect_filled(banner, CornerRadius::same(8), CARD);
            }
        }
        let close = Rect::from_center_size(banner.right_top() + vec2(-20.0, 20.0), vec2(28.0, 28.0));
        let cr = ui.interact(close, ui.scope_id().with("close_details"), Sense::click()).on_hover_text("Close");
        let bg = if cr.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(150) };
        ui.painter().circle_filled(close.center(), 14.0, bg);
        let (c, d) = (close.center(), 5.0);
        let st = Stroke::new(2.0, Color32::WHITE);
        ui.painter().line_segment([c + vec2(-d, -d), c + vec2(d, d)], st);
        ui.painter().line_segment([c + vec2(-d, d), c + vec2(d, -d)], st);
        if cr.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if cr.clicked() {
            self.selected = None;
        }

        ui.add_space(8.0);
        ui.label(RichText::new(&g.name).heading().strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = if g.source == Source::Steam && !g.installed { "Install" } else { "Play" };
            let play = egui::Button::new(RichText::new(label).size(16.0).strong().color(Color32::WHITE))
                .fill(ACCENT)
                .min_size(vec2(130.0, 36.0));
            if ui.add(play).clicked() {
                actions.push(Action::Play(id));
            }
            let fav = ui.add(egui::Button::new("").min_size(vec2(36.0, 36.0))).on_hover_text("Favorite");
            paint_star(ui.painter(), fav.rect.center(), 9.0, g.favorite);
            if fav.clicked()
            {
                actions.push(Action::ToggleFav(id));
            }
            if ui.add(egui::Button::new("Edit").min_size(vec2(0.0, 36.0))).clicked() {
                actions.push(Action::Edit(id));
            }
            ui.menu_button("More", |ui| context_menu(ui, &g, actions)).response.on_hover_cursor(egui::CursorIcon::PointingHand);
        });
        ui.add_space(8.0);

        egui::Grid::new("info").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
            let mut row = |k: &str, v: String| {
                ui.label(RichText::new(k).color(TEXT_DIM));
                ui.label(v);
                ui.end_row();
            };
            row("Playtime", util::playtime(g.playtime_min));
            row("Last played", util::date(g.last_played));
            row(
                "Source",
                match g.source {
                    Source::Steam => format!("Steam · {}", if g.installed { "installed" } else { "not installed" }),
                    Source::Custom => "Custom".into(),
                },
            );
            for (k, v) in [
                ("Developer", &g.developer),
                ("Publisher", &g.publisher),
                ("Released", &g.release_date),
                ("Genres", &g.genres),
            ] {
                if let Some(v) = v {
                    row(k, v.clone());
                }
            }
        });
        ui.add_space(8.0);
        ui.separator();
        ScrollArea::vertical().id_salt("desc").show(ui, |ui| {
            match &g.description {
                Some(d) => ui.label(d),
                None if g.meta_state == state::PENDING => ui.label(RichText::new("Fetching metadata…").color(TEXT_DIM)),
                None => ui.label(RichText::new("No description.").color(TEXT_DIM)),
            };
        });
    }

    // ---------- dialogs ----------

    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.editor.as_mut() else { return };
        let mut open = true;
        let mut save = false;
        let title = if ed.id == 0 { "Add game" } else { "Edit game" };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(480.0)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                egui::Grid::new("ed").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut ed.name).desired_width(340.0));
                    ui.end_row();
                    if ed.source == Source::Custom {
                        ui.label("Executable");
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut ed.exe).desired_width(260.0).hint_text("exe, shortcut or URL"));
                            if ui.button("Browse…").clicked() {
                                if let Some(p) = rfd::FileDialog::new()
                                    .add_filter("Programs", &["exe", "bat", "cmd", "lnk", "url"])
                                    .add_filter("All files", &["*"])
                                    .pick_file()
                                {
                                    if ed.name.trim().is_empty() {
                                        if let Some(stem) = p.file_stem() {
                                            ed.name = stem.to_string_lossy().into();
                                        }
                                    }
                                    ed.exe = p.to_string_lossy().into();
                                }
                            }
                        });
                        ui.end_row();
                        ui.label("Arguments");
                        ui.add(egui::TextEdit::singleline(&mut ed.args).desired_width(340.0));
                        ui.end_row();
                        ui.label("Working dir");
                        ui.add(
                            egui::TextEdit::singleline(&mut ed.work_dir)
                                .desired_width(340.0)
                                .hint_text("defaults to the exe's folder"),
                        );
                        ui.end_row();
                    }
                    ui.label("Cover");
                    ui.horizontal(|ui| {
                        if ui.button("Choose image…").clicked() {
                            ed.cover_file = rfd::FileDialog::new()
                                .add_filter("Images", &["jpg", "jpeg", "png", "webp"])
                                .pick_file();
                        }
                        let t = match &ed.cover_file {
                            Some(p) => p.file_name().unwrap_or_default().to_string_lossy().to_string(),
                            None => "auto (scraped)".into(),
                        };
                        ui.label(RichText::new(t).color(TEXT_DIM));
                    });
                    ui.end_row();
                    ui.label("Description");
                    ui.add(egui::TextEdit::multiline(&mut ed.description).desired_width(340.0).desired_rows(5));
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let ok = !ed.name.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new("Save").fill(ACCENT)).clicked() {
                        save = true;
                    }
                });
            });
        if save {
            if let Err(e) = self.save_editor() {
                self.error = Some(format!("{e:#}"));
            }
            self.editor = None;
        } else if !open {
            self.editor = None;
        }
    }

    fn save_editor(&mut self) -> anyhow::Result<()> {
        let Some(ed) = self.editor.take() else { return Ok(()) };
        let id = if ed.id == 0 {
            let mut g = Game::new_custom(ed.name.trim().into(), opt(&ed.exe));
            g.args = opt(&ed.args);
            g.work_dir = opt(&ed.work_dir);
            let id = self.lib.add_custom(&g)?;
            self.selected = Some(id);
            id
        } else {
            ed.id
        };
        let Some(mut g) = self.lib.get(id)? else { return Ok(()) };
        g.name = ed.name.trim().into();
        g.exe = opt(&ed.exe);
        g.args = opt(&ed.args);
        g.work_dir = opt(&ed.work_dir);
        g.description = opt(&ed.description);
        self.lib.update_user_fields(&g)?;
        if let Some(p) = ed.cover_file {
            let bytes = std::fs::read(p)?;
            images::store_cover(&bytes, &self.paths.cover(id), &self.paths.thumb(id))?;
            self.lib.set_cover_state(id, true, None, state::DONE)?;
        }
        self.reload_one(id);
        self.workers.kick();
        Ok(())
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        let Some(s) = self.settings.as_mut() else { return };
        let mut open = true;
        let mut save = false;
        let detected = gamut_core::steam::steam_path()
            .and_then(|p| gamut_core::steam::detect_steam_id(&p))
            .unwrap_or_default();
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.heading("Steam");
                egui::Grid::new("st").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("Web API key");
                    ui.add(egui::TextEdit::singleline(&mut s.steam_api_key).password(true).desired_width(320.0));
                    ui.end_row();
                    ui.label("SteamID64");
                    ui.add(
                        egui::TextEdit::singleline(&mut s.steam_id)
                            .hint_text(format!("auto: {detected}"))
                            .desired_width(320.0),
                    );
                    ui.end_row();
                });
                ui.hyperlink_to("Get a Steam Web API key", "https://steamcommunity.com/dev/apikey");
                ui.label(
                    RichText::new("Your profile's game details must be public, or the key must belong to this account.")
                        .color(TEXT_DIM)
                        .small(),
                );
                ui.add_space(10.0);
                ui.heading("SteamGridDB");
                egui::Grid::new("sg").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                    ui.label("API key");
                    ui.add(egui::TextEdit::singleline(&mut s.steamgriddb_key).password(true).desired_width(320.0));
                    ui.end_row();
                });
                ui.hyperlink_to("Get a SteamGridDB key", "https://www.steamgriddb.com/profile/preferences/api");
                ui.add_space(10.0);
                ui.label(RichText::new(format!("Data folder: {}", self.paths.root.display())).color(TEXT_DIM).small());
                ui.add_space(6.0);
                if ui.add(egui::Button::new("Save").fill(ACCENT)).clicked() {
                    save = true;
                }
            });
        if save {
            let new = self.settings.take().unwrap();
            let old = self.cfg.read().unwrap().clone();
            *self.cfg.write().unwrap() = new.clone();
            if let Err(e) = new.save(&self.paths.config) {
                self.error = Some(format!("Saving settings failed: {e:#}"));
            }
            if new.steamgriddb_key != old.steamgriddb_key {
                let _ = self.lib.retry_missing_covers();
            }
            self.workers.rescan();
        } else if !open {
            self.settings = None;
        }
    }

    fn confirm_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_remove else { return };
        let name = self.game(id).map(|g| g.name.clone()).unwrap_or_default();
        let mut close = false;
        egui::Window::new("Remove game")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(format!("Remove \"{name}\" from the library? The game's files aren't touched."));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        if let Err(e) = self.lib.delete(id) {
                            self.error = Some(format!("{e:#}"));
                        }
                        for p in [self.paths.cover(id), self.paths.thumb(id), self.paths.hero(id)] {
                            let _ = std::fs::remove_file(p);
                        }
                        if self.selected == Some(id) {
                            self.selected = None;
                        }
                        self.reload_all();
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.confirm_remove = None;
        }
    }

    fn error_window(&mut self, ctx: &egui::Context) {
        let Some(e) = self.error.clone() else { return };
        let mut open = true;
        egui::Window::new("Gamut")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(e);
            });
        if !open {
            self.error = None;
        }
    }
}

fn context_menu(ui: &mut Ui, g: &Game, actions: &mut Vec<Action>) {
    let id = g.id;
    let mut item = |ui: &mut Ui, label: &str, a: Action| {
        if ui.button(label).clicked() {
            actions.push(a);
            ui.close();
        }
    };
    item(ui, if g.source == Source::Steam && !g.installed { "Install" } else { "Play" }, Action::Play(id));
    item(ui, "Edit…", Action::Edit(id));
    item(ui, if g.favorite { "Unfavorite" } else { "Favorite" }, Action::ToggleFav(id));
    item(ui, if g.hidden { "Unhide" } else { "Hide" }, Action::ToggleHidden(id));
    item(ui, "Refresh metadata", Action::Refresh(id));
    if g.source == Source::Custom {
        ui.separator();
        item(ui, "Remove…", Action::Remove(id));
    }
}

/// Five-point star: gold fill when on, dim outline when off (emoji fonts ignore tint).
fn paint_star(p: &egui::Painter, c: egui::Pos2, r: f32, on: bool) {
    let pts: Vec<egui::Pos2> = (0..10)
        .map(|i| {
            let a = std::f32::consts::PI * (i as f32 / 5.0) - std::f32::consts::FRAC_PI_2;
            let rad = if i % 2 == 0 { r } else { r * 0.45 };
            c + vec2(a.cos(), a.sin()) * rad
        })
        .collect();
    if on {
        // Fill as triangles from the centre so the concave shape renders correctly.
        let gold = Color32::from_rgb(255, 205, 80);
        for i in 0..10 {
            p.add(egui::Shape::convex_polygon(vec![c, pts[i], pts[(i + 1) % 10]], gold, Stroke::NONE));
        }
    } else {
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.5, TEXT_DIM)));
    }
}

/// UV rect that crops an image of aspect `img` to fill a rect of aspect `rect`.
fn cover_uv(img: f32, rect: f32) -> Rect {
    if img > rect {
        let u = rect / img;
        Rect::from_min_max(pos2((1.0 - u) / 2.0, 0.0), pos2((1.0 + u) / 2.0, 1.0))
    } else {
        let v = img / rect;
        Rect::from_min_max(pos2(0.0, (1.0 - v) / 2.0), pos2(1.0, (1.0 + v) / 2.0))
    }
}

impl eframe::App for GamutApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events();
        self.update_view();
        self.thumbs.begin_frame(&ctx);

        let typing = ctx.memory(|m| m.focused().is_some());
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(Key::F) {
                self.focus_search = true;
            }
        });
        let mut actions = Vec::new();
        if !typing && self.editor.is_none() {
            if let Some(id) = self.selected {
                if ctx.input(|i| i.key_pressed(Key::Enter)) {
                    actions.push(Action::Play(id));
                }
            }
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.selected = None;
            }
        }

        egui::Panel::top("top")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 10)))
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 5)))
            .show(ui, |ui| self.status_bar(ui));
        let central = egui::CentralPanel::no_frame()
            .frame(egui::Frame::central_panel(ui.style()).fill(theme::BG).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| self.grid(ui, &mut actions))
            .response
            .rect;

        // Details float over the grid's right edge so the grid never reflows.
        if let Some(id) = self.selected {
            let w = 420.0_f32.min(central.width() * 0.9);
            let margin = 16.0;
            let frame = egui::Frame::new()
                .fill(theme::PANEL)
                .inner_margin(egui::Margin::same(margin as i8))
                .shadow(ctx.global_style().visuals.window_shadow)
                .stroke(Stroke::new(1.0, Color32::from_gray(45)));
            egui::Area::new(egui::Id::unique("details_overlay"))
                .order(egui::Order::Foreground)
                .fixed_pos(pos2(central.right() - w, central.top()))
                .show(&ctx, |ui| {
                    frame.show(ui, |ui| {
                        let size = vec2(w - 2.0 * margin, central.height() - 2.0 * margin);
                        ui.set_min_size(size);
                        ui.set_max_size(size);
                        self.details(ui, id, &mut actions);
                    });
                });
        }

        self.editor_window(&ctx);
        self.settings_window(&ctx);
        self.confirm_window(&ctx);
        self.error_window(&ctx);

        for a in actions {
            self.apply(a);
        }
        self.thumbs.end_frame();
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        let _ = self.cfg.read().unwrap().save_ui_prefs(&self.paths.config);
    }
}
