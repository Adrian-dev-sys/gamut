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

mod backups;
mod collections;
mod list;
mod storage;
mod stores;
mod uninstall;

use gamut_core::model::Collection;

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
    (Sort::PlayCount, "Play count"),
    (Sort::Added, "Recently added"),
    (Sort::Released, "Release date"),
    (Sort::CriticScore, "Critic score"),
    (Sort::CommunityScore, "User score"),
    (Sort::Completion, "Completion status"),
];

/// Grid order for completion statuses; anything unknown sorts after these.
const COMPLETION_ORDER: [&str; 8] = ["Playing", "Beaten", "Completed", "Played", "On Hold", "Plan to Play", "Not Played", "Abandoned"];

/// The library filters (hidden games only under Hidden); empty strings mean any.
fn matches(g: &Game, filter: Filter, platform: &str, tag: &str, completion: &str) -> bool {
    let pass = match filter {
        Filter::Hidden => g.hidden,
        _ if g.hidden => false,
        Filter::All => true,
        Filter::Installed => g.installed,
        Filter::Favorites => g.favorite,
        Filter::Steam => g.source == Source::Steam,
        Filter::Custom => g.source == Source::Custom,
    };
    pass && (platform.is_empty() || platforms_of(g).any(|p| p == platform))
        && (tag.is_empty() || split_list(&g.tags).any(|t| t == tag))
        && (completion.is_empty() || completion_of(g) == completion)
}

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

const SETTINGS_TABS: [&str; 6] = ["Library", "Stores", "Special K", "ReShade", "Backups", "General"];

enum Action {
    Play(i64),
    /// Play after the "another Steam game is running" warning.
    PlayAnyway(i64),
    Edit(i64),
    /// One of the game's extra play actions (index into `Game::actions`).
    RunAction(i64, usize),
    OpenFolder(i64),
    OpenSaves(i64),
    ToggleFav(i64),
    ToggleHidden(i64),
    Refresh(i64),
    Remove(i64),
    Uninstall(i64),
    PickArt(i64, ArtKind),
    /// Ask to close a running game.
    Stop(i64),
    /// Applies to every selected game.
    Bulk(Bulk),
    /// Puts a game (None: the selection) in a hand-filled collection, or takes it out.
    Collect(i64, Option<i64>, bool),
    /// Opens the new collection dialog for a game (None: the selection).
    NewCollection(Option<i64>),
}

#[derive(Clone, Copy)]
enum Bulk {
    Fav(bool),
    Hide(bool),
    Refresh,
}

/// Space above the first grid row and below the last.
const GRID_MARGIN: f32 = 18.0;

/// Filters reachable from big screen tabs and LB/RB, with their labels.
const BIG_TABS: [(Filter, &str); 5] =
    [(Filter::All, "All games"), (Filter::Installed, "Installed"), (Filter::Favorites, "Favorites"), (Filter::Steam, "Steam"), (Filter::Custom, "Custom")];

/// A grid cursor move (keyboard or controller).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Nav {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
}


pub struct GamutApp {
    tray: Option<crate::tray::Tray>,
    /// Exit was chosen (tray menu): closing really closes.
    quitting: bool,
    /// Frames left to keep the window hidden after starting in the tray (eframe shows it after the first paint).
    keep_hidden: u8,
    /// The window is hidden in the tray (egui's `visible()` only tracks minimized/occluded).
    hidden: bool,
    hwnd: Option<isize>,
    /// Restore maximized on the first show after starting in the tray.
    maximize_on_show: bool,
    tray_refreshed: Option<std::time::Instant>,
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
    /// Keyboard/controller cursor in the grid, and whether to draw it (mouse use hides it).
    cursor: Option<i64>,
    kb_focus: bool,
    /// Multi-selection and the Shift range anchor.
    multi: HashSet<i64>,
    anchor: Option<i64>,
    /// Grid layout from the last frame, for cursor moves: columns, rows per page, row pitch,
    /// (scroll offset, viewport height), and a pending scroll.
    grid_cols: usize,
    grid_page: usize,
    grid_pitch: f32,
    grid_scroll: (f32, f32),
    scroll_to: Option<f32>,
    confirm_refresh: Option<Vec<i64>>,
    pad: Receiver<crate::gamepad::Pad>,
    /// Controller input left for `handle_pad` (some becomes arrow/Enter keys in `raw_input_hook`).
    pad_buf: Vec<crate::gamepad::Pad>,
    /// Focus the details page's main button next frame (opened from controller/keyboard).
    focus_play: bool,
    /// Details page scroll: pending change, and the offset last frame.
    detail_scroll: Option<f32>,
    detail_off: f32,
    /// Selection last seen, and whether the details were opened since last drawn.
    detail_seen: Option<i64>,
    detail_fresh: bool,
    search_pop: Option<crate::search_pop::SearchPop>,
    /// Big screen mode: the zoom factor to restore when leaving it.
    big: Option<f32>,
    thumbs: ThumbCache,
    /// Covers for the Achievements tab rows, decoded at their exact size.
    small_thumbs: ThumbCache,
    workers: Workers,
    hero: Option<(i64, Option<TextureHandle>)>,
    hero_tx: Sender<(i64, Option<ColorImage>)>,
    hero_rx: Receiver<(i64, Option<ColorImage>)>,
    editor: Option<Editor>,
    /// Genre and completion suggestions, gathered when the editor opens.
    ed_lists: (Vec<(String, usize)>, Vec<(String, usize)>),
    settings: Option<Config>,
    confirm_remove: Option<i64>,
    /// Game, its uninstaller, and whether to also remove it from the library.
    confirm_uninstall: Option<(i64, gamut_core::uninstall::Uninstaller, bool)>,
    confirm_stop: Option<i64>,
    health: Option<crate::health_ui::HealthDialog>,
    /// (game to start, Steam game running) when Steam would have to close one for the other.
    confirm_play: Option<(i64, i64)>,
    /// Vulkan game about to get both Special K and ReShade: asks which to use (once per game).
    confirm_hooks: Option<i64>,
    /// Controller menu (Start) for this game.
    game_menu: Option<i64>,
    scans: Option<crate::scan_ui::ScanDialog>,
    /// A dialog was open last frame (the first controller frame in a new one focuses its first widget).
    was_dialog: bool,
    /// Controller input held for the next frame (egui moves focus once per frame).
    pad_later: Vec<crate::gamepad::Pad>,
    /// On-screen keyboard typing into the focused text field.
    osk: Option<crate::osk::Osk>,
    /// Details: a D-pad move from this focused widget; if focus didn't change, the page scrolls instead.
    nav_probe: Option<(egui::Id, f32)>,
    settings_tab: usize,
    backup_list: backups::BackupList,
    storage: storage::StorageView,
    /// Backup the user asked to restore (confirming).
    confirm_restore: Option<std::path::PathBuf>,
    store_review: Option<stores::Review>,
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
    saved_view: (Sort, Filter, String, String, String, Option<i64>),
    /// Achievements of the game on the details page.
    ach: Option<crate::achievements::GameAch>,
    ra_pick: Option<crate::achievements::RetroPick>,
    /// The Achievements tab, while it's the one shown.
    ach_tab: Option<crate::ach_tab::Overview>,
    stats_tab: Option<crate::stats_tab::Stats>,
    /// The all-games Screenshots tab is showing.
    shots_tab: bool,
    shots: crate::shots_view::ShotsView,
    /// Collections (Favorites first), the one the library is narrowed to, and the Collections tab.
    collections: Vec<Collection>,
    collection: Option<i64>,
    coll_tab: bool,
    /// Keyboard/controller cursor on the Collections tab (tile index), columns last frame, scroll-to deadline.
    coll_cursor: Option<usize>,
    coll_cols: usize,
    coll_scroll_until: f64,
    coll_stats: Option<collections::Stats>,
    /// Top bar widths last frame: logo + tabs, library controls, app buttons.
    bar_w: (f32, f32, f32),
    /// The collection whose sort was last applied, and the library sort it replaced.
    coll_seen: Option<i64>,
    sort_before: Option<Sort>,
    coll_dialog: Option<collections::CollDialog>,
    confirm_coll_delete: Option<i64>,
    /// Details page tab: 0 = About, 1 = Achievements (kept while browsing games).
    detail_tab: usize,
    /// Tab for the next details opened (About otherwise).
    detail_tab_next: Option<usize>,
    sessions: Option<crate::sessions_view::GameSessions>,
    /// HowLongToBeat times of the selected game, from the cache.
    hltb: Option<(i64, Option<gamut_core::hltb::Times>)>,
    key_checks: crate::keycheck::KeyChecks,
    theme_watch: theme::Watcher,
}

