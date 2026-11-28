//! Collections: Steam-style tiles for groups of games, filled by hand or by filters ("dynamic").
use super::*;
use crate::theme::TEXT;
use egui::Align2;
use gamut_core::model::{Collection, Rule};
use std::collections::HashMap;

const TILE: f32 = 184.0;
const GAP: f32 = 16.0;
/// The built-in Favorites tile; not stored in the database.
pub(super) const FAVORITES: i64 = -1;

/// Per collection: games in it and up to 9 covers, most recently played first.
pub(super) type Stats = HashMap<i64, (usize, Vec<i64>)>;

/// Create or edit dialog. `add`: games that go into a new hand-filled collection.
pub(super) struct CollDialog {
    id: Option<i64>,
    name: String,
    dynamic: bool,
    rule: Rule,
    add: Vec<i64>,
    queries: [String; 3],
}

impl CollDialog {
    pub(super) fn new(add: Vec<i64>, rule: Option<Rule>) -> Self {
        CollDialog { id: None, name: String::new(), dynamic: rule.is_some(), rule: rule.unwrap_or_default(), add, queries: Default::default() }
    }
}

/// Whether a game is in a collection (hidden games are left out by the caller's filter).
pub(super) fn contains(c: &Collection, g: &Game) -> bool {
    match &c.rule {
        None => c.games.contains(&g.id),
        Some(r) => matches(g, from_key(&FILTERS, &r.filter), &r.platform, &r.tag, &r.completion),
    }
}

/// The active collection when it's hand-filled.
pub(super) fn active(colls: &[Collection], id: Option<i64>) -> Option<&Collection> {
    id.and_then(|id| colls.iter().find(|c| c.id == id && c.rule.is_none()))
}

/// Add-to-collection submenu: checkboxes for one game, plain items for a selection (`game` None).
pub(super) fn collect_menu(ui: &mut Ui, colls: &[Collection], game: Option<i64>, actions: &mut Vec<Action>) {
    ui.menu_button("Add to collection", |ui| {
        let manual: Vec<&Collection> = colls.iter().filter(|c| c.rule.is_none()).collect();
        for c in &manual {
            match game {
                Some(id) => {
                    let mut on = c.games.contains(&id);
                    if ui.checkbox(&mut on, &c.name).clicked() {
                        actions.push(Action::Collect(c.id, Some(id), on));
                    }
                }
                None => {
                    if ui.button(&c.name).clicked() {
                        actions.push(Action::Collect(c.id, None, true));
                        ui.close();
                    }
                }
            }
        }
        if !manual.is_empty() {
            ui.separator();
        }
        if ui.button("New collection…").clicked() {
            actions.push(Action::NewCollection(game));
            ui.close();
        }
    });
}

impl GamutApp {
    /// Rereads collections (Favorites first, then by name) and drops cached tile numbers.
    pub(super) fn load_collections(&mut self) {
        let fav = Rule { filter: "favorites".into(), ..Default::default() };
        let mut v = vec![Collection { id: FAVORITES, name: "Favorites".into(), rule: Some(fav), games: Default::default() }];
        match self.lib.collections() {
            Ok(mut c) => {
                c.sort_by_cached_key(|c| util::nat_key(&c.name.to_lowercase()));
                v.extend(c);
            }
            Err(e) => self.toasts.error(format!("Loading collections failed: {e:#}")),
        }
        self.collections = v;
        if self.collection.is_some_and(|id| !self.collections.iter().any(|c| c.id == id)) {
            self.collection = None;
        }
        self.coll_stats = None;
        self.view_key = None;
    }

    fn stats(&mut self) -> &Stats {
        if self.coll_stats.is_none() {
            let mut order: Vec<&Game> = self.games.iter().filter(|g| !g.hidden).collect();
            order.sort_by_key(|g| std::cmp::Reverse((g.last_played, g.added)));
            let stats = self
                .collections
                .iter()
                .map(|c| {
                    let mut n = 0;
                    let mut covers = Vec::new();
                    for g in order.iter().filter(|g| contains(c, g)) {
                        n += 1;
                        if g.has_cover && covers.len() < 9 {
                            covers.push(g.id);
                        }
                    }
                    (c.id, (n, covers))
                })
                .collect();
            self.coll_stats = Some(stats);
        }
        self.coll_stats.as_ref().unwrap()
    }

