//! Background threads: Steam scan, cover + metadata scrapers, playtime tracking.
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::Context;
use gamut_core::{launch::Launched, steam, sync, Config, Library, Paths};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;

pub enum Event {
    LibraryChanged,
    GameUpdated(i64),
    Error(String),
}

pub struct Workers {
    pub rx: Receiver<Event>,
    tx: Sender<Event>,
    kick_cover: Sender<()>,
    kick_meta: Sender<()>,
    pub pending_cover: Arc<AtomicUsize>,
    pub pending_meta: Arc<AtomicUsize>,
    pub scanning: Arc<AtomicBool>,
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
            cfg,
            paths,
            ctx: ctx.clone(),
        };
        w.spawn_scraper(kc_rx, w.pending_cover.clone(), false);
        w.spawn_scraper(km_rx, w.pending_meta.clone(), true);
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
            loop {
                let list = if meta { lib.needing_meta() } else { lib.needing_cover() }.unwrap_or_default();
                pending.store(list.len(), Ordering::Relaxed);
                let mut ok = 0;
                for g in list {
                    let c = cfg.read().unwrap().clone();
                    let r = if meta { sync::fetch_meta(&lib, &g) } else { sync::fetch_cover(&lib, &paths, &c, &g) };
                    match r {
                        Ok(_) => {
                            ok += 1;
                            let _ = tx.send(Event::GameUpdated(g.id));
                        }
                        Err(e) => {
                            log::warn!("{} {}: {e:#}", if meta { "meta" } else { "cover" }, g.name);
                            // Steam store rate limit: back off.
                            let wait = if e.to_string().contains("429") { 60 } else { 2 };
                            std::thread::sleep(Duration::from_secs(wait));
                        }
                    }
                    pending.fetch_sub(1, Ordering::Relaxed);
                    ctx.request_repaint();
                    if meta {
                        std::thread::sleep(Duration::from_millis(1500));
                    }
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

    pub fn rescan(&self) {
        if self.scanning.swap(true, Ordering::SeqCst) {
            return;
        }
        let (paths, cfg, tx, ctx, scanning) =
            (self.paths.clone(), self.cfg.clone(), self.tx.clone(), self.ctx.clone(), self.scanning.clone());
        let (kc, km) = (self.kick_cover.clone(), self.kick_meta.clone());
        std::thread::spawn(move || {
            let c = cfg.read().unwrap().clone();
            let r = steam::scan(&c).and_then(|entries| Library::open(&paths.db)?.sync_steam(&entries));
            match r {
                Ok(_) => {
                    let _ = tx.send(Event::LibraryChanged);
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

    /// Adds the session's playtime when a tracked custom game exits.
    pub fn track(&self, id: i64, launched: Launched) {
        let Launched::Tracked(handle) = launched else { return };
        let (paths, tx, ctx) = (self.paths.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            if let Ok(Some((mins, start))) = handle.join() {
                if let Ok(lib) = Library::open(&paths.db) {
                    let _ = lib.add_playtime(id, mins, start);
                    let _ = tx.send(Event::GameUpdated(id));
                    ctx.request_repaint();
                }
            }
        });
    }
}
