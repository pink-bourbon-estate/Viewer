//! PureRef-like look: near-black canvas, quiet charcoal chrome, small type,
//! hairline separators, nothing competing with the picture.

use egui::{Color32, CornerRadius, FontId, Stroke, TextStyle, Visuals};

pub const CANVAS: Color32 = Color32::from_rgb(30, 30, 30);
pub const CHROME: Color32 = Color32::from_rgb(38, 38, 38);
pub const PANEL: Color32 = Color32::from_rgb(34, 34, 34);
pub const CARD: Color32 = Color32::from_rgb(44, 44, 44);
pub const LINE: Color32 = Color32::from_rgb(54, 54, 54);
pub const TEXT: Color32 = Color32::from_rgb(200, 200, 200);
pub const DIM: Color32 = Color32::from_rgb(140, 140, 140);
pub const ACCENT: Color32 = Color32::from_rgb(214, 160, 92); // warm ochre highlight
pub const SELECT: Color32 = Color32::from_rgb(78, 78, 78);

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = CHROME;
    v.extreme_bg_color = Color32::from_rgb(26, 26, 26);
    v.faint_bg_color = CARD;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(4);
    v.menu_corner_radius = CornerRadius::same(4);
    v.selection.bg_fill = Color32::from_rgb(92, 76, 52);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);
    let r = CornerRadius::same(3);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.hovered.bg_fill = Color32::from_rgb(58, 58, 58);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(58, 58, 58);
    v.widgets.active.bg_fill = SELECT;
    v.widgets.active.weak_bg_fill = SELECT;
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(6.0, 4.0);
        s.spacing.button_padding = egui::vec2(6.0, 2.0);
        s.spacing.interact_size.y = 20.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(12.5));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(12.5));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(10.5));
        s.text_styles.insert(TextStyle::Heading, FontId::proportional(14.0));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(11.5));
    });
}

pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(text.to_uppercase()).size(10.5).color(DIM).strong());
    ui.add_space(2.0);
}
