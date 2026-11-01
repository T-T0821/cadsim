//! ライン連携: 工程シミュレーション・AGV・作業者・タイミングチャート・安全機器を
//! 1つの時計で一緒に動かす
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `view.rs` | 3D のワーク (設備の上に積む・設備から設備へ渡る・AGV に積む) |
//! | `panel.rs` | 「ライン連携」ウィンドウ (再生・つなぎ方・PLC の信号表) |
//!
//! 判断と計算は `cadsim_core::line` にある (工程・AGV・コンベアを少しずつ進めて出来事を受け渡すのは `stepper`)。このファイルは Bevy とのつなぎで、
//! 毎フレーム「止める条件を見る → 進める → 結果をタイミングチャートや表示に写す」だけをする。
//! ライン連携で再生している間は、工程シミュレーションと作業者・AGV のそれぞれの再生ボタンは使わない
//! (同じ時計で進めるため、それぞれの再生は止めておく)。

mod panel;
mod view;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use cadsim_core::flow::model::FlowModel;
use cadsim_core::line::{
    ConveyorCounts, LineCoupler, LineInputs, LineParts, LineSnapshot, LineTimeline, Signal,
    SignalSources, line_signals, line_timeline, may_run, step,
};

use crate::conveyor::{ConveyorConfigRes, ConveyorSet, TimeMap, WorkState};
use crate::model::Model;

use crate::floor::agv_sim::AgvSim;
use crate::floor::{AgvLayoutRes, FloorClock, FloorState};
use crate::flow::{FlowRes, FlowRun};
use crate::motion::{ChartSchedule, MotionSet, Playback, TimingChart};
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
                    read_conveyor.after(ConveyorSet::Works),
                ),
            )
            .add_systems(EguiPrimaryContextPass, panel::line_window);
    }
}

/// ライン連携の再生の状態
#[derive(Resource)]
pub struct LineState {
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
    /// コンベアのワークの数 (ワーク搬送の投入口がなければ None)。信号表と集計に使う。
    conveyor: Option<ConveyorCounts>,
    /// 回し始めたばかりで、ワーク搬送をラインの時計につなぐ前
    conveyor_link_pending: bool,
    /// 止めた理由など
    message: Option<String>,
    /// 途中の状態の写し (時間スライダーで戻すため)
    timeline: LineTimeline,
    /// 時間スライダーで戻してほしい時刻 (次のフレームで戻す)
    seek_to: Option<f64>,
}

impl Default for LineState {
    fn default() -> Self {
        Self {
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
            conveyor: None,
            conveyor_link_pending: false,
            message: None,
            timeline: line_timeline(),
            seek_to: None,
        }
    }
}

impl LineState {
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
        self.conveyor_link_pending = true;
        self.message = None;
        self.timeline = line_timeline();
        self.seek_to = None;
        Ok(())
    }

    /// ラインが動いている (回し始めていて、一時停止・安全停止・運転 OFF で止まっていない)
    pub fn is_running(&self) -> bool {
        self.started && self.playing && !self.halted
    }

    /// 止めて最初に戻す (結果は工程シミュレーションのウィンドウに残る)
    fn stop(&mut self, reason: Option<String>) {
        self.started = false;
        self.playing = false;
        self.halted = false;
        self.time = 0.0;
        self.coupler = LineCoupler::new();
        self.message = reason;
        self.timeline = line_timeline();
        self.seek_to = None;
    }

    /// 今のライン一式の状態を、前の写しから間隔がたっていれば取っておく
    fn record(
        &mut self,
        run: &FlowRun,
        floor: &FloorState,
        clock: &FloorClock,
        works: &WorkState,
        playback: &Playback,
    ) {
        if self.timeline.next_due().is_some_and(|due| self.time < due) {
            return;
        }
        let snapshot = LineSnapshot {
            coupler: self.coupler.clone(),
            flow: run.sim.clone(),
            agv: floor.agv_sim().cloned(),
            conveyor: works.line_sim().cloned(),
            floor_time: clock.time,
            chart_time: playback.time,
        };
        self.timeline.record(self.time, &snapshot);
    }

    /// 時刻 `time` かそれより前の、いちばん近い写しに戻す
    fn restore(
        &mut self,
        time: f64,
        run: &mut FlowRun,
        floor: &mut FloorState,
        clock: &mut FloorClock,
        works: &mut WorkState,
        playback: &mut Playback,
    ) {
        let Some((at, snap)) = self.timeline.at_or_before(time) else {
            return;
        };
        self.time = *at;
        self.coupler = snap.coupler.clone();
        if let Some(flow) = &snap.flow {
            run.report = Some(flow.report());
            run.sim = Some(flow.clone());
        }
        if let (Some(agv), Some(now)) = (&snap.agv, floor.agv_sim_mut()) {
            *now = agv.clone();
        }
        if let Some(conveyor) = &snap.conveyor {
            works.restore_line_sim(conveyor);
        }
        clock.time = snap.floor_time;
        playback.time = snap.chart_time;
    }

    /// 記録してある時刻の範囲 (時間スライダーの幅)
    fn recorded_span(&self) -> Option<(f64, f64)> {
        self.timeline.span()
    }

    /// 詰まりを探す推移の記録 (いちばん先まで回したときの工程シミュレーション)
    fn furthest_flow(&self) -> Option<&cadsim_core::flow::sim::Simulation> {
        self.timeline.last().and_then(|(_, s)| s.flow.as_ref())
    }
}

