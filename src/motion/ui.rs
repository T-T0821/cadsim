//! タイミングチャートの編集画面と再生コントロール
//!
//! 画面下のウィンドウに、行 = アクチュエータ・センサー、列 = ステップのチャートを描く。
//! 列の幅はステップごとに同じで、列の中では時間に比例して波形 (各アクチュエータの値、
//! センサーの ON / OFF) を描く。インターロックの条件待ちの時間は列の左側に橙色で出る。
//!
//! - セルをクリック: そのステップ × アクチュエータ (センサー) を選び、右側で動作 (開始条件) を編集
//! - セルをダブルクリック: シリンダは 出 → 戻 → なし を切り替え、軸は動作を追加。
//!   センサーは開始条件を ON 待ち → OFF 待ち → なし で切り替え
//! - チャート上をドラッグ: 再生位置を動かす (スクラブ)
//!
//! アクチュエータの行の下に、ロボットの行 (教示点への移動) を並べる。

use bevy::{ecs::system::SystemParam, prelude::*};
use bevy_egui::egui;

use super::chart::{
    Action, Actuator, ActuatorKind, Axis, Easing, INTERLOCK_TIMEOUT, MoveKind, RobotMove, Schedule,
    Step, TimingChart,
};
use super::robot_ui::RobotUiState;
use super::sensor::SensorKind;
use super::{ChartSchedule, JogPose, Playback, SensorStates, SensorTimeline, SimulationPose, demo};
use crate::camera::CameraRequest;
use crate::conveyor::ConveyorConfigRes;
use crate::history::History;
use crate::model::{Model, NodeId, Selection};
use crate::recording::Recorder;

const NAME_WIDTH: f32 = 150.0;
const HEADER_HEIGHT: f32 = 34.0;
const ROW_HEIGHT: f32 = 32.0;
const MIN_COLUMN_WIDTH: f32 = 64.0;
const PROPERTIES_WIDTH: f32 = 330.0;
const PANEL_HEIGHT: f32 = 270.0;

const PLAYHEAD_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 70, 60);
const WAVE_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 170, 255);
const ACTION_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 110, 170, 70);
const SELECT_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 170, 60);
const SENSOR_ON_COLOR: egui::Color32 = egui::Color32::from_rgb(80, 200, 100);
const WAIT_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(120, 80, 20, 60);
const INTERLOCK_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 170, 60);
const STOP_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 80, 70);
const MOVE_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(150, 90, 30, 80);
pub(super) const WARN_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 180, 60);
pub(super) const ERROR_COLOR: egui::Color32 = egui::Color32::from_rgb(235, 80, 70);

#[derive(Resource, Default)]
pub struct ChartUiState {
    pub(super) selected_actuator: Option<u32>,
    selected_sensor: Option<u32>,
    pub(super) selected_step: Option<usize>,
    /// ロボットの行またはロボット画面で選んだロボット
    pub(super) selected_robot: Option<u32>,
    /// パネルをツールバーだけに畳んでいる
    collapsed: bool,
}

impl ChartUiState {
    /// チャートで選んでいるセンサー (3D ビューで強調する)
    pub fn selected_sensor(&self) -> Option<u32> {
        self.selected_sensor
    }

    fn select_actuator(&mut self, id: u32) {
        self.selected_actuator = Some(id);
        self.selected_sensor = None;
        self.selected_robot = None;
    }

    fn select_sensor(&mut self, id: u32) {
        self.selected_sensor = Some(id);
        self.selected_actuator = None;
        self.selected_robot = None;
    }

    fn select_robot(&mut self, id: u32) {
        self.selected_robot = Some(id);
        self.selected_actuator = None;
        self.selected_sensor = None;
    }
}

/// チャートの1行: アクチュエータ、センサー、ロボットの順に並べる
#[derive(Clone, Copy, PartialEq)]
enum Row {
    Actuator(u32),
    Sensor(u32),
    Robot(u32),
}

fn rows(chart: &TimingChart) -> Vec<Row> {
    let actuators = chart.actuators.iter().map(|a| Row::Actuator(a.id));
    let sensors = chart.sensors.iter().map(|s| Row::Sensor(s.id));
    let robots = chart.robots.iter().map(|r| Row::Robot(r.id));
    actuators.chain(sensors).chain(robots).collect()
}

fn row_count(chart: &TimingChart) -> usize {
    chart.actuators.len() + chart.sensors.len() + chart.robots.len()
}

/// タイミングチャートのパネルが読み書きするもの一式
#[derive(SystemParam)]
pub struct MotionUi<'w> {
    pub(super) chart: ResMut<'w, TimingChart>,
    pub(super) schedule: Res<'w, ChartSchedule>,
    timeline: Res<'w, SensorTimeline>,
    sensor_states: Res<'w, SensorStates>,
    playback: ResMut<'w, Playback>,
    pub(super) state: ResMut<'w, ChartUiState>,
    /// サンプル装置を読み込むときに、コンベアとグリッパも置き換える
    conveyors: ResMut<'w, ConveyorConfigRes>,
    recorder: Res<'w, Recorder>,
    pub(super) pose: Res<'w, SimulationPose>,
    pub(super) jog: ResMut<'w, JogPose>,
    pub(super) robot: ResMut<'w, RobotUiState>,
}

/// 画面下のタイミングチャートのパネル。ステータスバーの上、左右のパネルより外側に置く。
pub fn show_panel(
    root: &mut egui::Ui,
    motion: &mut MotionUi,
    model: &mut Model,
    selection: &Selection,
    camera: &mut MessageWriter<CameraRequest>,
    history: &mut History,
) {
    // 編集はコピーに対して行い、変わったときだけ書き戻す (毎フレーム変更扱いにしないため)
    let mut edited = motion.chart.clone();
    let mut play = *motion.playback;
    let schedule = &motion.schedule;
    let state = &mut *motion.state;
    let mut load_demo = None;

    // 畳んだときと開いたときで別のパネルにする (開いたときの高さを覚えておくため)
    let panel = if state.collapsed {
        egui::Panel::bottom("timing_chart_collapsed").resizable(false)
    } else {
        egui::Panel::bottom("timing_chart")
            .resizable(true)
            .default_size(PANEL_HEIGHT)
            .min_size(120.0)
    };
    panel.show(root, |ui| {
        ui.add_space(2.0);
        load_demo = toolbar(ui, &mut edited, schedule, &mut play, state);
        if state.collapsed {
            return;
        }
        ui.separator();
        ui.horizontal_top(|ui| {
            let chart_width = (ui.available_width() - PROPERTIES_WIDTH - 12.0).max(200.0);
            ui.allocate_ui(egui::vec2(chart_width, ui.available_height()), |ui| {
                egui::ScrollArea::both()
                    .id_salt("timing_chart")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let view = ChartView {
                            schedule,
                            timeline: &motion.timeline,
                            states: &motion.sensor_states,
                        };
                        chart_view(ui, &mut edited, &view, &mut play, state);
                    });
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .id_salt("timing_properties")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.set_width(PROPERTIES_WIDTH - 16.0);
                        properties(ui, &mut edited, schedule, model, selection, state);
                    });
                });
        });
    });

    // 動画の書き出し中は表示だけして、チャートや再生位置の操作は反映しない
    if motion.recorder.is_capturing() {
        return;
    }
    if let Some(which) = load_demo {
        // 部品の入れ替えは1件の履歴にする (チャートの入れ替えは見張りが別の1件にする)
        let conveyors = &mut motion.conveyors;
        history.replace_all(
            model,
            "サンプル装置を読み込み".into(),
            |model| match which {
                Demo::Gantry => demo::load_demo(model, &mut edited, conveyors),
                Demo::Robot => demo::load_robot_demo(model, &mut edited),
            },
        );
        camera.write(CameraRequest::FitAll);
        play = Playback::default();
        *state = ChartUiState::default();
    }
    if edited != *motion.chart {
        *motion.chart = edited;
    }
    motion.playback.set_if_neq(play);
}