    /// Library controls: the open collection (with a way out), or saving the current filters as one.
    pub(super) fn collection_chip(&mut self, ui: &mut Ui) {
        if let Some(c) = self.collection.and_then(|id| self.collections.iter().find(|c| c.id == id)) {
            let name = c.name.clone();
            egui::Frame::new().fill(ACCENT.gamma_multiply(0.25)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(8, 2)).show(ui, |ui| {
                ui.label(RichText::new(name).strong().color(Color32::WHITE));
                if theme::x_button(ui, 20.0).on_hover_text("Back to all games").clicked() {
                    self.collection = None;
                    self.scroll_to = Some(0.0);
                }
            });
            return;
        }
        let filtered = self.filter != Filter::All || !self.platform.is_empty() || !self.tag.is_empty() || !self.completion.is_empty();
        if filtered && self.filter != Filter::Hidden && ui.button("Save as collection").on_hover_text("A collection that always shows the games matching these filters").clicked() {
            let rule = Rule {
                filter: key_of(&FILTERS, self.filter).into(),
                platform: self.platform.clone(),
                tag: self.tag.clone(),
                completion: self.completion.clone(),
            };
            let mut d = CollDialog::new(Vec::new(), Some(rule));
            d.name = [&self.tag, &self.platform, &self.completion].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
            self.coll_dialog = Some(d);
        }
    }

    /// Shows a collection's games in the library, with nothing else filtered out.
    pub(super) fn open_collection(&mut self, id: i64) {
        self.collection = Some(id);
        self.coll_tab = false;
        self.filter = Filter::All;
        (self.platform, self.tag, self.completion) = (String::new(), String::new(), String::new());
        self.search.clear();
        (self.selected, self.cursor) = (None, None);
        self.multi.clear();
        self.scroll_to = Some(0.0);
    }

    pub(super) fn collect(&mut self, cid: i64, ids: &[i64], on: bool) -> anyhow::Result<()> {
        self.lib.set_in_collection(cid, ids, on)?;
        self.load_collections();
        if ids.len() > 1 {
            let name = self.collections.iter().find(|c| c.id == cid).map(|c| c.name.clone()).unwrap_or_default();
            let what = if on { "Added" } else { "Removed" };
            self.toasts.success(format!("{what} {} games {} {name}.", ids.len(), if on { "to" } else { "from" }));
        }
        Ok(())
    }

