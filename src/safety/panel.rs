//! 「安全」パネル: 安全機器の追加・編集、停止信号とリセット、動作範囲と距離の確認

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use plantwright_core::workspace::Tool;

use super::{SafetyLayoutRes, SafetyState};
use crate::ui::widgets;
use crate::view::camera::ViewportRect;
use plantwright_core::model::Model;
use plantwright_core::safety::device::DeviceKind;

pub fn safety_panel(
    mut contexts: EguiContexts,
    mut layout: ResMut<SafetyLayoutRes>,
    mut state: ResMut<SafetyState>,
    model: Res<Model>,
    viewport: Res<ViewportRect>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(Tool::Safety, &dock, viewport.rect) else {
        return Ok(());
    };
    let mut open = true;
    window.open(&mut open).show(contexts.ctx_mut()?, |ui| {
        // 毎フレームの変更検知を避けるため、編集したときだけ変更を知らせる
        let state = state.bypass_change_detection();
        let mut layout_changed = false;
        status_section(ui, state);
        ui.separator();
        layout_changed |= device_list(ui, layout.bypass_change_detection(), state, &model);
        ui.separator();
        clearance_section(ui, &layout, state);
        if layout_changed {
            layout.set_changed();
        }
    });
    if !open {
        dock.set_open(Tool::Safety, false);
    }
    Ok(())
}

fn status_section(ui: &mut egui::Ui, state: &mut SafetyState) {
    ui.horizontal(|ui| {
        if state.is_stopped() {
            ui.colored_label(egui::Color32::from_rgb(230, 60, 60), "● 安全停止中");
        } else {
            ui.colored_label(egui::Color32::from_rgb(60, 200, 90), "● 運転可");
        }
        if ui
            .add_enabled(state.is_stopped(), egui::Button::new("リセット"))
            .on_hover_text("侵入がなくなり、非常停止ボタンを戻してから押す")
            .clicked()
        {
            state.reset();
        }
    });
    for cause in &state.monitor.causes {
        ui.weak(format!("停止の原因: {}", cause.cause));
    }
    if let Some(message) = &state.message {
        ui.colored_label(egui::Color32::from_rgb(230, 160, 60), message);
    }
    ui.checkbox(&mut state.detect_machine, "機械の可動部も検知の対象にする")
        .on_hover_text("タイミングチャートで動く部品が検知範囲に入っても停止する");
}