// ---------------------------------------------------------------------------
// 上部のツールバー (再生ボタンなど)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Demo {
    Gantry,
    Robot,
}

/// サンプルのボタンが押されたら、読み込むサンプルを返す
fn toolbar(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &Schedule,
    play: &mut Playback,
    state: &mut ChartUiState,
) -> Option<Demo> {
    let mut load_demo = None;
    ui.horizontal(|ui| {
        let arrow = if state.collapsed { "▶" } else { "▼" };
        if ui
            .button(format!("{arrow} タイミングチャート"))
            .on_hover_text("パネルを開く / 畳む")
            .clicked()
        {
            state.collapsed = !state.collapsed;
        }
        ui.separator();
        if ui.button("⏮ 先頭へ").clicked() {
            play.time = 0.0;
            play.playing = false;
        }
        let label = if play.playing {
            "⏸ 一時停止"
        } else {
            "▶ 再生"
        };
        if ui.button(label).clicked() {
            if !play.playing && play.time >= schedule.cycle_time {
                play.time = 0.0;
            }
            play.playing = !play.playing;
        }
        ui.checkbox(&mut play.looping, "繰り返し");
        egui::ComboBox::from_id_salt("speed")
            .width(60.0)
            .selected_text(format!("x{}", play.speed))
            .show_ui(ui, |ui| {
                for speed in [0.1, 0.25, 0.5, 1.0, 2.0] {
                    ui.selectable_value(&mut play.speed, speed, format!("x{speed}"));
                }
            });

        let step = schedule
            .step_at(play.time)
            .map_or("-".to_string(), |i| format!("S{}", i + 1));
        ui.monospace(format!(
            "t = {:5.2} s / サイクル {:.2} s   ステップ {step}",
            play.time, schedule.cycle_time
        ));

        ui.separator();
        if ui.button("+ ステップ").clicked() {
            let at = state.selected_step.map_or(chart.steps.len(), |i| i + 1);
            chart.steps.insert(at, Step::default());
            state.selected_step = Some(at);
        }
        ui.menu_button("+ アクチュエータ", |ui| {
            for kind in ActuatorKind::ALL {
                if ui.button(kind.label()).clicked() {
                    state.select_actuator(chart.add_actuator(kind));
                }
            }
        });
        ui.menu_button("+ センサー", |ui| {
            for kind in SensorKind::ALL {
                if ui.button(kind.label()).clicked() {
                    state.select_sensor(chart.add_sensor(kind));
                }
            }
        });
        if ui.button("サンプル装置").clicked() {
            load_demo = Some(Demo::Gantry);
        }
        if ui.button("ロボットのサンプル").clicked() {
            load_demo = Some(Demo::Robot);
        }
    });

    // サイクルの最後で原点に戻っていないと、繰り返し再生で位置が飛ぶので知らせる
    let not_home: Vec<&str> = chart
        .actuators
        .iter()
        .filter(|a| schedule.final_value(a).abs() > 1e-9)
        .map(|a| a.name.as_str())
        .collect();
    interlock_status(ui, chart, schedule, play);
    if !not_home.is_empty() && schedule.stop.is_none() {
        ui.colored_label(
            WARN_COLOR,
            format!(
                "⚠ サイクル終了時に原点へ戻っていません: {}",
                not_home.join(", ")
            ),
        );
    }
    let unreachable: Vec<String> = schedule
        .moves
        .iter()
        .filter(|m| !m.reached)
        .filter_map(|m| {
            let robot = chart.robot(m.robot)?;
            let point = robot.point(m.point)?;
            Some(format!("S{} {}", m.step + 1, point.name))
        })
        .collect();
    if !unreachable.is_empty() {
        ui.colored_label(
            ERROR_COLOR,
            format!(
                "⚠ ロボットが届かない教示点があります (可動範囲か腕の長さが足りません): {}",
                unreachable.join(", ")
            ),
        );
    }
    load_demo
}

