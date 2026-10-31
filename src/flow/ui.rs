//! 「工程シミュレーション」ウィンドウ
//!
//! 上に実行の操作 (▶ / ⏸ / ⏩ 最後まで / 速度 / 色分け / Excel)、その下にタブ:
//! - 工程: 要素の一覧と、選んだ要素の設定。回す時間など
//! - 人員: シフト・作業者の名簿とできる工程・人数の比較 (`staff_ui.rs`)
//! - 案の比較: 設定を案として取っておき、繰り返し回して並べる (`scenario_ui.rs`)
//! - 結果: ラインの集計と設備ごとの表、作業者ごとの表
//! - グラフ: 状態の割合の積み上げ横棒、排出数と仕掛数の推移、状態の時系列

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Pos2, Rect as ERect, Stroke, Vec2};

use super::model::{ElementId, ElementKind, Failure, FlowModel, KindTag, Labor, sample_line};
use super::palette::{busy_rgb, state_rgb};
use super::scenario_ui::{ScenarioUiState, scenario_tab};
use super::sim::{Report, State, WorkerState};
use super::staff_ui::{StaffUiState, staff_tab};
use super::{ColorMode, FlowRes, FlowRun};
use crate::model::{Model, Selection};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Line,
    Staff,
    Scenarios,
    Results,
    Charts,
}

/// ウィンドウだけが使う状態
#[derive(Resource, Default)]
pub struct FlowUiState {
    tab: Tab,
    selected: Option<ElementId>,
    /// 回せなかった理由
    problems: Vec<String>,
    staff: StaffUiState,
    scenarios: ScenarioUiState,
}

#[derive(SystemParam)]
pub struct FlowUi<'w> {
    pub flow: ResMut<'w, FlowRes>,
    pub run: ResMut<'w, FlowRun>,
    ui: ResMut<'w, FlowUiState>,
}

impl FlowUi<'_> {
    pub fn toggle(&mut self) {
        self.run.open = !self.run.open;
    }

    pub fn load_sample(&mut self) {
        let scenarios = std::mem::take(&mut self.flow.0.scenarios);
        self.flow.0 = sample_line();
        self.flow.0.scenarios = scenarios;
        self.run.stop();
        self.ui.selected = None;
        self.run.open = true;
    }
}

const SPEEDS: [(f64, &str); 6] = [
    (1.0, "x1"),
    (10.0, "x10"),
    (60.0, "x60 (1秒で1分)"),
    (600.0, "x600 (1秒で10分)"),
    (3600.0, "x3600 (1秒で1時間)"),
    (36000.0, "x36000"),
];

pub fn show_window(ctx: &egui::Context, state: &mut FlowUi, model: &Model, selection: &Selection) {
    if !state.run.open {
        return;
    }
    let mut open = true;
    egui::Window::new("工程シミュレーション")
        .open(&mut open)
        .default_size([780.0, 520.0])
        .default_pos([280.0, 120.0])
        .resizable(true)
        .show(ctx, |ui| {
            run_controls(ui, state);
            ui.separator();
            ui.horizontal(|ui| {
                let tab = &mut state.ui.tab;
                ui.selectable_value(tab, Tab::Line, "工程");
                ui.selectable_value(tab, Tab::Staff, "人員");
                ui.selectable_value(tab, Tab::Scenarios, "案の比較");
                ui.selectable_value(tab, Tab::Results, "結果");
                ui.selectable_value(tab, Tab::Charts, "グラフ");
            });
            ui.separator();
            match state.ui.tab {
                Tab::Line => line_tab(ui, state, model, selection),
                Tab::Staff => {
                    if staff_tab(ui, &mut state.flow, &mut state.ui.staff) {
                        state.run.stop();
                        state.ui.selected = None;
                    }
                }
                Tab::Scenarios => {
                    if scenario_tab(ui, &mut state.flow, &mut state.ui.scenarios) {
                        state.run.stop();
                        state.ui.selected = None;
                    }
                }
                Tab::Results => results_tab(ui, state),
                Tab::Charts => charts_tab(ui, state),
            }
        });
    if !open {
        state.run.open = false;
    }
}

// ---------------------------------------------------------------------------
// 実行の操作
// ---------------------------------------------------------------------------

