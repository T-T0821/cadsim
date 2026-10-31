//! 画面左端のツールレールと、3D ビューに浮かべるパネルの開け閉め
//!
//! 道具の一覧と「どれを開いているか」は `cadsim_core::workspace` が持つ。
//! ここはそれを Bevy の Resource に包み、レールのボタンとパネルの枠を描くだけにする。

use bevy::prelude::*;
use bevy_egui::egui::{self, Align2, Color32, CornerRadius, Stroke, pos2, vec2};

use super::theme::palette;
use cadsim_core::workspace::{Tool, ToolGroup, Workspace};

/// 開いている道具 (UI 層の包み)
#[derive(Resource, Default, Deref, DerefMut)]
pub struct Dock(pub Workspace);

/// レールの幅
pub const RAIL_WIDTH: f32 = 64.0;
const BUTTON_SIZE: egui::Vec2 = egui::vec2(54.0, 46.0);
/// パネルを 3D ビューの端からどれだけ離して出すか
const MARGIN: f32 = 10.0;

/// 道具のアイコン (egui の絵文字フォントにある字)
pub fn icon(tool: Tool) -> &'static str {
    match tool {
        Tool::Library => "📦",
        Tool::Modeling => "🔨",
        Tool::Interference => "💥",
        Tool::Annotation => "📐",
        Tool::Physics => "⚙",
        Tool::Conveyor => "🔁",
        Tool::Floor => "🚶",
        Tool::Safety => "🛡",
        Tool::Plc => "🔌",
        Tool::Flow => "📊",
        Tool::LineLink => "🔗",
        Tool::Site => "🗺",
        Tool::Compose => "💬",
        Tool::SequenceGen => "✨",
    }
}

/// パネルとメニューに出す題名 (アイコン付き)
pub fn title(tool: Tool) -> String {
    format!("{}  {}", icon(tool), tool.title())
}

/// ビューに浮かべる小さなパネルの枠。開いていなければ None。
///
/// 使い方:
/// ```ignore
/// let Some(window) = dock::panel(Tool::Safety, &dock, viewport.rect) else { return Ok(()) };
/// let mut open = true;
/// window.open(&mut open).show(ctx, |ui| { ... });
/// if !open { dock.set_open(Tool::Safety, false); }
/// ```
pub fn panel(tool: Tool, dock: &Workspace, view: Option<Rect>) -> Option<egui::Window<'static>> {
    if !dock.is_open(tool) {
        return None;
    }
    let mut window = egui::Window::new(title(tool))
        .id(window_id(tool))
        .collapsible(true)
        .resizable(false);
    if let Some(view) = view {
        let view =
            egui::Rect::from_min_max(pos2(view.min.x, view.min.y), pos2(view.max.x, view.max.y));
        let [x, y] = dock.panel_position(tool, [view.min.x + MARGIN, view.min.y + MARGIN]);
        window = window
            .pivot(Align2::LEFT_TOP)
            .default_pos([x, y])
            .constrain_to(view.shrink(2.0));
    }
    Some(window)
}

/// 大きな作業用のウィンドウの枠 (真ん中寄りに開き、大きさを変えられる)。開いていなければ None。
pub fn workspace_window(
    tool: Tool,
    dock: &Workspace,
    default_size: [f32; 2],
) -> Option<egui::Window<'static>> {
    if !dock.is_open(tool) {
        return None;
    }
    Some(
        egui::Window::new(title(tool))
            .id(window_id(tool))
            .default_size(default_size)
            .default_pos([320.0, 130.0])
            .resizable(true),
    )
}

fn window_id(tool: Tool) -> egui::Id {
    egui::Id::new(("tool_window", tool.title()))
}

/// レールのボタンに出す小さな数字や印
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Badge {
    /// 件数 (赤い丸)
    Count(usize),
    /// 注意 (黄色い点)
    Alert,
}

