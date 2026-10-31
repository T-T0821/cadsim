//! egui の見た目 (ゲーム風 / シンプル)

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, Stroke, Vec2};

use cadsim_core::gamify::ThemeKind;

/// ゲーム風の配色
pub mod palette {
    use bevy_egui::egui::Color32;

    pub const NAVY: Color32 = Color32::from_rgb(0x1d, 0x21, 0x33);
    pub const NAVY_LIGHT: Color32 = Color32::from_rgb(0x27, 0x2c, 0x44);
    pub const NAVY_DEEP: Color32 = Color32::from_rgb(0x14, 0x17, 0x26);
    pub const PURPLE: Color32 = Color32::from_rgb(0x7c, 0x6c, 0xf2);
    pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0xa0, 0x45);
    pub const TEAL: Color32 = Color32::from_rgb(0x4e, 0xd8, 0xc8);
    pub const PINK: Color32 = Color32::from_rgb(0xff, 0x6b, 0x9a);
    pub const YELLOW: Color32 = Color32::from_rgb(0xff, 0xd9, 0x4a);
    pub const GREEN: Color32 = Color32::from_rgb(0x6b, 0xe0, 0x7a);

    /// 紙吹雪の色
    pub const CONFETTI: [Color32; 6] = [ORANGE, TEAL, PINK, YELLOW, PURPLE, GREEN];
}

/// 3D ビューの背景色
pub fn clear_color(theme: ThemeKind) -> Color {
    match theme {
        ThemeKind::Playful => Color::srgb(0.15, 0.17, 0.25),
        ThemeKind::Plain => Color::srgb(0.16, 0.17, 0.19),
    }
}

/// テーマを egui に反映する
pub fn apply(ctx: &egui::Context, theme: ThemeKind) {
    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.spacing = egui::Spacing::default();
    if theme == ThemeKind::Playful {
        playful(&mut style);
    }
    ctx.set_style_of(egui::Theme::Dark, style);
}

fn playful(style: &mut egui::Style) {
    use palette::*;

    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(8.0, 5.0);
    spacing.button_padding = Vec2::new(9.0, 4.0);
    spacing.window_margin = egui::Margin::same(10);
    spacing.menu_margin = egui::Margin::same(8);

    let v = &mut style.visuals;
    v.panel_fill = NAVY;
    v.window_fill = NAVY_LIGHT;
    v.extreme_bg_color = NAVY_DEEP;
    v.faint_bg_color = Color32::from_rgb(0x23, 0x28, 0x3d);
    v.code_bg_color = NAVY_DEEP;
    v.hyperlink_color = TEAL;
    v.warn_fg_color = ORANGE;
    v.error_fg_color = PINK;
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.5, Color32::from_rgb(0x45, 0x4c, 0x78));
    v.window_shadow.blur = 18;
    v.window_shadow.color = Color32::from_black_alpha(110);
    v.popup_shadow.color = Color32::from_black_alpha(110);
    v.selection.bg_fill = PURPLE;
    v.selection.stroke = Stroke::new(1.5, Color32::WHITE);

    let radius = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = NAVY_LIGHT;
    w.noninteractive.weak_bg_fill = NAVY_LIGHT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x38, 0x3f, 0x62));
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = Color32::from_rgb(0x34, 0x3b, 0x5c);
    w.inactive.weak_bg_fill = Color32::from_rgb(0x30, 0x36, 0x55);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = Color32::from_rgb(0x4b, 0x54, 0x88);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x4b, 0x54, 0x88);
    w.hovered.bg_stroke = Stroke::new(1.5, TEAL);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 1.5;
    w.active.bg_fill = PURPLE;
    w.active.weak_bg_fill = PURPLE;
    w.active.bg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.corner_radius = radius;
    w.active.expansion = 0.5;
    w.open.weak_bg_fill = Color32::from_rgb(0x40, 0x48, 0x74);
    w.open.corner_radius = radius;
}
