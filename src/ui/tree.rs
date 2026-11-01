//! 左のアセンブリツリー
//!
//! - 行をクリックで選択 (アセンブリを選ぶと、その中の部品がすべて強調表示される)
//! - 目のボタンで表示/非表示、行にカーソルを乗せるとビューで薄く強調
//! - ビューで部品をクリックすると、ツリーをそこまで開いてスクロールする
//! - 見えている行だけを描くので、数千部品のアセンブリでも重くならない

use std::collections::HashMap;

use bevy::ecs::change_detection::DetectChangesMut;
use bevy_egui::egui;

use super::UiState;
use crate::viewport::camera::CameraRequest;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    // ホバーは毎フレーム付け直す。値が変わったときだけ「選択が変わった」扱いにする
    // (毎フレーム変わった扱いにすると、数千部品の表示を毎フレーム計算し直してしまう)
    let previous_hover = state.selection.hovered;
    state.selection.bypass_change_detection().hovered = None;

    egui::Panel::left("assembly_tree")
        .resizable(true)
        .default_size(260.0)
        .min_size(180.0)
        .show(root, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("アセンブリ");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("すべて表示").clicked() {
                        state.model.show_all();
                    }
                });
            });
            let total = state
                .model
                .roots
                .iter()
                .map(|&r| state.model.measure(r).part_count)
                .sum::<usize>();
            ui.weak(format!("部品 {total} 個"));
            ui.separator();

            if state.model.is_empty() {
                ui.label("何も開いていません。");
                if ui.button("ファイルを開く…").clicked() {
                    state.jobs.request_dialog();
                }
                return;
            }

            // 選択中のノードまでの祖先 (ビューで選んだときに開く)
            let reveal = state
                .selection
                .reveal_in_tree
                .then(|| state.selection.node.map(|id| state.model.ancestors(id)))
                .flatten()
                .unwrap_or_default();
            if state.selection.reveal_in_tree {
                state.selection.reveal_in_tree = false;
            }

            rows(ui, state, &reveal);
        });

    if state.selection.hovered != previous_hover {
        state.selection.set_changed();
    }
}

/// ツリーの開閉状態 (開いているか)。触っていないノードは開いている。
type OpenState = HashMap<NodeId, bool>;

/// ツリーの1行
struct Row {
    id: NodeId,
    depth: usize,
    has_children: bool,
    open: bool,
}

/// 開いているノードをたどって行を並べ、画面に見えている行だけを描く。
/// 数千部品でも、描くのはスクロール位置にある数十行だけで済む。
fn rows(ui: &mut egui::Ui, state: &mut UiState, reveal: &[NodeId]) {
    let open_id = ui.make_persistent_id("tree_open");
    let mut open: OpenState = ui.data(|d| d.get_temp(open_id)).unwrap_or_default();
    // ビューで選んだ部品が見えるよう祖先を開く
    if let Some((_, ancestors)) = reveal.split_last() {
        for &a in ancestors {
            open.insert(a, true);
        }
    }

    let mut rows = Vec::new();
    let mut stack: Vec<(NodeId, usize)> = state.model.roots.iter().rev().map(|&r| (r, 0)).collect();
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = state.model.get(id) else {
            continue;
        };
        let has_children = !node.children.is_empty();
        let is_open = has_children && open.get(&id).copied().unwrap_or(true);
        rows.push(Row {
            id,
            depth,
            has_children,
            open: is_open,
        });
        if is_open {
            stack.extend(node.children.iter().rev().map(|&c| (c, depth + 1)));
        }
    }

    let row_height = ui.spacing().interact_size.y;
    let mut scroll = egui::ScrollArea::both().auto_shrink([false, false]);
    // 選んだ行が見えるところまでスクロールする (見えていない行は描かないので scroll_to_me は使えない)
    if let Some(index) = reveal
        .last()
        .and_then(|target| rows.iter().position(|r| r.id == *target))
    {
        let pitch = row_height + ui.spacing().item_spacing.y;
        let visible = ui.available_height();
        scroll = scroll.vertical_scroll_offset((index as f32 * pitch - visible / 2.0).max(0.0));
    }
    let indent = ui.spacing().indent;
    scroll.show_rows(ui, row_height, rows.len(), |ui, range| {
        for row in &rows[range] {
            ui.horizontal(|ui| {
                ui.add_space(row.depth as f32 * indent);
                let (rect, toggle) =
                    ui.allocate_exact_size(egui::vec2(indent, row_height), egui::Sense::click());
                if row.has_children {
                    let openness = if row.open { 1.0 } else { 0.0 };
                    let size = egui::Vec2::splat(ui.spacing().icon_width);
                    let icon = toggle
                        .clone()
                        .with_new_rect(egui::Rect::from_center_size(rect.center(), size));
                    egui::collapsing_header::paint_default_icon(ui, openness, &icon);
                    if toggle.clicked() {
                        open.insert(row.id, !row.open);
                    }
                }
                row_contents(ui, state, row.id);
            });
        }
    });
    ui.data_mut(|d| d.insert_temp(open_id, open));
}

/// 目のボタン・アイコン・名前
fn row_contents(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let model: &Model = &state.model;
    let Some(node) = model.get(id) else {
        return;
    };
    let visible = node.visible;
    let shown = model.is_shown(id);
    let icon = match &node.kind {
        NodeKind::Assembly { .. } => "📁",
        NodeKind::Box(_) => "⬛",
        NodeKind::Imported { .. } | NodeKind::StandardBody { .. } => "⚙",
        NodeKind::Standard(_) => "🔧",
    };
    // 親が非表示のときは薄く表示する
    let mut text = egui::RichText::new(format!("{icon} {}", node.name));
    if !shown {
        text = text.weak();
    }
    let hover = match node.kind.product() {
        Some(product) if product != node.name => format!("{} ({product})", node.kind.label()),
        _ => node.kind.label().to_owned(),
    };

    let eye = ui
        .add(
            egui::Button::new(if visible { "👁" } else { "–" })
                .small()
                .frame(false),
        )
        .on_hover_text(if visible {
            "隠す (H)"
        } else {
            "表示する"
        });
    if eye.clicked()
        && let Some(node) = state.model.get_mut(id)
    {
        node.visible = !visible;
    }

    let selected = state.selection.node == Some(id);
    let label = ui.selectable_label(selected, text).on_hover_text(hover);
    if label.clicked() {
        state.selection.select(id);
    }
    if label.double_clicked() {
        state.camera.write(CameraRequest::FitSelection);
    }
    if label.hovered() || eye.hovered() {
        state.selection.bypass_change_detection().hovered = Some(id);
    }

    label.context_menu(|ui| {
        if ui.button("選択を拡大").clicked() {
            state.selection.select(id);
            state.camera.write(CameraRequest::FitSelection);
        }
        if ui.button("これだけ表示").clicked() {
            state.model.isolate(id);
        }
        if ui
            .button(if visible { "隠す" } else { "表示する" })
            .clicked()
            && let Some(node) = state.model.get_mut(id)
        {
            node.visible = !visible;
        }
        ui.separator();
        if ui.button("削除").clicked() {
            state.history.remove(&mut state.model, id);
            state.selection.clear();
        }
    });
}