impl GamutApp {
    pub fn new(cc: &eframe::CreationContext, paths: Paths) -> anyhow::Result<Self> {
        let theme_err = theme::load(&theme::file(&paths.root)).err();
        let theme_watch = theme::Watcher::new(&paths.root, &cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let loaded = Config::load(&paths.config);
        let tray = crate::tray::Tray::new(&cc.egui_ctx);
        let (sort, filter) = (from_key(&SORTS, &loaded.sort), from_key(&FILTERS, &loaded.filter));
        let window = loaded.window;
        let (platform, tag, completion) = (loaded.platform.clone(), loaded.tag.clone(), loaded.completion.clone());
        let saved_coll = loaded.collection;
        let cfg = Arc::new(RwLock::new(loaded));
        let (lib, repair_notice) = Library::open_checked(&paths.db)?;
        let games = lib.all()?;
        let (platforms, tags) = facets(&games);
        let completions = completion_facets(&games);
        let workers = Workers::start(&cc.egui_ctx, paths.clone(), cfg.clone());
        workers.igdb_requeue();
        workers.rescan();
        let (hero_tx, hero_rx) = unbounded();
        let mut app = GamutApp {
            tray,
            quitting: false,
            keep_hidden: 0,
            hidden: false,
            hwnd: crate::startup::hwnd(cc),
            maximize_on_show: false,
            tray_refreshed: None,
            thumbs: ThumbCache::new(&cc.egui_ctx, paths.clone(), 3),
            small_thumbs: ThumbCache::new(&cc.egui_ctx, paths.clone(), 2),
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
            cursor: None,
            kb_focus: false,
            multi: HashSet::new(),
            anchor: None,
            grid_cols: 1,
            grid_page: 1,
            grid_pitch: 1.0,
            grid_scroll: (0.0, 0.0),
            scroll_to: None,
            confirm_refresh: None,
            pad: crate::gamepad::start(cc.egui_ctx.clone()),
            pad_buf: Vec::new(),
            focus_play: false,
            detail_scroll: None,
            detail_off: 0.0,
            detail_seen: None,
            detail_fresh: false,
            search_pop: None,
            big: None,
            workers,
            hero: None,
            hero_tx,
            hero_rx,
            editor: None,
            ed_lists: Default::default(),
            settings: None,
            confirm_remove: None,
            confirm_uninstall: None,
            confirm_stop: None,
            health: None,
            confirm_play: None,
            confirm_hooks: None,
            game_menu: None,
            scans: None,
            was_dialog: false,
            pad_later: Vec::new(),
            osk: None,
            nav_probe: None,
            settings_tab: 0,
            backup_list: Default::default(),
            storage: Default::default(),
            confirm_restore: None,
            store_review: None,
            picker: None,
            ctx_handle: cc.egui_ctx.clone(),
            toasts: Toasts::with(repair_notice.or(theme_err)),
            playnite: None,
            playnite_sure: false,
            focus_search: false,
            opened_at: 0.0,
            startup: window.as_ref().filter(|w| w.maximized).and_then(|_| crate::startup::Startup::maximize_hidden(cc)),
            window,
            saved_view: (sort, filter, platform, tag, completion, saved_coll),
            ach: None,
            ra_pick: None,
            ach_tab: None,
            stats_tab: None,
            shots_tab: false,
            shots: crate::shots_view::ShotsView::new(),
            detail_tab: 0,
            detail_tab_next: None,
            sessions: None,
            hltb: None,
            key_checks: Default::default(),
            theme_watch,
            collections: Vec::new(),
            collection: saved_coll,
            coll_tab: false,
            coll_cursor: None,
            coll_cols: 1,
            coll_scroll_until: 0.0,
            coll_stats: None,
            bar_w: (620.0, 9999.0, 660.0),
            coll_seen: None,
            sort_before: None,
            coll_dialog: None,
            confirm_coll_delete: None,
        };
        app.load_collections();
        Ok(app)
    }

    /// Ids of games running now: ones Gamut launched and is tracking, plus whatever Steam reports.
    /// List mode shows the open game in a side panel instead of the popup.
    fn side(&self) -> bool {
        self.cfg.read().unwrap().list_view && self.big.is_none() && self.ach_tab.is_none() && self.stats_tab.is_none() && !self.shots_tab && !self.coll_tab
    }

    /// The details popup is up (not the list's side panel).
    fn popup(&self) -> bool {
        self.selected.is_some() && !self.side()
    }

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

    /// One game, or (None) the selected games in view order.
    fn ids_for(&self, game: Option<i64>) -> Vec<i64> {
        match game {
            Some(id) => vec![id],
            None => self.view.iter().map(|&i| self.games[i].id).filter(|id| self.multi.contains(id)).collect(),
        }
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
        self.load_collections();
        if let Some(h) = self.health.as_mut() {
            h.stale = true;
        }
    }

    fn reload_one(&mut self, id: i64) {
        if let Ok(Some(g)) = self.lib.get(id) {
            if let Some(slot) = self.games.iter_mut().find(|x| x.id == id) {
                *slot = g;
            } else {
                self.games.push(g);
            }
            self.view_key = None;
            self.coll_stats = None;
        }
        self.thumbs.invalidate(id);
        self.small_thumbs.invalidate(id);
        // New art from the picker shows in the editor's Media tab.
        if let Some(ed) = self.editor.as_mut().filter(|e| e.id == id) {
            ed.reload_art();
        }
        if self.hero.as_ref().is_some_and(|(h, t)| *h == id && t.is_none()) {
            self.hero = None;
        }
    }

    fn handle_events(&mut self) {
        while let Ok(ev) = self.workers.rx.try_recv() {
            if let (Some(st), Event::LibraryChanged | Event::GameUpdated(_) | Event::Achievements(_)) = (self.stats_tab.as_mut(), &ev) {
                st.dirty = true;
            }
            match ev {
                Event::Shots(v) => self.shots.set_all(v),
                Event::ShotTaken => self.workers.index_shots(),
                Event::StoreReview(store, r) => match r {
                    Ok(plan) => self.store_review = Some(stores::Review::new(store, plan)),
                    Err(e) => self.toasts.error(format!("Reading {} failed: {e}", store.name())),
                },
                Event::LibraryChanged => {
                    self.workers.index_shots();
                    self.sessions = None; // Steam scans add sessions
                    self.reload_all()
                }
                Event::GameUpdated(id) => {
                    if self.sessions.as_ref().is_some_and(|s| s.id == id) {
                        self.sessions = None;
                    }
                    if self.hltb.as_ref().is_some_and(|h| h.0 == id) {
                        self.hltb = None;
                    }
                    self.reload_one(id)
                }
                Event::Error(e) => self.toasts.error(e),
                Event::Info(m) => self.toasts.info(m),
                Event::Achievements(id) => {
                    if let Some(ov) = self.ach_tab.as_mut() {
                        ov.dirty = true;
                    }
                    if let Some(a) = self.ach.as_mut().filter(|a| a.id == id) {
                        let show_all = a.show_all;
                        *a = crate::achievements::GameAch::load(&self.lib, id);
                        a.show_all = show_all;
                    }
                }
                Event::PlaynitePreview(Ok(p)) => self.playnite = Some(p),
                Event::PlaynitePreview(Err(e)) => self.toasts.error(e),
                Event::PlayniteImported(r) => {
                    // Covers were replaced in bulk.
                    self.thumbs.clear();
                    self.small_thumbs.clear();
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
        self.apply_coll_sort();
        let key = (
            self.search.clone(),
            self.filter,
            self.sort,
            self.games.len(),
            self.platform.clone(),
            format!("{}\u{1}{}\u{1}{:?}", self.tag, self.completion, self.collection),
        );
        if self.view_key.as_ref() == Some(&key) {
            return;
        }
        let q = self.search.to_lowercase();
        let coll = self.collection.and_then(|id| self.collections.iter().find(|c| c.id == id));
        let mut v: Vec<usize> = (0..self.games.len())
            .filter(|&i| {
                let g = &self.games[i];
                matches(g, self.filter, &self.platform, &self.tag, &self.completion)
                    && coll.is_none_or(|c| collections::contains(c, g))
                    && (q.is_empty() || g.name.to_lowercase().contains(&q))
            })
            .collect();
        let gs = &self.games;
        match self.sort {
            Sort::Name => v.sort_by_cached_key(|&i| util::nat_key(sort_name(&gs[i]))),
            Sort::LastPlayed => v.sort_by_key(|&i| std::cmp::Reverse(gs[i].last_played)),
            Sort::Playtime => v.sort_by_key(|&i| std::cmp::Reverse(gs[i].playtime_min)),
            Sort::Added => v.sort_by_key(|&i| std::cmp::Reverse((gs[i].added, gs[i].id))),
            // Highest first; games without a value go last, then by name.
            Sort::Released => v.sort_by_cached_key(|&i| {
                let d = gs[i].release_date.as_deref().map(sortable_date).unwrap_or_default();
                (d.is_empty(), std::cmp::Reverse(d), util::nat_key(sort_name(&gs[i])))
            }),
            Sort::CriticScore => v.sort_by_cached_key(|&i| (std::cmp::Reverse(gs[i].critic_score), util::nat_key(sort_name(&gs[i])))),
            Sort::CommunityScore => v.sort_by_cached_key(|&i| (std::cmp::Reverse(gs[i].community_score), util::nat_key(sort_name(&gs[i])))),
            Sort::PlayCount => v.sort_by_cached_key(|&i| (std::cmp::Reverse(gs[i].play_count), std::cmp::Reverse(gs[i].playtime_min))),
            Sort::Completion => v.sort_by_cached_key(|&i| {
                let c = completion_of(&gs[i]);
                let rank = COMPLETION_ORDER.iter().position(|s| *s == c).unwrap_or(COMPLETION_ORDER.len());
                (rank, c.to_string(), util::nat_key(sort_name(&gs[i])))
            }),
        }
        // Selected games that left the view (filtered, hidden) drop out of the selection.
        let ids: HashSet<i64> = v.iter().map(|&i| gs[i].id).collect();
        self.multi.retain(|id| ids.contains(id));
        self.view = v;
        self.view_key = Some(key);
    }

    fn apply(&mut self, a: Action) {
        let r: anyhow::Result<()> = (|| {
            match a {
                Action::Play(id) => {
                    let Some(g) = self.game(id).cloned() else { return Ok(()) };
                    let running = self.running();
                    if running.contains(&id) {
                        return Ok(());
                    }
                    // A store game that isn't installed: open its launcher, and sync once it's installed.
                    if g.needs_install() && g.source == Source::Custom {
                        gamut_core::launch::launch(&g)?;
                        self.workers.watch_install(&g);
                        return Ok(());
                    }
                    // Steam runs one game at a time; other games may run side by side.
                    if g.installed && g.source == Source::Steam {
                        let other = self.games.iter().find(|o| o.source == Source::Steam && running.contains(&o.id)).map(|o| o.id);
                        if let Some(other) = other {
                            self.confirm_play = Some((id, other));
                            return Ok(());
                        }
                    }
                    let clash = {
                        let c = self.cfg.read().unwrap();
                        gamut_core::reshade::vulkan_clash(&g, &c.reshade, &c.specialk)
                    };
                    if clash {
                        self.confirm_hooks = Some(id);
                        return Ok(());
                    }
                    self.workers.play(g)?;
                }
                Action::PlayAnyway(id) => {
                    if let Some(g) = self.game(id).cloned() {
                        self.workers.play(g)?;
                    }
                }
                Action::Edit(id) => {
                    if let Some(g) = self.game(id) {
                        let mut ed = Editor::from_game(g);
                        ed.art_paths = Some((self.paths.cover(id), self.paths.hero(id)));
                        ed.icon_path = Some(self.paths.icon(id));
                        ed.shared = gamut_core::shared_folders(&self.games);
                        self.editor = Some(ed);
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
                Action::Uninstall(id) => self.uninstall(id)?,
                Action::RunAction(id, i) => {
                    let Some(g) = self.game(id).cloned() else { return Ok(()) };
                    let Some(a) = g.actions().get(i).cloned() else { return Ok(()) };
                    if a.track && self.running().contains(&id) {
                        anyhow::bail!("{} is already running", g.name);
                    }
                    if a.track {
                        self.workers.play(g.with_action(&a))?;
                    } else {
                        gamut_core::launch::launch(&g.with_action(&a))?;
                    }
                }
                Action::OpenFolder(id) => {
                    let Some(d) = self.game(id).and_then(|g| g.folder()).filter(|d| d.is_dir()) else {
                        anyhow::bail!("this game's folder isn't there");
                    };
                    open::that_detached(d)?;
                }
                Action::OpenSaves(id) => {
                    let Some(g) = self.game(id).cloned() else { return Ok(()) };
                    match g.save_dir.as_deref().filter(|d| std::path::Path::new(d).is_dir()) {
                        Some(d) => open::that_detached(d)?,
                        None => self.workers.find_saves(g),
                    }
                }
                Action::Stop(id) => self.confirm_stop = Some(id),
                Action::Bulk(op) => {
                    let ids: Vec<i64> = self.view.iter().map(|&i| self.games[i].id).filter(|id| self.multi.contains(id)).collect();
                    match op {
                        Bulk::Fav(on) => ids.iter().try_for_each(|&id| self.lib.set_favorite(id, on))?,
                        Bulk::Hide(on) => ids.iter().try_for_each(|&id| self.lib.set_hidden(id, on))?,
                        Bulk::Refresh => {
                            self.confirm_refresh = Some(ids);
                            return Ok(());
                        }
                    }
                    self.reload_all();
                }
                Action::Collect(cid, game, on) => {
                    let ids = self.ids_for(game);
                    self.collect(cid, &ids, on)?;
                }
                Action::NewCollection(game) => {
                    let ids = self.ids_for(game);
                    self.coll_dialog = Some(collections::CollDialog::new(ids, None));
                }
                Action::PickArt(id, kind) => {
                    if let Some(g) = self.game(id) {
                        let cfg = self.cfg.read().unwrap().clone();
                        self.picker = Some(Picker::open(&self.ctx_handle, g, kind, &cfg, self.paths.clone()));
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

    /// Logo, tabs and app buttons; the library controls share the row only when everything fits.
    fn top_bar(&mut self, ui: &mut Ui) {
        let library = self.ach_tab.is_none() && self.stats_tab.is_none() && !self.shots_tab && !self.coll_tab;
        let full = ui.available_width();
        let (tabs_w, controls_w, buttons_w) = self.bar_w;
        // Narrow: the buttons fold into a menu. Not very wide: the controls get a row of their own.
        let menu = full < tabs_w + buttons_w + 16.0;
        let one_row = library && !menu && tabs_w + controls_w + buttons_w + 24.0 < full;
        ui.horizontal(|ui| {
            let left = ui.cursor().left();
            theme::logo(ui);
            ui.add_space(14.0);
            let now = ui.input(|i| i.time);
            let tab = |ui: &mut Ui, on: bool, label: &str| ui.selectable_label(on, RichText::new(label).size(15.0)).clicked();
            if tab(ui, library && self.collection.is_none(), "Library") {
                (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, false);
                self.collection = None;
            }
            if tab(ui, self.coll_tab || (library && self.collection.is_some()), "Collections") {
                (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, true);
                self.collection = None;
            }
            if tab(ui, self.ach_tab.is_some(), "Achievements") && self.ach_tab.is_none() {
                (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (Some(crate::ach_tab::Overview::load(&self.lib, now)), None, false, false);
            }
            if tab(ui, self.stats_tab.is_some(), "Statistics") && self.stats_tab.is_none() {
                (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, Some(crate::stats_tab::Stats::load(&self.lib, now)), false, false);
            }
            if tab(ui, self.shots_tab, "Screenshots") {
                (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, true, false);
            }
            ui.add_space(14.0);
            self.bar_w.0 = ui.cursor().left() - left;
            if one_row {
                self.bar_w.1 = ui.scope(|ui| self.grid_controls(ui)).response.rect.width();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if menu {
                    ui.menu_button("Menu", |ui| {
                        ui.set_min_width(180.0);
                        self.app_buttons(ui, true);
                    });
                } else {
                    self.app_buttons(ui, false);
                    self.bar_w.2 = ui.min_rect().width();
                }
            });
        });
        if library && !one_row {
            ui.add_space(6.0);
            let r = ui.horizontal_wrapped(|ui| self.grid_controls(ui)).response.rect;
            // Only an unwrapped row tells the controls' real width.
            if r.height() < 40.0 {
                self.bar_w.1 = r.width();
            }
        }
    }

    /// Add game … Big screen, in reading order (`in_menu`) or right to left for the top bar.
    fn app_buttons(&mut self, ui: &mut Ui, in_menu: bool) {
        let scanning = self.workers.scanning.load(Ordering::Relaxed);
        let items: [(&str, bool, &str); 7] = [
            ("Add game", true, ""),
            ("Scan folders", true, "Find games in a folder and add them"),
            ("Health", true, "Find games that can't start and likely duplicates"),
            ("Refresh", !scanning, "Rescan Steam (and its collections) and sync the stores in Settings → Stores"),
            ("Random game", !self.view.is_empty(), "Open a random game from the ones shown (Ctrl+R)"),
            ("Settings", true, ""),
            ("Big screen", true, "Full screen, controller-friendly (F11, or View on a controller)"),
        ];
        let order: Vec<usize> = if in_menu { (0..items.len()).collect() } else { (0..items.len()).rev().collect() };
        for i in order {
            let (label, enabled, tip) = items[i];
            let mut r = ui.add_enabled(enabled, egui::Button::new(label));
            if !tip.is_empty() {
                r = r.on_hover_text(tip);
            }
            if !r.clicked() {
                continue;
            }
            if in_menu {
                ui.close();
            }
            let ctx = ui.ctx().clone();
            match i {
                0 => {
                    self.editor = Some(Editor::new_custom());
                    self.ed_lists = editor_facets(&self.games);
                }
                1 => self.scans = Some(crate::scan_ui::ScanDialog::new(self.cfg.read().unwrap().scanners.clone())),
                2 => self.health = Some(crate::health_ui::HealthDialog::new(self.cfg.read().unwrap().health.clone(), &self.games)),
                3 => self.workers.rescan(),
                4 => self.random_game(&ctx),
                5 => {
                    self.settings = Some(self.cfg.read().unwrap().clone());
                    self.storage.reset();
                }
                _ => self.toggle_big(&ctx),
            }
        }
    }

    /// Enters or leaves big screen: full screen, larger UI, the library with a visible cursor.
    fn toggle_big(&mut self, ctx: &egui::Context) {
        match self.big.take() {
            Some(zoom) => {
                ctx.set_zoom_factor(zoom);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            }
            None => {
                self.big = Some(ctx.zoom_factor());
                ctx.set_zoom_factor(ctx.zoom_factor() * 1.35);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                (self.ach_tab, self.stats_tab, self.shots_tab) = (None, None, false);
                if self.filter == Filter::Hidden {
                    self.filter = Filter::All;
                }
                self.kb_focus = true;
            }
        }
    }

    /// Big screen top bar: filter tabs and the way out.
    fn big_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            theme::logo(ui);
            ui.add_space(24.0);
            let in_coll = self.coll_tab || self.collection.is_some();
            for (f, label) in BIG_TABS {
                if ui.selectable_label(self.filter == f && !in_coll, RichText::new(label).size(17.0)).clicked() {
                    self.filter = f;
                    (self.coll_tab, self.collection) = (false, None);
                }
                ui.add_space(4.0);
            }
            if ui.selectable_label(in_coll, RichText::new("Collections").size(17.0)).clicked() {
                (self.coll_tab, self.collection) = (true, None);
            }
            if let Some(c) = self.collection.and_then(|id| self.collections.iter().find(|c| c.id == id)) {
                ui.label(RichText::new(format!("›  {}", c.name)).size(17.0).color(Color32::WHITE));
            }
            ui.add_space(16.0);
            let label = if self.search.is_empty() { "Search  (Y)".to_string() } else { format!("Search: {}", self.search) };
            if ui.button(RichText::new(label).size(15.0)).clicked() {
                self.search_pop = Some(crate::search_pop::SearchPop::new());
            }
            if !self.search.is_empty() && ui.button(RichText::new("Clear").size(15.0)).clicked() {
                self.search.clear();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button(RichText::new("Exit big screen").size(15.0)).clicked() {
                    let ctx = ui.ctx().clone();
                    self.toggle_big(&ctx);
                }
                if ui.add_enabled(!self.view.is_empty(), egui::Button::new(RichText::new("Random game").size(15.0))).clicked() {
                    let ctx = ui.ctx().clone();
                    self.random_game(&ctx);
                }
                ui.label(RichText::new(format!("{} games", self.view.len())).color(TEXT_DIM()).size(15.0));
            });
        });
    }

    /// Controller buttons; ignored while Gamut isn't the focused window (e.g. a game is being played).
    fn handle_pad(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        use crate::gamepad::Pad;
        let events: Vec<Pad> = std::mem::take(&mut self.pad_buf);
        if events.is_empty() || !ctx.input(|i| i.focused) {
            return;
        }
        if self.search_pop.is_some() {
            for p in events {
                if matches!(p, Pad::BigScreen) {
                    self.toggle_big(ctx);
                } else if self.search_pop.as_mut().is_some_and(|sp| sp.pad(p, &mut self.search)) {
                    self.close_search();
                    return;
                }
            }
            return;
        }
        for p in events {
            let grid = self.ach_tab.is_none() && self.stats_tab.is_none() && !self.shots_tab && !self.coll_tab;
            match p {
                Pad::BigScreen => self.toggle_big(ctx),
                // In dialogs the D-pad/A drive egui focus (raw_input_hook); B cancels simple ones.
                Pad::Back if self.any_dialog() => {
                    (self.confirm_play, self.game_menu, self.confirm_stop, self.confirm_hooks) = (None, None, None, None);
                    (self.confirm_remove, self.confirm_refresh) = (None, None);
                    self.shots.viewer = None;
                }
                _ if self.any_dialog() => {}
                Pad::Menu => {
                    if let Some(id) = self.selected.or(self.cursor.filter(|_| self.kb_focus && grid)) {
                        self.game_menu = Some(id);
                    }
                }
                Pad::Nav(n) if !self.popup() && grid => self.nav(n, false),
                Pad::Nav(n) if self.coll_tab => self.coll_nav(n),
                Pad::Confirm if self.coll_tab => self.coll_confirm(),
                // Details: L2/R2 page through the description (the right stick in the side panel too).
                Pad::Nav(Nav::PageUp) if self.popup() => self.scroll_details(-420.0),
                Pad::Nav(Nav::PageDown) if self.popup() => self.scroll_details(420.0),
                Pad::Scroll(d) if self.selected.is_some() => self.scroll_details(d as f32 * 60.0),
                Pad::Scroll(_) | Pad::Screenshot => {}
                Pad::Nav(_) => {}
                Pad::Search if !self.popup() => {
                    (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, false);
                    if self.big.is_some() {
                        self.search_pop = Some(crate::search_pop::SearchPop::new());
                    } else {
                        self.focus_search = true;
                        self.osk = Some(crate::osk::Osk::new());
                    }
                }
                Pad::Search => {}
                Pad::Confirm if self.side() => match self.cursor.filter(|_| self.kb_focus).or(self.selected) {
                    Some(id) => actions.push(Action::Play(id)),
                    None => self.nav(Nav::Home, false),
                },
                Pad::Confirm => match self.selected {
                    Some(id) => actions.push(Action::Play(id)),
                    None if grid => match self.cursor.filter(|_| self.kb_focus) {
                        Some(id) => {
                            self.selected = Some(id);
                            self.opened_at = ctx.input(|i| i.time);
                            self.focus_play = true;
                        }
                        None => self.nav(Nav::Home, false),
                    },
                    None => {}
                },
                Pad::Back => self.back(),
                Pad::Favorite => {
                    if let Some(id) = self.selected.or(self.cursor.filter(|_| self.kb_focus)) {
                        actions.push(Action::ToggleFav(id));
                    }
                }
                // Big screen: filter tabs. Otherwise the top tabs (Library, Achievements, Statistics).
                Pad::PrevTab | Pad::NextTab if !self.popup() && self.big.is_some() => {
                    // The filter tabs, then Collections.
                    let n = BIG_TABS.len() as isize + 1;
                    let i = if self.coll_tab || self.collection.is_some() { n - 1 } else { BIG_TABS.iter().position(|(f, _)| *f == self.filter).unwrap_or(0) as isize };
                    let d = if matches!(p, Pad::NextTab) { 1 } else { -1 };
                    let to = (i + d).rem_euclid(n) as usize;
                    (self.coll_tab, self.collection) = (to == BIG_TABS.len(), None);
                    if let Some((f, _)) = BIG_TABS.get(to) {
                        self.filter = *f;
                    }
                    self.cursor = None;
                    self.scroll_to = Some(0.0);
                }
                Pad::PrevTab | Pad::NextTab if !self.popup() => {
                    let i = if self.coll_tab || self.collection.is_some() { 1 } else if self.ach_tab.is_some() { 2 } else if self.stats_tab.is_some() { 3 } else if self.shots_tab { 4 } else { 0 };
                    let d = if matches!(p, Pad::NextTab) { 1 } else { 4 };
                    let now = ctx.input(|i| i.time);
                    (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, false);
                    self.collection = None;
                    match (i + d) % 5 {
                        1 => self.coll_tab = true,
                        2 => self.ach_tab = Some(crate::ach_tab::Overview::load(&self.lib, now)),
                        3 => self.stats_tab = Some(crate::stats_tab::Stats::load(&self.lib, now)),
                        4 => self.shots_tab = true,
                        _ => {}
                    }
                }
                Pad::PrevTab | Pad::NextTab => {}
            }
            self.kb_focus = true;
        }
    }

    /// B / Esc-like step back: closes the game, clears the selection, leaves a collection or a tab.
    fn back(&mut self) {
        let grid = self.ach_tab.is_none() && self.stats_tab.is_none() && !self.shots_tab && !self.coll_tab;
        if self.selected.is_some() {
            self.selected = None;
        } else if !self.multi.is_empty() {
            self.multi.clear();
        } else if self.collection.take().is_some() {
            self.coll_tab = true;
            self.scroll_to = Some(0.0);
        } else if !grid {
            (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, false);
        }
    }

    fn scroll_details(&mut self, d: f32) {
        self.detail_scroll = Some(self.detail_scroll.unwrap_or(0.0) + d);
    }

    pub fn start_in_tray(&mut self) {
        let Some(h) = self.hwnd.filter(|_| self.tray.is_some()) else { return };
        crate::startup::cloak(h, true);
        self.keep_hidden = 6;
        self.hidden = true;
        // Maximizing would show the window; do it on the first show instead.
        self.maximize_on_show = self.startup.take().is_some();
        log::info!("started in the tray");
    }

    fn show_window(&mut self, ctx: &egui::Context) {
        if self.keep_hidden > 0 {
            self.keep_hidden = 0;
            if let Some(h) = self.hwnd {
                crate::startup::cloak(h, false);
            }
        }
        self.hidden = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        if std::mem::take(&mut self.maximize_on_show) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn close_search(&mut self) {
        self.search_pop = None;
        self.cursor = None;
        self.scroll_to = Some(0.0);
    }

    fn any_dialog(&self) -> bool {
        self.editor.is_some()
            || self.settings.is_some()
            || self.confirm_restore.is_some()
            || self.store_review.is_some()
            || self.confirm_uninstall.is_some()
            || self.confirm_remove.is_some()
            || self.confirm_refresh.is_some()
            || self.confirm_stop.is_some()
            || self.playnite.is_some()
            || self.picker.is_some()
            || self.ra_pick.is_some()
            || self.search_pop.is_some()
            || self.confirm_play.is_some()
            || self.confirm_hooks.is_some()
            || self.game_menu.is_some()
            || self.scans.is_some()
            || self.health.is_some()
            || self.shots.viewer.is_some()
            || self.coll_dialog.is_some()
            || self.confirm_coll_delete.is_some()
    }

    /// Search, filters, sort and cover size for the library grid.
    fn grid_controls(&mut self, ui: &mut Ui) {
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.search)
                .hint_text("Search  (Ctrl+F)")
                .desired_width(260.0),
        );
        if std::mem::take(&mut self.focus_search) {
            search.request_focus();
        }
        self.collection_chip(ui);
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
        let (mut w, list) = { let c = self.cfg.read().unwrap(); (c.card_width, c.list_view) };
        if !list
            && ui
                .add(egui::Slider::new(&mut w, 110.0..=280.0).show_value(false))
                .on_hover_text("Cover size")
                .changed()
        {
            self.cfg.write().unwrap().card_width = w;
        }
        for (on, label, tip) in [(false, "Grid", "Cover grid"), (true, "List", "One row per game, sortable columns")] {
            let b = ui.add(egui::Button::new(label).selected(list == on)).on_hover_text(tip);
            if b.clicked() && list != on {
                self.cfg.write().unwrap().list_view = on;
                self.scroll_to = Some(0.0);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let visible = self.games.iter().filter(|g| !g.hidden).count();
            let installed = self.games.iter().filter(|g| !g.hidden && g.installed).count();
            ui.label(RichText::new(format!("{visible} games · {installed} installed")).color(TEXT_DIM()));
            if self.workers.scanning.load(Ordering::Relaxed) {
                ui.spinner();
                ui.label(RichText::new("Scanning Steam…").color(TEXT_DIM()));
            }
            let imp = &self.workers.importing;
            let total = imp.1.load(Ordering::Relaxed);
            if total > 0 {
                ui.spinner();
                let done = imp.0.load(Ordering::Relaxed);
                ui.label(RichText::new(format!("Importing from Playnite: {done}/{total} images")).color(TEXT_DIM()));
            }
            let c = self.workers.pending_cover.load(Ordering::Relaxed);
            let m = self.workers.pending_meta.load(Ordering::Relaxed);
            if c + m > 0 {
                ui.spinner();
                ui.label(RichText::new(format!("Fetching: {c} covers, {m} metadata")).color(TEXT_DIM()));
            }
            if !self.workers.ach_busy.lock().unwrap().is_empty() {
                ui.spinner();
                ui.label(RichText::new("Checking achievements…").color(TEXT_DIM()));
            }
            let cfg = self.cfg.read().unwrap();
            if cfg.steam_api_key.trim().is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("Installed Steam games only — add a Steam API key in Settings for your full library")
                            .color(TEXT_DIM()),
                    );
                });
            }
        });
    }

    /// Selection bar shown while several games are selected.
    fn bulk_bar(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        if self.multi.is_empty() {
            return;
        }
        egui::Frame::new().fill(CARD()).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
            theme::raised_widgets(ui);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} selected", self.multi.len())).strong());
                ui.separator();
                for (label, op) in [
                    ("Favorite", Bulk::Fav(true)),
                    ("Unfavorite", Bulk::Fav(false)),
                    ("Hide", Bulk::Hide(true)),
                    ("Unhide", Bulk::Hide(false)),
                    ("Refresh metadata…", Bulk::Refresh),
                ] {
                    if ui.button(label).clicked() {
                        actions.push(Action::Bulk(op));
                    }
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Clear").on_hover_text("Esc").clicked() {
                        self.multi.clear();
                    }
                    if ui.button("Select all").on_hover_text("Ctrl+A").clicked() {
                        self.select_all();
                    }
                    ui.label(RichText::new("Ctrl+click to add, Shift+click for a range").color(TEXT_DIM()).size(12.0));
                });
            });
        });
        ui.add_space(8.0);
    }

    fn select_all(&mut self) {
        self.multi = self.view.iter().map(|&i| self.games[i].id).collect();
    }

    fn view_pos(&self, id: i64) -> Option<usize> {
        self.view.iter().position(|&i| self.games[i].id == id)
    }

    /// Adds the games between the anchor and `id` (view order) to the selection.
    fn select_range(&mut self, id: i64) {
        let to = self.view_pos(id);
        let from = self.anchor.and_then(|a| self.view_pos(a)).or(to);
        if let (Some(a), Some(b)) = (from, to) {
            for &gi in &self.view[a.min(b)..=a.max(b)] {
                self.multi.insert(self.games[gi].id);
            }
        }
    }

    /// Moves the grid cursor; shift extends the selection; keeps the cursor's row in view.
    /// Opens a random game from the current view (not the one already open), with the grid cursor on it.
    fn random_game(&mut self, ctx: &egui::Context) {
        use std::hash::{BuildHasher, Hasher};
        let n = self.view.len();
        if n == 0 {
            return;
        }
        let r = std::collections::hash_map::RandomState::new().build_hasher().finish() as usize;
        let mut i = r % n;
        if n > 1 && Some(self.games[self.view[i]].id) == self.selected {
            i = (i + 1 + r / n % (n - 1)) % n;
        }
        let id = self.games[self.view[i]].id;
        (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, false);
        (self.cursor, self.anchor) = (Some(id), Some(id));
        let cols = self.grid_cols.max(1);
        self.scroll_to = Some((i / cols as usize) as f32 * self.grid_pitch.max(1.0));
        self.selected = Some(id);
        self.opened_at = ctx.input(|i| i.time);
        self.focus_play = true;
    }

    pub(crate) fn nav(&mut self, nav: Nav, shift: bool) {
        let n = self.view.len();
        if n == 0 {
            return;
        }
        let cols = self.grid_cols.max(1) as isize;
        let page = self.grid_page.max(1) as isize * cols;
        let pos = self.cursor.and_then(|c| self.view_pos(c));
        let new = match (pos, nav) {
            (None, _) => 0,
            (Some(_), Nav::Home) => 0,
            (Some(_), Nav::End) => n - 1,
            (Some(p), _) => {
                let d = match nav {
                    Nav::Left => -1,
                    Nav::Right => 1,
                    Nav::Up => -cols,
                    Nav::Down => cols,
                    Nav::PageUp => -page,
                    _ => page,
                };
                (p as isize + d).clamp(0, n as isize - 1) as usize
            }
        };
        let id = self.games[self.view[new]].id;
        self.cursor = Some(id);
        self.kb_focus = true;
        // The list's side panel follows the cursor.
        if self.side() {
            self.selected = Some(id);
        }
        if shift {
            self.select_range(id);
        } else {
            self.anchor = Some(id);
        }
        let pitch = self.grid_pitch.max(1.0);
        let top = (new / cols as usize) as f32 * pitch;
        let (off, h) = self.grid_scroll;
        if top < off {
            self.scroll_to = Some(top);
        } else if top + pitch + GRID_MARGIN > off + h {
            self.scroll_to = Some(top + pitch + GRID_MARGIN - h);
        }
    }

    /// Grid keys: arrows/Home/End/PageUp/PageDown move, Enter opens, Space toggles, Ctrl+A all, Esc clears.
    fn grid_keys(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let (moves, shift, enter, space, all, esc) = ctx.input(|i| {
            let keys = [
                (Key::ArrowLeft, Nav::Left),
                (Key::ArrowRight, Nav::Right),
                (Key::ArrowUp, Nav::Up),
                (Key::ArrowDown, Nav::Down),
                (Key::PageUp, Nav::PageUp),
                (Key::PageDown, Nav::PageDown),
                (Key::Home, Nav::Home),
                (Key::End, Nav::End),
            ];
            let moves: Vec<Nav> = keys.iter().filter(|(k, _)| i.key_pressed(*k)).map(|(_, n)| *n).collect();
            (
                moves,
                i.modifiers.shift,
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Space),
                i.modifiers.command && i.key_pressed(Key::A),
                i.key_pressed(Key::Escape),
            )
        });
        for m in moves {
            self.nav(m, shift);
        }
        if all {
            self.select_all();
        }
        if esc {
            if self.multi.is_empty() && self.side() {
                self.selected = None;
            }
            self.multi.clear();
        }
        if let Some(id) = self.cursor.filter(|_| self.kb_focus) {
            if space && !self.multi.remove(&id) {
                self.multi.insert(id);
            }
            if enter && self.side() {
                actions.push(Action::Play(id));
            } else if enter {
                self.selected = Some(id);
                self.opened_at = ctx.input(|i| i.time);
                self.focus_play = true;
            }
        }
    }

    fn grid(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        self.bulk_bar(ui, actions);
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
        self.grid_cols = cols;
        self.grid_pitch = row_h + gap;
        self.grid_page = ((ui.available_height() / (row_h + gap)).floor() as usize).max(1);

        if self.view.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No games here. Try Refresh, or + Add game.").color(TEXT_DIM()));
            });
            return;
        }

        let running = self.running();
        let (colls, active) = (&self.collections, collections::active(&self.collections, self.collection));
        let games = &self.games;
        let view = &self.view;
        let thumbs = &mut self.thumbs;
        let multi = &self.multi;
        let cursor = self.cursor.filter(|_| self.kb_focus);
        let mut clicks: Vec<(i64, egui::Modifiers, bool)> = Vec::new();
        ui.spacing_mut().item_spacing.y = gap;
        let mut area = ScrollArea::vertical().auto_shrink([false; 2]);
        if let Some(y) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }
        let out = area.show_rows(ui, row_h, rows, |ui, range| {
            // Room for the cursor outline, glow and hover lift above the first row and below the last.
            ui.add_space(GRID_MARGIN);
            let last = range.end == rows;
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
                    let in_multi = multi.contains(&g.id);
                    let is_sel = in_multi || cursor == Some(g.id);
                    let painter = ui.painter();
                    let r = CornerRadius::same(6);

                    // Hover/cursor lift the card and light a glow in the cover's own color, eased in and out.
                    let is_cursor = cursor == Some(g.id);
                    let hot = ui.ctx().animate_bool_with_time_and_easing(resp.id.with("hot"), hovered || is_cursor, 0.18, egui::emath::easing::cubic_out);
                    let cover = cover.translate(vec2(0.0, -4.0 * hot));
                    let tint = thumbs.color(g.id).unwrap_or(ACCENT());
                    let strength = if is_cursor { 0.75 } else { 0.5 } * hot + if in_multi { 0.25 } else { 0.0 };
                    if strength > 0.01 {
                        let glow = egui::epaint::Shadow { offset: [0, 4], blur: 26, spread: 2, color: tint.gamma_multiply(strength.min(1.0)) };
                        painter.add(glow.as_shape(cover, r));
                    }
                    match thumbs.get(g.id, g.has_cover) {
                        Some(t) => {
                            // Covers fade in as they load (or scroll back into view).
                            let a = theme::appear(ui.ctx(), resp.id.with("tex"), 0.22);
                            if a < 1.0 {
                                painter.rect_filled(cover, r, CARD());
                            }
                            egui::Image::from_texture(t).corner_radius(r).tint(Color32::WHITE.gamma_multiply(a)).paint_at(ui, cover);
                        }
                        None => {
                            painter.rect_filled(cover, r, CARD());
                            let lower = Rect::from_min_max(pos2(cover.left(), cover.center().y), cover.max);
                            paint_vgradient(painter, lower, Color32::TRANSPARENT, CARD_HI());
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
                    if in_multi {
                        painter.rect_filled(cover, r, ACCENT().gamma_multiply(0.18));
                    }
                    if g.favorite {
                        let c = cover.right_top() + vec2(-16.0, 16.0);
                        painter.circle_filled(c, 13.0, Color32::from_black_alpha(140));
                        paint_star(painter, c, 9.0, true);
                    }
                    if running.contains(&g.id) {
                        badge(painter, cover.left_top() + vec2(8.0, 8.0), "RUNNING", GOOD());
                    }
                    if in_multi {
                        paint_check(painter, cover.left_bottom() + vec2(16.0, -16.0));
                    }
                    if is_cursor {
                        painter.rect_stroke(cover, r, Stroke::new(3.0, Color32::WHITE.gamma_multiply(0.4 + 0.6 * hot)), StrokeKind::Outside);
                    } else if in_multi {
                        painter.rect_stroke(cover, r, Stroke::new(2.5, ACCENT()), StrokeKind::Outside);
                    } else if hot > 0.01 {
                        painter.rect_stroke(cover, r, Stroke::new(1.5, tint.gamma_multiply(0.9 * hot)), StrokeKind::Outside);
                    }
                    // Platform pill right of the title (Steam, PS5, Switch…).
                    let title_w = {
                        let tag = painter.layout_no_wrap(short_platform(g), FontId::proportional(11.5), TEXT_DIM());
                        let pill = Rect::from_min_size(
                            pos2(x + card_w - tag.size().x - 10.0, cover.bottom() + 6.0),
                            tag.size() + vec2(10.0, 4.0),
                        );
                        painter.rect_filled(pill, CornerRadius::same(4), CARD());
                        painter.galley(pill.min + vec2(5.0, 2.0), tag, TEXT_DIM());
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
                        clicks.push((g.id, ui.input(|i| i.modifiers), false));
                    }
                    if resp.double_clicked() {
                        actions.push(Action::Play(g.id));
                    }
                    if in_multi && multi.len() > 1 {
                        resp.context_menu(|ui| bulk_menu(ui, multi.len(), colls, active, actions));
                    } else {
                        resp.context_menu(|ui| context_menu(ui, g, colls, actions));
                    }
                }
            }
            if last {
                ui.add_space(GRID_MARGIN);
            }
        });
        self.grid_scroll = (out.state.clamped_offset().y, out.inner_rect.height());
        if theme::to_top_button(ui, out.inner_rect, out.state.clamped_offset().y) {
            self.scroll_to = Some(0.0);
        }
        self.apply_clicks(ui, clicks);
    }

    /// Clicks on a grid card or list row: Ctrl toggles, Shift adds a range, a plain click opens the game.
    fn apply_clicks(&mut self, ui: &Ui, clicks: Vec<(i64, egui::Modifiers, bool)>) {
        for (id, m, _) in clicks {
            self.kb_focus = false;
            self.cursor = Some(id);
            if m.command {
                if !self.multi.remove(&id) {
                    self.multi.insert(id);
                }
                self.anchor = Some(id);
            } else if m.shift {
                self.select_range(id);
            } else {
                self.multi.clear();
                self.anchor = Some(id);
                self.selected = Some(id);
                self.opened_at = ui.input(|i| i.time);
            }
        }
    }

    fn details(&mut self, ui: &mut Ui, id: i64, actions: &mut Vec<Action>) {
        let Some(g) = self.game(id).cloned() else { return };
        let colls = self.collections.clone();
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
        // Achievements: from the cache now, refreshed in the background when stale.
        let local = self.workers.local_ach.read().unwrap().contains_key(&id);
        let ach_src = crate::achievements::Source::of(&g, &self.cfg.read().unwrap(), local);
        if ach_src.is_some() && self.ach.as_ref().map(|a| a.id) != Some(id) {
            let a = crate::achievements::GameAch::load(&self.lib, id);
            let due = match ach_src {
                Some(crate::achievements::Source::Retro { ra_id: None }) => true,
                Some(crate::achievements::Source::Retro { ra_id: Some(r) }) => r > 0 && a.stale(),
                _ => a.stale(),
            };
            if due {
                self.workers.achievements(id);
            }
            self.ach = Some(a);
        }
        let steam_ach = ach_src.is_some();
        if self.hltb.as_ref().is_none_or(|h| h.0 != id) {
            let t = self.lib.hltb(id).ok().flatten();
            if t.is_none_or(|t| t.stale(gamut_core::now())) {
                self.workers.hltb(id, g.name.clone());
            }
            self.hltb = Some((id, t));
        }
        let hltb = self.hltb.as_ref().and_then(|h| h.1).filter(|t| t.id > 0);
        for (hid, img) in self.hero_rx.try_iter() {
            if let (Some((cur, slot)), Some(img)) = (self.hero.as_mut(), img) {
                if *cur == hid {
                    *slot = Some(ui.ctx().load_texture(format!("hero{hid}"), img, TextureOptions::LINEAR));
                }
            }
        }

        // One scroll for the whole page, so long descriptions and clips get the full height.
        if self.kb_focus {
            // Controller/keyboard: make the focused button obvious.
            let w = &mut ui.visuals_mut().widgets;
            w.active.bg_stroke = Stroke::new(3.0, Color32::WHITE);
            w.active.weak_bg_fill = ACCENT().gamma_multiply(0.45);
        }
        let mut area = ScrollArea::vertical().id_salt(("details", id)).auto_shrink([false, false]);
        // Freshly opened: start at the top, not where it was last closed.
        if std::mem::take(&mut self.detail_fresh) {
            (self.detail_scroll, self.detail_off) = (None, 0.0);
            self.detail_tab = self.detail_tab_next.take().unwrap_or(0);
            area = area.vertical_scroll_offset(0.0);
        } else if let Some(d) = self.detail_scroll.take() {
            area = area.vertical_scroll_offset((self.detail_off + d).max(0.0));
        }
        let focus_play = std::mem::take(&mut self.focus_play);
        let out = area.show(ui, |ui| {
            let w = ui.available_width();
            // The list's side panel is narrow: smaller cover and margins.
            let narrow = w < 760.0;
            let pad = if narrow { 18.0 } else { 28.0 };
            // Wider than the popup (list mode's details): banner and cover grow with the window.
            let wide = w > 1400.0;
            let banner_h = if wide { (w * 0.28).min(ui.available_height() * 0.5).max(380.0) } else { (w * 0.30).clamp(200.0, 380.0) };
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
                    ui.painter().rect_filled(banner, top_r, CARD());
                }
            }
            // Fade the banner's lower part into the panel so the title stays readable.
            let fade = Rect::from_min_max(pos2(banner.left(), banner.bottom() - banner_h * 0.6), banner.right_bottom());
            paint_vgradient(ui.painter(), fade, Color32::TRANSPARENT, theme::PANEL());
            close_button(ui, banner.right_top() + vec2(-24.0, 24.0), &mut self.selected);
            if let Some(c) = collections::game_chips(ui, banner.left_top() + vec2(pad, 18.0), w - pad - 80.0, &colls, &g) {
                self.open_collection(c);
            }

            // Cover overlaps the banner's bottom edge.
            let cover_w = if narrow { 120.0 } else if wide { (banner_h * 0.55).clamp(190.0, 320.0) } else { 190.0 };
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
                    ui.painter().rect_filled(cover, CornerRadius::same(8), CARD());
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
                                .fill(GOOD())
                                .min_size(vec2(150.0, 38.0));
                            if ui.add(b).on_hover_text("This game is running. Click to close it.").clicked() {
                                actions.push(Action::Stop(id));
                            }
                        } else {
                            let label = if g.needs_install() { "Install" } else { "Play" };
                            let play = egui::Button::new(RichText::new(label).size(17.0).strong().color(Color32::WHITE))
                                .fill(ACCENT())
                                .min_size(vec2(150.0, 38.0));
                            let can_play = g.source == Source::Steam || g.exe.is_some();
                            let r = ui.add_enabled(can_play, play).on_disabled_hover_text("No way to start this game on this PC");
                            if focus_play && can_play {
                                r.request_focus();
                            }
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
                        ui.menu_button("More", |ui| context_menu(ui, &g, &colls, actions))
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
                        stat(ui, "ADDED", &if g.added > 0 { util::date(g.added) } else { "Unknown".into() });
                        stat(ui, "STATUS", status);
                        if let Some(c) = &g.completion {
                            stat(ui, "COMPLETION", c);
                        }
                        stat(ui, "PLAY COUNT", &g.play_count.to_string());
                        if let Some(t) = self.ach.as_ref().filter(|a| steam_ach && a.id == id).and_then(crate::achievements::tile) {
                            stat(ui, "ACHIEVEMENTS", &t);
                        }
                    });
                    ui.add_space(12.0);
                    ui.horizontal_wrapped(|ui| {
                        let own = g.links().into_iter().map(|l| (l.name, l.url));
                        let auto = links(&g).into_iter().map(|(n, u)| (n.to_string(), u));
                        let hltb_url = hltb.and_then(|t| t.url());
                        for (label, url) in auto.map(|(n, u)| match (&hltb_url, n.as_str()) {
                            (Some(h), "HowLongToBeat") => (n, h.clone()),
                            _ => (n, u),
                        }).chain(own) {
                            let b = egui::Button::new(RichText::new(label).size(12.5)).corner_radius(CornerRadius::same(12));
                            // Imported links could be any path or protocol: only web pages open.
                            let web = ["https://", "http://"].iter().any(|p| url.get(..p.len()).is_some_and(|u| u.eq_ignore_ascii_case(p)));
                            if ui.add_enabled(web, b).on_hover_text(&url).on_disabled_hover_text(format!("Not a web link: {url}")).clicked() {
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

            // About / Achievements / Sessions tabs, so they don't sit below a long description.
            let show_ach = ach_src.is_some() && self.detail_tab == 1;
            let show_sessions = self.detail_tab == 2;
            let show_shots = self.detail_tab == 3;
            if self.sessions.as_ref().is_none_or(|s| s.id != id) {
                self.sessions = Some(crate::sessions_view::GameSessions::load(&self.lib, id));
            }
            {
                let count = self.ach.as_ref().filter(|a| a.id == id).and_then(crate::achievements::tile);
                let n = self.sessions.as_ref().map_or(0, |s| s.count());
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    let mut tabs = vec![(0, "About".to_string())];
                    if ach_src.is_some() {
                        tabs.push((1, count.map_or("Achievements".into(), |c| format!("Achievements  {c}"))));
                    }
                    tabs.push((2, if n > 0 { format!("Sessions  {n}") } else { "Sessions".into() }));
                    let shots = self.shots.count(id);
                    tabs.push((3, if shots > 0 { format!("Screenshots  {shots}") } else { "Screenshots".into() }));
                    for (i, label) in tabs {
                        let on = self.detail_tab == i || (i == 0 && self.detail_tab == 1 && ach_src.is_none());
                        let text = RichText::new(label).size(15.0).color(if on { theme::TEXT() } else { TEXT_DIM() });
                        if ui.add(egui::Button::selectable(on, text).min_size(vec2(0.0, 30.0))).clicked() {
                            self.detail_tab = i;
                        }
                    }
                });
                ui.add_space(14.0);
            }

            // Lower area: details on the left, description (or achievements) on the right; the whole page scrolls.
            ui.horizontal_top(|ui| {
                ui.add_space(pad);
                let left_w = cover_w + 40.0;
                ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                    ui.set_width(left_w);
                    ui.vertical(|ui| {
                        let source = match (g.source, &g.store) {
                            (Source::Steam, _) => "Steam",
                            (_, Some(s)) => s.as_str(),
                            _ => "Custom",
                        };
                        let released = g.release_date.as_deref().map(util::release);
                        card(ui, "Details", |ui| {
                            fact(ui, "Source", source);
                            for (k, v) in [
                                ("Platform", &g.platform),
                                ("Developer", &g.developer),
                                ("Publisher", &g.publisher),
                                ("Released", &released),
                                ("Genres", &g.genres),
                                ("Series", &g.series),
                            ] {
                                if let Some(v) = v {
                                    fact(ui, k, v);
                                }
                            }
                        });
                        let scores: Vec<(&str, i64)> =
                            [("Critics", g.critic_score), ("Users", g.community_score), ("You", g.user_score)].into_iter().filter_map(|(k, v)| Some((k, v?))).collect();
                        if !scores.is_empty() {
                            card(ui, "Scores", |ui| {
                                for (k, v) in scores {
                                    fact(ui, k, &v.to_string());
                                }
                            });
                        }
                        if let Some(t) = hltb {
                            let h = gamut_core::hltb::hours;
                            card(ui, "HowLongToBeat", |ui| {
                                for (k, v) in [("Main story", t.main), ("Main + extras", t.plus), ("Completionist", t.full)] {
                                    fact(ui, k, &h(v));
                                }
                            });
                        }
                        let location = g.install_dir.as_deref().filter(|d| !d.contains('{')).or(g.exe.as_deref());
                        let display = g.display_mode();
                        if g.install_size.is_some() || g.steam_appid().is_some() || location.is_some() || display.is_some() {
                            card(ui, "Install", |ui| {
                                if let Some(m) = &display {
                                    fact(ui, "Display", &m.describe());
                                }
                                if let Some(b) = g.install_size {
                                    fact(ui, "Size", &gamut_core::format_size(b));
                                }
                                if let Some(a) = g.steam_appid() {
                                    fact(ui, "Steam ID", &a.to_string());
                                }
                                if let Some(d) = location {
                                    ui.add_space(2.0);
                                    ui.label(RichText::new("Location").color(TEXT_DIM()));
                                    ui.add(egui::Label::new(RichText::new(d).size(12.5)).truncate()); // egui shows the full text on hover when it's cut.
                                }
                            });
                        }
                        if g.tags.is_some() {
                            ui.add_space(4.0);
                            ui.label(theme::caption("Tags"));
                            ui.add_space(2.0);
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                                for t in split_list(&g.tags) {
                                    let b = egui::Button::new(RichText::new(t).size(12.5))
                                        .fill(CARD())
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
                    if let (true, Some(sv)) = (show_sessions, self.sessions.as_mut()) {
                        crate::sessions_view::section(ui, sv);
                    }
                    if show_shots {
                        crate::shots_view::game_tab(ui, &mut self.shots, &g);
                    }
                    if let (false, false, false, Some(n)) = (show_ach, show_sessions, show_shots, g.notes.as_deref()) {
                        theme::section(ui, "Notes");
                        ui.add(egui::Label::new(n).wrap());
                        ui.add_space(14.0);
                    }
                    if !show_ach && !show_sessions && !show_shots {
                        theme::section(ui, "Description");
                        ui.vertical(|ui| {
                            match &g.description {
                                Some(d) => {
                                    description(ui, d);
                                    ui.response()
                                }
                                None if g.meta_state == state::PENDING => {
                                    ui.label(RichText::new("Fetching metadata…").color(TEXT_DIM()))
                                }
                                None => ui.label(RichText::new("No description.").color(TEXT_DIM())),
                            };
                        });
                        if crate::shots_view::strip(ui, &mut self.shots, &g) {
                            self.detail_tab = 3;
                        }
                    }
                    if let (Some(a), Some(src), true) = (self.ach.as_mut().filter(|a| a.id == id), ach_src, show_ach) {
                        let busy = self.workers.ach_busy.lock().unwrap().contains(&id);
                        if let Some(crate::achievements::Ask::PickRetroGame) = crate::achievements::section(ui, a, src, busy) {
                            let cfg = self.cfg.read().unwrap().clone();
                            self.ra_pick = Some(crate::achievements::RetroPick::open(&g, &cfg, self.paths.root.join("ra"), ui.ctx()));
                        }
                    }
                });
            });
            ui.add_space(28.0);
        });
        self.detail_off = out.state.clamped_offset().y;
        if theme::to_top_button(ui, out.inner_rect, self.detail_off) {
            self.detail_scroll = Some(-self.detail_off);
        }
        if let Some(t) = pick_tag {
            self.tag = t;
            self.selected = None;
        }
    }

    // ---------- dialogs ----------

    fn editor_window(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.editor.as_mut() else { return };
        let cfg = self.cfg.read().unwrap().clone();
        let igdb = gamut_core::igdb::configured(&cfg).then_some(&cfg);
        let lists = Lists { platforms: &self.platforms, tags: &self.tags, genres: &self.ed_lists.0, completions: &self.ed_lists.1, igdb };
        match ed.show(ctx, &lists) {
            EditorOutcome::Open => {}
            EditorOutcome::Cancel => self.editor = None,
            EditorOutcome::Save => {
                if let Err(e) = self.save_editor() {
                    self.toasts.error(format!("Saving failed: {e:#}"));
                }
                self.editor = None;
                if let Some(h) = self.health.as_mut() {
                    h.stale = true;
                }
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
        if let Some((mins, count)) = ed.play_stats() {
            self.lib.set_play_stats(id, mins, count)?;
        }
        self.toasts.success(format!("Saved {}.", g.name));
        // New tags/platforms should show up in the filters right away.
        self.reload_one(id);
        (self.platforms, self.tags) = facets(&self.games);
        self.completions = completion_facets(&self.games);
        match &ed.icon_file {
            Some(Some(p)) => {
                std::fs::copy(p, self.paths.icon(id))?;
            }
            Some(None) => {
                let _ = std::fs::remove_file(self.paths.icon(id));
            }
            None => {}
        }
        if let Some(p) = ed.cover_file {
            let bytes = std::fs::read(p)?;
            images::store_cover(&bytes, &self.paths.cover(id), &self.paths.thumb(id))?;
            self.lib.set_cover_state(id, true, None, state::DONE)?;
        }
        // A hand-picked IGDB entry also supplies a missing cover (the cover worker fetches it).
        if let Some(ig) = ed.igdb_pick {
            self.lib.set_igdb_id(id, ig)?;
            if let Some(g) = self.lib.get(id)?.filter(|g| !g.has_cover) {
                self.lib.set_cover_state(id, false, None, state::PENDING)?;
                log::info!("IGDB pick for {}: fetching its cover", g.name);
            }
        }
        self.reload_one(id);
        self.workers.kick();
        // A new platform or name may mean another RetroAchievements game.
        self.ach = None;
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
        let mut pn_extras = false;
        let importing = self.workers.importing.1.load(Ordering::Relaxed) > 0;
        let detected = gamut_core::steam::steam_path()
            .and_then(|p| gamut_core::steam::detect_steam_id(&p))
            .unwrap_or_default();
        let mut tab = self.settings_tab;
        let mut restore = None;
        let mut review_req = None;
        self.key_checks.update(s, ctx);
        let checks = &self.key_checks;
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
                            checks.row(ui, crate::keycheck::STEAM);
                        });
                        ui.hyperlink_to("Get a Steam Web API key", "https://steamcommunity.com/dev/apikey");
                        ui.label(theme::hint("Your profile's game details must be public, or the key must belong to this account."));
                        let mut sync = !s.no_steam_collections;
                        if ui.checkbox(&mut sync, "Sync collections from Steam").changed() {
                            s.no_steam_collections = !sync;
                        }
                        if !s.steam_collections_ignored.is_empty() {
                            ui.horizontal(|ui| {
                                ui.label(theme::hint(&format!("{} deleted in Gamut, not synced.", s.steam_collections_ignored.len())));
                                if ui.button("Sync them again").clicked() {
                                    s.steam_collections_ignored.clear();
                                }
                            });
                        }
                        ui.label(theme::hint("Steam's Favorites (as \"To play\") and your own Steam collections, updated on every scan. Steam decides which Steam games are in them; games from elsewhere you add in Gamut stay."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "Folder scanners");
                        let mut auto = !s.no_auto_scan;
                        if ui.checkbox(&mut auto, "Scan folders when Gamut starts").changed() {
                            s.no_auto_scan = !auto;
                        }
                        ui.label(theme::hint(
                            "After the Steam and store sync, each scanner (top bar → Scan folders) adds new games on its own. \
                             Turn it off per scanner there; games you left unticked stay out.",
                        ));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "SteamGridDB");
                        egui::Grid::new("sg").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            ui.label("API key");
                            ui.add(egui::TextEdit::singleline(&mut s.steamgriddb_key).password(true).desired_width(420.0));
                            ui.end_row();
                            checks.row(ui, crate::keycheck::SGDB);
                        });
                        ui.hyperlink_to("Get a SteamGridDB key", "https://www.steamgriddb.com/profile/preferences/api");
                        ui.label(theme::hint("Used for covers and banners Steam doesn't have."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "RetroAchievements");
                        egui::Grid::new("ra").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            ui.label("Username");
                            ui.add(egui::TextEdit::singleline(&mut s.ra_user).desired_width(420.0));
                            ui.end_row();
                            ui.label("Web API key");
                            ui.add(egui::TextEdit::singleline(&mut s.ra_api_key).password(true).desired_width(420.0));
                            ui.end_row();
                            checks.row(ui, crate::keycheck::RETRO);
                        });
                        ui.hyperlink_to("Get your RetroAchievements key", "https://retroachievements.org/settings");
                        ui.label(theme::hint("Achievements for emulated games, matched by platform and title."));
                        ui.add_space(8.0);
                        let mut popup = !s.no_ach_popup;
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut popup, "Pop up new achievements in PC games with emulator achievement files").changed() {
                                s.no_ach_popup = !popup;
                            }
                            if ui.button("Preview").clicked() {
                                crate::popup::show(crate::popup::Popup {
                                    caption: "Achievement unlocked · 21 / 56".into(),
                                    title: "Wise Researcher".into(),
                                    text: "Complete the game with puzzle difficulty set to \"Hard\".".into(),
                                    icon: Some((vec![232, 184, 74, 255].repeat(64 * 64), 64, 64)),
                                });
                            }
                        });
                        ui.label(theme::hint("GSE/Goldberg, CODEX, RUNE or OnlineFix files. Shows over windowed and borderless games."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "IGDB");
                        egui::Grid::new("igdb").num_columns(2).spacing([10.0, 8.0]).show(ui, |ui| {
                            ui.label("Client ID");
                            ui.add(egui::TextEdit::singleline(&mut s.igdb_client_id).desired_width(420.0));
                            ui.end_row();
                            ui.label("Client secret");
                            ui.add(egui::TextEdit::singleline(&mut s.igdb_client_secret).password(true).desired_width(420.0));
                            ui.end_row();
                            checks.row(ui, crate::keycheck::IGDB);
                        });
                        ui.hyperlink_to("Register a Twitch app for IGDB", "https://dev.twitch.tv/console/apps");
                        ui.label(theme::hint(
                            "Metadata and covers for games not on Steam. Any app name, redirect URL http://localhost,                              category Application Integration; needs a Twitch account with two-factor sign-in.",
                        ));
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
                            if ui
                                .add_enabled(!importing, egui::Button::new("Links, notes and more"))
                                .on_hover_text("Brings over links, notes, your score, sorting names, other actions, scripts, install sizes and icons, for games that don't have them in Gamut yet.")
                                .clicked()
                            {
                                pn_extras = true;
                            }
                        });
                        }
                        2 => {
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
                        3 => {
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
                        1 => {
                            if let Some(st) = stores::tab(ui, s, &self.lib, self.workers.store_busy.load(Ordering::Relaxed)) {
                                review_req = Some(st);
                            }
                        }
                        4 => backups::tab(ui, &mut s.backup, &self.paths, &self.workers, &mut self.backup_list, &mut restore),
                        _ => {
                        theme::section(ui, "Window");
                        let mut tray = !s.quit_on_close;
                        ui.checkbox(&mut tray, "Closing the window keeps Gamut in the tray");
                        s.quit_on_close = !tray;
                        ui.checkbox(&mut s.start_with_windows, "Start with Windows (in the tray)");
                        let mut key = !s.no_screenshot_key;
                        if ui.checkbox(&mut key, "F12 takes screenshots of non-Steam games (Steam games use Steam's key)").changed() {
                            s.no_screenshot_key = !key;
                        }
                        let mut sound = !s.no_screenshot_sound;
                        if ui.add_enabled(key, egui::Checkbox::new(&mut sound, "Play a shutter sound when a screenshot is taken")).changed() {
                            s.no_screenshot_sound = !sound;
                        }
                        ui.label(theme::hint("The tray icon opens Gamut, big screen, or one of your recently played games."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "Appearance");
                        let tf = theme::file(&self.paths.root);
                        ui.horizontal(|ui| {
                            let label = if tf.exists() { "Edit theme" } else { "Customize theme" };
                            if ui.button(label).on_hover_text("Colors, corner radius, spacing, text size and fonts in theme.toml").clicked() {
                                let r = if tf.exists() { Ok(()) } else { std::fs::write(&tf, theme::template()) };
                                if let Err(e) = r.and_then(|_| open::that_detached(&tf)) {
                                    log::warn!("theme file: {e}");
                                }
                            }
                            if tf.exists() && ui.button("Built-in theme").on_hover_text("Renames theme.toml to theme.toml.bak").clicked() {
                                let _ = std::fs::rename(&tf, tf.with_extension("toml.bak"));
                            }
                        });
                        ui.label(theme::hint("Edit theme.toml in any text editor; saved changes show right away."));
                        ui.add_space(12.0);
                        ui.separator();
                        theme::section(ui, "Files");
                        ui.label(theme::hint(&format!("Data folder: {}", self.paths.root.display())));
                        ui.horizontal(|ui| {
                            if ui.button("Open log").on_hover_text("What Gamut did this run, including errors").clicked() {
                                let _ = open::that(crate::logging::path(&self.paths.root));
                            }
                            if ui.button("Open data folder").clicked() {
                                let _ = open::that(&self.paths.root);
                            }
                        });
                        ui.add_space(12.0);
                        ui.separator();
                        let backups = gamut_core::backup::dir(&self.paths, &s.backup);
                        if storage::section(ui, &mut self.storage, &self.paths, backups) {
                            self.thumbs.clear();
                            self.small_thumbs.clear();
                        }
                        }
                    }
                });
                ui.separator();
                ui.add_space(8.0);
                if ui.add(egui::Button::new(RichText::new("Save").color(Color32::WHITE)).fill(ACCENT())).clicked() {
                    save = true;
                }
            });
        self.settings_tab = tab;
        if restore.is_some() {
            self.confirm_restore = restore;
        }
        if let Some(st) = review_req {
            self.workers.store_review(st);
        }
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
        if pn_extras {
            self.workers.playnite_extras();
        }
        if save {
            let new = self.settings.take().unwrap();
            let old = self.cfg.read().unwrap().clone();
            *self.cfg.write().unwrap() = new.clone();
            if let Err(e) = new.save(&self.paths.config) {
                self.toasts.error(format!("Saving settings failed: {e:#}"));
            }
            if new.start_with_windows != old.start_with_windows {
                if let Err(e) = crate::tray::set_autostart(new.start_with_windows) {
                    self.toasts.error(format!("Couldn't change Start with Windows: {e:#}"));
                }
            }
            if new.steamgriddb_key != old.steamgriddb_key {
                let _ = self.lib.retry_missing_covers();
            }
            if (&new.igdb_client_id, &new.igdb_client_secret) != (&old.igdb_client_id, &old.igdb_client_secret) {
                self.workers.igdb_requeue();
            }
            self.workers.rescan();
        } else if !open {
            self.settings = None;
        }
        if self.settings.is_none() {
            // Next open checks again.
            self.key_checks.reset();
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
                        match self.remove_games(&[id]) {
                            Ok(()) => self.toasts.success(format!("Removed {name}.")),
                            Err(e) => self.toasts.error(format!("{e:#}")),
                        }
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

    /// Deletes games from the library (not their files) with their cached art.
    fn remove_games(&mut self, ids: &[i64]) -> anyhow::Result<()> {
        let r = ids.iter().try_for_each(|&id| {
            self.lib.delete(id)?;
            for p in [self.paths.cover(id), self.paths.thumb(id), self.paths.hero(id), self.paths.icon(id)] {
                let _ = std::fs::remove_file(p);
            }
            if self.selected == Some(id) {
                self.selected = None;
            }
            anyhow::Ok(())
        });
        self.reload_all();
        r
    }

    /// Confirms "Refresh metadata" for the selected games (it replaces their fetched fields).
    fn refresh_window(&mut self, ctx: &egui::Context) {
        let Some(ids) = self.confirm_refresh.clone() else { return };
        let (mut close, mut open) = (false, true);
        theme::dialog(ctx, "Refresh metadata", false, |ui| {
            ui.set_max_width(480.0);
            dialog_header(ui, "Refresh metadata", &mut open);
            ui.label(format!(
                "Fetch covers and metadata again for {} games? Their description, developer, publisher, \
                 release date and genres are replaced, including edits you made to those.",
                ids.len()
            ));
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() {
                    let r = ids.iter().try_for_each(|&id| self.lib.requeue(id, true));
                    match r {
                        Ok(()) => self.toasts.success(format!("Refreshing {} games.", ids.len())),
                        Err(e) => self.toasts.error(format!("{e:#}")),
                    }
                    self.hero = None;
                    self.reload_all();
                    self.workers.kick();
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || !open {
            self.confirm_refresh = None;
        }
    }

    /// Library health; Edit/Remove open the usual dialogs on top.
    fn health_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        use crate::health_ui::HealthOut;
        let Some(d) = self.health.as_mut() else { return };
        if d.stale && self.editor.is_none() && self.confirm_remove.is_none() {
            d.run(&self.games);
        }
        match d.show(ctx, &self.games, &self.platforms) {
            Some(HealthOut::Edit(id)) => actions.push(Action::Edit(id)),
            Some(HealthOut::Remove(id)) => actions.push(Action::Remove(id)),
            Some(HealthOut::KeepOnly { keep, remove, move_playtime }) => {
                let r = (|| {
                    if move_playtime {
                        let all: Vec<&Game> = self.games.iter().filter(|g| g.id == keep || remove.contains(&g.id)).collect();
                        let mins = all.iter().map(|g| g.playtime_min).sum();
                        let count = all.iter().map(|g| g.play_count).sum();
                        self.lib.set_play_stats(keep, mins, count)?;
                    }
                    self.remove_games(&remove)
                })();
                match r {
                    Ok(()) => self.toasts.success(format!("Removed {} duplicate copies.", remove.len())),
                    Err(e) => self.toasts.error(format!("Removing duplicates failed: {e:#}")),
                }
            }
            Some(HealthOut::Save) => {
                let s = d.settings.clone();
                d.run(&self.games);
                let mut c = self.cfg.write().unwrap();
                c.health = s;
                if let Err(e) = c.save(&self.paths.config) {
                    self.toasts.error(format!("Saving health settings failed: {e:#}"));
                }
            }
            Some(HealthOut::Close) => self.health = None,
            None => {}
        }
    }

    fn scan_window(&mut self, ctx: &egui::Context) {
        let Some(d) = self.scans.as_mut() else { return };
        // Exes and install folders already in the library (Steam/GOG games have only the folder).
        let games = &self.games;
        let known = || -> HashSet<String> {
            games
                .iter()
                .flat_map(|g| [g.exe.as_deref(), g.install_dir.as_deref()])
                .flatten()
                .map(|p| gamut_core::scanner::key(std::path::Path::new(p)))
                .collect()
        };
        let out = d.show(ctx, known, &self.platforms, &self.tags);
        let out = match out {
            crate::scan_ui::ScanOut::ApplyTags(s) => {
                let nested = gamut_core::scanner::nested_dirs(&s.dir, &d.scanners);
                self.apply_scanner_tags(&s, &nested);
                return;
            }
            crate::scan_ui::ScanOut::Open => return,
            o => o,
        };
        let scanners = self.scans.take().map(|d| d.scanners).unwrap_or_default();
        if self.cfg.read().unwrap().scanners != scanners {
            let mut c = self.cfg.write().unwrap();
            c.scanners = scanners;
            if let Err(e) = c.save(&self.paths.config) {
                self.toasts.error(format!("Saving scanners failed: {e:#}"));
            }
        }
        if let crate::scan_ui::ScanOut::Import(s, picked) = out {
            match self.import_scanned(&s, &picked) {
                Ok(n) => {
                    // Show them: newest first, nothing filtered out.
                    (self.sort, self.filter, self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (Sort::Added, Filter::All, None, None, false, false);
                    self.collection = None;
                    (self.platform, self.tag, self.completion) = (String::new(), String::new(), String::new());
                    self.search.clear();
                    self.scroll_to = Some(0.0);
                    self.toasts.success(format!("Added {n} games, shown first (sorted by recently added). Fetching their details and covers…"));
                }
                Err(e) => self.toasts.error(format!("Adding games failed: {e:#}")),
            }
        }
    }

    /// Gives games under a scanner's folder (not under `nested` scanners' folders) its tags and tag rules,
    /// matched against the exe's folders inside the scanned folder.
    fn apply_scanner_tags(&mut self, s: &gamut_core::scanner::Scanner, nested: &[String]) {
        use gamut_core::scanner::key;
        let base = format!("{}\\", key(std::path::Path::new(s.dir.trim())).trim_end_matches('\\'));
        let add: Vec<(i64, String)> = self
            .games
            .iter()
            .filter_map(|g| {
                let exe = key(std::path::Path::new(g.exe.as_deref()?));
                let rel = exe.strip_prefix(&base)?;
                if nested.iter().any(|n| exe.starts_with(&format!("{n}\\"))) {
                    return None;
                }
                let dir = rel.rsplit_once('\\').map_or("", |(d, _)| d);
                let tags = s.tags_for(dir);
                (!tags.is_empty()).then(|| (g.id, tags))
            })
            .collect();
        match self.lib.add_tags(&add) {
            Ok(n) => {
                log::info!("scanner \"{}\": tagged {n} of {} games", s.name, add.len());
                self.reload_all();
                (self.platforms, self.tags) = facets(&self.games);
                self.toasts.success(format!("Tagged {n} games from {}.", s.dir.trim()));
            }
            Err(e) => self.toasts.error(format!("Tagging failed: {e:#}")),
        }
    }

    /// Adds scanned games as custom entries (exe, its folder as working dir, the scanner's platform/tags).
    fn import_scanned(&mut self, s: &gamut_core::scanner::Scanner, picked: &[(String, std::path::PathBuf, String)]) -> anyhow::Result<usize> {
        let rows: Vec<_> = picked
            .iter()
            .map(|(n, e, t)| (n.clone(), e.display().to_string(), e.parent().map(|p| p.display().to_string()), t.clone()))
            .collect();
        // One transaction: quick even for hundreds of games.
        self.lib.add_scanned(&rows, &s.platform)?;
        log::info!("scanner \"{}\": added {} games from {}", s.name, picked.len(), s.dir);
        self.reload_all();
        (self.platforms, self.tags) = facets(&self.games);
        self.workers.kick();
        Ok(picked.len())
    }

    /// Controller (Start) version of the right-click menu.
    fn game_menu_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(id) = self.game_menu else { return };
        let Some(g) = self.game(id).cloned() else {
            self.game_menu = None;
            return;
        };
        let bulk = self.multi.len() > 1 && self.multi.contains(&id) && self.selected.is_none();
        let before = actions.len();
        let mut open = true;
        let resp = theme::dialog(ctx, "game_menu", self.selected.is_some(), |ui| {
            ui.set_width(300.0);
            dialog_header(ui, if bulk { "Selected games" } else { &g.name }, &mut open);
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                ui.spacing_mut().button_padding = vec2(14.0, 9.0);
                if bulk {
                    bulk_menu(ui, self.multi.len(), &self.collections, collections::active(&self.collections, self.collection), actions);
                } else {
                    context_menu(ui, &g, &self.collections, actions);
                }
            });
        });
        if !open || actions.len() > before || resp.should_close() {
            self.game_menu = None;
        }
    }

    /// Steam can't run two games: starting one closes (or asks to close) the other.
    fn play_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some((id, other)) = self.confirm_play else { return };
        let (Some(g), Some(o)) = (self.game(id).cloned(), self.game(other).cloned()) else {
            self.confirm_play = None;
            return;
        };
        let (mut close, mut open) = (false, true);
        let resp = theme::dialog(ctx, "confirm_play", self.selected.is_some(), |ui| {
            ui.set_max_width(480.0);
            dialog_header(ui, "Another game is running", &mut open);
            ui.label(format!("\"{}\" is running. Steam runs one game at a time, so starting \"{}\" may close it.", o.name, g.name));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                // Cancel first: it gets the controller focus, so a double press doesn't close the game.
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button(format!("Start {}", g.name)).clicked() {
                    actions.push(Action::PlayAnyway(id));
                    close = true;
                }
            });
        });
        if close || !open || resp.should_close() {
            self.confirm_play = None;
        }
    }

    /// Special K + ReShade in a Vulkan game: pick per game, then launch.
    fn hooks_window(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(id) = self.confirm_hooks else { return };
        let Some(g) = self.game(id).cloned() else {
            self.confirm_hooks = None;
            return;
        };
        let (mut close, mut open, mut pick) = (false, true, None);
        let resp = theme::dialog(ctx, "confirm_hooks", self.selected.is_some(), |ui| {
            ui.set_max_width(500.0);
            dialog_header(ui, "Special K and ReShade", &mut open);
            ui.label(format!(
                "\"{}\" is a Vulkan game. Special K and ReShade together can crash Vulkan games. \
                 Your choice is kept for this game (Edit → Launch changes it).",
                g.name
            ));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if ui.button("Special K only").clicked() {
                    pick = Some((true, false));
                }
                if ui.button("ReShade only").clicked() {
                    pick = Some((false, true));
                }
                if ui.button("Keep both").clicked() {
                    pick = Some((true, true));
                }
            });
        });
        if let Some((sk, rs)) = pick {
            let r = self.lib.set_hooks(id, Some(sk), Some(rs)).and_then(|_| {
                // ReShade's Vulkan layer stays active while its files are there.
                if !rs {
                    gamut_core::reshade::uninstall(&g)?;
                }
                Ok(())
            });
            match r {
                Ok(()) => {
                    if let Some(x) = self.games.iter_mut().find(|x| x.id == id) {
                        (x.specialk, x.reshade) = (Some(sk), Some(rs));
                    }
                    actions.push(Action::PlayAnyway(id));
                }
                Err(e) => self.toasts.error(format!("Saving the choice failed: {e:#}")),
            }
            close = true;
        }
        if close || !open || resp.should_close() {
            self.confirm_hooks = None;
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
                    .add(egui::Button::new(RichText::new("Force close").color(Color32::WHITE)).fill(theme::DANGER()))
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
                        .color(theme::DANGER()),
                    );
                    ui.add_space(8.0);
                }
                ui.horizontal(|ui| {
                    if sure {
                        if ui.add(egui::Button::new(RichText::new("Yes, overwrite").color(Color32::WHITE)).fill(theme::DANGER())).clicked() {
                            go = true;
                        }
                        if ui.button("Back").clicked() {
                            back = true;
                        }
                    } else {
                        if ui.add(egui::Button::new(RichText::new("Import").color(Color32::WHITE)).fill(ACCENT())).clicked() {
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
        // Big screen's full screen and zoom aren't the normal placement.
        if minimized || self.big.is_some() {
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
        // A collection's own sort isn't the library's.
        c.sort = key_of(&SORTS, self.sort_before.unwrap_or(self.sort)).into();
        c.filter = key_of(&FILTERS, self.filter).into();
        c.platform = self.platform.clone();
        c.tag = self.tag.clone();
        c.completion = self.completion.clone();
        c.collection = self.collection;
        c.window = self.window;
        let _ = c.save_ui_prefs(&self.paths.config);
    }
}

fn context_menu(ui: &mut Ui, g: &Game, colls: &[Collection], actions: &mut Vec<Action>) {
    let id = g.id;
    let item = |ui: &mut Ui, actions: &mut Vec<Action>, label: &str, a: Action| {
        if ui.button(label).clicked() {
            actions.push(a);
            ui.close();
        }
    };
    item(ui, actions, if g.needs_install() { "Install" } else { "Play" }, Action::Play(id));
    for (i, a) in g.actions().iter().enumerate() {
        item(ui, actions, &a.name, Action::RunAction(id, i));
    }
    item(ui, actions, "Edit…", Action::Edit(id));
    item(ui, actions, if g.favorite { "Unfavorite" } else { "Favorite" }, Action::ToggleFav(id));
    item(ui, actions, if g.hidden { "Unhide" } else { "Hide" }, Action::ToggleHidden(id));
    collections::collect_menu(ui, colls, Some(g), actions);
    item(ui, actions, "Change cover…", Action::PickArt(id, ArtKind::Cover));
    item(ui, actions, "Change banner…", Action::PickArt(id, ArtKind::Banner));
    item(ui, actions, "Refresh metadata", Action::Refresh(id));
    ui.separator();
    if g.folder().is_some() {
        item(ui, actions, "Open install folder", Action::OpenFolder(id));
    }
    item(ui, actions, "Open save folder", Action::OpenSaves(id));
    let (un, rm) = (uninstall::offered(g), g.source == Source::Custom);
    if un || rm {
        ui.separator();
    }
    if un {
        item(ui, actions, "Uninstall…", Action::Uninstall(id));
    }
    if rm {
        item(ui, actions, "Remove…", Action::Remove(id));
    }
}

/// Big screen footer: what the controller buttons do.
fn big_hints(ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (k, v) in [("A", "Open / Play"), ("B", "Back"), ("X", "Favorite"), ("Y", "Search"), ("Start", "Menu"), ("LB/RB", "Filter"), ("LT/RT", "Page"), ("View / PS", "Exit big screen")] {
            egui::Frame::new()
                .fill(CARD_HI())
                .corner_radius(CornerRadius::same(4))
                .inner_margin(egui::Margin::symmetric(7, 2))
                .show(ui, |ui| ui.label(RichText::new(k).strong().size(14.0)));
            ui.label(RichText::new(v).color(TEXT_DIM()).size(14.0));
            ui.add_space(14.0);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Keyboard: arrows, Enter, Esc, F11").color(TEXT_DIM()).size(13.0));
        });
    });
}

