//! Background threads: Steam scan, cover + metadata scrapers, playtime tracking.
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::Context;
use gamut_core::{launch::{self, Launched}, specialk, steam, sync, Config, Game, Library, Paths};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

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
    /// Steam apps Gamut asked Steam to start, until Steam reports them running (or 90 s pass).
    pub steam_starting: Arc<Mutex<HashMap<u32, Instant>>>,
    /// Newer Special K release, when one was found; true while it installs.
    pub sk_update: Arc<Mutex<Option<specialk::update::Update>>>,
    pub sk_installing: Arc<AtomicBool>,
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
            steam_starting: Default::default(),
            sk_update: Default::default(),
            sk_installing: Default::default(),
            cfg,
            paths,
            ctx: ctx.clone(),
        };
        let paths = w.paths.clone();
        std::thread::spawn(move || prune_orphan_art(&paths));
        w.spawn_scraper(kc_rx, w.pending_cover.clone(), false);
        w.spawn_scraper(km_rx, w.pending_meta.clone(), true);
        w.spawn_sk_update_check();
        let (steam, ctx, starting) = (w.steam_running.clone(), w.ctx.clone(), w.steam_starting.clone());
        std::thread::spawn(move || loop {
            let id = steam::running_appid().unwrap_or(0);
            {
                let mut st = starting.lock().unwrap();
                let n = st.len();
                st.retain(|&app, t| app != id && t.elapsed() < Duration::from_secs(90));
                if st.len() != n {
                    ctx.request_repaint();
                }
            }
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

    /// Re-reads only game descriptions from Playnite (restores their images).
    pub fn playnite_descriptions(&self, dir: Option<std::path::PathBuf>) {
        let (paths, tx, ctx) = (self.paths.clone(), self.tx.clone(), self.ctx.clone());
        let _ = tx.send(Event::Info("Reading descriptions from Playnite…".into()));
        std::thread::spawn(move || {
            let r = (|| {
                let dir = dir.or_else(gamut_core::playnite::default_dir).ok_or_else(|| anyhow::anyhow!("Playnite folder not found"))?;
                let items = gamut_core::playnite::load(&dir)?;
                let mut lib = Library::open(&paths.db)?;
                gamut_core::playnite::refresh_descriptions(&mut lib, &items)
            })();
            match r {
                Ok((n, kept)) => {
                    let extra = if kept > 0 { format!(" Kept {kept} you edited in Gamut.") } else { String::new() };
                    log::info!("Playnite descriptions: {n} updated, {kept} kept (edited)");
                    let _ = tx.send(Event::LibraryChanged);
                    let _ = tx.send(Event::Info(format!("Updated {n} descriptions from Playnite.{extra}")));
                }
                Err(e) => {
                    let _ = tx.send(Event::Error(format!("Reading Playnite descriptions failed: {e:#}")));
                }
            }
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

    /// Starts a game on a worker thread: HDR, ReShade and Special K first, then the game.
    pub fn play(&self, g: Game) -> anyhow::Result<()> {
        let (sk, rs) = {
            let c = self.cfg.read().unwrap();
            (c.specialk.clone(), c.reshade.clone())
        };
        if g.installed {
            if let (gamut_core::Source::Steam, Some(app)) = (g.source, g.steam_appid()) {
                self.steam_starting.lock().unwrap().insert(app, Instant::now());
            }
        }
        let t = Tracker { paths: self.paths.clone(), tx: self.tx.clone(), ctx: self.ctx.clone(), running: self.running.clone() };
        let cache = self.reshade_cache();
        // Counts as running while it starts, so a second click doesn't launch twice.
        t.running.lock().unwrap().insert(g.id);
        std::thread::spawn(move || {
            let hdr = if g.auto_hdr && g.installed {
                gamut_core::hdr::Guard::acquire()
                    .map_err(|e| {
                        let _ = t.tx.send(Event::Error(format!("Couldn't turn HDR on: {e:#}")));
                    })
                    .ok()
            } else {
                None
            };
            with_reshade(&g, &rs, &cache, &t.tx);
            let guard = if sk.mode == "off" && g.specialk != Some(true) { None } else { with_specialk(&g, &sk, &t.tx) };
            match launch::launch_with(&g, &specialk::game_env(&sk, &g)) {
                Ok(l) => t.track(&g, l, guard, hdr),
                Err(e) => {
                    t.running.lock().unwrap().remove(&g.id);
                    let _ = t.tx.send(Event::Error(format!("Couldn't start {}: {e:#}", g.name)));
                }
            }
            t.ctx.request_repaint();
        });
        Ok(())
    }

    /// Where the downloaded ReShade and shader packages live.
    pub fn reshade_cache(&self) -> std::path::PathBuf {
        self.paths.root.join("reshade")
    }

    /// Downloads (or with `force`, re-downloads) ReShade and its shaders.
    pub fn reshade_download(&self, force: bool) {
        let (cache, tx, ctx) = (self.reshade_cache(), self.tx.clone(), self.ctx.clone());
        let _ = tx.send(Event::Info("Downloading ReShade…".into()));
        std::thread::spawn(move || {
            match gamut_core::reshade::download(&cache, force) {
                Ok(v) => {
                    let _ = tx.send(Event::Info(format!("ReShade {v} is ready (Standard effects + SweetFX).")));
                }
                Err(e) => {
                    let _ = tx.send(Event::Error(format!("ReShade download failed: {e:#}")));
                }
            }
            ctx.request_repaint();
        });
    }
}

impl Workers {
    /// Checks SK's update feed at start, every 6 hours, and when the channel changes.
    fn spawn_sk_update_check(&self) {
        let (cfg, tx, ctx, slot) = (self.cfg.clone(), self.tx.clone(), self.ctx.clone(), self.sk_update.clone());
        std::thread::spawn(move || {
            let mut last: Option<(Instant, String)> = None;
            let mut told = String::new();
            loop {
                let sk = cfg.read().unwrap().specialk.clone();
                let due = last.as_ref().is_none_or(|(t, ch)| *ch != sk.channel || t.elapsed() > Duration::from_secs(6 * 3600));
                if let (true, true, Some(dir)) = (sk.check_updates, due, specialk::install_dir(&sk.path)) {
                    last = Some((Instant::now(), sk.channel.clone()));
                    match specialk::update::check(&dir, &sk.channel) {
                        Ok(u) => {
                            if let Some(u) = &u {
                                if told != u.latest {
                                    told = u.latest.clone();
                                    log::info!("Special K update available: {} -> {}", u.current, u.latest);
                                    let _ = tx.send(Event::Info(format!(
                                        "Special K {} is available (you have {}). Install it from Settings → Special K.",
                                        u.latest, u.current
                                    )));
                                }
                            }
                            *slot.lock().unwrap() = u;
                            ctx.request_repaint();
                        }
                        Err(e) => log::warn!("Special K update check failed: {e:#}"),
                    }
                }
                std::thread::sleep(Duration::from_secs(300));
            }
        });
    }

    /// Downloads, verifies and runs the Special K installer.
    pub fn install_sk_update(&self) {
        let Some(u) = self.sk_update.lock().unwrap().clone() else { return };
        let sk = self.cfg.read().unwrap().specialk.clone();
        let Some(dir) = specialk::install_dir(&sk.path) else { return };
        if self.sk_installing.swap(true, Ordering::SeqCst) {
            return;
        }
        let (tx, ctx, slot, busy) = (self.tx.clone(), self.ctx.clone(), self.sk_update.clone(), self.sk_installing.clone());
        std::thread::spawn(move || {
            match specialk::update::install(&dir, &u) {
                Ok(()) => {
                    let now = specialk::update::installed_version(&dir).unwrap_or_default();
                    log::info!("Special K updated to {now}");
                    *slot.lock().unwrap() = None;
                    let _ = tx.send(Event::Info(format!("Special K updated ({now}).")));
                }
                Err(e) => {
                    let _ = tx.send(Event::Error(format!("Special K update failed: {e:#}")));
                }
            }
            busy.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }
}

/// What a launch needs to record playtime when the game exits.
struct Tracker {
    paths: Paths,
    tx: Sender<Event>,
    ctx: Context,
    running: Arc<Mutex<HashSet<i64>>>,
}

/// SK's service handle plus how to release it; None when SK isn't used for this launch.
type SkGuard = Option<(specialk::Guard, Option<specialk::InjectionSignal>, String)>;

/// Installs ReShade into the game when it should have it. Problems are reported, never fatal.
fn with_reshade(g: &Game, rs: &gamut_core::reshade::Settings, cache: &std::path::Path, tx: &Sender<Event>) {
    use gamut_core::reshade::{self, Outcome};
    if let Err(why) = reshade::decide(g, rs) {
        if rs.enabled || g.reshade.is_some() {
            log::info!("ReShade: skipping {}: {why}", g.name);
        }
        return;
    }
    match reshade::install(g, cache) {
        Ok(Outcome::Installed(p)) => {
            let _ = tx.send(Event::Info(format!("ReShade installed into {} ({}). Press Home in game to open it.", g.name, p.display())));
        }
        Ok(Outcome::Skipped(why)) => log::info!("ReShade: skipping {}: {why}", g.name),
        Ok(Outcome::AlreadyInstalled) => {}
        Err(e) => {
            let _ = tx.send(Event::Error(format!("Installing ReShade into {} failed: {e:#}", g.name)));
        }
    }
}

/// Starts Special K's service if this game should get it. Problems are reported, never fatal.
fn with_specialk(g: &Game, sk: &specialk::Settings, tx: &Sender<Event>) -> SkGuard {
    if let Err(why) = specialk::decide(g, sk) {
        log::info!("Special K: skipping {}: {why}", g.name);
        return None;
    }
    let Some(dir) = specialk::install_dir(&sk.path) else {
        let _ = tx.send(Event::Error("Special K wasn't found. Install it, or set its folder in Settings.".into()));
        return None;
    };
    // Created before the game starts so SK can signal it.
    let signal = specialk::InjectionSignal::new();
    match specialk::acquire(&dir) {
        Ok(guard) => {
            specialk::prepare(&dir, sk, g);
            log::info!("Special K: injecting into {} (stop: {})", g.name, sk.stop);
            Some((guard, signal, sk.stop.clone()))
        }
        Err(e) => {
            let _ = tx.send(Event::Error(format!("Special K service failed to start: {e:#}")));
            None
        }
    }
}

/// Releases the SK service per the stop setting; "exit" falls back to injection for launches
/// Gamut can't follow (Steam, URLs).
fn release_specialk(sk: SkGuard, name: String, exited: bool) {
    let Some((guard, signal, stop)) = sk else { return };
    match stop.as_str() {
        "never" => std::mem::forget(guard),
        "exit" if exited => drop(guard),
        _ => {
            std::thread::spawn(move || {
                let injected = signal.is_some_and(|s| s.wait(Duration::from_secs(300)));
                if injected {
                    log::info!("Special K: injected into {name}");
                } else {
                    log::warn!("Special K: no injection reported for {name} within 5 minutes; stopping the service");
                }
                drop(guard);
            });
        }
    }
}

impl Tracker {
    /// Adds the session's playtime when a tracked game exits, and releases Special K and HDR.
    fn track(&self, g: &Game, launched: Launched, sk: SkGuard, hdr: Option<gamut_core::hdr::Guard>) {
        let (id, name) = (g.id, g.name.clone());
        let exit_stop = sk.as_ref().is_some_and(|s| s.2 == "exit");
        let Launched::Tracked(handle) = launched else {
            self.running.lock().unwrap().remove(&id);
            release_specialk(sk, name, false);
            if let Some(h) = hdr {
                let g = g.clone();
                std::thread::spawn(move || {
                    wait_exit(&g);
                    drop(h);
                });
            }
            return;
        };
        let (paths, tx, ctx, running) = (self.paths.clone(), self.tx.clone(), self.ctx.clone(), self.running.clone());
        running.lock().unwrap().insert(id);
        let sk = if exit_stop {
            sk
        } else {
            release_specialk(sk, name.clone(), false);
            None
        };
        std::thread::spawn(move || {
            let r = handle.join();
            release_specialk(sk, name, true);
            drop(hdr);
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

/// Waits until a game Gamut can't track directly (Steam, shortcuts) has started and exited again.
fn wait_exit(g: &Game) {
    let running: Box<dyn Fn() -> bool> = match (g.source, g.steam_appid()) {
        (gamut_core::Source::Steam, Some(app)) => Box::new(move || steam::running_appid() == Some(app)),
        _ => {
            let dirs = gamut_core::procs::game_dirs(g);
            Box::new(move || !gamut_core::procs::under(&dirs).is_empty())
        }
    };
    let start = Instant::now();
    while !running() {
        if start.elapsed() > Duration::from_secs(600) {
            log::warn!("{} didn't seem to start within 10 minutes", g.name);
            return;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    while running() {
        std::thread::sleep(Duration::from_secs(3));
    }
    log::info!("{} exited", g.name);
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
