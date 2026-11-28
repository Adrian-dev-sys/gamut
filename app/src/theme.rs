//! Colors, type scale and small styling helpers shared by every view.
use egui::{Color32, Context, CornerRadius, FontId, Margin, RichText, Shadow, Stroke, TextStyle};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// The palette: built-in colors, overridable from `theme.toml` (hot-reloaded).
const DEFAULTS: [(&str, Color32); 11] = [
    ("accent", Color32::from_rgb(110, 140, 255)),
    // Second gradient stop, used sparingly (logo, top bar edge).
    ("accent_2", Color32::from_rgb(176, 112, 255)),
    ("background", Color32::from_rgb(13, 15, 22)),
    ("panel", Color32::from_rgb(19, 22, 32)),
    ("card", Color32::from_rgb(30, 34, 48)),
    ("card_hover", Color32::from_rgb(40, 46, 64)),
    ("border", Color32::from_rgb(44, 50, 70)),
    ("text", Color32::from_rgb(228, 232, 242)),
    ("text_dim", Color32::from_rgb(146, 154, 178)),
    ("good", Color32::from_rgb(56, 180, 112)),
    ("danger", Color32::from_rgb(196, 64, 76)),
];

static PALETTE: [AtomicU32; 11] = [const { AtomicU32::new(0) }; 11];
/// Set once a palette is loaded (until then, the defaults).
static LOADED: AtomicBool = AtomicBool::new(false);

fn color(i: usize) -> Color32 {
    if !LOADED.load(Ordering::Relaxed) {
        return DEFAULTS[i].1;
    }
    let [r, g, b, a] = PALETTE[i].load(Ordering::Relaxed).to_be_bytes();
    Color32::from_rgba_premultiplied(r, g, b, a)
}

macro_rules! colors {
    ($($name:ident = $i:expr),* $(,)?) => {
        $(#[allow(non_snake_case)] #[inline] pub fn $name() -> Color32 { color($i) })*
    };
}
colors!(ACCENT = 0, ACCENT_2 = 1, BG = 2, PANEL = 3, CARD = 4, CARD_HI = 5, BORDER = 6, TEXT = 7, TEXT_DIM = 8, GOOD = 9, DANGER = 10);

/// Dim layer behind every dialog.
pub const BACKDROP: Color32 = Color32::from_black_alpha(160);
/// Extra dim for a popup stacked on another (art picker over details).
pub const STACKED: Color32 = Color32::from_black_alpha(110);

/// Sizes and fonts from `theme.toml` (built-in values when unset).
#[derive(Clone, Default, PartialEq, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Style {
    corner_radius: Option<f32>,
    spacing: Option<f32>,
    text_size: Option<f32>,
    font: Option<String>,
    bold_font: Option<String>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ThemeFile {
    colors: std::collections::BTreeMap<String, String>,
    style: Style,
}

static STYLE: std::sync::RwLock<Option<Style>> = std::sync::RwLock::new(None);
/// Whether a bold font was loaded.
static BOLD: AtomicBool = AtomicBool::new(false);

pub fn file(root: &std::path::Path) -> std::path::PathBuf {
    root.join("theme.toml")
}

/// Reads `theme.toml` into the palette and style; no file = built-in theme. On error nothing changes.
pub fn load(path: &std::path::Path) -> Result<(), String> {
    let t: ThemeFile = match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("theme.toml: {}", e.message()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ThemeFile::default(),
        Err(e) => return Err(format!("theme.toml: {e}")),
    };
    let mut pal = DEFAULTS.map(|(_, c)| c);
    for (k, v) in &t.colors {
        let Some(i) = DEFAULTS.iter().position(|(n, _)| n == k) else {
            return Err(format!("theme.toml: unknown color \"{k}\""));
        };
        pal[i] = Color32::from_hex(v.trim()).map_err(|_| format!("theme.toml: {k} = \"{v}\" isn't a #rrggbb color"))?;
    }
    for f in [&t.style.font, &t.style.bold_font].into_iter().flatten().filter(|f| !f.is_empty()) {
        if !std::path::Path::new(f).is_file() {
            return Err(format!("theme.toml: font file not found: {f}"));
        }
    }
    for (i, c) in pal.iter().enumerate() {
        PALETTE[i].store(u32::from_be_bytes(c.to_array()), Ordering::Relaxed);
    }
    LOADED.store(true, Ordering::Relaxed);
    *STYLE.write().unwrap() = Some(t.style);
    Ok(())
}

/// A theme file with the built-in values, to start editing from.
pub fn template() -> String {
    let mut s = String::from(
        "# Gamut theme. Saved changes apply right away; delete a line (or this file) for the built-in value.\n\
         # Colors are #rrggbb (or #rrggbbaa).\n[colors]\n",
    );
    for (name, c) in DEFAULTS {
        s += &format!("{name} = \"{}\"\n", c.to_hex().get(..7).unwrap_or_default());
    }
    s += "\n[style]\n\
          # Buttons and fields; windows get twice this.\ncorner_radius = 6\n\
          # Gaps and padding, as a multiple of the built-in ones.\nspacing = 1.0\n\
          # Body text size (titles and small text follow).\ntext_size = 14\n\
          # A .ttf/.otf file for all text, and one for bold titles; empty = Windows' fonts.\n\
          font = \"\"\nbold_font = \"\"\n";
    s
}

/// Bold text, or the regular font where no bold font is there.
pub fn bold(size: f32) -> FontId {
    if BOLD.load(Ordering::Relaxed) {
        FontId::new(size, egui::FontFamily::Name("bold".into()))
    } else {
        FontId::proportional(size)
    }
}

fn read_font(path: Option<&str>, fallback: Option<&str>) -> Option<Vec<u8>> {
    path.filter(|p| !p.is_empty()).or(fallback).and_then(|p| std::fs::read(p).ok())
}

/// egui's fonts plus the theme's (first in line) and a bold family (Segoe UI Bold by default).
fn load_fonts(ctx: &Context, st: &Style) {
    let mut fonts = egui::FontDefinitions::default();
    if let Some(data) = read_font(st.font.as_deref(), None) {
        fonts.font_data.insert("custom".into(), std::sync::Arc::new(egui::FontData::from_owned(data)));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "custom".into());
    }
    let bold = read_font(st.bold_font.as_deref(), Some(r"C:\Windows\Fonts\segoeuib.ttf"));
    BOLD.store(bold.is_some(), Ordering::Relaxed);
    if let Some(data) = bold {
        fonts.font_data.insert("bold".into(), std::sync::Arc::new(egui::FontData::from_owned(data)));
        // Glyphs it lacks fall back to the regular fonts.
        let mut fam = vec!["bold".to_string()];
        fam.extend(fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default());
        fonts.families.insert(egui::FontFamily::Name("bold".into()), fam);
    }
    ctx.set_fonts(fonts);
}

