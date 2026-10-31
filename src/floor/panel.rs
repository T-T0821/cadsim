//! 「作業者・AGV」パネル: 時計、作業者の手順と集計、AGV の経路・搬送の要求・台数比較

use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};

use super::agv_sim::{Flow, compare_fleets};
use super::operator_plan::OperatorTask;
use super::{AgvLayoutRes, FloorClock, FloorState, FloorTool, OperatorsRes, sample};
use crate::camera::ViewportRect;
use crate::safety::SafetyState;
use crate::ui::widgets::round_for_display;

const RED: egui::Color32 = egui::Color32::from_rgb(230, 60, 60);

pub fn floor_panel(
    mut contexts: EguiContexts,
    mut clock: ResMut<FloorClock>,
    mut operators: ResMut<OperatorsRes>,
    mut layout: ResMut<AgvLayoutRes>,
    mut state: ResMut<FloorState>,
    safety: Res<SafetyState>,
    viewport: Res<ViewportRect>,
) -> Result {
    // 3D ビューの右上、ビューキューブの下に浮かべる (右下は寸法・注記)
    let corner = viewport.rect.map_or(egui::pos2(1000.0, 180.0), |r| {
        egui::pos2(r.max.x - 8.0, r.min.y + 170.0)
    });
    egui::Window::new("作業者・AGV")
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(corner)
        .resizable(false)
        .collapsible(true)
        .default_open(false)
        .show(contexts.ctx_mut()?, |ui| {
            // 毎フレームの変更検知を避けるため、編集したときだけ変更を知らせる
            let state = state.bypass_change_detection();
            clock_section(ui, &mut clock, state, safety.is_stopped());
            if ui
                .button("サンプルを置く")
                .on_hover_text("作業者 1人と、十字の通路を走る AGV のサンプル")
                .clicked()
            {
                sample::load(&mut operators, &mut layout, state);
                clock.time = 0.0;
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::CollapsingHeader::new("作業者")
                        .default_open(true)
                        .show(ui, |ui| {
                            if operator_section(
                                ui,
                                operators.bypass_change_detection(),
                                state,
                                clock.time,
                            ) {
                                operators.set_changed();
                            }
                        });
                    egui::CollapsingHeader::new("AGV")
                        .default_open(true)
                        .show(ui, |ui| {
                            if agv_section(ui, layout.bypass_change_detection(), state) {
                                layout.set_changed();
                                clock.time = 0.0;
                            }
                        });
                });
        });
    Ok(())
}

fn clock_section(ui: &mut egui::Ui, clock: &mut FloorClock, state: &mut FloorState, stopped: bool) {
    ui.horizontal(|ui| {
        if ui
            .button(if clock.playing {
                "⏸ 一時停止"
            } else {
                "▶ 再生"
            })
            .clicked()
        {
            clock.playing = !clock.playing;
        }
        if ui.button("⏮ 最初から").clicked() {
            clock.time = 0.0;
            state.reset_sim();
        }
        ui.add(
            egui::Slider::new(&mut clock.speed, 0.25..=20.0)
                .logarithmic(true)
                .text("倍速"),
        );
    });
    ui.label(format!("経過 {}", format_time(clock.time)));
    if stopped {
        ui.colored_label(
            RED,
            "安全停止中: AGV は止まっています (「安全」パネルでリセット)",
        );
    }
}

fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let m = (s / 60.0).floor();
    format!("{m:.0}:{:04.1}", s - m * 60.0)
}

