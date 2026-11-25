use egui::{Color32, Context, CornerRadius, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(92, 140, 255);
pub const BG: Color32 = Color32::from_rgb(18, 20, 25);
pub const PANEL: Color32 = Color32::from_rgb(24, 27, 33);
pub const CARD: Color32 = Color32::from_rgb(36, 40, 48);
pub const TEXT_DIM: Color32 = Color32::from_rgb(150, 156, 168);

pub fn apply(ctx: &Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.selection.bg_fill = ACCENT.linear_multiply(0.6);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.window_corner_radius = CornerRadius::same(8);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(5);
    }
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}