/// Watches `theme.toml` and re-applies it when it changes.
pub struct Watcher {
    path: std::path::PathBuf,
    stamp: Option<std::time::SystemTime>,
    next: f64,
}

impl Watcher {
    pub fn new(root: &std::path::Path) -> Self {
        let path = file(root);
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        Watcher { path, stamp, next: 0.0 }
    }

    /// Checks about once a second; Some after a reload (Err keeps the previous theme).
    pub fn poll(&mut self, ctx: &Context) -> Option<Result<(), String>> {
        let now = ctx.input(|i| i.time);
        if now < self.next {
            return None;
        }
        self.next = now + 1.0;
        let stamp = std::fs::metadata(&self.path).and_then(|m| m.modified()).ok();
        if stamp == self.stamp {
            return None;
        }
        self.stamp = stamp;
        let r = load(&self.path);
        if r.is_ok() {
            apply(ctx);
        }
        ctx.request_repaint();
        Some(r)
    }
}

pub fn apply(ctx: &Context) {
    let st = STYLE.read().unwrap().clone().unwrap_or_default();
    // Fonts rebuild the atlas: only when they change.
    static FONTS: std::sync::Mutex<Option<(Option<String>, Option<String>)>> = std::sync::Mutex::new(None);
    let want = Some((st.font.clone(), st.bold_font.clone()));
    if *FONTS.lock().unwrap() != want {
        load_fonts(ctx, &st);
        *FONTS.lock().unwrap() = want;
    }
    let r = st.corner_radius.unwrap_or(6.0).clamp(0.0, 30.0);
    let (sp, ts) = (st.spacing.unwrap_or(1.0).clamp(0.3, 3.0), st.text_size.unwrap_or(14.0).clamp(9.0, 30.0));
    let rad = |k: f32| CornerRadius::same((r * k).round() as u8);
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL();
    v.window_fill = PANEL();
    v.window_stroke = Stroke::new(1.0, BORDER());
    v.extreme_bg_color = BG().lerp_to_gamma(PANEL(), 0.5);
    v.faint_bg_color = CARD();
    v.code_bg_color = CARD();
    v.selection.bg_fill = ACCENT().gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT());
    v.hyperlink_color = ACCENT();
    v.window_corner_radius = rad(2.0);
    v.menu_corner_radius = rad(4.0 / 3.0);
    v.window_shadow = Shadow { offset: [0, 12], blur: 36, spread: 0, color: Color32::from_black_alpha(160) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER());
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT());
    for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = rad(1.0);
        s.fg_stroke = Stroke::new(1.0, TEXT());
    }
    w.inactive.bg_fill = CARD();
    w.inactive.weak_bg_fill = CARD();
    w.hovered.bg_fill = CARD_HI();
    w.hovered.weak_bg_fill = CARD_HI();
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT().gamma_multiply(0.7));
    w.active.weak_bg_fill = ACCENT().gamma_multiply(0.35);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT());
    w.open.weak_bg_fill = CARD_HI();
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    ctx.set_visuals(v);

    // One wheel notch scrolls ~100 px (egui's default of 40 feels sluggish for a cover grid).
    ctx.options_mut(|o| o.input_options.line_scroll_speed = 100.0);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(ts - 2.0)),
            (TextStyle::Body, FontId::proportional(ts)),
            (TextStyle::Button, FontId::proportional(ts)),
            (TextStyle::Heading, FontId::proportional(ts + 4.0)),
            (TextStyle::Monospace, FontId::monospace(ts - 1.0)),
        ]
        .into();
        s.spacing.button_padding = egui::vec2(12.0, 6.0) * sp;
        s.spacing.item_spacing = egui::vec2(8.0, 8.0) * sp;
        s.spacing.interact_size.y = 28.0;
        s.spacing.window_margin = Margin::same((18.0 * sp).round() as i8);
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.combo_width = 150.0;
    });
}

