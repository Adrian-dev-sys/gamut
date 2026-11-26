use crate::editor::{Editor, EditorOutcome, Lists};
use crate::notify::Toasts;
use crate::picker::{ArtKind, Outcome, Picker};
use crate::theme::{self, ACCENT, CARD, CARD_HI, GOOD, TEXT_DIM};
use crate::thumbs::{self, ThumbCache};
use crate::util::{self, opt};
use crate::workers::{Event, Workers};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{
    pos2, vec2, Align, Color32, ColorImage, CornerRadius, FontId, Key, Layout, Rect, RichText, ScrollArea,
    Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui,
};
use gamut_core::model::state;
use gamut_core::{images, Config, Game, Library, Paths, Source};
use std::sync::atomic::Ordering;
use std::collections::HashSet;
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
    Released,
    CriticScore,
    CommunityScore,
    PlayCount,
    Completion,
}

// Stable names used in config.toml.
const FILTERS: [(Filter, &str); 6] = [
    (Filter::All, "all"),
    (Filter::Installed, "installed"),
    (Filter::Favorites, "favorites"),
    (Filter::Steam, "steam"),
    (Filter::Custom, "custom"),
    (Filter::Hidden, "hidden"),
];
const SORTS: [(Sort, &str); 9] = [
    (Sort::Name, "name"),
    (Sort::LastPlayed, "last_played"),
    (Sort::Playtime, "playtime"),
    (Sort::Added, "added"),
    (Sort::Released, "released"),
    (Sort::CriticScore, "critic_score"),
    (Sort::CommunityScore, "community_score"),
    (Sort::PlayCount, "play_count"),
    (Sort::Completion, "completion"),
];

/// Menu labels, in menu order.
const SORT_LABELS: [(Sort, &str); 9] = [
    (Sort::Name, "Name"),
    (Sort::LastPlayed, "Last played"),
    (Sort::Playtime, "Playtime"),
    (Sort::PlayCount, "Most played"),
    (Sort::Added, "Recently added"),
    (Sort::Released, "Release date"),
    (Sort::CriticScore, "Critic score"),
    (Sort::CommunityScore, "User score"),
    (Sort::Completion, "Completion status"),
];

/// Grid order for completion statuses; anything unknown sorts after these.
const COMPLETION_ORDER: [&str; 8] = ["Playing", "Beaten", "Completed", "Played", "On Hold", "Plan to Play", "Not Played", "Abandoned"];

/// A game's completion status; games without one count as "Not Played".
fn completion_of(g: &Game) -> &str {
    g.completion.as_deref().filter(|c| !c.is_empty()).unwrap_or("Not Played")
}

