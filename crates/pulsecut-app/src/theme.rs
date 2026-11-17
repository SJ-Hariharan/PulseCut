//! PulseCut design system: one dark theme, one accent, consistent spacing and radii.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(14, 15, 20);
pub const SURFACE: Color32 = Color32::from_rgb(22, 24, 31);
pub const SURFACE_2: Color32 = Color32::from_rgb(30, 33, 42);
pub const BORDER: Color32 = Color32::from_rgb(44, 48, 60);
pub const TEXT: Color32 = Color32::from_rgb(232, 234, 240);
pub const MUTED: Color32 = Color32::from_rgb(140, 146, 162);
pub const ACCENT: Color32 = Color32::from_rgb(255, 77, 109); // "pulse" coral
pub const ACCENT_2: Color32 = Color32::from_rgb(139, 92, 246); // violet
pub const OK: Color32 = Color32::from_rgb(52, 211, 153);
pub const WARN: Color32 = Color32::from_rgb(251, 191, 36);
pub const BAD: Color32 = Color32::from_rgb(248, 113, 113);
pub const INFO: Color32 = Color32::from_rgb(96, 165, 250);

pub const RADIUS: u8 = 10;
pub const GAP: f32 = 12.0;

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = SURFACE;
    v.extreme_bg_color = Color32::from_rgb(10, 11, 15);
    v.faint_bg_color = SURFACE;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = INFO;
    v.selection.bg_fill = ACCENT.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.widgets.noninteractive.bg_fill = SURFACE;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = SURFACE_2;
    v.widgets.inactive.weak_bg_fill = SURFACE_2;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.hovered.bg_fill = Color32::from_rgb(40, 44, 56);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(40, 44, 56);
    v.widgets.active.bg_fill = ACCENT.linear_multiply(0.6);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.window_corner_radius = CornerRadius::same(RADIUS);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 8.0);
        s.spacing.button_padding = egui::vec2(12.0, 6.0);
        s.spacing.interact_size.y = 28.0;
        s.text_styles
            .insert(TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional));
        s.text_styles
            .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        s.text_styles
            .insert(TextStyle::Button, FontId::new(14.0, FontFamily::Proportional));
        s.text_styles
            .insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
    });
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(Margin::same(14))
}

pub fn status_color(s: pulsecut_core::ClipStatus) -> Color32 {
    use pulsecut_core::ClipStatus::*;
    match s {
        Capturing | Rendering => INFO,
        Review => WARN,
        Approved => ACCENT_2,
        Publishing => INFO,
        Published => OK,
        Rejected => MUTED,
        Failed => BAD,
    }
}

pub fn kind_color(k: pulsecut_core::MomentKind) -> Color32 {
    use pulsecut_core::MomentKind::*;
    match k {
        Hype => ACCENT,
        Funny => WARN,
        Fail => BAD,
        Clutch => ACCENT_2,
        Shock => INFO,
        Wholesome => OK,
        Generic => MUTED,
    }
}
