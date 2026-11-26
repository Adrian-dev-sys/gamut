//! Metadata lookups for the editor: Steam, GOG, IGDB and VNDB searches, and the "Compare all" view.
use crate::theme::{self, ACCENT, CARD_HI, TEXT, TEXT_DIM};
use crate::util;
use crossbeam_channel::{unbounded, Receiver};
use egui::{CornerRadius, RichText, Stroke, Ui};
use gamut_core::gog::{self, GogGame};
use gamut_core::igdb::{self, IgdbGame};
use gamut_core::model::Metadata;
use gamut_core::vndb::{self, Vn, VnDetails};
use gamut_core::{scrape, Config};

#[derive(Clone, Copy, PartialEq)]
pub enum Src {
    Steam,
    Gog,
    Igdb,
    Vndb,
}

impl Src {
    pub const ALL: [Src; 4] = [Src::Steam, Src::Gog, Src::Igdb, Src::Vndb];

    pub fn name(self) -> &'static str {
        match self {
            Src::Steam => "Steam",
            Src::Gog => "GOG",
            Src::Igdb => "IGDB",
            Src::Vndb => "VNDB",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Src::Steam => "Steam store page (works for games you don't own there)",
            Src::Gog => "GOG store page",
            Src::Igdb => "IGDB, the broadest database incl. consoles",
            Src::Vndb => "VNDB, for visual novels",
        }
    }
}

#[derive(Clone)]
pub enum Hit {
    Steam(u32, String),
    Gog(GogGame),
    Igdb(IgdbGame),
    Vndb(Vn),
}

impl Hit {
    pub fn title(&self) -> &str {
        match self {
            Hit::Steam(_, n) => n,
            Hit::Gog(g) => &g.title,
            Hit::Igdb(g) => &g.name,
            Hit::Vndb(v) => &v.title,
        }
    }

    /// "Name (year) · platforms/edition".
    pub fn label(&self) -> String {
        let (year, extra) = match self {
            Hit::Steam(..) => (None, String::new()),
            Hit::Gog(g) => (g.year, if g.kind == "pack" { "Edition/bundle".into() } else { String::new() }),
            Hit::Igdb(g) => (g.year, g.platforms.join(", ")),
            Hit::Vndb(v) => (v.year(), v.platforms.join(", ").to_uppercase()),
        };
        let year = year.map(|y| format!(" ({y})")).unwrap_or_default();
        format!("{}{year}{}{extra}", self.title(), if extra.is_empty() { "" } else { " · " })
    }
}

pub fn search(src: Src, cfg: Option<&Config>, q: &str) -> anyhow::Result<Vec<Hit>> {
    Ok(match src {
        Src::Steam => scrape::steam_find(q)?.into_iter().map(|(id, n)| Hit::Steam(id, n)).collect(),
        Src::Gog => gog::search(q)?.into_iter().map(Hit::Gog).collect(),
        Src::Vndb => vndb::search(q)?.into_iter().map(Hit::Vndb).collect(),
        Src::Igdb => match cfg {
            Some(c) => igdb::search(c, q)?.into_iter().map(Hit::Igdb).collect(),
            None => anyhow::bail!("IGDB isn't set up"),
        },
    })
}

/// Full details of a picked entry (IGDB hits already carry theirs).
pub fn fetch_details(h: Hit) -> anyhow::Result<VnDetails> {
    match h {
        Hit::Vndb(v) => vndb::details(&v.id),
        Hit::Steam(id, title) => {
            let d = scrape::steam_details(id)?.ok_or_else(|| anyhow::anyhow!("Steam has no store page for {title}"))?;
            Ok(VnDetails { title, meta: d.meta, tags: Vec::new() })
        }
        Hit::Gog(g) => {
            let mut meta = g.meta;
            meta.description = gog::description(&g.id)?;
            Ok(VnDetails { title: g.title, meta, tags: g.tags })
        }
        Hit::Igdb(g) => Ok(VnDetails { title: g.name, meta: g.meta, tags: Vec::new() }),
    }
}

/// The likeliest result for `name`: IGDB's strict match, else an exact title (games before bundles), else the first.
fn auto_pick(hits: &[Hit], name: &str, platform: Option<&str>, year: Option<i32>) -> usize {
    let igdb: Vec<IgdbGame> = hits.iter().filter_map(|h| if let Hit::Igdb(g) = h { Some(g.clone()) } else { None }).collect();
    if let Some(b) = igdb::best(name, platform, year, &igdb) {
        if let Some(i) = hits.iter().position(|h| matches!(h, Hit::Igdb(g) if g.id == b.id)) {
            return i;
        }
    }
    let want = scrape::normalize(&igdb::search_name(name));
    let exact = |h: &Hit| scrape::normalize(h.title()) == want;
    let pack = |h: &Hit| matches!(h, Hit::Gog(g) if g.kind == "pack");
    hits.iter().position(|h| exact(h) && !pack(h)).or_else(|| hits.iter().position(exact)).unwrap_or(0)
}

