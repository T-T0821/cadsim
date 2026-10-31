//! ライン連携: 工程シミュレーション・AGV・作業者・タイミングチャート・安全機器を
//! 1つの時計で一緒に動かす
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `stepper.rs` | 工程と AGV を少しずつ進めて出来事を受け渡す (Bevy 非依存) |
//! | `view.rs` | 3D のワーク (設備の上に積む・設備から設備へ渡る・AGV に積む) |
//! | `panel.rs` | 「ライン連携」ウィンドウ (再生・つなぎ方・PLC の信号表) |
//!
//! 判断と計算は `cadsim_core::line` にある。このファイルは Bevy とのつなぎで、
//! 毎フレーム「止める条件を見る → 進める → 結果をタイミングチャートや表示に写す」だけをする。
//! ライン連携で再生している間は、工程シミュレーションと作業者・AGV のそれぞれの再生ボタンは使わない
//! (同じ時計で進めるため、それぞれの再生は止めておく)。

mod panel;
mod stepper;
mod view;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use cadsim_core::flow::model::FlowModel;
use cadsim_core::line::{LineCoupler, LineInputs, Signal, SignalSources, line_signals, may_run};

use crate::floor::agv_sim::AgvSim;
use crate::floor::{AgvLayoutRes, FloorClock, FloorState};
use crate::flow::{FlowRes, FlowRun};
use crate::motion::{MotionSet, Playback, TimingChart};
use crate::plc::Plc;
use crate::safety::{SafetySet, SafetyState};

pub struct LinePlugin;

impl Plugin for LinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LineState>()
            .add_systems(
                Update,
                (
                    advance_line.before(SafetySet::Intruders),
                    exchange_plc.before(MotionSet::Drive),
                    view::draw.after(advance_line),
                ),
            )
            .add_systems(EguiPrimaryContextPass, panel::line_window);
    }
}

/// ライン連携の再生の状態
#[derive(Resource)]
pub struct LineState {
    /// ウィンドウを出しているか (メニューの「ライン」から開く)
    pub open: bool,
    /// 最初から回し始めたか (一時停止中も true)
    started: bool,
    playing: bool,
    /// 実時間 1 秒あたりに進める秒数
    speed: f64,
    /// 回し始めてからの時刻 (秒)
    time: f64,
    /// 回し始めたときに工程モデルがあったか (なければ AGV と作業者だけ動かす)
    with_flow: bool,
    coupler: LineCoupler,
    /// PLC → 設備 の信号 (運転・投入許可)。PLC をつなぐまでは画面で切り替える。
    inputs: LineInputs,
    /// タイミングチャートもラインと一緒に再生する
    chart_follows: bool,
    /// 安全停止か運転 OFF で止まっている
    halted: bool,
    /// PLC (外部の PLC かログ再生) が運転・投入許可を書いている
    plc_drives_inputs: bool,
    /// 止めた理由など
    message: Option<String>,
}

impl Default for LineState {
    fn default() -> Self {
        Self {
            open: false,
            started: false,
            playing: false,
            speed: 10.0,
            time: 0.0,
            with_flow: false,
            coupler: LineCoupler::new(),
            inputs: LineInputs::default(),
            chart_follows: true,
            halted: false,
            plc_drives_inputs: false,
            message: None,
        }
    }
}

impl LineState {
    pub fn toggle(&mut self) {
        self.open = !self.open;
    }

    /// 最初から回し始める。工程モデルが回せなければ理由を返す。
    fn start(
        &mut self,
        flow: &FlowModel,
        run: &mut FlowRun,
        floor: &mut FloorState,
        layout: &AgvLayoutRes,
        clock: &mut FloorClock,
    ) -> Result<(), Vec<String>> {
        if flow.is_empty() {
            run.stop();
        } else {
            run.restart(flow)?;
            if let Some(sim) = run.sim.as_mut() {
                sim.link(LineCoupler::carried_elements(flow));
            }
        }
        floor.restart_agv(layout);
        clock.time = 0.0;
        clock.playing = false;
        self.with_flow = !flow.is_empty();
        self.coupler = LineCoupler::new();
        self.time = 0.0;
        self.started = true;
        self.playing = true;
        self.message = None;
        Ok(())
    }

    /// 止めて最初に戻す (結果は工程シミュレーションのウィンドウに残る)
    fn stop(&mut self, reason: Option<String>) {
        self.started = false;
        self.playing = false;
        self.halted = false;
        self.time = 0.0;
        self.coupler = LineCoupler::new();
        self.message = reason;
    }
}

