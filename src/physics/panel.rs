//! 3D ビュー左下 (「安全」パネルの右) の「物理演算」パネル: 開始・停止、統計、サンプルライン、ケーブル・ホースの一覧と編集
//!
//! 部品ごとの物理の設定は右のプロパティパネル (ui/properties.rs) で編集する。

use crate::ui::dock::{self, Dock};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cadsim_core::workspace::Tool;

use super::{CableSim, PhysicsState, RunRequest, RunState};
use crate::camera::ViewportRect;
use crate::model::{Model, NodeId, Selection};
use crate::ui::widgets::{length, round_for_display, xyz_editor};
use cadsim_core::physics::cable::{Cable, CableAnchor, CableKind, CableReport};
use cadsim_core::physics::settings::meters_per_unit;

pub fn physics_panel(
    mut contexts: EguiContexts,
    mut state: ResMut<PhysicsState>,
    mut model: ResMut<Model>,
    selection: Res<Selection>,
    cables: Res<CableSim>,
    viewport: Res<ViewportRect>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::panel(Tool::Physics, &dock, viewport.rect) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    window.open(&mut open).show(ctx, |ui| {
            ui.set_max_width(320.0);
            run_section(ui, state.bypass_change_detection());
            ui.separator();
            if ui
                .button("サンプルラインを読み込む")
                .on_hover_text("ベルトコンベア・ストッパ・シュートとワーク 100 個のライン (今のモデルと置き換えます)")
                .clicked()
            {
                state.load_sample = true;
            }
            ui.separator();
            cable_section(ui, &mut model, &selection, &cables.reports);
        });
    if !open {
        dock.set_open(Tool::Physics, false);
    }
    Ok(())
}

fn run_section(ui: &mut egui::Ui, state: &mut PhysicsState) {
    ui.horizontal(|ui| {
        let running = state.state == RunState::Running;
        if ui
            .add_enabled(!running, egui::Button::new("▶ 開始"))
            .on_hover_text("重力・接触・摩擦で部品を動かす。タイミングチャートも一緒に再生する")
            .clicked()
        {
            state.request = Some(RunRequest::Start);
        }
        if ui
            .add_enabled(running, egui::Button::new("⏸ 一時停止"))
            .clicked()
        {
            state.request = Some(RunRequest::Pause);
        }
        if ui
            .add_enabled(
                state.state != RunState::Stopped,
                egui::Button::new("⏹ 停止"),
            )
            .on_hover_text("部品を設計位置に戻す")
            .clicked()
        {
            state.request = Some(RunRequest::Stop);
        }
    });
    let stats = &state.stats;
    match state.state {
        RunState::Stopped => {
            ui.weak("停止中 (部品は設計位置)");
            ui.weak("部品の振る舞いはプロパティの「物理」で選びます");
        }
        RunState::Running | RunState::Paused => {
            egui::Grid::new("physics_stats")
                .num_columns(2)
                .spacing([12.0, 2.0])
                .show(ui, |ui| {
                    ui.weak("時刻");
                    ui.label(format!("{:.2} 秒", stats.time));
                    ui.end_row();
                    ui.weak("剛体");
                    ui.label(format!(
                        "{} 個 (物理で動く {} 個)",
                        stats.bodies, stats.dynamic
                    ));
                    ui.end_row();
                    ui.weak("計算時間");
                    ui.label(format!(
                        "{} ms / フレーム",
                        crate::ui_check::measured(stats.frame_ms, 2)
                    ));
                    ui.end_row();
                    ui.weak("画面");
                    ui.label(format!("{} fps", crate::ui_check::measured(stats.fps, 0)));
                    ui.end_row();
                });
        }
    }
    if !stats.skipped.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(230, 160, 40),
            format!("当たり判定を作れなかった部品: {} 個", stats.skipped.len()),
        )
        .on_hover_text(stats.skipped.join("\n"));
    }
}

fn cable_section(
    ui: &mut egui::Ui,
    model: &mut Model,
    selection: &Selection,
    reports: &[CableReport],
) {
    ui.horizontal(|ui| {
        ui.strong("ケーブル・ホース");
        if ui
            .button("＋ 追加")
            .on_hover_text("選択中の部品から伸ばす。もう一方の端はワールドに固定 (あとで部品に付け替えられます)")
            .clicked()
        {
            let cable = new_cable(model, selection.node, model.cables.len() + 1);
            model.cables.push(cable);
        }
    });
    if model.cables.is_empty() {
        ui.weak("部品を選んで「追加」を押すと、そこから紐が伸びます");
        return;
    }
    let unit = model.unit.clone();
    let mut remove = None;
    let mut cables = model.cables.clone();
    for (i, cable) in cables.iter_mut().enumerate() {
        let report = reports.get(i);
        let header = format!("{} {}", status_icon(report), cable.name);
        egui::CollapsingHeader::new(header)
            .id_salt(("cable", i))
            .default_open(false)
            .show(ui, |ui| {
                if let Some(report) = report {
                    report_lines(ui, model, cable, report, &unit);
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut cable.name).desired_width(140.0));
                    egui::ComboBox::from_id_salt(("cable_kind", i))
                        .selected_text(cable.kind.label())
                        .show_ui(ui, |ui| {
                            for kind in CableKind::ALL {
                                ui.selectable_value(&mut cable.kind, kind, kind.label());
                            }
                        });
                });
                egui::Grid::new(("cable_size", i))
                    .num_columns(2)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("長さ", &mut cable.length),
                            ("直径", &mut cable.diameter),
                            ("許容曲げ半径", &mut cable.min_bend_radius),
                        ] {
                            ui.weak(label);
                            ui.add(
                                egui::DragValue::new(value)
                                    .speed(1.0)
                                    .range(0.01..=f64::MAX)
                                    .suffix(format!(" {unit}")),
                            );
                            ui.end_row();
                        }
                    });
                anchor_editor(ui, model, selection, "始点", &mut cable.start, ("start", i));
                anchor_editor(ui, model, selection, "終点", &mut cable.end, ("end", i));
                if ui.button("🗑 削除").clicked() {
                    remove = Some(i);
                }
            });
    }
    if let Some(i) = remove {
        cables.remove(i);
    }
    // 毎フレーム書くとモデルが変わった扱いになるので、変わったときだけ戻す
    if cables != model.cables {
        model.cables = cables;
    }
}

