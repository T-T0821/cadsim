//! 右上の「Simulation」パネル: 干渉チェックの設定・結果一覧と、距離計測の一覧

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::{
    InterferenceCheck, SimBody,
    interference::PairStatus,
    measure::{MeasureTool, label_text},
    view::{CONTACT_COLOR, INTERFERENCE_COLOR, NEAR_COLOR},
};

pub fn simulation_panel(
    mut contexts: EguiContexts,
    mut check: ResMut<InterferenceCheck>,
    mut measure: ResMut<MeasureTool>,
    bodies: Query<(&SimBody, Option<&Name>)>,
) -> Result {
    let names: HashMap<u32, String> = bodies
        .iter()
        .map(|(body, name)| {
            let name = name.map_or_else(|| format!("#{}", body.id), |n| n.to_string());
            (body.id, name)
        })
        .collect();

    egui::Window::new("Simulation")
        .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
        .resizable(false)
        .collapsible(true)
        .show(contexts.ctx_mut()?, |ui| {
            // 毎フレームの変更検知を避けるため、編集は一度コピーしてから書き戻す
            interference_section(ui, check.bypass_change_detection(), &names);
            ui.separator();
            measure_section(ui, measure.bypass_change_detection());
        });
    Ok(())
}

fn interference_section(
    ui: &mut egui::Ui,
    check: &mut InterferenceCheck,
    names: &HashMap<u32, String>,
) {
    ui.heading("Interference check");
    let before = check.settings;
    ui.horizontal(|ui| {
        ui.checkbox(&mut check.auto, "Auto");
        if ui.button("Check now").clicked() {
            check.request();
        }
    });
    egui::Grid::new("check_settings")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("Tolerance");
            ui.add(
                egui::DragValue::new(&mut check.settings.tolerance)
                    .speed(1e-5)
                    .range(1e-7..=0.1)
                    .max_decimals(6),
            );
            ui.end_row();
            ui.label("Warn if clearance <");
            ui.add(
                egui::DragValue::new(&mut check.settings.clearance_warning)
                    .speed(0.005)
                    .range(0.0..=10.0)
                    .max_decimals(4),
            );
            ui.end_row();
        });
    ui.checkbox(&mut check.settings.compute_overlap, "Overlap volume & mesh");
    if check.settings != before {
        check.request();
    }

    let report = &check.report;
    let count = |f: fn(&PairStatus) -> bool| report.pairs.iter().filter(|p| f(&p.status)).count();
    let interfering = count(|s| matches!(s, PairStatus::Interfering { .. }));
    let contact = count(|s| matches!(s, PairStatus::Contact));
    let near = count(|s| matches!(s, PairStatus::Near { .. }));
    ui.label(format!(
        "{} parts checked in {:.2} ms",
        report.body_count, report.elapsed_ms
    ));
    ui.horizontal(|ui| {
        ui.colored_label(
            color(INTERFERENCE_COLOR),
            format!("Interference {interfering}"),
        );
        ui.colored_label(color(CONTACT_COLOR), format!("Contact {contact}"));
        ui.colored_label(color(NEAR_COLOR), format!("Near {near}"));
    });
    if report.pairs.is_empty() {
        ui.label("No problems found.");
        return;
    }

    let name = |id: u32| names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));
    egui::Grid::new("check_results")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("Pair");
            ui.strong("Status");
            ui.strong("Value");
            ui.end_row();
            for pair in &report.pairs {
                ui.label(format!("{} - {}", name(pair.a), name(pair.b)));
                let (status, value, c) = match &pair.status {
                    PairStatus::Interfering { overlap } => (
                        "Interference",
                        overlap
                            .as_ref()
                            .map_or("-".into(), |o| format!("V = {:.4}", o.volume)),
                        INTERFERENCE_COLOR,
                    ),
                    PairStatus::Contact => ("Contact", "gap = 0".into(), CONTACT_COLOR),
                    PairStatus::Near { clearance } => {
                        ("Near", format!("gap = {clearance:.4}"), NEAR_COLOR)
                    }
                };
                ui.colored_label(color(c), status);
                ui.label(value);
                ui.end_row();
            }
        });
}

fn measure_section(ui: &mut egui::Ui, tool: &mut MeasureTool) {
    ui.heading("Measure");
    let mut active = tool.active;
    if ui
        .toggle_value(&mut active, "Measure distance (M)")
        .changed()
    {
        tool.set_active(active);
    }
    if tool.active {
        ui.label("Click two points on parts. Snaps to vertices.\nEsc: cancel point / leave");
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
    if !tool.measurements.is_empty() && ui.button("Clear all").clicked() {
        tool.measurements.clear();
    }
}

fn color(c: Color) -> egui::Color32 {
    let [r, g, b, _] = c.to_srgba().to_u8_array();
    egui::Color32::from_rgb(r, g, b)
}
