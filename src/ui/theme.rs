//! egui の見た目 (ゲーム風の配色・角の丸み・文字の大きさ)

use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Vec2};

/// 配色。夜空のような紺を土台に、紫・水色・橙を差し色にする。
pub mod palette {
    use bevy_egui::egui::Color32;

    /// いちばん奥 (入力欄・レールの背景)
    pub const INK: Color32 = Color32::from_rgb(0x11, 0x13, 0x1f);
    /// パネルの地
    pub const NAVY: Color32 = Color32::from_rgb(0x19, 0x1c, 0x2c);
    /// 浮かぶウィンドウの地
    pub const NAVY_LIGHT: Color32 = Color32::from_rgb(0x21, 0x25, 0x3a);
    /// 入力欄など一段沈んだ地
    pub const NAVY_DEEP: Color32 = Color32::from_rgb(0x13, 0x16, 0x24);
    /// ボタンの地
    pub const NAVY_BUTTON: Color32 = Color32::from_rgb(0x2b, 0x30, 0x4d);
    /// カーソルを乗せたときの地
    pub const NAVY_HOVER: Color32 = Color32::from_rgb(0x38, 0x3f, 0x68);
    /// 枠線・区切り線
    pub const LINE: Color32 = Color32::from_rgb(0x34, 0x3a, 0x5c);

    pub const TEXT: Color32 = Color32::from_rgb(0xc9, 0xcd, 0xe4);
    pub const TEXT_STRONG: Color32 = Color32::from_rgb(0xf2, 0xf3, 0xfa);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x7c, 0x82, 0xa6);

    pub const PURPLE: Color32 = Color32::from_rgb(0x85, 0x74, 0xff);
    pub const ORANGE: Color32 = Color32::from_rgb(0xff, 0xa8, 0x4a);
    pub const TEAL: Color32 = Color32::from_rgb(0x4a, 0xe0, 0xcb);
    pub const PINK: Color32 = Color32::from_rgb(0xff, 0x63, 0x94);
    pub const YELLOW: Color32 = Color32::from_rgb(0xff, 0xd6, 0x4a);
    pub const GREEN: Color32 = Color32::from_rgb(0x6b, 0xe0, 0x7a);

    /// 紙吹雪の色
    pub const CONFETTI: [Color32; 6] = [ORANGE, TEAL, PINK, YELLOW, PURPLE, GREEN];
}

/// 3D ビューの背景色
pub const CLEAR_COLOR: Color = Color::srgb(0.105, 0.118, 0.18);

/// 見た目を egui に反映する
pub fn apply(ctx: &egui::Context) {
    use palette::*;

    ctx.set_theme(egui::Theme::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();

    // 日本語は欧文より字面が小さく見えるので、標準より少し大きくする
    style.text_styles = [
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Heading,
            FontId::new(17.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(12.5, FontFamily::Monospace),
        ),
    ]
    .into();

    let spacing = &mut style.spacing;
    spacing.item_spacing = Vec2::new(8.0, 6.0);
    spacing.button_padding = Vec2::new(10.0, 4.0);
    spacing.window_margin = egui::Margin::same(12);
    spacing.menu_margin = egui::Margin::same(8);
    spacing.interact_size.y = 24.0;
    spacing.scroll.bar_width = 8.0;
    spacing.scroll.floating = true;

    let v = &mut style.visuals;
    v.panel_fill = NAVY;
    v.window_fill = NAVY_LIGHT;
    v.extreme_bg_color = NAVY_DEEP;
    v.faint_bg_color = Color32::from_rgb(0x1f, 0x23, 0x37);
    v.code_bg_color = NAVY_DEEP;
    v.hyperlink_color = TEAL;
    v.warn_fg_color = ORANGE;
    v.error_fg_color = PINK;
    v.window_corner_radius = CornerRadius::same(14);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 6],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.selection.bg_fill = PURPLE.gamma_multiply(0.85);
    v.selection.stroke = Stroke::new(1.5, TEXT_STRONG);
    v.striped = true;
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;

    let radius = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = NAVY_LIGHT;
    w.noninteractive.weak_bg_fill = NAVY_LIGHT;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = radius;
    w.inactive.bg_fill = NAVY_BUTTON;
    w.inactive.weak_bg_fill = NAVY_BUTTON;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT_STRONG);
    w.inactive.corner_radius = radius;
    w.hovered.bg_fill = NAVY_HOVER;
    w.hovered.weak_bg_fill = NAVY_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, TEAL.gamma_multiply(0.8));
    w.hovered.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.hovered.corner_radius = radius;
    w.hovered.expansion = 1.0;
    w.active.bg_fill = PURPLE;
    w.active.weak_bg_fill = PURPLE;
    w.active.bg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.fg_stroke = Stroke::new(1.5, Color32::WHITE);
    w.active.corner_radius = radius;
    w.active.expansion = 0.5;
    w.open.bg_fill = NAVY_HOVER;
    w.open.weak_bg_fill = NAVY_HOVER;
    w.open.bg_stroke = Stroke::new(1.0, PURPLE);
    w.open.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.open.corner_radius = radius;

    ctx.set_style_of(egui::Theme::Dark, style);
}