fn key_of<T: PartialEq + Copy>(table: &[(T, &'static str)], v: T) -> &'static str {
    table.iter().find(|(t, _)| *t == v).map(|(_, k)| *k).unwrap_or("")
}
fn from_key<T: Copy>(table: &[(T, &'static str)], k: &str) -> T {
    table.iter().find(|(_, n)| *n == k).map(|(t, _)| *t).unwrap_or(table[0].0)
}

const SETTINGS_TABS: [&str; 4] = ["Library", "Special K", "ReShade", "General"];

enum Action {
    Play(i64),
    Edit(i64),
    ToggleFav(i64),
    ToggleHidden(i64),
    Refresh(i64),
    Remove(i64),
    PickArt(i64, ArtKind),
    /// Ask to close a running game.
    Stop(i64),
}


pub struct GamutApp {
    paths: Paths,
    cfg: Arc<RwLock<Config>>,
    lib: Library,
    games: Vec<Game>,
    view: Vec<usize>,
    view_key: Option<(String, Filter, Sort, usize, String, String)>,
    search: String,
    filter: Filter,
    sort: Sort,
    /// Platform / tag narrowing ("" = any), with the values present in the library and their counts.
    platform: String,
    tag: String,
    completion: String,
    completions: Vec<(String, usize)>,
    completion_query: String,
    platforms: Vec<(String, usize)>,
    tags: Vec<(String, usize)>,
    tag_query: String,
    platform_query: String,
    selected: Option<i64>,
    thumbs: ThumbCache,
    workers: Workers,
    hero: Option<(i64, Option<TextureHandle>)>,
    hero_tx: Sender<(i64, Option<ColorImage>)>,
    hero_rx: Receiver<(i64, Option<ColorImage>)>,
    editor: Option<Editor>,
    /// Genre and completion suggestions, gathered when the editor opens.
    ed_lists: (Vec<(String, usize)>, Vec<(String, usize)>),
    settings: Option<Config>,
    confirm_remove: Option<i64>,
    confirm_stop: Option<i64>,
    settings_tab: usize,
    picker: Option<Picker>,
    ctx_handle: egui::Context,
    toasts: Toasts,
    /// Playnite folder + preview report awaiting confirmation.
    playnite: Option<(std::path::PathBuf, gamut_core::playnite::Summary)>,
    /// Import was clicked once; the dialog now asks to confirm the overwrite.
    playnite_sure: bool,
    focus_search: bool,
    /// When the details popup was opened by a click; a quick second click there means Play.
    opened_at: f64,
    startup: Option<crate::startup::Startup>,
    /// Latest un-maximized placement + maximized flag, saved on exit.
    window: Option<gamut_core::config::WindowState>,
    /// Sort/filter as last written to config.
    saved_view: (Sort, Filter, String, String, String),
}

impl GamutApp {
    pub fn new(cc: &eframe::CreationContext, paths: Paths) -> anyhow::Result<Self> {
        theme::apply(&cc.egui_ctx);
        let loaded = Config::load(&paths.config);
        let (sort, filter) = (from_key(&SORTS, &loaded.sort), from_key(&FILTERS, &loaded.filter));
        let window = loaded.window;
        let (platform, tag, completion) = (loaded.platform.clone(), loaded.tag.clone(), loaded.completion.clone());
        let cfg = Arc::new(RwLock::new(loaded));
        let (lib, repair_notice) = Library::open_checked(&paths.db)?;
        let games = lib.all()?;
        let (platforms, tags) = facets(&games);
        let completions = completion_facets(&games);
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
            filter,
            sort,
            platform: platform.clone(),
            tag: tag.clone(),
            completion: completion.clone(),
            completions,
            completion_query: String::new(),
            platforms,
            tags,
            tag_query: String::new(),
            platform_query: String::new(),
            selected: None,
            workers,
            hero: None,
            hero_tx,
            hero_rx,
            editor: None,
            ed_lists: Default::default(),
            settings: None,
            confirm_remove: None,
            confirm_stop: None,
            settings_tab: 0,
            picker: None,
            ctx_handle: cc.egui_ctx.clone(),
            toasts: Toasts::with(repair_notice),
            playnite: None,
            playnite_sure: false,
            focus_search: false,
            opened_at: 0.0,
            startup: window.as_ref().filter(|w| w.maximized).and_then(|_| crate::startup::Startup::maximize_hidden(cc)),
            window,
            saved_view: (sort, filter, platform, tag, completion),
        })
    }

    /// Ids of games running now: ones Gamut launched and is tracking, plus whatever Steam reports.
    fn running(&self) -> HashSet<i64> {
        let mut set = self.workers.running.lock().unwrap().clone();
        let steam = self.workers.steam_running.load(Ordering::Relaxed);
        let starting = self.workers.steam_starting.lock().unwrap();
        if !starting.is_empty() {
            set.extend(self.games.iter().filter(|g| g.source == Source::Steam && g.steam_appid().is_some_and(|a| starting.contains_key(&a))).map(|g| g.id));
        }
        drop(starting);
        if steam != 0 {
            set.extend(self.games.iter().filter(|g| g.steam_appid() == Some(steam)).map(|g| g.id));
        }
        set
    }

    fn game(&self, id: i64) -> Option<&Game> {
        self.games.iter().find(|g| g.id == id)
    }

    fn reload_all(&mut self) {
        match self.lib.all() {
            Ok(g) => self.games = g,
            Err(e) => self.toasts.error(format!("{e:#}")),
        }
        (self.platforms, self.tags) = facets(&self.games);
        self.completions = completion_facets(&self.games);
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
                Event::Error(e) => self.toasts.error(e),
                Event::Info(m) => self.toasts.info(m),
                Event::PlaynitePreview(Ok(p)) => self.playnite = Some(p),
                Event::PlaynitePreview(Err(e)) => self.toasts.error(e),
                Event::PlayniteImported(r) => {
                    // Covers were replaced in bulk.
                    self.thumbs.clear();
                    self.hero = None;
                    match r {
                        Ok(m) => self.toasts.success(m),
                        Err(e) => self.toasts.error(e),
                    }
                }
            }
        }
    }

    /// Rebuilds the filtered + sorted index list when inputs change.
    fn update_view(&mut self) {
        let key = (
            self.search.clone(),
            self.filter,
            self.sort,
            self.games.len(),
            self.platform.clone(),
            format!("{}\u{1}{}", self.tag, self.completion),
        );
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
                pass
                    && (self.platform.is_empty() || platforms_of(g).any(|p| p == self.platform))
                    && (self.tag.is_empty() || split_list(&g.tags).any(|t| t == self.tag))
                    && (self.completion.is_empty() || completion_of(g) == self.completion)
                    && (q.is_empty() || g.name.to_lowercase().contains(&q))
            })
            .collect();
        let gs = &self.games;
        match self.sort {
            Sort::Name => v.sort_by_cached_key(|&i| gs[i].name.to_lowercase()),
            Sort::LastPlayed => v.sort_by_key(|&i| std::cmp::Reverse(gs[i].last_played)),
            Sort::Playtime => v.sort_by_key(|&i| std::cmp::Reverse(gs[i].playtime_min)),
            Sort::Added => v.sort_by_key(|&i| std::cmp::Reverse((gs[i].added, gs[i].id))),
            // Highest first; games without a value go last, then by name.
            Sort::Released => v.sort_by_cached_key(|&i| {
                let d = gs[i].release_date.as_deref().map(sortable_date).unwrap_or_default();
                (d.is_empty(), std::cmp::Reverse(d), gs[i].name.to_lowercase())
            }),
            Sort::CriticScore => v.sort_by_cached_key(|&i| (std::cmp::Reverse(gs[i].critic_score), gs[i].name.to_lowercase())),
            Sort::CommunityScore => v.sort_by_cached_key(|&i| (std::cmp::Reverse(gs[i].community_score), gs[i].name.to_lowercase())),
            Sort::PlayCount => v.sort_by_cached_key(|&i| (std::cmp::Reverse(gs[i].play_count), std::cmp::Reverse(gs[i].playtime_min))),
            Sort::Completion => v.sort_by_cached_key(|&i| {
                let c = completion_of(&gs[i]);
                let rank = COMPLETION_ORDER.iter().position(|s| *s == c).unwrap_or(COMPLETION_ORDER.len());
                (rank, c.to_string(), gs[i].name.to_lowercase())
            }),
        }
        self.view = v;
        self.view_key = Some(key);
    }

    fn apply(&mut self, a: Action) {
        let r: anyhow::Result<()> = (|| {
            match a {
                Action::Play(id) => {
                    let Some(g) = self.game(id).cloned() else { return Ok(()) };
                    if self.running().contains(&id) {
                        return Ok(());
                    }
                    self.workers.play(g)?;
                }
                Action::Edit(id) => {
                    if let Some(g) = self.game(id) {
                        self.editor = Some(Editor::from_game(g));
                        self.ed_lists = editor_facets(&self.games);
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
                Action::Stop(id) => self.confirm_stop = Some(id),
                Action::PickArt(id, kind) => {
                    if let Some(g) = self.game(id) {
                        let key = self.cfg.read().unwrap().steamgriddb_key.clone();
                        self.picker = Some(Picker::open(&self.ctx_handle, g, kind, &key, self.paths.clone()));
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = r {
            self.toasts.error(format!("{e:#}"));
        }
    }

    // ---------- panels ----------

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            theme::logo(ui);
            ui.add_space(14.0);
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
            if let Some(p) = facet_picker(ui, "platform", "All platforms", &self.platform, &mut self.platform_query, &self.platforms) {
                self.platform = p;
            }
            if let Some(t) = facet_picker(ui, "tag", "All tags", &self.tag, &mut self.tag_query, &self.tags) {
                self.tag = t;
            }
            if let Some(c) = facet_picker(ui, "completion", "Any status", &self.completion, &mut self.completion_query, &self.completions) {
                self.completion = c;
            }
            egui::ComboBox::from_id_salt("sort")
                .selected_text(SORT_LABELS.iter().find(|(x, _)| *x == self.sort).map_or("", |(_, l)| *l))
                .height(400.0)
                .show_ui(ui, |ui| {
                    for (x, l) in SORT_LABELS {
                        ui.selectable_value(&mut self.sort, x, l);
                    }
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
                    self.editor = Some(Editor::new_custom());
                    self.ed_lists = editor_facets(&self.games);
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
            let imp = &self.workers.importing;
            let total = imp.1.load(Ordering::Relaxed);
            if total > 0 {
                ui.spinner();
                let done = imp.0.load(Ordering::Relaxed);
                ui.label(RichText::new(format!("Importing from Playnite: {done}/{total} images")).color(TEXT_DIM));
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

        let running = self.running();
        let games = &self.games;
        let view = &self.view;
        let thumbs = &mut self.thumbs;
        let selected = &mut self.selected;
        let opened_at = &mut self.opened_at;
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

                    // Lift hovered cards slightly, with a soft accent glow.
                    let cover = if hovered { cover.translate(vec2(0.0, -3.0)) } else { cover };
                    if hovered || is_sel {
                        let glow = egui::epaint::Shadow { offset: [0, 4], blur: 22, spread: 1, color: ACCENT.gamma_multiply(0.32) };
                        painter.add(glow.as_shape(cover, r));
                    }
                    match thumbs.get(g.id, g.has_cover) {
                        Some(t) => {
                            egui::Image::from_texture(t).corner_radius(r).paint_at(ui, cover);
                        }
                        None => {
                            painter.rect_filled(cover, r, CARD);
                            let lower = Rect::from_min_max(pos2(cover.left(), cover.center().y), cover.max);
                            paint_vgradient(painter, lower, Color32::TRANSPARENT, CARD_HI);
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
                    if running.contains(&g.id) {
                        badge(painter, cover.left_top() + vec2(8.0, 8.0), "RUNNING", GOOD);
                    }
                    if is_sel || hovered {
                        let s = if is_sel { Stroke::new(2.5, ACCENT) } else { Stroke::new(1.5, ACCENT.gamma_multiply(0.8)) };
                        painter.rect_stroke(cover, r, s, StrokeKind::Outside);
                    }
                    // Platform pill right of the title (Steam, PS5, Switch…).
                    let title_w = {
                        let tag = painter.layout_no_wrap(short_platform(g), FontId::proportional(11.5), TEXT_DIM);
                        let pill = Rect::from_min_size(
                            pos2(x + card_w - tag.size().x - 10.0, cover.bottom() + 6.0),
                            tag.size() + vec2(10.0, 4.0),
                        );
                        painter.rect_filled(pill, CornerRadius::same(4), CARD);
                        painter.galley(pill.min + vec2(5.0, 2.0), tag, TEXT_DIM);
                        pill.left() - x - 6.0
                    };
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        g.name.clone(),
                        FontId::proportional(13.5),
                        if is_sel { Color32::WHITE } else { Color32::from_gray(200) },
                    );
                    job.wrap = egui::text::TextWrapping::truncate_at_width(title_w.max(20.0));
                    let galley = painter.layout_job(job);
                    painter.galley(pos2(x, cover.bottom() + 6.0), galley, Color32::WHITE);

                    if resp.clicked() {
                        *selected = Some(g.id);
                        *opened_at = ui.input(|i| i.time);
                    }
                    if resp.double_clicked() {
                        actions.push(Action::Play(g.id));
                    }
                    resp.context_menu(|ui| context_menu(ui, g, actions));
                }
            }
        });
    }

    fn details(&mut self, ui: &mut Ui, id: i64, actions: &mut Vec<Action>) {
        let Some(g) = self.game(id).cloned() else { return };
        let is_running = self.running().contains(&id);
        let mut pick_tag: Option<String> = None;

        // Load banner off-thread on selection change.
        if self.hero.as_ref().map(|(h, _)| *h) != Some(id) {
            self.hero = Some((id, None));
            let path = if g.has_hero { self.paths.hero(id) } else { self.paths.cover(id) };
            let tx = self.hero_tx.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                let _ = tx.send((id, thumbs::load_large(&path, 1920)));
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

        // One scroll for the whole page, so long descriptions and clips get the full height.
        ScrollArea::vertical().id_salt(("details", id)).auto_shrink([false, false]).show(ui, |ui| {
            let pad = 28.0;
            let w = ui.available_width();
            let banner_h = (w * 0.30).clamp(200.0, 380.0);
            let banner = Rect::from_min_size(ui.cursor().min, vec2(w, banner_h));
            ui.allocate_rect(banner, Sense::hover());
            let top_r = CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 };
            match self.hero.as_ref().and_then(|(_, t)| t.as_ref()) {
                Some(t) => {
                    let [iw, ih] = t.size();
                    egui::Image::from_texture(t)
                        .uv(cover_uv(iw as f32 / ih as f32, banner.aspect_ratio()))
                        .corner_radius(top_r)
                        .paint_at(ui, banner);
                }
                None => {
                    ui.painter().rect_filled(banner, top_r, CARD);
                }
            }
            // Fade the banner's lower part into the panel so the title stays readable.
            let fade = Rect::from_min_max(pos2(banner.left(), banner.bottom() - banner_h * 0.6), banner.right_bottom());
            paint_vgradient(ui.painter(), fade, Color32::TRANSPARENT, theme::PANEL);
            close_button(ui, banner.right_top() + vec2(-24.0, 24.0), &mut self.selected);

            // Cover overlaps the banner's bottom edge.
            let cover_w = 190.0;
            let cover = Rect::from_min_size(
                pos2(banner.left() + pad, banner.bottom() - cover_w * 1.5 * 0.55),
                vec2(cover_w, cover_w * 1.5),
            );
            let shadow = egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(170) };
            ui.painter().add(shadow.as_shape(cover, CornerRadius::same(8)));
            match self.thumbs.get(id, g.has_cover) {
                Some(t) => {
                    egui::Image::from_texture(t).corner_radius(CornerRadius::same(8)).paint_at(ui, cover);
                }
                None => {
                    ui.painter().rect_filled(cover, CornerRadius::same(8), CARD);
                }
            }
            // Art pickers appear on hover: one for the cover, one for the banner.
            if ui.rect_contains_pointer(cover) {
                if art_pill(ui, pos2(cover.center().x, cover.bottom() - 26.0), "Change cover", "pick_cover") {
                    actions.push(Action::PickArt(id, ArtKind::Cover));
                }
            } else if ui.rect_contains_pointer(banner) {
                if art_pill(ui, banner.right_top() + vec2(-112.0, 24.0), "Change banner", "pick_banner") {
                    actions.push(Action::PickArt(id, ArtKind::Banner));
                }
            }

            // Title sits on the faded banner, right of the cover.
            let text_x = cover.right() + 24.0;
            let title_w = banner.right() - pad - text_x;
            let mut job = egui::text::LayoutJob::simple(g.name.clone(), FontId::proportional(30.0), Color32::WHITE, title_w);
            job.wrap.max_rows = 2;
            let galley = ui.painter().layout_job(job);
            let tpos = pos2(text_x, banner.bottom() - galley.size().y - 14.0);
            ui.painter().galley_with_override_text_color(tpos + vec2(0.0, 2.0), galley.clone(), Color32::from_black_alpha(200));
            ui.painter().galley(tpos, galley, Color32::WHITE);

            // Right of the cover: actions, stat tiles, links.
            let header_bottom = cover.bottom() + 20.0;
            ui.horizontal(|ui| {
                ui.add_space(text_x - banner.left());
                ui.vertical(|ui| {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        if is_running {
                            let b = egui::Button::new(RichText::new("Running").size(17.0).strong().color(Color32::WHITE))
                                .fill(GOOD)
                                .min_size(vec2(150.0, 38.0));
                            if ui.add(b).on_hover_text("This game is running. Click to close it.").clicked() {
                                actions.push(Action::Stop(id));
                            }
                        } else {
                            let label = if g.source == Source::Steam && !g.installed { "Install" } else { "Play" };
                            let play = egui::Button::new(RichText::new(label).size(17.0).strong().color(Color32::WHITE))
                                .fill(ACCENT)
                                .min_size(vec2(150.0, 38.0));
                            let can_play = g.source == Source::Steam || g.exe.is_some();
                            let r = ui.add_enabled(can_play, play).on_disabled_hover_text("No way to start this game on this PC");
                            if r.clicked() {
                                actions.push(Action::Play(id));
                            }
                        }
                        let fav = ui
                            .add(egui::Button::new("").min_size(vec2(38.0, 38.0)))
                            .on_hover_text(if g.favorite { "Unfavorite" } else { "Favorite" });
                        paint_star(ui.painter(), fav.rect.center(), 10.0, g.favorite);
                        if fav.clicked() {
                            actions.push(Action::ToggleFav(id));
                        }
                        if ui.add(egui::Button::new("Edit").min_size(vec2(0.0, 38.0))).clicked() {
                            actions.push(Action::Edit(id));
                        }
                        ui.menu_button("More", |ui| context_menu(ui, &g, actions))
                            .response
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                    });
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let status = match (g.source, g.installed) {
                            (Source::Steam, true) => "Installed",
                            (Source::Steam, false) => "Not installed",
                            (Source::Custom, true) => "Ready",
                            (Source::Custom, false) => "Not installed",
                        };
                        let status = if is_running { "Running" } else { status };
                        stat(ui, "LAST PLAYED", &util::date(g.last_played));
                        stat(ui, "TIME PLAYED", &util::playtime(g.playtime_min));
                        stat(ui, "ADDED", &util::date(g.added));
                        stat(ui, "STATUS", status);
                        if let Some(c) = &g.completion {
                            stat(ui, "COMPLETION", c);
                        }
                        if g.play_count > 0 {
                            stat(ui, "PLAY COUNT", &g.play_count.to_string());
                        }
                    });
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        for (label, url) in links(&g) {
                            let b = egui::Button::new(RichText::new(label).size(12.5)).corner_radius(CornerRadius::same(12));
                            if ui.add(b).on_hover_text(&url).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                            }
                        }
                    });
                });
            });
            let y = ui.cursor().min.y;
            if y < header_bottom {
                ui.add_space(header_bottom - y);
            }
            ui.add_space(20.0);

            // Lower area: details on the left, description on the right; the whole page scrolls.
            ui.horizontal_top(|ui| {
                ui.add_space(pad);
                let left_w = cover_w + 40.0;
                ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(left_w);
                    ui.vertical(|ui| {
                        theme::section(ui, "Details");
                        let source = match (g.source, &g.store) {
                            (Source::Steam, _) => "Steam",
                            (_, Some(s)) => s.as_str(),
                            _ => "Custom",
                        };
                        fact(ui, "Source", source);
                        let scores = match (g.critic_score, g.community_score) {
                            (Some(c), Some(u)) => Some(format!("Critics {c} · Users {u}")),
                            (Some(c), None) => Some(format!("Critics {c}")),
                            (None, Some(u)) => Some(format!("Users {u}")),
                            _ => None,
                        };
                        let released = g.release_date.as_deref().map(util::release);
                        for (k, v) in [
                            ("Platform", &g.platform),
                            ("Developer", &g.developer),
                            ("Publisher", &g.publisher),
                            ("Released", &released),
                            ("Genres", &g.genres),
                            ("Series", &g.series),
                            ("Score", &scores),
                        ] {
                            if let Some(v) = v {
                                fact(ui, k, v);
                            }
                        }
                        if let Some(a) = g.steam_appid() {
                            fact(ui, "Steam ID", &a.to_string());
                        }
                        if let Some(d) = g.install_dir.as_deref().or(g.exe.as_deref()) {
                            ui.label(theme::caption("Location"));
                            ui.add(egui::Label::new(d).truncate()).on_hover_text(d);
                            ui.add_space(6.0);
                        }
                        if g.tags.is_some() {
                            ui.label(theme::caption("Tags"));
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                for t in split_list(&g.tags) {
                                    let b = egui::Button::new(RichText::new(t).size(12.5))
                                        .fill(CARD)
                                        .corner_radius(CornerRadius::same(10));
                                    if ui.add(b).on_hover_text("Show games with this tag").clicked() {
                                        pick_tag = Some(t.to_string());
                                    }
                                }
                            });
                        }
                    });
                });
                ui.add_space(32.0);
                let right_w = ui.available_width() - pad;
                ui.allocate_ui_with_layout(vec2(right_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(right_w);
                    theme::section(ui, "Description");
                    ui.vertical(|ui| {
                        match &g.description {
                            Some(d) => {
                                description(ui, d);
                                ui.response()
                            }
                            None if g.meta_state == state::PENDING => {
                                ui.label(RichText::new("Fetching metadata…").color(TEXT_DIM))
                            }
                            None => ui.label(RichText::new("No description.").color(TEXT_DIM)),
                        };
                    });
                });
            });
            ui.add_space(28.0);
        });
        if let Some(t) = pick_tag {
            self.tag = t;
            self.selected = None;
        }
    }

    // ---------- dialogs ----------

    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.editor.as_mut() else { return };
        let lists = Lists { platforms: &self.platforms, tags: &self.tags, genres: &self.ed_lists.0, completions: &self.ed_lists.1 };
        match ed.show(ctx, &lists) {
            EditorOutcome::Open => {}
            EditorOutcome::Cancel => self.editor = None,
            EditorOutcome::Save => {
                if let Err(e) = self.save_editor() {
                    self.toasts.error(format!("Saving failed: {e:#}"));
                }
                self.editor = None;
            }
            EditorOutcome::PickArt(id, kind) => self.apply(Action::PickArt(id, kind)),
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
        ed.apply_to(&mut g);
        self.lib.update_user_fields(&g)?;
        self.toasts.success(format!("Saved {}.", g.name));
        // New tags/platforms should show up in the filters right away.
        self.reload_one(id);
        (self.platforms, self.tags) = facets(&self.games);
        self.completions = completion_facets(&self.games);
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
        let mut preview: Option<Option<std::path::PathBuf>> = None;
        let mut sk_install = false;
        let mut rs_download = false;
        let mut pn_desc = false;
        let importing = self.workers.importing.1.load(Ordering::Relaxed) > 0;
        let detected = gamut_core::steam::steam_path()
            .and_then(|p| gamut_core::steam::detect_steam_id(&p))
            .unwrap_or_default();
        let mut tab = self.settings_tab;
        let screen = ctx.content_rect();
        let size = vec2(680.0_f32.min(screen.width() - 80.0), 600.0_f32.min(screen.height() - 120.0));
        theme::dialog(ctx, "Settings", false, |ui| {
                ui.set_min_size(size);
                ui.set_max_size(size);
                dialog_header(ui, "Settings", &mut open);
                ui.horizontal(|ui| {
                    for (t, label) in SETTINGS_TABS.iter().enumerate() {
                        ui.selectable_value(&mut tab, t, RichText::new(*label).size(14.5));
                    }
                });
                ui.separator();
                let body_h = ui.available_height() - 48.0;
                egui::ScrollArea::vertical().id_salt("settings_body").max_height(body_h).auto_shrink([false, false]).show(ui, |ui| {
                    match tab {
                        0 => {
                        theme::section(ui, "Steam");
                        egui::Grid::new("st").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            ui.label("Web API key");
                            ui.add(egui::TextEdit::singleline(&mut s.steam_api_key).password(true).desired_width(420.0));
                            ui.end_row();
                            ui.label("SteamID64");
                            ui.add(
                                egui::TextEdit::singleline(&mut s.steam_id)
                                    .hint_text(format!("auto: {detected}"))
                                    .desired_width(420.0),
                            );
                            ui.end_row();
                        });
                        ui.hyperlink_to("Get a Steam Web API key", "https://steamcommunity.com/dev/apikey");
                        ui.label(theme::hint("Your profile's game details must be public, or the key must belong to this account."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "SteamGridDB");
                        egui::Grid::new("sg").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            ui.label("API key");
                            ui.add(egui::TextEdit::singleline(&mut s.steamgriddb_key).password(true).desired_width(420.0));
                            ui.end_row();
                        });
                        ui.hyperlink_to("Get a SteamGridDB key", "https://www.steamgriddb.com/profile/preferences/api");
                        ui.label(theme::hint("Used for covers and banners Steam doesn't have."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "Playnite");
                        ui.label(theme::hint("Copies games, playtime, favorites, covers and launch settings. Close Playnite first."));
                        ui.horizontal(|ui| {
                            if ui.add_enabled(!importing, egui::Button::new("Import from Playnite…")).clicked() {
                                preview = Some(None);
                            }
                            let pick = ui
                                .add_enabled(!importing, egui::Button::new("Choose folder…"))
                                .on_hover_text("For portable Playnite: the folder that contains \"library\"");
                            if pick.clicked() {
                                if let Some(d) = rfd::FileDialog::new().set_title("Playnite folder").pick_folder() {
                                    preview = Some(Some(d));
                                }
                            }
                            if ui
                                .add_enabled(!importing, egui::Button::new("Update descriptions only"))
                                .on_hover_text("Re-reads just the descriptions (with their images) from Playnite. Nothing else changes; descriptions you edited in Gamut are kept.")
                                .clicked()
                            {
                                pn_desc = true;
                            }
                        });
                        }
                        1 => {
                        theme::section(ui, "Special K");
                        let sk = &mut s.specialk;
                        egui::Grid::new("sk").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            const MODES: [(&str, &str); 3] =
                                [("off", "Off"), ("global", "Every game (unless turned off)"), ("selective", "Only games turned on")];
                            const STOPS: [(&str, &str); 3] =
                                [("injection", "Once it's in the game"), ("exit", "When the game exits"), ("never", "Never")];
                            ui.label("Inject into");
                            combo(ui, "sk_mode", &mut sk.mode, &MODES);
                            ui.end_row();
                            ui.label("Stop service");
                            combo(ui, "sk_stop", &mut sk.stop, &STOPS);
                            ui.end_row();
                            ui.label("Folder");
                            let found = gamut_core::specialk::install_dir("").map(|p| p.display().to_string());
                            ui.add(
                                egui::TextEdit::singleline(&mut sk.path)
                                    .hint_text(found.map_or("not found".into(), |p| format!("auto: {p}")))
                                    .desired_width(420.0),
                            );
                            ui.end_row();
                        });
                        ui.checkbox(&mut sk.skip_anticheat, "Skip games with EasyAntiCheat or BattlEye");
                        ui.label(theme::hint("Starts Special K's injection service before a game launches. Set a game to always/never in Edit → Launch."));
                        ui.add_space(6.0);
                        ui.label(RichText::new("Defaults for new game profiles").strong());
                        ui.checkbox(&mut sk.steam_overlay, "Load the Steam overlay (writes steam_appid.txt for non-Steam games)");
                        ui.checkbox(&mut sk.reshade, "Enable ReShade (with a preset per game)");
                        ui.checkbox(&mut sk.flip_model, "Use the flip model presentation");
                        ui.checkbox(&mut sk.disable_nvidia_bloat, "Disable NVIDIA bloatware (overlay, telemetry)");
                        ui.horizontal(|ui| {
                            ui.label("Frame rate limit");
                            ui.add(egui::DragValue::new(&mut sk.fps_limit).range(0.0..=1000.0).speed(1.0).suffix(" fps"));
                            ui.label(theme::hint("0 = none"));
                        });
                        ui.checkbox(&mut sk.bpm_overlay, "Big Picture overlay for non-Steam games while Steam is in Big Picture");
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut sk.check_updates, "Check for updates on");
                            let mut ch = sk.channel.clone();
                            let chans: Vec<(&str, &str)> = gamut_core::specialk::update::CHANNELS.iter().map(|c| (*c, *c)).collect();
                            combo(ui, "sk_chan", &mut ch, &chans);
                            sk.channel = ch;
                        });
                        let installing = self.workers.sk_installing.load(Ordering::Relaxed);
                        if let Some(u) = self.workers.sk_update.lock().unwrap().clone() {
                            ui.horizontal(|ui| {
                                ui.label(format!("Special K {} is available (installed {}).", u.latest, u.current));
                                let b = ui
                                    .add_enabled(!installing, egui::Button::new(if installing { "Installing…" } else { "Install update" }))
                                    .on_hover_text(u.notes.chars().take(1500).collect::<String>());
                                if b.clicked() {
                                    sk_install = true;
                                }
                            });
                        }
                        }
                        2 => {
                        theme::section(ui, "ReShade");
                        ui.checkbox(&mut s.reshade.enabled, "Install ReShade into games when they first start");
                        ui.checkbox(&mut s.reshade.skip_anticheat, "Skip games with EasyAntiCheat or BattlEye");
                        ui.label(theme::hint(
                            "Like ReShade's installer, with Standard effects + SweetFX, all effects off; press Home in game. \
                             Games that already have ReShade (or another dxgi/d3d9 mod) are left alone. Per game: Edit → Launch.",
                        ));
                        let cache = self.workers.reshade_cache();
                        ui.horizontal(|ui| {
                            match gamut_core::reshade::version(&cache) {
                                Some(v) => ui.label(format!("Downloaded: ReShade {v}")),
                                None => ui.label(theme::hint("Not downloaded yet (happens on first use).")),
                            };
                            if ui.button("Download latest").clicked() {
                                rs_download = true;
                            }
                        });
                        }
                        _ => {
                        ui.label(theme::hint(&format!("Data folder: {}", self.paths.root.display())));
                        ui.horizontal(|ui| {
                            if ui.button("Open log").on_hover_text("What Gamut did this run, including errors").clicked() {
                                let _ = open::that(crate::logging::path(&self.paths.root));
                            }
                            if ui.button("Open data folder").clicked() {
                                let _ = open::that(&self.paths.root);
                            }
                        });
                        }
                    }
                });
                ui.separator();
                ui.add_space(8.0);
                if ui.add(egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    save = true;
                }
            });
        self.settings_tab = tab;
        if let Some(dir) = preview {
            self.workers.playnite_preview(dir);
        }
        if sk_install {
            self.workers.install_sk_update();
        }
        if rs_download {
            self.workers.reshade_download(true);
        }
        if pn_desc {
            self.workers.playnite_descriptions(None);
        }
        if save {
            let new = self.settings.take().unwrap();
            let old = self.cfg.read().unwrap().clone();
            *self.cfg.write().unwrap() = new.clone();
            if let Err(e) = new.save(&self.paths.config) {
                self.toasts.error(format!("Saving settings failed: {e:#}"));
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
        let (mut close, mut open) = (false, true);
        theme::dialog(ctx, "Remove game", false, |ui| {
                ui.set_max_width(460.0);
                dialog_header(ui, "Remove game", &mut open);
                ui.label(format!("Remove \"{name}\" from the library? The game's files aren't touched."));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        match self.lib.delete(id) {
                            Ok(()) => self.toasts.success(format!("Removed {name}.")),
                            Err(e) => self.toasts.error(format!("{e:#}")),
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
        if close || !open {
            self.confirm_remove = None;
        }
    }

    /// "Close game?" for a running game: politely (WM_CLOSE) or by force.
    fn stop_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_stop else { return };
        let Some(g) = self.game(id).cloned() else {
            self.confirm_stop = None;
            return;
        };
        let (mut close, mut open) = (false, true);
        let mut act: Option<bool> = None;
        theme::dialog(ctx, "Close game", false, |ui| {
            ui.set_max_width(460.0);
            dialog_header(ui, "Close game", &mut open);
            ui.label(format!("Close \"{}\"? Unsaved progress may be lost.", g.name));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Close game").on_hover_text("Asks the game to quit, like clicking its X").clicked() {
                    act = Some(false);
                }
                if ui
                    .add(egui::Button::new(RichText::new("Force close").color(Color32::WHITE)).fill(theme::DANGER))
                    .on_hover_text("Ends the game's processes immediately")
                    .clicked()
                {
                    act = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if let Some(force) = act {
            let pids: Vec<u32> = gamut_core::procs::under(&gamut_core::procs::game_dirs(&g)).into_iter().map(|(p, _)| p).collect();
            if pids.is_empty() {
                self.toasts.error(format!("Couldn't find {}'s processes.", g.name));
            } else if force {
                let n = gamut_core::procs::kill(&pids);
                log::info!("force closed {} ({n}/{} processes)", g.name, pids.len());
                self.toasts.success(format!("Closed {}.", g.name));
            } else if gamut_core::procs::close(&pids) == 0 {
                self.toasts.error(format!("{} has no window to close; use Force close.", g.name));
            } else {
                log::info!("asked {} to close", g.name);
            }
            close = true;
        }
        if close || !open {
            self.confirm_stop = None;
        }
    }

    fn playnite_window(&mut self, ctx: &egui::Context) {
        let Some((dir, sum)) = self.playnite.as_ref() else { return };
        let mut open = true;
        let (mut go, mut cancel, mut ask, mut back) = (false, false, false, false);
        let sure = self.playnite_sure;
        theme::dialog(ctx, "Import from Playnite", false, |ui| {
                ui.set_width(520.0);
                dialog_header(ui, "Import from Playnite", &mut open);
                ui.label(theme::hint(&dir.display().to_string()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    stat(ui, "Games", &sum.games.to_string());
                    stat(ui, "Covers", &sum.covers.to_string());
                    stat(ui, "Backgrounds", &sum.backgrounds.to_string());
                });
                ui.add_space(6.0);
                ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                    let table = |ui: &mut Ui, title: &str, rows: &[(String, usize)]| {
                        if rows.is_empty() {
                            return;
                        }
                        theme::section(ui, title);
                        egui::Grid::new(title).num_columns(2).striped(true).spacing([24.0, 6.0]).show(ui, |ui| {
                            for (k, n) in rows {
                                ui.add(egui::Label::new(k).wrap());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(RichText::new(n.to_string()).strong()));
                                ui.end_row();
                            }
                        });
                        ui.add_space(8.0);
                    };
                    table(ui, "Can be started", &sum.playable);
                    table(ui, "Imported without a way to start", &sum.unplayable);
                    table(ui, "Why some can't start", &sum.reasons);
                });
                ui.add_space(6.0);
                ui.label(theme::hint(
                    "Steam games merge into your existing ones (playtime and last played keep the higher value, \
                     favorites and hidden are added). Playnite's covers and backgrounds replace Gamut's. \
                     Importing again later updates the same games instead of duplicating them.",
                ));
                ui.add_space(10.0);
                if sure {
                    ui.label(
                        RichText::new(
                            "Are you sure? For every matching game, Playnite's description, tags, genres, developer, \
                             publisher, release date, series, scores and completion replace Gamut's, including \
                             anything you edited here. Covers and backgrounds are replaced too, and so are launch \
                             settings for non-Steam games. This can't be undone.",
                        )
                        .color(theme::DANGER),
                    );
                    ui.add_space(8.0);
                }
                ui.horizontal(|ui| {
                    if sure {
                        if ui.add(egui::Button::new(RichText::new("Yes, overwrite").color(Color32::WHITE)).fill(theme::DANGER)).clicked() {
                            go = true;
                        }
                        if ui.button("Back").clicked() {
                            back = true;
                        }
                    } else {
                        if ui.add(egui::Button::new(RichText::new("Import").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                            ask = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel = true;
                        }
                    }
                });
            });
        if ask {
            self.playnite_sure = true;
        } else if back {
            self.playnite_sure = false;
        } else if go {
            let dir = dir.clone();
            self.workers.playnite_import(dir);
            self.playnite = None;
            self.playnite_sure = false;
            self.settings = None;
        } else if cancel || !open {
            self.playnite = None;
            self.playnite_sure = false;
        }
    }
}

impl GamutApp {
    /// Remembers the normal (un-maximized) size/position and whether the window is maximized.
    fn track_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, maximized, minimized) = ctx.input(|i| {
            let v = i.viewport();
            (v.inner_rect, v.outer_rect, v.maximized.unwrap_or(false), v.minimized.unwrap_or(false))
        });
        if minimized {
            return;
        }
        if !maximized {
            if let (Some(inner), Some(outer)) = (inner, outer) {
                self.window = Some(gamut_core::config::WindowState {
                    x: outer.min.x,
                    y: outer.min.y,
                    width: inner.width(),
                    height: inner.height(),
                    maximized: false,
                });
            }
        }
        match self.window.as_mut() {
            Some(w) => w.maximized = maximized,
            // Maximized since the first launch: remember that, with the default normal size.
            None if maximized => {
                self.window = Some(gamut_core::config::WindowState {
                    x: 100.0,
                    y: 100.0,
                    width: 1440.0,
                    height: 880.0,
                    maximized: true,
                })
            }
            None => {}
        }
    }

    fn save_prefs(&self) {
        let mut c = self.cfg.write().unwrap();
        c.sort = key_of(&SORTS, self.sort).into();
        c.filter = key_of(&FILTERS, self.filter).into();
        c.platform = self.platform.clone();
        c.tag = self.tag.clone();
        c.completion = self.completion.clone();
        c.window = self.window;
        let _ = c.save_ui_prefs(&self.paths.config);
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
    item(ui, "Change cover…", Action::PickArt(id, ArtKind::Cover));
    item(ui, "Change banner…", Action::PickArt(id, ArtKind::Banner));
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

/// Small translucent button centred at `c`; true when clicked.
fn art_pill(ui: &mut Ui, c: egui::Pos2, text: &str, salt: &str) -> bool {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), Color32::WHITE);
    let rect = Rect::from_center_size(c, galley.size() + vec2(24.0, 14.0));
    let r = ui.interact(rect, ui.scope_id().with(salt), Sense::click());
    let bg = if r.hovered() { ACCENT } else { Color32::from_black_alpha(170) };
    ui.painter().rect_filled(rect, CornerRadius::same(16), bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, Color32::WHITE);
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r.clicked()
}

