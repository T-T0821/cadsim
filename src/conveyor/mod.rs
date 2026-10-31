//! ワーク搬送 (投入口・コンベア・グリッパ・排出口) のアプリ側
//!
//! 計算は `cadsim_core::conveyor` にあり、ここは次だけを行う。
//! - 入力を集める: 設定、投入口・排出口・ベルトにする部品の外接箱、タイミングチャートの再生位置
//! - `ConveyorSim::seek` を呼ぶ (グリッパと動く台は [`ChartMotion`] がチャートから求める)
//! - 結果のワークを 3D 表示に写す
//!
//! 繰り返し再生ではサイクルをまたいでもワークが残り、流れ続ける。シーク位置を戻すと 0 から計算し直す。
//!
//! ## センサーなどとのつなぎ方
//!
//! 現在のワークは [`WorkState::works`]、投入数・排出数は [`WorkState::counts`] で取れる。
//! 毎フレームの位置を使う処理は `.after(ConveyorSet::Works)` で登録する。時刻表を作りながら任意の時刻の
//! ワークを知りたい場合は、`ConveyorSim` を [`ChartMotion`] (`repeat: false`) と一緒に作り、
//! `advance_to` で時刻順に進める。

mod ui;

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use cadsim_core::conveyor::{
    Bounds, Carrier, ConveyorConfig, ConveyorInputs, ConveyorSim, Counts, CycleClock, Gripper,
    GripperState, MachineMotion, Place, Pose, Work,
};
use glam::DVec3;

use crate::model::{Model, NodeId};
use crate::motion::{
    ActuatorKind, ChartSchedule, MotionSet, Playback, RigidPose, Schedule, TimingChart,
    actuator_poses,
};

pub struct ConveyorPlugin;

impl Plugin for ConveyorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConveyorConfigRes>()
            .init_resource::<WorkState>()
            .init_resource::<ConveyorClock>()
            .init_resource::<WorkMaterials>()
            .add_systems(Startup, setup_work_mesh)
            .add_systems(EguiPrimaryContextPass, ui::conveyor_window)
            .add_systems(
                Update,
                (update_works, sync_work_entities, draw_conveyor_overlay)
                    .chain()
                    .in_set(ConveyorSet::Works)
                    .after(MotionSet::Pose),
            );
    }
}

/// ワークの位置が決まるシステムのまとまり。
/// ワークを検知するセンサーなどは `.after(ConveyorSet::Works)` で登録する。
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConveyorSet {
    Works,
}

/// ワーク搬送の設定 (ドメインの [`ConveyorConfig`] を Resource として包む)
#[derive(Resource, Default, Deref, DerefMut)]
pub struct ConveyorConfigRes(pub ConveyorConfig);

/// いま表示しているワークの状態
#[derive(Resource, Default)]
pub struct WorkState {
    sim: ConveyorSim,
    /// 計算に使った外接箱。部品が編集されたら計算し直すために覚えておく。
    parts: HashMap<NodeId, Bounds>,
    carrier_parts: HashMap<u32, Vec<Bounds>>,
}

impl WorkState {
    pub fn works(&self) -> &[Work] {
        &self.sim.works
    }

    pub fn counts(&self) -> &Counts {
        &self.sim.counts
    }

    /// 投入されて、まだ排出されていないワークの数
    pub fn in_process(&self) -> usize {
        self.sim.in_process()
    }
}

#[derive(Resource, Default, Deref, DerefMut)]
struct ConveyorClock(CycleClock);

/// タイミングチャートから装置の動きを求める。
/// - チャックはシリンダで、値が 1 (出端) になり切ったら閉じている。掴んだワークは
///   チャックの「載せ先」アクチュエータと一緒に動く。
/// - 部品を割り当てたアクチュエータは、その部品の上面にワークを載せて動ける台になる。
pub struct ChartMotion<'a> {
    pub chart: &'a TimingChart,
    pub schedule: &'a Schedule,
    /// アクチュエータ id → 割り当てた部品の設計位置での外接箱
    pub carrier_parts: &'a HashMap<u32, Vec<Bounds>>,
    /// true ならサイクルの最後まで行ったらチャートの先頭に戻って繰り返す。
    /// false なら経過時刻 = チャート上の時刻 (作りかけの時刻表でも使える)。
    pub repeat: bool,
}

impl ChartMotion<'_> {
    fn chart_time(&self, time: f64) -> f64 {
        let cycle = self.schedule.cycle_time;
        if self.repeat && cycle > 0.0 {
            time % cycle
        } else {
            time
        }
    }
}

