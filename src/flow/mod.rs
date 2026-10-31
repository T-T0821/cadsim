//! コンベアによるワーク搬送と、グリッパによる把持・解放
//!
//! - `domain`: 設定とワークの動きの計算 (Bevy・egui に依存しない。単体テストはここ)
//! - このファイル: Bevy のプラグイン。入力を集めて `domain` を呼び、ワークを 3D 表示に写す
//! - `ui`: 設定の編集画面 (egui)
//!
//! タイミングチャートの再生位置 ([`Playback`]) に合わせてワークを動かす。繰り返し再生では
//! サイクルをまたいでもワークが残り、流れ続ける。シーク位置を戻したときは 0 から計算し直す。
//!
//! ## センサーなどとのつなぎ方
//!
//! 現在のワークは [`WorkState`] の `works()` で取れる。毎フレームの位置を使う処理は
//! `.after(FlowSet::Works)` で登録する。時刻表を作りながら任意の時刻のワークを知りたい場合は、
//! [`FlowSim`] を `repeat: false` で作って `advance_to` で進めればよい。

mod domain;
mod ui;

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use glam::DVec3;

#[allow(unused_imports)] // FlowInputs などはセンサー・インターロックから使う
pub use domain::{
    Belt, Conveyor, ConveyorEnd, CycleClock, FlowConfig, FlowInputs, FlowSim, Gripper, Place, Work,
};

use crate::model::Model;
use crate::motion::{ChartSchedule, MotionSet, Playback, TimingChart};

pub struct FlowPlugin;

impl Plugin for FlowPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FlowConfigRes>()
            .init_resource::<WorkState>()
            .init_resource::<FlowClock>()
            .add_systems(Startup, setup_work_assets)
            .add_systems(EguiPrimaryContextPass, ui::flow_window)
            .add_systems(
                Update,
                (update_works, sync_work_entities, draw_flow_overlay)
                    .chain()
                    .in_set(FlowSet::Works)
                    .after(MotionSet::Pose),
            );
    }
}

/// ワークの位置が決まるシステムのまとまり。
/// ワークを検知するセンサーなどは `.after(FlowSet::Works)` で登録する。
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlowSet {
    Works,
}

/// コンベアとグリッパの設定 (ドメインの [`FlowConfig`] を Bevy の Resource として包む)
#[derive(Resource, Default, Deref, DerefMut)]
pub struct FlowConfigRes(pub FlowConfig);

/// いま表示しているワークの状態
#[derive(Resource, Default)]
pub struct WorkState {
    sim: FlowSim,
    /// 計算に使ったベルトの形。部品が編集されたら計算し直すために覚えておく。
    belts: HashMap<u32, Belt>,
}

impl WorkState {
    pub fn works(&self) -> &[Work] {
        &self.sim.works
    }
}

#[derive(Resource, Default, Deref, DerefMut)]
struct FlowClock(CycleClock);

/// 設定と部品から、コンベア id → ベルトの形 を作る
fn belts_from_model(config: &FlowConfig, model: &Model) -> HashMap<u32, Belt> {
    config
        .conveyors
        .iter()
        .filter_map(|conveyor| {
            // 設計位置での外接箱をベルトとする (アセンブリや取り込んだ形状でもよい)
            let bounds = model.world_bounds(conveyor.part?)?;
            let belt = Belt {
                center: (bounds.min + bounds.max) / 2.0,
                size: bounds.max - bounds.min,
            };
            Some((conveyor.id, belt))
        })
        .collect()
}

fn update_works(
    config: Res<FlowConfigRes>,
    model: Res<Model>,
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    playback: Res<Playback>,
    mut clock: ResMut<FlowClock>,
    mut state: ResMut<WorkState>,
) {
    let target = clock.elapsed(playback.time, playback.playing, schedule.cycle_time);

    // 設定・チャート・ベルトの形が変わったら、最初から計算し直す
    let belts = belts_from_model(&config, &model);
    if config.is_changed() || chart.is_changed() || belts != state.belts {
        state.sim = FlowSim::default();
        state.belts = belts;
    }
    let state = &mut *state;
    let inputs = FlowInputs {
        config: &config,
        belts: &state.belts,
        chart: &chart,
        schedule: &schedule,
        repeat: true,
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
fn draw_flow_overlay(
    mut gizmos: Gizmos,
    config: Res<FlowConfigRes>,
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
        let top = belt.center + DVec3::Y * (belt.size.y / 2.0 + 0.01);
        gizmos.arrow(
            (top - direction * half_length).as_vec3(),
            (top + direction * half_length).as_vec3(),
            arrow_color,
        );
    }

    let poses = crate::motion::actuator_poses(&chart, &schedule, playback.time);
    for gripper in &config.grippers {
        let mount = gripper
            .chuck
            .and_then(|id| chart.actuator(id))
            .and_then(|a| a.mounted_on);
        let carrier = mount
            .and_then(|id| poses.get(&id).copied())
            .unwrap_or_default();
        let tcp = carrier.transform_point(DVec3::from_array(gripper.tcp));
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
