//! 物理演算: 重力・接触・摩擦でワークを動かす。ベルトコンベア、ケーブル・ホースの紐モデル。
//!
//! | ファイル | 層 | 役割 |
//! | --- | --- | --- |
//! | `plantwright_core::physics::settings` | ドメイン | 部品ごとの物理の設定 (動かない / 機構で動く / 物理で動く / 当たらない、当たり判定の形、ベルト) |
//! | `plantwright_core::physics::cable` | ドメイン | ケーブル・ホースの紐モデルと、曲げ半径・長さ不足・接触の判定 |
//! | `plantwright_core::physics::world` | ユースケース | 部品 → 剛体の変換、一定刻みでの進行、物理エンジンのポート `PhysicsEngine` |
//! | `engine.rs` | アダプター | ポートの bevy_rapier3d (Bevy の物理プラグイン) 実装。剛体はエンティティ、ベルトは接触フック |
//! | `records.rs` | アダプター | .plantwright に書く形。Bevy に依存しない |
//! | `mod.rs` / `panel.rs` / `sample.rs` | UI | Bevy のシステム、「物理演算」パネル、サンプルライン |
//!
//! このファイルのシステムは「部品と動きを集める → ユースケースを呼ぶ → 姿勢を [`PhysicsPose`] に写す」だけ。
//! 物理を進める [`step_physics`] は、プラグインのスケジュールを走らせるため `World` を丸ごと借りる。
//! 3D 表示 (viewport.rs) は [`displayed_matrix`] で、物理で動いた部品をその位置に描く。

mod engine;
mod panel;
mod records;
mod sample;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use bevy::ecs::system::SystemState;
use bevy::math::{DMat4, DVec3};
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use plantwright_core::physics::cable::{Cable, CableAnchor, CableKind, CableReport};
use plantwright_core::physics::settings::BodyMode;
pub use records::{CableRecord, PartPhysicsRecord};

use engine::{RapierBackendPlugin, RapierEngine};
use plantwright_core::physics::cable::{CableState, Obstacle};
use plantwright_core::physics::settings;
use plantwright_core::physics::world::{
    BodyDesc, BodyKey, BodyKind, Iso, PartGeometry, PartInput, PhysicsEngine, PhysicsRun, STEP_DT,
    part_body,
};

use crate::history::{History, TrackDocExt, TrackedDoc};
use crate::input::command_line::CommandLine;
use crate::motion::{ChartSchedule, MotionSet, Playback, SimulationPose};
use crate::simulation::{InterferenceCheck, SimBody, TriMesh};
use crate::view::camera::CameraRequest;
use plantwright_core::math::euler_deg_to_quat;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::model::NodeKind;
use plantwright_core::motion::chart::TimingChart;

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RapierBackendPlugin)
            .init_resource::<PhysicsState>()
            .track_doc::<CablesDoc>()
            .init_resource::<PhysicsPose>()
            .init_resource::<CableSim>()
            .init_gizmo_group::<CableGizmos>()
            .add_systems(Startup, (configure_cable_gizmos, start_from_command_line))
            .add_systems(EguiPrimaryContextPass, panel::physics_panel)
            // チャートを書き換えるので、時刻表を作り直す前に読み込む
            .add_systems(Update, load_sample.before(MotionSet::Pose))
            .add_systems(
                Update,
                (step_physics, step_cables)
                    .chain()
                    .in_set(PhysicsSet::Step)
                    .after(MotionSet::Pose),
            )
            .add_systems(Update, draw_cables.after(PhysicsSet::Step));
    }
}

/// 物理で動いた部品の位置が決まるシステムのまとまり。
/// 表示や干渉チェックなど、動いた後の位置を使う処理は `.after(PhysicsSet::Step)` で登録する。
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PhysicsSet {
    Step,
}

/// 物理演算の実行状態
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RunState {
    /// 部品は設計位置 (物理の世界はない)
    #[default]
    Stopped,
    Running,
    Paused,
}