pub const FIELDS: [&str; 9] =
    ["Description", "Developer", "Publisher", "Release date", "Genres", "Series", "Critic score", "Community score", "Tags"];

/// A field's value for display (index into `FIELDS`).
fn field(d: &VnDetails, i: usize) -> Option<String> {
    let m = &d.meta;
    let s = |v: &Option<String>| v.clone().filter(|v| !v.trim().is_empty());
    match i {
        0 => s(&m.description).filter(|d| !scrape::text_only(d).is_empty()),
        1 => s(&m.developer),
        2 => s(&m.publisher),
        3 => m.release_date.as_deref().map(util::release),
        4 => s(&m.genres),
        5 => s(&m.series),
        6 => m.critic_score.map(|v| v.to_string()),
        7 => m.community_score.map(|v| v.to_string()),
        _ => (!d.tags.is_empty()).then(|| d.tags.join(", ")),
    }
}

/// Number of fields an entry has.
pub fn count(d: &VnDetails) -> usize {
    (0..FIELDS.len()).filter(|&i| field(d, i).is_some()).count()
}

fn words(d: &VnDetails) -> usize {
    d.meta.description.as_deref().map_or(0, |t| scrape::text_only(t).split_whitespace().count())
}

enum ColMsg {
    Results(Vec<Hit>, usize),
    Details(usize, Result<VnDetails, String>),
    Failed(String),
}

struct Col {
    src: Src,
    hits: Vec<Hit>,
    pick: usize,
    details: Option<VnDetails>,
    busy: bool,
    error: Option<String>,
    rx: Option<Receiver<ColMsg>>,
}

/// What "Use selected" hands the editor.
pub struct Picked {
    pub meta: Metadata,
    pub tags: Vec<String>,
    pub igdb_id: Option<i64>,
    pub label: String,
}

pub enum CompareOut {
    Close,
    Apply(Picked),
}

/// All sources side by side; each field row picks which source it comes from.
pub struct Compare {
    query: String,
    name: String,
    platform: Option<String>,
    year: Option<i32>,
    cols: Vec<Col>,
    /// Chosen column per field; follows the suggestion until the user clicks a cell.
    choice: [Option<usize>; 9],
    touched: bool,
}

impl Compare {
    pub fn new(ctx: &egui::Context, cfg: Option<&Config>, name: &str, platform: &str, year: Option<i32>) -> Self {
        let cols = Src::ALL
            .iter()
            .map(|&src| Col { src, hits: Vec::new(), pick: 0, details: None, busy: false, error: None, rx: None })
            .collect();
        let platform = Some(platform.trim().to_string()).filter(|p| !p.is_empty());
        let mut c = Compare { query: igdb::search_name(name), name: name.to_string(), platform, year, cols, choice: [None; 9], touched: false };
        c.search_all(ctx, cfg);
        c
    }

    fn search_all(&mut self, ctx: &egui::Context, cfg: Option<&Config>) {
        self.choice = [None; 9];
        self.touched = false;
        for i in 0..self.cols.len() {
            let col = &mut self.cols[i];
            (col.hits, col.pick, col.details, col.error) = (Vec::new(), 0, None, None);
            if col.src == Src::Igdb && cfg.is_none() {
                col.error = Some("Not set up (Settings → IGDB)".into());
                continue;
            }
            let (tx, rx) = unbounded();
            (col.busy, col.rx) = (true, Some(rx));
            let (src, cfg, q, ctx) = (col.src, cfg.cloned(), self.query.trim().to_string(), ctx.clone());
            let (name, platform, year) = (if q.is_empty() { self.name.clone() } else { q.clone() }, self.platform.clone(), self.year);
            std::thread::spawn(move || {
                let send = |m| {
                    let _ = tx.send(m);
                    ctx.request_repaint();
                };
                match search(src, cfg.as_ref(), &q) {
                    Err(e) => send(ColMsg::Failed(format!("{e:#}"))),
                    Ok(hits) if hits.is_empty() => send(ColMsg::Results(hits, 0)),
                    Ok(hits) => {
                        let pick = auto_pick(&hits, &name, platform.as_deref(), year);
                        let h = hits[pick].clone();
                        send(ColMsg::Results(hits, pick));
                        send(ColMsg::Details(pick, fetch_details(h).map_err(|e| format!("{e:#}"))));
                    }
                }
            });
        }
    }