impl MachineMotion for ChartMotion<'_> {
    fn gripper_state(&self, gripper: &Gripper, time: f64) -> Option<GripperState> {
        let chuck = self
            .chart
            .actuator(gripper.chuck?)
            .filter(|a| a.kind == ActuatorKind::Cylinder)?;
        let t = self.chart_time(time);
        let carrier = chuck
            .mounted_on
            .filter(|id| self.chart.actuator(*id).is_some())
            .and_then(|id| actuator_poses(self.chart, self.schedule, t).remove(&id))
            .map_or(Pose::IDENTITY, |p| to_pose(&p));
        Some(GripperState {
            closed: self.schedule.value_at(chuck, t) >= 1.0 - 1e-6,
            carrier,
        })
    }

    fn carriers(&self, time: f64) -> Vec<Carrier> {
        if self.carrier_parts.is_empty() {
            return Vec::new();
        }
        let poses = actuator_poses(self.chart, self.schedule, self.chart_time(time));
        self.carrier_parts
            .iter()
            .filter_map(|(&id, parts)| {
                let pose = to_pose(poses.get(&id)?);
                Some(Carrier {
                    id,
                    pose,
                    surfaces: parts.iter().map(|b| b.transformed(pose)).collect(),
                })
            })
            .collect()
    }
}

fn to_pose(pose: &RigidPose) -> Pose {
    Pose {
        rotation: pose.rotation,
        translation: pose.translation,
    }
}

/// 設計位置での外接箱 (アセンブリや取り込んだ形状でもよい)
fn node_bounds(model: &Model, id: NodeId) -> Option<Bounds> {
    let aabb = model.world_bounds(id)?;
    Some(Bounds::from_min_max(aabb.min, aabb.max))
}

/// 投入口・排出口・ベルトに使う部品の外接箱
fn part_bounds(config: &ConveyorConfig, model: &Model) -> HashMap<NodeId, Bounds> {
    config
        .parts_in_use()
        .into_iter()
        .filter_map(|id| Some((id, node_bounds(model, id)?)))
        .collect()
}

/// 部品を割り当てたアクチュエータ → その部品の外接箱 (ワークを載せる台)
fn carrier_parts(chart: &TimingChart, model: &Model) -> HashMap<u32, Vec<Bounds>> {
    chart
        .actuators
        .iter()
        .filter(|a| !a.parts.is_empty())
        .map(|a| {
            let parts = a.parts.iter().filter_map(|&id| node_bounds(model, id));
            (a.id, parts.collect())
        })
        .collect()
}

fn update_works(
    config: Res<ConveyorConfigRes>,
    model: Res<Model>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    playback: Res<Playback>,
    mut clock: ResMut<ConveyorClock>,
    mut state: ResMut<WorkState>,
) {
    let target = clock.elapsed(playback.time, playback.playing, schedule.cycle_time);
    if config.is_empty() {
        if !state.sim.works.is_empty() {
            *state = WorkState::default();
        }
        return;
    }

    // 設定・チャート・部品の形が変わったら、最初から計算し直す
    let parts = part_bounds(&config, &model);
    let carriers = carrier_parts(&chart, &model);
    if config.is_changed()
        || chart.is_changed()
        || parts != state.parts
        || carriers != state.carrier_parts
    {
        *state = WorkState {
            parts,
            carrier_parts: carriers,
            ..default()
        };
    }
    let state = &mut *state;
    let motion = ChartMotion {
        chart: &chart,
        schedule: &schedule,
        carrier_parts: &state.carrier_parts,
        repeat: true,
    };
    let inputs = ConveyorInputs {
        config: &config,
        parts: &state.parts,
        motion: &motion,
    };
    state.sim.seek(&inputs, target);
}

// ---------------------------------------------------------------------------
// 表示
// ---------------------------------------------------------------------------

/// ワークの表示用エンティティ
#[derive(Component)]
struct WorkEntity(u32);

/// 1 x 1 x 1 の箱。Transform の scale でワークの寸法に伸ばす。
#[derive(Resource)]
struct WorkMesh(Handle<Mesh>);

/// ワーク種類の色 → マテリアル
#[derive(Resource, Default)]
struct WorkMaterials(HashMap<[u32; 3], Handle<StandardMaterial>>);

fn setup_work_mesh(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(WorkMesh(meshes.add(Cuboid::new(1.0, 1.0, 1.0))));
}