/// 毎フレーム: 止める条件を見て、工程・AGV・作業者・チャートを同じだけ進める
#[allow(clippy::too_many_arguments)]
fn advance_line(
    time: Res<Time>,
    mut line: ResMut<LineState>,
    flow: Res<FlowRes>,
    mut run: ResMut<FlowRun>,
    mut floor: ResMut<FloorState>,
    mut clock: ResMut<FloorClock>,
    layout: Res<AgvLayoutRes>,
    safety: Res<SafetyState>,
    mut playback: ResMut<Playback>,
) {
    if !line.started {
        return;
    }
    // 途中で設定が変わったら、その先は意味がないので止める
    if layout.is_changed() {
        line.stop(Some("AGV の経路・台数が変わったので止めました".into()));
        return;
    }
    if line.with_flow {
        if run.is_stale(&flow) {
            line.stop(Some(
                "工程モデルが変わったので止めました。もう一度再生すると最初から回します".into(),
            ));
            return;
        }
        if !run.sim.as_ref().is_some_and(|s| s.is_linked()) {
            line.stop(Some(
                "工程シミュレーションのウィンドウで回し直したので止めました".into(),
            ));
            return;
        }
    }

    // それぞれの再生ボタンでは進めない (同じ時計で進める)
    if run.playing {
        run.playing = false;
    }
    if clock.playing {
        clock.playing = false;
    }

    let halted = !may_run(safety.is_stopped(), &line.inputs);
    if line.halted != halted {
        line.halted = halted;
    }
    if !line.playing || halted {
        if line.chart_follows && playback.playing {
            playback.playing = false;
        }
        return;
    }

    let dt = time.delta_secs_f64() * line.speed;
    let line = &mut *line;
    line.time += dt;
    clock.time += dt;
    let run = &mut *run;
    if let Some(sim) = run.sim.as_mut() {
        for e in &flow.elements {
            let enabled = line.inputs.feed_enabled(e.id);
            if sim.source_enabled(e.id) != enabled {
                sim.set_source_enabled(e.id, enabled);
            }
        }
    }
    let result = stepper::step(
        &flow,
        run.sim.as_mut(),
        floor.agv_sim_mut(),
        &mut line.coupler,
        dt,
    );
    if let Some(sim) = &run.sim {
        run.report = Some(sim.report());
    }

    // 工程の加工開始で頭から再生する設定があれば、それに任せる。なければラインと一緒に流す。
    let triggered = flow.elements.iter().any(|e| e.starts_chart);
    if result.restart_chart {
        playback.time = 0.0;
        playback.playing = true;
    } else if line.chart_follows && !triggered && !playback.playing {
        playback.playing = true;
    }
    if line.chart_follows && playback.speed != line.speed {
        playback.speed = line.speed;
    }
}

/// 今の信号表 (工程・AGV・安全の状態と、運転・投入許可)
fn current_signals(
    line: &LineState,
    flow: &FlowModel,
    run: &FlowRun,
    floor: &FloorState,
    safety: &SafetyState,
) -> Vec<Signal> {
    let sim = run.sim.as_ref().filter(|s| s.is_linked());
    let agv_loaded = floor
        .agv_sim()
        .map(|a| {
            a.vehicles
                .iter()
                .map(|v| {
                    AgvSim::is_loaded(v)
                        && v.job
                            .as_ref()
                            .and_then(|j| j.tag)
                            .is_some_and(|t| line.coupler.is_loaded(t))
                })
                .collect()
        })
        .unwrap_or_default();
    let sources = SignalSources {
        running: line.playing && !line.halted,
        safety_stopped: safety.is_stopped(),
        agv_loaded,
        agv_waiting: line.coupler.waiting_for_agv(),
    };
    line_signals(flow, sim, &sources, &line.inputs)
}

/// 信号表を PLC 連携の I/O に載せ、PLC が書いた運転・投入許可をラインに写す
fn exchange_plc(
    mut line: ResMut<LineState>,
    flow: Res<FlowRes>,
    run: Res<FlowRun>,
    floor: Res<FloorState>,
    safety: Res<SafetyState>,
    chart: Res<TimingChart>,
    mut plc: ResMut<Plc>,
) {
    let signals = current_signals(&line, &flow, &run, &floor, &safety);
    let (changed, outputs) = plc
        .bypass_change_detection()
        .exchange_line(&signals, &chart);
    if changed {
        plc.set_changed();
    }
    // 表示と判断のための値なので、変更を知らせない
    let line = line.bypass_change_detection();
    line.plc_drives_inputs = outputs.is_some();
    for (name, on) in outputs.unwrap_or_default() {
        line.inputs.set(&flow, &name, on);
    }
}