/// Right-click menu on a multi-selection.
fn bulk_menu(ui: &mut Ui, n: usize, colls: &[Collection], active: Option<&Collection>, actions: &mut Vec<Action>) {
    ui.label(RichText::new(format!("{n} selected")).color(TEXT_DIM()));
    ui.separator();
    for (label, op) in [
        ("Favorite", Bulk::Fav(true)),
        ("Unfavorite", Bulk::Fav(false)),
        ("Hide", Bulk::Hide(true)),
        ("Unhide", Bulk::Hide(false)),
        ("Refresh metadata…", Bulk::Refresh),
    ] {
        if ui.button(label).clicked() {
            actions.push(Action::Bulk(op));
            ui.close();
        }
    }
    collections::collect_menu(ui, colls, None, actions);
    if let Some(c) = active {
        if ui.button(format!("Remove from {}", c.name)).clicked() {
            actions.push(Action::Collect(c.id, None, false));
            ui.close();
        }
    }
}

/// Round accent badge with a check mark (multi-selected card).
fn paint_check(p: &egui::Painter, c: egui::Pos2) {
    p.circle_filled(c, 11.0, ACCENT());
    p.circle_stroke(c, 11.0, Stroke::new(1.5, Color32::WHITE));
    let s = Stroke::new(2.2, Color32::WHITE);
    p.line_segment([c + vec2(-5.0, 0.0), c + vec2(-1.5, 3.5)], s);
    p.line_segment([c + vec2(-1.5, 3.5), c + vec2(5.0, -3.5)], s);
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
        p.add(egui::Shape::closed_line(pts, Stroke::new(1.5, TEXT_DIM())));
    }
}

