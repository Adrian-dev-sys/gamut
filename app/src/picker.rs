//! Cover/banner picker: browse SteamGridDB images for a game, or use a local file.
use crate::theme::{self, ACCENT, CARD, TEXT_DIM};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{vec2, Color32, ColorImage, CornerRadius, RichText, Sense, Stroke, TextureHandle, TextureOptions};
use gamut_core::scrape::{self, SgdbArt, SgdbMatch};
use gamut_core::{images, model::state, Game, Library, Paths};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ArtKind {
    Cover,
    Banner,
}

enum Msg {
    Games(u64, Result<Vec<SgdbMatch>, String>),
    Arts(u64, Result<Vec<SgdbArt>, String>),
    Thumb(u64, usize, Option<ColorImage>),
    Applied(Result<(), String>),
}

struct Tile {
    art: SgdbArt,
    tex: Option<TextureHandle>,
}

pub struct Picker {
    pub id: i64,
    pub kind: ArtKind,
    game_name: String,
    has_cover: bool,
    cover_state: i32,
    nsfw: bool,
    key: String,
    paths: Paths,
    query: String,
    matches: Vec<SgdbMatch>,
    chosen: Option<usize>,
    tiles: Vec<Tile>,
    busy: bool,
    applying: bool,
    note: Option<String>,
    /// Bumped on every new search/match so late results from old ones are dropped.
    round: Arc<AtomicU64>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

/// What the caller must refresh after the picker closes.
pub enum Outcome {
    Open,
    Closed,
    Changed(i64, ArtKind),
}

impl Picker {
    pub fn open(ctx: &egui::Context, g: &Game, kind: ArtKind, key: &str, paths: Paths) -> Self {
        let (tx, rx) = unbounded();
        let tags = g.tags.as_deref().unwrap_or("").to_lowercase();
        let mut p = Picker {
            id: g.id,
            kind,
            game_name: g.name.clone(),
            has_cover: g.has_cover,
            cover_state: g.cover_state,
            nsfw: ["nudity", "sexual content", "hentai"].iter().any(|t| tags.contains(t)),
            key: key.trim().to_string(),
            paths,
            query: g.name.clone(),
            matches: Vec::new(),
            chosen: None,
            tiles: Vec::new(),
            busy: false,
            applying: false,
            note: None,
            round: Default::default(),
            tx,
            rx,
        };
        if p.key.is_empty() {
            p.note = Some("Add a SteamGridDB API key in Settings to search for images online.".into());
        } else {
            // Steam games resolve exactly by app id; the name search fills in alternatives.
            if let Some(appid) = g.steam_appid().filter(|_| g.source == gamut_core::Source::Steam) {
                p.matches.push(SgdbMatch { path: format!("steam/{appid}"), name: format!("{} (Steam)", g.name), year: None });
                p.choose(ctx, 0);
            }
            p.search(ctx);
        }
        p
    }

    fn search(&mut self, ctx: &egui::Context) {
        let round = self.round.fetch_add(1, Ordering::SeqCst) + 1;
        let (key, q, tx, ctx) = (self.key.clone(), self.query.trim().to_string(), self.tx.clone(), ctx.clone());
        if self.chosen.is_none() {
            self.busy = true;
        }
        std::thread::spawn(move || {
            let r = scrape::sgdb_search(&key, &q).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Games(round, r));
            ctx.request_repaint();
        });
    }

