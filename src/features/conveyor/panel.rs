//! ワーク搬送の編集画面 (3D ビュー右上、ビューキューブの下の「ワーク搬送」ウィンドウ)

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use plantwright_core::workspace::Tool;

use plantwright_core::conveyor::{
    BeltAxis, ConveyorConfig, ConveyorEnd, Counts, Exit, SourceOutput,
};

use super::{ConveyorConfigRes, WorkState};
use crate::document::selection::Selection;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::motion::chart::ActuatorKind;
use plantwright_core::motion::chart::TimingChart;

/// 長さの入力の上限 (モデル単位)
const MAX_LENGTH: f64 = 100_000.0;

#[allow(clippy::too_many_arguments)]
pub fn conveyor_window(
    mut contexts: EguiContexts,
    mut config: ResMut<ConveyorConfigRes>,
    model: Res<Model>,
    selection: Res<Selection>,
    chart: Res<TimingChart>,
    state: Res<WorkState>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(contexts.ctx_mut()?, Tool::Conveyor, &dock) else {
        return Ok(());
    };
    // 編集はコピーに対して行い、変わったときだけ書き戻す (毎フレーム計算し直さないため)
    let mut edited = config.0.clone();
    let parts = PartPicker::new(&model, selection.node);
    let mut open = true;
    window.open(&mut open).show(contexts.ctx_mut()?, |ui| {
        counts_ui(ui, &edited, &state);
        ui.separator();
        egui::ScrollArea::vertical()
            .max_height(480.0)
            .show(ui, |ui| {
                kinds_ui(ui, &mut edited);
                ui.separator();
                sources_ui(ui, &mut edited, &parts);
                ui.separator();
                sinks_ui(ui, &mut edited, &parts);
                ui.separator();
                conveyors_ui(ui, &mut edited, &parts);
                ui.separator();
                grippers_ui(ui, &mut edited, &chart);
            });
    });
    if !open {
        dock.set_open(Tool::Conveyor, false);
    }
    if edited != config.0 {
        config.0 = edited;
    }
    Ok(())
}

/// 投入数・排出数・仕掛数
fn counts_ui(ui: &mut egui::Ui, config: &ConveyorConfig, state: &WorkState) {
    let counts: &Counts = state.counts();
    let held = state.works().iter().filter(|w| w.is_held()).count();
    ui.label(format!(
        "投入 {} / 排出 {} / 仕掛 {} (把持中 {held})",
        counts.total_fed(),
        counts.total_discharged(),
        state.in_process(),
    ));
    if counts.total_fed() == 0 {
        return;
    }
    egui::CollapsingHeader::new("内訳")
        .id_salt("work_counts")
        .show(ui, |ui| {
            egui::Grid::new("work_counts_grid")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    ui.strong("");
                    ui.strong("投入");
                    ui.strong("排出");
                    ui.end_row();
                    for kind in &config.kinds {
                        let (fed, out) = counts.by_kind.get(&kind.id).copied().unwrap_or((0, 0));
                        ui.label(format!("種類: {}", kind.name));
                        ui.label(fed.to_string());
                        ui.label(out.to_string());
                        ui.end_row();
                    }
                    for source in &config.sources {
                        let fed = counts.fed.get(&source.id).copied().unwrap_or(0);
                        ui.label(&source.name);
                        ui.label(fed.to_string());
                        ui.label("");
                        ui.end_row();
                    }
                    for sink in &config.sinks {
                        let out = counts.discharged.get(&Exit::Sink(sink.id)).copied();
                        ui.label(&sink.name);
                        ui.label("");
                        ui.label(out.unwrap_or(0).to_string());
                        ui.end_row();
                    }
                    for conveyor in &config.conveyors {
                        if conveyor.end != ConveyorEnd::Discharge {
                            continue;
                        }
                        let out = counts.discharged.get(&Exit::Conveyor(conveyor.id)).copied();
                        ui.label(format!("{} の終端", conveyor.name));
                        ui.label("");
                        ui.label(out.unwrap_or(0).to_string());
                        ui.end_row();
                    }
                });
        });
}