    /// The Collections tab: a "new" tile, then one tile per collection.
    pub(super) fn collections_home(&mut self, ui: &mut Ui) {
        self.stats();
        let stats = self.coll_stats.take().unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(RichText::new("YOUR COLLECTIONS").size(12.5).color(TEXT_DIM));
            let r = ui.available_rect_before_wrap();
            ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, CARD_HI));
        });
        ui.add_space(10.0);
        let avail = ui.available_width() - 8.0;
        let cols = (((avail + GAP) / (TILE + GAP)).floor() as usize).max(1);
        let n = self.collections.len() + 1;
        let mut open = None;
        let mut edit = None;
        ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            for row in 0..n.div_ceil(cols) {
                let (row_rect, _) = ui.allocate_exact_size(vec2(avail, TILE + GAP), Sense::hover());
                for c in 0..cols {
                    let i = row * cols + c;
                    if i >= n {
                        break;
                    }
                    let rect = Rect::from_min_size(row_rect.min + vec2(c as f32 * (TILE + GAP), 0.0), vec2(TILE, TILE));
                    if i == 0 {
                        if new_tile(ui, rect).clicked() {
                            self.coll_dialog = Some(CollDialog::new(Vec::new(), None));
                        }
                        continue;
                    }
                    let coll = &self.collections[i - 1];
                    let (count, covers) = stats.get(&coll.id).cloned().unwrap_or_default();
                    let texs: Vec<TextureHandle> = covers.iter().filter_map(|&id| self.thumbs.get(id, true).cloned()).collect();
                    let tint = covers.first().and_then(|&id| self.thumbs.color(id));
                    let resp = tile(ui, rect, coll, count, &texs, tint);
                    if resp.clicked() {
                        open = Some(coll.id);
                    }
                    if coll.id != FAVORITES {
                        resp.context_menu(|ui| {
                            if ui.button(if coll.rule.is_some() { "Edit…" } else { "Rename…" }).clicked() {
                                edit = Some(coll.id);
                            }
                            if ui.button("Delete…").clicked() {
                                self.confirm_coll_delete = Some(coll.id);
                            }
                        });
                    }
                }
            }
        });
        self.coll_stats = Some(stats);
        if let Some(id) = open {
            self.open_collection(id);
        }
        if let Some(c) = edit.and_then(|id| self.collections.iter().find(|c| c.id == id)) {
            let mut d = CollDialog::new(Vec::new(), c.rule.clone());
            (d.id, d.name) = (Some(c.id), c.name.clone());
            self.coll_dialog = Some(d);
        }
    }

    /// Create / edit dialog for a collection.
    pub(super) fn coll_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.coll_dialog.take() else { return };
        let (mut close, mut open) = (false, true);
        let title = if d.id.is_some() { "Edit collection" } else { "New collection" };
        let probe = Collection { id: 0, name: String::new(), rule: Some(d.rule.clone()), games: Default::default() };
        let matching = self.games.iter().filter(|g| !g.hidden && contains(&probe, g)).count();
        theme::dialog(ctx, "collection", false, |ui| {
            ui.set_width(520.0);
            dialog_header(ui, title, &mut open);
            let name = ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("Name").desired_width(f32::INFINITY));
            if ui.memory(|m| m.focused().is_none()) {
                name.request_focus();
            }
            ui.add_space(8.0);
            if d.id.is_none() || d.dynamic {
                ui.radio_value(&mut d.dynamic, false, "Games I add (right-click a game → Add to collection)");
                ui.radio_value(&mut d.dynamic, true, "Every game matching these filters (updates by itself)");
            }
            if d.dynamic {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let cur = from_key(&FILTERS, &d.rule.filter);
                    egui::ComboBox::from_id_salt("coll_filter")
                        .selected_text(BIG_TABS.iter().find(|(f, _)| *f == cur).map_or("All games", |(_, l)| *l))
                        .show_ui(ui, |ui| {
                            for (f, l) in BIG_TABS {
                                if ui.selectable_label(cur == f, l).clicked() {
                                    d.rule.filter = key_of(&FILTERS, f).into();
                                }
                            }
                        });
                    let [pq, tq, cq] = &mut d.queries;
                    if let Some(p) = facet_picker(ui, "coll_platform", "All platforms", &d.rule.platform, pq, &self.platforms) {
                        d.rule.platform = p;
                    }
                    if let Some(t) = facet_picker(ui, "coll_tag", "All tags", &d.rule.tag, tq, &self.tags) {
                        d.rule.tag = t;
                    }
                    if let Some(c) = facet_picker(ui, "coll_status", "Any status", &d.rule.completion, cq, &self.completions) {
                        d.rule.completion = c;
                    }
                });
                ui.label(RichText::new(format!("{matching} games match")).color(TEXT_DIM));
            } else if !d.add.is_empty() {
                ui.label(RichText::new(format!("{} games will be added", d.add.len())).color(TEXT_DIM));
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let ok = !d.name.trim().is_empty();
                let enter = ok && name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add_enabled(ok, egui::Button::new(if d.id.is_some() { "Save" } else { "Create" })).clicked() || enter {
                    let rule = d.dynamic.then_some(&d.rule);
                    let r = match d.id {
                        Some(id) => self.lib.update_collection(id, d.name.trim(), rule),
                        None => self.lib.create_collection(d.name.trim(), rule).and_then(|id| {
                            if !d.dynamic && !d.add.is_empty() {
                                self.lib.set_in_collection(id, &d.add, true)?;
                            }
                            Ok(())
                        }),
                    };
                    if let Err(e) = r {
                        self.toasts.error(format!("{e:#}"));
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.load_collections();
        }
        if !close && open {
            self.coll_dialog = Some(d);
        }
    }

    pub(super) fn coll_delete_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.confirm_coll_delete else { return };
        let name = self.collections.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default();
        let (mut close, mut open) = (false, true);
        theme::dialog(ctx, "Delete collection", false, |ui| {
            ui.set_max_width(460.0);
            dialog_header(ui, "Delete collection", &mut open);
            ui.label(format!("Delete \"{name}\"? Its games stay in the library."));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    if let Err(e) = self.lib.delete_collection(id) {
                        self.toasts.error(format!("{e:#}"));
                    }
                    self.load_collections();
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close || !open {
            self.confirm_coll_delete = None;
        }
    }
}

/// Dashed-looking "+" tile that creates a collection.
fn new_tile(ui: &mut Ui, rect: Rect) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with("new_coll"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    let r = CornerRadius::same(4);
    if resp.hovered() {
        p.rect_filled(rect, r, CARD);
    }
    p.rect_stroke(rect, r, Stroke::new(1.5, if resp.hovered() { TEXT_DIM } else { CARD_HI }), StrokeKind::Inside);
    let c = rect.center() - vec2(0.0, 14.0);
    let s = Stroke::new(2.0, TEXT);
    p.line_segment([c - vec2(9.0, 0.0), c + vec2(9.0, 0.0)], s);
    p.line_segment([c - vec2(0.0, 9.0), c + vec2(0.0, 9.0)], s);
    p.text(rect.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, "CREATE A NEW\nCOLLECTION", FontId::proportional(11.5), TEXT);
    resp
}

/// A collection tile: tilted covers under a dark wash, the name and game count on top.
fn tile(ui: &mut Ui, rect: Rect, c: &Collection, count: usize, covers: &[TextureHandle], tint: Option<Color32>) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with(("coll", c.id)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hot = ui.ctx().animate_bool_with_time_and_easing(resp.id.with("hot"), resp.hovered(), 0.18, egui::emath::easing::cubic_out);
    let rect = rect.translate(vec2(0.0, -3.0 * hot));
    let r = CornerRadius::same(4);
    let p = ui.painter().clone();
    p.add(egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(rect, r));
    p.rect_filled(rect, r, Color32::from_rgb(44, 52, 74));
    if !covers.is_empty() {
        // 3x3 covers on a grid turned by the tilt angle, clipped to the tile.
        let clip = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        let mut clip = clip;
        clip.set_clip_rect(rect.intersect(ui.clip_rect()));
        let angle = -0.5;
        let rot = egui::emath::Rot2::from_angle(angle);
        let (w, h) = (TILE * 0.40, TILE * 0.60);
        for i in 0..9 {
            let (col, row) = ((i % 3) as f32 - 1.0, (i / 3) as f32 - 1.0);
            // Odd columns shift half a cover, like a staggered shelf.
            let off = vec2(col * (w + 6.0), row * (h + 6.0) + col * h * 0.5);
            let center = rect.center() + rot * off;
            egui::Image::from_texture(&covers[i % covers.len()]).rotate(angle, vec2(0.5, 0.5)).paint_at(&clip, Rect::from_center_size(center, vec2(w, h)));
        }
        let wash = tint.map_or(Color32::from_black_alpha(150), |t| Color32::from_rgba_unmultiplied(t.r() / 3, t.g() / 3, t.b() / 3, 150));
        p.rect_filled(rect, r, wash);
        paint_vgradient(&p, Rect::from_min_max(pos2(rect.left(), rect.center().y - 20.0), rect.max), Color32::TRANSPARENT, Color32::from_black_alpha(170));
    }
    if hot > 0.01 {
        p.rect_stroke(rect, r, Stroke::new(1.5, Color32::WHITE.gamma_multiply(0.7 * hot)), StrokeKind::Inside);
    }
    let name = p.layout(c.name.to_uppercase(), FontId::proportional(14.5), Color32::WHITE, TILE - 24.0);
    let count_g = p.layout_no_wrap(format!("( {count} )"), FontId::proportional(12.0), TEXT.gamma_multiply(0.8));
    let total = name.size().y + 4.0 + count_g.size().y;
    let top = rect.center().y - total / 2.0;
    p.galley(pos2(rect.center().x - name.size().x / 2.0, top), name.clone(), Color32::WHITE);
    p.galley(pos2(rect.center().x - count_g.size().x / 2.0, top + name.size().y + 4.0), count_g, TEXT);
    if c.rule.is_some() && c.id != FAVORITES {
        paint_bolt(&p, rect.right_bottom() + vec2(-15.0, -15.0));
    }
    resp
}

/// Small lightning bolt badge marking a dynamic (filter-based) collection.
fn paint_bolt(p: &egui::Painter, c: egui::Pos2) {
    p.rect_filled(Rect::from_center_size(c, vec2(18.0, 18.0)), CornerRadius::same(3), Color32::from_black_alpha(150));
    // Two triangles: the bolt isn't convex.
    for t in [[vec2(2.0, -7.0), vec2(-4.5, 1.5), vec2(1.0, 0.5)], [vec2(-2.0, 7.0), vec2(4.5, -1.5), vec2(-1.0, -0.5)]] {
        p.add(egui::Shape::convex_polygon(t.iter().map(|v| c + *v).collect(), Color32::WHITE, Stroke::NONE));
    }
}