/// ライン連携で一緒に進めるワーク搬送とその入力
#[derive(SystemParam)]
struct ConveyorParams<'w> {
    config: Res<'w, ConveyorConfigRes>,
    works: ResMut<'w, WorkState>,
    model: Res<'w, Model>,
    chart: Res<'w, TimingChart>,
    schedule: Res<'w, ChartSchedule>,
}

impl ConveyorParams<'_> {
    /// ワーク搬送を時刻 0 からラインの時計で進め始める。工程から送り込む投入口を決める。
    fn start(&mut self, flow: &FlowModel) {
        if self.config.is_empty() {
            return;
        }
        let driven = flow
            .elements
            .iter()
            .filter(|e| e.carrier.is_none())
            .filter_map(|e| e.conveyor);
        let (config, model, chart) = (&self.config.0, &*self.model, &*self.chart);
        self.works.start_line(config, model, chart, driven);
    }

    fn stop(&mut self) {
        if self.works.is_line_driven() {
            self.works.stop_line();
        }
    }
}

/// 毎フレーム: 止める条件を見て、工程・AGV・作業者・コンベア・チャートを同じだけ進める
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
    mut conveyor: ConveyorParams,
) {
    if !line.started {
        conveyor.stop();
        return;
    }
    if line.conveyor_link_pending {
        line.conveyor_link_pending = false;
        conveyor.start(&flow);
    } else if conveyor.config.is_changed() && conveyor.works.is_line_driven() {
        line.stop(Some(
            "ワーク搬送の設定が変わったので止めました。もう一度再生すると最初から回します".into(),
        ));
        conveyor.stop();
        return;
    }
    // 途中で設定が変わったら、その先は意味がないので止める
    if layout.is_changed() {
        line.stop(Some("AGV の経路・台数が変わったので止めました".into()));
        conveyor.stop();
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

    if let Some(t) = line.seek_to.take() {
        line.playing = false;
        let line = &mut *line;
        line.restore(
            t,
            &mut run,
            &mut floor,
            &mut clock,
            &mut conveyor.works,
            &mut playback,
        );
        if line.chart_follows {
            playback.playing = false;
        }
        return;
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
    // 巻き戻した時点から進め直すと PLC の信号などで別の進み方になりうるので、先の記録は捨てる
    line.timeline.truncate_after(line.time);
    line.record(&run, &floor, &clock, &conveyor.works, &playback);
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
    // コンベアのグリッパ・台の動きは、このフレームのチャートの再生位置から同じ速さで進むとみなす
    let map = TimeMap {
        from: conveyor.works.line_time(),
        base: playback.time,
        rate: if playback.playing {
            playback.speed / line.speed.max(1e-9)
        } else {
            0.0
        },
    };
    let ConveyorParams {
        config,
        works,
        chart,
        schedule,
        ..
    } = &mut conveyor;
    let looping = playback.looping;
    let linked = works.with_line(config, chart, schedule, map, looping, |link| {
        let parts = LineParts {
            flow: run.sim.as_mut(),
            agv: floor.agv_sim_mut(),
            conveyor: Some(link),
        };
        step(&flow, parts, &mut line.coupler, dt)
    });
    let result = linked.unwrap_or_else(|| {
        let parts = LineParts {
            flow: run.sim.as_mut(),
            agv: floor.agv_sim_mut(),
            conveyor: None,
        };
        step(&flow, parts, &mut line.coupler, dt)
    });
    if let Some(sim) = &run.sim {
        run.report = Some(sim.report());
    }
    line.record(run, &floor, &clock, &conveyor.works, &playback);

    // 工程の加工開始で頭から再生する設定があれば、それに任せる。なければラインと一緒に流す。
    // 工程からコンベアへ送り込んでいるときは、グリッパがチャートで動かないとワークが進まず
    // 加工も始まらないので、「チャート起動」があっても流し続ける (加工開始で頭に戻すのは同じ)。
    let triggered = flow.elements.iter().any(|e| e.starts_chart);
    let feeds_conveyor = conveyor.works.is_line_driven()
        && flow
            .elements
            .iter()
            .any(|e| e.carrier.is_none() && e.conveyor.is_some());
    if result.restart_chart {
        playback.time = 0.0;
        playback.playing = true;
    } else if line.chart_follows && (!triggered || feeds_conveyor) && !playback.playing {
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
        conveyor: line.conveyor,
    };
    line_signals(flow, sim, &sources, &line.inputs)
}

/// コンベアのワークの数を覚える (信号表は次のフレームの PLC とのやりとりで使う)
fn read_conveyor(
    mut line: ResMut<LineState>,
    config: Res<ConveyorConfigRes>,
    works: Res<WorkState>,
) {
    let counts = (!config.sources.is_empty()).then(|| ConveyorCounts {
        fed: works.counts().total_fed(),
        discharged: works.counts().total_discharged(),
        in_process: works.in_process(),
    });
    if line.conveyor != counts {
        line.bypass_change_detection().conveyor = counts;
    }
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