/// Dialog title row with a close button (also Esc); clears `open` when used.
pub(crate) fn dialog_header(ui: &mut Ui, title: &str, open: &mut bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(17.0).color(Color32::WHITE));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = theme::x_button(ui, 28.0).on_hover_text("Close (Esc)");
            if r.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                *open = false;
            }
        });
    });
    ui.add_space(4.0);
}

/// Round close "X" button; clears `selected` when clicked.
fn close_button(ui: &mut Ui, c: egui::Pos2, selected: &mut Option<i64>) {
    let rect = Rect::from_center_size(c, vec2(32.0, 32.0));
    let r = ui.interact(rect, ui.scope_id().with("close_details"), Sense::click()).on_hover_text("Close (Esc)");
    let bg = if r.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(140) };
    ui.painter().circle_filled(c, 16.0, bg);
    let st = Stroke::new(2.0, Color32::WHITE);
    ui.painter().line_segment([c + vec2(-5.5, -5.5), c + vec2(5.5, 5.5)], st);
    ui.painter().line_segment([c + vec2(-5.5, 5.5), c + vec2(5.5, -5.5)], st);
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if r.clicked() {
        *selected = None;
    }
}

/// Vertical gradient from `top` to `bottom` color.
fn paint_vgradient(p: &egui::Painter, r: Rect, top: Color32, bottom: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(r.left_top(), top);
    m.colored_vertex(r.right_top(), top);
    m.colored_vertex(r.left_bottom(), bottom);
    m.colored_vertex(r.right_bottom(), bottom);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 2, 3);
    p.add(m);
}