    fn choose(&mut self, ctx: &egui::Context, i: usize) {
        let round = self.round.fetch_add(1, Ordering::SeqCst) + 1;
        self.chosen = Some(i);
        self.tiles.clear();
        self.busy = true;
        self.note = None;
        let (key, path, hero, nsfw) = (self.key.clone(), self.matches[i].path.clone(), self.kind == ArtKind::Banner, self.nsfw);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = scrape::sgdb_list(&key, &path, hero, nsfw).map_err(|e| format!("{e:#}"));
            let _ = tx.send(Msg::Arts(round, r));
            ctx.request_repaint();
        });
    }

    /// Downloads previews with a few threads; stops early once a newer search starts.
    fn load_thumbs(&self, ctx: &egui::Context, round: u64) {
        let queue: Arc<Mutex<Vec<(usize, String)>>> =
            Arc::new(Mutex::new(self.tiles.iter().enumerate().rev().map(|(i, t)| (i, t.art.thumb.clone())).collect()));
        for _ in 0..4 {
            let (queue, tx, ctx, cur) = (queue.clone(), self.tx.clone(), ctx.clone(), self.round.clone());
            std::thread::spawn(move || {
                while cur.load(Ordering::SeqCst) == round {
                    let Some((i, url)) = queue.lock().unwrap().pop() else { break };
                    let img = gamut_core::http::get_bytes(&url)
                        .ok()
                        .and_then(|b| images::decode_bytes(&b, 400).ok())
                        .map(|r| ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &r.pixels));
                    if tx.send(Msg::Thumb(round, i, img)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }
    }

    /// Stores the chosen image (url or local file) off the UI thread.
    fn apply(&mut self, ctx: &egui::Context, src: Result<String, std::path::PathBuf>) {
        self.applying = true;
        let (id, kind, has_cover, cover_state) = (self.id, self.kind, self.has_cover, self.cover_state);
        let (paths, tx, ctx) = (self.paths.clone(), self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = (|| {
                let bytes = match src {
                    Ok(url) => gamut_core::http::get_bytes(&url)?,
                    Err(path) => std::fs::read(path)?,
                };
                let lib = Library::open(&paths.db)?;
                match kind {
                    ArtKind::Cover => {
                        images::store_cover(&bytes, &paths.cover(id), &paths.thumb(id))?;
                        lib.set_cover_state(id, true, None, state::DONE)?;
                    }
                    ArtKind::Banner => {
                        images::store_hero(&bytes, &paths.hero(id))?;
                        lib.set_cover_state(id, has_cover, Some(true), cover_state)?;
                    }
                }
                anyhow::Ok(())
            })();
            let _ = tx.send(Msg::Applied(r.map_err(|e| format!("Couldn't use that image: {e:#}"))));
            ctx.request_repaint();
        });
    }

    fn handle(&mut self, ctx: &egui::Context) -> Option<Outcome> {
        let cur = self.round.load(Ordering::SeqCst);
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Games(g, r) => {
                    if g != cur {
                        continue;
                    }
                    match r {
                        Ok(list) => {
                            let steam = self.matches.first().filter(|m| m.path.starts_with("steam/")).cloned();
                            self.matches = steam.into_iter().chain(list).collect();
                            if self.chosen.is_none() {
                                if self.matches.is_empty() {
                                    self.busy = false;
                                    self.note = Some(format!("No game called \"{}\" on SteamGridDB.", self.query.trim()));
                                } else {
                                    self.choose(ctx, 0);
                                }
                            }
                        }
                        Err(e) => {
                            self.busy = false;
                            self.note = Some(e);
                        }
                    }
                }
                Msg::Arts(g, r) => {
                    if g != cur {
                        continue;
                    }
                    self.busy = false;
                    match r {
                        Ok(arts) => {
                            if arts.is_empty() {
                                let what = if self.kind == ArtKind::Cover { "covers" } else { "banners" };
                                self.note = Some(format!("SteamGridDB has no {what} for this game. Try another match or search."));
                            }
                            self.tiles = arts.into_iter().map(|art| Tile { art, tex: None }).collect();
                            self.load_thumbs(ctx, g);
                        }
                        Err(e) => self.note = Some(e),
                    }
                }
                Msg::Thumb(g, i, img) => {
                    if let (true, Some(img), Some(t)) = (g == cur, img, self.tiles.get_mut(i)) {
                        t.tex = Some(ctx.load_texture(format!("pick{g}-{i}"), img, TextureOptions::LINEAR));
                    }
                }
                Msg::Applied(Ok(())) => return Some(Outcome::Changed(self.id, self.kind)),
                Msg::Applied(Err(e)) => {
                    self.applying = false;
                    self.note = Some(e);
                }
            }
        }
        None
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        if let Some(o) = self.handle(ctx) {
            return o;
        }
        let screen = ctx.content_rect();
        let size = vec2((screen.width() * 0.8).min(1100.0), (screen.height() * 0.8).min(760.0));
        let mut close = false;
        let what = if self.kind == ArtKind::Cover { "cover" } else { "banner" };
        let resp = egui::Modal::new(egui::Id::unique("art_picker")).backdrop_color(Color32::from_black_alpha(120)).show(ctx, |ui| {
            ui.set_width(size.x);
            ui.horizontal(|ui| {
                ui.heading(format!("Choose a {what}"));
                ui.label(theme::hint(&self.game_name));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                    if ui.add_enabled(!self.applying, egui::Button::new("From file…")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() {
                            self.apply(ctx, Err(p));
                        }
                    }
                });
            });
            ui.add_space(4.0);
            if !self.key.is_empty() {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search SteamGridDB").desired_width(320.0));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (ui.button("Search").clicked() || enter) && !self.query.trim().is_empty() {
                        self.chosen = None;
                        self.tiles.clear();
                        self.matches.clear();
                        self.search(ctx);
                    }
                    if self.busy || self.applying {
                        ui.spinner();
                    }
                });
                if self.matches.len() > 1 || self.chosen.is_none() && !self.matches.is_empty() {
                    ui.add_space(2.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(theme::caption("Game"));
                        let mut pick = None;
                        for (i, m) in self.matches.iter().enumerate().take(12) {
                            let label = match m.year {
                                Some(y) => format!("{} ({y})", m.name),
                                None => m.name.clone(),
                            };
                            if ui.selectable_label(self.chosen == Some(i), label).clicked() && self.chosen != Some(i) {
                                pick = Some(i);
                            }
                        }
                        if let Some(i) = pick {
                            self.choose(ctx, i);
                        }
                    });
                }
            }
            if let Some(n) = &self.note {
                ui.add_space(4.0);
                ui.label(theme::hint(n));
            }
            ui.add_space(6.0);
            ui.separator();
            let tile = if self.kind == ArtKind::Cover { vec2(150.0, 225.0) } else { vec2(330.0, 107.0) };
            let mut picked = None;
            egui::ScrollArea::vertical().max_height(size.y - 130.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                    for (i, t) in self.tiles.iter().enumerate() {
                        let (rect, r) = ui.allocate_exact_size(tile, Sense::click());
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        let round = CornerRadius::same(6);
                        match &t.tex {
                            Some(tex) => {
                                let [w, h] = tex.size();
                                egui::Image::from_texture(tex)
                                    .uv(crate::ui::cover_uv(w as f32 / h as f32, rect.aspect_ratio()))
                                    .corner_radius(round)
                                    .paint_at(ui, rect);
                            }
                            None => {
                                ui.painter().rect_filled(rect, round, CARD);
                            }
                        }
                        if r.hovered() && !self.applying {
                            ui.painter().rect_stroke(rect, round, Stroke::new(2.0, ACCENT), egui::StrokeKind::Inside);
                        }
                        let r = r.on_hover_text(format!("{} × {}", t.art.width, t.art.height));
                        if r.clicked() && !self.applying {
                            picked = Some(i);
                        }
                    }
                });
                if self.tiles.is_empty() && self.busy {
                    ui.add_space(20.0);
                    ui.label(RichText::new("Searching…").color(TEXT_DIM));
                }
            });
            if let Some(i) = picked {
                let url = self.tiles[i].art.url.clone();
                self.apply(ctx, Ok(url));
            }
        });
        if close || resp.should_close() {
            self.round.fetch_add(1, Ordering::SeqCst);
            Outcome::Closed
        } else {
            Outcome::Open
        }
    }
}