/// インターロックの条件待ち・異常停止を知らせる
fn interlock_status(ui: &mut egui::Ui, chart: &TimingChart, schedule: &Schedule, play: &Playback) {
    let describe = |interlocks: &[super::chart::Interlock]| {
        interlocks
            .iter()
            .map(|i| {
                let name = chart
                    .sensor(i.sensor)
                    .map_or("(削除済み)", |s| s.name.as_str());
                format!("{name} が {}", if i.on { "ON" } else { "OFF" })
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    if let Some(stop) = &schedule.stop {
        let text = format!(
            "⚠ S{} のインターロックが {INTERLOCK_TIMEOUT:.0} 秒たっても成立せず、異常停止します: {} にならない",
            stop.step + 1,
            describe(&stop.unmet)
        );
        ui.colored_label(STOP_COLOR, text);
        return;
    }
    // 再生位置がちょうど条件待ちのところなら、何を待っているかを出す
    let Some(index) = schedule.step_at(play.time) else {
        return;
    };
    let (Some(span), Some(step)) = (schedule.steps.get(index), chart.steps.get(index)) else {
        return;
    };
    if play.time < span.action_start {
        ui.colored_label(
            INTERLOCK_COLOR,
            format!("S{} 条件待ち: {}", index + 1, describe(&step.interlocks)),
        );
    }
}

// ---------------------------------------------------------------------------
// チャート本体
// ---------------------------------------------------------------------------

/// チャートを描くのに使う、再生前に計算済みのデータ
struct ChartView<'a> {
    schedule: &'a Schedule,
    timeline: &'a SensorTimeline,
    states: &'a SensorStates,
}

/// チャート上の座標と時刻の対応 (列 = ステップ、列の中は時間に比例)
struct ChartLayout {
    origin: egui::Pos2,
    column_width: f32,
}

impl ChartLayout {
    fn column_x(&self, step: usize) -> f32 {
        self.origin.x + NAME_WIDTH + step as f32 * self.column_width
    }

    fn row_top(&self, row: usize) -> f32 {
        self.origin.y + HEADER_HEIGHT + row as f32 * ROW_HEIGHT
    }

    fn time_to_x(&self, schedule: &Schedule, t: f64) -> f32 {
        // 異常停止したときは、止まったステップより後の列 (長さ 0) には進まない
        let last = schedule.last_step().unwrap_or(0);
        for (i, span) in schedule.steps.iter().enumerate() {
            if t < span.end || i == last {
                let length = span.end - span.start;
                let fraction = if length > 0.0 {
                    ((t - span.start) / length).clamp(0.0, 1.0)
                } else {
                    // 時間 0 のステップは、そこを過ぎていれば列の右端に置く
                    if t >= span.end { 1.0 } else { 0.0 }
                };
                return self.column_x(i) + fraction as f32 * self.column_width;
            }
        }
        self.column_x(0)
    }

    fn x_to_time(&self, schedule: &Schedule, x: f32) -> f64 {
        let Some(last) = schedule.last_step() else {
            return 0.0;
        };
        let column = (x - self.column_x(0)) / self.column_width;
        let step = (column.floor().max(0.0) as usize).min(last);
        let fraction = (column - step as f32).clamp(0.0, 1.0) as f64;
        let span = schedule.steps[step];
        span.start + fraction * (span.end - span.start)
    }

    /// 座標がどのステップ列にあるか
    fn step_at_x(&self, x: f32, step_count: usize) -> Option<usize> {
        let column = (x - self.column_x(0)) / self.column_width;
        (column >= 0.0 && (column as usize) < step_count).then_some(column as usize)
    }
}

fn chart_view(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    view: &ChartView,
    play: &mut Playback,
    state: &mut ChartUiState,
) {
    let schedule = view.schedule;
    if chart.steps.is_empty() || row_count(chart) == 0 {
        ui.label(
            "上のボタンでアクチュエータとステップを追加するか、「サンプル装置」を押してください。",
        );
        if chart.steps.is_empty() && row_count(chart) == 0 {
            return;
        }
    }
    let step_count = chart.steps.len().max(1);
    let column_width =
        ((ui.available_width() - NAME_WIDTH) / step_count as f32).max(MIN_COLUMN_WIDTH);
    let size = egui::vec2(
        NAME_WIDTH + column_width * step_count as f32,
        HEADER_HEIGHT + ROW_HEIGHT * row_count(chart) as f32 + 2.0,
    );
    let (response, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
    let layout = ChartLayout {
        origin: response.rect.min,
        column_width,
    };

    handle_chart_input(&response, &layout, chart, schedule, play, state);
    draw_chart(&painter, &layout, chart, view, play, state, ui.visuals());
}

fn handle_chart_input(
    response: &egui::Response,
    layout: &ChartLayout,
    chart: &mut TimingChart,
    schedule: &Schedule,
    play: &mut Playback,
    state: &mut ChartUiState,
) {
    let Some(pos) = response.interact_pointer_pos() else {
        return;
    };
    let in_steps = pos.x >= layout.column_x(0);

    // ドラッグ: 再生位置を動かす
    if response.dragged() && in_steps {
        play.time = layout.x_to_time(schedule, pos.x);
        play.playing = false;
        return;
    }
    if !(response.clicked() || response.double_clicked()) {
        return;
    }

    let step = layout.step_at_x(pos.x, chart.steps.len());
    let row = ((pos.y - layout.row_top(0)) / ROW_HEIGHT).floor();
    let row = (row >= 0.0)
        .then(|| rows(chart).get(row as usize).copied())
        .flatten();

    if pos.y < layout.row_top(0) {
        // 見出しのクリック: そのステップを選び、ステップの開始へ移動
        if let Some(step) = step {
            state.selected_step = Some(step);
            play.time = schedule.steps[step].start;
            play.playing = false;
        }
        return;
    }
    match row {
        Some(Row::Actuator(id)) => state.select_actuator(id),
        Some(Row::Sensor(id)) => state.select_sensor(id),
        Some(Row::Robot(id)) => state.select_robot(id),
        None => {}
    }
    if in_steps {
        state.selected_step = step;
    }
    if !response.double_clicked() {
        return;
    }
    match (step, row) {
        (Some(step), Some(Row::Actuator(id))) => quick_toggle_action(chart, schedule, step, id),
        (Some(step), Some(Row::Sensor(id))) => quick_toggle_interlock(&mut chart.steps[step], id),
        (Some(step), Some(Row::Robot(id))) => add_robot_move(chart, step, id),
        _ => {}
    }
}

/// センサーのセルのダブルクリック: 開始条件を ON 待ち → OFF 待ち → なし の順に切り替える
fn quick_toggle_interlock(step: &mut Step, sensor: u32) {
    let next = match step.interlock(sensor).map(|i| i.on) {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    };
    step.set_interlock(sensor, next);
}

/// ロボットの移動を置く。教示点は、直前に移動した点の次 (なければ最初の点)。
fn add_robot_move(chart: &mut TimingChart, step: usize, robot: u32) {
    if chart.steps[step].robot_move(robot).is_some() {
        return;
    }
    let Some(r) = chart.robot(robot) else {
        return;
    };
    let previous = chart.steps[..step]
        .iter()
        .rev()
        .find_map(|s| s.robot_move(robot))
        .and_then(|m| r.points.iter().position(|p| p.id == m.point));
    let index = previous.map_or(0, |i| (i + 1) % r.points.len().max(1));
    let Some(point) = r.points.get(index) else {
        return;
    };
    let point = point.id;
    chart.steps[step].moves.push(RobotMove {
        robot,
        point,
        duration: 1.0,
        kind: Default::default(),
    });
}

/// セルのダブルクリック: シリンダは 出 → 戻 → なし、軸は (なければ) 動作を追加
fn quick_toggle_action(chart: &mut TimingChart, schedule: &Schedule, step: usize, id: u32) {
    let Some(actuator) = chart.actuator(id).cloned() else {
        return;
    };
    let start = schedule.steps[step].start;
    let current = schedule.value_at(&actuator, start);
    let cell = &mut chart.steps[step];
    match (actuator.kind, cell.action(id).map(|a| a.target)) {
        (ActuatorKind::Cylinder, None) => {
            let target = if current >= 0.5 { 0.0 } else { 1.0 };
            cell.actions.push(new_action(&actuator, target));
        }
        (ActuatorKind::Cylinder, Some(target)) if target >= 0.5 => {
            cell.action_mut(id).unwrap().target = 0.0;
        }
        (ActuatorKind::Cylinder, Some(_)) => cell.actions.retain(|a| a.actuator != id),
        (_, None) => cell.actions.push(new_action(&actuator, current)),
        (_, Some(_)) => {}
    }
}

fn new_action(actuator: &Actuator, target: f64) -> Action {
    Action {
        actuator: actuator.id,
        target,
        duration: actuator.default_duration,
    }
}

/// 値の表示範囲 (0 と全目標値を含む。ロボットの移動で決まった値も含める)
fn value_range(actuator: &Actuator, schedule: &Schedule) -> (f64, f64) {
    if actuator.kind == ActuatorKind::Cylinder {
        return (0.0, 1.0);
    }
    let targets = schedule
        .segments
        .get(&actuator.id)
        .into_iter()
        .flatten()
        .map(|s| s.target);
    let (lo, hi) = targets.fold((0.0f64, 0.0f64), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if hi - lo < 1e-9 {
        (lo, lo + 1.0)
    } else {
        (lo, hi)
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_chart(
    painter: &egui::Painter,
    layout: &ChartLayout,
    chart: &TimingChart,
    view: &ChartView,
    play: &Playback,
    state: &ChartUiState,
    visuals: &egui::Visuals,
) {
    let schedule = view.schedule;
    let text_color = visuals.text_color();
    let weak = visuals.weak_text_color();
    let grid = egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    let small = egui::FontId::proportional(11.0);
    let normal = egui::FontId::proportional(13.0);
    let left = layout.origin.x;
    let right = layout.column_x(chart.steps.len());
    let bottom = layout.row_top(row_count(chart));
    let current_step = schedule.step_at(play.time);

    // 見出し: ステップ番号と、そのステップにかかる時間
    for (i, step) in chart.steps.iter().enumerate() {
        let x = layout.column_x(i);
        let rect = egui::Rect::from_min_max(
            egui::pos2(x, layout.origin.y),
            egui::pos2(x + layout.column_width, layout.row_top(0)),
        );
        if current_step == Some(i) {
            painter.rect_filled(
                rect,
                0.0,
                egui::Color32::from_rgba_unmultiplied(230, 70, 60, 40),
            );
        }
        if state.selected_step == Some(i) {
            painter.rect_stroke(
                rect.shrink(1.0),
                2.0,
                (1.5, SELECT_COLOR),
                egui::StrokeKind::Inside,
            );
        }
        painter.text(
            egui::pos2(x + 4.0, layout.origin.y + 2.0),
            egui::Align2::LEFT_TOP,
            format!("S{}", i + 1),
            normal.clone(),
            text_color,
        );
        // 条件待ちを含めた実際の時間。異常停止したステップとその後は印を出す
        // (チャートを編集した直後は時刻表がまだ古いことがあるので get で引く)
        let length = schedule
            .steps
            .get(i)
            .map_or(0.0, |span| span.end - span.start);
        let (time_text, time_color) = match &schedule.stop {
            Some(stop) if stop.step == i => ("停止".to_string(), STOP_COLOR),
            Some(stop) if stop.step < i => ("未実行".to_string(), weak),
            _ => (format!("{length:.2}s"), weak),
        };
        painter.text(
            egui::pos2(x + 4.0, layout.origin.y + 18.0),
            egui::Align2::LEFT_TOP,
            time_text,
            small.clone(),
            time_color,
        );
        if !step.interlocks.is_empty() {
            painter.text(
                egui::pos2(x + layout.column_width - 4.0, layout.origin.y + 2.0),
                egui::Align2::RIGHT_TOP,
                "条件",
                small.clone(),
                INTERLOCK_COLOR,
            );
        }
    }

    // 行: アクチュエータ名・現在値・動作の帯・波形
    for (row, actuator) in chart.actuators.iter().enumerate() {
        let top = layout.row_top(row);
        let row_rect =
            egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + ROW_HEIGHT));
        if row % 2 == 1 {
            painter.rect_filled(row_rect, 0.0, visuals.faint_bg_color);
        }
        if state.selected_actuator == Some(actuator.id) {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    row_rect.min,
                    egui::pos2(left + NAME_WIDTH, row_rect.max.y),
                ),
                0.0,
                visuals.selection.bg_fill.gamma_multiply(0.5),
            );
        }
        painter.text(
            egui::pos2(left + 4.0, top + 1.0),
            egui::Align2::LEFT_TOP,
            &actuator.name,
            normal.clone(),
            text_color,
        );
        painter.text(
            egui::pos2(left + 4.0, top + 17.5),
            egui::Align2::LEFT_TOP,
            format!(
                "{}  {}",
                actuator.kind.label(),
                actuator.format_value(schedule.value_at(actuator, play.time))
            ),
            small.clone(),
            weak,
        );

        // 動作の帯 (動いている時間の範囲) と目標値
        let mut labels = Vec::new();
        for (i, step) in chart.steps.iter().enumerate() {
            let cell = egui::Rect::from_min_max(
                egui::pos2(layout.column_x(i), top),
                egui::pos2(layout.column_x(i) + layout.column_width, top + ROW_HEIGHT),
            );
            // 異常停止で実行されない動作は帯を描かない (目標値の文字だけ出す)
            let executed = schedule.stop.as_ref().is_none_or(|stop| i < stop.step);
            let span = schedule.steps.get(i).filter(|_| executed);
            if let Some(action) = step.action(actuator.id) {
                if let Some(span) = span {
                    // 条件待ちがあれば、帯は動作を始めた時刻から
                    let start_x = layout.time_to_x(schedule, span.action_start);
                    let end_x = layout.time_to_x(schedule, span.action_start + action.duration);
                    let end_x = end_x.clamp(start_x + 3.0, cell.max.x);
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(start_x + 1.0, cell.min.y + 2.0),
                            egui::pos2(end_x - 1.0, cell.max.y - 2.0),
                        ),
                        2.0,
                        ACTION_FILL,
                    );
                }
                // 目標値は波形の上に重ねるので、描くのは波形の後
                labels.push((
                    egui::pos2(cell.max.x - 3.0, cell.min.y + 2.0),
                    actuator.format_value(action.target),
                ));
            }
            if state.selected_step == Some(i) && state.selected_actuator == Some(actuator.id) {
                painter.rect_stroke(
                    cell.shrink(1.0),
                    2.0,
                    (1.5, SELECT_COLOR),
                    egui::StrokeKind::Inside,
                );
            }
        }

        // 波形: 各列を時間で細かく区切って値をなぞる
        let (lo, hi) = value_range(actuator, schedule);
        let y_of = |v: f64| {
            let fraction = ((v - lo) / (hi - lo)) as f32;
            top + ROW_HEIGHT - 5.0 - fraction * (ROW_HEIGHT - 10.0)
        };
        let mut points = Vec::new();
        for span in &schedule.steps {
            const SAMPLES: usize = 16;
            for k in 0..=SAMPLES {
                let t = span.start + (span.end - span.start) * k as f64 / SAMPLES as f64;
                let x = layout.time_to_x(schedule, t);
                points.push(egui::pos2(x, y_of(schedule.value_at(actuator, t))));
            }
        }
        if points.len() >= 2 {
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(2.0, WAVE_COLOR),
            ));
        }
        for (pos, text) in labels {
            let galley = painter.layout_no_wrap(text, small.clone(), text_color);
            let rect = egui::Align2::RIGHT_TOP.anchor_size(pos, galley.size());
            painter.rect_filled(rect.expand(1.0), 2.0, visuals.extreme_bg_color);
            painter.galley(rect.min, galley, text_color);
        }
        painter.line_segment(
            [
                egui::pos2(left, top + ROW_HEIGHT),
                egui::pos2(right, top + ROW_HEIGHT),
            ],
            grid,
        );
    }

    draw_sensor_rows(painter, layout, chart, view, state, visuals);
    draw_waits_and_stop(painter, layout, schedule, bottom);

    // ロボットの行: 教示点への移動の帯と、移動先の点の名前
    for (index, robot) in chart.robots.iter().enumerate() {
        let row = chart.actuators.len() + chart.sensors.len() + index;
        let top = layout.row_top(row);
        let row_rect =
            egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + ROW_HEIGHT));
        if row % 2 == 1 {
            painter.rect_filled(row_rect, 0.0, visuals.faint_bg_color);
        }
        if state.selected_robot == Some(robot.id) {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    row_rect.min,
                    egui::pos2(left + NAME_WIDTH, row_rect.max.y),
                ),
                0.0,
                visuals.selection.bg_fill.gamma_multiply(0.5),
            );
        }
        painter.text(
            egui::pos2(left + 4.0, top + 1.0),
            egui::Align2::LEFT_TOP,
            &robot.name,
            normal.clone(),
            text_color,
        );
        painter.text(
            egui::pos2(left + 4.0, top + 17.5),
            egui::Align2::LEFT_TOP,
            format!("ロボット  教示点 {}", robot.points.len()),
            small.clone(),
            weak,
        );
        for (i, step) in chart.steps.iter().enumerate() {
            let cell = egui::Rect::from_min_max(
                egui::pos2(layout.column_x(i), top),
                egui::pos2(layout.column_x(i) + layout.column_width, top + ROW_HEIGHT),
            );
            if let Some(robot_move) = step.robot_move(robot.id) {
                // 異常停止で実行されない移動は帯を描かない。条件待ちがあれば帯は動作開始から
                let executed = schedule.stop.as_ref().is_none_or(|stop| i < stop.step);
                if let Some(span) = schedule.steps.get(i).filter(|_| executed) {
                    let start_x = layout.time_to_x(schedule, span.action_start);
                    let end_x = layout.time_to_x(schedule, span.action_start + robot_move.duration);
                    let end_x = end_x.clamp(start_x + 3.0, cell.max.x);
                    let band = egui::Rect::from_min_max(
                        egui::pos2(start_x + 1.0, cell.min.y + 2.0),
                        egui::pos2(end_x - 1.0, cell.max.y - 2.0),
                    );
                    painter.rect_filled(band, 2.0, MOVE_FILL);
                }
                let solved = schedule.solved_move(i, robot.id);
                let name = robot
                    .point(robot_move.point)
                    .map_or("?", |p| p.name.as_str());
                // 直線補間は「L→」。届かない点は赤、特異点の近くを通るときは黄色の ⚠
                let arrow = match robot_move.kind {
                    MoveKind::Joint => "→",
                    MoveKind::Linear => "L→",
                };
                let (text, color) = match solved {
                    Some(m) if !m.reached || !m.path_reached => (format!("⚠ {name}"), ERROR_COLOR),
                    Some(m) if m.singular => (format!("⚠ {arrow} {name}"), WARN_COLOR),
                    _ => (format!("{arrow} {name}"), text_color),
                };
                painter.text(
                    cell.min + egui::vec2(4.0, ROW_HEIGHT / 2.0),
                    egui::Align2::LEFT_CENTER,
                    text,
                    small.clone(),
                    color,
                );
            }
            if state.selected_step == Some(i) && state.selected_robot == Some(robot.id) {
                painter.rect_stroke(
                    cell.shrink(1.0),
                    2.0,
                    (1.5, SELECT_COLOR),
                    egui::StrokeKind::Inside,
                );
            }
        }
        painter.line_segment(
            [
                egui::pos2(left, top + ROW_HEIGHT),
                egui::pos2(right, top + ROW_HEIGHT),
            ],
            grid,
        );
    }

    // 縦の区切り線
    for i in 0..=chart.steps.len() {
        let x = layout.column_x(i);
        painter.line_segment(
            [egui::pos2(x, layout.origin.y), egui::pos2(x, bottom)],
            grid,
        );
    }
    painter.line_segment(
        [
            egui::pos2(left, layout.row_top(0)),
            egui::pos2(right, layout.row_top(0)),
        ],
        grid,
    );

    // 再生位置
    if !chart.steps.is_empty() {
        let x = layout.time_to_x(schedule, play.time);
        painter.line_segment(
            [egui::pos2(x, layout.origin.y), egui::pos2(x, bottom)],
            egui::Stroke::new(2.0, PLAYHEAD_COLOR),
        );
        let tip = egui::pos2(x, layout.row_top(0));
        painter.add(egui::Shape::convex_polygon(
            vec![
                tip,
                tip + egui::vec2(-5.0, -7.0),
                tip + egui::vec2(5.0, -7.0),
            ],
            PLAYHEAD_COLOR,
            egui::Stroke::NONE,
        ));
    }
}