/// Caption over a value, used for stat tiles.
fn stat(ui: &mut Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(theme::caption(label));
                ui.label(RichText::new(value).size(15.0).strong());
            });
        });
}

/// Caption + wrapped value in the details column.
fn fact(ui: &mut Ui, k: &str, v: &str) {
    ui.spacing_mut().item_spacing.y = 3.0;
    ui.label(theme::caption(k));
    ui.add(egui::Label::new(v).wrap());
    ui.add_space(8.0);
}

/// Small filled pill with uppercase text.
fn badge(p: &egui::Painter, at: egui::Pos2, text: &str, fill: Color32) {
    let galley = p.layout_no_wrap(text.into(), FontId::proportional(10.5), Color32::WHITE);
    let rect = Rect::from_min_size(at, galley.size() + vec2(14.0, 6.0));
    p.rect_filled(rect, CornerRadius::same(9), fill);
    p.galley(rect.min + vec2(7.0, 3.0), galley, Color32::WHITE);
}

/// Items of a comma-joined list column.
/// Type-to-filter field with a suggestion list; returns the picked value ("" = all).
fn facet_picker(ui: &mut Ui, id: &str, all: &str, current: &str, query: &mut String, items: &[(String, usize)]) -> Option<String> {
    use egui::containers::{Popup, PopupCloseBehavior, SetOpenCommand};
    let te_id = ui.make_persistent_id(id);
    let pid = te_id.with("popup");
    let editing = ui.memory(|m| m.has_focus(te_id)) || Popup::is_id_open(ui.ctx(), pid);
    let mut text = if editing { query.clone() } else { current.to_string() };
    let hint = if current.is_empty() { all } else { current };
    let r = ui.add(egui::TextEdit::singleline(&mut text).id(te_id).hint_text(hint).desired_width(160.0));
    if r.gained_focus() {
        text.clear();
    }
    *query = text;
    let q = query.trim().to_lowercase();
    let mut hits: Vec<&(String, usize)> = items.iter().filter(|(t, _)| t.to_lowercase().contains(&q)).collect();
    hits.sort_by_key(|(t, _)| !t.to_lowercase().starts_with(&q));

    let mut pick = None;
    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        pick = Some(if q.is_empty() { String::new() } else { hits.first().map_or(current.to_string(), |(t, _)| t.clone()) });
    }
    let open = (r.gained_focus() || r.changed()).then_some(SetOpenCommand::Bool(true));
    Popup::from_response(&r)
        .id(pid)
        .open_memory(open)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .width(r.rect.width().max(260.0))
        .show(|ui| {
            egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                if q.is_empty() && ui.selectable_label(current.is_empty(), all).clicked() {
                    pick = Some(String::new());
                }
                for (t, n) in hits.iter().take(200) {
                    if ui.selectable_label(t == current, format!("{t}  ·  {n}")).clicked() {
                        pick = Some(t.clone());
                    }
                }
                if hits.is_empty() {
                    ui.label(theme::hint("No matches"));
                }
            });
        });
    if !current.is_empty() && !editing && theme::x_button(ui, 22.0).on_hover_text(format!("Show {}", all.to_lowercase())).clicked() {
        pick = Some(String::new());
    }
    if pick.is_some() {
        Popup::close_id(ui.ctx(), pid);
        ui.memory_mut(|m| m.surrender_focus(te_id));
        query.clear();
    }
    pick
}