/// パネルのボタンから頼まれた操作
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunRequest {
    Start,
    Pause,
    Stop,
}

/// 物理の世界と、パネルに出す統計
#[derive(Resource, Default)]
pub struct PhysicsState {
    pub state: RunState,
    pub request: Option<RunRequest>,
    /// パネルで「サンプルラインを読み込む」が押された
    pub load_sample: bool,
    /// サンプルラインを読み込んだら、そのまま物理演算を始める (`--physics`)
    start_after_load: bool,
    pub stats: Stats,
    run: Option<Running>,
    /// タイミングチャートの停止 (先頭へ戻す) を見つけるため、前のフレームの再生状態を覚える
    last_playback: (f64, bool),
}

/// 動いている物理の世界1つ分 (剛体そのものはエンジンのエンティティ)
struct Running {
    run: PhysicsRun,
    /// 機構で動く部品 (毎刻み、動作シミュレーションの位置へ動かす)
    mechanisms: Vec<NodeId>,
    /// 剛体にした部品 (部品が消えたら作り直す)
    parts: Vec<NodeId>,
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// 剛体の数 (うち物理で動くもの)
    pub bodies: usize,
    pub dynamic: usize,
    /// 1 フレームの物理の計算時間 (ミリ秒, なめらかにした値)
    pub frame_ms: f64,
    /// 画面のフレームレート (なめらかにした値)
    pub fps: f64,
    /// 開始からの物理の時刻 (秒)
    pub time: f64,
    /// 当たり判定を作れなかった部品
    pub skipped: Vec<String>,
}

/// 物理演算が止まっていて、部品が設計位置にあるとき true (ギズモでの編集はこのときだけ)
pub fn physics_stopped(state: Res<PhysicsState>) -> bool {
    state.state == RunState::Stopped
}

/// 物理で動いている部品のワールドでの配置
#[derive(Resource, Default)]
pub struct PhysicsPose {
    pub parts: HashMap<NodeId, DMat4>,
}

/// 部品をいま表示する配置。物理で動いていればその位置、なければ設計位置に動作シミュレーションの動きを重ねる。
pub fn displayed_matrix(
    model: &Model,
    motion: &SimulationPose,
    physics: &PhysicsPose,
    id: NodeId,
) -> DMat4 {
    if let Some(m) = physics.parts.get(&id) {
        return *m;
    }
    let matrix = model.world_matrix(id);
    match motion.pose_of(model, id) {
        Some(moved) => moved.to_dmat4() * matrix,
        None => matrix,
    }
}

fn to_iso(m: DMat4) -> Iso {
    let (_, rotation, translation) = m.to_scale_rotation_translation();
    Iso::new(rotation, translation)
}

fn to_matrix(iso: Iso) -> DMat4 {
    DMat4::from_rotation_translation(iso.rotation, iso.translation)
}

/// すべての部品 (アセンブリを除く)
fn all_parts(model: &Model) -> Vec<NodeId> {
    model
        .roots
        .iter()
        .flat_map(|&root| model.subtree(root))
        .filter(|&id| model.get(id).is_some_and(|n| n.kind.is_part()))
        .collect()
}

/// 部品の形。箱はパラメータから、取り込んだ部品は表示用メッシュから。
fn geometry<'a>(
    model: &Model,
    id: NodeId,
    meshes: &'a HashMap<u32, Arc<TriMesh>>,
) -> Option<PartGeometry<'a>> {
    match &model.get(id)?.kind {
        NodeKind::Box(part) => Some(PartGeometry::Box {
            size: DVec3::from(part.size),
            offset: Iso::new(
                euler_deg_to_quat(part.rotation_deg),
                DVec3::from(part.position),
            ),
        }),
        NodeKind::Imported { .. } | NodeKind::StandardBody { .. } => {
            Some(PartGeometry::Mesh(meshes.get(&id)?.triangles()))
        }
        NodeKind::Assembly { .. } | NodeKind::Standard(_) => None,
    }
}

