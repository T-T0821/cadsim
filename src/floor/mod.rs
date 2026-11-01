//! 床の上を動くもの: 作業者と AGV (無人搬送車)
//!
//! | ファイル | 役割 |
//! | --- | --- |
//! | `plantwright_core::floor` | 床の上の長方形・距離 (geom)、作業者の時刻表 (operator_plan)、AGV の配車 (agv_sim) |
//! | `plantwright_core::floor::figure` | 作業者と AGV の 3D の形・歩く動き・向き |
//! | `plantwright_core::floor::figure_motion` | 動きのクリップ (歩く・作業・待つ、車輪・荷) と切り替えで混ぜる重み |
//! | `figures.rs` | 作業者と AGV の 3D モデルの表示 (形をメッシュに、動きのクリップを Bevy のアニメーションに写す) |
//! | `view.rs` | 歩行経路・AGV の通路の線と、床をクリックして点を置く操作 |
//! | `heatmap_view.rs` | 人と AGV の動線のヒートマップ (床に敷く色と PNG の書き出し) |
//! | `panel.rs` | 「作業者・AGV」パネル |
//!
//! 作業者と AGV はタイミングチャートとは別の時計 ([`FloorClock`]) で動く。
//! どちらも安全機器の検知の対象 (`SafetyIntruders`) に入り、AGV は安全停止中は止まる。

use plantwright_core::floor::{agv_sim, operator_plan};
pub(crate) mod figures;
mod heatmap_view;
mod panel;
pub(crate) mod sample;
mod view;

use std::collections::HashMap;

use bevy::ecs::system::{SystemParam, SystemState};
use bevy::math::{DVec2, DVec3};
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use crate::floor_record::{FloorLayout, FloorRecord};
use crate::history::{TrackDocExt, TrackedDoc};
use crate::safety::{Intruder, SafetyIntruders, SafetyLayoutRes, SafetySet, SafetyState};
use crate::view::camera::pointer_in_3d_view;
use agv_sim::{AgvGraph, AgvParams, AgvSim, FleetResult, Flow};
use operator_plan::{Operator, OperatorPlan};
use plantwright_core::math::Aabb;

/// 作業者の体の大きさ (mm)
const PERSON_RADIUS: f64 = 300.0;
const PERSON_HEIGHT: f64 = 1700.0;
/// AGV の高さ (mm)
pub(crate) const AGV_HEIGHT: f64 = 400.0;

pub struct FloorPlugin;

impl Plugin for FloorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FloorClock>()
            .track_doc::<FloorTracked>()
            .init_resource::<OperatorsRes>()
            .init_resource::<AgvLayoutRes>()
            .init_resource::<FloorState>()
            .init_resource::<figures::FigureAssets>()
            .init_resource::<heatmap_view::HeatmapRes>()
            .add_systems(Startup, heatmap_view::setup)
            .add_systems(
                Update,
                (
                    (
                        update_plans,
                        advance_clock,
                        heatmap_view::record,
                        heatmap_view::sync_overlay,
                    )
                        .chain(),
                    heatmap_view::export_if_requested,
                    view::floor_click.run_if(pointer_in_3d_view),
                    add_intruders.in_set(SafetySet::Intruders),
                    view::draw.after(SafetySet::Detect),
                    figures::sync_figures.after(SafetySet::Detect),
                ),
            )
            .add_systems(EguiPrimaryContextPass, panel::floor_panel);
    }
}

/// 作業者と AGV の時計
#[derive(Resource)]
pub struct FloorClock {
    pub time: f64,
    pub playing: bool,
    pub speed: f64,
}

impl Default for FloorClock {
    fn default() -> Self {
        Self {
            time: 0.0,
            playing: false,
            speed: 1.0,
        }
    }
}

/// 作業者の一覧
#[derive(Resource, Default)]
pub struct OperatorsRes {
    pub operators: Vec<Operator>,
    pub next_id: u32,
}

impl OperatorsRes {
    fn add(&mut self, start: [f64; 2]) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.operators.push(Operator::new(id, start));
        id
    }
}

