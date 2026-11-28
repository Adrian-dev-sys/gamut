//! Screenshots: a game's tab (grouped by day), the all-games tab (grouped by game) and a full-size viewer.
//! Thumbnails decode on worker threads, only for what's on screen.
use crate::theme::{self, CARD, TEXT, TEXT_DIM};
use crossbeam_channel::{unbounded, Receiver, Sender};
use egui::{vec2, Align2, Color32, ColorImage, CornerRadius, FontId, Key, Rect, RichText, Sense, TextureHandle, Ui};
use gamut_core::screenshots::Shot;
use gamut_core::Game;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

const THUMB_W: u32 = 640;
const FULL_W: u32 = 3840;

type Key2 = (PathBuf, u32);

pub struct ShotsView {
    pub all: Arc<Vec<Shot>>,
    /// Texture (None while loading) and the frame it was last drawn.
    tex: HashMap<Key2, (Option<TextureHandle>, u64)>,
    /// What to decode, and a preview file to use instead when it's at least that wide.
    req: Sender<(Key2, Option<PathBuf>)>,
    done: Receiver<(Key2, Option<ColorImage>)>,
    frame: u64,
    pub viewer: Option<Viewer>,
    /// The viewer put the window in full screen (undone when it closes).
    full_screen: bool,
    pub filter: Filter,
}

/// The all-games tab's filters.
#[derive(Default, Clone, PartialEq)]
pub struct Filter {
    pub search: String,
    /// 0 all, 1 Gamut (F12), 2 Steam.
    pub source: u8,
    /// "" = any.
    pub platform: String,
    /// 0 all time, else the last N days.
    pub days: i64,
    /// 0 latest shot, 1 name, 2 most shots.
    pub sort: u8,
}

pub struct Viewer {
    /// Indices into `all`.
    list: Vec<usize>,
    pos: usize,
}

/// What a view asks the page to do.
pub enum Ask {
    OpenGame(i64),
}

impl ShotsView {
    pub fn new() -> Self {
        let (req, rx) = unbounded::<(Key2, Option<PathBuf>)>();
        let (tx, done) = unbounded();
        for _ in 0..3 {
            let (rx, tx) = (rx.clone(), tx.clone());
            std::thread::spawn(move || {
                for (k, preview) in rx {
                    // Steam's previews are 200 px wide: too small, so the full image is decoded then.
                    let src = preview.filter(|p| image::image_dimensions(p).is_ok_and(|(w, _)| w >= k.1)).unwrap_or_else(|| k.0.clone());
                    let img = crate::thumbs::load_large(&src, k.1);
                    if tx.send((k, img)).is_err() {
                        return;
                    }
                }
            });
        }
        ShotsView { all: Arc::new(Vec::new()), tex: HashMap::new(), req, done, frame: 0, viewer: None, full_screen: false, filter: Filter::default() }
    }

    /// New index; an open viewer keeps its shots (found by path).
    pub fn set_all(&mut self, all: Arc<Vec<Shot>>) {
        if let Some(vw) = self.viewer.take() {
            let at: HashMap<&PathBuf, usize> = all.iter().enumerate().map(|(i, s)| (&s.path, i)).collect();
            let cur = &self.all[vw.list[vw.pos]].path;
            let list: Vec<usize> = vw.list.iter().filter_map(|&i| at.get(&self.all[i].path).copied()).collect();
            let pos = at.get(cur).and_then(|c| list.iter().position(|i| i == c)).unwrap_or(vw.pos.min(list.len().saturating_sub(1)));
            self.viewer = (!list.is_empty()).then_some(Viewer { list, pos });
        }
        self.all = all;
    }

    pub fn count(&self, game: i64) -> usize {
        self.all.iter().filter(|s| s.game_id == game).count()
    }

