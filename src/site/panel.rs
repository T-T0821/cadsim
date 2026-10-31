//! 3D ビュー左下の「平面図」パネル: 下敷きの一覧・配置・縮尺合わせ

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::PlanTool;
use crate::camera::ViewportRect;
use crate::model::Model;

pub fn site_panel(
    mut contexts: EguiContexts,
    mut model: ResMut<Model>,
    mut tool: ResMut<PlanTool>,
    viewport: Res<ViewportRect>,
) -> Result {
    if model.site.is_empty() {
        tool.open = false;
        if tool.calibrating.is_some() {
            tool.stop();
        }
        return Ok(());
    }
    let corner = viewport.rect.map_or(egui::pos2(8.0, 400.0), |r| {
        egui::pos2(r.min.x + 8.0, r.max.y - 8.0)
    });
    let unit = model.unit.clone();
    let model = model.bypass_change_detection();
    let tool = tool.bypass_change_detection();
    egui::Window::new("平面図")
        .pivot(egui::Align2::LEFT_BOTTOM)
        .default_pos(corner)
        .resizable(false)
        .collapsible(true)
        .default_open(true)
        .show(contexts.ctx_mut()?, |ui| {
            let mut remove = None;
            let count = model.site.underlays.len();
            for index in 0..count {
                let underlay = &mut model.site.underlays[index];
                ui.push_id(index, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut underlay.visible, "")
                            .on_hover_text("表示 / 非表示");
                        ui.strong(&underlay.name);
                        if ui.small_button("削除").clicked() {
                            remove = Some(index);
                        }
                    });
                    egui::Grid::new("underlay").num_columns(2).show(ui, |ui| {
                        ui.label("縮尺");
                        ui.horizontal(|ui| {
                            ui.label("図面の 1 =");
                            let speed = underlay.scale * 0.005;
                            ui.add(
                                egui::DragValue::new(&mut underlay.scale)
                                    .speed(speed)
                                    .range(1e-9..=1e9)
                                    .max_decimals(6),
                            );
                            ui.label(&unit);
                        });
                        ui.end_row();
                        ui.label("原点 X / Z");
                        ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut underlay.origin[0]).speed(10.0));
                            ui.add(egui::DragValue::new(&mut underlay.origin[1]).speed(10.0));
                        });
                        ui.end_row();
                        ui.label("回転");
                        ui.add(
                            egui::DragValue::new(&mut underlay.rotation_deg)
                                .speed(0.5)
                                .suffix("°"),
                        );
                        ui.end_row();
                        ui.label("高さ (Y)");
                        ui.add(egui::DragValue::new(&mut underlay.elevation).speed(1.0));
                        ui.end_row();
                        ui.label("不透明度");
                        ui.add(egui::Slider::new(&mut underlay.opacity, 0.05..=1.0));
                        ui.end_row();
                    });

                    if tool.calibrating == Some(index) {
                        calibration_ui(ui, tool, underlay, &unit);
                    } else if ui
                        .button("📏 縮尺合わせ")
                        .on_hover_text("図面上の2点をクリックし、その実際の距離を入れて縮尺を合わせる")
                        .clicked()
                    {
                        tool.start(index);
                    }
                });
                if index + 1 < count {
                    ui.separator();
                }
            }
            if let Some(index) = remove {
                if tool.calibrating.is_some() {
                    tool.stop();
                }
                model.site.remove_underlay(index);
            }
        });
    Ok(())
}

fn calibration_ui(
    ui: &mut egui::Ui,
    tool: &mut PlanTool,
    underlay: &mut super::underlay::Underlay,
    unit: &str,
) {
    ui.group(|ui| {
        ui.strong("縮尺合わせ");
        match tool.points.len() {
            0 => {
                ui.label("1点目をクリック (線の端に吸い付きます)");
            }
            1 => {
                ui.label("2点目をクリック (Esc で1点目をやり直し)");
            }
            _ => {
                let shown = (tool.points[1] - tool.points[0]).length();
                ui.label(format!("いまの2点間: {shown:.1} {unit}"));
                ui.horizontal(|ui| {
                    ui.label("実際の距離");
                    ui.add(
                        egui::DragValue::new(&mut tool.distance)
                            .speed(1.0)
                            .range(1e-6..=1e9),
                    );
                    ui.label(unit);
                });
                ui.checkbox(&mut tool.align_x, "1点目 → 2点目 の向きを X 軸に合わせる");
                ui.checkbox(&mut tool.move_to_origin, "1点目を原点 (0, 0) に移す");
            }
        }
        ui.horizontal(|ui| {
            let ready = tool.points.len() == 2;
            if ui.add_enabled(ready, egui::Button::new("適用")).clicked() {
                tool.apply(underlay);
            }
            if ui.button("やめる").clicked() {
                tool.stop();
            }
        });
    });
}
