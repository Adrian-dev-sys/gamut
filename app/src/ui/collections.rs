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
    /// Sort while it's open; None = the library's.
    sort: Option<Sort>,
}

impl CollDialog {
    pub(super) fn new(add: Vec<i64>, rule: Option<Rule>) -> Self {
        let mut rule = rule.unwrap_or_default();
        // The old single tag joins the list.
        rule.tags = rule.tag_list().into_iter().map(String::from).collect();
        rule.tag.clear();
        CollDialog { id: None, name: String::new(), dynamic: add.is_empty() && rule != Rule::default(), rule, add, queries: Default::default(), sort: None }
    }
}

/// Whether a game is in a collection (hidden games only match a Hidden rule).
pub(super) fn contains(c: &Collection, g: &Game) -> bool {
    match &c.rule {
        None => c.games.contains(&g.id),
        Some(r) => {
            let tags = r.tag_list();
            let has = |t: &&str| split_list(&g.tags).any(|x| x == *t);
            matches(g, from_key(&FILTERS, &r.filter), &r.platform, "", &r.completion)
                && (tags.is_empty() || if r.all_tags { tags.iter().all(has) } else { tags.iter().any(has) })
        }
    }
}

/// The active collection when it's hand-filled.
pub(super) fn active(colls: &[Collection], id: Option<i64>) -> Option<&Collection> {
    id.and_then(|id| colls.iter().find(|c| c.id == id && c.rule.is_none()))
}

/// Add-to-collection submenu: checkboxes for one game, plain items for a selection (`game` None).
pub(super) fn collect_menu(ui: &mut Ui, colls: &[Collection], game: Option<&Game>, actions: &mut Vec<Action>) {
    ui.menu_button("Add to collection", |ui| {
        let manual: Vec<&Collection> = colls.iter().filter(|c| c.rule.is_none()).collect();
        for c in &manual {
            match game {
                Some(g) => {
                    let mut on = c.games.contains(&g.id);
                    // Steam decides which Steam games are in collections synced from it.
                    let synced = c.steam.is_some() && g.source == Source::Steam;
                    let r = ui.add_enabled(!synced, egui::Checkbox::new(&mut on, &c.name));
                    if r.clicked() {
                        actions.push(Action::Collect(c.id, Some(g.id), on));
                    }
                    r.on_disabled_hover_text("Synced from Steam: change it in Steam");
                }
                None => {
                    let r = ui.button(&c.name);
                    if r.clicked() {
                        actions.push(Action::Collect(c.id, None, true));
                        ui.close();
                    }
                    if c.steam.is_some() {
                        r.on_hover_text("Synced from Steam: Steam games go back to Steam's list on the next scan");
                    }
                }
            }
        }
        if !manual.is_empty() {
            ui.separator();
        }
        if ui.button("New collection…").clicked() {
            actions.push(Action::NewCollection(game.map(|g| g.id)));
            ui.close();
        }
    });
}

/// Chips for the collections a game is in (over the details banner); returns a clicked one.
pub(super) fn game_chips(ui: &mut Ui, at: egui::Pos2, max_w: f32, colls: &[Collection], g: &Game) -> Option<i64> {
    let mut x = at.x;
    let mut clicked = None;
    for c in colls.iter().filter(|c| c.id != FAVORITES && contains(c, g)) {
        let text = ui.painter().layout_no_wrap(c.name.to_uppercase(), theme::bold(11.5), Color32::WHITE);
        let r = Rect::from_min_size(pos2(x, at.y), text.size() + vec2(18.0, 10.0));
        if r.right() > at.x + max_w {
            break;
        }
        let resp = ui.interact(r, ui.scope_id().with(("chip", c.id)), Sense::click()).on_hover_text("Show this collection");
        let bg = if resp.hovered() { Color32::from_black_alpha(220) } else { Color32::from_black_alpha(150) };
        ui.painter().rect_filled(r, CornerRadius::same(4), bg);
        ui.painter().galley(r.min + vec2(9.0, 5.0), text, Color32::WHITE);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            clicked = Some(c.id);
        }
        x = r.right() + 6.0;
    }
    clicked
}