fn run_controls(ui: &mut egui::Ui, state: &mut FlowUi) {
    let FlowUi {
        flow,
        run,
        ui: ui_state,
    } = state;
    ui.horizontal_wrapped(|ui| {
        let needs_restart = run.sim.is_none() || run.is_stale(flow);
        let finished = run.sim.as_ref().is_some_and(|s| s.finished());
        if run.playing {
            if ui.button("⏸ 一時停止").clicked() {
                run.playing = false;
            }
        } else if ui
            .button("▶ 実行")
            .on_hover_text("設定した速度で早送りしながら回す。3D ビューの部品が状態の色になる")
            .clicked()
        {
            start(run, flow, ui_state, needs_restart || finished, None);
        }
        if ui
            .button("⏩ 最後まで")
            .on_hover_text("回す時間の最後まで一気に回して集計する")
            .clicked()
        {
            start(
                run,
                flow,
                ui_state,
                needs_restart || finished,
                Some(f64::INFINITY),
            );
        }
        if ui
            .add_enabled(run.sim.is_some(), egui::Button::new("⏹ リセット"))
            .clicked()
        {
            run.stop();
        }
        ui.separator();
        ui.label("速度");
        let label = SPEEDS
            .iter()
            .find(|(s, _)| *s == run.speed)
            .map_or("最後まで", |(_, l)| l);
        egui::ComboBox::from_id_salt("flow_speed")
            .selected_text(label)
            .show_ui(ui, |ui| {
                for (speed, label) in SPEEDS {
                    ui.selectable_value(&mut run.speed, speed, label);
                }
            });
        ui.separator();
        ui.label("3D の色");
        egui::ComboBox::from_id_salt("flow_color")
            .selected_text(run.color_mode.label())
            .show_ui(ui, |ui| {
                for mode in ColorMode::ALL {
                    ui.selectable_value(&mut run.color_mode, mode, mode.label());
                }
            });
        ui.separator();
        if ui
            .add_enabled(run.report.is_some(), egui::Button::new("Excel に書き出し…"))
            .on_hover_text("集計・設備別の表・推移を xlsx で保存する")
            .clicked()
        {
            run.export_requested = true;
        }
    });

    if let Some(sim) = &run.sim {
        let duration = sim.settings().duration;
        let progress = (sim.now() / duration) as f32;
        ui.add(egui::ProgressBar::new(progress).text(format!(
            "{} / {}",
            hours(sim.now()),
            hours(duration)
        )));
    }
    if run.is_stale(flow) {
        ui.colored_label(
            Color32::from_rgb(230, 170, 60),
            "工程の設定が変わりました。結果は前の設定のものです。▶ で回し直してください。",
        );
    }
    for p in &ui_state.problems {
        ui.colored_label(Color32::from_rgb(230, 90, 90), format!("⚠ {p}"));
    }
}

/// 実行を始める。`speed` を渡すとその速度にする (最後までなら ∞)。
fn start(
    run: &mut FlowRun,
    flow: &FlowModel,
    ui_state: &mut FlowUiState,
    restart: bool,
    speed: Option<f64>,
) {
    if restart && let Err(problems) = run.restart(flow) {
        ui_state.problems = problems;
        ui_state.tab = Tab::Line;
        return;
    }
    ui_state.problems.clear();
    if speed == Some(f64::INFINITY) {
        // 早送りの速度設定は変えずに、今回だけ最後まで回す
        if let Some(sim) = run.sim.as_mut() {
            sim.advance(f64::INFINITY, usize::MAX);
            run.report = Some(sim.report());
        }
        run.playing = false;
        ui_state.tab = Tab::Results;
    } else {
        run.playing = true;
    }
}

fn hours(seconds: f64) -> String {
    let h = (seconds / 3600.0).floor();
    let m = ((seconds - h * 3600.0) / 60.0).floor();
    format!("{h:.0}時間{m:02.0}分")
}

// ---------------------------------------------------------------------------
// 工程タブ
// ---------------------------------------------------------------------------