/// Compact platform label for a card ("PS5", "Switch", "Steam", "GOG").
fn short_platform(g: &Game) -> String {
    if g.source == Source::Steam {
        return "Steam".into();
    }
    let platform = g.platform.as_deref().unwrap_or("PC (Windows)");
    if platform.starts_with("PC") {
        return g.store.clone().unwrap_or_else(|| "PC".into());
    }
    let p = ["Sony ", "Nintendo ", "Microsoft ", "Sega "].iter().fold(platform, |p, v| p.strip_prefix(v).unwrap_or(p));
    match p {
        "PlayStation" => "PS1".into(),
        "PlayStation Vita" => "Vita".into(),
        "Game Boy Advance" => "GBA".into(),
        "Game Boy Color" => "GBC".into(),
        "Game Boy" => "GB".into(),
        "Xbox 360" => "X360".into(),
        _ => p.strip_prefix("PlayStation ").map_or(p.to_string(), |n| format!("PS{n}")),
    }
}

fn split_list(v: &Option<String>) -> impl Iterator<Item = &str> {
    v.as_deref().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// A game's platforms; games without one are PC games.
fn platforms_of(g: &Game) -> impl Iterator<Item = &str> {
    let v: Vec<&str> = split_list(&g.platform).collect();
    let pc = v.is_empty().then_some("PC (Windows)");
    v.into_iter().chain(pc)
}

/// Platforms and tags in the library with game counts, most common first.
fn facets(games: &[Game]) -> (Vec<(String, usize)>, Vec<(String, usize)>) {
    let mut p: std::collections::HashMap<&str, usize> = Default::default();
    let mut t: std::collections::HashMap<&str, usize> = Default::default();
    for g in games.iter().filter(|g| !g.hidden) {
        for x in platforms_of(g) {
            *p.entry(x).or_default() += 1;
        }
        for x in split_list(&g.tags) {
            *t.entry(x).or_default() += 1;
        }
    }
    let sorted = |m: std::collections::HashMap<&str, usize>, by_count: bool| {
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        if by_count {
            v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        } else {
            v.sort_by_key(|(k, _)| k.to_lowercase());
        }
        v
    };
    (sorted(p, true), sorted(t, false))
}

/// Completion statuses with counts, in grid order.
fn completion_facets(games: &[Game]) -> Vec<(String, usize)> {
    let mut m: std::collections::HashMap<&str, usize> = Default::default();
    for g in games.iter().filter(|g| !g.hidden) {
        *m.entry(completion_of(g)).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
    v.sort_by_key(|(k, _)| (COMPLETION_ORDER.iter().position(|s| s == k).unwrap_or(99), k.to_lowercase()));
    v
}

/// "YYYY-MM-DD"-like key from any stored release text, so dates compare chronologically.
fn sortable_date(s: &str) -> String {
    let d = util::release(s);
    match d.split('/').collect::<Vec<_>>().as_slice() {
        [dd, mm, yyyy] => format!("{yyyy}-{mm}-{dd}"),
        _ => s.chars().filter(|c| c.is_ascii_digit()).take(4).collect(),
    }
}

/// Genre and completion values in the library, for the editor's suggestions.
fn editor_facets(games: &[Game]) -> (Vec<(String, usize)>, Vec<(String, usize)>) {
    let mut genres: std::collections::HashMap<&str, usize> = Default::default();
    let mut comp: std::collections::HashMap<&str, usize> = Default::default();
    for g in games {
        for x in split_list(&g.genres) {
            *genres.entry(x).or_default() += 1;
        }
        if let Some(c) = g.completion.as_deref() {
            *comp.entry(c).or_default() += 1;
        }
    }
    let sorted = |m: std::collections::HashMap<&str, usize>| {
        let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    (sorted(genres), sorted(comp))
}

/// External reference pages for a game (just links, nothing is fetched).
fn links(g: &Game) -> Vec<(&'static str, String)> {
    let q = gamut_core::http::encode(&g.name);
    let mut v = Vec::new();
    if let Some(a) = g.steam_appid() {
        v.push(("Steam store", format!("https://store.steampowered.com/app/{a}")));
        v.push(("PCGamingWiki", format!("https://www.pcgamingwiki.com/api/appid.php?appid={a}")));
        v.push(("SteamDB", format!("https://steamdb.info/app/{a}/")));
        v.push(("ProtonDB", format!("https://www.protondb.com/app/{a}")));
    } else {
        v.push(("PCGamingWiki", format!("https://www.pcgamingwiki.com/w/index.php?search={q}")));
    }
    v.push(("HowLongToBeat", format!("https://howlongtobeat.com/?q={q}")));
    v.push(("IGDB", format!("https://www.igdb.com/search?q={q}")));
    v
}

/// UV rect that crops an image of aspect `img` to fill a rect of aspect `rect`.
pub(crate) fn cover_uv(img: f32, rect: f32) -> Rect {
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

        let top = egui::Panel::top("top")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(18, 10)))
            .show(ui, |ui| self.top_bar(ui));
        let line = Rect::from_min_size(top.response.rect.left_bottom() - vec2(0.0, 1.0), vec2(top.response.rect.width(), 1.5));
        theme::hgradient(ui.painter(), line, ACCENT.gamma_multiply(0.7), theme::ACCENT_2.gamma_multiply(0.15));
        egui::Panel::bottom("status")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(18, 5)))
            .show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::central_panel(ui.style()).fill(theme::BG).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| self.grid(ui, &mut actions));

        if let Some(s) = self.startup.as_mut() {
            if s.tick(&ctx) {
                self.startup = None;
            }
        }
        let any_popup = self.selected.is_some()
            || self.editor.is_some()
            || self.settings.is_some()
            || self.confirm_remove.is_some()
            || self.confirm_stop.is_some()
            || self.playnite.is_some()
            || self.picker.is_some();
        if any_popup {
            theme::paint_backdrop(&ctx);
        }
        // Other dialogs would sit under the modal, so it steps aside while they are open.
        let dialog_open =
            self.editor.is_some()
            || self.settings.is_some()
            || self.confirm_remove.is_some()
            || self.confirm_stop.is_some()
            || self.playnite.is_some();
        if let (Some(id), false) = (self.selected, dialog_open) {
            let screen = ctx.content_rect();
            let size = vec2((screen.width() * 0.86).min(1320.0), (screen.height() * 0.88).min(940.0));
            let frame = egui::Frame::new()
                .fill(theme::PANEL)
                .corner_radius(CornerRadius::same(10))
                .shadow(ctx.global_style().visuals.window_shadow)
                .stroke(Stroke::new(1.0, Color32::from_gray(50)));
            let resp = egui::Modal::new(egui::Id::unique("details_modal"))
                .area(theme::no_fade(egui::Id::unique("details_modal")))
                .frame(frame)
                .backdrop_color(Color32::TRANSPARENT)
                .show(&ctx, |ui| {
                    ui.set_min_size(size);
                    ui.set_max_size(size);
                    ui.set_clip_rect(Rect::from_min_size(ui.max_rect().min, size));
                    self.details(ui, id, &mut actions);
                });
            // The popup opens on the first click, so the grid never sees a double-click: catch it here.
            let quick = ctx.input(|i| {
                i.time - self.opened_at < 0.5 && i.pointer.button_double_clicked(egui::PointerButton::Primary)
            });
            if quick {
                actions.push(Action::Play(id));
                self.selected = None;
            } else if resp.should_close() {
                self.selected = None;
            }
        }

        if let Some(p) = self.picker.as_mut() {
            match p.show(&ctx) {
                Outcome::Open => {}
                Outcome::Closed => self.picker = None,
                Outcome::Changed(id, kind) => {
                    self.picker = None;
                    let what = if kind == ArtKind::Cover { "Cover" } else { "Banner" };
                    let name = self.game(id).map(|g| g.name.clone()).unwrap_or_default();
                    self.toasts.success(format!("{what} updated for {name}."));
                    if kind == ArtKind::Banner || self.game(id).is_some_and(|g| !g.has_hero) {
                        self.hero = None;
                    }
                    self.reload_one(id);
                }
            }
        }
        self.editor_window(&ctx);
        self.settings_window(&ctx);
        self.confirm_window(&ctx);
        self.stop_window(&ctx);
        self.playnite_window(&ctx);
        self.toasts.show(&ctx);

        for a in actions {
            self.apply(a);
        }
        self.thumbs.end_frame();
        self.track_window(&ctx);
        let view = (self.sort, self.filter, self.platform.clone(), self.tag.clone(), self.completion.clone());
        if self.saved_view != view {
            self.saved_view = view;
            self.save_prefs();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_prefs();
    }
}

