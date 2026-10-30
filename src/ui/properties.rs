//! 右のプロパティパネル: 選択中の部品・アセンブリの情報と編集

use bevy::math::{DMat4, DVec3};
use bevy_egui::egui;

use super::UiState;
use super::widgets::{length, round_for_display, row, xyz_editor};
use crate::camera::CameraRequest;
use crate::math::{euler_deg_to_quat, quat_to_euler_deg, split_placement};
use crate::model::{NodeId, NodeKind};

pub fn show(root: &mut egui::Ui, state: &mut UiState) {
    egui::Panel::right("properties")
        .resizable(true)
        .default_size(290.0)
        .min_size(220.0)
        .show(root, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.heading("プロパティ");
                    ui.separator();
                    match state
                        .selection
                        .node
                        .filter(|&id| state.model.get(id).is_some())
                    {
                        Some(id) => selected_node(ui, state, id),
                        None => nothing_selected(ui, state),
                    }
                });
        });
}

fn nothing_selected(ui: &mut egui::Ui, state: &mut UiState) {
    ui.label("ツリーかビューで部品を選んでください。");
    ui.add_space(8.0);
    egui::CollapsingHeader::new("新しい箱")
        .default_open(true)
        .show(ui, |ui| {
            let mut size = state.model.new_box_size;
            xyz_editor(ui, "寸法 (幅 / 高さ / 奥行)", &mut size, 0.1, Some(0.001));
            state.model.new_box_size = size;
            if ui.button("箱を追加").clicked() {
                super::menu::add_box(state);
            }
        });
}

fn selected_node(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    general_section(ui, state, id);
    ui.add_space(4.0);
    actions(ui, state, id);
    ui.separator();

    let is_box = matches!(state.model.get(id).map(|n| &n.kind), Some(NodeKind::Box(_)));
    egui::CollapsingHeader::new(if is_box {
        "形状と配置"
    } else {
        "配置 (親基準)"
    })
    .default_open(true)
    .show(ui, |ui| {
        if is_box {
            box_editor(ui, state, id);
        } else {
            placement_editor(ui, state, id);
        }
    });
    egui::CollapsingHeader::new("寸法 (外接箱)")
        .default_open(true)
        .show(ui, |ui| bounds_section(ui, state, id));
    egui::CollapsingHeader::new("計測")
        .default_open(true)
        .show(ui, |ui| measurements_section(ui, state, id));
}

/// 名前・種類・表示・色
fn general_section(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let unit = state.model.unit.clone();
    let path = state.model.path_label(id);
    let Some(node) = state.model.get_mut(id) else {
        return;
    };
    ui.add(egui::TextEdit::singleline(&mut node.name).desired_width(f32::INFINITY));
    ui.add_space(4.0);
    egui::Grid::new("general")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            row(ui, "種類", node.kind.label());
            if let Some(product) = node.kind.product() {
                row(ui, "製品名", product);
            }
            ui.weak("場所");
            ui.add(egui::Label::new(path).wrap());
            ui.end_row();
            if !node.children.is_empty() {
                row(ui, "子", format!("{} 個", node.children.len()));
            }
            row(ui, "単位", unit);

            ui.weak("表示");
            ui.checkbox(&mut node.visible, "表示する");
            ui.end_row();

            ui.weak("色");
            ui.horizontal(|ui| {
                let mut rgb = node.color.unwrap_or(match &node.kind {
                    NodeKind::Imported { shape, .. } => {
                        shape.color.unwrap_or(crate::model::DEFAULT_PART_COLOR)
                    }
                    _ => crate::model::DEFAULT_PART_COLOR,
                });
                if ui.color_edit_button_rgb(&mut rgb).changed() {
                    node.color = Some(rgb);
                }
                if node.color.is_some() && ui.small_button("元に戻す").clicked() {
                    node.color = None;
                }
            });
            ui.end_row();
        });
}