/// 物理の世界を作るための剛体の一覧
struct Build {
    gravity: DVec3,
    meters_per_unit: f64,
    /// 部品・表示名・剛体
    bodies: Vec<(NodeId, String, BodyDesc)>,
    /// 当たり判定を作れなかった部品
    skipped: Vec<String>,
}

/// 今のモデルから剛体の一覧を作る。非表示の部品は入れない。
fn build_bodies(
    model: &Model,
    motion: &SimulationPose,
    meshes: &HashMap<u32, Arc<TriMesh>>,
) -> Build {
    let meters_per_unit = settings::meters_per_unit(&model.unit);
    let mut build = Build {
        gravity: settings::gravity(&model.unit),
        meters_per_unit,
        bodies: Vec::new(),
        skipped: Vec::new(),
    };
    for id in all_parts(model) {
        let Some(node) = model.get(id) else {
            continue;
        };
        if !model.is_shown(id) || node.physics.mode == BodyMode::Ignored {
            continue;
        }
        let Some(geometry) = geometry(model, id, meshes) else {
            continue;
        };
        let matrix = match node.physics.mode {
            BodyMode::Fixed => model.world_matrix(id),
            _ => displayed_matrix(model, motion, &PhysicsPose::default(), id),
        };
        let input = PartInput {
            settings: node.physics,
            geometry,
            pose: to_iso(matrix),
            meters_per_unit,
        };
        match part_body(&input) {
            Some(desc) => build.bodies.push((id, model.path_label(id), desc)),
            None => build.skipped.push(model.path_label(id)),
        }
    }
    build
}

/// 剛体の一覧をエンジンに入れて、物理の世界を作る
fn build_world(engine: &mut RapierEngine, build: Build) -> (Running, Stats) {
    engine.reset(build.gravity, build.meters_per_unit);
    let mut stats = Stats {
        skipped: build.skipped,
        ..Default::default()
    };
    let mut running = Running {
        run: PhysicsRun::new(),
        mechanisms: Vec::new(),
        parts: Vec::new(),
    };
    for (id, label, desc) in build.bodies {
        running.parts.push(id);
        if !engine.add_body(BodyKey::Part(id), &desc) {
            stats.skipped.push(label);
            continue;
        }
        stats.bodies += 1;
        match desc.kind {
            BodyKind::Kinematic => running.mechanisms.push(id),
            BodyKind::Dynamic => stats.dynamic += 1,
            BodyKind::Fixed => {}
        }
    }
    (running, stats)
}

/// `cargo run --release -- --physics --bench 20` のように、起動してすぐサンプルラインを流す
fn start_from_command_line(mut state: ResMut<PhysicsState>) {
    if CommandLine::parse().physics {
        state.load_sample = true;
        state.start_after_load = true;
    }
}

/// ケーブル・ホースの追加・削除・設定の変更を元に戻す履歴に載せる
struct CablesDoc;

impl TrackedDoc for CablesDoc {
    type Value = Vec<Cable>;
    const LABEL: &'static str = "ケーブル・ホースを編集";

    fn read(world: &bevy::ecs::world::World) -> Vec<Cable> {
        world.resource::<Model>().cables.clone()
    }

    fn write(world: &mut bevy::ecs::world::World, value: Vec<Cable>) {
        world.resource_mut::<Model>().cables = value;
    }

    fn same(a: &Vec<Cable>, b: &Vec<Cable>) -> bool {
        a == b
    }
}

