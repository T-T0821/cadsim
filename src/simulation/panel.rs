//! 右上の「シミュレーション」パネル: 干渉チェックの設定・結果一覧と、距離計測の一覧

use crate::ui::dock::{self, Dock};
use plantwright_core::workspace::Tool;
use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use crate::view::camera::ViewportRect;

use super::{
    InterferenceCheck, SimBody,
    measure::{MeasureTool, label_text},
    view::{CONTACT_COLOR, INTERFERENCE_COLOR, NEAR_COLOR},
};
use plantwright_core::simulation::interference::PairStatus;

pub fn simulation_panel(
    mut contexts: EguiContexts,
    mut check: ResMut<InterferenceCheck>,
    mut measure: ResMut<MeasureTool>,
    bodies: Query<(&SimBody, Option<&Name>)>,
    viewport: Res<ViewportRect>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(Tool::Interference, &dock, viewport.rect) else {
        return Ok(());
    };
    let names: HashMap<u32, String> = bodies
        .iter()
        .map(|(body, name)| {
            let name = name.map_or_else(|| format!("#{}", body.id), |n| n.to_string());
            (body.id, name)
        })
        .collect();

    let mut open = true;
    window.open(&mut open).show(contexts.ctx_mut()?, |ui| {
        // 毎フレームの変更検知を避けるため、編集は一度コピーしてから書き戻す
        interference_section(ui, check.bypass_change_detection(), &names);
        ui.separator();
        measure_section(ui, measure.bypass_change_detection());
    });
    if !open {
        dock.set_open(Tool::Interference, false);
    }
    Ok(())
}

fn interference_section(
    ui: &mut egui::Ui,
    check: &mut InterferenceCheck,
    names: &HashMap<u32, String>,
) {
    ui.heading("干渉チェック");
    let before = check.settings;
    ui.horizontal(|ui| {
        ui.checkbox(&mut check.auto, "自動");
        if ui.button("今すぐチェック").clicked() {
            check.request();
        }
    });
    egui::Grid::new("check_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("許容差");
            ui.add(
                egui::DragValue::new(&mut check.settings.tolerance)
                    .speed(1e-5)
                    .range(1e-7..=0.1)
                    .max_decimals(6),
            );
            ui.end_row();
            ui.label("警告するすき間 <");
            ui.add(
                egui::DragValue::new(&mut check.settings.clearance_warning)
                    .speed(0.005)
                    .range(0.0..=10.0)
                    .max_decimals(4),
            );
            ui.end_row();
        });
    ui.checkbox(&mut check.settings.compute_overlap, "干渉部分の体積と形状");
    if check.settings != before {
        check.request();
    }

    let report = &check.report;
    let count = |f: fn(&PairStatus) -> bool| report.pairs.iter().filter(|p| f(&p.status)).count();
    let interfering = count(|s| matches!(s, PairStatus::Interfering { .. }));
    let contact = count(|s| matches!(s, PairStatus::Contact));
    let near = count(|s| matches!(s, PairStatus::Near { .. }));
    ui.label(format!(
        "{} 部品を {} ms でチェック",
        report.body_count,
        crate::ui_check::measured(report.elapsed_ms, 2)
    ));
    ui.horizontal(|ui| {
        ui.colored_label(color(INTERFERENCE_COLOR), format!("干渉 {interfering}"));
        ui.colored_label(color(CONTACT_COLOR), format!("接触 {contact}"));
        ui.colored_label(color(NEAR_COLOR), format!("近接 {near}"));
    });
    if report.pairs.is_empty() {
        ui.label("問題は見つかりませんでした。");
        return;
    }

    let name = |id: u32| names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));
    egui::Grid::new("check_results")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("部品の組");
            ui.strong("状態");
            ui.strong("値");
            ui.end_row();
            for pair in &report.pairs {
                ui.label(format!("{} - {}", name(pair.a), name(pair.b)));
                let (status, value, c) = match &pair.status {
                    PairStatus::Interfering { overlap } => (
                        "干渉",
                        overlap
                            .as_ref()
                            .map(|o| format!("体積 {:.4}", o.volume))
                            .unwrap_or_else(|| {
                                if report.overlap_pending {
                                    "計算中…"
                                } else {
                                    "-"
                                }
                                .into()
                            }),
                        INTERFERENCE_COLOR,
                    ),
                    PairStatus::Contact => ("接触", "すき間 0".into(), CONTACT_COLOR),
                    PairStatus::Near { clearance } => {
                        ("近接", format!("すき間 {clearance:.4}"), NEAR_COLOR)
                    }
                };
                ui.colored_label(color(c), status);
                ui.label(value);
                ui.end_row();
            }
        });
}

fn measure_section(ui: &mut egui::Ui, tool: &mut MeasureTool) {
    ui.heading("距離計測");
    let mut active = tool.active;
    if ui.toggle_value(&mut active, "距離を測る (M)").changed() {
        tool.set_active(active);
    }
    if tool.active {
        ui.label(
            "部品の表面を2回クリック (頂点の近くは頂点に吸着)\nEsc: 1点目を取り消し / 計測をやめる",
        );
    }
    let mut remove = None;
    for (i, m) in tool.measurements.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.monospace(label_text(i + 1, m));
            if ui.small_button("x").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        tool.measurements.remove(i);
    }
    if !tool.measurements.is_empty() && ui.button("すべて消す").clicked() {
        tool.measurements.clear();
    }
}

fn color(c: Color) -> egui::Color32 {
    let [r, g, b, _] = c.to_srgba().to_u8_array();
    egui::Color32::from_rgb(r, g, b)
}
