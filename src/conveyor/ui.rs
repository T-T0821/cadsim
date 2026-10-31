//! コンベアとグリッパの編集画面 (3D ビュー右上、ビューキューブの下の「ワーク搬送」ウィンドウ)

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use cadsim_core::conveyor::{BeltAxis, ConveyorConfig, ConveyorEnd};

use super::{ConveyorConfigRes, WorkState};
use crate::camera::ViewportRect;
use crate::model::{Model, NodeId, Selection};
use crate::motion::{ActuatorKind, TimingChart};

pub fn conveyor_window(
    mut contexts: EguiContexts,
    mut config: ResMut<ConveyorConfigRes>,
    model: Res<Model>,
    selection: Res<Selection>,
    chart: Res<TimingChart>,
    state: Res<WorkState>,
    viewport: Res<ViewportRect>,
) -> Result {
    let Some(view) = viewport.rect else {
        return Ok(());
    };
    // 編集はコピーに対して行い、変わったときだけ書き戻す (毎フレーム計算し直さないため)
    let mut edited = config.0.clone();
    egui::Window::new("ワーク搬送")
        // 3D ビューの右上、ビューキューブの下に浮かべる (左上は干渉チェックのパネル)
        .pivot(egui::Align2::RIGHT_TOP)
        .default_pos([view.max.x - 8.0, view.min.y + 170.0])
        .default_width(300.0)
        .collapsible(true)
        .show(contexts.ctx_mut()?, |ui| {
            let held = state.works().iter().filter(|w| w.is_held()).count();
            ui.label(format!("ワーク {} 個 (把持中 {held})", state.works().len()));
            egui::ScrollArea::vertical()
                .max_height(500.0)
                .show(ui, |ui| {
                    conveyors_ui(ui, &mut edited, &model, selection.node);
                    ui.separator();
                    grippers_ui(ui, &mut edited, &chart);
                });
        });
    if edited != config.0 {
        config.0 = edited;
    }
    Ok(())
}

fn node_name(model: &Model, id: Option<NodeId>) -> String {
    id.and_then(|id| model.get(id))
        .map_or("(未設定)".to_string(), |node| node.name.clone())
}

fn conveyors_ui(
    ui: &mut egui::Ui,
    config: &mut ConveyorConfig,
    model: &Model,
    selected: Option<NodeId>,
) {
    // ベルトの候補: ツリーにあるすべての部品とアセンブリ
    let candidates: Vec<(NodeId, String)> = model
        .roots
        .iter()
        .flat_map(|&root| model.subtree(root))
        .map(|id| (id, model.path_label(id)))
        .collect();
    ui.horizontal(|ui| {
        ui.heading("コンベア");
        if ui.button("+ 追加").clicked() {
            config.add_conveyor();
        }
    });
    let mut remove = None;
    for conveyor in &mut config.conveyors {
        egui::CollapsingHeader::new(&conveyor.name)
            .id_salt(("conveyor", conveyor.id))
            .show(ui, |ui| {
                egui::Grid::new(("conveyor_grid", conveyor.id))
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("名前");
                        ui.text_edit_singleline(&mut conveyor.name);
                        ui.end_row();

                        ui.label("ベルト部品");
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt(("belt", conveyor.id))
                                .selected_text(node_name(model, conveyor.part))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut conveyor.part, None, "(未設定)");
                                    for (id, label) in &candidates {
                                        ui.selectable_value(&mut conveyor.part, Some(*id), label);
                                    }
                                });
                            if ui
                                .add_enabled(selected.is_some(), egui::Button::new("選択中"))
                                .on_hover_text("ツリーで選んでいる部品をベルトにする")
                                .clicked()
                            {
                                conveyor.part = selected;
                            }
                        });
                        ui.end_row();

                        ui.label("流れる向き");
                        ui.horizontal(|ui| {
                            for axis in BeltAxis::ALL {
                                ui.selectable_value(&mut conveyor.axis, axis, axis.label());
                            }
                            ui.checkbox(&mut conveyor.reversed, "逆向き");
                        });
                        ui.end_row();

                        ui.label("速度");
                        ui.add(
                            egui::DragValue::new(&mut conveyor.speed)
                                .speed(0.01)
                                .range(0.0..=20.0)
                                .suffix(" /s"),
                        );
                        ui.end_row();

                        ui.label("終端");
                        ui.horizontal(|ui| {
                            for end in ConveyorEnd::ALL {
                                ui.selectable_value(&mut conveyor.end, end, end.label());
                            }
                        });
                        ui.end_row();

                        ui.label("ワーク供給");
                        ui.horizontal(|ui| {
                            let mut feeds = conveyor.feed_interval.is_some();
                            if ui.checkbox(&mut feeds, "あり").changed() {
                                conveyor.feed_interval = feeds.then_some(3.0);
                            }
                            if let Some(interval) = &mut conveyor.feed_interval {
                                ui.add(
                                    egui::DragValue::new(interval)
                                        .speed(0.05)
                                        .range(0.1..=600.0)
                                        .prefix("間隔 ")
                                        .suffix(" s"),
                                );
                            }
                        });
                        ui.end_row();

                        if conveyor.feed_interval.is_some() {
                            ui.label("ワーク寸法");
                            ui.horizontal(|ui| {
                                for v in &mut conveyor.work_size {
                                    ui.add(egui::DragValue::new(v).speed(0.01).range(0.01..=100.0));
                                }
                            });
                            ui.end_row();
                        }
                    });
                if ui.button("コンベアを削除").clicked() {
                    remove = Some(conveyor.id);
                }
            });
    }
    if let Some(id) = remove {
        config.conveyors.retain(|c| c.id != id);
    }
}

