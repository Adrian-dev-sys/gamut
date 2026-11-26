//! The Statistics tab: playtime over time, most played, platforms, genres, achievements per period.
use crate::theme::{self, ACCENT, CARD, CARD_HI, TEXT, TEXT_DIM};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Ui};
use gamut_core::{Game, Library, Source};
use std::collections::HashMap;

const DAY: i64 = 86_400;
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// Local day number (days since 1970-01-01 in local time).
fn day_of(t: i64, off: i64) -> i64 {
    (t + off).div_euclid(DAY)
}

/// Day number → (year, month 1-12, day).
fn ymd(days: i64) -> (i64, usize, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m as usize, d)
}

fn month_of(day: i64) -> i64 {
    let (y, m, _) = ymd(day);
    y * 12 + m as i64 - 1
}

fn hours(min: f32) -> String {
    match min as i64 {
        0 => "0h".into(),
        m if m < 60 => format!("{m}m"),
        m if m < 6000 => format!("{}h {:02}m", m / 60, m % 60),
        m => format!("{}h", m / 60),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Span {
    Days,
    Months,
    Years,
}

/// Everything the tab shows, computed once per refresh.
pub struct Stats {
    off: i64,
    today: i64,
    /// (game id, local day, minutes)
    sessions: Vec<(i64, i64, i64)>,
    /// (game id, local day)
    unlocks: Vec<(i64, i64)>,
    since: Option<i64>,
    play_span: Span,
    ach_span: Span,
    recent_top: bool,
    pub loaded: f64,
    pub dirty: bool,
}

impl Stats {
    pub fn load(lib: &Library, now: f64) -> Self {
        let off = gamut_core::utc_offset();
        let today = day_of(gamut_core::now(), off);
        let sessions = lib
            .sessions_since(0)
            .unwrap_or_default()
            .into_iter()
            .map(|(g, start, m)| (g, day_of(start, off), m))
            .collect();
        let unlocks = lib.unlock_times().unwrap_or_default().into_iter().map(|(g, t)| (g, day_of(t, off))).collect();
        Stats {
            off,
            today,
            sessions,
            unlocks,
            since: lib.stats_since().ok().flatten(),
            play_span: Span::Days,
            ach_span: Span::Months,
            recent_top: false,
            loaded: now,
            dirty: false,
        }
    }

    pub fn refresh(&mut self, lib: &Library, now: f64) {
        let (p, a, r) = (self.play_span, self.ach_span, self.recent_top);
        *self = Stats { play_span: p, ach_span: a, recent_top: r, ..Stats::load(lib, now) };
    }
}

/// One bar of a time chart: axis label, value, tooltip.
struct Bar {
    label: String,
    value: f32,
    tip: String,
}

/// Buckets (game id, day, amount) into the last 30 days, 12 months, or every year.
fn buckets(items: &[(i64, i64, f32)], span: Span, today: i64, names: &HashMap<i64, &Game>, fmt: &dyn Fn(f32) -> String) -> Vec<Bar> {
    let key = |day: i64| match span {
        Span::Days => day,
        Span::Months => month_of(day),
        Span::Years => ymd(day).0,
    };
    let keys: Vec<i64> = match span {
        Span::Days => (today - 29..=today).collect(),
        Span::Months => (month_of(today) - 11..=month_of(today)).collect(),
        Span::Years => {
            let first = items.iter().map(|i| ymd(i.1).0).min().unwrap_or(ymd(today).0).max(ymd(today).0 - 19);
            (first..=ymd(today).0).collect()
        }
    };
    let mut sum: HashMap<i64, f32> = HashMap::new();
    let mut per_game: HashMap<(i64, i64), f32> = HashMap::new();
    for &(g, day, v) in items {
        let k = key(day);
        *sum.entry(k).or_default() += v;
        *per_game.entry((k, g)).or_default() += v;
    }
    keys.into_iter()
        .map(|k| {
            let (label, long) = match span {
                Span::Days => {
                    let (y, m, d) = ymd(k);
                    (format!("{d}/{m}"), format!("{} {d:02}/{m:02}/{y}", WEEKDAYS[(k + 3).rem_euclid(7) as usize]))
                }
                Span::Months => (MONTHS[k.rem_euclid(12) as usize].to_string(), format!("{} {}", MONTHS[k.rem_euclid(12) as usize], k.div_euclid(12))),
                Span::Years => (k.to_string(), k.to_string()),
            };
            let v = sum.get(&k).copied().unwrap_or(0.0);
            let mut top: Vec<(i64, f32)> = per_game.iter().filter(|((kk, _), _)| *kk == k).map(|((_, g), v)| (*g, *v)).collect();
            top.sort_by(|a, b| b.1.total_cmp(&a.1));
            let mut tip = format!("{long}\n{}", fmt(v));
            for (g, gv) in top.iter().take(3) {
                if let Some(game) = names.get(g) {
                    tip.push_str(&format!("\n  {} · {}", game.name, fmt(*gv)));
                }
            }
            Bar { label, value: v, tip }
        })
        .collect()
}

/// Draws the tab; returns a game the user clicked.
pub fn show(ui: &mut Ui, st: &mut Stats, games: &[Game]) -> Option<i64> {
    let mut open = None;
    egui::ScrollArea::vertical().id_salt("stats_tab").auto_shrink([false, false]).show(ui, |ui| {
        crate::ach_tab::page(ui, |ui| body(ui, st, games, &mut open));
    });
    open
}

fn body(ui: &mut Ui, st: &mut Stats, games: &[Game], open: &mut Option<i64>) {
    let visible: Vec<&Game> = games.iter().filter(|g| !g.hidden).collect();
    let names: HashMap<i64, &Game> = visible.iter().map(|g| (g.id, *g)).collect();
    let total: i64 = visible.iter().map(|g| g.playtime_min).sum();
    let played = visible.iter().filter(|g| g.playtime_min > 0).count();
    let sessions: Vec<(i64, i64, f32)> =
        st.sessions.iter().filter(|s| names.contains_key(&s.0)).map(|&(g, d, m)| (g, d, m as f32)).collect();
    let this_month: f32 = sessions.iter().filter(|s| month_of(s.1) == month_of(st.today)).map(|s| s.2).sum();
    let days: std::collections::HashSet<i64> = sessions.iter().map(|s| s.1).collect();
    let streak = {
        // Today counts once played; until then the streak runs to yesterday.
        let mut d = if days.contains(&st.today) { st.today } else { st.today - 1 };
        let mut n = 0;
        while days.contains(&d) {
            n += 1;
            d -= 1;
        }
        n
    };

    ui.add_space(10.0);
    ui.horizontal_wrapped(|ui| {
        tile(ui, "TOTAL PLAYTIME", &hours(total as f32), "All games, including playtime from before Gamut");
        tile(ui, "GAMES PLAYED", &format!("{played} / {}", visible.len()), "Games with any playtime");
        tile(ui, "NEVER PLAYED", &(visible.len() - played).to_string(), "Your backlog");
        tile(ui, "THIS MONTH", &hours(this_month), "Playtime recorded this month");
        tile(ui, "PLAY STREAK", &format!("{streak} day{}", if streak == 1 { "" } else { "s" }), "Days in a row with recorded play");
    });
    if let Some(since) = st.since {
        let d = day_of(since, st.off);
        let (y, m, dd) = ymd(d);
        if st.today - d < 60 {
            ui.add_space(6.0);
            ui.label(theme::hint(&format!(
                "Daily playtime is recorded from {dd:02}/{m:02}/{y}: games Gamut starts are timed, Steam playtime is picked up from library scans."
            )));
        }
    }
    ui.add_space(18.0);

    // Playtime over time.
    span_header(ui, "Playtime", &mut st.play_span);
    let bars = buckets(&sessions, st.play_span, st.today, &names, &|v| hours(v));
    if sessions.is_empty() {
        empty(ui, "Nothing recorded yet. Play something and it shows up here.");
    } else {
        column_chart(ui, "play", &bars, &|v| hours(v));
    }
    ui.add_space(20.0);

    // Achievements over time (unlock dates go back years).
    let unlocks: Vec<(i64, i64, f32)> = st.unlocks.iter().filter(|u| names.contains_key(&u.0)).map(|&(g, d)| (g, d, 1.0)).collect();
    span_header(ui, "Achievements unlocked", &mut st.ach_span);
    if unlocks.is_empty() {
        empty(ui, "No achievements yet. Add a Steam or RetroAchievements key in Settings.");
    } else {
        let count = |v: f32| format!("{}", v as i64);
        column_chart(ui, "ach", &buckets(&unlocks, st.ach_span, st.today, &names, &count), &count);
        let best = unlocks.iter().fold(HashMap::<i64, i64>::new(), |mut m, u| {
            *m.entry(u.1).or_default() += 1;
            m
        });
        if let Some((d, n)) = best.iter().max_by_key(|(d, n)| (**n, **d)) {
            let (y, m, dd) = ymd(*d);
            ui.label(theme::hint(&format!("Best day: {n} unlocks on {dd:02}/{m:02}/{y}")));
        }
    }
    ui.add_space(24.0);

    // Two columns of rankings.
    let gap = 32.0;
    let col = (ui.available_width() - gap) / 2.0;
    let half = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        ui.allocate_ui_with_layout(vec2(col, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(col);
            add(ui);
        });
    };
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        half(ui, &mut |ui| {
            ui.horizontal(|ui| {
                theme::section(ui, "Most played");
                ui.add_space(12.0);
                ui.selectable_value(&mut st.recent_top, false, "All time");
                ui.selectable_value(&mut st.recent_top, true, "Last 30 days");
            });
            let mut rows: Vec<(String, f32, Option<i64>)> = if st.recent_top {
                let mut m: HashMap<i64, f32> = HashMap::new();
                for s in sessions.iter().filter(|s| s.1 > st.today - 30) {
                    *m.entry(s.0).or_default() += s.2;
                }
                m.into_iter().map(|(g, v)| (names[&g].name.clone(), v, Some(g))).collect()
            } else {
                visible.iter().filter(|g| g.playtime_min > 0).map(|g| (g.name.clone(), g.playtime_min as f32, Some(g.id))).collect()
            };
            rows.sort_by(|a, b| b.1.total_cmp(&a.1));
            rows.truncate(12);
            if rows.is_empty() {
                empty(ui, "Nothing played in the last 30 days.");
            }
            if let Some(g) = hbars(ui, &rows, &|v| hours(v)) {
                *open = Some(g);
            }
        });
        ui.add_space(gap);
        half(ui, &mut |ui| {
            theme::section(ui, "Playtime by platform");
            let mut m: HashMap<String, f32> = HashMap::new();
            for g in visible.iter().filter(|g| g.playtime_min > 0) {
                let p = match (g.source, g.platform.as_deref()) {
                    (Source::Steam, _) => "Steam".to_string(),
                    (_, Some(p)) => p.split(',').next().unwrap_or(p).trim().to_string(),
                    _ => "Other".to_string(),
                };
                *m.entry(p).or_default() += g.playtime_min as f32;
            }
            hbars(ui, &top(m, 10), &|v| hours(v));
        });
    });
    ui.add_space(24.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        half(ui, &mut |ui| {
            theme::section(ui, "Playtime by genre");
            let mut m: HashMap<String, f32> = HashMap::new();
            for g in visible.iter().filter(|g| g.playtime_min > 0) {
                for genre in g.genres.as_deref().unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    *m.entry(genre.to_string()).or_default() += g.playtime_min as f32;
                }
            }
            if m.is_empty() {
                empty(ui, "No genres yet; they come with game metadata.");
            }
            hbars(ui, &top(m, 10), &|v| hours(v));
        });
        ui.add_space(gap);
        half(ui, &mut |ui| {
            theme::section(ui, "Completion status");
            let mut m: HashMap<String, f32> = HashMap::new();
            for g in &visible {
                *m.entry(g.completion.clone().unwrap_or_else(|| "Not set".into())).or_default() += 1.0;
            }
            hbars(ui, &top(m, 10), &|v| format!("{} games", v as i64));
        });
    });
    ui.add_space(24.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        half(ui, &mut |ui| {
            theme::section(ui, "When you play");
            let mut by_day = [0.0f32; 7];
            for s in &sessions {
                by_day[(s.1 + 3).rem_euclid(7) as usize] += s.2;
            }
            if sessions.is_empty() {
                empty(ui, "Fills in as sessions are recorded.");
            } else {
                let bars: Vec<Bar> =
                    (0..7).map(|i| Bar { label: WEEKDAYS[i].into(), value: by_day[i], tip: format!("{}\n{}", WEEKDAYS[i], hours(by_day[i])) }).collect();
                column_chart(ui, "weekday", &bars, &|v| hours(v));
            }
        });
        ui.add_space(gap);
        half(ui, &mut |ui| {
            theme::section(ui, "Added to library");
            let added: Vec<(i64, i64, f32)> = visible.iter().filter(|g| g.added > 0).map(|g| (g.id, day_of(g.added, st.off), 1.0)).collect();
            let count = |v: f32| format!("{} games", v as i64);
            let mut bars = buckets(&added, Span::Months, st.today, &HashMap::new(), &count);
            for b in &mut bars {
                b.tip = b.tip.lines().take(2).collect::<Vec<_>>().join("\n");
            }
            column_chart(ui, "added", &bars, &count);
        });
    });
    ui.add_space(28.0);
}