/// AGV の経路・搬送の要求・性能
#[derive(Resource, Default)]
pub struct AgvLayoutRes {
    pub graph: AgvGraph,
    pub flows: Vec<Flow>,
    pub params: AgvParams,
}

/// 床をクリックしたときの操作
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FloorTool {
    #[default]
    None,
    /// AGV の経路の点を置く
    AddNode,
    /// AGV のステーションを置く
    AddStation,
    /// 2つの点をクリックして通路を結ぶ (もう一度で外す)
    Connect,
    /// 作業者の歩く先を足す
    AddWalk(u32),
}

/// 計算結果とパネルの状態
#[derive(Resource, Default)]
pub struct FloorState {
    /// 作業者ごとの計画
    plans: HashMap<u32, OperatorPlan>,
    /// 実行中の AGV シミュレーション (経路や台数を変えると作り直す)
    sim: Option<AgvSim>,
    compare: Vec<FleetResult>,
    compare_hours: f64,
    compare_max: usize,
    tool: FloorTool,
    /// 通路を結ぶときの 1つ目の点
    connect_from: Option<u32>,
    selected_operator: Option<u32>,
    selected_node: Option<u32>,
    show_paths: bool,
}

impl FloorState {
    fn reset_sim(&mut self) {
        self.sim = None;
    }

    /// 実行中の AGV シミュレーション (ライン連携から使う)
    pub(crate) fn agv_sim(&self) -> Option<&AgvSim> {
        self.sim.as_ref()
    }

    pub(crate) fn agv_sim_mut(&mut self) -> Option<&mut AgvSim> {
        self.sim.as_mut()
    }

    /// AGV を最初の位置から走らせ直す
    pub(crate) fn restart_agv(&mut self, layout: &AgvLayoutRes) {
        self.sim = Some(AgvSim::new(&layout.graph, &layout.flows, &layout.params));
    }
}

/// 手順や経路が変わったら計画を作り直す
fn update_plans(
    operators: Res<OperatorsRes>,
    layout: Res<AgvLayoutRes>,
    mut state: ResMut<FloorState>,
) {
    if operators.is_changed() {
        state.plans = operators
            .operators
            .iter()
            .map(|op| (op.id, OperatorPlan::new(op)))
            .collect();
    }
    if layout.is_changed() {
        state.reset_sim();
    }
    if state.sim.is_none() {
        state.sim = Some(AgvSim::new(&layout.graph, &layout.flows, &layout.params));
    }
}

fn advance_clock(
    time: Res<Time>,
    mut clock: ResMut<FloorClock>,
    mut state: ResMut<FloorState>,
    safety: Res<SafetyState>,
) {
    if !clock.playing {
        return;
    }
    let dt = time.delta_secs_f64() * clock.speed;
    clock.time += dt;
    if let Some(sim) = &mut state.sim {
        sim.advance(dt, safety.is_stopped());
    }
}

/// 作業者の床の位置
fn operator_position(state: &FloorState, op: &Operator, time: f64) -> DVec2 {
    let start = DVec2::from_array(op.start);
    state
        .plans
        .get(&op.id)
        .map_or(start, |plan| plan.position_at(time, start))
}

/// 作業者と AGV を安全機器の検知の対象に入れる
fn add_intruders(
    clock: Res<FloorClock>,
    operators: Res<OperatorsRes>,
    state: Res<FloorState>,
    mut intruders: ResMut<SafetyIntruders>,
) {
    for op in &operators.operators {
        let p = operator_position(&state, op, clock.time);
        intruders.0.push(Intruder {
            label: op.name.clone(),
            bounds: Aabb {
                min: DVec3::new(p.x - PERSON_RADIUS, 0.0, p.y - PERSON_RADIUS),
                max: DVec3::new(p.x + PERSON_RADIUS, PERSON_HEIGHT, p.y + PERSON_RADIUS),
            },
        });
    }
    let Some(sim) = &state.sim else {
        return;
    };
    for v in &sim.vehicles {
        let corners = agv_corners(sim, v);
        let mut bounds = Aabb {
            min: DVec3::new(f64::INFINITY, 0.0, f64::INFINITY),
            max: DVec3::new(f64::NEG_INFINITY, AGV_HEIGHT, f64::NEG_INFINITY),
        };
        for c in corners {
            bounds.min.x = bounds.min.x.min(c.x);
            bounds.min.z = bounds.min.z.min(c.y);
            bounds.max.x = bounds.max.x.max(c.x);
            bounds.max.z = bounds.max.z.max(c.y);
        }
        intruders.0.push(Intruder {
            label: format!("AGV {}", v.id + 1),
            bounds,
        });
    }
}

