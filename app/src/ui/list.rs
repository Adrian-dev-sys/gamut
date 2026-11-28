//! List mode: one row per game with sortable columns. Shares the grid's cursor, selection, menus and keys
//! (it's a one-column grid to `nav`).
use super::*;
use crate::theme::{self, TEXT};
use egui::Align2;

const ROW_H: f32 = 44.0;
const HEAD_H: f32 = 30.0;

#[derive(Clone, Copy, PartialEq)]
enum Col {
    Name,
    Platform,
    Completion,
    Playtime,
    LastPlayed,
    Added,
    Score,
}

impl Col {
    fn label(self) -> &'static str {
        match self {
            Col::Name => "Name",
            Col::Platform => "Platform",
            Col::Completion => "Status",
            Col::Playtime => "Time played",
            Col::LastPlayed => "Last played",
            Col::Added => "Added",
            Col::Score => "Score",
        }
    }
    fn width(self) -> f32 {
        match self {
            Col::Name => 0.0,
            Col::Platform => 120.0,
            Col::Completion => 120.0,
            Col::Playtime => 100.0,
            Col::LastPlayed | Col::Added => 100.0,
            Col::Score => 60.0,
        }
    }
    fn sort(self) -> Option<Sort> {
        match self {
            Col::Name => Some(Sort::Name),
            Col::Platform => None,
            Col::Completion => Some(Sort::Completion),
            Col::Playtime => Some(Sort::Playtime),
            Col::LastPlayed => Some(Sort::LastPlayed),
            Col::Added => Some(Sort::Added),
            Col::Score => Some(Sort::CriticScore),
        }
    }
    fn text(self, g: &Game) -> String {
        match self {
            Col::Name => g.name.clone(),
            Col::Platform => short_platform(g),
            Col::Completion => g.completion.clone().unwrap_or_default(),
            Col::Playtime => if g.playtime_min > 0 { util::playtime(g.playtime_min) } else { String::new() },
            Col::LastPlayed => if g.last_played > 0 { util::date(g.last_played) } else { String::new() },
            Col::Added => if g.added > 0 { util::date(g.added) } else { String::new() },
            Col::Score => g.critic_score.map(|s| s.to_string()).unwrap_or_default(),
        }
    }
}

/// Columns that fit `w`: the least useful drop first on narrow windows.
fn columns(w: f32) -> Vec<Col> {
    let mut cols = vec![Col::Name, Col::Platform, Col::Completion, Col::Playtime, Col::LastPlayed, Col::Added, Col::Score];
    for drop in [Col::Added, Col::Score, Col::Completion, Col::LastPlayed, Col::Platform] {
        let fixed: f32 = cols.iter().map(|c| c.width()).sum::<f32>() + 56.0;
        if w - fixed >= 260.0 {
            break;
        }
        cols.retain(|c| *c != drop);
    }
    cols
}

/// x offset and width of each column; Name takes what's left.
fn layout(cols: &[Col], left: f32, w: f32) -> Vec<(Col, f32, f32)> {
    let name_w = w - 56.0 - cols.iter().map(|c| c.width()).sum::<f32>();
    let mut x = left + 56.0;
    cols.iter()
        .map(|&c| {
            let cw = if c == Col::Name { name_w } else { c.width() };
            let out = (c, x, cw);
            x += cw;
            out
        })
        .collect()
}

impl GamutApp {
    pub(super) fn list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        self.bulk_bar(ui, actions);
        let avail = ui.available_width() - 8.0;
        let cols = layout(&columns(avail), ui.cursor().left(), avail);
        // Only Name fits (the narrow list beside the details): a second line with last played and time.
        let compact = cols.len() == 1;
        self.grid_cols = 1;
        self.grid_pitch = ROW_H;
        self.grid_page = (((ui.available_height() - HEAD_H) / ROW_H).floor() as usize).max(1);