/// 部品を選ぶ欄 (一覧から選ぶか、ツリーで選んでいる部品を使う)
struct PartPicker {
    candidates: Vec<(NodeId, String)>,
    names: std::collections::HashMap<NodeId, String>,
    selected: Option<NodeId>,
}

impl PartPicker {
    fn new(model: &Model, selected: Option<NodeId>) -> Self {
        // 候補: ツリーにあるすべての部品とアセンブリ
        let candidates: Vec<(NodeId, String)> = model
            .roots
            .iter()
            .flat_map(|&root| model.subtree(root))
            .map(|id| (id, model.path_label(id)))
            .collect();
        let names = candidates
            .iter()
            .filter_map(|(id, _)| model.get(*id).map(|n| (*id, n.name.clone())))
            .collect();
        Self {
            candidates,
            names,
            selected,
        }
    }

    fn show(&self, ui: &mut egui::Ui, salt: (&str, u32), part: &mut Option<NodeId>) {
        ui.horizontal(|ui| {
            let text = part
                .and_then(|id| self.names.get(&id).cloned())
                .unwrap_or_else(|| "(未設定)".to_string());
            egui::ComboBox::from_id_salt(salt)
                .selected_text(text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(part, None, "(未設定)");
                    for (id, label) in &self.candidates {
                        ui.selectable_value(part, Some(*id), label);
                    }
                });
            if ui
                .add_enabled(self.selected.is_some(), egui::Button::new("選択中"))
                .on_hover_text("ツリーで選んでいる部品を使う")
                .clicked()
            {
                *part = self.selected;
            }
        });
    }
}

fn header(ui: &mut egui::Ui, title: &str) -> bool {
    ui.horizontal(|ui| {
        ui.heading(title);
        ui.button("+ 追加").clicked()
    })
    .inner
}

fn length(value: &mut f64) -> egui::DragValue<'_> {
    egui::DragValue::new(value)
        .speed(1.0)
        .range(0.0..=MAX_LENGTH)
}

fn kinds_ui(ui: &mut egui::Ui, config: &mut ConveyorConfig) {
    if header(ui, "ワーク種類") {
        config.add_kind();
    }
    let mut remove = None;
    for kind in &mut config.kinds {
        ui.horizontal(|ui| {
            ui.color_edit_button_rgb(&mut kind.color);
            ui.add(egui::TextEdit::singleline(&mut kind.name).desired_width(80.0));
            for v in &mut kind.size {
                ui.add(egui::DragValue::new(v).speed(1.0).range(0.1..=MAX_LENGTH));
            }
            if ui.small_button("削除").clicked() {
                remove = Some(kind.id);
            }
        });
    }
    if !config.kinds.is_empty() {
        ui.weak("寸法は 幅 / 高さ / 奥行");
    }
    if let Some(id) = remove {
        config.remove_kind(id);
    }
}