/// 作業者の追加・手順の編集・集計。手順が変わったら true。
fn operator_section(
    ui: &mut egui::Ui,
    operators: &mut OperatorsRes,
    state: &mut FloorState,
    time: f64,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        if ui.button("作業者を追加").clicked() {
            let id = operators.add([0.0, 2000.0]);
            state.selected_operator = Some(id);
            changed = true;
        }
        ui.checkbox(&mut state.show_paths, "全員の歩行経路を表示");
    });
    let mut remove = None;
    for op in &operators.operators {
        let plan = state.plans.get(&op.id);
        ui.horizontal(|ui| {
            if ui
                .selectable_label(state.selected_operator == Some(op.id), &op.name)
                .clicked()
            {
                state.selected_operator = Some(op.id);
            }
            if let Some(plan) = plan {
                let current = plan
                    .segment_at(time)
                    .map(|s| match s.task.and_then(|i| op.tasks.get(i)) {
                        Some(task) => task.label(),
                        None => "開始位置へ戻る".into(),
                    })
                    .unwrap_or_default();
                ui.weak(current);
            }
        });
    }
    let Some(id) = state.selected_operator else {
        return changed;
    };
    let Some(op) = operators.operators.iter_mut().find(|o| o.id == id) else {
        return changed;
    };
    ui.separator();
    changed |= ui.text_edit_singleline(&mut op.name).changed();
    ui.horizontal(|ui| {
        ui.weak("開始位置 X");
        changed |= ui
            .add(egui::DragValue::new(&mut op.start[0]).speed(10.0))
            .changed();
        ui.weak("Z");
        changed |= ui
            .add(egui::DragValue::new(&mut op.start[1]).speed(10.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.weak("歩く速さ");
        let mut speed = op.walk_speed / 1000.0;
        if ui
            .add(
                egui::DragValue::new(&mut speed)
                    .speed(0.01)
                    .range(0.1..=3.0)
                    .suffix(" m/s"),
            )
            .changed()
        {
            op.walk_speed = speed * 1000.0;
            changed = true;
        }
    });

    ui.label("手順");
    let mut action: Option<(usize, i32)> = None;
    for (index, task) in op.tasks.iter_mut().enumerate() {
        let kind = match task {
            OperatorTask::Pick { .. } => "ピック",
            _ => "ボタン",
        };
        ui.horizontal(|ui| {
            ui.weak(format!("{}.", index + 1));
            match task {
                OperatorTask::Walk { to } => {
                    ui.label("歩く");
                    ui.weak("X");
                    changed |= ui
                        .add(egui::DragValue::new(&mut to[0]).speed(10.0))
                        .changed();
                    ui.weak("Z");
                    changed |= ui
                        .add(egui::DragValue::new(&mut to[1]).speed(10.0))
                        .changed();
                }
                OperatorTask::Pick { label, duration }
                | OperatorTask::Button { label, duration } => {
                    ui.label(kind);
                    changed |= ui
                        .add(egui::TextEdit::singleline(label).desired_width(80.0))
                        .changed();
                    changed |= ui
                        .add(
                            egui::DragValue::new(duration)
                                .speed(0.1)
                                .range(0.0..=600.0)
                                .suffix(" s"),
                        )
                        .changed();
                }
                OperatorTask::Wait { duration } => {
                    ui.label("待つ");
                    changed |= ui
                        .add(
                            egui::DragValue::new(duration)
                                .speed(0.1)
                                .range(0.0..=600.0)
                                .suffix(" s"),
                        )
                        .changed();
                }
            }
            if ui.small_button("↑").clicked() {
                action = Some((index, -1));
            }
            if ui.small_button("↓").clicked() {
                action = Some((index, 1));
            }
            if ui.small_button("削除").clicked() {
                action = Some((index, 0));
            }
        });
    }
    if let Some((index, dir)) = action {
        match dir {
            0 => {
                op.tasks.remove(index);
            }
            -1 if index > 0 => op.tasks.swap(index, index - 1),
            1 if index + 1 < op.tasks.len() => op.tasks.swap(index, index + 1),
            _ => {}
        }
        changed = true;
    }
    ui.horizontal_wrapped(|ui| {
        let walking = state.tool == FloorTool::AddWalk(op.id);
        if ui
            .selectable_label(walking, "歩く先を床でクリック")
            .clicked()
        {
            state.tool = if walking {
                FloorTool::None
            } else {
                FloorTool::AddWalk(op.id)
            };
        }
        if ui.button("+ ピック").clicked() {
            op.tasks.push(OperatorTask::Pick {
                label: "ワーク".into(),
                duration: 3.0,
            });
            changed = true;
        }
        if ui.button("+ ボタン").clicked() {
            op.tasks.push(OperatorTask::Button {
                label: "起動".into(),
                duration: 1.0,
            });
            changed = true;
        }
        if ui.button("+ 待つ").clicked() {
            op.tasks.push(OperatorTask::Wait { duration: 2.0 });
            changed = true;
        }
    });

    if let Some(plan) = state.plans.get(&op.id) {
        let [walk, work, wait] = plan.ratios();
        egui::Grid::new("operator_stats").show(ui, |ui| {
            ui.weak("サイクル時間");
            ui.label(format!("{} s", round_for_display(plan.cycle_time)));
            ui.end_row();
            ui.weak("歩行");
            ui.label(format!(
                "{} s ({walk:.0}%)",
                round_for_display(plan.walk_time)
            ));
            ui.end_row();
            ui.weak("作業");
            ui.label(format!(
                "{} s ({work:.0}%)",
                round_for_display(plan.work_time)
            ));
            ui.end_row();
            ui.weak("待ち");
            ui.label(format!(
                "{} s ({wait:.0}%)",
                round_for_display(plan.wait_time)
            ));
            ui.end_row();
            ui.weak("歩行距離");
            ui.label(format!(
                "{} m / サイクル",
                round_for_display(plan.walk_distance / 1000.0)
            ));
            ui.end_row();
        });
    }
    if ui.button("この作業者を削除").clicked() {
        remove = Some(op.id);
    }
    if let Some(id) = remove {
        operators.operators.retain(|o| o.id != id);
        state.selected_operator = None;
        if state.tool == FloorTool::AddWalk(id) {
            state.tool = FloorTool::None;
        }
        changed = true;
    }
    changed
}

/// AGV の経路・搬送の要求・性能・集計・台数比較。経路などが変わったら true。
fn agv_section(ui: &mut egui::Ui, layout: &mut AgvLayoutRes, state: &mut FloorState) -> bool {
    let mut changed = false;
    ui.label("経路 (床をクリックして置く)");
    ui.horizontal_wrapped(|ui| {
        for (tool, label, hint) in [
            (
                FloorTool::AddNode,
                "点",
                "クリックで経路の点を置く。直前に選んだ点と通路でつながる",
            ),
            (
                FloorTool::AddStation,
                "ステーション",
                "クリックで積み降ろしの場所を置く。既存の点の上ならステーションに変える",
            ),
            (
                FloorTool::Connect,
                "通路を結ぶ・外す",
                "2つの点を順にクリック。通路があれば外す",
            ),
        ] {
            let active = state.tool == tool;
            if ui
                .selectable_label(active, label)
                .on_hover_text(hint)
                .clicked()
            {
                state.tool = if active { FloorTool::None } else { tool };
                state.connect_from = None;
            }
        }
        if state.tool != FloorTool::None && ui.button("配置をやめる").clicked() {
            state.tool = FloorTool::None;
            state.connect_from = None;
        }
    });

    // 選んだ点の編集
    let mut remove = None;
    if let Some(node) = state.selected_node.and_then(|id| layout.graph.node_mut(id)) {
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::TextEdit::singleline(&mut node.name).desired_width(60.0))
                .changed();
            ui.weak("X");
            changed |= ui
                .add(egui::DragValue::new(&mut node.pos[0]).speed(10.0))
                .changed();
            ui.weak("Z");
            changed |= ui
                .add(egui::DragValue::new(&mut node.pos[1]).speed(10.0))
                .changed();
            changed |= ui.checkbox(&mut node.station, "ステーション").changed();
            if ui
                .small_button("削除")
                .on_hover_text("この点を消す")
                .clicked()
            {
                remove = Some(node.id);
            }
        });
    }
    if let Some(id) = remove {
        layout.graph.remove_node(id);
        layout.flows.retain(|f| f.from != id && f.to != id);
        state.selected_node = None;
        changed = true;
    }
    ui.weak(format!(
        "点 {} / 通路 {} / ステーション {}",
        layout.graph.nodes.len(),
        layout.graph.edges.len(),
        layout.graph.stations().len()
    ));

    // 搬送の要求
    ui.separator();
    ui.label("搬送の要求");
    let stations: Vec<(u32, String)> = layout
        .graph
        .nodes
        .iter()
        .filter(|n| n.station)
        .map(|n| (n.id, n.name.clone()))
        .collect();
    let name_of = |id: u32| {
        stations
            .iter()
            .find(|(s, _)| *s == id)
            .map_or("?".to_string(), |(_, n)| n.clone())
    };
    let mut remove_flow = None;
    for (index, flow) in layout.flows.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            for (which, value) in [("from", &mut flow.from), ("to", &mut flow.to)] {
                egui::ComboBox::from_id_salt(("flow", index, which))
                    .width(70.0)
                    .selected_text(name_of(*value))
                    .show_ui(ui, |ui| {
                        for (id, name) in &stations {
                            changed |= ui.selectable_value(value, *id, name).changed();
                        }
                    });
                if which == "from" {
                    ui.label("→");
                }
            }
            ui.weak("間隔");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut flow.interval)
                        .speed(1.0)
                        .range(1.0..=3600.0)
                        .suffix(" s"),
                )
                .changed();
            if ui.small_button("削除").clicked() {
                remove_flow = Some(index);
            }
        });
    }
    if let Some(index) = remove_flow {
        layout.flows.remove(index);
        changed = true;
    }
    if ui
        .add_enabled(stations.len() >= 2, egui::Button::new("+ 要求を追加"))
        .on_hover_text("ステーションが 2つ以上あると追加できる")
        .clicked()
    {
        layout.flows.push(Flow {
            from: stations[0].0,
            to: stations[1].0,
            interval: 60.0,
        });
        changed = true;
    }

    // 性能と台数
    ui.separator();
    let p = &mut layout.params;
    egui::Grid::new("agv_params").show(ui, |ui| {
        ui.weak("台数");
        changed |= ui
            .add(egui::DragValue::new(&mut p.count).range(0..=50))
            .changed();
        ui.end_row();
        ui.weak("速度");
        let mut speed = p.speed / 1000.0;
        if ui
            .add(
                egui::DragValue::new(&mut speed)
                    .speed(0.01)
                    .range(0.1..=5.0)
                    .suffix(" m/s"),
            )
            .changed()
        {
            p.speed = speed * 1000.0;
            changed = true;
        }
        ui.end_row();
        ui.weak("積込・荷降ろし");
        ui.horizontal(|ui| {
            for value in [&mut p.load_time, &mut p.unload_time] {
                changed |= ui
                    .add(
                        egui::DragValue::new(value)
                            .speed(0.5)
                            .range(0.0..=600.0)
                            .suffix(" s"),
                    )
                    .changed();
            }
        });
        ui.end_row();
        ui.weak("車体 長さ・幅");
        ui.horizontal(|ui| {
            for value in [&mut p.length, &mut p.width] {
                changed |= ui
                    .add(
                        egui::DragValue::new(value)
                            .speed(10.0)
                            .range(100.0..=5000.0)
                            .suffix(" mm"),
                    )
                    .changed();
            }
        });
        ui.end_row();
        ui.weak("車間");
        changed |= ui
            .add(
                egui::DragValue::new(&mut p.gap)
                    .speed(10.0)
                    .range(0.0..=5000.0)
                    .suffix(" mm"),
            )
            .changed();
        ui.end_row();
    });

    // 実行中の集計
    if let Some(sim) = &state.sim {
        ui.separator();
        egui::Grid::new("agv_stats").show(ui, |ui| {
            ui.weak("搬送数");
            ui.label(format!(
                "{} 件 ({} 件/時)",
                sim.stats.delivered,
                round_for_display(sim.throughput_per_hour())
            ));
            ui.end_row();
            ui.weak("平均リードタイム");
            ui.label(format!("{} s", round_for_display(sim.average_lead_time())));
            ui.end_row();
            ui.weak("稼働率");
            ui.label(format!("{:.0}%", sim.utilization()));
            ui.end_row();
            ui.weak("待ちの要求");
            ui.label(format!("{} 件", sim.pending.len()));
            ui.end_row();
        });
        for v in &sim.vehicles {
            let mut text = format!("AGV {}: {}", v.id + 1, v.phase.label());
            if v.blocked {
                text.push_str(" (待ち)");
            }
            ui.weak(text);
        }
    }

    // 台数の比較
    ui.separator();
    ui.label("台数の比較");
    if state.compare_max == 0 {
        state.compare_max = 5;
        state.compare_hours = 1.0;
    }
    ui.horizontal(|ui| {
        ui.weak("1 〜");
        ui.add(egui::DragValue::new(&mut state.compare_max).range(1..=20));
        ui.weak("台を");
        ui.add(
            egui::DragValue::new(&mut state.compare_hours)
                .speed(0.1)
                .range(0.1..=24.0)
                .suffix(" 時間"),
        );
        if ui
            .add_enabled(!layout.flows.is_empty(), egui::Button::new("比べる"))
            .clicked()
        {
            state.compare = compare_fleets(
                &layout.graph,
                &layout.flows,
                &layout.params,
                1..=state.compare_max,
                state.compare_hours * 3600.0,
            );
        }
    });
    if !state.compare.is_empty() {
        egui::Grid::new("fleet_compare")
            .striped(true)
            .show(ui, |ui| {
                for h in ["台数", "件/時", "リードタイム", "稼働率", "渋滞", "残り"]
                {
                    ui.strong(h);
                }
                ui.end_row();
                let best = state
                    .compare
                    .iter()
                    .map(|r| r.throughput_per_hour)
                    .fold(0.0, f64::max);
                for r in &state.compare {
                    let text = format!("{}", r.count);
                    if r.throughput_per_hour >= best * 0.98 && r.backlog <= 2 {
                        ui.colored_label(egui::Color32::from_rgb(60, 200, 90), text);
                    } else {
                        ui.label(text);
                    }
                    ui.label(round_for_display(r.throughput_per_hour));
                    ui.label(format!("{} s", round_for_display(r.average_lead_time)));
                    ui.label(format!("{:.0}%", r.utilization));
                    ui.label(format!("{:.0}%", r.blocked_ratio));
                    ui.label(format!("{}", r.backlog));
                    ui.end_row();
                }
            });
        ui.weak("緑: 要求をさばききれる台数。渋滞 = 交差点や前の車を待った時間の割合");
    }
    changed
}