    /// Uploads finished decodes and drops textures unseen for a while.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok((k, img)) = self.done.try_recv() {
            let opts = egui::TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..egui::TextureOptions::LINEAR };
            let t = img.map(|i| ctx.load_texture(format!("shot:{}:{}", k.0.display(), k.1), i, opts));
            self.tex.insert(k, (t, self.frame));
            ctx.request_repaint();
        }
        // Full-size images (tens of MB) go as soon as they're off screen; thumbnails past 300.
        let (f, many) = (self.frame, self.tex.len() > 300);
        self.tex.retain(|(_, w), (t, seen)| t.is_none() || f - *seen < if *w == FULL_W { 3 } else if many { 120 } else { u64::MAX });
    }

    fn texture(&mut self, path: &PathBuf, w: u32, preview: Option<&PathBuf>) -> Option<TextureHandle> {
        let k = (path.clone(), w);
        match self.tex.get_mut(&k) {
            Some((t, seen)) => {
                *seen = self.frame;
                t.clone()
            }
            None => {
                self.tex.insert(k.clone(), (None, self.frame));
                let _ = self.req.send((k, preview.cloned()));
                None
            }
        }
    }

    /// One thumbnail, cropped to fill `size`; clicked opens the viewer on `list` at `pos`.
    fn thumb(&mut self, ui: &mut Ui, size: egui::Vec2, list: &[usize], pos: usize) {
        let (rect, r) = ui.allocate_exact_size(size, Sense::click());
        if !ui.is_rect_visible(rect) {
            return;
        }
        let s = &self.all[list[pos]];
        let (path, preview) = (s.path.clone(), s.thumb.clone());
        let radius = CornerRadius::same(6);
        ui.painter().rect_filled(rect, radius, CARD);
        if let Some(t) = self.texture(&path, THUMB_W, preview.as_ref()) {
            let [tw, th] = t.size().map(|v| v as f32);
            // Cover: crop the longer side.
            let (ia, ra) = (tw / th, rect.width() / rect.height());
            let uv = if ia > ra {
                let w = ra / ia;
                Rect::from_min_max(egui::pos2((1.0 - w) / 2.0, 0.0), egui::pos2((1.0 + w) / 2.0, 1.0))
            } else {
                let h = ia / ra;
                Rect::from_min_max(egui::pos2(0.0, (1.0 - h) / 2.0), egui::pos2(1.0, (1.0 + h) / 2.0))
            };
            egui::Image::new(&t).uv(uv).corner_radius(radius).paint_at(ui, rect);
        }
        if r.hovered() {
            ui.painter().rect_stroke(rect, radius, egui::Stroke::new(2.0, theme::ACCENT), egui::StrokeKind::Inside);
        }
        if r.clicked() {
            self.viewer = Some(Viewer { list: list.to_vec(), pos });
        }
    }
}

fn tile_size(width: f32) -> (usize, egui::Vec2) {
    let cols = ((width + 8.0) / (240.0 + 8.0)).floor().max(1.0) as usize;
    let w = (width - 8.0 * (cols - 1) as f32) / cols as f32;
    (cols, vec2(w, (w * 9.0 / 16.0).round()))
}

fn day(taken: i64) -> i64 {
    (taken + gamut_core::utc_offset()).div_euclid(86_400)
}

/// The About tab's row of the newest screenshots (the viewer browses all of them).
/// Returns true when "All screenshots" is clicked.
pub fn strip(ui: &mut Ui, v: &mut ShotsView, game: &Game) -> bool {
    let list: Vec<usize> = (0..v.all.len()).filter(|&i| v.all[i].game_id == game.id).collect();
    if list.is_empty() {
        return false;
    }
    let mut all = false;
    ui.add_space(18.0);
    ui.horizontal(|ui| {
        ui.label(theme::caption("Screenshots"));
        if ui.button(RichText::new(format!("All {}", list.len())).size(12.5)).clicked() {
            all = true;
        }
    });
    ui.add_space(6.0);
    // Bigger tiles than the Screenshots tab: one or two rows.
    let width = ui.available_width();
    let cols = ((width + 8.0) / (320.0 + 8.0)).floor().clamp(1.0, 4.0) as usize;
    let w = (width - 8.0 * (cols - 1) as f32) / cols as f32;
    let size = vec2(w, (w * 9.0 / 16.0).round());
    let shown = list.len().min(cols * 2);
    for row in (0..shown).collect::<Vec<_>>().chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            for &p in row {
                v.thumb(ui, size, &list, p);
            }
        });
        ui.add_space(8.0);
    }
    all
}

