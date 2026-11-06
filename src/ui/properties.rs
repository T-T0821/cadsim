//! 右のプロパティパネル: 選択中の部品・アセンブリの情報と編集

use bevy::math::{DMat4, DVec3};
use bevy_egui::egui;

use super::UiState;
use super::widgets::{length, round_for_display, row, xyz_editor};
use crate::features::motion;
use crate::viewport::camera::CameraRequest;
use crate::viewport::gizmo::snap_settings_ui;
use plantwright_core::math::euler_deg_to_quat;
use plantwright_core::math::quat_to_euler_deg;
use plantwright_core::math::split_placement;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;
use plantwright_core::physics::settings::BeltDrive;
use plantwright_core::physics::settings::BodyMode;
use plantwright_core::physics::settings::ColliderShape;
use plantwright_core::physics::settings::LocalAxis;
use plantwright_core::workspace::Tool;

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
                    ui.separator();
                    egui::CollapsingHeader::new("ギズモのスナップ")
                        .default_open(false)
                        .show(ui, |ui| snap_settings_ui(ui, &mut state.gizmo));
                    motion::show_robot_section(ui, &mut state.motion);
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
            // 変わったときだけ書く (毎フレーム書くとモデルが変わった扱いになり、表示を計算し直す)
            if xyz_editor(ui, "寸法 (幅 / 高さ / 奥行)", &mut size, 0.1, Some(0.001)) {
                state.model.new_box_size = size;
            }
            if ui.button("箱を追加").clicked() {
                super::menu::add_box(state);
            }
        });
}

fn selected_node(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    general_section(ui, state, id);
    ui.add_space(4.0);
    actions(ui, state, id);
    modeling_actions(ui, state);
    ui.separator();

    if matches!(
        state.model.get(id).map(|n| &n.kind),
        Some(NodeKind::Standard(_))
    ) {
        egui::CollapsingHeader::new("標準部品")
            .default_open(true)
            .show(ui, |ui| {
                crate::features::library::properties_section(ui, state, id)
            });
    }
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
    if state.model.get(id).is_some_and(|n| n.kind.is_part()) {
        egui::CollapsingHeader::new("物理")
            .default_open(false)
            .show(ui, |ui| physics_section(ui, state, id));
    }
    egui::CollapsingHeader::new("スクリプト")
        .default_open(state.model.get(id).is_some_and(|n| n.script.is_some()))
        .show(ui, |ui| {
            crate::features::scripting::script_section(ui, &mut state.model, &mut state.history, id)
        });
    egui::CollapsingHeader::new("自作部品")
        .default_open(state.model.get(id).is_some_and(|n| n.custom_part.is_some()))
        .show(ui, |ui| {
            let changed = crate::features::custom_part::properties_section(
                ui,
                &state.model,
                state.motion.chart(),
                &mut state.custom_dialog,
                id,
            );
            if let Some((index, value)) = changed {
                crate::features::custom_part::set_param(
                    &mut state.model,
                    state.motion.chart_mut(),
                    id,
                    index,
                    value,
                );
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
    let Some(node) = state.model.get(id) else {
        return;
    };
    // 写しを編集し、変わったときだけモデルに書き戻す
    // (毎フレーム書くとモデルが変わった扱いになり、数千部品の表示を計算し直してしまう)
    let mut name = node.name.clone();
    let mut visible = node.visible;
    let mut color = node.color;
    let mut skip_inner = node.skip_inner_interference;
    ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
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
            ui.checkbox(&mut visible, "表示する");
            ui.end_row();

            // 標準部品の本体どうしは、もともと調べない
            if !node.children.is_empty() && !matches!(node.kind, NodeKind::Standard(_)) {
                ui.weak("干渉");
                ui.checkbox(&mut skip_inner, "中の部品どうしは調べない")
                    .on_hover_text(
                        "メーカーの CAD を取り込んだ購入品のように、組み付いた状態で形状が重なっている\n\
                         アセンブリに付けます。中の部品とほかの部品との干渉は調べます。",
                    );
                ui.end_row();
            }

            ui.weak("色");
            ui.horizontal(|ui| {
                let mut rgb = color.unwrap_or(match &node.kind {
                    NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
                        shape.color.unwrap_or(plantwright_core::model::DEFAULT_PART_COLOR)
                    }
                    _ => plantwright_core::model::DEFAULT_PART_COLOR,
                });
                if ui.color_edit_button_rgb(&mut rgb).changed() {
                    color = Some(rgb);
                }
                if color.is_some() && ui.small_button("元に戻す").clicked() {
                    color = None;
                }
            });
            ui.end_row();
        });
    let named_or_colored =
        name != node.name || color != node.color || skip_inner != node.skip_inner_interference;
    let visibility_changed = visible != node.visible;
    // 名前・色・干渉の設定は履歴に載せる (変更前のモデルを覚えさせてから書く)
    if named_or_colored {
        state.history.begin_edit(&state.model, id);
    }
    if (named_or_colored || visibility_changed)
        && let Some(node) = state.model.get_mut(id)
    {
        node.name = name;
        node.visible = visible;
        node.color = color;
        node.skip_inner_interference = skip_inner;
    }
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
            state.history.remove(&mut state.model, id);
            state.selection.clear();
        }
    });
}

