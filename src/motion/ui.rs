//! タイミングチャートの編集画面と再生コントロール
//!
//! 画面下のウィンドウに、行 = アクチュエータ、列 = ステップのチャートを描く。
//! 列の幅はステップごとに同じで、列の中では時間に比例して波形 (各アクチュエータの値) を描く。
//!
//! - セルをクリック: そのステップ × アクチュエータを選び、右側で動作を編集
//! - セルをダブルクリック: シリンダは 出 → 戻 → なし を切り替え、軸は動作を追加
//! - チャート上をドラッグ: 再生位置を動かす (スクラブ)

use bevy::{ecs::system::SystemParam, prelude::*};
use bevy_egui::egui;

use super::chart::{Action, Actuator, ActuatorKind, Axis, Easing, Schedule, Step, TimingChart};
use super::{ChartSchedule, Playback, demo};
use crate::camera::CameraRequest;
use crate::conveyor::ConveyorConfigRes;
use crate::model::{Model, NodeId, Selection};

const NAME_WIDTH: f32 = 150.0;
const HEADER_HEIGHT: f32 = 34.0;
const ROW_HEIGHT: f32 = 32.0;
const MIN_COLUMN_WIDTH: f32 = 64.0;
const PROPERTIES_WIDTH: f32 = 330.0;
const PANEL_HEIGHT: f32 = 250.0;

const PLAYHEAD_COLOR: egui::Color32 = egui::Color32::from_rgb(230, 70, 60);
const WAVE_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 170, 255);
const ACTION_FILL: egui::Color32 = egui::Color32::from_rgba_premultiplied(60, 110, 170, 70);
const SELECT_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 170, 60);

#[derive(Resource, Default)]
pub struct ChartUiState {
    selected_actuator: Option<u32>,
    selected_step: Option<usize>,
    /// パネルをツールバーだけに畳んでいる
    collapsed: bool,
}

/// タイミングチャートのパネルが読み書きするもの一式
#[derive(SystemParam)]
pub struct MotionUi<'w> {
    chart: ResMut<'w, TimingChart>,
    schedule: Res<'w, ChartSchedule>,
    playback: ResMut<'w, Playback>,
    state: ResMut<'w, ChartUiState>,
    /// サンプル装置を読み込むときに、コンベアとグリッパも置き換える
    conveyors: ResMut<'w, ConveyorConfigRes>,
}

/// 画面下のタイミングチャートのパネル。ステータスバーの上、左右のパネルより外側に置く。
pub fn show_panel(
    root: &mut egui::Ui,
    motion: &mut MotionUi,
    model: &mut Model,
    selection: &Selection,
    camera: &mut MessageWriter<CameraRequest>,
) {
    // 編集はコピーに対して行い、変わったときだけ書き戻す (毎フレーム変更扱いにしないため)
    let mut edited = motion.chart.clone();
    let mut play = *motion.playback;
    let schedule = &motion.schedule;
    let state = &mut *motion.state;
    let mut load_demo = false;

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
                        chart_view(ui, &mut edited, schedule, &mut play, state);
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

    if load_demo {
        demo::load_demo(model, &mut edited, &mut motion.conveyors);
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

/// 「サンプル装置」が押されたら true を返す
fn toolbar(
    ui: &mut egui::Ui,
    chart: &mut TimingChart,
    schedule: &Schedule,
    play: &mut Playback,
    state: &mut ChartUiState,
) -> bool {
    let mut load_demo = false;
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
                    state.selected_actuator = Some(chart.add_actuator(kind));
                }
            }
        });
        if ui.button("サンプル装置").clicked() {
            load_demo = true;
        }
    });

    // サイクルの最後で原点に戻っていないと、繰り返し再生で位置が飛ぶので知らせる
    let not_home: Vec<&str> = chart
        .actuators
        .iter()
        .filter(|a| schedule.final_value(a).abs() > 1e-9)
        .map(|a| a.name.as_str())
        .collect();
    if !not_home.is_empty() {
        ui.colored_label(
            egui::Color32::from_rgb(240, 180, 60),
            format!(
                "⚠ サイクル終了時に原点へ戻っていません: {}",
                not_home.join(", ")
            ),
        );
    }
    load_demo
}