/// センサーの行: 名前・現在の ON / OFF・波形・開始条件の印
fn draw_sensor_rows(
    painter: &egui::Painter,
    layout: &ChartLayout,
    chart: &TimingChart,
    view: &ChartView,
    state: &ChartUiState,
    visuals: &egui::Visuals,
) {
    let schedule = view.schedule;
    let text_color = visuals.text_color();
    let weak = visuals.weak_text_color();
    let grid = egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    let small = egui::FontId::proportional(11.0);
    let normal = egui::FontId::proportional(13.0);
    let left = layout.origin.x;
    let right = layout.column_x(chart.steps.len());

    for (k, sensor) in chart.sensors.iter().enumerate() {
        let row = chart.actuators.len() + k;
        let top = layout.row_top(row);
        let row_rect =
            egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, top + ROW_HEIGHT));
        if row % 2 == 1 {
            painter.rect_filled(row_rect, 0.0, visuals.faint_bg_color);
        }
        if state.selected_sensor == Some(sensor.id) {
            painter.rect_filled(
                egui::Rect::from_min_max(
                    row_rect.min,
                    egui::pos2(left + NAME_WIDTH, row_rect.max.y),
                ),
                0.0,
                visuals.selection.bg_fill.gamma_multiply(0.5),
            );
        }
        painter.text(
            egui::pos2(left + 4.0, top + 1.0),
            egui::Align2::LEFT_TOP,
            &sensor.name,
            normal.clone(),
            text_color,
        );
        let on = view.states.is_on(sensor.id);
        let forced = match sensor.force {
            Some(true) => " (強制ON)",
            Some(false) => " (強制OFF)",
            None => "",
        };
        painter.text(
            egui::pos2(left + 4.0, top + 17.5),
            egui::Align2::LEFT_TOP,
            format!(
                "{}  {}{forced}",
                sensor.kind.label(),
                if on { "ON" } else { "OFF" }
            ),
            small.clone(),
            if on { SENSOR_ON_COLOR } else { weak },
        );

        // 波形: ON の区間を塗り、ON / OFF の2段で線を引く
        let high = top + 7.0;
        let low = top + ROW_HEIGHT - 6.0;
        let samples = view.timeline.samples.get(&sensor.id);
        let mut points: Vec<egui::Pos2> = Vec::new();
        for &(t, on) in samples.into_iter().flatten() {
            if t > schedule.cycle_time {
                break;
            }
            let x = layout.time_to_x(schedule, t);
            let y = if on { high } else { low };
            if let Some(&last) = points.last()
                && last.y != y
            {
                points.push(egui::pos2(x, last.y));
            }
            points.push(egui::pos2(x, y));
        }
        for pair in points.windows(2) {
            if pair[0].y == high && pair[1].y == high && pair[1].x > pair[0].x {
                painter.rect_filled(
                    egui::Rect::from_min_max(pair[0], egui::pos2(pair[1].x, low)),
                    0.0,
                    SENSOR_ON_COLOR.gamma_multiply(0.25),
                );
            }
        }
        if points.len() >= 2 {
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(2.0, SENSOR_ON_COLOR),
            ));
        }

        // 開始条件の印 (列の左端に「ON待」「OFF待」)
        for (i, step) in chart.steps.iter().enumerate() {
            let cell = egui::Rect::from_min_max(
                egui::pos2(layout.column_x(i), top),
                egui::pos2(layout.column_x(i) + layout.column_width, top + ROW_HEIGHT),
            );
            if let Some(interlock) = step.interlock(sensor.id) {
                let text = if interlock.on { "ON待" } else { "OFF待" };
                let galley =
                    painter.layout_no_wrap(text.into(), small.clone(), egui::Color32::BLACK);
                let rect = egui::Align2::LEFT_TOP
                    .anchor_size(cell.min + egui::vec2(3.0, 2.0), galley.size())
                    .expand(1.0);
                painter.rect_filled(rect, 2.0, INTERLOCK_COLOR);
                painter.galley(
                    rect.min + egui::vec2(1.0, 1.0),
                    galley,
                    egui::Color32::BLACK,
                );
            }
            if state.selected_step == Some(i) && state.selected_sensor == Some(sensor.id) {
                painter.rect_stroke(
                    cell.shrink(1.0),
                    2.0,
                    (1.5, SELECT_COLOR),
                    egui::StrokeKind::Inside,
                );
            }
        }
        painter.line_segment(
            [
                egui::pos2(left, top + ROW_HEIGHT),
                egui::pos2(right, top + ROW_HEIGHT),
            ],
            grid,
        );
    }
}

