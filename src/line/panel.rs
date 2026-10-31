//! 「ライン連携」ウィンドウ: まとめて再生、つなぎ方 (AGV 搬送・チャートの起動)、PLC の信号表

use crate::ui::dock::{self, Dock};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, egui};
use cadsim_core::workspace::Tool;

use cadsim_core::flow::model::{AgvCall, ElementKind, KindTag, sample_line};
use cadsim_core::line::{Direction, Value};

use super::LineState;
use super::stepper::link_problems;
use crate::floor::agv_sim::AgvSim;
use crate::floor::{AgvLayoutRes, FloorClock, FloorState, OperatorsRes, sample};
use crate::flow::{FlowRes, FlowRun};
use crate::safety::SafetyState;

const RED: egui::Color32 = egui::Color32::from_rgb(230, 60, 60);
const ON: egui::Color32 = egui::Color32::from_rgb(80, 200, 110);
const SPEEDS: [f64; 7] = [1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0];

#[allow(clippy::too_many_arguments)]
pub fn line_window(
    mut contexts: EguiContexts,
    mut line: ResMut<LineState>,
    mut flow: ResMut<FlowRes>,
    mut run: ResMut<FlowRun>,
    mut floor: ResMut<FloorState>,
    mut layout: ResMut<AgvLayoutRes>,
    mut clock: ResMut<FloorClock>,
    mut operators: ResMut<OperatorsRes>,
    safety: Res<SafetyState>,
    mut dock: ResMut<Dock>,
) -> Result {
    let Some(window) = dock::workspace_window(Tool::LineLink, &dock, [520.0, 480.0]) else {
        return Ok(());
    };
    let ctx = contexts.ctx_mut()?;
    let mut open = true;
    let mut action = None;
    let mut edits = Vec::new();
    let mut sample_clicked = false;
    // 表示のための読み書きは変更を知らせない (毎フレームの変更検知を避ける)
    let line = line.bypass_change_detection();
    window
        .open(&mut open)
        .show(ctx, |ui| {
            action = controls(ui, line, &run, &safety);
            ui.separator();
            egui::ScrollArea::vertical().show(ui, |ui| {
                summary(ui, line, &run, &floor);
                ui.separator();
                egui::CollapsingHeader::new("つなぎ方")
                    .default_open(true)
                    .show(ui, |ui| edits = links(ui, &flow, &layout, &floor));
                egui::CollapsingHeader::new("PLC の信号")
                    .default_open(false)
                    .show(ui, |ui| signals(ui, line, &flow, &run, &floor, &safety));
                ui.separator();
                sample_clicked = ui
                    .button("サンプルのラインを置く")
                    .on_hover_text(
                        "サンプル工程と十字の AGV 通路を置き、バッファから組立へ AGV で運ぶようにつなぐ",
                    )
                    .clicked();
            });
        });
    if !open {
        dock.set_open(Tool::LineLink, false);
    }

    match action {
        Some(Action::PlayPause) => {
            if line.started {
                line.playing = !line.playing;
            } else if let Err(problems) =
                line.start(&flow, &mut run, &mut floor, &layout, &mut clock)
            {
                line.message = Some(problems.join("\n"));
            }
        }
        Some(Action::Reset) => {
            line.stop(None);
            run.stop();
            floor.restart_agv(&layout);
            clock.time = 0.0;
        }
        None => {}
    }
    for (id, carrier, starts) in edits {
        if let Some(e) = flow.get_mut(id) {
            e.carrier = carrier;
            e.starts_chart = starts;
        }
    }
    if sample_clicked {
        load_sample(&mut flow, &mut operators, &mut layout, &mut floor);
        line.stop(None);
    }
    Ok(())
}

enum Action {
    PlayPause,
    Reset,
}

