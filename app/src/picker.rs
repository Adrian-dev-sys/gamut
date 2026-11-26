//! Cover/banner picker: SteamGridDB, IGDB, VNDB and official Steam art for a game, or a local file.
use crate::theme::{self, ACCENT, CARD, TEXT_DIM};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{vec2, ColorImage, CornerRadius, RichText, Sense, Stroke, TextureHandle, TextureOptions};
use gamut_core::scrape::{self, SgdbArt, SgdbMatch};
use gamut_core::{igdb, images, model::state, Config, Game, Library, Paths};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ArtKind {
    Cover,
    Banner,
}

enum Msg {
    Games(u64, Result<Vec<SgdbMatch>, String>),
    Vndb(u64, Result<Vec<SgdbMatch>, String>),
    Igdb(u64, Result<Vec<SgdbMatch>, String>),
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

/// "Name · IGDB · PS2, Wii" (at most three platforms).
fn igdb_label(g: &igdb::IgdbGame) -> String {
    let mut p: Vec<&str> = g.platforms.iter().map(String::as_str).take(3).collect();
    if g.platforms.len() > 3 {
        p.push("…");
    }
    if p.is_empty() { format!("{} · IGDB", g.name) } else { format!("{} · IGDB · {}", g.name, p.join(", ")) }
}

/// An IGDB entry's cover, or its artworks then screenshots as banners.
fn igdb_art(g: &igdb::IgdbGame, hero: bool) -> Vec<SgdbArt> {
    let art = |id: &String, cover: bool| SgdbArt {
        url: if cover { igdb::cover_url(id) } else { igdb::hero_url(id) },
        thumb: igdb::thumb_url(id, cover),
        width: if cover { 528 } else { 1920 },
        height: if cover { 748 } else { 1080 },
        source: Some(if cover { "IGDB cover" } else { "IGDB artwork" }),
    };
    if hero {
        g.artworks.iter().chain(&g.screenshots).map(|i| art(i, false)).collect()
    } else {
        g.cover.iter().map(|i| art(i, true)).collect()
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
    /// Name search results per source; `matches` = pinned + sgdb + igdb + vndb.
    sgdb: Vec<SgdbMatch>,
    igdb: Vec<SgdbMatch>,
    vndb: Vec<SgdbMatch>,
    sgdb_done: bool,
    igdb_done: bool,
    vndb_done: bool,
    /// Set when IGDB credentials are configured.
    igdb_cfg: Option<Config>,
    /// Tagged as a visual novel: VNDB results come first.
    vn: bool,
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
    pub fn open(ctx: &egui::Context, g: &Game, kind: ArtKind, cfg: &Config, paths: Paths) -> Self {
        let key = &cfg.steamgriddb_key;
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
            sgdb: Vec::new(),
            igdb: Vec::new(),
            vndb: Vec::new(),
            sgdb_done: false,
            igdb_done: false,
            vndb_done: false,
            igdb_cfg: igdb::configured(cfg).then(|| cfg.clone()),
            vn: tags.contains("visual novel"),
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
        p.search(ctx);
        if p.key.is_empty() {
            p.note = Some("Add a SteamGridDB API key in Settings to get community art too.".into());
        }
        p
    }

    /// Name search on SteamGridDB, IGDB (with keys) and VNDB, in parallel.
    fn search(&mut self, ctx: &egui::Context) {
        let round = self.search_round.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!("art picker: searching for {:?}", self.query.trim());
        (self.sgdb, self.igdb, self.vndb) = (Vec::new(), Vec::new(), Vec::new());
        (self.sgdb_done, self.igdb_done, self.vndb_done) = (self.key.is_empty(), self.igdb_cfg.is_none(), false);
        if let Some(cfg) = self.igdb_cfg.clone() {
            let (q, tx, ctx) = (self.query.trim().to_string(), self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = igdb::search(&cfg, &q).map_err(|e| format!("{e:#}")).map(|hits| {
                    let m = |h: &igdb::IgdbGame| SgdbMatch { path: format!("igdb/{}", h.id), year: h.year, name: igdb_label(h) };
                    hits.iter().take(8).map(m).collect()
                });
                let _ = tx.send(Msg::Igdb(round, r));
                ctx.request_repaint();
            });
        }
        if self.chosen.is_none() {
            self.busy = true;
        }
        if !self.key.is_empty() {
            let (key, q, tx, ctx) = (self.key.clone(), self.query.trim().to_string(), self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let r = scrape::sgdb_search(&key, &q).map_err(|e| format!("{e:#}"));
                let _ = tx.send(Msg::Games(round, r));
                ctx.request_repaint();
            });
        }
        let (q, vn, tx, ctx) = (self.query.trim().to_string(), self.vn, self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = gamut_core::vndb::search(&q).map_err(|e| format!("{e:#}")).map(|hits| {
                // VNDB's search is fuzzy: outside visual novels, keep only titles that really match.
                let want = scrape::normalize(&q);
                let related = |t: &str| {
                    let t = scrape::normalize(t);
                    !t.is_empty() && (t.starts_with(&want) || want.starts_with(&t))
                };
                hits.into_iter()
                    .filter(|h| vn || related(&h.title) || h.alttitle.as_deref().is_some_and(related))
                    .take(5)
                    .map(|h| SgdbMatch { path: format!("vndb/{}", h.id), year: h.year(), name: format!("{} · VNDB", h.title) })
                    .collect()
            });
            let _ = tx.send(Msg::Vndb(round, r));
            ctx.request_repaint();
        });
    }

    /// Recomputes the match list; picks one once enough results are in.
    fn rebuild(&mut self, ctx: &egui::Context) {
        self.matches = self.pinned.iter().chain(&self.sgdb).chain(&self.igdb).chain(&self.vndb).cloned().collect();
        if self.chosen.is_some() {
            return;
        }
        let (p, s, i) = (self.pinned.len(), self.sgdb.len(), self.igdb.len());
        let first_sgdb = (s > 0).then_some(p);
        let first_igdb = (i > 0).then_some(p + s);
        let first_vndb = (!self.vndb.is_empty()).then_some(p + s + i);
        // Don't jump ahead while a preferred source is still searching.
        let after_sgdb = || first_igdb.or(if self.igdb_done { first_vndb } else { None });
        let pick = if self.vn {
            first_vndb.or(if self.vndb_done { first_sgdb.or(first_igdb) } else { None })
        } else {
            first_sgdb.or(if self.sgdb_done { after_sgdb() } else { None })
        };
        let all_done = self.sgdb_done && self.igdb_done && self.vndb_done;
        match pick.or(if all_done && !self.matches.is_empty() { Some(0) } else { None }) {
            Some(i) => self.choose(ctx, i),
            None if all_done => {
                self.busy = false;
                self.note = Some(format!("Nothing found for \"{}\". Try a shorter or different name.", self.query.trim()));
            }
            None => {}
        }
    }

    fn choose(&mut self, ctx: &egui::Context, i: usize) {
        let round = self.art_round.fetch_add(1, Ordering::SeqCst) + 1;
        log::info!("art picker: listing {:?} for {}", self.kind, self.matches[i].path);
        self.chosen = Some(i);
        self.tiles.clear();
        self.busy = true;
        self.note = None;
        let (key, path, hero, nsfw) = (self.key.clone(), self.matches[i].path.clone(), self.kind == ArtKind::Banner, self.nsfw);
        let (tx, ctx, igdb_cfg) = (self.tx.clone(), ctx.clone(), self.igdb_cfg.clone());
        std::thread::spawn(move || {
            let igdb_id = path.strip_prefix("igdb/").and_then(|a| a.parse::<i64>().ok());
            let r = if let Some(appid) = path.strip_prefix("official/").and_then(|a| a.parse().ok()) {
                Ok(scrape::steam_official_art(appid, hero))
            } else if let (Some(id), Some(cfg)) = (igdb_id, igdb_cfg) {
                igdb::game(&cfg, id).map_err(|e| format!("{e:#}")).map(|g| g.map(|g| igdb_art(&g, hero)).unwrap_or_default())
            } else if let Some(id) = path.strip_prefix("vndb/") {
                gamut_core::vndb::images(id, nsfw).map_err(|e| format!("{e:#}")).map(|mut v| {
                    // Closest to the wanted shape first (portrait covers, wide banners).
                    let want: f32 = if hero { 3.1 } else { 2.0 / 3.0 };
                    let off = |a: &SgdbArt| ((a.width.max(1) as f32 / a.height.max(1) as f32) / want).ln().abs();
                    v.sort_by(|a, b| off(a).total_cmp(&off(b)));
                    v
                })
            } else {
                scrape::sgdb_list(&key, &path, hero, nsfw).map_err(|e| format!("{e:#}"))
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
                Msg::Games(g, r) | Msg::Vndb(g, r) | Msg::Igdb(g, r) if g != self.search_round.load(Ordering::SeqCst) => {
                    let _ = r;
                }
                Msg::Games(_, r) => {
                    self.sgdb_done = true;
                    self.sgdb = r.unwrap_or_else(|e| {
                        log::warn!("art picker: SteamGridDB search failed: {e}");
                        Vec::new()
                    });
                    self.rebuild(ctx);
                }
                Msg::Igdb(_, r) => {
                    self.igdb_done = true;
                    self.igdb = r.unwrap_or_else(|e| {
                        log::warn!("art picker: IGDB search failed: {e}");
                        Vec::new()
                    });
                    self.rebuild(ctx);
                }
                Msg::Vndb(_, r) => {
                    self.vndb_done = true;
                    self.vndb = r.unwrap_or_else(|e| {
                        log::warn!("art picker: VNDB search failed: {e}");
                        Vec::new()
                    });
                    self.rebuild(ctx);
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
                                let path = self.chosen.map_or("", |c| self.matches[c].path.as_str());
                                self.note = Some(if path.starts_with("official/") {
                                    format!("Steam has no official {what} for this game.")
                                } else if path.starts_with("igdb/") {
                                    format!("IGDB has no {what} for this entry.")
                                } else if path.starts_with("vndb/") {
                                    format!("VNDB has no suitable images for this entry.")
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
                    self.chosen = self.chosen.map(|c| c + 1);
                    self.rebuild(ctx);
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
        let resp = theme::dialog(ctx, "art_picker", true, |ui| {
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
            {
                ui.horizontal(|ui| {
                    let r = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search by name").desired_width(320.0));
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