/// 条件待ちの時間 (列の左側) を橙色に、異常停止で実行されない列を灰色に塗る
fn draw_waits_and_stop(
    painter: &egui::Painter,
    layout: &ChartLayout,
    schedule: &Schedule,
    bottom: f32,
) {
    let top = layout.row_top(0);
    for (i, span) in schedule.steps.iter().enumerate() {
        let skipped = schedule.stop.as_ref().is_some_and(|stop| i > stop.step);
        if skipped {
            let x = layout.column_x(i);
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(x, top),
                    egui::pos2(x + layout.column_width, bottom),
                ),
                0.0,
                egui::Color32::from_black_alpha(90),
            );
        } else if span.wait() > 0.0 {
            let fill = if schedule.stop.as_ref().is_some_and(|stop| stop.step == i) {
                STOP_COLOR.gamma_multiply(0.25)
            } else {
                WAIT_FILL
            };
            let x0 = layout.column_x(i);
            let x1 = layout.time_to_x(schedule, span.action_start).max(x0 + 2.0);
            painter.rect_filled(
                egui::Rect::from_min_max(egui::pos2(x0, top), egui::pos2(x1, bottom)),
                0.0,
                fill,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 右側の設定欄
// ---------------------------------------------------------------------------

fn properties(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &Schedule,
    model: &Model,
    selection: &Selection,
    state: &mut ChartUiState,
) {
    // 消されたものを選んだままにしない
    if state.selected_step.is_some_and(|i| i >= chart.steps.len()) {
        state.selected_step = None;
    }
    if state
        .selected_actuator
        .is_some_and(|id| chart.actuator(id).is_none())
    {
        state.selected_actuator = None;
    }
    if state
        .selected_sensor
        .is_some_and(|id| chart.sensor(id).is_none())
    {
        state.selected_sensor = None;
    }
    if state
        .selected_robot
        .is_some_and(|id| chart.robot(id).is_none())
    {
        state.selected_robot = None;
    }

    if let (Some(step), Some(id)) = (state.selected_step, state.selected_actuator) {
        cell_editor(ui, chart, schedule, step, id);
        ui.separator();
    }
    if let (Some(step), Some(id)) = (state.selected_step, state.selected_sensor) {
        interlock_editor(ui, chart, step, id);
        ui.separator();
    }
    if let (Some(step), Some(id)) = (state.selected_step, state.selected_robot) {
        robot_cell_editor(ui, chart, schedule, step, id);
        ui.separator();
    }
    if let Some(step) = state.selected_step {
        step_editor(ui, chart, step, state);
        ui.separator();
    }
    if let Some(id) = state.selected_actuator {
        actuator_editor(ui, chart, model, selection, id, state);
    }
    if let Some(id) = state.selected_sensor {
        sensor_editor(ui, chart, id, state);
    }
    if state.selected_robot.is_some() && state.selected_step.is_none() {
        ui.label("ロボットの設定・ジョグ・教示点は右のプロパティ欄の「ロボット」で行います。");
    }
    if state.selected_step.is_none()
        && state.selected_actuator.is_none()
        && state.selected_sensor.is_none()
        && state.selected_robot.is_none()
    {
        ui.label("セルをクリックで編集。\nダブルクリック: シリンダは 出→戻→なし、\n軸は動作を追加、センサーは開始条件を\nON待→OFF待→なし。\nチャート上をドラッグで再生位置を移動。");
    }
}

/// 選択中のセル (ステップ × センサー) の開始条件
fn interlock_editor(ui: &mut egui::Ui, chart: &mut TimingChart, step: usize, sensor: u32) {
    let Some(name) = chart.sensor(sensor).map(|s| s.name.clone()) else {
        return;
    };
    ui.strong(format!("S{} × {name}", step + 1));
    let step = &mut chart.steps[step];
    let mut condition = step.interlock(sensor).map(|i| i.on);
    ui.label("開始条件 (インターロック)");
    ui.horizontal(|ui| {
        ui.selectable_value(&mut condition, None, "なし");
        ui.selectable_value(&mut condition, Some(true), "ON を待つ");
        ui.selectable_value(&mut condition, Some(false), "OFF を待つ");
    });
    if condition != step.interlock(sensor).map(|i| i.on) {
        step.set_interlock(sensor, condition);
    }
    ui.weak(format!(
        "前のステップの完了後、条件が成立するまで動作を始めません。\n{INTERLOCK_TIMEOUT:.0} 秒で成立しなければ異常停止します。"
    ));
}

fn sensor_editor(ui: &mut egui::Ui, chart: &mut TimingChart, id: u32, state: &mut ChartUiState) {
    let mounts: Vec<(u32, String)> = chart
        .actuators
        .iter()
        .map(|a| (a.id, a.name.clone()))
        .collect();
    let mount_name = |mount: Option<u32>| {
        mount
            .and_then(|m| mounts.iter().find(|(c, _)| *c == m))
            .map_or("(固定)".to_string(), |(_, name)| name.clone())
    };
    let Some(sensor) = chart.sensor_mut(id) else {
        return;
    };

    ui.strong("センサー");
    egui::Grid::new("sensor_editor")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("名前");
            ui.text_edit_singleline(&mut sensor.name);
            ui.end_row();

            ui.label("種類");
            egui::ComboBox::from_id_salt("sensor_kind")
                .selected_text(sensor.kind.label())
                .show_ui(ui, |ui| {
                    for kind in SensorKind::ALL {
                        ui.selectable_value(&mut sensor.kind, kind, kind.label());
                    }
                });
            ui.end_row();

            ui.label(match sensor.kind {
                SensorKind::Photoelectric => "投光位置 X/Y/Z",
                SensorKind::Proximity => "検出面 X/Y/Z",
            });
            ui.horizontal(|ui| {
                // 1ピクセルのドラッグで 0.5 (mm など) 動かす
                for v in &mut sensor.position {
                    ui.add(egui::DragValue::new(v).speed(0.5));
                }
            });
            ui.end_row();

            if sensor.kind == SensorKind::Photoelectric {
                ui.label("光軸の向き");
                ui.horizontal(|ui| {
                    for axis in Axis::ALL {
                        ui.selectable_value(&mut sensor.axis, axis, axis.label());
                    }
                    ui.checkbox(&mut sensor.reversed, "逆向き");
                });
                ui.end_row();
            }

            ui.label(match sensor.kind {
                SensorKind::Photoelectric => "光軸の長さ",
                SensorKind::Proximity => "検出距離",
            });
            ui.add(
                egui::DragValue::new(&mut sensor.range)
                    .speed(0.5)
                    .range(0.0..=100_000.0),
            );
            ui.end_row();

            ui.label("取り付け先");
            egui::ComboBox::from_id_salt("sensor_mount")
                .selected_text(mount_name(sensor.mounted_on))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut sensor.mounted_on, None, "(固定)");
                    for (candidate, name) in &mounts {
                        ui.selectable_value(&mut sensor.mounted_on, Some(*candidate), name);
                    }
                });
            ui.end_row();

            ui.label("強制");
            ui.horizontal(|ui| {
                ui.selectable_value(&mut sensor.force, None, "自動");
                ui.selectable_value(&mut sensor.force, Some(true), "ON");
                ui.selectable_value(&mut sensor.force, Some(false), "OFF");
            });
            ui.end_row();
        });
    ui.weak("取り付け先と一緒に動きます。取り付け先が動かす部品は検知しません。");

    ui.horizontal(|ui| {
        let index = chart.sensors.iter().position(|s| s.id == id).unwrap_or(0);
        if ui.button("▲ 上へ").clicked() && index > 0 {
            chart.sensors.swap(index, index - 1);
        }
        if ui.button("▼ 下へ").clicked() && index + 1 < chart.sensors.len() {
            chart.sensors.swap(index, index + 1);
        }
        if ui.button("センサーを削除").clicked() {
            chart.remove_sensor(id);
            state.selected_sensor = None;
        }
    });
}