/// レールの1つのボタンの状態
pub struct RailItem {
    pub tool: Tool,
    pub open: bool,
    pub enabled: bool,
    pub badge: Option<Badge>,
    /// 使えないときの理由 (ツールチップに出す)
    pub disabled_reason: &'static str,
}

/// 画面左端のツールレールを描き、押された道具を返す
pub fn rail(root: &mut egui::Ui, items: &[RailItem]) -> Option<Tool> {
    let mut clicked = None;
    egui::Panel::left("tool_rail")
        .resizable(false)
        .exact_size(RAIL_WIDTH)
        .frame(
            egui::Frame::new()
                .fill(palette::INK)
                .inner_margin(egui::Margin::symmetric(5, 6)),
        )
        .show(root, |ui| {
            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for group in ToolGroup::ALL {
                        group_caption(ui, group);
                        for item in items.iter().filter(|i| i.tool.group() == group) {
                            if rail_button(ui, item).clicked() {
                                clicked = Some(item.tool);
                            }
                        }
                        ui.add_space(6.0);
                    }
                });
        });
    clicked
}

fn group_caption(ui: &mut egui::Ui, group: ToolGroup) {
    ui.vertical_centered(|ui| {
        ui.label(
            egui::RichText::new(group.label())
                .size(9.5)
                .color(palette::TEXT_FAINT),
        );
    });
}

fn rail_button(ui: &mut egui::Ui, item: &RailItem) -> egui::Response {
    let sense = if item.enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(BUTTON_SIZE, sense);
    let tool = item.tool;
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = response.hovered() && item.enabled;
        let fill = if item.open {
            palette::PURPLE.gamma_multiply(0.38)
        } else if hovered {
            palette::NAVY_HOVER
        } else {
            Color32::TRANSPARENT
        };
        painter.rect_filled(rect, CornerRadius::same(9), fill);
        if item.open {
            // 開いている印: 左端の光る棒
            let bar = egui::Rect::from_min_size(
                pos2(rect.min.x - 4.0, rect.min.y + 9.0),
                vec2(3.0, rect.height() - 18.0),
            );
            painter.rect_filled(bar, CornerRadius::same(2), palette::TEAL);
        }
        let alpha = if item.enabled { 1.0 } else { 0.35 };
        let fg = if item.open || hovered {
            Color32::WHITE
        } else {
            palette::TEXT
        };
        painter.text(
            rect.center_top() + vec2(0.0, 15.0),
            Align2::CENTER_CENTER,
            icon(tool),
            egui::FontId::proportional(19.0),
            fg.gamma_multiply(alpha),
        );
        painter.text(
            rect.center_bottom() - vec2(0.0, 10.0),
            Align2::CENTER_CENTER,
            tool.short_label(),
            egui::FontId::proportional(10.5),
            fg.gamma_multiply(alpha * 0.9),
        );
        match item.badge {
            Some(Badge::Count(n)) if n > 0 => {
                let center = rect.right_top() + vec2(-9.0, 9.0);
                let text = if n > 99 { "99+".into() } else { n.to_string() };
                let w = 7.0 + 3.5 * text.len() as f32;
                painter.rect(
                    egui::Rect::from_center_size(center, vec2(w.max(15.0), 15.0)),
                    CornerRadius::same(8),
                    palette::PINK,
                    Stroke::new(1.5, palette::INK),
                    egui::StrokeKind::Outside,
                );
                painter.text(
                    center,
                    Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(9.5),
                    Color32::WHITE,
                );
            }
            Some(Badge::Alert) => {
                painter.circle(
                    rect.right_top() + vec2(-9.0, 9.0),
                    5.0,
                    palette::YELLOW,
                    Stroke::new(1.5, palette::INK),
                );
            }
            _ => {}
        }
    }
    let hint = if item.enabled {
        format!(
            "{}\n{}\n({})",
            tool.title(),
            tool.hint(),
            if item.open {
                "クリックで閉じる"
            } else {
                "クリックで開く"
            }
        )
    } else {
        format!("{}\n{}", tool.title(), item.disabled_reason)
    };
    response.on_hover_text(hint)
}
