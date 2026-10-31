//! コンベアによるワーク搬送と、グリッパによる把持・解放 (アプリ側)
//!
//! 計算は `cadsim_core::conveyor` にあり、ここは次だけを行う。
//! - 入力を集める: 設定、ベルトにする部品の外接箱、タイミングチャートの再生位置
//! - `ConveyorSim::seek` を呼ぶ (グリッパの動きは [`ChartGripperMotion`] がチャートから求める)
//! - 結果のワークを 3D 表示に写す
//!
//! 繰り返し再生ではサイクルをまたいでもワークが残り、流れ続ける。シーク位置を戻すと 0 から計算し直す。
//!
//! ## センサーなどとのつなぎ方
//!
//! 現在のワークは [`WorkState::works`] で取れる。毎フレームの位置を使う処理は
//! `.after(ConveyorSet::Works)` で登録する。時刻表を作りながら任意の時刻のワークを知りたい場合は、
//! `ConveyorSim` を [`ChartGripperMotion`] (`repeat: false`) と一緒に作り、`advance_to` で時刻順に進める。

mod ui;

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use cadsim_core::conveyor::{
    Belt, ConveyorConfig, ConveyorInputs, ConveyorSim, CycleClock, Gripper, GripperMotion,
    GripperState, Place, Pose, Work,
};
use glam::DVec3;

use crate::model::Model;
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
            .add_systems(Startup, setup_work_assets)
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

/// コンベアとグリッパの設定 (ドメインの [`ConveyorConfig`] を Resource として包む)
#[derive(Resource, Default, Deref, DerefMut)]
pub struct ConveyorConfigRes(pub ConveyorConfig);

/// いま表示しているワークの状態
#[derive(Resource, Default)]
pub struct WorkState {
    sim: ConveyorSim,
    /// 計算に使ったベルトの形。部品が編集されたら計算し直すために覚えておく。
    belts: HashMap<u32, Belt>,
}

impl WorkState {
    pub fn works(&self) -> &[Work] {
        &self.sim.works
    }
}

#[derive(Resource, Default, Deref, DerefMut)]
struct ConveyorClock(CycleClock);

/// タイミングチャートからグリッパの動きを求める。
/// チャックはシリンダで、値が 1 (出端) になり切ったら閉じている。
/// 掴んだワークは、チャックの「載せ先」アクチュエータと一緒に動く。
pub struct ChartGripperMotion<'a> {
    pub chart: &'a TimingChart,
    pub schedule: &'a Schedule,
    /// true ならサイクルの最後まで行ったらチャートの先頭に戻って繰り返す。
    /// false なら経過時刻 = チャート上の時刻 (作りかけの時刻表でも使える)。
    pub repeat: bool,
}

impl ChartGripperMotion<'_> {
    fn chart_time(&self, time: f64) -> f64 {
        let cycle = self.schedule.cycle_time;
        if self.repeat && cycle > 0.0 {
            time % cycle
        } else {
            time
        }
    }

    /// 時刻 `chart_time` でチャックの載せ先を動かす変換 (載っていなければ動かない)
    fn carrier(&self, chuck_mount: Option<u32>, chart_time: f64) -> Pose {
        let Some(mount) = chuck_mount.filter(|id| self.chart.actuator(*id).is_some()) else {
            return Pose::IDENTITY;
        };
        actuator_poses(self.chart, self.schedule, chart_time)
            .get(&mount)
            .map_or(Pose::IDENTITY, to_pose)
    }
}

impl GripperMotion for ChartGripperMotion<'_> {
    fn gripper_state(&self, gripper: &Gripper, time: f64) -> Option<GripperState> {
        let chuck = self
            .chart
            .actuator(gripper.chuck?)
            .filter(|a| a.kind == ActuatorKind::Cylinder)?;
        let t = self.chart_time(time);
        Some(GripperState {
            closed: self.schedule.value_at(chuck, t) >= 1.0 - 1e-6,
            carrier: self.carrier(chuck.mounted_on, t),
        })
    }
}

fn to_pose(pose: &RigidPose) -> Pose {
    Pose {
        rotation: pose.rotation,
        translation: pose.translation,
    }
}

/// 設定と部品から、コンベア id → ベルトの形 を作る
fn belts_from_model(config: &ConveyorConfig, model: &Model) -> HashMap<u32, Belt> {
    config
        .conveyors
        .iter()
        .filter_map(|conveyor| {
            // 設計位置での外接箱をベルトとする (アセンブリや取り込んだ形状でもよい)
            let bounds = model.world_bounds(conveyor.part?)?;
            let belt = Belt {
                center: bounds.center(),
                size: bounds.size(),
            };
            Some((conveyor.id, belt))
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

    // 設定・チャート・ベルトの形が変わったら、最初から計算し直す
    let belts = belts_from_model(&config, &model);
    if config.is_changed() || chart.is_changed() || belts != state.belts {
        state.sim = ConveyorSim::default();
        state.belts = belts;
    }
    let state = &mut *state;
    let motion = ChartGripperMotion {
        chart: &chart,
        schedule: &schedule,
        repeat: true,
    };
    let inputs = ConveyorInputs {
        config: &config,
        belts: &state.belts,
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

#[derive(Resource)]
struct WorkAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

fn setup_work_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(WorkAssets {
        // 1 x 1 x 1 の箱を Transform の scale でワークの寸法に伸ばす
        mesh: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.95, 0.75, 0.25),
            perceptual_roughness: 0.5,
            ..default()
        }),
    });
}

/// ワークの増減と移動を、表示用エンティティに反映する
fn sync_work_entities(
    mut commands: Commands,
    state: Res<WorkState>,
    assets: Res<WorkAssets>,
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
        commands.spawn((
            WorkEntity(work.id),
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(assets.material.clone()),
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

/// ワークの稜線、コンベアの流れる向き、グリッパの掴む位置を線で描く
fn draw_conveyor_overlay(
    mut gizmos: Gizmos,
    config: Res<ConveyorConfigRes>,
    state: Res<WorkState>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    playback: Res<Playback>,
) {
    for work in state.works() {
        gizmos.cube(work_transform(work), Color::srgb(0.35, 0.22, 0.05));
    }

    let arrow_color = Color::srgb(0.1, 0.75, 0.35);
    for conveyor in &config.conveyors {
        let Some(belt) = state.belts.get(&conveyor.id) else {
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

    let motion = ChartGripperMotion {
        chart: &chart,
        schedule: &schedule,
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