fn status_icon(report: Option<&CableReport>) -> &'static str {
    match report {
        Some(r) if !r.is_ok() => "⚠",
        _ => "〰",
    }
}

fn report_lines(ui: &mut egui::Ui, model: &Model, cable: &Cable, report: &CableReport, unit: &str) {
    let warn = egui::Color32::from_rgb(240, 90, 80);
    if report.is_taut() {
        ui.colored_label(warn, "長さが足りません (両端に引っ張られています)");
    }
    let radius = if report.min_bend_radius.is_finite() {
        length(report.min_bend_radius, unit)
    } else {
        "まっすぐ".into()
    };
    if report.over_bent.is_empty() {
        ui.label(format!("最小曲げ半径 {radius}"));
    } else {
        ui.colored_label(
            warn,
            format!(
                "最小曲げ半径 {radius} (許容 {} より小さい)",
                length(cable.min_bend_radius, unit)
            ),
        );
    }
    ui.weak(format!(
        "両端の距離 / 長さ = {}",
        round_for_display(report.stretch)
    ));
    if !report.touching.is_empty() {
        let names: Vec<String> = report
            .touching
            .iter()
            .filter_map(|id| model.get(*id).map(|n| n.name.clone()))
            .collect();
        ui.label(format!("当たっている部品: {}", names.join(", ")));
    }
}

fn anchor_editor(
    ui: &mut egui::Ui,
    model: &Model,
    selection: &Selection,
    label: &str,
    anchor: &mut CableAnchor,
    salt: (&str, usize),
) {
    ui.horizontal(|ui| {
        ui.weak(label);
        let name = match anchor.node {
            Some(id) => model
                .get(id)
                .map_or("(見つからない部品)".into(), |n| n.name.clone()),
            None => "ワールドに固定".into(),
        };
        ui.label(name);
    });
    ui.horizontal(|ui| {
        let part = selection
            .node
            .filter(|&id| model.get(id).is_some_and(|n| n.kind.is_part()));
        if ui
            .add_enabled(part.is_some(), egui::Button::new("選択中の部品に付ける"))
            .clicked()
            && let Some(id) = part
        {
            *anchor = CableAnchor {
                node: Some(id),
                local: part_center(model, id),
            };
        }
        if anchor.node.is_some() && ui.button("ワールドに固定").clicked() {
            let world = anchor_world(model, anchor);
            *anchor = CableAnchor {
                node: None,
                local: world,
            };
        }
    });
    ui.push_id(salt, |ui| {
        let mut local = anchor.local.to_array();
        let what = if anchor.node.is_some() {
            "位置 (部品の座標系)"
        } else {
            "位置 (ワールド)"
        };
        if xyz_editor(ui, what, &mut local, 1.0, None) {
            anchor.local = DVec3::from(local);
        }
    });
}

/// 部品の外接箱の中心 (部品の座標系)
fn part_center(model: &Model, id: NodeId) -> DVec3 {
    match model.world_bounds(id) {
        Some(bounds) => model
            .world_matrix(id)
            .inverse()
            .transform_point3(bounds.center()),
        None => DVec3::ZERO,
    }
}

/// 端の設計位置 (ワールド)
fn anchor_world(model: &Model, anchor: &CableAnchor) -> DVec3 {
    match anchor.node {
        Some(id) if model.get(id).is_some() => {
            model.world_matrix(id).transform_point3(anchor.local)
        }
        _ => anchor.local,
    }
}

/// 選択中の部品の中心から横へ伸ばした新しいケーブル
fn new_cable(model: &Model, selected: Option<NodeId>, number: usize) -> Cable {
    let unit = 0.001 / meters_per_unit(&model.unit);
    let part = selected.filter(|&id| model.get(id).is_some_and(|n| n.kind.is_part()));
    let start = match part {
        Some(id) => CableAnchor {
            node: Some(id),
            local: part_center(model, id),
        },
        None => CableAnchor {
            node: None,
            local: DVec3::ZERO,
        },
    };
    let from = anchor_world(model, &start);
    let span = 300.0 * unit;
    Cable {
        name: format!("ケーブル {number}"),
        kind: CableKind::Cable,
        start,
        end: CableAnchor {
            node: None,
            local: from + DVec3::new(span, 0.0, 0.0),
        },
        length: span * 1.5,
        diameter: 10.0 * unit,
        min_bend_radius: 50.0 * unit,
    }
}