fn grippers_ui(ui: &mut egui::Ui, config: &mut ConveyorConfig, chart: &TimingChart) {
    ui.horizontal(|ui| {
        ui.heading("グリッパ");
        if ui.button("+ 追加").clicked() {
            config.add_gripper();
        }
    });
    ui.weak("チャックはシリンダ。出 = 閉で掴み、戻り始めで離す");
    let cylinders: Vec<_> = chart
        .actuators
        .iter()
        .filter(|a| a.kind == ActuatorKind::Cylinder)
        .collect();
    let mut remove = None;
    for gripper in &mut config.grippers {
        egui::CollapsingHeader::new(&gripper.name)
            .id_salt(("gripper", gripper.id))
            .show(ui, |ui| {
                egui::Grid::new(("gripper_grid", gripper.id))
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("名前");
                        ui.text_edit_singleline(&mut gripper.name);
                        ui.end_row();

                        ui.label("チャック");
                        let selected = gripper
                            .chuck
                            .and_then(|id| chart.actuator(id))
                            .map_or("(未設定)".to_string(), |a| a.name.clone());
                        egui::ComboBox::from_id_salt(("chuck", gripper.id))
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut gripper.chuck, None, "(未設定)");
                                for cylinder in &cylinders {
                                    ui.selectable_value(
                                        &mut gripper.chuck,
                                        Some(cylinder.id),
                                        &cylinder.name,
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("一緒に動く");
                        let mount = gripper
                            .chuck
                            .and_then(|id| chart.actuator(id))
                            .and_then(|a| a.mounted_on)
                            .and_then(|id| chart.actuator(id))
                            .map_or("(固定)".to_string(), |a| a.name.clone());
                        ui.label(mount)
                            .on_hover_text("チャックの「載せ先」はタイミングチャートで設定する");
                        ui.end_row();

                        ui.label("掴む位置 X/Y/Z");
                        ui.horizontal(|ui| {
                            for v in &mut gripper.tcp {
                                ui.add(egui::DragValue::new(v).speed(0.01));
                            }
                        });
                        ui.end_row();

                        ui.label("掴める距離");
                        ui.add(
                            egui::DragValue::new(&mut gripper.reach)
                                .speed(0.01)
                                .range(0.01..=10.0),
                        );
                        ui.end_row();
                    });
                if ui.button("グリッパを削除").clicked() {
                    remove = Some(gripper.id);
                }
            });
    }
    if let Some(id) = remove {
        config.grippers.retain(|g| g.id != id);
    }
}
