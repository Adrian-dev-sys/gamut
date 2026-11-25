//! Background threads: Steam scan, cover + metadata scrapers, playtime tracking.
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::Context;
use gamut_core::{launch::Launched, steam, sync, Config, Library, Paths};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

pub enum Event {
    LibraryChanged,
    GameUpdated(i64),
    Error(String),
    Info(String),
    /// Playnite folder + dry-run report, or why it couldn't be read.
    PlaynitePreview(Result<(std::path::PathBuf, gamut_core::playnite::Summary), String>),
    /// Finished import summary (or error); covers changed for these ids.
    PlayniteImported(Result<String, String>),
}

pub struct Workers {
    pub rx: Receiver<Event>,
    tx: Sender<Event>,
    kick_cover: Sender<()>,
    kick_meta: Sender<()>,
    pub pending_cover: Arc<AtomicUsize>,
    pub pending_meta: Arc<AtomicUsize>,
    pub scanning: Arc<AtomicBool>,
    /// Playnite import progress: (images done, images total); total 0 while idle.
    pub importing: Arc<(AtomicUsize, AtomicUsize)>,
    /// Games Gamut started itself and whose process is still alive.
    pub running: Arc<Mutex<HashSet<i64>>>,
    /// Steam's currently running app id (0 = none).
    pub steam_running: Arc<AtomicU32>,
    cfg: Arc<RwLock<Config>>,
    paths: Paths,
    ctx: Context,
}

impl Workers {
    pub fn start(ctx: &Context, paths: Paths, cfg: Arc<RwLock<Config>>) -> Self {
        let (tx, rx) = unbounded();
        let (kick_cover, kc_rx) = unbounded();
        let (kick_meta, km_rx) = unbounded();
        let w = Workers {
            rx,
            tx,
            kick_cover,
            kick_meta,
            pending_cover: Default::default(),
            pending_meta: Default::default(),
            scanning: Default::default(),
            importing: Default::default(),
            running: Default::default(),
            steam_running: Default::default(),
            cfg,
            paths,
            ctx: ctx.clone(),
        };
        let paths = w.paths.clone();
        std::thread::spawn(move || prune_orphan_art(&paths));
        w.spawn_scraper(kc_rx, w.pending_cover.clone(), false);
        w.spawn_scraper(km_rx, w.pending_meta.clone(), true);
        let (steam, ctx) = (w.steam_running.clone(), w.ctx.clone());
        std::thread::spawn(move || loop {
            let id = steam::running_appid().unwrap_or(0);
            if steam.swap(id, Ordering::Relaxed) != id {
                ctx.request_repaint();
            }
            std::thread::sleep(Duration::from_secs(2));
        });
        w
    }

    /// Loops over games needing work; sleeps until kicked when nothing progresses.
    fn spawn_scraper(&self, kick: Receiver<()>, pending: Arc<AtomicUsize>, meta: bool) {
        let (paths, cfg, tx, ctx) = (self.paths.clone(), self.cfg.clone(), self.tx.clone(), self.ctx.clone());
        let kick_cover = self.kick_cover.clone();
        std::thread::spawn(move || {
            let lib = match Library::open(&paths.db) {
                Ok(l) => l,
                Err(e) => return log::error!("scraper db: {e}"),
            };
            // Games that keep failing are skipped until the next app start.
            let mut failures: std::collections::HashMap<i64, u32> = Default::default();
            loop {
                let list = if meta { lib.needing_meta() } else { lib.needing_cover() }.unwrap_or_default();
                pending.store(list.len(), Ordering::Relaxed);
                let mut ok = 0;
                for g in list {
                    if failures.get(&g.id).copied().unwrap_or(0) >= 3 {
                        pending.fetch_sub(1, Ordering::Relaxed);
                        continue;
                    }
                    let c = cfg.read().unwrap().clone();
                    let r = if meta { sync::fetch_meta(&lib, &g) } else { sync::fetch_cover(&lib, &paths, &c, &g) };
                    match r {
                        Ok(_) => {
                            ok += 1;
                            let _ = tx.send(Event::GameUpdated(g.id));
                        }
                        Err(e) if e.to_string().contains("(401)") => {
                            // Bad key: tell the user and stop until settings change.
                            let _ = tx.send(Event::Error(format!("{e:#}")));
                            break;
                        }
                        Err(e) => {
                            *failures.entry(g.id).or_default() += 1;
                            log::warn!("{} {}: {e:#}", if meta { "meta" } else { "cover" }, g.name);
                            // Steam store rate limit: back off.
                            let wait = if e.to_string().contains("429") { 60 } else { 2 };
                            std::thread::sleep(Duration::from_secs(wait));
                        }
                    }
                    pending.fetch_sub(1, Ordering::Relaxed);
                    ctx.request_repaint();
                }
                pending.store(0, Ordering::Relaxed);
                ctx.request_repaint();
                if meta && ok > 0 {
                    let _ = kick_cover.send(());
                }
                if ok == 0 {
                    let _ = kick.recv_timeout(Duration::from_secs(600));
                    while kick.try_recv().is_ok() {}
                }
            }
        });
    }