fn controls(
    ui: &mut egui::Ui,
    line: &mut LineState,
    run: &FlowRun,
    safety: &SafetyState,
) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        let label = if line.playing {
            "⏸ 一時停止"
        } else {
            "▶ 再生"
        };
        if ui.button(label).clicked() {
            action = Some(Action::PlayPause);
        }
        if ui
            .add_enabled(line.started, egui::Button::new("⏹ 最初に戻す"))
            .clicked()
        {
            action = Some(Action::Reset);
        }
        ui.separator();
        ui.label("速度");
        egui::ComboBox::from_id_salt("line_speed")
            .selected_text(format!("{} 倍", line.speed))
            .show_ui(ui, |ui| {
                for s in SPEEDS {
                    ui.selectable_value(&mut line.speed, s, format!("{s} 倍"));
                }
            });
        ui.checkbox(&mut line.chart_follows, "タイミングチャートも再生")
            .on_hover_text(
                "ラインと一緒にタイミングチャートを流す。工程に「チャート起動」を付けたときは、その工程が加工を始めるたびに頭から再生する",
            );
    });
    ui.label(format!("経過 {}", format_time(line.time)));
    if safety.is_stopped() {
        ui.colored_label(
            RED,
            "安全停止中: ライン全体が止まっています (「安全」パネルでリセット)",
        );
    } else if !line.inputs.run {
        ui.colored_label(RED, "PLC の「ライン.運転」が OFF なので止まっています");
    }
    if let Some(sim) = &run.sim
        && line.with_flow
        && sim.finished()
    {
        ui.label("工程シミュレーションは回す時間の終わりまで来ました (AGV と作業者は動き続けます)");
    }
    if let Some(message) = &line.message {
        ui.colored_label(RED, message);
    }
    action
}

fn summary(ui: &mut egui::Ui, line: &LineState, run: &FlowRun, floor: &FloorState) {
    egui::Grid::new("line_summary")
        .num_columns(2)
        .show(ui, |ui| {
            if let Some(sim) = run.sim.as_ref().filter(|s| s.is_linked()) {
                ui.label("排出");
                ui.label(format!("{} 個", sim.output_total()));
                ui.end_row();
                ui.label("ライン内のワーク");
                ui.label(format!(
                    "{} 個 (うち AGV で運搬中 {})",
                    sim.wip(),
                    sim.in_transit()
                ));
                ui.end_row();
            }
            ui.label("AGV を待っている搬送");
            ui.label(format!("{} 件", line.coupler.waiting_for_agv()));
            ui.end_row();
            ui.label("AGV が運んだワーク");
            ui.label(format!("{} 個", line.coupler.carried()));
            ui.end_row();
            if let Some(c) = line.conveyor {
                ui.label("コンベア");
                ui.label(format!(
                    "投入 {} 個 / 排出 {} 個 / 仕掛 {} 個",
                    c.fed, c.discharged, c.in_process
                ));
                ui.end_row();
            }
            if let Some(agv) = floor.agv_sim() {
                ui.label("AGV");
                ui.label(format!(
                    "{} 台 (荷あり {})",
                    agv.vehicles.len(),
                    agv.vehicles.iter().filter(|v| AgvSim::is_loaded(v)).count()
                ));
                ui.end_row();
            }
        });
}

/// 工程ごとの送り出し方 (すぐ下流へ / AGV) と、チャートの起動
fn links(
    ui: &mut egui::Ui,
    flow: &FlowRes,
    layout: &AgvLayoutRes,
    floor: &FloorState,
) -> Vec<(u32, Option<AgvCall>, bool)> {
    if flow.is_empty() {
        ui.label("工程モデルがありません。ツールレールの「工程」(工程シミュレーション) で並べると、ここでつなげます。");
        return Vec::new();
    }
    ui.label("変えると、次に再生したときに最初から回します。");
    let stations: Vec<(u32, String)> = layout
        .graph
        .nodes
        .iter()
        .filter(|n| n.station)
        .map(|n| (n.id, n.name.clone()))
        .collect();
    let station_name = |id: u32| {
        layout
            .graph
            .node(id)
            .map_or_else(|| format!("#{id}"), |n| n.name.clone())
    };
    // 書き換えるのは、本当に変わったときだけ (工程モデルの変更は結果を古くする)。呼び出し側で書く。
    let mut edits: Vec<(u32, Option<AgvCall>, bool)> = Vec::new();
    egui::Grid::new("line_links")
        .num_columns(3)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("要素");
            ui.strong("送り出し");
            ui.strong("チャート起動");
            ui.end_row();
            for e in &flow.elements {
                if e.kind.tag() == KindTag::Sink {
                    continue;
                }
                ui.label(&e.name);
                let mut carrier = e.carrier;
                ui.horizontal(|ui| {
                    let mut by_agv = carrier.is_some();
                    let can = stations.len() >= 2 || by_agv;
                    if ui
                        .add_enabled(can, egui::Checkbox::new(&mut by_agv, "AGV"))
                        .on_disabled_hover_text(
                            "「作業者・AGV」パネルでステーションを 2つ以上置くと選べます",
                        )
                        .changed()
                    {
                        carrier = by_agv.then(|| AgvCall {
                            from: stations[0].0,
                            to: stations[1].0,
                        });
                    }
                    if let Some(call) = carrier.as_mut() {
                        for (salt, value) in [("from", &mut call.from), ("to", &mut call.to)] {
                            egui::ComboBox::from_id_salt(("line_station", e.id, salt))
                                .width(70.0)
                                .selected_text(station_name(*value))
                                .show_ui(ui, |ui| {
                                    for (id, name) in &stations {
                                        ui.selectable_value(value, *id, name);
                                    }
                                });
                            if salt == "from" {
                                ui.label("→");
                            }
                        }
                    } else {
                        ui.label("すぐ下流へ");
                    }
                });
                let mut starts = e.starts_chart;
                if matches!(e.kind, ElementKind::Process { .. }) {
                    ui.checkbox(&mut starts, "").on_hover_text(
                        "この工程が加工を始めるたびにタイミングチャートを頭から再生する",
                    );
                } else {
                    ui.label("");
                }
                ui.end_row();
                if carrier != e.carrier || starts != e.starts_chart {
                    edits.push((e.id, carrier, starts));
                }
            }
        });
    let vehicles = floor.agv_sim().map_or(0, |a| a.vehicles.len());
    for problem in link_problems(flow, &layout.graph, vehicles) {
        ui.colored_label(RED, problem);
    }
    edits
}