/// 選択中のセル (ステップ × アクチュエータ) の動作
fn cell_editor(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &Schedule,
    step: usize,
    id: u32,
) {
    let Some(actuator) = chart.actuator(id).cloned() else {
        return;
    };
    ui.strong(format!("S{} × {}", step + 1, actuator.name));
    let cell = &mut chart.steps[step];
    let Some(action) = cell.action_mut(id) else {
        ui.horizontal(|ui| {
            if actuator.kind == ActuatorKind::Cylinder {
                for (label, target) in [("出 を追加", 1.0), ("戻 を追加", 0.0)] {
                    if ui.button(label).clicked() {
                        cell.actions.push(new_action(&actuator, target));
                    }
                }
            } else if ui.button("動作を追加").clicked() {
                let current = schedule.value_at(&actuator, schedule.steps[step].start);
                cell.actions.push(new_action(&actuator, current));
            }
        });
        return;
    };

    egui::Grid::new("cell_editor")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("目標");
            match actuator.kind {
                ActuatorKind::Cylinder => {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut action.target, 1.0, "出 (前進端)");
                        ui.selectable_value(&mut action.target, 0.0, "戻 (後退端)");
                    });
                }
                kind => {
                    // 1ピクセルのドラッグで 0.5 (mm など) / 1° 動かす。正確な値はクリックして入力する。
                    let speed = if kind == ActuatorKind::RotaryTable {
                        1.0
                    } else {
                        0.5
                    };
                    ui.add(
                        egui::DragValue::new(&mut action.target)
                            .speed(speed)
                            .suffix(kind.unit()),
                    );
                }
            }
            ui.end_row();
            ui.label("動作時間");
            ui.add(
                egui::DragValue::new(&mut action.duration)
                    .speed(0.01)
                    .range(0.0..=60.0)
                    .suffix(" s"),
            );
            ui.end_row();
        });
    if ui.button("動作を削除").clicked() {
        cell.actions.retain(|a| a.actuator != id);
    }
}

