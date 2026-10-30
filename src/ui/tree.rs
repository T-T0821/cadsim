//! 左のアセンブリツリー
//!
//! - 行をクリックで選択 (アセンブリを選ぶと、その中の部品がすべて強調表示される)
//! - 目のボタンで表示/非表示、行にカーソルを乗せるとビューで薄く強調
//! - ビューで部品をクリックすると、ツリーをそこまで開いてスクロールする

use bevy_egui::egui;

use super::UiState;
use crate::camera::CameraRequest;
use crate::model::{Model, NodeId, NodeKind};

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    // ホバーは毎フレーム付け直す
    state.selection.hovered = None;

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
            state.selection.reveal_in_tree = false;

            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for root_id in state.model.roots.clone() {
                        node_row(ui, state, root_id, &reveal);
                    }
                });
        });
}

/// ツリーの1行と、開いていればその子
fn node_row(ui: &mut egui::Ui, state: &mut UiState, id: NodeId, reveal: &[NodeId]) {
    let Some(node) = state.model.get(id) else {
        return;
    };
    let children = node.children.clone();
    let row_id = ui.make_persistent_id(("tree", id));

    if children.is_empty() {
        // 子の無い行は開閉ボタンの分だけ字下げして揃える
        ui.horizontal(|ui| {
            ui.add_space(ui.spacing().indent);
            row_contents(ui, state, id, reveal);
        });
        return;
    }

    let mut collapsing =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), row_id, true);
    if reveal.contains(&id) && reveal.last() != Some(&id) {
        collapsing.set_open(true);
    }
    collapsing
        .show_header(ui, |ui| row_contents(ui, state, id, reveal))
        .body(|ui| {
            for child in children {
                node_row(ui, state, child, reveal);
            }
        });
}

/// 目のボタン・アイコン・名前
fn row_contents(ui: &mut egui::Ui, state: &mut UiState, id: NodeId, reveal: &[NodeId]) {
    let model: &Model = &state.model;
    let Some(node) = model.get(id) else {
        return;
    };
    let visible = node.visible;
    let shown = model.is_shown(id);
    let icon = match &node.kind {
        NodeKind::Assembly { .. } => "📁",
        NodeKind::Box(_) => "⬛",
        NodeKind::Imported { .. } => "⚙",
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
        state.selection.hovered = Some(id);
    }
    if selected && reveal.last() == Some(&id) {
        label.scroll_to_me(Some(egui::Align::Center));
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