/// Combo box over (stored value, label) pairs.
fn combo(ui: &mut Ui, id: &str, value: &mut String, opts: &[(&str, &str)]) {
    egui::ComboBox::from_id_salt(id)
        .width(320.0)
        .selected_text(opts.iter().find(|o| o.0 == value.as_str()).map_or(value.as_str(), |o| o.1))
        .show_ui(ui, |ui| {
            for (v, l) in opts {
                if ui.selectable_label(value == v, *l).clicked() {
                    *value = v.to_string();
                }
            }
        });
}

/// Description text with its images (`[img]URL` lines kept by `scrape::strip_html`).
fn description(ui: &mut Ui, d: &str) {
    // Markup from `scrape::strip_html`: blank line = paragraph, `## ` heading, `• ` item, `**bold**`.
    use gamut_core::scrape::{BULLET, HEADING};
    ui.spacing_mut().item_spacing.y = 0.0;
    let body = Color32::from_rgb(196, 202, 214);
    let (mut first, mut blank, mut prev_bullet) = (true, false, false);
    for line in d.lines().map(str::trim) {
        if line.is_empty() {
            blank = true;
            continue;
        }
        if let Some(url) = line.strip_prefix(gamut_core::scrape::IMG_MARKER) {
            if !url.starts_with("http") {
                continue;
            }
            let w = ui.available_width().min(900.0);
            let img = egui::Image::new(url.to_string()).corner_radius(6);
            // Exact size from the image itself: in a scroll area egui would otherwise fit it to the
            // visible space left, which is zero below the fold. Broken/unsupported images are skipped.
            let (size, ready) = match img.load_for_size(ui.ctx(), egui::vec2(w, w)) {
                Ok(egui::load::TexturePoll::Ready { texture }) => (Some(texture.size * (w / texture.size.x).min(1.0)), true),
                Ok(egui::load::TexturePoll::Pending { .. }) => (Some(egui::vec2(w.min(640.0), w.min(640.0) * 9.0 / 16.0)), false),
                Err(_) => (None, false),
            };
            if let Some(size) = size {
                ui.add_space(if first { 4.0 } else { 16.0 });
                if !ready {
                    // Faint placeholder with a small spinner (egui's own spinner fills the whole image).
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    ui.painter().rect_filled(rect, 6.0, CARD);
                    ui.put(Rect::from_center_size(rect.center(), vec2(22.0, 22.0)), egui::Spinner::new().size(22.0));
                } else if crate::video::is_clip_poster(url) {
                    crate::video::show(ui, url, img, size);
                } else {
                    ui.add(img.fit_to_exact_size(size));
                }
                ui.add_space(6.0);
                (first, blank, prev_bullet) = (false, false, false);
            }
            continue;
        }
        let (heading, bullet, text) = match (line.strip_prefix(HEADING), line.strip_prefix(BULLET)) {
            (Some(h), _) => (true, false, h),
            (_, Some(b)) => (false, true, b),
            // Older stored text has no `## `; guess titles from their shape.
            _ => (is_heading(&line.replace("**", "")), false, line),
        };
        if !first {
            let gap = match () {
                _ if heading => 22.0,
                _ if bullet && prev_bullet => 8.0,
                _ if blank => 12.0,
                _ => 2.0,
            };
            ui.add_space(gap);
        }
        if heading {
            ui.add(egui::Label::new(RichText::new(text.replace("**", "")).size(16.0).color(theme::TEXT)).wrap());
            ui.add_space(4.0);
        } else if bullet {
            ui.horizontal_top(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("\u{2022}").size(14.5).line_height(Some(22.0)).color(body));
                let w = ui.available_width();
                ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(w);
                    ui.add(egui::Label::new(rich_line(text, body)).wrap());
                });
            });
        } else {
            ui.add(egui::Label::new(rich_line(text, body)).wrap());
        }
        (first, blank, prev_bullet) = (false, false, bullet);
    }
}