/// AGV の車体の四隅 (床の座標)
fn agv_corners(sim: &AgvSim, v: &agv_sim::Vehicle) -> [DVec2; 4] {
    let (p, dir) = v.pose(sim.graph());
    let side = dir.perp();
    let (l, w) = (sim.params().length / 2.0, sim.params().width / 2.0);
    [
        p - dir * l - side * w,
        p + dir * l - side * w,
        p + dir * l + side * w,
        p - dir * l + side * w,
    ]
}

/// 保存・読み込みのための、床の配置 (安全機器・作業者・AGV) の読み取り
#[derive(SystemParam)]
pub struct FloorView<'w> {
    safety: Res<'w, SafetyLayoutRes>,
    operators: Res<'w, OperatorsRes>,
    agv: Res<'w, AgvLayoutRes>,
}

impl FloorView<'_> {
    pub fn record(&self) -> FloorRecord {
        FloorRecord::capture(
            &self.safety,
            &self.operators.operators,
            &self.agv.graph,
            &self.agv.flows,
            &self.agv.params,
        )
    }
}

/// 安全機器・作業者・AGV の配置の編集を元に戻す履歴に載せる。
/// 比べるのは保存するのと同じ内容 (`FloorRecord`) なので、動かしている間の状態は含まない。
struct FloorTracked;

impl TrackedDoc for FloorTracked {
    type Value = FloorRecord;
    const LABEL: &'static str = "安全機器・作業者・AGV を編集";

    fn read(world: &World) -> FloorRecord {
        let (Some(safety), Some(operators), Some(agv)) = (
            world.get_resource::<SafetyLayoutRes>(),
            world.get_resource::<OperatorsRes>(),
            world.get_resource::<AgvLayoutRes>(),
        ) else {
            return FloorRecord::default();
        };
        FloorRecord::capture(
            safety,
            &operators.operators,
            &agv.graph,
            &agv.flows,
            &agv.params,
        )
    }

    fn write(world: &mut World, value: FloorRecord) {
        let mut state = SystemState::<FloorDoc>::new(world);
        if let Ok(mut doc) = state.get_mut(world) {
            doc.apply(value.restore());
        }
        state.apply(world);
    }

    fn same(a: &FloorRecord, b: &FloorRecord) -> bool {
        a == b
    }
}

/// 保存・読み込みのための、床の配置の読み書き
#[derive(SystemParam)]
pub struct FloorDoc<'w> {
    safety: ResMut<'w, SafetyLayoutRes>,
    safety_state: ResMut<'w, SafetyState>,
    operators: ResMut<'w, OperatorsRes>,
    agv: ResMut<'w, AgvLayoutRes>,
    state: ResMut<'w, FloorState>,
    clock: ResMut<'w, FloorClock>,
    heatmap: ResMut<'w, heatmap_view::HeatmapRes>,
}

impl FloorDoc<'_> {
    pub fn record(&self) -> FloorRecord {
        FloorRecord::capture(
            &self.safety,
            &self.operators.operators,
            &self.agv.graph,
            &self.agv.flows,
            &self.agv.params,
        )
    }

    /// 開いた (または新規の) プロジェクトの配置に置き換え、時計と停止状態を最初に戻す
    pub fn apply(&mut self, layout: FloorLayout) {
        **self.safety = layout.safety;
        self.safety_state.clear_runtime();
        self.operators.next_id = layout.operators.iter().map(|o| o.id + 1).max().unwrap_or(1);
        self.operators.operators = layout.operators;
        self.agv.graph = layout.graph;
        self.agv.flows = layout.flows;
        self.agv.params = layout.params;
        *self.state = FloorState::default();
        *self.clock = FloorClock::default();
        self.heatmap.clear();
    }
}