/// A game's screenshots, newest first, grouped by day.
pub fn game_tab(ui: &mut Ui, v: &mut ShotsView, game: &Game) {
    let list: Vec<usize> = (0..v.all.len()).filter(|&i| v.all[i].game_id == game.id).collect();
    if list.is_empty() {
        let key = if game.source == gamut_core::Source::Steam { "Steam's screenshot key" } else { "F12" };
        ui.label(RichText::new(format!("No screenshots yet. Press {key} while playing.")).color(TEXT_DIM));
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} screenshots", list.len())).size(15.0).color(TEXT));
        if let Some(dir) = v.all[list[0]].path.parent().map(PathBuf::from) {
            if ui.button(RichText::new("Open folder").size(12.5)).clicked() {
                let _ = open::that_detached(dir);
            }
        }
    });
    ui.add_space(8.0);
    let (cols, size) = tile_size(ui.available_width());
    let mut i = 0;
    while i < list.len() {
        let d = day(v.all[list[i]].taken);
        let end = (i..list.len()).find(|&j| day(v.all[list[j]].taken) != d).unwrap_or(list.len());
        ui.label(theme::caption(&crate::util::date(d * 86_400)));
        ui.add_space(4.0);
        for row in (i..end).collect::<Vec<_>>().chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                for &p in row {
                    v.thumb(ui, size, &list, p);
                }
            });
            ui.add_space(8.0);
        }
        ui.add_space(6.0);
        i = end;
    }
}

/// Every game with screenshots, the most recent first; one row of the newest per game.
pub fn overview(ui: &mut Ui, v: &mut ShotsView, games: &[Game]) -> Option<Ask> {
    let f = &mut v.filter;
    let mut platforms: Vec<&str> = games.iter().filter_map(|g| g.platform.as_deref()).collect();
    platforms.sort_unstable();
    platforms.dedup();
    ui.horizontal(|ui| {
        theme::section(ui, "Screenshots");
        ui.add_space(8.0);
        ui.add(egui::TextEdit::singleline(&mut f.search).hint_text("Search games").desired_width(220.0));
        let pick = |ui: &mut Ui, id: &str, cur: &mut u8, opts: &[&str]| {
            egui::ComboBox::from_id_salt(id).selected_text(opts[*cur as usize]).show_ui(ui, |ui| {
                for (i, o) in opts.iter().enumerate() {
                    ui.selectable_value(cur, i as u8, *o);
                }
            });
        };
        pick(ui, "shot_src", &mut f.source, &["All sources", "Gamut (F12)", "Steam"]);
        let plat = if f.platform.is_empty() { "All platforms" } else { f.platform.as_str() };
        egui::ComboBox::from_id_salt("shot_plat").selected_text(plat).width(170.0).show_ui(ui, |ui| {
            ui.selectable_value(&mut f.platform, String::new(), "All platforms");
            for p in &platforms {
                ui.selectable_value(&mut f.platform, p.to_string(), *p);
            }
        });
        let periods = [(0, "All time"), (7, "Last 7 days"), (30, "Last 30 days"), (365, "Last year")];
        let label = periods.iter().find(|p| p.0 == f.days).map_or("All time", |p| p.1);
        egui::ComboBox::from_id_salt("shot_days").selected_text(label).show_ui(ui, |ui| {
            for (d, l) in periods {
                ui.selectable_value(&mut f.days, d, l);
            }
        });
        pick(ui, "shot_sort", &mut f.sort, &["Latest first", "By name", "Most screenshots"]);
        if *f != Filter::default() && ui.button("Clear").clicked() {
            *f = Filter::default();
        }
    });
    let f = v.filter.clone();
    let since = if f.days > 0 { gamut_core::now() - f.days * 86_400 } else { i64::MIN };
    let words: Vec<String> = f.search.to_lowercase().split_whitespace().map(String::from).collect();
    let shown: HashMap<i64, &Game> = games
        .iter()
        .filter(|g| !g.hidden)
        .filter(|g| f.platform.is_empty() || g.platform.as_deref() == Some(f.platform.as_str()))
        .filter(|g| {
            let n = g.name.to_lowercase();
            words.iter().all(|w| n.contains(w.as_str()))
        })
        .map(|g| (g.id, g))
        .collect();
    let mut order: Vec<i64> = Vec::new();
    let mut by_game: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, s) in v.all.iter().enumerate() {
        if !shown.contains_key(&s.game_id) || s.taken < since || (f.source == 1 && s.steam) || (f.source == 2 && !s.steam) {
            continue;
        }
        by_game.entry(s.game_id).or_insert_with(|| {
            order.push(s.game_id);
            Vec::new()
        });
        by_game.get_mut(&s.game_id).unwrap().push(i);
    }
    match f.sort {
        1 => order.sort_by_cached_key(|id| shown[id].name.to_lowercase()),
        2 => order.sort_by_key(|id| std::cmp::Reverse(by_game[id].len())),
        _ => {}
    }
    let names: HashMap<i64, &str> = shown.iter().map(|(id, g)| (*id, g.name.as_str())).collect();
    let n: usize = by_game.values().map(Vec::len).sum();
    ui.label(RichText::new(format!("{n} screenshots in {} games · F12 or PS/Guide + R1 in non-Steam games, Steam's key in Steam games", order.len())).color(TEXT_DIM));
    ui.add_space(6.0);
    if order.is_empty() {
        ui.label(RichText::new(if v.all.is_empty() { "No screenshots yet." } else { "No screenshots match." }).color(TEXT_DIM));
        return None;
    }
    let width = ui.available_width() - 16.0;
    let (cols, size) = tile_size(width);
    // Every shot, grouped by game; only groups in view are laid out.
    const HEAD: f32 = 34.0;
    const GAP: f32 = 22.0;
    let row = size.y + 8.0;
    let heights: Vec<f32> = order.iter().map(|id| HEAD + by_game[id].len().div_ceil(cols) as f32 * row + GAP).collect();
    let total: f32 = heights.iter().sum();
    let mut ask = None;
    egui::ScrollArea::vertical().auto_shrink(false).show_viewport(ui, |ui, view| {
        ui.set_height(total);
        let origin = ui.min_rect().min;
        let mut top = 0.0;
        for (id, h) in order.iter().zip(&heights) {
            let (y, h) = (top, *h);
            top += h;
            if y + h < view.min.y || y > view.max.y {
                continue;
            }
            let list = &by_game[id];
            let rect = Rect::from_min_size(origin + vec2(0.0, y), vec2(width, h));
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
                ui.horizontal(|ui| {
                    let name = RichText::new(names[id]).size(16.0).color(TEXT).strong();
                    if ui.add(egui::Label::new(name).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        ask = Some(Ask::OpenGame(*id));
                    }
                    let latest = crate::util::date(v.all[list[0]].taken + gamut_core::utc_offset());
                    ui.label(RichText::new(format!("{} · latest {latest}", list.len())).color(TEXT_DIM));
                });
                ui.add_space(HEAD - 26.0);
                for chunk in (0..list.len()).collect::<Vec<_>>().chunks(cols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for &p in chunk {
                            v.thumb(ui, size, list, p);
                        }
                    });
                    ui.add_space(8.0 - ui.spacing().item_spacing.y);
                }
            });
        }
    });
    ask
}