/// サンプルラインを読み込み、物理演算を止めて全体を表示する
#[allow(clippy::too_many_arguments)]
fn load_sample(
    mut state: ResMut<PhysicsState>,
    mut model: ResMut<Model>,
    mut chart: ResMut<TimingChart>,
    mut playback: ResMut<Playback>,
    mut camera: MessageWriter<CameraRequest>,
    mut interference: ResMut<InterferenceCheck>,
    mut history: ResMut<History>,
) {
    if !state.load_sample {
        return;
    }
    state.load_sample = false;
    state.state = RunState::Stopped;
    if std::mem::take(&mut state.start_after_load) {
        state.request = Some(RunRequest::Start);
    }
    // 部品の入れ替えは1件の履歴にする (チャートとケーブルは見張りがそれぞれ1件にする)
    history.replace_all(
        &mut model,
        "サンプルラインを読み込み".into(),
        |model| sample::load_sample_line(model, &mut chart),
    );
    *playback = Playback::default();
    camera.write(CameraRequest::FitAll);
    // 載っているだけのワークも「干渉」と数えられ、一覧が埋まって重くなる
    interference.auto = false;
}

type StepParams<'w, 's> = (
    Res<'w, Time>,
    Res<'w, Model>,
    Res<'w, SimulationPose>,
    Res<'w, ChartSchedule>,
    ResMut<'w, Playback>,
    Query<'w, 's, &'static SimBody>,
    ResMut<'w, PhysicsState>,
);

/// このフレームで物理の世界にすること
#[derive(Default)]
struct Frame {
    /// 物理の世界を消す (止めた、部品が消えた)
    clear: bool,
    /// 物理の世界を作る
    build: Option<Build>,
    /// 進める時間 (秒) と、機構で動く剛体の目標の姿勢
    advance: Option<(f64, Vec<(BodyKey, Iso)>)>,
}

/// 物理を進める。前半で部品と動きを集めてすることを決め、後半でエンジン (プラグインのスケジュール) を動かす。
fn step_physics(world: &mut World, params: &mut SystemState<StepParams>) {
    let Ok(inputs) = params.get_mut(world) else {
        return;
    };
    let frame = plan_frame(inputs);
    params.apply(world);
    let Frame {
        clear,
        build,
        advance,
    } = frame;
    if clear {
        RapierEngine::new(world).clear();
        world.resource_mut::<PhysicsPose>().parts.clear();
        return;
    }
    let mut running = world.resource_mut::<PhysicsState>().run.take();
    let mut engine = RapierEngine::new(world);
    if let Some(build) = build {
        let (built, stats) = build_world(&mut engine, build);
        running = Some(built);
        let mut state = engine.world_mut().resource_mut::<PhysicsState>();
        state.stats = Stats {
            fps: state.stats.fps,
            ..stats
        };
    }
    if let (Some(running), Some((dt, targets))) = (running.as_mut(), advance) {
        let started = Instant::now();
        running.run.advance(&mut engine, dt, &targets);
        let parts = engine
            .dynamic_poses()
            .into_iter()
            .map(|(BodyKey::Part(id), iso)| (id, to_matrix(iso)))
            .collect();
        world.resource_mut::<PhysicsPose>().parts = parts;
        let mut state = world.resource_mut::<PhysicsState>();
        state.stats.time = running.run.time;
        state.stats.frame_ms = smooth(
            state.stats.frame_ms,
            started.elapsed().as_secs_f64() * 1000.0,
        );
    }
    world.resource_mut::<PhysicsState>().run = running;
}