fn sources_ui(ui: &mut egui::Ui, config: &mut ConveyorConfig, parts: &PartPicker) {
    if header(ui, "投入口") {
        config.add_source();
    }
    let kinds: Vec<(u32, String)> = config
        .kinds
        .iter()
        .map(|k| (k.id, k.name.clone()))
        .collect();
    let conveyors: Vec<(u32, String)> = config
        .conveyors
        .iter()
        .map(|c| (c.id, c.name.clone()))
        .collect();
    let mut remove = None;
    for source in &mut config.sources {
        egui::CollapsingHeader::new(&source.name)
            .id_salt(("source", source.id))
            .show(ui, |ui| {
                egui::Grid::new(("source_grid", source.id))
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label("名前");
                        ui.text_edit_singleline(&mut source.name);
                        ui.end_row();

                        ui.label("部品");
                        parts.show(ui, ("source_part", source.id), &mut source.part);
                        ui.end_row();

                        ui.label("出す先");
                        let current = match source.output {
                            SourceOutput::OnPart => "部品の上".to_string(),
                            SourceOutput::Conveyor(id) => conveyors
                                .iter()
                                .find(|(c, _)| *c == id)
                                .map_or("(なし)".to_string(), |(_, n)| format!("{n} の始端")),
                        };
                        egui::ComboBox::from_id_salt(("source_output", source.id))
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut source.output,
                                    SourceOutput::OnPart,
                                    "部品の上",
                                );
                                for (id, name) in &conveyors {
                                    ui.selectable_value(
                                        &mut source.output,
                                        SourceOutput::Conveyor(*id),
                                        format!("{name} の始端"),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("間隔");
                        ui.add(
                            egui::DragValue::new(&mut source.interval)
                                .speed(0.05)
                                .range(0.1..=3600.0)
                                .suffix(" s"),
                        );
                        ui.end_row();

                        ui.label("種類");
                        ui.vertical(|ui| {
                            for (id, name) in &kinds {
                                let mut on = source.kinds.contains(id);
                                if ui.checkbox(&mut on, name).changed() {
                                    if on {
                                        source.kinds.push(*id);
                                    } else {
                                        source.kinds.retain(|k| k != id);
                                    }
                                }
                            }
                            ui.weak("複数選ぶと順番に出す");
                        });
                        ui.end_row();

                        ui.label("個数");
                        ui.horizontal(|ui| {
                            let mut limited = source.limit.is_some();
                            if ui.checkbox(&mut limited, "上限あり").changed() {
                                source.limit = limited.then_some(10);
                            }
                            if let Some(limit) = &mut source.limit {
                                ui.add(
                                    egui::DragValue::new(limit).range(1..=100_000).suffix(" 個"),
                                );
                            }
                        });
                        ui.end_row();
                    });
                if ui.button("投入口を削除").clicked() {
                    remove = Some(source.id);
                }
            });
    }
    if let Some(id) = remove {
        config.sources.retain(|s| s.id != id);
    }
}

fn sinks_ui(ui: &mut egui::Ui, config: &mut ConveyorConfig, parts: &PartPicker) {
    if header(ui, "排出口") {
        config.add_sink();
    }
    ui.weak("部品に触れたワークを消して数える");
    let mut remove = None;
    for sink in &mut config.sinks {
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut sink.name).desired_width(80.0));
            parts.show(ui, ("sink_part", sink.id), &mut sink.part);
            if ui.small_button("削除").clicked() {
                remove = Some(sink.id);
            }
        });
    }
    if let Some(id) = remove {
        config.sinks.retain(|s| s.id != id);
    }
}

fn conveyors_ui(ui: &mut egui::Ui, config: &mut ConveyorConfig, parts: &PartPicker) {
    if header(ui, "コンベア") {
        config.add_conveyor();
    }
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
                        parts.show(ui, ("belt", conveyor.id), &mut conveyor.part);
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
                        ui.add(length(&mut conveyor.speed).suffix(" /s"));
                        ui.end_row();

                        ui.label("終端");
                        ui.horizontal(|ui| {
                            for end in ConveyorEnd::ALL {
                                ui.selectable_value(&mut conveyor.end, end, end.label());
                            }
                        });
                        ui.end_row();
                    });
                if ui.button("コンベアを削除").clicked() {
                    remove = Some(conveyor.id);
                }
            });
    }
    if let Some(id) = remove {
        config.remove_conveyor(id);
    }
}

fn grippers_ui(ui: &mut egui::Ui, config: &mut ConveyorConfig, chart: &TimingChart) {
    if header(ui, "グリッパ") {
        config.add_gripper();
    }
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
                                ui.add(egui::DragValue::new(v).speed(1.0));
                            }
                        });
                        ui.end_row();

                        ui.label("掴める距離");
                        ui.add(length(&mut gripper.reach));
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