impl GamutApp {
    /// Rereads collections (Favorites first, then by name) and drops cached tile numbers.
    pub(super) fn load_collections(&mut self) {
        let fav = Rule { filter: "favorites".into(), ..Default::default() };
        let mut v = vec![Collection { id: FAVORITES, name: "Favorites".into(), rule: Some(fav), games: Default::default(), steam: None, sort: None }];
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

    fn stats(&mut self) {
        if self.coll_stats.is_some() {
            return;
        }
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
                tags: if self.tag.is_empty() { Vec::new() } else { vec![self.tag.clone()] },
                completion: self.completion.clone(),
                ..Default::default()
            };
            let mut d = CollDialog::new(Vec::new(), Some(rule));
            d.dynamic = true;
            d.name = [&self.tag, &self.platform, &self.completion].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_default();
            self.coll_dialog = Some(d);
        }
    }

    /// A collection with its own sort switches to it while open; leaving restores the library's.
    pub(super) fn apply_coll_sort(&mut self) {
        if self.coll_seen == self.collection {
            return;
        }
        if let Some(s) = self.sort_before.take() {
            self.sort = s;
        }
        let own = self.collection.and_then(|id| self.collections.iter().find(|c| c.id == id)).and_then(|c| c.sort.clone());
        if let Some(k) = own {
            self.sort_before = Some(self.sort);
            self.sort = from_key(&SORTS, &k);
        }
        self.coll_seen = self.collection;
    }

    /// Shows a collection's games in the library, with nothing else filtered out.
    pub(super) fn open_collection(&mut self, id: i64) {
        self.collection = Some(id);
        (self.ach_tab, self.stats_tab, self.shots_tab, self.coll_tab) = (None, None, false, false);
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

    /// Tiles in order: the "new" tile (not in big screen), then the collections.
    fn tile_ids(&self) -> Vec<Option<i64>> {
        let new = (self.big.is_none()).then_some(None);
        new.into_iter().chain(self.collections.iter().map(|c| Some(c.id))).collect()
    }

    /// Keyboard / controller move between tiles.
    pub(super) fn coll_nav(&mut self, n: Nav) {
        let len = self.tile_ids().len() as isize;
        let cols = self.coll_cols.max(1) as isize;
        let Some(cur) = self.coll_cursor.filter(|_| self.kb_focus) else {
            self.coll_cursor = Some(0);
            return;
        };
        let to = match n {
            Nav::Left => cur as isize - 1,
            Nav::Right => cur as isize + 1,
            Nav::Up => cur as isize - cols,
            Nav::Down => cur as isize + cols,
            Nav::PageUp | Nav::Home => 0,
            Nav::PageDown | Nav::End => len - 1,
        };
        if (0..len).contains(&to) {
            self.coll_cursor = Some(to as usize);
            self.coll_scroll_until = self.ctx_handle.input(|i| i.time) + 0.4;
        }
    }

    /// Enter / A on a tile.
    pub(super) fn coll_confirm(&mut self) {
        let Some(i) = self.coll_cursor.filter(|_| self.kb_focus) else {
            self.coll_cursor = Some(0);
            return;
        };
        match self.tile_ids().get(i) {
            Some(Some(id)) => self.open_collection(*id),
            Some(None) => self.coll_dialog = Some(CollDialog::new(Vec::new(), None)),
            None => {}
        }
    }

    /// The Collections tab.
    pub(super) fn collections_home(&mut self, ui: &mut Ui) {
        self.stats();
        let stats = self.coll_stats.take().unwrap_or_default();
        // Keyboard: arrows and Enter (the controller goes through handle_pad).
        if !self.any_dialog() {
            let (moves, enter) = ui.input(|i| {
                let keys = [(Key::ArrowLeft, Nav::Left), (Key::ArrowRight, Nav::Right), (Key::ArrowUp, Nav::Up), (Key::ArrowDown, Nav::Down), (Key::Home, Nav::Home), (Key::End, Nav::End)];
                (keys.iter().filter(|(k, _)| i.key_pressed(*k)).map(|(_, n)| *n).collect::<Vec<_>>(), i.key_pressed(Key::Enter))
            });
            for m in moves {
                self.coll_nav(m);
                self.kb_focus = true;
            }
            if enter {
                self.coll_confirm();
                self.kb_focus = true;
            }
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("YOUR COLLECTIONS").font(theme::bold(13.0)).color(TEXT_DIM));
            let r = ui.available_rect_before_wrap();
            ui.painter().hline(r.x_range(), r.center().y, Stroke::new(1.0, CARD_HI));
        });
        let avail = ui.available_width() - 8.0;
        let cols = (((avail + GAP) / (TILE + GAP)).floor() as usize).max(1);
        self.coll_cols = cols;
        let ids = self.tile_ids();
        let cursor = self.coll_cursor.filter(|_| self.kb_focus);
        let mut open = None;
        let mut edit = None;
        ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            // Room for the hover lift and cursor outline above the first row.
            ui.add_space(10.0);
            for row in 0..ids.len().div_ceil(cols) {
                let (row_rect, _) = ui.allocate_exact_size(vec2(avail, TILE + GAP), Sense::hover());
                for c in 0..cols {
                    let i = row * cols + c;
                    let Some(&id) = ids.get(i) else { break };
                    let rect = Rect::from_min_size(row_rect.min + vec2(c as f32 * (TILE + GAP) + 4.0, 0.0), vec2(TILE, TILE));
                    let is_cursor = cursor == Some(i);
                    if is_cursor && ui.ctx().input(|inp| inp.time) < self.coll_scroll_until {
                        ui.scroll_to_rect(rect.expand(12.0), None);
                    }
                    let Some(id) = id else {
                        if new_tile(ui, rect, is_cursor).clicked() {
                            self.coll_dialog = Some(CollDialog::new(Vec::new(), None));
                        }
                        continue;
                    };
                    let Some(coll) = self.collections.iter().find(|c| c.id == id) else { continue };
                    let (count, covers) = stats.get(&coll.id).cloned().unwrap_or_default();
                    let texs: Vec<TextureHandle> = covers.iter().filter_map(|&id| self.thumbs.get(id, true).cloned()).collect();
                    let tint = covers.first().and_then(|&id| self.thumbs.color(id));
                    let resp = tile(ui, rect, coll, count, &texs, tint, is_cursor);
                    let resp = if coll.steam.is_some() { resp.on_hover_text("Synced from Steam") } else { resp };
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
            self.kb_focus = false;
            self.open_collection(id);
        }
        if let Some(c) = edit.and_then(|id| self.collections.iter().find(|c| c.id == id)) {
            let mut d = CollDialog::new(Vec::new(), c.rule.clone());
            (d.id, d.name, d.dynamic) = (Some(c.id), c.name.clone(), c.rule.is_some());
            d.sort = c.sort.as_deref().map(|k| from_key(&SORTS, k));
            self.coll_dialog = Some(d);
        }
    }

    /// Create / edit dialog for a collection.
    pub(super) fn coll_window(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.coll_dialog.take() else { return };
        let (mut close, mut open) = (false, true);
        let title = if d.id.is_some() { "Edit collection" } else { "New collection" };
        let probe = Collection { id: 0, name: String::new(), rule: Some(d.rule.clone()), games: Default::default(), steam: None, sort: None };
        let matching = self.games.iter().filter(|g| !g.hidden && contains(&probe, g)).count();
        theme::dialog(ctx, "collection", false, |ui| {
            ui.set_width(600.0);
            dialog_header(ui, title, &mut open);
            let name = ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("Name").desired_width(f32::INFINITY));
            if ui.memory(|m| m.focused().is_none()) && d.name.is_empty() {
                name.request_focus();
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Sort by").color(TEXT_DIM));
                let label = |s: Option<Sort>| s.and_then(|s| SORT_LABELS.iter().find(|(x, _)| *x == s)).map_or("Library default", |(_, l)| *l);
                egui::ComboBox::from_id_salt("coll_sort").width(180.0).height(400.0).selected_text(label(d.sort)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut d.sort, None, "Library default");
                    for (s, l) in SORT_LABELS {
                        ui.selectable_value(&mut d.sort, Some(s), l);
                    }
                });
            });
            ui.add_space(8.0);
            if d.id.is_none() || d.dynamic {
                ui.radio_value(&mut d.dynamic, false, "Games I add (right-click a game → Add to collection)");
                ui.radio_value(&mut d.dynamic, true, "Every game matching these filters (updates by itself)");
            }
            if d.dynamic {
                ui.add_space(8.0);
                egui::Grid::new("coll_rule").num_columns(2).spacing(vec2(14.0, 10.0)).show(ui, |ui| {
                    ui.label(RichText::new("Show").color(TEXT_DIM));
                    let cur = from_key(&FILTERS, &d.rule.filter);
                    egui::ComboBox::from_id_salt("coll_filter")
                        .width(160.0)
                        .selected_text(BIG_TABS.iter().find(|(f, _)| *f == cur).map_or("All games", |(_, l)| *l))
                        .show_ui(ui, |ui| {
                            for (f, l) in BIG_TABS {
                                if ui.selectable_label(cur == f, l).clicked() {
                                    d.rule.filter = key_of(&FILTERS, f).into();
                                }
                            }
                        });
                    ui.end_row();
                    let [pq, tq, cq] = &mut d.queries;
                    ui.label(RichText::new("Platform").color(TEXT_DIM));
                    ui.horizontal(|ui| {
                        if let Some(p) = facet_picker(ui, "coll_platform", "All platforms", &d.rule.platform, pq, &self.platforms) {
                            d.rule.platform = p;
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Tags").color(TEXT_DIM));
                    ui.vertical(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            let mut drop = None;
                            for (i, t) in d.rule.tags.iter().enumerate() {
                                egui::Frame::new().fill(CARD_HI).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin { left: 8, right: 2, top: 2, bottom: 2 }).show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(t);
                                        if theme::x_button(ui, 18.0).on_hover_text("Remove").clicked() {
                                            drop = Some(i);
                                        }
                                    });
                                });
                            }
                            if let Some(i) = drop {
                                d.rule.tags.remove(i);
                            }
                            let hint = if d.rule.tags.is_empty() { "Any tag (add one…)" } else { "Add a tag…" };
                            if let Some(t) = facet_picker(ui, "coll_tag", hint, "", tq, &self.tags).filter(|t| !t.is_empty()) {
                                if !d.rule.tags.contains(&t) {
                                    d.rule.tags.push(t);
                                }
                            }
                        });
                        if d.rule.tags.len() > 1 {
                            ui.horizontal(|ui| {
                                ui.radio_value(&mut d.rule.all_tags, false, "Any of these tags (OR)");
                                ui.radio_value(&mut d.rule.all_tags, true, "All of these tags (AND)");
                            });
                        }
                    });
                    ui.end_row();
                    ui.label(RichText::new("Status").color(TEXT_DIM));
                    ui.horizontal(|ui| {
                        if let Some(c) = facet_picker(ui, "coll_status", "Any status", &d.rule.completion, cq, &self.completions) {
                            d.rule.completion = c;
                        }
                    });
                    ui.end_row();
                });
                ui.add_space(4.0);
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
                    let sort = d.sort.map(|s| key_of(&SORTS, s));
                    let r = match d.id {
                        Some(id) => self.lib.update_collection(id, d.name.trim(), rule).and_then(|_| self.lib.set_collection_sort(id, sort)),
                        None => self.lib.create_collection(d.name.trim(), rule).and_then(|id| {
                            self.lib.set_collection_sort(id, sort)?;
                            if !d.dynamic && !d.add.is_empty() {
                                self.lib.set_in_collection(id, &d.add, true)?;
                            }
                            Ok(())
                        }),
                    };
                    // The open collection picks up a new sort right away.
                    if d.id.is_some() && d.id == self.collection {
                        self.coll_seen = None;
                    }
                    if let Err(e) = r {
                        self.toasts.error(format!("{e:#}"));
                    }
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
                if !ok {
                    ui.label(theme::hint("Name it first"));
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
        let synced = self.collections.iter().any(|c| c.id == id && c.steam.is_some());
        let (mut close, mut open) = (false, true);
        theme::dialog(ctx, "Delete collection", false, |ui| {
            ui.set_max_width(460.0);
            dialog_header(ui, "Delete collection", &mut open);
            ui.label(format!("Delete \"{name}\"? Its games stay in the library."));
            if synced {
                ui.label(theme::hint("It's synced from Steam; it won't be synced again (Settings → Library can bring it back)."));
            }
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    if let Err(e) = self.lib.delete_collection(id) {
                        self.toasts.error(format!("{e:#}"));
                    }
                    // A synced one stays deleted: stop syncing it.
                    if let Some(key) = self.collections.iter().find(|c| c.id == id).and_then(|c| c.steam.clone()) {
                        let mut cfg = self.cfg.write().unwrap();
                        cfg.steam_collections_ignored.push(key);
                        if let Err(e) = cfg.save(&self.paths.config) {
                            self.toasts.error(format!("Saving settings failed: {e:#}"));
                        }
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

/// White outline for the keyboard / controller cursor.
fn cursor_ring(p: &egui::Painter, rect: Rect) {
    p.rect_stroke(rect, CornerRadius::same(4), Stroke::new(3.0, Color32::WHITE), StrokeKind::Outside);
}

/// Outlined "+" tile that creates a collection.
fn new_tile(ui: &mut Ui, rect: Rect, is_cursor: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with("new_coll"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    let r = CornerRadius::same(4);
    let hot = resp.hovered() || is_cursor;
    if hot {
        p.rect_filled(rect, r, CARD);
    }
    p.rect_stroke(rect, r, Stroke::new(1.5, if hot { TEXT_DIM } else { CARD_HI }), StrokeKind::Inside);
    let c = rect.center() - vec2(0.0, 18.0);
    let s = Stroke::new(2.5, TEXT);
    p.line_segment([c - vec2(11.0, 0.0), c + vec2(11.0, 0.0)], s);
    p.line_segment([c - vec2(0.0, 11.0), c + vec2(0.0, 11.0)], s);
    p.text(rect.center() + vec2(0.0, 22.0), Align2::CENTER_CENTER, "CREATE A NEW\nCOLLECTION", theme::bold(13.0), TEXT);
    if is_cursor {
        cursor_ring(p, rect);
    }
    resp
}

/// A collection tile: tilted covers under a dark wash, the name and game count on top.
fn tile(ui: &mut Ui, rect: Rect, c: &Collection, count: usize, covers: &[TextureHandle], tint: Option<Color32>, is_cursor: bool) -> egui::Response {
    let resp = ui.interact(rect, ui.scope_id().with(("coll", c.id)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    let hot = ui.ctx().animate_bool_with_time_and_easing(resp.id.with("hot"), resp.hovered() || is_cursor, 0.18, egui::emath::easing::cubic_out);
    let rect = rect.translate(vec2(0.0, -3.0 * hot));
    let r = CornerRadius::same(4);
    let p = ui.painter().clone();
    p.add(egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(120) }.as_shape(rect, r));
    p.rect_filled(rect, r, Color32::from_rgb(44, 52, 74));
    if !covers.is_empty() {
        // 3x3 covers on a grid turned by the tilt angle, clipped to the tile.
        let mut clip = ui.new_child(egui::UiBuilder::new().max_rect(rect));
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
    if is_cursor {
        cursor_ring(&p, rect);
    } else if hot > 0.01 {
        p.rect_stroke(rect, r, Stroke::new(1.5, Color32::WHITE.gamma_multiply(0.7 * hot)), StrokeKind::Inside);
    }
    let mut job = egui::text::LayoutJob::simple(c.name.to_uppercase(), theme::bold(17.0), Color32::WHITE, TILE - 24.0);
    job.halign = Align::Center;
    let name = p.layout_job(job);
    let count_g = p.layout_no_wrap(format!("( {count} )"), theme::bold(13.5), TEXT);
    let total = name.size().y + 4.0 + count_g.size().y;
    let top = rect.center().y - total / 2.0;
    // Centered galley: x is the middle of each line.
    p.galley(pos2(rect.center().x, top), name.clone(), Color32::WHITE);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_rules_and_or() {
        let mut g = Game::new_custom("X".into(), None);
        g.tags = Some("2D, Platformer".into());
        let rule = |all, tags: &[&str]| Collection {
            id: 1,
            name: String::new(),
            rule: Some(Rule { tags: tags.iter().map(|t| t.to_string()).collect(), all_tags: all, ..Default::default() }),
            games: Default::default(),
            steam: None,
            sort: None,
        };
        assert!(contains(&rule(false, &["3D", "2D"]), &g));
        assert!(!contains(&rule(true, &["3D", "2D"]), &g));
        assert!(contains(&rule(true, &["Platformer", "2D"]), &g));
        // The first version's single tag still counts.
        let old = Collection { rule: Some(Rule { tag: "3D".into(), ..Default::default() }), ..rule(false, &[]) };
        assert!(!contains(&old, &g));
    }
}