fn line_tab(ui: &mut egui::Ui, state: &mut FlowUi, model: &Model, selection: &Selection) {
    let FlowUi {
        flow,
        run,
        ui: ui_state,
    } = state;
    ui.horizontal_wrapped(|ui| {
        ui.label("追加:");
        for tag in KindTag::ALL {
            if ui
                .button(format!("+ {}", tag.label()))
                .on_hover_text("末尾に追加し、前の要素からつなぐ")
                .clicked()
            {
                let id = flow.append(tag);
                ui_state.selected = Some(id);
            }
        }
        let picked = selection.node.filter(|&n| model.get(n).is_some());
        if ui
            .add_enabled(
                picked.is_some(),
                egui::Button::new("+ 選択中の部品を工程に"),
            )
            .on_hover_text(
                "3D で選んだ部品やアセンブリを工程として追加する (名前と色分けの対象になる)",
            )
            .clicked()
            && let Some(node) = picked
        {
            let id = flow.append(KindTag::Process);
            let e = flow.get_mut(id).unwrap();
            e.part = Some(node);
            e.name = model.get(node).map_or(e.name.clone(), |n| n.name.clone());
            ui_state.selected = Some(id);
        }
        ui.separator();
        if ui
            .button("サンプル工程")
            .on_hover_text(
                "投入 → 加工 → バッファ → 組立 (故障あり) → 検査 → 排出 のラインに置き換える",
            )
            .clicked()
        {
            let scenarios = std::mem::take(&mut flow.0.scenarios);
            flow.0 = sample_line();
            flow.0.scenarios = scenarios;
            run.stop();
            ui_state.selected = None;
        }
    });
    ui.add_space(4.0);

    if flow.is_empty() {
        ui.label("「+ 生成」「+ 工程」「+ バッファ」「+ 排出」の順に押すと、つながったラインができます。");
        ui.label("試すときは「サンプル工程」を押してください。");
    }

    // 区切り線が下まで伸びないよう、一覧と設定欄の高さを決めておく
    let size = Vec2::new(ui.available_width(), LIST_HEIGHT);
    ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Min), |ui| {
        // 左: 要素の一覧
        ui.vertical(|ui| {
            ui.set_width(260.0);
            egui::ScrollArea::vertical()
                .id_salt("flow_list")
                .max_height(LIST_HEIGHT)
                .show(ui, |ui| element_list(ui, flow, run, ui_state));
        });
        ui.separator();
        // 右: 選んだ要素の設定
        ui.vertical(|ui| {
            egui::ScrollArea::vertical()
                .id_salt("flow_detail")
                .max_height(LIST_HEIGHT)
                .show(ui, |ui| {
                    match ui_state.selected.filter(|&id| flow.get(id).is_some()) {
                        Some(id) => element_editor(ui, flow, ui_state, id, model, selection),
                        None => {
                            ui.weak("左の一覧で要素を選ぶと、ここで設定できます。");
                        }
                    }
                });
        });
    });
    ui.separator();
    settings_editor(ui, flow);
}

fn element_list(ui: &mut egui::Ui, flow: &FlowModel, run: &FlowRun, ui_state: &mut FlowUiState) {
    for e in &flow.elements {
        let summary = match &e.kind {
            ElementKind::Source { interval, .. } => format!("{interval} 秒ごと"),
            ElementKind::Process {
                cycle_time,
                failure,
                ..
            } => format!(
                "{cycle_time} 秒{}",
                if failure.is_some() { " ・故障" } else { "" }
            ),
            ElementKind::Buffer { capacity } => format!("容量 {capacity}"),
            ElementKind::Sink => String::new(),
        };
        let next: Vec<&str> = e
            .next
            .iter()
            .filter_map(|n| flow.get(*n).map(|t| t.name.as_str()))
            .collect();
        let live = run
            .sim
            .as_ref()
            .filter(|_| !run.is_stale(flow))
            .and_then(|s| Some((s.state_of(e.id)?, s.content_of(e.id)?)));
        ui.horizontal(|ui| {
            if let Some((state, _)) = live {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 4.5, color(state_rgb(state)));
            }
            let text = format!("[{}] {}  {summary}", e.kind.tag().label(), e.name);
            if ui
                .selectable_label(ui_state.selected == Some(e.id), text)
                .clicked()
            {
                ui_state.selected = Some(e.id);
            }
            if let Some((_, content)) = live
                && content > 0
            {
                ui.weak(format!("{content}個"));
            }
        });
        if !next.is_empty() {
            ui.weak(format!("      → {}", next.join(", ")));
        }
    }
}

