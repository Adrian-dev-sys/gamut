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
    /// Steam app id found by name, for games not from Steam.
    Official(u32),
}

struct Tile {
    art: SgdbArt,
    tex: Option<TextureHandle>,
    /// Preview couldn't be loaded (e.g. a 404): hidden.
    failed: bool,
}

/// Bytes of an image url, or of a local file path (Steam's library cache).
fn fetch(url: &str) -> anyhow::Result<Vec<u8>> {
    if url.starts_with("http") {
        gamut_core::http::get_bytes(url)
    } else {
        Ok(std::fs::read(url)?)
    }
}

fn official(appid: u32) -> SgdbMatch {
    SgdbMatch { path: format!("official/{appid}"), name: "Official Steam art".into(), year: None }
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
    /// Matches that survive a new name search (official Steam art, the Steam id on SteamGridDB).
    pinned: Vec<SgdbMatch>,
    chosen: Option<usize>,
    tiles: Vec<Tile>,
    busy: bool,
    applying: bool,
    note: Option<String>,
    /// Bumped per name search / per image listing so late results from older ones are dropped.
    search_round: Arc<AtomicU64>,
    art_round: Arc<AtomicU64>,
    /// The current match was picked automatically, so an empty result may fall through to the next.
    auto: bool,
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
            pinned: Vec::new(),
            chosen: None,
            tiles: Vec::new(),
            busy: false,
            applying: false,
            note: None,
            search_round: Default::default(),
            art_round: Default::default(),
            auto: true,
            tx,
            rx,
        };
        let steam = g.steam_appid().filter(|_| g.source == gamut_core::Source::Steam);
        if let Some(appid) = steam {
            p.pinned.push(official(appid));
            if !p.key.is_empty() {
                // Steam games resolve exactly by app id; the name search fills in alternatives.
                p.pinned.push(SgdbMatch { path: format!("steam/{appid}"), name: format!("{} (Steam id)", g.name), year: None });
            }
            p.matches = p.pinned.clone();
            p.choose(ctx, p.matches.len() - 1);
        } else {
            // Not from Steam: look for the same game on the store to offer its official art.
            let (name, tx, ctx) = (g.name.clone(), p.tx.clone(), ctx.clone());
            std::thread::spawn(move || match scrape::steam_search(&name) {
                Ok(Some(appid)) => {
                    let _ = tx.send(Msg::Official(appid));
                    ctx.request_repaint();
                }
                Ok(None) => {}
                Err(e) => log::warn!("art picker: Steam store search for {name:?}: {e:#}"),
            });
        }
        if p.key.is_empty() {
            p.note = Some("Add a SteamGridDB API key in Settings to get community art too.".into());
        } else {
            p.search(ctx);
        }
        p
    }

    fn search(&mut self, ctx: &egui::Context) {
        let round = self.search_round.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!("art picker: searching SteamGridDB for {:?}", self.query.trim());
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
        let round = self.art_round.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!("art picker: listing {:?} for {}", self.kind, self.matches[i].path);
        self.chosen = Some(i);
        self.tiles.clear();
        self.busy = true;
        self.note = None;
        let (key, path, hero, nsfw) = (self.key.clone(), self.matches[i].path.clone(), self.kind == ArtKind::Banner, self.nsfw);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = match path.strip_prefix("official/").and_then(|a| a.parse().ok()) {
                Some(appid) => Ok(scrape::steam_official_art(appid, hero)),
                None => scrape::sgdb_list(&key, &path, hero, nsfw).map_err(|e| format!("{e:#}")),
            };
            let _ = tx.send(Msg::Arts(round, r));
            ctx.request_repaint();
        });
    }

    /// Downloads previews with a few threads; stops early once a newer search starts.
    fn load_thumbs(&self, ctx: &egui::Context, round: u64) {
        let queue: Arc<Mutex<Vec<(usize, String)>>> =
            Arc::new(Mutex::new(self.tiles.iter().enumerate().rev().map(|(i, t)| (i, t.art.thumb.clone())).collect()));
        for _ in 0..4 {
            let (queue, tx, ctx, cur) = (queue.clone(), self.tx.clone(), ctx.clone(), self.art_round.clone());
            std::thread::spawn(move || {
                while cur.load(Ordering::SeqCst) == round {
                    let Some((i, url)) = queue.lock().unwrap().pop() else { break };
                    let img = fetch(&url)
                        .and_then(|b| images::decode_bytes(&b, 400))
                        .inspect_err(|e| log::warn!("art picker: preview {url}: {e:#}"))
                        .ok()
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
                    Ok(url) => fetch(&url)?,
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
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Games(g, r) => {
                    if g != self.search_round.load(Ordering::SeqCst) {
                        continue;
                    }
                    match r {
                        Ok(list) => {
                            self.matches = self.pinned.iter().cloned().chain(list).collect();
                            if self.chosen.is_none() {
                                if self.matches.is_empty() {
                                    self.busy = false;
                                    self.note = Some(format!("No game called \"{}\" on SteamGridDB.", self.query.trim()));
                                } else {
                                    // Land on the first search hit, not the pinned Steam entries.
                                    let first = if self.matches.len() > self.pinned.len() { self.pinned.len() } else { 0 };
                                    self.choose(ctx, first);
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!("art picker: search failed: {e}");
                            if self.chosen.is_none() {
                                self.busy = false;
                                self.note = Some(format!("Search failed: {e}"));
                            }
                        }
                    }
                }
                Msg::Arts(g, r) => {
                    if g != self.art_round.load(Ordering::SeqCst) {
                        continue;
                    }
                    self.busy = false;
                    match r {
                        Ok(arts) => {
                            let next = self.chosen.map_or(0, |c| c + 1);
                            if arts.is_empty() && self.auto && next < self.matches.len() {
                                // e.g. no art filed under the Steam id: try the best name match instead.
                                log::info!("art picker: nothing for {}, trying the next match", self.matches[self.chosen.unwrap_or(0)].path);
                                self.choose(ctx, next);
                                continue;
                            }
                            if arts.is_empty() {
                                let what = if self.kind == ArtKind::Cover { "cover" } else { "banner" };
                                let from_steam = self.chosen.is_some_and(|c| self.matches[c].path.starts_with("official/"));
                                self.note = Some(if from_steam {
                                    format!("Steam has no official {what} for this game.")
                                } else {
                                    format!("SteamGridDB has no {what}s for this game. Try another match or search.")
                                });
                            }
                            self.tiles = arts.into_iter().map(|art| Tile { art, tex: None, failed: false }).collect();
                            self.load_thumbs(ctx, g);
                        }
                        Err(e) => {
                            log::warn!("art picker: listing failed: {e}");
                            self.note = Some(format!("Couldn't load images: {e}"));
                        }
                    }
                }
                Msg::Thumb(g, i, img) => {
                    let cur = self.art_round.load(Ordering::SeqCst);
                    if let (true, Some(t)) = (g == cur, self.tiles.get_mut(i)) {
                        match img {
                            Some(img) => t.tex = Some(ctx.load_texture(format!("pick{g}-{i}"), img, TextureOptions::LINEAR)),
                            None => t.failed = true,
                        }
                    }
                }
                Msg::Official(appid) => {
                    self.pinned.insert(0, official(appid));
                    self.matches.insert(0, official(appid));
                    self.chosen = self.chosen.map(|c| c + 1);
                    if self.chosen.is_none() && self.key.is_empty() {
                        self.choose(ctx, 0);
                    }
                }
                Msg::Applied(Ok(())) => return Some(Outcome::Changed(self.id, self.kind)),
                Msg::Applied(Err(e)) => {
                    log::warn!("art picker: {e}");
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
        let size = vec2((screen.width() * 0.8).min(1100.0), (screen.height() * 0.88).min(980.0));
        let mut close = false;
        let what = if self.kind == ArtKind::Cover { "cover" } else { "banner" };
        let resp = egui::Modal::new(egui::Id::unique("art_picker")).backdrop_color(Color32::from_black_alpha(120)).show(ctx, |ui| {
            ui.set_width(size.x);
            let top = ui.cursor().top();
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
                        self.auto = true;
                        self.tiles.clear();
                        self.matches.clear();
                        self.search(ctx);
                    }
                    if self.busy || self.applying {
                        ui.spinner();
                    }
                });
            }
            {
                if !self.matches.is_empty() {
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
                            self.auto = false;
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
            // The grid fills whatever height the header left over.
            let grid_h = (size.y - (ui.cursor().top() - top)).max(240.0);
            egui::ScrollArea::vertical().min_scrolled_height(grid_h).max_height(grid_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
                    for (i, t) in self.tiles.iter().enumerate().filter(|(_, t)| !t.failed) {
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
                        let size = format!("{} × {}", t.art.width, t.art.height);
                        let r = r.on_hover_text(match t.art.source {
                            Some(src) => format!("{src}\n{size}"),
                            None => size,
                        });
                        if r.clicked() && !self.applying {
                            picked = Some(i);
                        }
                    }
                });
                if self.tiles.is_empty() && self.busy {
                    ui.add_space(20.0);
                    let what = if self.chosen.is_some() { "Loading images from SteamGridDB…" } else { "Searching SteamGridDB…" };
                    ui.label(RichText::new(what).color(TEXT_DIM));
                }
            });
            if let Some(i) = picked {
                let url = self.tiles[i].art.url.clone();
                self.apply(ctx, Ok(url));
            }
        });
        if close || resp.should_close() {
            self.search_round.fetch_add(1, Ordering::SeqCst);
            self.art_round.fetch_add(1, Ordering::SeqCst);
            Outcome::Closed
        } else {
            Outcome::Open
        }
    }
}