/// PLC とやりとりする信号の一覧。PLC → 設備 の信号はクリックで切り替えられる。
fn signals(
    ui: &mut egui::Ui,
    line: &mut LineState,
    flow: &FlowRes,
    run: &FlowRun,
    floor: &FloorState,
    safety: &SafetyState,
) {
    ui.label(
        "工程・AGV・コンベア・安全の状態を PLC の入力に、PLC の出力をラインの運転と投入許可に割り付ける表です。この表は「PLC 連携」の I/O 一覧の後ろにも並び、外部の PLC (Modbus TCP / OPC UA) とやりとりします。",
    );
    if line.plc_drives_inputs {
        ui.colored_label(ON, "PLC → 設備 の信号は PLC が書いています");
    } else {
        ui.label("PLC から書いていない間は、PLC → 設備 の信号をクリックして切り替えられます。");
    }
    let rows = super::current_signals(line, flow, run, floor, safety);
    let mut toggled = None;
    egui::Grid::new("line_signals")
        .num_columns(4)
        .striped(true)
        .show(ui, |ui| {
            ui.strong("信号");
            ui.strong("向き");
            ui.strong("番号");
            ui.strong("値");
            ui.end_row();
            for s in &rows {
                ui.label(&s.name);
                ui.label(match s.direction {
                    Direction::ToPlc => "設備 → PLC",
                    Direction::FromPlc => "PLC → 設備",
                });
                let area = match (s.direction, s.value) {
                    (_, Value::Word(_)) => "D",
                    (Direction::ToPlc, _) => "X",
                    (Direction::FromPlc, _) => "Y",
                };
                ui.label(format!("{area}{}", s.address));
                match s.value {
                    Value::Bit(on) => {
                        let text =
                            egui::RichText::new(if on { "ON" } else { "OFF" }).color(if on {
                                ON
                            } else {
                                ui.visuals().weak_text_color()
                            });
                        if s.direction == Direction::FromPlc && !line.plc_drives_inputs {
                            if ui.button(text).clicked() {
                                toggled = Some((s.name.clone(), !on));
                            }
                        } else {
                            ui.label(text);
                        }
                    }
                    Value::Word(v) => {
                        ui.label(v.to_string());
                    }
                }
                ui.end_row();
            }
        });
    if let Some((name, on)) = toggled {
        line.inputs.set(flow, &name, on);
    }
}

/// サンプル工程 + 十字の AGV 通路。バッファの送り出しを AGV で運ぶ。
fn load_sample(
    flow: &mut FlowRes,
    operators: &mut OperatorsRes,
    layout: &mut AgvLayoutRes,
    floor: &mut FloorState,
) {
    sample::load(operators, layout, floor);
    // 決まった間隔の搬送はやめ、工程から頼まれた搬送だけを走らせる
    layout.flows.clear();
    let stations = layout.graph.stations();
    let mut model = sample_line();
    if let (Some(&from), Some(&to)) = (stations.first(), stations.get(1))
        && let Some(buffer) = model
            .elements
            .iter_mut()
            .find(|e| matches!(e.kind, ElementKind::Buffer { .. }))
    {
        buffer.carrier = Some(AgvCall { from, to });
    }
    **flow = model;
}

fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0);
    let h = (s / 3600.0).floor();
    let m = ((s - h * 3600.0) / 60.0).floor();
    format!("{h:.0}:{m:02.0}:{:04.1}", s - h * 3600.0 - m * 60.0)
}