/// パネルの操作とタイミングチャートの再生状態から、このフレームですることを決める
fn plan_frame(
    (time, model, motion, schedule, mut playback, bodies, mut state): StepParams,
) -> Frame {
    let state = &mut *state;
    let chart_runs = schedule.cycle_time > 0.0;

    // タイミングチャートを先頭に戻したら、物理も設計位置に戻す
    let now = (playback.time, playback.playing);
    let rewound = chart_runs && now == (0.0, false) && state.last_playback != (0.0, false);
    state.last_playback = now;

    match state.request.take() {
        Some(RunRequest::Start) => {
            state.state = RunState::Running;
            if chart_runs {
                playback.playing = true;
            }
        }
        Some(RunRequest::Pause) => {
            state.state = RunState::Paused;
            playback.playing = false;
        }
        Some(RunRequest::Stop) => {
            state.state = RunState::Stopped;
            playback.playing = false;
            playback.time = 0.0;
            state.last_playback = (0.0, false);
        }
        None if rewound && state.state != RunState::Stopped => state.state = RunState::Stopped,
        None => {}
    }

    // 部品が消えたら (プロジェクトを開き直したなど) 作り直す
    let stale = state
        .run
        .as_ref()
        .is_some_and(|r| r.parts.iter().any(|id| model.get(*id).is_none()));
    if state.state == RunState::Stopped || stale {
        if stale {
            state.state = RunState::Stopped;
        }
        return Frame {
            clear: state.run.take().is_some(),
            ..Default::default()
        };
    }

    let mut frame = Frame::default();
    let mechanisms = match &state.run {
        Some(running) => running.mechanisms.clone(),
        None => {
            let meshes = bodies.iter().map(|b| (b.id, b.mesh.clone())).collect();
            let build = build_bodies(&model, &motion, &meshes);
            let mechanisms = build
                .bodies
                .iter()
                .filter(|(_, _, desc)| desc.kind == BodyKind::Kinematic)
                .map(|(id, _, _)| *id)
                .collect();
            frame.build = Some(build);
            mechanisms
        }
    };
    let dt = time.delta_secs_f64();
    if dt > 0.0 {
        state.stats.fps = smooth(state.stats.fps, 1.0 / dt);
    }
    // チャートで動かしているときは、チャートの一時停止に合わせて止める
    let advancing = state.state == RunState::Running && (!chart_runs || playback.playing);
    if !advancing {
        return frame;
    }
    let empty = PhysicsPose::default();
    let targets = mechanisms
        .into_iter()
        .filter(|id| model.get(*id).is_some())
        .map(|id| {
            (
                BodyKey::Part(id),
                to_iso(displayed_matrix(&model, &motion, &empty, id)),
            )
        })
        .collect();
    frame.advance = Some((dt * playback.speed, targets));
    frame
}

/// 表示用の値をなめらかにする (指数移動平均)
fn smooth(previous: f64, value: f64) -> f64 {
    if previous == 0.0 {
        value
    } else {
        previous * 0.9 + value * 0.1
    }
}

// ---------------------------------------------------------------------------
// ケーブル・ホース
// ---------------------------------------------------------------------------

/// ケーブル・ホースの紐の状態と、調べた結果 (`model.cables` と同じ順)
#[derive(Resource, Default)]
pub struct CableSim {
    states: Vec<CableState>,
    pub reports: Vec<CableReport>,
    accumulator: f64,
}

/// 紐が当たる箱 (表示中の部品の外接箱)
fn obstacles(
    model: &Model,
    motion: &SimulationPose,
    physics: &PhysicsPose,
    meshes: &HashMap<u32, Arc<TriMesh>>,
) -> Vec<Obstacle> {
    all_parts(model)
        .into_iter()
        .filter(|&id| {
            model.is_shown(id)
                && model
                    .get(id)
                    .is_some_and(|n| n.physics.mode != BodyMode::Ignored)
        })
        .filter_map(|id| {
            let (center, rotation, half) = match geometry(model, id, meshes)? {
                PartGeometry::Box { size, offset } => {
                    (offset.translation, offset.rotation, size / 2.0)
                }
                PartGeometry::Mesh(_) => {
                    let aabb = meshes.get(&id)?.aabb();
                    (
                        (aabb.min + aabb.max) / 2.0,
                        Default::default(),
                        (aabb.max - aabb.min) / 2.0,
                    )
                }
            };
            let placed = to_iso(displayed_matrix(model, motion, physics, id));
            Some(Obstacle {
                node: id,
                pose: Iso::new(placed.rotation * rotation, placed.apply(center)),
                half,
            })
        })
        .collect()
}