fn top(m: HashMap<String, f32>, n: usize) -> Vec<(String, f32, Option<i64>)> {
    let mut v: Vec<_> = m.into_iter().map(|(k, x)| (k, x, None)).collect();
    v.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

fn tile(ui: &mut Ui, label: &str, value: &str, tip: &str) {
    egui::Frame::new()
        .fill(CARD)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.label(theme::caption(label));
                ui.label(RichText::new(value).size(20.0).strong());
            });
        })
        .response
        .on_hover_text(tip);
}

fn empty(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).color(TEXT_DIM));
}

fn span_header(ui: &mut Ui, title: &str, span: &mut Span) {
    ui.horizontal(|ui| {
        theme::section(ui, title);
        ui.add_space(12.0);
        ui.selectable_value(span, Span::Days, "30 days");
        ui.selectable_value(span, Span::Months, "12 months");
        ui.selectable_value(span, Span::Years, "Years");
    });
}

/// Single-hue column chart with a quiet grid; each bar has a hover tooltip.
fn column_chart(ui: &mut Ui, id: &str, bars: &[Bar], fmt: &dyn Fn(f32) -> String) {
    let (axis_w, plot_h, label_h) = (56.0, 150.0, 18.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), plot_h + label_h + 8.0), Sense::hover());
    if !ui.is_rect_visible(rect) || bars.is_empty() {
        return;
    }
    let plot = Rect::from_min_max(pos2(rect.left() + axis_w, rect.top() + 6.0), pos2(rect.right(), rect.top() + 6.0 + plot_h));
    let max = bars.iter().map(|b| b.value).fold(0.0, f32::max).max(1.0);
    let p = ui.painter();
    let grid = Color32::from_white_alpha(14);
    for i in 0..=2 {
        let v = max * i as f32 / 2.0;
        let y = plot.bottom() - plot.height() * i as f32 / 2.0;
        p.hline(plot.x_range(), y, egui::Stroke::new(1.0, grid));
        p.text(pos2(plot.left() - 8.0, y), Align2::RIGHT_CENTER, fmt(v), FontId::proportional(11.0), TEXT_DIM);
    }
    let slot = plot.width() / bars.len() as f32;
    let bar_w = (slot * 0.62).clamp(3.0, 36.0);
    // Label every bar when there's room, else every few.
    let every = ((44.0 / slot).ceil() as usize).max(1);
    let hover = ui.input(|i| i.pointer.hover_pos());
    for (i, b) in bars.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let col = Rect::from_min_max(pos2(cx - slot / 2.0, plot.top()), pos2(cx + slot / 2.0, plot.bottom()));
        let hot = hover.is_some_and(|h| col.contains(h));
        if b.value > 0.0 {
            let h = (plot.height() * b.value / max).max(2.0);
            let r = Rect::from_min_max(pos2(cx - bar_w / 2.0, plot.bottom() - h), pos2(cx + bar_w / 2.0, plot.bottom()));
            let fill = if hot { Color32::from_rgb(150, 172, 255) } else { ACCENT };
            ui.painter().rect_filled(r, CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, fill);
        }
        if i % every == (bars.len() - 1) % every {
            ui.painter().text(pos2(cx, plot.bottom() + 6.0), Align2::CENTER_TOP, &b.label, FontId::proportional(11.0), TEXT_DIM);
        }
        ui.interact(col, egui::Id::unique(("stats_bar", id, i)), Sense::hover()).on_hover_text(&b.tip);
    }
}