/// 種類の色。種類が消えていたら灰色。
fn work_color(config: &ConveyorConfig, work: &Work) -> [f32; 3] {
    config.kind(work.kind).map_or([0.6; 3], |k| k.color)
}

/// ワークの増減と移動を、表示用エンティティに反映する
fn sync_work_entities(
    mut commands: Commands,
    config: Res<ConveyorConfigRes>,
    state: Res<WorkState>,
    mesh: Res<WorkMesh>,
    mut materials: ResMut<WorkMaterials>,
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut entities: Query<(Entity, &WorkEntity, &mut Transform)>,
) {
    let works: HashMap<u32, &Work> = state.works().iter().map(|w| (w.id, w)).collect();
    let mut shown = Vec::new();
    for (entity, tag, mut transform) in &mut entities {
        match works.get(&tag.0) {
            Some(work) => {
                transform.set_if_neq(work_transform(work));
                shown.push(tag.0);
            }
            None => commands.entity(entity).despawn(),
        }
    }
    for work in state.works().iter().filter(|w| !shown.contains(&w.id)) {
        let color = work_color(&config, work);
        let material = materials
            .0
            .entry(color.map(f32::to_bits))
            .or_insert_with(|| {
                assets.add(StandardMaterial {
                    base_color: Color::srgb(color[0], color[1], color[2]),
                    perceptual_roughness: 0.5,
                    ..default()
                })
            })
            .clone();
        commands.spawn((
            WorkEntity(work.id),
            Mesh3d(mesh.0.clone()),
            MeshMaterial3d(material),
            work_transform(work),
        ));
    }
}

fn work_transform(work: &Work) -> Transform {
    Transform {
        translation: work.pose.translation.as_vec3(),
        rotation: work.pose.rotation.as_quat(),
        scale: work.size.as_vec3(),
    }
}

fn bounds_transform(bounds: &Bounds) -> Transform {
    Transform::from_translation(bounds.center.as_vec3()).with_scale(bounds.size.as_vec3())
}

/// ワークの稜線、投入口 (緑) と排出口 (赤) の枠、コンベアの向き、グリッパの掴む位置を線で描く
fn draw_conveyor_overlay(
    mut gizmos: Gizmos,
    config: Res<ConveyorConfigRes>,
    state: Res<WorkState>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    playback: Res<Playback>,
) {
    for work in state.works() {
        gizmos.cube(work_transform(work), Color::srgb(0.25, 0.2, 0.1));
    }
    for source in &config.sources {
        if let Some(bounds) = source.part.and_then(|id| state.parts.get(&id)) {
            gizmos.cube(bounds_transform(bounds), Color::srgb(0.2, 0.85, 0.4));
        }
    }
    for sink in &config.sinks {
        if let Some(bounds) = sink.part.and_then(|id| state.parts.get(&id)) {
            gizmos.cube(bounds_transform(bounds), Color::srgb(0.95, 0.3, 0.3));
        }
    }

    let arrow_color = Color::srgb(0.1, 0.75, 0.35);
    for conveyor in &config.conveyors {
        let Some(belt) = conveyor.part.and_then(|id| state.parts.get(&id)) else {
            continue;
        };
        let direction = conveyor.direction();
        let half_length = conveyor.along(belt.size) / 2.0 * 0.8;
        let top = belt.center + DVec3::Y * belt.size.y * 0.51;
        gizmos.arrow(
            (top - direction * half_length).as_vec3(),
            (top + direction * half_length).as_vec3(),
            arrow_color,
        );
    }

    let motion = ChartMotion {
        chart: &chart,
        schedule: &schedule,
        carrier_parts: &state.carrier_parts,
        repeat: false,
    };
    for gripper in &config.grippers {
        let Some(gripper_state) = motion.gripper_state(gripper, playback.time) else {
            continue;
        };
        let tcp = gripper_state
            .carrier
            .transform_point(DVec3::from_array(gripper.tcp));
        let holding = state
            .works()
            .iter()
            .any(|w| matches!(w.place, Place::Held { gripper: g, .. } if g == gripper.id));
        let color = if holding {
            Color::srgb(0.95, 0.2, 0.2)
        } else {
            Color::srgb(0.2, 0.6, 0.95)
        };
        gizmos.sphere(
            Isometry3d::from_translation(tcp.as_vec3()),
            gripper.reach as f32,
            color,
        );
    }
}