fn element_editor(
    ui: &mut egui::Ui,
    flow: &mut FlowModel,
    ui_state: &mut FlowUiState,
    id: ElementId,
    model: &Model,
    selection: &Selection,
) {
    let others: Vec<(ElementId, String, KindTag)> = flow
        .elements
        .iter()
        .filter(|e| e.id != id)
        .map(|e| (e.id, e.name.clone(), e.kind.tag()))
        .collect();
    let e = flow.get_mut(id).unwrap();
    let tag = e.kind.tag();
    egui::Grid::new("flow_element")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.weak("種類");
            ui.label(tag.label());
            ui.end_row();
            ui.weak("名前");
            ui.text_edit_singleline(&mut e.name);
            ui.end_row();

            match &mut e.kind {
                ElementKind::Source {
                    interval,
                    spread,
                    limit,
                } => {
                    ui.weak("間隔");
                    seconds(ui, interval);
                    ui.end_row();
                    ui.weak("ばらつき ±");
                    percent(ui, spread);
                    ui.end_row();
                    ui.weak("個数の上限");
                    ui.horizontal(|ui| {
                        let mut limited = limit.is_some();
                        if ui.checkbox(&mut limited, "").changed() {
                            *limit = limited.then_some(100);
                        }
                        if let Some(l) = limit {
                            ui.add(egui::DragValue::new(l).range(1..=10_000_000).suffix(" 個"));
                        } else {
                            ui.weak("なし");
                        }
                    });
                    ui.end_row();
                }
                ElementKind::Process {
                    cycle_time,
                    spread,
                    failure,
                } => {
                    ui.weak("サイクルタイム");
                    seconds(ui, cycle_time);
                    ui.end_row();
                    ui.weak("ばらつき ±");
                    percent(ui, spread);
                    ui.end_row();
                    ui.weak("故障");
                    let mut enabled = failure.is_some();
                    if ui
                        .checkbox(&mut enabled, "乱数で故障させる")
                        .on_hover_text("加工している時間で数え、MTBF・MTTR を平均とする指数分布で故障・修理する")
                        .changed()
                    {
                        *failure = enabled.then_some(Failure {
                            mtbf: 3600.0,
                            mttr: 300.0,
                        });
                    }
                    ui.end_row();
                    if let Some(f) = failure {
                        ui.weak("MTBF (平均故障間隔)");
                        minutes(ui, &mut f.mtbf);
                        ui.end_row();
                        ui.weak("MTTR (平均修理時間)");
                        minutes(ui, &mut f.mttr);
                        ui.end_row();
                    }
                    ui.weak("作業者");
                    labor_editor(ui, &mut e.labor);
                    ui.end_row();
                }
                ElementKind::Buffer { capacity } => {
                    ui.weak("容量");
                    ui.add(egui::DragValue::new(capacity).range(1..=100_000).suffix(" 個"));
                    ui.end_row();
                }
                ElementKind::Sink => {}
            }

            if tag != KindTag::Sink {
                ui.weak("送り先");
                ui.vertical(|ui| {
                    for (other, name, other_tag) in &others {
                        if *other_tag == KindTag::Source {
                            continue;
                        }
                        let mut linked = e.next.contains(other);
                        if ui.checkbox(&mut linked, name).changed() {
                            if linked {
                                e.next.push(*other);
                            } else {
                                e.next.retain(|n| n != other);
                            }
                        }
                    }
                    if e.next.len() > 1 {
                        ui.weak("複数あるときは、上から順に空いている先へ送ります");
                    }
                });
                ui.end_row();
            }

            ui.weak("3D の部品");
            ui.horizontal(|ui| {
                match e.part.and_then(|p| model.get(p)) {
                    Some(node) => {
                        ui.label(&node.name);
                        if ui.small_button("✕").on_hover_text("対応付けを外す").clicked() {
                            e.part = None;
                        }
                    }
                    None => {
                        ui.weak("なし");
                    }
                }
                let picked = selection.node.filter(|&n| model.get(n).is_some());
                if ui
                    .add_enabled(picked.is_some(), egui::Button::new("選択中の部品にする"))
                    .on_hover_text("3D ビューやツリーで選んだ部品・アセンブリを、この要素の色分けの対象にする")
                    .clicked()
                {
                    e.part = picked;
                }
            });
            ui.end_row();

            ui.weak("定格電力");
            ui.add(
                egui::DragValue::new(&mut e.energy.rated_kw)
                    .speed(0.05)
                    .range(0.0..=10_000.0)
                    .suffix(" kW"),
            )
            .on_hover_text("稼働中の電力");
            ui.end_row();
            ui.weak("待機電力");
            ui.add(
                egui::DragValue::new(&mut e.energy.standby_kw)
                    .speed(0.05)
                    .range(0.0..=10_000.0)
                    .suffix(" kW"),
            )
            .on_hover_text("待ち・詰まり・故障中の電力");
            ui.end_row();
            ui.weak("エア消費量");
            ui.add(
                egui::DragValue::new(&mut e.energy.air_l_per_min)
                    .speed(1.0)
                    .range(0.0..=1_000_000.0)
                    .suffix(" L/min"),
            )
            .on_hover_text("稼働中のエア消費量 (ANR)");
            ui.end_row();
        });

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("↑").on_hover_text("一覧で上へ").clicked() {
            flow.move_element(id, true);
        }
        if ui.button("↓").on_hover_text("一覧で下へ").clicked() {
            flow.move_element(id, false);
        }
        if ui.button("🗑 削除").clicked() {
            flow.remove(id);
            ui_state.selected = None;
        }
    });
}