/// Ranked horizontal bars: label, bar, value. Returns the game of a clicked row.
fn hbars(ui: &mut Ui, rows: &[(String, f32, Option<i64>)], fmt: &dyn Fn(f32) -> String) -> Option<i64> {
    let max = rows.iter().map(|r| r.1).fold(0.0, f32::max).max(1.0);
    let mut clicked = None;
    for (label, v, game) in rows {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), if game.is_some() { Sense::click() } else { Sense::hover() });
        if !ui.is_rect_visible(rect) {
            continue;
        }
        if resp.hovered() && game.is_some() {
            ui.painter().rect_filled(rect, 4.0, CARD);
        }
        let label_w = (rect.width() * 0.42).min(240.0);
        let value_w = 70.0;
        let p = ui.painter();
        let mut job = egui::text::LayoutJob::simple_singleline(label.clone(), FontId::proportional(13.0), TEXT);
        job.wrap = egui::text::TextWrapping::truncate_at_width(label_w - 8.0);
        let galley = p.layout_job(job);
        p.galley(pos2(rect.left() + 4.0, rect.center().y - galley.size().y / 2.0), galley, TEXT);
        let track = Rect::from_min_max(pos2(rect.left() + label_w, rect.center().y - 4.0), pos2(rect.right() - value_w, rect.center().y + 4.0));
        p.rect_filled(track, 4.0, CARD_HI);
        let mut fill = track;
        fill.set_width((track.width() * v / max).max(4.0));
        p.rect_filled(fill, 4.0, ACCENT);
        p.text(pos2(rect.right() - 4.0, rect.center().y), Align2::RIGHT_CENTER, fmt(*v), FontId::proportional(12.0), TEXT_DIM);
        let resp = resp.on_hover_text(format!("{label}\n{}", fmt(*v)));
        if resp.clicked() {
            clicked = *game;
        }
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_helpers() {
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(ymd(20_418), (2025, 11, 26));
        assert_eq!(WEEKDAYS[(0i64 + 3).rem_euclid(7) as usize], "Thu");
        assert_eq!(month_of(20_418), 2025 * 12 + 10);
        assert_eq!(day_of(DAY - 1, 3600), 1, "local time moves late evening into the next day");
    }

    #[test]
    fn buckets_last_30_days_with_top_games() {
        let names = HashMap::new();
        let bars = buckets(&[(1, 100, 30.0), (2, 100, 60.0), (1, 95, 15.0), (1, 10, 999.0)], Span::Days, 100, &names, &|v| hours(v));
        assert_eq!(bars.len(), 30);
        assert_eq!(bars[29].value, 90.0);
        assert_eq!(bars[24].value, 15.0);
        assert_eq!(bars.iter().map(|b| b.value).sum::<f32>(), 105.0, "older days fall outside");
    }
}