/// Small uppercase label for section titles and field names.
/// Buttons/combos inside a CARD-filled panel: lighter fill and a visible border so they don't blend in.
pub fn raised_widgets(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    let edge = Color32::from_rgb(70, 80, 108);
    (w.inactive.bg_fill, w.inactive.weak_bg_fill, w.inactive.bg_stroke) = (CARD_HI(), CARD_HI(), Stroke::new(1.0, edge));
    (w.hovered.bg_fill, w.hovered.weak_bg_fill) = (Color32::from_rgb(52, 60, 84), Color32::from_rgb(52, 60, 84));
    w.open.bg_stroke = Stroke::new(1.0, edge);
}

pub fn caption(t: &str) -> RichText {
    RichText::new(t.to_uppercase()).size(11.0).color(TEXT_DIM()).extra_letter_spacing(0.8)
}

/// Secondary explanatory text.
pub fn hint(t: &str) -> RichText {
    RichText::new(t).size(12.5).color(TEXT_DIM())
}

/// Caption with consistent spacing around it.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(caption(title));
    ui.add_space(2.0);
}

/// Horizontal two-color gradient fill.
pub fn hgradient(p: &egui::Painter, r: egui::Rect, left: Color32, right: Color32) {
    let mut m = egui::Mesh::default();
    m.colored_vertex(r.left_top(), left);
    m.colored_vertex(r.right_top(), right);
    m.colored_vertex(r.left_bottom(), left);
    m.colored_vertex(r.right_bottom(), right);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 2, 3);
    p.add(m);
}

/// "GAMUT" wordmark with each letter stepping from ACCENT to ACCENT_2.
pub fn logo(ui: &mut egui::Ui) {
    let text = "GAMUT";
    let mut job = egui::text::LayoutJob::default();
    let n = text.chars().count().max(2) - 1;
    for (i, ch) in text.chars().enumerate() {
        let color = ACCENT().lerp_to_gamma(ACCENT_2(), i as f32 / n as f32);
        job.append(
            &ch.to_string(),
            0.0,
            egui::TextFormat {
                font_id: FontId::proportional(19.0),
                color,
                extra_letter_spacing: 1.5,
                ..Default::default()
            },
        );
    }
    ui.label(job);
}

/// Centered dialog; every popup uses this so they look and behave alike. The dim layer itself is
/// [`paint_backdrop`], drawn outside the popup so it survives egui's invisible first (sizing) frame.
pub fn dialog<R>(ctx: &Context, id: &str, stacked: bool, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    let frame = egui::Frame::new()
        .fill(PANEL())
        .stroke(Stroke::new(1.0, BORDER()))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
        .shadow(ctx.global_style().visuals.window_shadow);
    let dim = if stacked { STACKED } else { Color32::TRANSPARENT };
    popup(ctx, egui::Id::unique(id), frame, dim, add)
}

/// Modal that fades and slides in when it opens; the frame is drawn inside so it fades too.
pub fn popup<R>(ctx: &Context, id: egui::Id, frame: egui::Frame, dim: Color32, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::ModalResponse<R> {
    let t = appear(ctx, id, 0.16);
    let area = no_fade(id).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 16.0 * (1.0 - t)));
    egui::Modal::new(id).area(area).frame(egui::Frame::NONE).backdrop_color(dim).show(ctx, |ui| {
        ui.multiply_opacity(t);
        frame.show(ui, add).inner
    })
}

