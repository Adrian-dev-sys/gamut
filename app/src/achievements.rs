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

    /// Never fetched, last fetch failed, or it's over 6 hours old.
    pub fn stale(&self) -> bool {
        self.state.as_ref().is_none_or(|s| s.error.is_some() || s.fetched < gamut_core::now() - 6 * 3600)
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

/// Where a game's achievements come from, as far as the details page needs to know.
#[derive(Clone, Copy, PartialEq)]
pub enum Source {
    Steam { has_key: bool },
    /// Steam emulator files on disk (GSE, CODEX, RUNE…), names from Steam.
    Local { has_key: bool },
    /// RetroAchievements game id as stored on the game (see `retro::NOT_FOUND` / `UNSUPPORTED`).
    Retro { ra_id: Option<i64> },
}

impl Source {
    /// Which source a game uses; None when it has none (or RA isn't set up).
    pub fn of(g: &gamut_core::Game, cfg: &gamut_core::Config, local: bool) -> Option<Source> {
        use gamut_core::{retro, Source as S};
        if local {
            return Some(Source::Local { has_key: !cfg.steam_api_key.trim().is_empty() });
        }
        match g.source {
            S::Steam if g.steam_appid().is_some() => Some(Source::Steam { has_key: !cfg.steam_api_key.trim().is_empty() }),
            S::Custom if !cfg.ra_user.trim().is_empty() && !cfg.ra_api_key.trim().is_empty() && g.platform.is_some() => {
                (g.ra_id != Some(retro::UNSUPPORTED)).then_some(Source::Retro { ra_id: g.ra_id })
            }
            _ => None,
        }
    }
}

/// Something the section asks the page to do.
pub enum Ask {
    PickRetroGame,
}

pub fn section(ui: &mut Ui, a: &mut GameAch, src: Source, busy: bool) -> Option<Ask> {
    theme::section(ui, "Achievements");
    let note = |ui: &mut Ui, t: &str| {
        ui.add(egui::Label::new(RichText::new(t).color(TEXT_DIM)).wrap());
    };
    let spinner = |ui: &mut Ui, t: &str| {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(14.0));
            ui.label(RichText::new(t).color(TEXT_DIM));
        });
    };
    let mut ask = None;
    if let Source::Retro { ra_id: Some(r) } = src {
        if r <= 0 || !a.list.is_empty() {
            ui.horizontal(|ui| {
                if r > 0 {
                    ui.hyperlink_to(RichText::new("RetroAchievements").size(12.5), format!("https://retroachievements.org/game/{r}"));
                } else {
                    note(ui, "Not found on RetroAchievements.");
                }
                let label = if r > 0 { "Wrong game?" } else { "Pick the game…" };
                if ui.button(RichText::new(label).size(12.5)).clicked() {
                    ask = Some(Ask::PickRetroGame);
                }
            });
            ui.add_space(8.0);
        }
        if r <= 0 {
            return ask;
        }
    }
    if a.list.is_empty() {
        let err = a.state.as_ref().and_then(|s| s.error.as_deref());
        let lookup = "Looking this game up on RetroAchievements…";
        match (src, err) {
            (Source::Steam { has_key: false } | Source::Local { has_key: false }, _) => note(ui, "Add a Steam Web API key in Settings to see achievements."),
            (Source::Retro { ra_id: None }, _) if busy => spinner(ui, lookup),
            _ if busy => spinner(ui, "Loading achievements…"),
            (_, Some(PRIVATE)) => note(
                ui,
                "Steam didn't share achievements for this game. Either this account doesn't own it, or your profile's Game details aren't Public.",
            ),
            (_, Some(e)) => note(ui, &format!("Couldn't load achievements: {e}")),
            (Source::Retro { ra_id: None }, _) => spinner(ui, lookup),
            _ if a.state.is_none() => spinner(ui, "Loading achievements…"),
            _ => note(ui, "This game has no achievements."),
        }
        return ask;
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
    ask
}

/// Mipmapped filtering so badges scaled to a non-native size stay smooth.
pub const ICON_TEX: egui::TextureOptions =
    egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));

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
                let size = vec2(64.0, 64.0);
                if icon.starts_with("http") {
                    let img = egui::Image::new(icon.as_str())
                        .corner_radius(6)
                        .texture_options(ICON_TEX)
                        .fit_to_exact_size(size);
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
                        if a.points > 0 {
                            ui.label(RichText::new(format!("{} pts", a.points)).size(11.5).color(TEXT_DIM));
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

/// Dialog to choose a game's RetroAchievements entry by hand, from its system's game list.
pub struct RetroPick {
    pub game_id: i64,
    name: String,
    query: String,
    list: Option<Result<Vec<(i64, String)>, String>>,
    rx: crossbeam_channel::Receiver<Result<Vec<(i64, String)>, String>>,
}

pub enum PickOutcome {
    Open,
    Closed,
    Picked(i64, i64),
}

impl RetroPick {
    pub fn open(g: &gamut_core::Game, cfg: &gamut_core::Config, cache: std::path::PathBuf, ctx: &egui::Context) -> Self {
        use gamut_core::retro;
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (key, platform, ctx) = (cfg.ra_api_key.trim().to_string(), g.platform.clone().unwrap_or_default(), ctx.clone());
        std::thread::spawn(move || {
            let r = retro::consoles(&key, &cache).and_then(|c| {
                let id = retro::console_for(&platform, &c)
                    .ok_or_else(|| anyhow::anyhow!("RetroAchievements doesn't support {platform}"))?;
                retro::games(&key, id, &cache)
            });
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
        RetroPick { game_id: g.id, name: g.name.clone(), query: g.name.clone(), list: None, rx }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> PickOutcome {
        if let Ok(r) = self.rx.try_recv() {
            self.list = Some(r);
        }
        let (mut open, mut picked) = (true, None);
        theme::dialog(ctx, "Pick RetroAchievements game", true, |ui| {
            ui.set_width(560.0);
            crate::ui::dialog_header(ui, "Pick RetroAchievements game", &mut open);
            ui.label(RichText::new(format!("Which RetroAchievements game is \"{}\"?", self.name)).color(TEXT_DIM));
            ui.add_space(6.0);
            ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Search").desired_width(f32::INFINITY));
            ui.add_space(6.0);
            match &self.list {
                None => {
                    ui.add(egui::Spinner::new());
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(theme::DANGER));
                }
                Some(Ok(list)) => {
                    let words: Vec<String> = gamut_core::retro::title_key(&self.query).split(' ').map(str::to_string).collect();
                    let hits: Vec<&(i64, String)> = list
                        .iter()
                        .filter(|(_, t)| {
                            let k = gamut_core::retro::title_key(t);
                            words.iter().all(|w| k.contains(w.as_str()))
                        })
                        .collect();
                    egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show_rows(ui, 22.0, hits.len(), |ui, rows| {
                        for (id, title) in &hits[rows] {
                            if ui.selectable_label(false, title.as_str()).clicked() {
                                picked = Some(*id);
                            }
                        }
                    });
                    if hits.is_empty() {
                        ui.label(RichText::new("No games match.").color(TEXT_DIM));
                    }
                }
            }
        });
        match (picked, open) {
            (Some(ra), _) => PickOutcome::Picked(self.game_id, ra),
            (None, false) => PickOutcome::Closed,
            _ => PickOutcome::Open,
        }
    }
}