/// 工程に人が要るか。「人員」タブの名簿のうち、この工程ができる人が来て作業する。
fn labor_editor(ui: &mut egui::Ui, labor: &mut Labor) {
    ui.vertical(|ui| {
        let options = [Labor::None, Labor::Full, Labor::Load { seconds: 10.0 }];
        egui::ComboBox::from_id_salt("flow_labor")
            .selected_text(labor.label())
            .show_ui(ui, |ui| {
                for option in options {
                    let selected = std::mem::discriminant(labor) == std::mem::discriminant(&option);
                    if ui.selectable_label(selected, option.label()).clicked() && !selected {
                        *labor = option;
                    }
                }
            })
            .response
            .on_hover_text(
                "必要にすると、「人員」タブの名簿でこの工程ができる人が来るまで「人待ち」で止まる",
            );
        if let Labor::Load { seconds } = labor {
            ui.horizontal(|ui| {
                ui.label("人が付く時間");
                ui.add(
                    egui::DragValue::new(seconds)
                        .speed(0.1)
                        .range(0.0..=1_000_000.0)
                        .max_decimals(2)
                        .suffix(" 秒"),
                )
                .on_hover_text("1 個ごとの最初のこの時間だけ人が付き、残りは機械が自動で加工する");
            });
        }
    });
}

fn settings_editor(ui: &mut egui::Ui, flow: &mut FlowModel) {
    let s = &mut flow.settings;
    ui.horizontal_wrapped(|ui| {
        ui.label("回す時間");
        let mut h = s.duration / 3600.0;
        if ui
            .add(
                egui::DragValue::new(&mut h)
                    .speed(0.1)
                    .range(0.01..=1000.0)
                    .suffix(" 時間"),
            )
            .changed()
        {
            s.duration = h * 3600.0;
        }
        ui.label("立ち上がり (集計しない)");
        let mut w = s.warmup / 3600.0;
        if ui
            .add(
                egui::DragValue::new(&mut w)
                    .speed(0.05)
                    .range(0.0..=1000.0)
                    .suffix(" 時間"),
            )
            .changed()
        {
            s.warmup = w * 3600.0;
        }
        ui.label("乱数の種");
        ui.add(egui::DragValue::new(&mut s.seed));
    });
}

fn seconds(ui: &mut egui::Ui, value: &mut f64) {
    ui.add(
        egui::DragValue::new(value)
            .speed(0.1)
            .range(super::model::MIN_TIME..=1_000_000.0)
            .max_decimals(2)
            .suffix(" 秒"),
    );
}

fn minutes(ui: &mut egui::Ui, seconds: &mut f64) {
    let mut m = *seconds / 60.0;
    if ui
        .add(
            egui::DragValue::new(&mut m)
                .speed(0.5)
                .range(0.01..=1_000_000.0)
                .max_decimals(2)
                .suffix(" 分"),
        )
        .changed()
    {
        *seconds = m * 60.0;
    }
}

fn percent(ui: &mut egui::Ui, ratio: &mut f64) {
    let mut p = *ratio * 100.0;
    if ui
        .add(
            egui::DragValue::new(&mut p)
                .speed(0.5)
                .range(0.0..=100.0)
                .suffix(" %"),
        )
        .changed()
    {
        *ratio = p / 100.0;
    }
}

// ---------------------------------------------------------------------------
// 結果タブ
// ---------------------------------------------------------------------------

fn results_tab(ui: &mut egui::Ui, state: &mut FlowUi) {
    let Some(report) = &state.run.report else {
        ui.label("まだ結果がありません。▶ 実行 か ⏩ 最後まで を押してください。");
        return;
    };
    egui::ScrollArea::both()
        .id_salt("flow_results")
        .show(ui, |ui| {
            summary(ui, report);
            ui.add_space(8.0);
            element_table(ui, report);
            if !report.workers.is_empty() {
                ui.add_space(8.0);
                worker_table(ui, report);
            }
        });
}