/// Full-size viewer over everything: ←/→ to browse, Esc to close. Runs before other key handling.
pub fn viewer(ctx: &egui::Context, v: &mut ShotsView, games: &[Game]) {
    let Some(vw) = v.viewer.as_ref() else { return };
    let (n, mut pos) = (vw.list.len(), vw.pos);
    let (mut close, left, right, mut del) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::Escape),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowLeft),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowRight),
            i.consume_key(egui::Modifiers::NONE, Key::Delete),
        )
    });
    if left && pos > 0 {
        pos -= 1;
    }
    if right && pos + 1 < n {
        pos += 1;
    }
    let shot = v.all[vw.list[pos]].clone();
    let mut fs_toggle = None;
    let mut list = vw.list.clone();
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("shot_viewer")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        let (rect, bg) = ui.allocate_exact_size(screen.size(), Sense::click());
        ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(235));
        let area = rect.shrink2(vec2(24.0, 64.0));
        // The thumbnail shows until the full image is decoded.
        let full = v.texture(&shot.path, FULL_W, None);
        let thumb = v.tex.get(&(shot.path.clone(), THUMB_W)).and_then(|x| x.0.clone());
        if let Some(t) = full.or(thumb) {
            let [tw, th] = t.size().map(|v| v as f32);
            let s = (area.width() / tw).min(area.height() / th);
            let r = Rect::from_center_size(area.center(), vec2(tw * s, th * s));
            egui::Image::new(&t).paint_at(ui, r);
        } else {
            ui.put(Rect::from_center_size(area.center(), vec2(30.0, 30.0)), egui::Spinner::new());
        }
        let name = games.iter().find(|g| g.id == shot.game_id).map_or("", |g| g.name.as_str());
        let when = crate::util::date(shot.taken + gamut_core::utc_offset());
        let secs = (shot.taken + gamut_core::utc_offset()).rem_euclid(86_400);
        let text = format!("{name}  ·  {when} {:02}:{:02}  ·  {} / {n}{}", secs / 3600, secs / 60 % 60, pos + 1, if shot.steam { "  ·  Steam" } else { "" });
        ui.painter().text(rect.min + vec2(24.0, 30.0), Align2::LEFT_CENTER, text, FontId::proportional(15.0), TEXT);
        let bar = Rect::from_min_size(egui::pos2(rect.right() - 520.0, rect.top() + 14.0), vec2(496.0, 32.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
            if ui.button("Close").clicked() {
                close = true;
            }
            let full = ui.input(|i| i.viewport().fullscreen.unwrap_or(false));
            if ui.button(if full { "Exit full screen" } else { "Full screen" }).clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
                fs_toggle = Some(!full);
            }
            if ui.button("Show in folder").clicked() {
                show_in_folder(&shot.path);
            }
            if ui.button("Open").clicked() {
                let _ = open::that_detached(&shot.path);
            }
            // Steam's shots stay Steam's: it re-syncs them from the cloud.
            let b = ui.add_enabled(!shot.steam, egui::Button::new("Delete"));
            let b = if shot.steam { b.on_disabled_hover_text("Delete Steam screenshots in Steam") } else { b.on_hover_text("Moves it to the Recycle Bin (Del)") };
            del |= b.clicked();
        });
        // Arrows on the sides.
        for (go, at, label) in [(pos > 0, Align2::LEFT_CENTER, "‹"), (pos + 1 < n, Align2::RIGHT_CENTER, "›")] {
            if !go {
                continue;
            }
            let r = at.align_size_within_rect(vec2(56.0, 120.0), rect.shrink(8.0));
            let resp = ui.interact(r, ui.scope_id().with(label), Sense::click());
            let c = if resp.hovered() { TEXT } else { TEXT_DIM };
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(54.0), c);
            if resp.clicked() {
                pos = if label == "‹" { pos - 1 } else { pos + 1 };
            }
        }
        if bg.clicked() && !area.contains(bg.interact_pointer_pos().unwrap_or_default()) {
            close = true;
        }
        // Preload the neighbours.
        for p in [pos.wrapping_sub(1), pos + 1] {
            if let Some(&i) = list.get(p) {
                let path = v.all[i].path.clone();
                let _ = v.texture(&path, FULL_W, None);
            }
        }
    });
    if del && !shot.steam {
        match recycle(&shot) {
            Ok(()) => {
                let gone = list.remove(pos);
                let mut all = (*v.all).clone();
                all.remove(gone);
                v.all = Arc::new(all);
                list.iter_mut().filter(|i| **i > gone).for_each(|i| *i -= 1);
                close |= list.is_empty();
                pos = pos.min(list.len().saturating_sub(1));
            }
            Err(e) => log::warn!("deleting {}: {e:#}", shot.path.display()),
        }
    }
    if let Some(on) = fs_toggle {
        v.full_screen = on;
    }
    if close && v.full_screen {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
        v.full_screen = false;
    }
    v.viewer = (!close).then_some(Viewer { list, pos });
}

/// Sends a Gamut shot and its preview to the Recycle Bin.
fn recycle(shot: &Shot) -> anyhow::Result<()> {
    let mut files = vec![shot.path.clone()];
    files.extend(shot.thumb.clone());
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::Shell::*;
        // Double-null-terminated list of paths.
        let mut from: Vec<u16> = files.iter().flat_map(|f| f.as_os_str().encode_wide().chain([0])).collect();
        from.push(0);
        let mut op = SHFILEOPSTRUCTW {
            wFunc: FO_DELETE,
            pFrom: windows::core::PCWSTR(from.as_ptr()),
            fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
            ..Default::default()
        };
        // SAFETY: `from` outlives the call.
        let r = unsafe { SHFileOperationW(&mut op) };
        anyhow::ensure!(r == 0 && !op.fAnyOperationsAborted.as_bool(), "Recycle Bin refused (code {r})");
    }
    #[cfg(not(windows))]
    for f in files {
        std::fs::remove_file(f)?;
    }
    Ok(())
}

/// Explorer with the file selected (`/select,"path"` must reach it unquoted as a whole).
fn show_in_folder(path: &std::path::Path) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
    }
    #[cfg(not(windows))]
    let _ = open::that_detached(path.parent().unwrap_or(path));
}
