//! 3D ビュー左下の「平面図・点群」パネル: 下敷きの一覧・配置・縮尺合わせと、
//! 点群の一覧・配置・表示の細かさ・設備との干渉の結果

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cadsim_core::workspace::Tool;

use super::cloud_view::{CloudCheck, CloudDisplay};
use super::{PlanTool, format_count};
use crate::camera::ViewportRect;
use crate::drawing::format_length;
use crate::model::Model;
use crate::simulation::SimBody;

#[allow(clippy::too_many_arguments)]
pub fn site_panel(
    mut contexts: EguiContexts,
    mut model: ResMut<Model>,
    mut tool: ResMut<PlanTool>,
    mut display: ResMut<CloudDisplay>,
    mut check: ResMut<CloudCheck>,
    bodies: Query<(&SimBody, Option<&Name>)>,
    viewport: Res<ViewportRect>,
    mut dock: ResMut<Dock>,
) -> Result {
    if model.site.is_empty() {
        dock.set_open(Tool::Site, false);
        if tool.calibrating.is_some() {
            tool.stop();
        }
        return Ok(());
    }
    let Some(window) = dock::panel(Tool::Site, &dock, viewport.rect) else {
        if tool.calibrating.is_some() {
            tool.stop();
        }
        return Ok(());
    };
    let unit = model.unit.clone();
    let model = model.bypass_change_detection();
    let tool = tool.bypass_change_detection();
    let mut open = true;
    window.open(&mut open).show(contexts.ctx_mut()?, |ui| {
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
        if !model.site.clouds.is_empty() {
            if count > 0 {
                ui.separator();
            }
            let names = |id: u32| {
                bodies
                    .iter()
                    .find(|(b, _)| b.id == id)
                    .and_then(|(_, n)| n.map(|n| n.to_string()))
                    .unwrap_or_else(|| format!("#{id}"))
            };
            clouds_ui(
                ui,
                model,
                display.bypass_change_detection(),
                check.bypass_change_detection(),
                &unit,
                &names,
            );
        }
    });
    if !open {
        dock.set_open(Tool::Site, false);
    }
    Ok(())
}

fn clouds_ui(
    ui: &mut egui::Ui,
    model: &mut Model,
    display: &mut CloudDisplay,
    check: &mut CloudCheck,
    unit: &str,
    names: &dyn Fn(u32) -> String,
) {
    let mut remove = None;
    for (index, placed) in model.site.clouds.iter_mut().enumerate() {
        ui.push_id(("cloud", index), |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut placed.visible, "")
                    .on_hover_text("表示 / 非表示");
                ui.strong(&placed.name);
                if ui.small_button("削除").clicked() {
                    remove = Some(index);
                }
            });
            ui.weak(format!(
                "{} 点 → {} 点 (間隔 {} {unit})",
                format_count(placed.cloud.source_points),
                format_count(placed.cloud.points.len() as u64),
                format_length(placed.cloud.voxel),
            ));
            egui::Grid::new("cloud").num_columns(2).show(ui, |ui| {
                ui.label("位置 X / Y / Z");
                ui.horizontal(|ui| {
                    for v in &mut placed.offset {
                        ui.add(egui::DragValue::new(v).speed(10.0));
                    }
                });
                ui.end_row();
                ui.label("回転");
                ui.add(
                    egui::DragValue::new(&mut placed.rotation_deg)
                        .speed(0.5)
                        .suffix("°"),
                );
                ui.end_row();
            });
        });
    }
    if let Some(index) = remove {
        model.site.remove_cloud(index);
    }
    ui.horizontal(|ui| {
        ui.label("表示の細かさ");
        ui.add(egui::Slider::new(&mut display.detail, 0.25..=4.0).logarithmic(true))
            .on_hover_text("大きくすると遠くの点まで細かく描く (重くなる)");
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.strong("点群との干渉");
        ui.checkbox(&mut check.auto, "自動");
        if ui.button("今すぐチェック").clicked() {
            check.request();
        }
        if check.running {
            ui.spinner();
        }
    });
    ui.horizontal(|ui| {
        ui.label("近接の警告距離");
        ui.add(
            egui::DragValue::new(&mut check.warning)
                .speed(1.0)
                .range(0.0..=1e7),
        );
        ui.label(unit);
    });
    if check.hits.is_empty() {
        ui.weak("点群とぶつかる・近づく部品はありません");
    }
    egui::ScrollArea::vertical()
        .max_height(160.0)
        .show(ui, |ui| {
            for (_, hit) in &check.hits {
                let name = names(hit.body);
                if hit.inside > 0 {
                    ui.colored_label(
                        egui::Color32::from_rgb(240, 60, 60),
                        format!("干渉  {name}: 内側に {} 点", hit.inside),
                    );
                } else if let Some(d) = hit.distance() {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 200, 40),
                        format!("近接  {name}: {} {unit}", format_length(d)),
                    );
                }
            }
        });
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