/// 選択中のセル (ステップ × ロボット) の移動
fn robot_cell_editor(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &Schedule,
    step: usize,
    id: u32,
) {
    let Some(robot) = chart.robot(id).cloned() else {
        return;
    };
    ui.strong(format!("S{} × {}", step + 1, robot.name));
    if robot.points.is_empty() {
        ui.label("教示点がありません。「ロボット」画面で登録してください。");
        return;
    }
    if chart.steps[step].robot_move(id).is_none() {
        if ui.button("教示点への移動を追加").clicked() {
            add_robot_move(chart, step, id);
        }
        return;
    }
    let robot_move = chart.steps[step].robot_move_mut(id).unwrap();
    egui::Grid::new("robot_cell_editor")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("移動先");
            let current = robot
                .point(robot_move.point)
                .map_or("?".to_string(), |p| p.name.clone());
            egui::ComboBox::from_id_salt("move_point")
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for point in &robot.points {
                        ui.selectable_value(&mut robot_move.point, point.id, &point.name);
                    }
                });
            ui.end_row();
            ui.label("補間");
            egui::ComboBox::from_id_salt("move_kind")
                .selected_text(robot_move.kind.label())
                .show_ui(ui, |ui| {
                    for kind in [MoveKind::Joint, MoveKind::Linear] {
                        ui.selectable_value(&mut robot_move.kind, kind, kind.label());
                    }
                });
            ui.end_row();
            ui.label("動作時間");
            ui.add(
                egui::DragValue::new(&mut robot_move.duration)
                    .speed(0.01)
                    .range(0.0..=60.0)
                    .suffix(" s"),
            );
            ui.end_row();
        });
    if let Some(solved) = schedule.solved_move(step, id) {
        if solved.ok() {
            ui.weak(match solved.kind {
                MoveKind::Joint => "各関節を同時に動かして教示点へ移動します。",
                MoveKind::Linear => "TCP をまっすぐ教示点へ動かします。",
            });
        }
        if !solved.reached {
            ui.colored_label(
                ERROR_COLOR,
                format!(
                    "⚠ 届きません (位置 {:.3} / 向き {:.1}° ずれ)",
                    solved.position_error, solved.angle_error_deg
                ),
            );
        } else if !solved.path_reached {
            ui.colored_label(ERROR_COLOR, "⚠ 直線の途中に届かないところがあります");
        }
        if solved.singular {
            ui.colored_label(
                WARN_COLOR,
                format!(
                    "⚠ 特異点の近くを通ります (動かしやすさ {:.2})。関節が急に大きく回ることがあります",
                    solved.min_dexterity
                ),
            );
        }
    }
    if ui.button("移動を削除").clicked() {
        chart.steps[step].moves.retain(|m| m.robot != id);
    }
}