/// One description line, with `**bold**` parts in the bright text color (no bold font weight).
fn rich_line(text: &str, body: Color32) -> egui::text::LayoutJob {
    let parts: Vec<&str> = text.split("**").collect();
    let balanced = parts.len() % 2 == 1;
    let mut job = egui::text::LayoutJob::default();
    for (i, p) in parts.iter().enumerate() {
        let bold = balanced && i % 2 == 1;
        let fmt = egui::TextFormat {
            font_id: FontId::proportional(14.5),
            line_height: Some(22.0),
            color: if bold { theme::TEXT } else { body },
            ..Default::default()
        };
        job.append(p, 0.0, fmt);
    }
    job
}

/// Section titles in store text ("Story", "KEY FEATURES"): short, no sentence punctuation.
fn is_heading(l: &str) -> bool {
    let n = l.chars().count();
    // "A Brand New Experience!" is a title; a longer line ending in "!" is a sentence.
    n <= 48
        && !(l.ends_with('!') && n > 36)
        && !l.ends_with(['.', '?', ',', ';', ':', ')', '"', '\u{201d}', '…'])
        && !l.starts_with(['-', '*', '\u{2022}'])
        && l.chars().next().is_some_and(|c| !c.is_lowercase())
}

#[cfg(test)]
mod tests {
    #[test]
    fn headings() {
        for h in ["Story", "A Living World", "A Brand New Experience!", "KEY FEATURES"] {
            assert!(super::is_heading(h), "{h}");
        }
        for t in ["The ultimate hunting experience awaits you in Monster Hunter Wilds.", "- Online co-op", "and thrive", "Play cards in ascending mana order to land devastating combos!"] {
            assert!(!super::is_heading(t), "{t}");
        }
    }
}