/// Small translucent button centred at `c`; true when clicked.
fn art_pill(ui: &mut Ui, c: egui::Pos2, text: &str, salt: &str) -> bool {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(13.0), Color32::WHITE);
    let rect = Rect::from_center_size(c, galley.size() + vec2(24.0, 14.0));
    let r = ui.interact(rect, ui.scope_id().with(salt), Sense::click());
    let bg = if r.hovered() { ACCENT() } else { Color32::from_black_alpha(170) };
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
        .fill(CARD())
        .stroke(Stroke::new(1.0, theme::BORDER()))
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

/// A titled group in the details column.
fn card(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(CARD()).corner_radius(CornerRadius::same(8)).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = 6.0;
        ui.label(theme::caption(title));
        ui.add_space(2.0);
        body(ui);
    });
    ui.add_space(10.0);
}

/// Dim label on the left, value (wrapping) on the right.
fn fact(ui: &mut Ui, k: &str, v: &str) {
    const LABEL_W: f32 = 96.0;
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(vec2(LABEL_W, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(LABEL_W);
            ui.label(RichText::new(k).color(TEXT_DIM()));
        });
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 0.0), Layout::top_down(Align::Min), |ui| {
            ui.add(egui::Label::new(v).wrap());
        });
    });
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
    // Modals move to the top each frame; keep the list above them.
    if Popup::is_id_open(ui.ctx(), pid) {
        ui.ctx().move_to_top(egui::LayerId::new(egui::Order::Foreground, pid));
    }
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
/// What the grid sorts by: the sorting name when set.
fn sort_name(g: &Game) -> &str {
    g.sort_name.as_deref().filter(|s| !s.trim().is_empty()).unwrap_or(&g.name)
}

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
    /// Moves controller input into `pad_buf`; inside the details popup, D-pad and A become arrow/Enter
    /// keys so egui moves focus between its buttons and presses the focused one.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        use crate::gamepad::Pad;
        let dialog = self.any_dialog() && self.search_pop.is_none();
        let details = self.popup() && !self.any_dialog();
        let focused = ctx.memory(|m| m.focused());
        let mut has_focus = focused.is_some();
        let text_field = focused.filter(|id| egui::text_edit::TextEditState::load(ctx, *id).is_some());
        // Last frame's D-pad move in the details found nothing that way: scroll the page instead.
        if let Some((from, d)) = self.nav_probe.take() {
            if details && focused == Some(from) {
                self.scroll_details(d);
            }
        }
        if self.osk.is_some() && (text_field.is_none() || self.search_pop.is_some()) {
            self.osk = None;
        }
        if let Some(osk) = self.osk.as_mut() {
            use crate::osk::Out;
            let events: Vec<Pad> = std::mem::take(&mut self.pad_later).into_iter().chain(self.pad.try_iter()).collect();
            if !raw.focused {
                return;
            }
            let key = |raw: &mut egui::RawInput, key: Key, modifiers: egui::Modifiers| {
                for pressed in [true, false] {
                    raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
                }
            };
            for p in events {
                if p == Pad::BigScreen {
                    self.pad_buf.push(p);
                    continue;
                }
                match osk.pad(p) {
                    Out::None => {}
                    Out::Text(s) => raw.events.push(egui::Event::Text(s)),
                    Out::Backspace => key(raw, Key::Backspace, Default::default()),
                    Out::Clear => {
                        key(raw, Key::A, egui::Modifiers::COMMAND);
                        key(raw, Key::Backspace, Default::default());
                    }
                    Out::Close => {
                        self.osk = None;
                        // Outside dialogs, leave the field so the D-pad drives the grid again.
                        if let (false, Some(id)) = (dialog, text_field) {
                            ctx.memory_mut(|m| m.surrender_focus(id));
                        }
                        break;
                    }
                }
            }
            ctx.request_repaint();
            return;
        }
        let push = |raw: &mut egui::RawInput, key: Key, shift: bool| {
            let modifiers = egui::Modifiers { shift, ..Default::default() };
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers });
            }
        };
        // A dialog just opened from the controller: focus its first widget.
        if dialog && !self.was_dialog && self.kb_focus {
            push(raw, Key::Tab, false);
            has_focus = true;
        }
        self.was_dialog = dialog;
        let mut events = std::mem::take(&mut self.pad_later);
        events.extend(self.pad.try_iter());
        let mut moved = false;
        for (i, p) in events.iter().copied().enumerate() {
            if !raw.focused {
                continue;
            }
            // egui applies one focus move per frame; the rest wait for the next one.
            if moved && matches!(p, Pad::Nav(_) | Pad::Confirm) && (dialog || details) {
                self.pad_later.extend_from_slice(&events[i..]);
                ctx.request_repaint();
                break;
            }
            // Dialogs are mostly forms whose text fields keep the arrows, so the D-pad steps with Tab.
            let key = match (p, dialog) {
                (Pad::Nav(Nav::Up | Nav::Left), true) => Some((Key::Tab, true)),
                (Pad::Nav(Nav::Down | Nav::Right), true) => Some((Key::Tab, false)),
                (Pad::Nav(Nav::Up), false) => Some((Key::ArrowUp, false)),
                (Pad::Nav(Nav::Down), false) => Some((Key::ArrowDown, false)),
                (Pad::Nav(Nav::Left), false) => Some((Key::ArrowLeft, false)),
                (Pad::Nav(Nav::Right), false) => Some((Key::ArrowRight, false)),
                (Pad::Confirm, _) if has_focus => Some((Key::Enter, false)),
                (Pad::Confirm, true) => Some((Key::Tab, false)),
                (Pad::Back, true) => Some((Key::Escape, false)),
                _ => None,
            };
            // A on a text field opens the on-screen keyboard instead.
            if p == Pad::Confirm && text_field.is_some() && self.search_pop.is_none() {
                self.osk = Some(crate::osk::Osk::new());
                self.pad_later.extend_from_slice(&events[i + 1..]);
                ctx.request_repaint();
                break;
            }
            match key.filter(|_| dialog || details) {
                // Details opened with the mouse: the first move lands on Play.
                Some(_) if details && !has_focus => {
                    self.focus_play = true;
                    self.kb_focus = true;
                    moved = true;
                }
                Some((key, shift)) => {
                    if details && matches!(key, Key::ArrowUp | Key::ArrowDown) {
                        if let Some(f) = focused {
                            self.nav_probe = Some((f, if key == Key::ArrowUp { -160.0 } else { 160.0 }));
                            ctx.request_repaint();
                        }
                    }
                    push(raw, key, shift);
                    self.kb_focus = true;
                    moved = matches!(p, Pad::Nav(_) | Pad::Confirm);
                    has_focus = true;
                    // B also reaches handle_pad, which closes the simple dialogs.
                    if key == Key::Escape {
                        self.pad_buf.push(p);
                    }
                }
                None => self.pad_buf.push(p),
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.handle_events();
        match self.theme_watch.poll(&ctx) {
            Some(Ok(())) => self.toasts.info("Theme applied."),
            Some(Err(e)) => self.toasts.error(e),
            None => {}
        }
        match crate::RESTORED.lock().unwrap().take() {
            Some(Ok(m)) => self.toasts.success(m),
            Some(Err(e)) => self.toasts.error(format!("Restoring the backup failed: {e}")),
            None => {}
        }
        self.shots.begin_frame(&ctx);
        crate::shots_view::viewer(&ctx, &mut self.shots, &self.games);
        self.update_view();
        // A dialog over the details (editor, settings) doesn't count as reopening it.
        if self.detail_seen != self.selected {
            self.detail_seen = self.selected;
            self.detail_fresh = self.selected.is_some();
        }
        if let Some(ov) = self.ach_tab.as_mut().filter(|o| o.dirty) {
            // The sweep sends many updates; reread at most every few seconds.
            let now = ctx.input(|i| i.time);
            if now - ov.loaded > 3.0 {
                ov.refresh(&self.lib, now);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs(3));
            }
        }
        if let Some(st) = self.stats_tab.as_mut().filter(|s| s.dirty) {
            let now = ctx.input(|i| i.time);
            if now - st.loaded > 5.0 {
                st.refresh(&self.lib, now);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs(5));
            }
        }
        self.thumbs.begin_frame(&ctx);
        self.small_thumbs.begin_frame(&ctx);

        let typing = ctx.memory(|m| m.focused().is_some());
        if !self.any_dialog() && ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1)) {
            self.back();
        }
        ctx.input(|i| {
            if i.modifiers.command && i.key_pressed(Key::F) {
                self.focus_search = true;
            }
        });
        let mut actions = Vec::new();
        let grid_shown = self.ach_tab.is_none() && self.stats_tab.is_none() && !self.shots_tab && !self.coll_tab;
        let blocked = self.popup() || self.any_dialog();
        // Before handle_pad: a B that closes a dialog mustn't also close the details under it.
        let blocked_by_dialog = self.any_dialog();
        if ctx.input(|i| i.key_pressed(Key::F11)) {
            self.toggle_big(&ctx);
        }
        if !self.any_dialog() && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::R)) {
            self.random_game(&ctx);
        }
        self.handle_pad(&ctx, &mut actions);
        if self.big.is_some() && grid_shown && self.cursor.is_none() && !self.view.is_empty() {
            self.nav(Nav::Home, false);
        }
        if !typing && grid_shown && !blocked {
            self.grid_keys(&ctx, &mut actions);
        } else if !typing && !blocked_by_dialog {
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
            .show(ui, |ui| if self.big.is_some() { self.big_bar(ui) } else { self.top_bar(ui) });
        let line = Rect::from_min_size(top.response.rect.left_bottom() - vec2(0.0, 1.0), vec2(top.response.rect.width(), 1.5));
        theme::hgradient(ui.painter(), line, ACCENT().gamma_multiply(0.7), theme::ACCENT_2().gamma_multiply(0.15));
        egui::Panel::bottom("status")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(18, 5)))
            .show(ui, |ui| if self.big.is_some() { big_hints(ui) } else { self.status_bar(ui) });
        // List mode with a game open: the list narrows to the left (like Playnite), details take the rest.
        let side_open = self.selected.filter(|_| self.side());
        if side_open.is_some() {
            egui::Panel::left("list_side")
                .resizable(true)
                .default_size(340.0)
                .size_range(240.0..=900.0)
                .frame(egui::Frame::side_top_panel(ui.style()).fill(theme::BG()).inner_margin(egui::Margin::same(8)))
                .show(ui, |ui| self.list(ui, &mut actions));
        }
        egui::CentralPanel::no_frame()
            .frame(egui::Frame::central_panel(ui.style()).fill(theme::BG()).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| {
                // Switching tabs cross-fades the content in.
                let tab = if self.ach_tab.is_some() { 1 } else if self.stats_tab.is_some() { 2 } else if self.shots_tab { 3 } else if self.coll_tab { 4 } else { 0 };
                ui.multiply_opacity(theme::appear(&ctx, egui::Id::unique("tab_fade").with(tab), 0.18));
                let clicked = match (self.ach_tab.as_mut(), self.stats_tab.as_mut()) {
                    (Some(ov), _) => {
                        let busy = !self.workers.ach_busy.lock().unwrap().is_empty();
                        crate::ach_tab::show(ui, ov, &self.games, &mut self.small_thumbs, busy)
                    }
                    (_, Some(st)) => crate::stats_tab::show(ui, st, &self.games),
                    _ if self.coll_tab => {
                        self.collections_home(ui);
                        None
                    }
                    _ if self.shots_tab => match crate::shots_view::overview(ui, &mut self.shots, &self.games) {
                        Some(crate::shots_view::Ask::OpenGame(id)) => {
                            self.detail_tab_next = Some(3);
                            Some(id)
                        }
                        None => None,
                    },
                    _ => {
                        if let Some(id) = side_open {
                            self.details(ui, id, &mut actions);
                        } else if self.cfg.read().unwrap().list_view && self.big.is_none() {
                            self.list(ui, &mut actions);
                        } else {
                            self.grid(ui, &mut actions);
                        }
                        None
                    }
                };
                if let Some(id) = clicked {
                    self.selected = Some(id);
                    self.opened_at = ui.input(|i| i.time);
                }
            });

        if let Some(s) = self.startup.as_mut() {
            if s.tick(&ctx) {
                self.startup = None;
            }
        }
        let any_popup = self.popup() || self.any_dialog();
        if any_popup {
            theme::paint_backdrop(&ctx);
        }
        // Other dialogs would sit under the modal, so it steps aside while they are open.
        let dialog_open =
            self.editor.is_some()
            || self.settings.is_some()
            || self.confirm_remove.is_some()
            || self.confirm_refresh.is_some()
            || self.confirm_stop.is_some()
            || self.playnite.is_some()
            || self.scans.is_some();
        if let (Some(id), false, false) = (self.selected, dialog_open, self.side()) {
            let screen = ctx.content_rect();
            let size = vec2((screen.width() * 0.9).min(1800.0), (screen.height() * 0.92).min(1200.0));
            let frame = egui::Frame::new()
                .fill(theme::PANEL())
                .corner_radius(CornerRadius::same(10))
                .shadow(ctx.global_style().visuals.window_shadow)
                .stroke(Stroke::new(1.0, Color32::from_gray(50)));
            let resp = theme::popup(&ctx, egui::Id::unique("details_modal"), frame, Color32::TRANSPARENT, |ui| {
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
                    let what = match kind {
                        ArtKind::Cover => "Cover",
                        ArtKind::Banner => "Banner",
                        ArtKind::Icon => "Icon",
                    };
                    let name = self.game(id).map(|g| g.name.clone()).unwrap_or_default();
                    self.toasts.success(format!("{what} updated for {name}."));
                    if kind == ArtKind::Banner || self.game(id).is_some_and(|g| !g.has_hero) {
                        self.hero = None;
                    }
                    self.reload_one(id);
                }
            }
        }
        // Reopening a game rereads (and if needed refreshes) its achievements.
        if self.selected.is_none() {
            self.ach = None;
            self.sessions = None;
        }
        if let Some(p) = self.ra_pick.as_mut() {
            use crate::achievements::PickOutcome;
            match p.show(&ctx) {
                PickOutcome::Open => {}
                PickOutcome::Closed => self.ra_pick = None,
                PickOutcome::Picked(id, ra) => {
                    self.ra_pick = None;
                    if let Err(e) = self.lib.set_ra_id(id, ra) {
                        self.toasts.error(format!("{e:#}"));
                    }
                    self.reload_one(id);
                    self.ach = None;
                    self.workers.achievements(id);
                }
            }
        }
        if let Some(sp) = self.search_pop.as_mut() {
            let n = self.view.len();
            if sp.show(&ctx, &mut self.search, n) {
                self.close_search();
            }
        }
        self.health_window(&ctx, &mut actions);
        self.editor_window(&ctx);
        self.settings_window(&ctx);
        self.confirm_window(&ctx);
        self.refresh_window(&ctx);
        self.stop_window(&ctx);
        self.playnite_window(&ctx);
        self.game_menu_window(&ctx, &mut actions);
        self.scan_window(&ctx);
        self.coll_window(&ctx);
        self.coll_delete_window(&ctx);
        self.restore_window(&ctx);
        self.review_window(&ctx);
        self.uninstall_window(&ctx);
        if let Some(o) = &self.osk {
            o.show(&ctx);
        }
        self.play_window(&ctx, &mut actions);
        self.hooks_window(&ctx, &mut actions);
        if self.any_dialog() != self.was_dialog {
            ctx.request_repaint();
        }
        self.toasts.show(&ctx);

        for a in actions {
            self.apply(a);
        }
        self.thumbs.end_frame();
        self.small_thumbs.end_frame();
        self.track_window(&ctx);
        let view = (self.sort, self.filter, self.platform.clone(), self.tag.clone(), self.completion.clone(), self.collection);
        if self.saved_view != view {
            self.saved_view = view;
            self.save_prefs();
        }
    }

    /// Runs even while the window is hidden in the tray.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        use crate::tray::Cmd;
        // Starting in the tray: eframe shows the window after its first paint, so hide it again for a few
        // frames while it's cloaked, then uncloak (a hidden window stays hidden).
        if self.keep_hidden > 0 {
            self.keep_hidden -= 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.request_repaint();
            if self.keep_hidden == 0 {
                if let Some(h) = self.hwnd {
                    crate::startup::cloak(h, false);
                }
            }
        }
        if self.hidden {
            self.handle_events();
            // Controller presses made in a game mustn't replay when the window comes back.
            self.pad.try_iter().for_each(drop);
            self.pad_later.clear();
        }
        let cmds: Vec<Cmd> = self.tray.as_ref().map(|t| t.rx.try_iter().collect()).unwrap_or_default();
        for c in cmds {
            match c {
                Cmd::Show => self.show_window(ctx),
                Cmd::BigScreen => {
                    self.show_window(ctx);
                    if self.big.is_none() {
                        self.toggle_big(ctx);
                    }
                }
                Cmd::Play(id) => {
                    self.apply(Action::Play(id));
                    // "Another game is running" needs the window.
                    if self.confirm_play.is_some() || self.confirm_hooks.is_some() {
                        self.show_window(ctx);
                    }
                }
                Cmd::Quit => {
                    log::info!("exiting from the tray");
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            }
        }
        let to_tray = self.tray.is_some() && !self.quitting && !self.cfg.read().unwrap().quit_on_close;
        if to_tray && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            self.hidden = true;
            self.save_prefs();
            log::info!("hidden in the tray");
        }
        // Recently played games for the tray menu, every few seconds.
        if self.tray_refreshed.is_none_or(|t| t.elapsed().as_secs() >= 3) {
            self.tray_refreshed = Some(std::time::Instant::now());
            let mut recent: Vec<&Game> = self.games.iter().filter(|g| g.last_played > 0 && !g.hidden).collect();
            recent.sort_by_key(|g| std::cmp::Reverse(g.last_played));
            let recent = recent.iter().take(crate::tray::RECENT).map(|g| (g.id, g.name.clone())).collect();
            if let Some(t) = self.tray.as_mut() {
                t.set_recent(recent);
            }
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
            let placeholder = egui::vec2(w.min(640.0), w.min(640.0) * 9.0 / 16.0);
            if crate::video::is_gif(url) {
                if !crate::video::gif_failed(url) {
                    let size = crate::video::gif_dims(url).map_or(placeholder, |s| s * (w / s.x).min(1.0));
                    ui.add_space(if first { 4.0 } else { 16.0 });
                    crate::video::show_gif(ui, url, size);
                    ui.add_space(6.0);
                    (first, blank, prev_bullet) = (false, false, false);
                }
                continue;
            }
            let img = egui::Image::new(url.to_string()).corner_radius(6);
            // Exact size from the image itself: in a scroll area egui would otherwise fit it to the
            // visible space left, which is zero below the fold. Broken/unsupported images are skipped.
            let (size, ready) = match img.load_for_size(ui.ctx(), egui::vec2(w, w)) {
                Ok(egui::load::TexturePoll::Ready { texture }) => (Some(texture.size * (w / texture.size.x).min(1.0)), true),
                Ok(egui::load::TexturePoll::Pending { .. }) => (Some(placeholder), false),
                Err(_) => (None, false),
            };
            if let Some(size) = size {
                ui.add_space(if first { 4.0 } else { 16.0 });
                if !ready {
                    // Faint placeholder with a small spinner (egui's own spinner fills the whole image).
                    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                    ui.painter().rect_filled(rect, 6.0, CARD());
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
            ui.add(egui::Label::new(RichText::new(text.replace("**", "")).size(16.0).color(theme::TEXT())).wrap());
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
            color: if bold { theme::TEXT() } else { body },
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