/// 0→1 (eased) over `secs` from when `id` was first drawn after not being drawn; repaints meanwhile.
pub fn appear(ctx: &Context, id: egui::Id, secs: f64) -> f32 {
    let (now, pass) = (ctx.input(|i| i.time), ctx.cumulative_pass_nr());
    let start = ctx.data_mut(|d| {
        let e = d.get_temp_mut_or_insert_with(id.with("appear"), || (now, pass));
        if e.1 + 2 < pass {
            e.0 = now;
        }
        e.1 = pass;
        e.0
    });
    let t = ((now - start) / secs).clamp(0.0, 1.0) as f32;
    if t < 1.0 {
        ctx.request_repaint();
    }
    egui::emath::easing::cubic_out(t)
}

/// Modal area without egui's fade-in: switching dialogs (details -> edit) would blink the backdrop.
pub fn no_fade(id: egui::Id) -> egui::Area {
    egui::Modal::default_area(id).fade_in(false)
}

/// Small ✕ drawn as two centered strokes (font glyphs sit off-center); highlights on hover.
pub fn x_at(ui: &mut egui::Ui, rect: egui::Rect, id: egui::Id) -> egui::Response {
    let r = ui.interact(rect, id, egui::Sense::CLICK); // not focusable: controller focus skips the ✕
    paint_x(ui, &r);
    r
}

/// [`x_at`] laid out as a square widget of `size`.
pub fn x_button(ui: &mut egui::Ui, size: f32) -> egui::Response {
    let (_, r) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::CLICK);
    paint_x(ui, &r);
    r
}

fn paint_x(ui: &egui::Ui, r: &egui::Response) {
    let c = r.rect.center();
    if r.hovered() {
        ui.painter().circle_filled(c, r.rect.width() / 2.0, CARD_HI());
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let h = (r.rect.width() * 0.18).max(3.0);
    let st = Stroke::new(1.5, if r.hovered() { Color32::WHITE } else { TEXT_DIM() });
    ui.painter().line_segment([c + egui::vec2(-h, -h), c + egui::vec2(h, h)], st);
    ui.painter().line_segment([c + egui::vec2(-h, h), c + egui::vec2(h, -h)], st);
}

/// Dims the library under popups, on its own layer between the panels and the popups.
pub fn paint_backdrop(ctx: &Context) {
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("gamut_backdrop"));
    ctx.layer_painter(layer).rect_filled(ctx.content_rect(), 0.0, BACKDROP);
}

/// Round "back to top" button in a scroll area's corner once it's scrolled past ~1.5 screens; true when clicked.
pub fn to_top_button(ui: &mut egui::Ui, view: egui::Rect, offset: f32) -> bool {
    if offset < view.height() * 1.5 {
        return false;
    }
    let c = egui::pos2(view.right() - 42.0, view.bottom() - 38.0);
    let b = egui::Rect::from_center_size(c, egui::vec2(46.0, 46.0));
    let r = ui.interact(b, ui.scope_id().with("to_top"), egui::Sense::click()).on_hover_text("Back to top");
    let p = ui.painter();
    p.circle_filled(c + egui::vec2(0.0, 3.0), 23.0, Color32::from_black_alpha(90));
    p.circle(c, 22.0, if r.hovered() { ACCENT() } else { CARD_HI() }, Stroke::new(1.0, BORDER()));
    let s = Stroke::new(2.5, TEXT());
    p.line_segment([c + egui::vec2(-8.0, 3.0), c + egui::vec2(0.0, -5.0)], s);
    p.line_segment([c + egui::vec2(8.0, 3.0), c + egui::vec2(0.0, -5.0)], s);
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_loads_and_errors_keep_the_theme() {
        let dir = std::env::temp_dir().join(format!("gamut-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = file(&dir);
        std::fs::write(&f, template()).unwrap();
        load(&f).unwrap();
        assert_eq!(ACCENT(), DEFAULTS[0].1);
        std::fs::write(&f, "[colors]\naccent = \"#ff0000\"\n").unwrap();
        load(&f).unwrap();
        assert_eq!(ACCENT(), Color32::from_rgb(255, 0, 0));
        assert_eq!(TEXT(), DEFAULTS[7].1);
        for bad in ["[colors]\naccent = \"red\"\n", "[colors]\nacent = \"#fff\"\n", "[style]\nfont = \"Z:/nope.ttf\"\n"] {
            std::fs::write(&f, bad).unwrap();
            assert!(load(&f).is_err(), "{bad}");
            assert_eq!(ACCENT(), Color32::from_rgb(255, 0, 0));
        }
        std::fs::remove_file(&f).unwrap();
        load(&f).unwrap();
        assert_eq!(ACCENT(), DEFAULTS[0].1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