fn step_editor(ui: &mut egui::Ui, chart: &mut TimingChart, index: usize, state: &mut ChartUiState) {
    ui.strong(format!("ステップ S{}", index + 1));
    let step = &mut chart.steps[index];
    egui::Grid::new("step_editor")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("コメント");
            ui.text_edit_singleline(&mut step.comment);
            ui.end_row();
            ui.label("タイマー");
            ui.add(
                egui::DragValue::new(&mut step.dwell)
                    .speed(0.01)
                    .range(0.0..=60.0)
                    .suffix(" s"),
            );
            ui.end_row();
        });
    ui.horizontal(|ui| {
        if ui.button("◀ 前へ").clicked() && index > 0 {
            chart.steps.swap(index, index - 1);
            state.selected_step = Some(index - 1);
        }
        if ui.button("後へ ▶").clicked() && index + 1 < chart.steps.len() {
            chart.steps.swap(index, index + 1);
            state.selected_step = Some(index + 1);
        }
        if ui.button("複製").clicked() {
            let copy = chart.steps[index].clone();
            chart.steps.insert(index + 1, copy);
            state.selected_step = Some(index + 1);
        }
        if ui.button("ステップを削除").clicked() {
            chart.steps.remove(index);
            state.selected_step = None;
        }
    });
}

fn actuator_editor(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    model: &Model,
    selection: &Selection,
    id: u32,
    state: &mut ChartUiState,
) {
    // 載せ先の候補: 自分自身と、自分の上に載っているもの以外
    let mount_candidates: Vec<(u32, String)> = chart
        .actuators
        .iter()
        .filter(|a| a.id != id && !chart.is_mounted_under(a.id, id))
        .map(|a| (a.id, a.name.clone()))
        .collect();
    let mount_name = |mount: Option<u32>| {
        mount
            .and_then(|m| mount_candidates.iter().find(|(c, _)| *c == m))
            .map_or("(固定)".to_string(), |(_, name)| name.clone())
    };
    let Some(actuator) = chart.actuator_mut(id) else {
        return;
    };

    ui.strong("アクチュエータ");
    egui::Grid::new("actuator_editor")
        .num_columns(2)
        .show(ui, |ui| {
            ui.label("名前");
            ui.text_edit_singleline(&mut actuator.name);
            ui.end_row();

            ui.label("種類");
            egui::ComboBox::from_id_salt("actuator_kind")
                .selected_text(actuator.kind.label())
                .show_ui(ui, |ui| {
                    for kind in ActuatorKind::ALL {
                        ui.selectable_value(&mut actuator.kind, kind, kind.label());
                    }
                });
            ui.end_row();

            ui.label(if actuator.kind == ActuatorKind::RotaryTable {
                "回転軸"
            } else {
                "方向"
            });
            ui.horizontal(|ui| {
                for axis in Axis::ALL {
                    ui.selectable_value(&mut actuator.axis, axis, axis.label());
                }
                ui.checkbox(&mut actuator.reversed, "逆向き");
            });
            ui.end_row();

            match actuator.kind {
                ActuatorKind::Cylinder => {
                    ui.label("ストローク");
                    ui.add(
                        egui::DragValue::new(&mut actuator.stroke)
                            .speed(0.01)
                            .range(0.0..=100.0),
                    );
                    ui.end_row();
                }
                ActuatorKind::RotaryTable => {
                    ui.label("回転中心 X/Y/Z");
                    ui.horizontal(|ui| {
                        for v in &mut actuator.pivot {
                            ui.add(egui::DragValue::new(v).speed(0.05));
                        }
                    });
                    ui.end_row();
                }
                ActuatorKind::LinearAxis => {}
            }

            ui.label("加減速");
            egui::ComboBox::from_id_salt("easing")
                .selected_text(actuator.easing.label())
                .show_ui(ui, |ui| {
                    for easing in [Easing::Smooth, Easing::Linear] {
                        ui.selectable_value(&mut actuator.easing, easing, easing.label());
                    }
                });
            ui.end_row();

            ui.label("標準動作時間");
            ui.add(
                egui::DragValue::new(&mut actuator.default_duration)
                    .speed(0.01)
                    .range(0.0..=60.0)
                    .suffix(" s"),
            );
            ui.end_row();

            ui.label("載せ先");
            egui::ComboBox::from_id_salt("mounted_on")
                .selected_text(mount_name(actuator.mounted_on))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut actuator.mounted_on, None, "(固定)");
                    for (candidate, name) in &mount_candidates {
                        ui.selectable_value(&mut actuator.mounted_on, Some(*candidate), name);
                    }
                });
            ui.end_row();

            if actuator.is_joint() {
                ui.label("可動範囲");
                ui.horizontal(|ui| {
                    let mut limited = actuator.limits.is_some();
                    if ui.checkbox(&mut limited, "").changed() {
                        actuator.limits = limited.then(|| match actuator.kind {
                            ActuatorKind::RotaryTable => [-180.0, 180.0],
                            _ => [-1.0, 1.0],
                        });
                    }
                    if let Some([lo, hi]) = &mut actuator.limits {
                        let unit = actuator.kind.unit();
                        ui.add(egui::DragValue::new(lo).speed(0.05).suffix(unit));
                        ui.label("〜");
                        ui.add(egui::DragValue::new(hi).speed(0.05).suffix(unit));
                        *hi = hi.max(*lo);
                    }
                });
                ui.end_row();
            }
        });

    // 動かす部品: ツリーやビューで選んだノードを追加する。アセンブリを追加すると子孫がまとめて動く。
    ui.label("動かす部品:");
    let mut remove: Option<NodeId> = None;
    for &node in &actuator.parts {
        ui.horizontal(|ui| {
            if ui.small_button("✖").on_hover_text("外す").clicked() {
                remove = Some(node);
            }
            match model.get(node) {
                Some(_) => ui.label(model.path_label(node)),
                None => ui.weak(format!("(削除された部品 #{node})")),
            };
        });
    }
    if let Some(node) = remove {
        actuator.parts.retain(|p| *p != node);
    }
    let selected = selection.node.filter(|n| model.get(*n).is_some());
    let can_add = selected.is_some_and(|n| !actuator.parts.contains(&n));
    let add_label = match selected {
        Some(n) => format!(
            "+ 選択中の「{}」を追加",
            model.get(n).map_or("", |n| n.name.as_str())
        ),
        None => "+ 選択中の部品を追加 (ツリーかビューで選択)".to_string(),
    };
    if ui
        .add_enabled(can_add, egui::Button::new(add_label))
        .clicked()
        && let Some(node) = selected
    {
        actuator.parts.push(node);
    }

    ui.horizontal(|ui| {
        let index = chart.actuators.iter().position(|a| a.id == id).unwrap_or(0);
        if ui.button("▲ 上へ").clicked() && index > 0 {
            chart.actuators.swap(index, index - 1);
        }
        if ui.button("▼ 下へ").clicked() && index + 1 < chart.actuators.len() {
            chart.actuators.swap(index, index + 1);
        }
        if ui.button("アクチュエータを削除").clicked() {
            chart.remove_actuator(id);
            state.selected_actuator = None;
        }
    });
}