fn actions(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    ui.horizontal_wrapped(|ui| {
        if ui
            .button("🔍 拡大")
            .on_hover_text("選択を画面いっぱいに表示 (F)")
            .clicked()
        {
            state.camera.write(CameraRequest::FitSelection);
        }
        if ui.button("これだけ表示").clicked() {
            state.model.isolate(id);
        }
        let parent = state.model.get(id).and_then(|n| n.parent);
        if ui
            .add_enabled(parent.is_some(), egui::Button::new("⬆ 親を選択"))
            .clicked()
            && let Some(parent) = parent
        {
            state.selection.select(parent);
            state.selection.reveal_in_tree = true;
        }
        if ui.button("🗑 削除").clicked() {
            state.model.remove(id);
            state.selection.clear();
        }
    });
}

/// 箱: 寸法・位置・回転のパラメータを直接編集する
fn box_editor(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let Some(NodeKind::Box(part)) = state.model.get(id).map(|n| &n.kind) else {
        return;
    };
    let mut edited = part.clone();
    let mut changed = false;
    changed |= xyz_editor(
        ui,
        "寸法 (幅 / 高さ / 奥行)",
        &mut edited.size,
        0.05,
        Some(0.001),
    );
    changed |= xyz_editor(ui, "位置", &mut edited.position, 0.05, None);
    changed |= xyz_editor(ui, "回転 (度)", &mut edited.rotation_deg, 1.0, None);
    ui.horizontal(|ui| {
        for (label, axis) in [("+15° X", 0), ("+15° Y", 1), ("+15° Z", 2)] {
            if ui.button(label).clicked() {
                edited.rotation_deg[axis] = (edited.rotation_deg[axis] + 15.0) % 360.0;
                changed = true;
            }
        }
    });
    if changed && let Some(node) = state.model.get_mut(id) {
        node.kind = NodeKind::Box(edited);
        state.model.dirty.insert(id);
    }
}

/// 取り込んだ部品・アセンブリ: 親から見た位置と回転を編集する
fn placement_editor(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let Some(node) = state.model.get(id) else {
        return;
    };
    let (translation, rotation) = split_placement(node.placement);
    let mut position = translation.to_array();
    let mut rotation_deg = quat_to_euler_deg(rotation);
    let mut changed = xyz_editor(ui, "位置", &mut position, 0.5, None);
    changed |= xyz_editor(ui, "回転 (度)", &mut rotation_deg, 1.0, None);
    if changed && let Some(node) = state.model.get_mut(id) {
        node.placement = DMat4::from_rotation_translation(
            euler_deg_to_quat(rotation_deg),
            DVec3::from(position),
        );
    }
    ui.weak("ビューでは W / E キーのギズモでも動かせます");
}

fn bounds_section(ui: &mut egui::Ui, state: &UiState, id: NodeId) {
    let unit = &state.model.unit;
    let Some(bounds) = state.model.world_bounds(id) else {
        ui.weak("形状がありません");
        return;
    };
    let size = bounds.size();
    let center = bounds.center();
    egui::Grid::new("bounds")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            row(ui, "幅 (X)", length(size.x, unit));
            row(ui, "高さ (Y)", length(size.y, unit));
            row(ui, "奥行 (Z)", length(size.z, unit));
            row(
                ui,
                "中心",
                format!(
                    "{}, {}, {}",
                    round_for_display(center.x),
                    round_for_display(center.y),
                    round_for_display(center.z)
                ),
            );
        });
    ui.weak("ワールド座標 (Y が上) の軸に沿った大きさ");
}

fn measurements_section(ui: &mut egui::Ui, state: &UiState, id: NodeId) {
    let unit = &state.model.unit;
    let m = state.model.measure(id);
    egui::Grid::new("measurements")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            row(ui, "部品数", format!("{}", m.part_count));
            row(ui, "面", format!("{}", m.face_count));
            row(ui, "稜線", format!("{}", m.edge_count));
            row(
                ui,
                "体積",
                format!("{} {unit}³", round_for_display(m.volume)),
            );
            row(
                ui,
                "表面積",
                format!("{} {unit}²", round_for_display(m.area)),
            );
        });
    ui.weak("体積と表面積は表示用メッシュからの近似値");
}