/// 紐の端のワールドでの位置
fn anchor_point(
    model: &Model,
    motion: &SimulationPose,
    physics: &PhysicsPose,
    anchor: &CableAnchor,
) -> Option<DVec3> {
    match anchor.node {
        Some(id) => {
            model.get(id)?;
            Some(displayed_matrix(model, motion, physics, id).transform_point3(anchor.local))
        }
        None => Some(anchor.local),
    }
}

/// 紐は物理演算を止めていても毎フレーム進める (端を動かせば、その場で垂れ方が変わる)
fn step_cables(
    time: Res<Time>,
    model: Res<Model>,
    motion: Res<SimulationPose>,
    physics: Res<PhysicsPose>,
    bodies: Query<&SimBody>,
    mut sim: ResMut<CableSim>,
) {
    let sim = &mut *sim;
    if model.cables.is_empty() {
        sim.states.clear();
        sim.reports.clear();
        return;
    }
    sim.accumulator = (sim.accumulator + time.delta_secs_f64()).min(STEP_DT * 4.0);
    let meshes: HashMap<u32, Arc<TriMesh>> =
        bodies.iter().map(|b| (b.id, b.mesh.clone())).collect();
    let obstacles = obstacles(&model, &motion, &physics, &meshes);
    let gravity = settings::gravity(&model.unit);
    sim.states
        .resize_with(model.cables.len(), CableState::default);
    sim.reports
        .resize_with(model.cables.len(), CableReport::default);
    let steps = (sim.accumulator / STEP_DT).floor() as usize;
    sim.accumulator -= steps as f64 * STEP_DT;
    for (i, cable) in model.cables.iter().enumerate() {
        let ends = (
            anchor_point(&model, &motion, &physics, &cable.start),
            anchor_point(&model, &motion, &physics, &cable.end),
        );
        let (Some(start), Some(end)) = ends else {
            continue;
        };
        let state = &mut sim.states[i];
        if state.points.is_empty() {
            *state = CableState::new(cable, start, end);
        }
        for _ in 0..steps {
            state.step(cable, start, end, &obstacles, gravity, STEP_DT);
        }
        sim.reports[i] = state.report(cable, &obstacles);
    }
}

const CABLE_COLOR: Color = Color::srgb(0.15, 0.15, 0.15);
const HOSE_COLOR: Color = Color::srgb(0.20, 0.55, 0.95);
const WARNING_COLOR: Color = Color::srgb(1.0, 0.15, 0.15);

/// ケーブル・ホースを描くギズモ。細い線だと見えにくいので太くする。
#[derive(Default, Reflect, GizmoConfigGroup)]
struct CableGizmos;

fn configure_cable_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<CableGizmos>();
    config.line.width = 5.0;
}

fn draw_cables(mut gizmos: Gizmos<CableGizmos>, model: Res<Model>, sim: Res<CableSim>) {
    for ((cable, state), report) in model.cables.iter().zip(&sim.states).zip(&sim.reports) {
        let base = match cable.kind {
            CableKind::Cable => CABLE_COLOR,
            CableKind::Hose => HOSE_COLOR,
        };
        let points: Vec<Vec3> = state.points.iter().map(|p| p.as_vec3()).collect();
        if points.len() < 2 {
            continue;
        }
        // 長さが足りなければ全体を、曲げすぎならその前後の区間を赤くする
        for (i, pair) in points.windows(2).enumerate() {
            let bent = report.over_bent.iter().any(|&b| b == i || b == i + 1);
            let color = if report.is_taut() || bent {
                WARNING_COLOR
            } else {
                base
            };
            gizmos.line(pair[0], pair[1], color);
        }
        let radius = (cable.diameter / 2.0) as f32;
        for end in [points[0], points[points.len() - 1]] {
            gizmos.sphere(Isometry3d::from_translation(end), radius * 1.5, base);
        }
    }
}
