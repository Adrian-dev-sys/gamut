//! Colors, type scale and small styling helpers shared by every view.
use egui::{Color32, Context, CornerRadius, FontId, Margin, RichText, Shadow, Stroke, TextStyle};

pub const ACCENT: Color32 = Color32::from_rgb(110, 140, 255);
/// Second gradient stop, used sparingly (logo, top bar edge).
pub const ACCENT_2: Color32 = Color32::from_rgb(176, 112, 255);
pub const BG: Color32 = Color32::from_rgb(13, 15, 22);
pub const PANEL: Color32 = Color32::from_rgb(19, 22, 32);
pub const CARD: Color32 = Color32::from_rgb(30, 34, 48);
pub const CARD_HI: Color32 = Color32::from_rgb(40, 46, 64);
pub const BORDER: Color32 = Color32::from_rgb(44, 50, 70);
pub const TEXT: Color32 = Color32::from_rgb(228, 232, 242);
pub const TEXT_DIM: Color32 = Color32::from_rgb(146, 154, 178);
pub const GOOD: Color32 = Color32::from_rgb(56, 180, 112);
pub const DANGER: Color32 = Color32::from_rgb(196, 64, 76);
/// Dim layer behind every dialog.
pub const BACKDROP: Color32 = Color32::from_black_alpha(160);
/// Extra dim for a popup stacked on another (art picker over details).
pub const STACKED: Color32 = Color32::from_black_alpha(110);

pub fn apply(ctx: &Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.extreme_bg_color = Color32::from_rgb(16, 18, 27);
    v.faint_bg_color = CARD;
    v.code_bg_color = CARD;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow { offset: [0, 12], blur: 36, spread: 0, color: Color32::from_black_alpha(160) };
    v.popup_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(140) };

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    for s in [&mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(6);
        s.fg_stroke = Stroke::new(1.0, TEXT);
    }
    w.inactive.bg_fill = CARD;
    w.inactive.weak_bg_fill = CARD;
    w.hovered.bg_fill = CARD_HI;
    w.hovered.weak_bg_fill = CARD_HI;
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT.gamma_multiply(0.7));
    w.active.weak_bg_fill = ACCENT.gamma_multiply(0.35);
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.open.weak_bg_fill = CARD_HI;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    ctx.set_visuals(v);

    // One wheel notch scrolls ~100 px (egui's default of 40 feels sluggish for a cover grid).
    ctx.options_mut(|o| o.input_options.line_scroll_speed = 100.0);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, FontId::proportional(12.0)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (TextStyle::Heading, FontId::proportional(18.0)),
            (TextStyle::Monospace, FontId::monospace(13.0)),
        ]
        .into();
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.interact_size.y = 28.0;
        s.spacing.window_margin = Margin::same(18);
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.combo_width = 150.0;
    });
}

/// Small uppercase label for section titles and field names.
/// Buttons/combos inside a CARD-filled panel: lighter fill and a visible border so they don't blend in.
pub fn raised_widgets(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    let edge = Color32::from_rgb(70, 80, 108);
    (w.inactive.bg_fill, w.inactive.weak_bg_fill, w.inactive.bg_stroke) = (CARD_HI, CARD_HI, Stroke::new(1.0, edge));
    (w.hovered.bg_fill, w.hovered.weak_bg_fill) = (Color32::from_rgb(52, 60, 84), Color32::from_rgb(52, 60, 84));
    w.open.bg_stroke = Stroke::new(1.0, edge);
}

pub fn caption(t: &str) -> RichText {
    RichText::new(t.to_uppercase()).size(11.0).color(TEXT_DIM).extra_letter_spacing(0.8)
}

/// Secondary explanatory text.
pub fn hint(t: &str) -> RichText {
    RichText::new(t).size(12.5).color(TEXT_DIM)
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
        let color = ACCENT.lerp_to_gamma(ACCENT_2, i as f32 / n as f32);
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
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
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
        ui.painter().circle_filled(c, r.rect.width() / 2.0, CARD_HI);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let h = (r.rect.width() * 0.18).max(3.0);
    let st = Stroke::new(1.5, if r.hovered() { Color32::WHITE } else { TEXT_DIM });
    ui.painter().line_segment([c + egui::vec2(-h, -h), c + egui::vec2(h, h)], st);
    ui.painter().line_segment([c + egui::vec2(-h, h), c + egui::vec2(h, -h)], st);
}

/// Dims the library under popups, on its own layer between the panels and the popups.
pub fn paint_backdrop(ctx: &Context) {
    let layer = egui::LayerId::new(egui::Order::Middle, egui::Id::unique("gamut_backdrop"));
    ctx.layer_painter(layer).rect_filled(ctx.content_rect(), 0.0, BACKDROP);
}