        // Header: click a column to sort by it.
        let (head, _) = ui.allocate_exact_size(vec2(avail, HEAD_H), Sense::hover());
        for &(c, x, w) in &cols {
            let r = Rect::from_min_size(pos2(x, head.top()), vec2(w, HEAD_H));
            let on = c.sort() == Some(self.sort);
            let label = if on { format!("{}  ↓", c.label()) } else { c.label().into() };
            let mut color = if on { TEXT } else { TEXT_DIM };
            if let Some(s) = c.sort() {
                let resp = ui.interact(r, ui.scope_id().with(("head", c.label())), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.hovered() {
                    color = TEXT;
                }
                if resp.clicked() {
                    self.sort = s;
                }
            }
            ui.painter().text(pos2(x + 8.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.5), color);
        }
        ui.painter().hline(head.x_range(), head.bottom() - 0.5, Stroke::new(1.0, CARD_HI));

        if self.view.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("No games here. Try Refresh, or + Add game.").color(TEXT_DIM));
            });
            return;
        }
        // Moving between the full width and the side panel starts a new scroll area: keep the open game in view.
        let side = self.selected.is_some();
        let key = egui::Id::unique("list_in_side");
        let was = ui.ctx().data_mut(|d| std::mem::replace(d.get_temp_mut_or(key, !side), side));
        if was != side {
            if let Some(i) = self.selected.or(self.cursor).and_then(|id| self.view.iter().position(|&gi| self.games[gi].id == id)) {
                self.scroll_to = Some((i as f32 * ROW_H - ui.available_height() / 3.0).max(0.0));
            }
        }
        let running = self.running();
        let (games, view, thumbs, multi) = (&self.games, &self.view, &mut self.thumbs, &self.multi);
        let (colls, active) = (&self.collections, collections::active(&self.collections, self.collection));
        let cursor = self.cursor.filter(|_| self.kb_focus);
        let mut clicks: Vec<(i64, egui::Modifiers, bool)> = Vec::new();
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut area = ScrollArea::vertical().auto_shrink([false; 2]);
        if let Some(y) = self.scroll_to.take() {
            area = area.vertical_scroll_offset(y);
        }
        let out = area.show_rows(ui, ROW_H, view.len(), |ui, range| {
            for i in range {
                let g = &games[view[i]];
                let (row, resp) = ui.allocate_exact_size(vec2(avail, ROW_H), Sense::click());
                let hovered = resp.hovered();
                let (in_multi, is_cursor) = (multi.contains(&g.id), cursor == Some(g.id));
                let p = ui.painter();
                let r = CornerRadius::same(6);
                if in_multi {
                    p.rect_filled(row, r, ACCENT.gamma_multiply(0.18));
                } else if hovered || is_cursor {
                    p.rect_filled(row, r, CARD_HI);
                } else if i % 2 == 1 {
                    p.rect_filled(row, r, CARD.gamma_multiply(0.45));
                }
                if is_cursor {
                    p.rect_stroke(row.shrink(1.0), r, Stroke::new(2.0, Color32::WHITE.gamma_multiply(0.8)), StrokeKind::Inside);
                }
                // Small cover, cropped to the middle of the 2:3 art.
                let art = Rect::from_min_size(row.min + vec2(14.0, 4.0), vec2(ROW_H - 8.0, ROW_H - 8.0));
                match thumbs.get(g.id, g.has_cover) {
                    Some(t) => {
                        egui::Image::from_texture(t).uv(Rect::from_min_max(pos2(0.0, 1.0 / 6.0), pos2(1.0, 5.0 / 6.0))).corner_radius(CornerRadius::same(4)).paint_at(ui, art);
                    }
                    None => {
                        p.rect_filled(art, CornerRadius::same(4), CARD);
                    }
                }
                if in_multi {
                    paint_check(p, art.center());
                }
                for &(c, x, w) in &cols {
                    let mut left = x + 8.0;
                    let right = x + w - 8.0;
                    if c == Col::Name {
                        if g.favorite {
                            paint_star(p, pos2(left + 6.0, row.center().y), 6.5, true);
                            left += 18.0;
                        }
                        if running.contains(&g.id) {
                            badge(p, pos2(left, row.center().y - 9.0), "RUNNING", GOOD);
                            left += 70.0;
                        }
                    }
                    let (size, color) = if c == Col::Name { (14.0, if in_multi || is_cursor { Color32::WHITE } else { TEXT }) } else { (12.5, TEXT_DIM) };
                    let mut job = egui::text::LayoutJob::simple_singleline(c.text(g), FontId::proportional(size), color);
                    job.wrap = egui::text::TextWrapping::truncate_at_width((right - left).max(10.0));
                    let galley = p.layout_job(job);
                    let y = row.center().y - if compact { 8.0 } else { 0.0 };
                    p.galley(pos2(left, y - galley.size().y / 2.0), galley, color);
                    if compact {
                        let sub = [Col::LastPlayed, Col::Playtime].map(|c| c.text(g)).into_iter().filter(|t| !t.is_empty()).collect::<Vec<_>>().join("  ·  ");
                        let mut job = egui::text::LayoutJob::simple_singleline(sub, FontId::proportional(11.5), TEXT_DIM);
                        job.wrap = egui::text::TextWrapping::truncate_at_width((right - x - 8.0).max(10.0));
                        let galley = p.layout_job(job);
                        p.galley(pos2(x + 8.0, row.center().y + 10.0 - galley.size().y / 2.0), galley, TEXT_DIM);
                    }
                }
                if hovered {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
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
        });
        self.grid_scroll = (out.state.clamped_offset().y, out.inner_rect.height());
        if theme::to_top_button(ui, out.inner_rect, out.state.clamped_offset().y) {
            self.scroll_to = Some(0.0);
        }
        self.apply_clicks(ui, clicks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_drop_on_narrow_windows() {
        assert_eq!(columns(1400.0).len(), 7);
        let narrow = columns(700.0);
        assert!(narrow.contains(&Col::Name) && narrow.contains(&Col::Playtime) && !narrow.contains(&Col::Added));
        let cols = layout(&columns(1400.0), 0.0, 1400.0);
        let end = cols.last().map(|(_, x, w)| x + w).unwrap();
        assert!((end - 1400.0).abs() < 0.5);
    }
}
