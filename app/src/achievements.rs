//! Achievements section on the details page, drawn from the library's cache.
use crate::theme::{self, ACCENT, CARD, GOOD, TEXT, TEXT_DIM};
use egui::{vec2, Color32, CornerRadius, RichText, Sense, Ui};
use gamut_core::achievements::{Achievement, State, PRIVATE};
use gamut_core::Library;

/// Cached achievements of the game on screen.
pub struct GameAch {
    pub id: i64,
    pub list: Vec<Achievement>,
    pub state: Option<State>,
    pub show_all: bool,
}

impl GameAch {
    pub fn load(lib: &Library, id: i64) -> Self {
        GameAch {
            id,
            list: lib.achievements(id).unwrap_or_default(),
            state: lib.achievement_state(id).ok().flatten(),
            show_all: false,
        }
    }

    /// Never fetched, or last fetched over 6 hours ago.
    pub fn stale(&self) -> bool {
        self.state.as_ref().is_none_or(|s| s.fetched < gamut_core::now() - 6 * 3600)
    }

    pub fn unlocked(&self) -> usize {
        self.list.iter().filter(|a| a.unlocked > 0).count()
    }
}

const SHOWN: usize = 10;
const RARE: f32 = 10.0;
const GOLD: Color32 = Color32::from_rgb(232, 186, 82);

/// "12 / 50" for the header tile; None when there's nothing to show.
pub fn tile(a: &GameAch) -> Option<String> {
    (!a.list.is_empty()).then(|| format!("{} / {}", a.unlocked(), a.list.len()))
}

pub fn section(ui: &mut Ui, a: &mut GameAch, busy: bool, has_key: bool) {
    theme::section(ui, "Achievements");
    let note = |ui: &mut Ui, t: &str| {
        ui.label(RichText::new(t).color(TEXT_DIM));
    };
    if a.list.is_empty() {
        let err = a.state.as_ref().and_then(|s| s.error.as_deref());
        match () {
            _ if !has_key => note(ui, "Add a Steam Web API key in Settings to see achievements."),
            _ if busy || a.state.is_none() => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(14.0));
                    note(ui, "Loading achievements…");
                });
            }
            _ if err == Some(PRIVATE) => {
                note(ui, "Steam didn't share achievements for this game. Either this account doesn't own it, or your profile's Game details aren't Public.")
            }
            _ if err.is_some() => note(ui, "Couldn't load achievements."),
            _ => note(ui, "This game has no achievements."),
        }
        return;
    }

    let (done, total) = (a.unlocked(), a.list.len());
    let pct = done as f32 / total as f32;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{done} of {total} unlocked")).size(15.0).color(TEXT));
        ui.label(RichText::new(format!("{:.0}%", pct * 100.0)).size(15.0).color(if done == total { GOLD } else { TEXT_DIM }));
        if busy {
            ui.add(egui::Spinner::new().size(13.0));
        }
    });
    ui.add_space(6.0);
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().min(520.0), 6.0), Sense::hover());
    ui.painter().rect_filled(bar, 3.0, CARD);
    let mut fill = bar;
    fill.set_width(bar.width() * pct);
    ui.painter().rect_filled(fill, 3.0, if done == total { GOLD } else { ACCENT });
    ui.add_space(14.0);

    // Newest unlocks first, then locked ones from most to least common.
    let mut order: Vec<&Achievement> = a.list.iter().collect();
    order.sort_by(|x, y| match (x.unlocked > 0, y.unlocked > 0) {
        (true, true) => y.unlocked.cmp(&x.unlocked),
        (false, false) => y.percent.unwrap_or(0.0).total_cmp(&x.percent.unwrap_or(0.0)),
        (u, _) => if u { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater },
    });
    let n = if a.show_all { order.len() } else { order.len().min(SHOWN) };
    let w = ui.available_width();
    let cols = if w > 760.0 { 2 } else { 1 };
    let gap = 12.0;
    let cell_w = (w - gap * (cols - 1) as f32) / cols as f32;
    for row in order[..n].chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for x in row {
                card(ui, x, cell_w);
            }
        });
        ui.add_space(8.0);
    }
    if order.len() > SHOWN {
        let label = if a.show_all { "Show fewer".to_string() } else { format!("Show all {total}") };
        if ui.button(label).clicked() {
            a.show_all = !a.show_all;
        }
    }
}

fn card(ui: &mut Ui, a: &Achievement, w: f32) {
    let open = a.unlocked > 0;
    let veiled = a.hidden && !open;
    let r = egui::Frame::new()
        .fill(CARD)
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_width(w - 16.0);
            ui.horizontal_top(|ui| {
                let icon = if open || a.icon_locked.is_empty() { &a.icon } else { &a.icon_locked };
                let size = vec2(48.0, 48.0);
                if icon.starts_with("http") {
                    let img = egui::Image::new(icon.as_str()).corner_radius(4).fit_to_exact_size(size);
                    ui.add(if open || !a.icon_locked.is_empty() { img } else { img.tint(Color32::from_gray(90)) });
                } else {
                    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                    ui.painter().rect_filled(r, 4.0, theme::CARD_HI);
                }
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let name = if veiled { "Hidden achievement" } else { &a.name };
                    let color = if open { TEXT } else { TEXT_DIM };
                    ui.add(egui::Label::new(RichText::new(name).size(14.0).color(color)).truncate());
                    let desc = if veiled { "Hover to reveal" } else { &a.description };
                    if !desc.is_empty() {
                        ui.add(egui::Label::new(RichText::new(desc).size(12.5).color(TEXT_DIM)).wrap());
                    }
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        if open {
                            ui.label(RichText::new(crate::util::date(a.unlocked)).size(11.5).color(GOOD));
                        }
                        if let Some(p) = a.percent {
                            let rare = p < RARE;
                            let t = format!("{}{p:.1}% of players", if rare { "Rare · " } else { "" });
                            ui.label(RichText::new(t).size(11.5).color(if rare { GOLD } else { TEXT_DIM }));
                        }
                    });
                });
            });
        });
    if veiled {
        let text = if a.description.is_empty() { a.name.clone() } else { format!("{}\n{}", a.name, a.description) };
        r.response.on_hover_text(text);
    }
}