/// 機器の追加・一覧・選んだ機器の編集。配置が変わったら true。
fn device_list(
    ui: &mut egui::Ui,
    layout: &mut plantwright_core::safety::device::SafetyLayout,
    state: &mut SafetyState,
    model: &Model,
) -> bool {
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("追加:");
        for kind in DeviceKind::ALL {
            if ui.button(kind.label()).clicked() {
                // モデルの手前 (Z の大きい側) の床に置く
                let floor = model
                    .scene_bounds()
                    .map_or([0.0, 0.0], |b| [b.center().x, b.max.z + 500.0]);
                state.selected = Some(layout.add(kind, floor));
                changed = true;
            }
        }
    });

    let mut remove = None;
    egui::ScrollArea::vertical()
        .max_height(140.0)
        .show(ui, |ui| {
            for device in &layout.devices {
                ui.horizontal(|ui| {
                    let mark = if state.is_detecting(device.id) {
                        egui::RichText::new("●").color(egui::Color32::from_rgb(230, 60, 60))
                    } else {
                        egui::RichText::new("●").color(egui::Color32::from_rgb(60, 200, 90))
                    };
                    ui.label(mark);
                    if ui
                        .selectable_label(state.selected == Some(device.id), &device.name)
                        .clicked()
                    {
                        state.selected = Some(device.id);
                    }
                    match device.kind {
                        DeviceKind::EmergencyStop => {
                            let pressed = state.pressed.contains(&device.id);
                            if ui.button(if pressed { "戻す" } else { "押す" }).clicked() {
                                if pressed {
                                    state.pressed.remove(&device.id);
                                } else {
                                    state.pressed.insert(device.id);
                                }
                            }
                        }
                        kind if kind.detects() => {
                            let mut blocked = state.blocked.contains(&device.id);
                            if ui
                                .checkbox(&mut blocked, "遮光")
                                .on_hover_text("検知したことにして停止を試す")
                                .changed()
                            {
                                if blocked {
                                    state.blocked.insert(device.id);
                                } else {
                                    state.blocked.remove(&device.id);
                                }
                            }
                        }
                        _ => {}
                    }
                });
            }
        });

    if let Some(device) = state.selected.and_then(|id| layout.get_mut(id)) {
        ui.separator();
        ui.horizontal(|ui| {
            ui.weak(device.kind.label());
            changed |= ui.text_edit_singleline(&mut device.name).changed();
        });
        let mut floor = [device.position[0], device.position[2]];
        ui.horizontal(|ui| {
            ui.weak("位置 X");
            changed |= ui
                .add(egui::DragValue::new(&mut floor[0]).speed(10.0))
                .changed();
            ui.weak("Z");
            changed |= ui
                .add(egui::DragValue::new(&mut floor[1]).speed(10.0))
                .changed();
            ui.weak("高さ");
            changed |= ui
                .add(egui::DragValue::new(&mut device.position[1]).speed(10.0))
                .changed();
        });
        device.position[0] = floor[0];
        device.position[2] = floor[1];
        ui.horizontal(|ui| {
            ui.weak("向き");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut device.yaw_deg)
                        .speed(1.0)
                        .suffix("°"),
                )
                .changed();
        });
        let size_label = match device.kind {
            DeviceKind::LightCurtain => "幅・高さ・厚み (mm)",
            DeviceKind::AreaSensor => "幅・検知する高さ・奥行き (mm)",
            DeviceKind::Fence => "長さ・高さ・厚み (mm)",
            DeviceKind::EmergencyStop => "太さ・高さ・太さ (mm)",
        };
        changed |= widgets::xyz_editor(ui, size_label, &mut device.size, 10.0, Some(1.0));
        match device.kind {
            DeviceKind::LightCurtain | DeviceKind::AreaSensor => {
                ui.horizontal(|ui| {
                    ui.weak("停止までの時間");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut device.stop_time)
                                .speed(0.01)
                                .range(0.0..=10.0)
                                .suffix(" s"),
                        )
                        .on_hover_text("センサの応答時間 + 機械が止まるまでの時間")
                        .changed();
                });
                if device.kind == DeviceKind::LightCurtain {
                    ui.horizontal(|ui| {
                        ui.weak("検出能力");
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut device.resolution)
                                    .speed(1.0)
                                    .range(1.0..=500.0)
                                    .suffix(" mm"),
                            )
                            .on_hover_text("検知できる最小の物の太さ (指 14 mm、手 30 mm など)")
                            .changed();
                    });
                }
                if let Some(s) = device.safety_distance() {
                    ui.weak(format!(
                        "必要な安全距離 S = {} mm",
                        widgets::round_for_display(s)
                    ));
                }
            }
            DeviceKind::Fence => {
                ui.horizontal(|ui| {
                    ui.weak("動作範囲から離す距離");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut device.required_clearance)
                                .speed(10.0)
                                .range(0.0..=10000.0)
                                .suffix(" mm"),
                        )
                        .changed();
                });
            }
            DeviceKind::EmergencyStop => {}
        }
        if ui.button("この機器を削除").clicked() {
            remove = Some(device.id);
        }
    }
    if let Some(id) = remove {
        layout.remove(id);
        state.blocked.remove(&id);
        state.pressed.remove(&id);
        state.selected = None;
        changed = true;
    }
    changed
}

fn clearance_section(ui: &mut egui::Ui, layout: &SafetyLayoutRes, state: &mut SafetyState) {
    ui.checkbox(&mut state.show_envelopes, "動作範囲を表示 (半透明の橙)");
    if state.clearances.is_empty() {
        ui.weak("安全柵・ライトカーテンと、タイミングチャートで動く部品があると距離を確かめます");
        return;
    }
    egui::Grid::new("safety_clearance")
        .striped(true)
        .show(ui, |ui| {
            ui.strong("機器");
            ui.strong("近い動作範囲");
            ui.strong("距離");
            ui.strong("必要");
            ui.strong("");
            ui.end_row();
            for c in &state.clearances {
                let device = layout.get(c.device).map_or("?", |d| d.name.as_str());
                let envelope = state
                    .envelopes
                    .iter()
                    .find(|e| e.actuator == c.actuator)
                    .map_or("?", |e| e.name.as_str());
                ui.label(device);
                ui.label(envelope);
                ui.label(format!("{} mm", widgets::round_for_display(c.distance)));
                ui.label(format!("{} mm", widgets::round_for_display(c.required)));
                if c.is_ok() {
                    ui.colored_label(egui::Color32::from_rgb(60, 200, 90), "OK");
                } else {
                    ui.colored_label(egui::Color32::from_rgb(230, 60, 60), "近すぎ");
                }
                ui.end_row();
            }
        });
}