fn summary(ui: &mut egui::Ui, r: &Report) {
    let opt = |v: Option<f64>, unit: &str| v.map_or("-".into(), |v| format!("{v:.2} {unit}"));
    let bottleneck = r
        .bottleneck
        .and_then(|id| r.element(id))
        .map_or("-".to_owned(), |e| {
            format!("{} (稼働+故障 {:.1}%)", e.name, e.busy_ratio() * 100.0)
        });
    egui::Grid::new("flow_summary")
        .num_columns(4)
        .spacing([16.0, 4.0])
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, a: &str, b: String, c: &str, d: String| {
                ui.weak(a);
                ui.strong(b);
                ui.weak(c);
                ui.strong(d);
                ui.end_row();
            };
            row(
                ui,
                "排出数",
                format!("{} 個", r.output),
                "スループット",
                format!("{:.1} 個/時", r.throughput_per_hour),
            );
            row(
                ui,
                "サイクルタイム",
                opt(r.cycle_time, "秒/個"),
                "リードタイム",
                opt(r.lead_time, "秒"),
            );
            row(
                ui,
                "仕掛数 (平均)",
                format!("{:.2} 個", r.wip_average),
                "仕掛数 (最大)",
                format!("{} 個", r.wip_max),
            );
            row(
                ui,
                "電力量",
                format!("{:.2} kWh", r.energy_kwh),
                "1個あたり",
                opt(r.energy_per_work_kwh, "kWh/個"),
            );
            row(
                ui,
                "エア消費量",
                format!("{:.2} m³", r.air_m3),
                "集計した時間",
                hours(r.measured),
            );
            ui.weak("ボトルネック");
            ui.colored_label(color(busy_rgb(1.0, true)), bottleneck);
            ui.end_row();
        });
}

fn element_table(ui: &mut egui::Ui, r: &Report) {
    egui::Grid::new("flow_elements")
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "要素",
                "種類",
                "稼働",
                "待ち",
                "詰まり",
                "故障",
                "人待ち",
                "通過数",
                "故障回数",
                "平均加工",
                "平均の中身",
                "電力量",
                "エア",
            ] {
                ui.strong(title);
            }
            ui.end_row();
            for e in &r.elements {
                let is_bottleneck = r.bottleneck == Some(e.id);
                if is_bottleneck {
                    ui.colored_label(color(busy_rgb(1.0, true)), format!("★ {}", e.name));
                } else {
                    ui.label(&e.name);
                }
                ui.weak(e.kind.label());
                let shows_states = matches!(e.kind, KindTag::Source | KindTag::Process);
                for state in State::ALL {
                    if shows_states {
                        ui.label(format!("{:.1}%", e.times.ratio(state) * 100.0));
                    } else {
                        ui.weak("-");
                    }
                }
                ui.label(e.count.to_string());
                ui.label(if e.kind == KindTag::Process {
                    e.failures.to_string()
                } else {
                    "-".into()
                });
                ui.label(e.average_cycle.map_or("-".into(), |c| format!("{c:.1} 秒")));
                ui.label(if matches!(e.kind, KindTag::Process | KindTag::Buffer) {
                    format!("{:.2} 個", e.content_average)
                } else {
                    "-".into()
                });
                ui.label(format!("{:.2} kWh", e.energy_kwh));
                ui.label(format!("{:.2} m³", e.air_m3));
                ui.end_row();
            }
        });
}

fn worker_table(ui: &mut egui::Ui, r: &Report) {
    ui.strong("作業者");
    egui::Grid::new("flow_workers_result")
        .striped(true)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for title in [
                "作業者",
                "稼働率",
                "作業",
                "歩行",
                "手待ち",
                "出勤時間",
                "作業数",
                "歩いた回数",
            ] {
                ui.strong(title);
            }
            ui.end_row();
            for w in &r.workers {
                ui.label(&w.name);
                ui.strong(format!("{:.0}%", w.utilization() * 100.0))
                    .on_hover_text("出勤している時間のうち、作業と歩行の割合");
                for state in [
                    WorkerState::Working,
                    WorkerState::Walking,
                    WorkerState::Idle,
                ] {
                    ui.label(format!("{:.1}%", w.times.ratio(state) * 100.0));
                }
                ui.label(hours(w.times.on_duty()));
                ui.label(w.tasks.to_string());
                ui.label(w.walks.to_string());
                ui.end_row();
            }
        });
}