    pub fn kick(&self) {
        let _ = self.kick_cover.send(());
        let _ = self.kick_meta.send(());
    }

    /// Reads a Playnite library (default location when `dir` is None) and reports what an import would do.
    pub fn playnite_preview(&self, dir: Option<std::path::PathBuf>) {
        let (tx, ctx) = (self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let r = (|| {
                let dir = match dir {
                    Some(d) => d,
                    None => gamut_core::playnite::default_dir()
                        .ok_or_else(|| anyhow::anyhow!("Couldn't find a Playnite library in %APPDATA%\\Playnite. Use \"Choose folder\" for a portable Playnite."))?,
                };
                let items = gamut_core::playnite::load(&dir)?;
                anyhow::Ok((dir, gamut_core::playnite::summarize(&items)))
            })();
            let _ = tx.send(Event::PlaynitePreview(r.map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    pub fn playnite_import(&self, dir: std::path::PathBuf) {
        let progress = self.importing.clone();
        if progress.1.swap(1, Ordering::SeqCst) != 0 {
            return;
        }
        progress.0.store(0, Ordering::SeqCst);
        let (paths, tx, ctx) = (self.paths.clone(), self.tx.clone(), self.ctx.clone());
        let (kc, km) = (self.kick_cover.clone(), self.kick_meta.clone());
        std::thread::spawn(move || {
            let r = (|| {
                let items = gamut_core::playnite::load(&dir)?;
                let mut lib = Library::open(&paths.db)?;
                let tick = |done: usize, total: usize| {
                    progress.0.store(done, Ordering::Relaxed);
                    progress.1.store(total.max(1), Ordering::Relaxed);
                    if done % 25 == 0 {
                        ctx.request_repaint();
                    }
                };
                let st = gamut_core::playnite::import(&mut lib, &paths, &items, &tick)?;
                anyhow::Ok(format!(
                    "Imported {} games from Playnite, with {} covers and {} backgrounds.",
                    st.games, st.covers, st.backgrounds
                ))
            })();
            progress.1.store(0, Ordering::SeqCst);
            let _ = tx.send(Event::LibraryChanged);
            let _ = tx.send(Event::PlayniteImported(r.map_err(|e| format!("Playnite import failed: {e:#}"))));
            let _ = kc.send(());
            let _ = km.send(());
            ctx.request_repaint();
        });
    }

    pub fn rescan(&self) {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return;
        }
        let (paths, cfg, tx, ctx, scanning) =
            (self.paths.clone(), self.cfg.clone(), self.tx.clone(), self.ctx.clone(), self.scanning.clone());
        let (kc, km) = (self.kick_cover.clone(), self.kick_meta.clone());
        std::thread::spawn(move || {
            let c = cfg.read().unwrap().clone();
            let r = steam::scan(&c).and_then(|(entries, warn)| {
                Library::open(&paths.db)?.sync_steam(&entries)?;
                Ok(warn)
            });
            match r {
                Ok(warn) => {
                    let _ = tx.send(Event::LibraryChanged);
                    if let Some(w) = warn {
                        let _ = tx.send(Event::Info(format!("Steam library import: {w}")));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Event::Error(format!("Steam scan failed: {e:#}")));
                }
            }
            scanning.store(false, Ordering::SeqCst);
            let _ = kc.send(());
            let _ = km.send(());
            ctx.request_repaint();
        });
    }

    /// Adds the session's playtime when a tracked game exits.
    pub fn track(&self, id: i64, launched: Launched) {
        let Launched::Tracked(handle) = launched else { return };
        let (paths, tx, ctx, running) = (self.paths.clone(), self.tx.clone(), self.ctx.clone(), self.running.clone());
        running.lock().unwrap().insert(id);
        std::thread::spawn(move || {
            let r = handle.join();
            running.lock().unwrap().remove(&id);
            ctx.request_repaint();
            if let Ok(Some((mins, start))) = r {
                if let Ok(lib) = Library::open(&paths.db) {
                    let _ = lib.add_playtime(id, mins, start);
                    let _ = tx.send(Event::GameUpdated(id));
                    ctx.request_repaint();
                }
            }
        });
    }
}

/// Deletes art whose game no longer exists, so a reused id never shows another game's cover.
fn prune_orphan_art(paths: &Paths) {
    let started = std::time::SystemTime::now();
    let Ok(games) = Library::open(&paths.db).and_then(|l| l.all()) else { return };
    if games.is_empty() {
        return;
    }
    let ids: HashSet<i64> = games.iter().map(|g| g.id).collect();
    for dir in [&paths.covers, &paths.heroes, &paths.thumbs] {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let id = p.file_stem().and_then(|s| s.to_str()).and_then(|s| s.parse::<i64>().ok());
            // Files newer than the id snapshot may belong to a game added meanwhile.
            let old = e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < started);
            if old && id.is_some_and(|id| !ids.contains(&id)) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}