// ---------------------------------------------------------------------------
// チャート本体
// ---------------------------------------------------------------------------

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
        let last = schedule.steps.len().saturating_sub(1);
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
        let Some(last) = schedule.steps.len().checked_sub(1) else {
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
    schedule: &Schedule,
    play: &mut Playback,
    state: &mut ChartUiState,
) {
    if chart.steps.is_empty() || chart.actuators.is_empty() {
        ui.label(
            "上のボタンでアクチュエータとステップを追加するか、「サンプル装置」を押してください。",
        );
        if chart.steps.is_empty() && chart.actuators.is_empty() {
            return;
        }
    }
    let step_count = chart.steps.len().max(1);
    let column_width =
        ((ui.available_width() - NAME_WIDTH) / step_count as f32).max(MIN_COLUMN_WIDTH);
    let size = egui::vec2(
        NAME_WIDTH + column_width * step_count as f32,
        HEADER_HEIGHT + ROW_HEIGHT * chart.actuators.len() as f32 + 2.0,
    );
    let (response, painter) = ui.allocate_painter(size, egui::Sense::click_and_drag());
    let layout = ChartLayout {
        origin: response.rect.min,
        column_width,
    };

    handle_chart_input(&response, &layout, chart, schedule, play, state);
    draw_chart(
        &painter,
        &layout,
        chart,
        schedule,
        play,
        state,
        ui.visuals(),
    );
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
    let actuator = (row >= 0.0)
        .then(|| chart.actuators.get(row as usize))
        .flatten()
        .map(|a| a.id);

    if pos.y < layout.row_top(0) {
        // 見出しのクリック: そのステップを選び、ステップの開始へ移動
        if let Some(step) = step {
            state.selected_step = Some(step);
            play.time = schedule.steps[step].start;
            play.playing = false;
        }
        return;
    }
    if let Some(id) = actuator {
        state.selected_actuator = Some(id);
    }
    if in_steps {
        state.selected_step = step;
    }
    if response.double_clicked()
        && let (Some(step), Some(id)) = (step, actuator)
    {
        quick_toggle_action(chart, schedule, step, id);
    }
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

/// 値の表示範囲 (0 と全目標値を含む)
fn value_range(actuator: &Actuator, chart: &TimingChart) -> (f64, f64) {
    if actuator.kind == ActuatorKind::Cylinder {
        return (0.0, 1.0);
    }
    let targets = chart
        .steps
        .iter()
        .filter_map(|s| s.action(actuator.id))
        .map(|a| a.target);
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
    schedule: &Schedule,
    play: &Playback,
    state: &ChartUiState,
    visuals: &egui::Visuals,
) {
    let text_color = visuals.text_color();
    let weak = visuals.weak_text_color();
    let grid = egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color);
    let small = egui::FontId::proportional(11.0);
    let normal = egui::FontId::proportional(13.0);
    let left = layout.origin.x;
    let right = layout.column_x(chart.steps.len());
    let bottom = layout.row_top(chart.actuators.len());
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
        painter.text(
            egui::pos2(x + 4.0, layout.origin.y + 18.0),
            egui::Align2::LEFT_TOP,
            format!("{:.2}s", step.duration()),
            small.clone(),
            weak,
        );
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
            if let Some(action) = step.action(actuator.id) {
                let span = schedule.steps[i];
                let end_x = layout.time_to_x(schedule, span.start + action.duration);
                let end_x = end_x.clamp(cell.min.x + 3.0, cell.max.x);
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        cell.min + egui::vec2(1.0, 2.0),
                        egui::pos2(end_x - 1.0, cell.max.y - 2.0),
                    ),
                    2.0,
                    ACTION_FILL,
                );
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
        let (lo, hi) = value_range(actuator, chart);
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

    if let (Some(step), Some(id)) = (state.selected_step, state.selected_actuator) {
        cell_editor(ui, chart, schedule, step, id);
        ui.separator();
    }
    if let Some(step) = state.selected_step {
        step_editor(ui, chart, step, state);
        ui.separator();
    }
    if let Some(id) = state.selected_actuator {
        actuator_editor(ui, chart, model, selection, id, state);
    }
    if state.selected_step.is_none() && state.selected_actuator.is_none() {
        ui.label("セルをクリックで編集。\nダブルクリック: シリンダは 出→戻→なし、\n軸は動作を追加。\nチャート上をドラッグで再生位置を移動。");
    }
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