// ---------------------------------------------------------------------------
// グラフタブ
// ---------------------------------------------------------------------------

fn charts_tab(ui: &mut egui::Ui, state: &mut FlowUi) {
    let Some(report) = &state.run.report else {
        ui.label("まだ結果がありません。▶ 実行 か ⏩ 最後まで を押してください。");
        return;
    };
    egui::ScrollArea::vertical()
        .id_salt("flow_charts")
        .show(ui, |ui| {
            legend(ui);
            ui.strong("設備ごとの状態の割合");
            state_bars(ui, report);
            ui.add_space(10.0);
            ui.strong("状態の移り変わり");
            state_timeline(ui, report);
            ui.add_space(10.0);
            ui.strong("排出数 (累計) と仕掛数の推移");
            series_chart(ui, report);
        });
}

fn color(rgb: [f32; 3]) -> Color32 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(c(rgb[0]), c(rgb[1]), c(rgb[2]))
}

/// 工程タブの一覧と設定欄の高さ
const LIST_HEIGHT: f32 = 300.0;
const NAME_WIDTH: f32 = 110.0;
const ROW_HEIGHT: f32 = 18.0;

fn legend(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        for state in State::ALL {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 2.0, color(state_rgb(state)));
            ui.label(state.label());
            ui.add_space(6.0);
        }
    });
}

/// 状態を持つ要素 (生成と工程)
fn charted(report: &Report) -> impl Iterator<Item = (usize, &super::sim::ElementStats)> {
    report
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, KindTag::Source | KindTag::Process))
}

fn state_bars(ui: &mut egui::Ui, report: &Report) {
    let rows: Vec<_> = charted(report).collect();
    let width = ui.available_width().max(200.0);
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, rows.len() as f32 * (ROW_HEIGHT + 4.0)),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let text = ui.visuals().text_color();
    let bar_width = rect.width() - NAME_WIDTH - 50.0;
    let mut hovered = None;
    for (row, (_, e)) in rows.iter().enumerate() {
        let y = rect.top() + row as f32 * (ROW_HEIGHT + 4.0);
        let name = if report.bottleneck == Some(e.id) {
            format!("★ {}", e.name)
        } else {
            e.name.clone()
        };
        painter.text(
            Pos2::new(rect.left(), y + ROW_HEIGHT / 2.0),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(12.0),
            text,
        );
        let mut x = rect.left() + NAME_WIDTH;
        for state in State::ALL {
            let w = e.times.ratio(state) as f32 * bar_width;
            if w > 0.0 {
                let bar = ERect::from_min_size(Pos2::new(x, y), Vec2::new(w, ROW_HEIGHT));
                painter.rect_filled(bar, 0.0, color(state_rgb(state)));
                if response.hover_pos().is_some_and(|p| bar.contains(p)) {
                    hovered = Some(format!(
                        "{} の{}: {:.1}%",
                        e.name,
                        state.label(),
                        e.times.ratio(state) * 100.0
                    ));
                }
            }
            x += w;
        }
        painter.text(
            Pos2::new(
                rect.left() + NAME_WIDTH + bar_width + 4.0,
                y + ROW_HEIGHT / 2.0,
            ),
            egui::Align2::LEFT_CENTER,
            format!("{:.0}%", e.times.ratio(State::Working) * 100.0),
            egui::FontId::proportional(11.0),
            text,
        );
    }
    if let Some(text) = hovered {
        response.on_hover_text(text);
    }
}