/// 形を編集する道具 (箱とモデリングで作った部品だけ): 穴あけは 3D で面をクリック、ブーリアンはパネルで
fn modeling_actions(ui: &mut egui::Ui, state: &mut UiState) {
    if !super::menu::part_can_be_modeled(state) {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.label("形を編集:");
        let drilling = state.hole_tool.active;
        if ui
            .selectable_label(drilling, "穴をあける")
            .on_hover_text(
                "3D ビューで面をクリックし、丸い点をドラッグで直径、矢印の先で深さ。Enter で確定",
            )
            .clicked()
        {
            if drilling {
                state.hole_tool.stop();
            } else {
                crate::features::modeling::panel::start_hole_tool(state);
            }
        }
        if ui
            .button("ブーリアン…")
            .on_hover_text("ほかの部品と 和 / 差 / 積")
            .clicked()
        {
            state
                .modeling
                .open_tab(crate::features::modeling::panel::Tab::Boolean);
            state.dock.set_open(Tool::Modeling, true);
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
    if changed {
        state.history.begin_edit(&state.model, id);
    }
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
    if changed {
        state.history.begin_edit(&state.model, id);
    }
    if changed && let Some(node) = state.model.get_mut(id) {
        node.placement = DMat4::from_rotation_translation(
            euler_deg_to_quat(rotation_deg),
            DVec3::from(position),
        );
    }
    ui.weak("ビューでは W / E キーのギズモでも動かせます");
}

/// 物理演算での振る舞い・当たり判定の形・摩擦などと、ベルトコンベアの設定
fn physics_section(ui: &mut egui::Ui, state: &mut UiState, id: NodeId) {
    let Some(node) = state.model.get(id) else {
        return;
    };
    let unit = state.model.unit.clone();
    let before = node.physics;
    let mut p = before;
    egui::Grid::new("physics")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.weak("振る舞い");
            egui::ComboBox::from_id_salt("physics_mode")
                .selected_text(p.mode.label())
                .show_ui(ui, |ui| {
                    for mode in BodyMode::ALL {
                        ui.selectable_value(&mut p.mode, mode, mode.label());
                    }
                });
            ui.end_row();
            if p.mode == BodyMode::Ignored {
                return;
            }
            ui.weak("当たり判定");
            egui::ComboBox::from_id_salt("physics_shape")
                .selected_text(p.shape.label())
                .show_ui(ui, |ui| {
                    for shape in ColliderShape::ALL {
                        ui.selectable_value(&mut p.shape, shape, shape.label());
                    }
                });
            ui.end_row();
            ui.weak("摩擦係数");
            ui.add(
                egui::DragValue::new(&mut p.friction)
                    .speed(0.01)
                    .range(0.0..=2.0),
            );
            ui.end_row();
            ui.weak("反発係数");
            ui.add(
                egui::DragValue::new(&mut p.restitution)
                    .speed(0.01)
                    .range(0.0..=1.0),
            );
            ui.end_row();
            if p.mode == BodyMode::Dynamic {
                ui.weak("密度");
                ui.add(
                    egui::DragValue::new(&mut p.density)
                        .speed(0.05)
                        .range(0.01..=30.0)
                        .suffix(" g/cm³"),
                );
                ui.end_row();
            }
        });
    if p.mode != BodyMode::Ignored && p.mode != BodyMode::Dynamic {
        let mut is_belt = p.belt.is_some();
        ui.checkbox(&mut is_belt, "ベルトコンベア (上に載ったものを運ぶ)");
        if is_belt != p.belt.is_some() {
            p.belt = is_belt.then(BeltDrive::default);
        }
        if let Some(belt) = &mut p.belt {
            ui.horizontal(|ui| {
                ui.weak("向き");
                for axis in LocalAxis::ALL {
                    ui.selectable_value(&mut belt.axis, axis, axis.label());
                }
                ui.checkbox(&mut belt.reversed, "逆向き");
            });
            ui.horizontal(|ui| {
                ui.weak("速度");
                ui.add(
                    egui::DragValue::new(&mut belt.speed)
                        .speed(1.0)
                        .range(0.0..=f64::MAX)
                        .suffix(format!(" {unit}/s")),
                );
            });
        }
    }
    if p.shape == ColliderShape::ConvexDecomposition {
        ui.weak("凸分解は大きな部品だと開始に時間がかかります");
    }
    ui.weak("物理演算は 3D ビュー左下の「物理演算」パネルで開始します");
    if p != before {
        state.history.begin_edit(&state.model, id);
        if let Some(node) = state.model.get_mut(id) {
            node.physics = p;
        }
    }
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