    fn fetch(&mut self, ctx: &egui::Context, i: usize) {
        let col = &mut self.cols[i];
        let (tx, rx) = unbounded();
        (col.busy, col.rx, col.details, col.error) = (true, Some(rx), None, None);
        let (pick, h, ctx) = (col.pick, col.hits[col.pick].clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(ColMsg::Details(pick, fetch_details(h).map_err(|e| format!("{e:#}"))));
            ctx.request_repaint();
        });
    }

    /// Column with the most fields; a longer description breaks ties.
    fn suggested(&self) -> Option<usize> {
        let loaded = self.cols.iter().enumerate().filter_map(|(i, c)| c.details.as_ref().map(|d| (i, d)));
        loaded.filter(|(_, d)| count(d) > 0).max_by_key(|(i, d)| (count(d), words(d), std::cmp::Reverse(*i))).map(|(i, _)| i)
    }

    /// Default choices: the suggested source where it has the field, else the first source that does.
    fn auto_choose(&mut self) {
        let best = self.suggested();
        for f in 0..FIELDS.len() {
            let has = |i: usize| self.cols[i].details.as_ref().is_some_and(|d| field(d, f).is_some());
            self.choice[f] = best.filter(|&b| has(b)).or_else(|| (0..self.cols.len()).find(|&i| has(i)));
        }
    }

    pub fn show(&mut self, ui: &mut Ui, cfg: Option<&Config>) -> Option<CompareOut> {
        for c in &mut self.cols {
            while let Some(m) = c.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
                match m {
                    ColMsg::Results(h, pick) => {
                        c.busy = !h.is_empty();
                        (c.hits, c.pick) = (h, pick);
                    }
                    ColMsg::Details(pick, r) if pick == c.pick => {
                        c.busy = false;
                        match r {
                            Ok(d) => c.details = Some(d),
                            Err(e) => c.error = Some(e),
                        }
                    }
                    ColMsg::Details(..) => {}
                    ColMsg::Failed(e) => (c.busy, c.error) = (false, Some(e)),
                }
            }
        }
        if !self.touched {
            self.auto_choose();
        }
        let ctx = ui.ctx().clone();
        let mut out = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Compare all").strong().size(15.0));
            let r = ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(300.0).hint_text("Game name"));
            if ui.button("Search").clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) {
                self.search_all(&ctx, cfg);
            }
            if self.cols.iter().any(|c| c.busy) {
                ui.add(egui::Spinner::new().size(14.0));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    out = Some(CompareOut::Close);
                }
            });
        });
        ui.label(
            RichText::new("Each source's closest match is shown. Click a cell to take that field from that source; ★ marks the most complete one.")
                .color(TEXT_DIM),
        );
        ui.add_space(6.0);

        let best = self.suggested();
        let label_w = 110.0;
        let col_w = ((ui.available_width() - label_w - 20.0) / self.cols.len() as f32 - 12.0).max(120.0);
        let mut refetch = None;
        let mut take_all = None;
        egui::Grid::new("cmp_grid").num_columns(self.cols.len() + 1).spacing([12.0, 6.0]).min_col_width(label_w).show(ui, |ui| {
            // Header: source, its picked entry, field count.
            ui.label("");
            for (i, c) in self.cols.iter_mut().enumerate() {
                ui.vertical(|ui| {
                    ui.set_width(col_w);
                    let star = if best == Some(i) { "★ " } else { "" };
                    let head = match &c.details {
                        Some(d) => format!("{star}{}  ·  {} of 9", c.src.name(), count(d)),
                        None => c.src.name().to_string(),
                    };
                    ui.label(RichText::new(head).strong().color(if best == Some(i) { ACCENT } else { TEXT }))
                        .on_hover_text(if best == Some(i) { "Suggested: the most complete entry" } else { c.src.hint() });
                    if !c.hits.is_empty() {
                        let before = c.pick;
                        let text: String = c.hits[c.pick].label().chars().take(40).collect();
                        egui::ComboBox::from_id_salt(("cmp_pick", i)).width(col_w - 8.0).selected_text(text).show_ui(ui, |ui| {
                            for (j, h) in c.hits.iter().enumerate() {
                                ui.selectable_value(&mut c.pick, j, h.label());
                            }
                        });
                        if c.pick != before {
                            refetch = Some(i);
                        }
                    }
                    if c.busy {
                        ui.add(egui::Spinner::new().size(12.0));
                    } else if let Some(e) = &c.error {
                        ui.label(RichText::new(e).color(theme::DANGER).size(12.0));
                    } else if c.hits.is_empty() {
                        ui.label(RichText::new("Nothing found").color(TEXT_DIM).size(12.0));
                    } else if c.details.is_some() && ui.small_button("Use all from here").clicked() {
                        take_all = Some(i);
                    }
                });
            }
            ui.end_row();

            for f in 0..FIELDS.len() {
                ui.label(RichText::new(FIELDS[f]).color(TEXT_DIM));
                for i in 0..self.cols.len() {
                    let v = self.cols[i].details.as_ref().and_then(|d| field(d, f));
                    let Some(v) = v else {
                        ui.label(RichText::new("—").color(TEXT_DIM));
                        continue;
                    };
                    let chosen = self.choice[f] == Some(i);
                    let (shown, hover) = if f == 0 {
                        let d = self.cols[i].details.as_ref().unwrap();
                        let text = scrape::text_only(&v);
                        let pics = v.lines().filter(|l| l.trim_start().starts_with(scrape::IMG_MARKER)).count();
                        let snip: String = text.chars().take(160).collect();
                        let more = if text.chars().count() > 160 { "…" } else { "" };
                        let pics = if pics > 0 { format!(", {pics} images") } else { String::new() };
                        (format!("{snip}{more}\n{} words{pics}", words(d)), Some(text.chars().take(1500).collect::<String>()))
                    } else {
                        (v, None)
                    };
                    let frame = egui::Frame::new()
                        .fill(if chosen { CARD_HI } else { egui::Color32::TRANSPARENT })
                        .stroke(if chosen { Stroke::new(1.5, ACCENT) } else { Stroke::new(1.0, CARD_HI) })
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::same(6))
                        .show(ui, |ui| {
                            ui.set_width(col_w - 14.0);
                            ui.add(egui::Label::new(RichText::new(&shown).color(if chosen { TEXT } else { TEXT_DIM })).wrap().selectable(false));
                        });
                    let r = ui.interact(frame.response.rect, ui.scope_id().with(("cmp_cell", f, i)), egui::Sense::click());
                    let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
                    let r = match hover {
                        Some(h) => r.on_hover_ui(|ui| {
                            ui.set_max_width(480.0);
                            ui.label(h);
                        }),
                        None => r,
                    };
                    if r.clicked() {
                        self.touched = true;
                        // Clicking the chosen cell again leaves the field as it is in the form.
                        self.choice[f] = if chosen { None } else { Some(i) };
                    }
                }
                ui.end_row();
            }
        });
        if let Some(i) = take_all {
            self.touched = true;
            for f in 0..FIELDS.len() {
                if self.cols[i].details.as_ref().is_some_and(|d| field(d, f).is_some()) {
                    self.choice[f] = Some(i);
                }
            }
        }
        if let Some(i) = refetch {
            self.fetch(&ctx, i);
            self.touched = false;
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let n = self.choice.iter().filter(|c| c.is_some()).count();
            let apply = egui::Button::new(RichText::new(format!("Fill {n} fields")).color(egui::Color32::WHITE)).fill(ACCENT);
            if ui.add_enabled(n > 0, apply).on_hover_text("Fill the form with the selected cells (nothing is saved until Save)").clicked() {
                out = Some(CompareOut::Apply(self.picked()));
            }
            ui.label(RichText::new("Unselected fields keep what the form has.").color(TEXT_DIM));
        });
        out
    }

    fn picked(&self) -> Picked {
        let d = |f: usize| self.choice[f].and_then(|i| self.cols[i].details.as_ref());
        let m = |f: usize| d(f).map(|d| &d.meta);
        let meta = Metadata {
            description: m(0).and_then(|m| m.description.clone()),
            developer: m(1).and_then(|m| m.developer.clone()),
            publisher: m(2).and_then(|m| m.publisher.clone()),
            release_date: m(3).and_then(|m| m.release_date.clone()),
            genres: m(4).and_then(|m| m.genres.clone()),
            series: m(5).and_then(|m| m.series.clone()),
            critic_score: m(6).and_then(|m| m.critic_score),
            community_score: m(7).and_then(|m| m.community_score),
        };
        let mut used: Vec<usize> = self.choice.iter().flatten().copied().collect();
        used.sort_unstable();
        used.dedup();
        // Any field taken from IGDB makes that entry the game's match (for covers).
        let igdb_id = used.iter().find_map(|&i| match (&self.cols[i].src, self.cols[i].hits.get(self.cols[i].pick)) {
            (Src::Igdb, Some(Hit::Igdb(g))) => Some(g.id),
            _ => None,
        });
        let label = used.iter().map(|&i| self.cols[i].src.name()).collect::<Vec<_>>().join(" + ");
        Picked { meta, tags: d(8).map(|d| d.tags.clone()).unwrap_or_default(), igdb_id, label }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_pick_prefers_exact_title() {
        let h = |n: &str| Hit::Steam(1, n.into());
        let hits = [h("Hollow Knight: Silksong"), h("Hollow Knight"), h("Hollow Knight - Soundtrack")];
        assert_eq!(auto_pick(&hits, "Hollow Knight (USA)", None, None), 1);
        assert_eq!(auto_pick(&hits, "Something else", None, None), 0);
    }
}