fn state_timeline(ui: &mut egui::Ui, report: &Report) {
    let rows: Vec<_> = charted(report).collect();
    let samples = &report.samples;
    if samples.len() < 2 {
        ui.weak("回し始めたばかりです");
        return;
    }
    let width = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(width, rows.len() as f32 * (ROW_HEIGHT + 4.0) + 16.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter_at(rect);
    let text = ui.visuals().text_color();
    let left = rect.left() + NAME_WIDTH;
    let span = rect.right() - 50.0 - left;
    let end = samples.last().unwrap().time.max(1e-9);
    for (row, (index, e)) in rows.iter().enumerate() {
        let y = rect.top() + row as f32 * (ROW_HEIGHT + 4.0);
        painter.text(
            Pos2::new(rect.left(), y + ROW_HEIGHT / 2.0),
            egui::Align2::LEFT_CENTER,
            &e.name,
            egui::FontId::proportional(12.0),
            text,
        );
        for pair in samples.windows(2) {
            let x0 = left + (pair[0].time / end) as f32 * span;
            let x1 = left + (pair[1].time / end) as f32 * span;
            let state = pair[0].states[*index];
            painter.rect_filled(
                ERect::from_min_max(
                    Pos2::new(x0, y),
                    Pos2::new(x1.max(x0 + 0.5), y + ROW_HEIGHT),
                ),
                0.0,
                color(state_rgb(state)),
            );
        }
    }
    axis_labels(&painter, rect, left, span, end, text);
}

fn axis_labels(
    painter: &egui::Painter,
    rect: ERect,
    left: f32,
    span: f32,
    end: f64,
    text: Color32,
) {
    for k in 0..=4 {
        let t = end * k as f64 / 4.0;
        painter.text(
            Pos2::new(left + span * k as f32 / 4.0, rect.bottom() - 2.0),
            egui::Align2::CENTER_BOTTOM,
            format!("{:.1}h", t / 3600.0),
            egui::FontId::proportional(10.0),
            text,
        );
    }
}

fn series_chart(ui: &mut egui::Ui, report: &Report) {
    let samples = &report.samples;
    if samples.len() < 2 {
        ui.weak("回し始めたばかりです");
        return;
    }
    let width = ui.available_width().max(200.0);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 180.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let text = ui.visuals().text_color();
    let plot = ERect::from_min_max(
        Pos2::new(rect.left() + 50.0, rect.top() + 6.0),
        Pos2::new(rect.right() - 50.0, rect.bottom() - 16.0),
    );
    painter.rect_stroke(
        plot,
        0.0,
        Stroke::new(1.0, ui.visuals().weak_text_color()),
        egui::StrokeKind::Inside,
    );
    let end = samples.last().unwrap().time.max(1e-9);
    let max_output = samples.iter().map(|s| s.output).max().unwrap_or(0).max(1) as f32;
    let max_wip = samples.iter().map(|s| s.wip).max().unwrap_or(0).max(1) as f32;
    let output_color = Color32::from_rgb(90, 150, 240);
    let wip_color = Color32::from_rgb(240, 140, 60);
    let x = |t: f64| plot.left() + (t / end) as f32 * plot.width();
    let output: Vec<Pos2> = samples
        .iter()
        .map(|s| {
            Pos2::new(
                x(s.time),
                plot.bottom() - s.output as f32 / max_output * plot.height(),
            )
        })
        .collect();
    let wip: Vec<Pos2> = samples
        .iter()
        .map(|s| {
            Pos2::new(
                x(s.time),
                plot.bottom() - s.wip as f32 / max_wip * plot.height(),
            )
        })
        .collect();
    painter.add(egui::Shape::line(output, Stroke::new(2.0, output_color)));
    painter.add(egui::Shape::line(wip, Stroke::new(1.5, wip_color)));
    let font = egui::FontId::proportional(10.0);
    painter.text(
        plot.left_top(),
        egui::Align2::RIGHT_TOP,
        format!("{max_output:.0}個 "),
        font.clone(),
        output_color,
    );
    painter.text(
        plot.left_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        "0 ",
        font.clone(),
        output_color,
    );
    painter.text(
        plot.right_top(),
        egui::Align2::LEFT_TOP,
        format!(" {max_wip:.0}個"),
        font.clone(),
        wip_color,
    );
    painter.text(
        plot.right_bottom(),
        egui::Align2::LEFT_BOTTOM,
        " 0",
        font.clone(),
        wip_color,
    );
    axis_labels(&painter, rect, plot.left(), plot.width(), end, text);
    if let Some(p) = response.hover_pos().filter(|p| plot.contains(*p)) {
        let t = ((p.x - plot.left()) / plot.width()) as f64 * end;
        if let Some(s) = samples
            .iter()
            .min_by(|a, b| (a.time - t).abs().total_cmp(&(b.time - t).abs()))
        {
            painter.vline(x(s.time), plot.y_range(), Stroke::new(1.0, text));
            response.on_hover_text(format!(
                "{}: 排出 {} 個 / 仕掛 {} 個",
                hours(s.time),
                s.output,
                s.wip
            ));
        }
    }
    ui.horizontal(|ui| {
        ui.colored_label(output_color, "━ 排出数 (左の目盛)");
        ui.colored_label(wip_color, "━ 仕掛数 (右の目盛)");
    });
}
