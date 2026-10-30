//! タイミングチャート (ステップベース) からの動作シミュレーション
//!
//! - `chart`: アクチュエータとステップのデータ、時刻表への変換 (Bevy 非依存)
//! - `pose`: 時刻 t における部品の位置・姿勢 (f64 の剛体変換)
//! - `sensor`: 光電・近接センサーの検知判定と、インターロック付きの時刻表 (Bevy 非依存)
//! - `ui`: タイミングチャートの編集画面と再生コントロール
//! - `sensor_view`: 3D ビューにセンサーの光軸・検出範囲を描く
//! - `demo`: 動作確認用のサンプル装置 (ピック & プレース)
//!
//! 設計データ (BoxPart) は書き換えず、再生中の動きは表示用 Transform にだけ反映する。
//!
//! ## 干渉チェックとのつなぎ方
//!
//! 再生中の部品の位置は [`SimulationPose`] に入る。干渉チェックのシステムを
//! `.after(MotionSet::Pose)` で登録し、`SimulationPose` が変わったフレームだけ
//! `builder::transformed(&part.build_solid(), pose.to_truck_matrix())` で動いた後の
//! B-rep を作って判定すればよい (動かない部品は `parts` に入っていない)。

mod chart;
mod demo;
mod pose;
mod sensor;
mod sensor_view;
mod ui;

use std::collections::HashMap;

use bevy::math::{DQuat, DVec3};
use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

pub use chart::{Schedule, TimingChart};
pub use pose::RigidPose;
pub use sensor::TargetBox;

use crate::{BoxPart, Model, PartRoot};
use sensor::DesignTarget;

pub struct MotionPlugin;

impl Plugin for MotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimingChart>()
            .init_resource::<ChartSchedule>()
            .init_resource::<Playback>()
            .init_resource::<SimulationPose>()
            .init_resource::<SensorStates>()
            .init_resource::<SensorTimeline>()
            .init_resource::<ui::ChartUiState>()
            .add_systems(EguiPrimaryContextPass, ui::timing_chart_window)
            .configure_sets(Update, MotionSet::Sensors.after(MotionSet::Pose))
            .add_systems(
                Update,
                (
                    update_schedule,
                    advance_playback,
                    update_pose,
                    apply_pose_to_parts,
                )
                    .chain()
                    .in_set(MotionSet::Pose),
            )
            .add_systems(
                Update,
                (update_sensor_states, sensor_view::draw_sensors)
                    .chain()
                    .in_set(MotionSet::Sensors),
            );
    }
}

/// 再生中の部品位置が決まるシステムのまとまり。
/// 干渉チェックなど、動いた後の位置を使う処理は `.after(MotionSet::Pose)` で登録する。
/// センサーの ON / OFF ([`SensorStates`]) を使う処理は `.after(MotionSet::Sensors)` で登録する。
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MotionSet {
    Pose,
    Sensors,
}

/// チャートを時刻に展開したもの。チャートか部品が編集されたときだけ作り直す。
/// インターロックの待ち時間はセンサーの検知で決まるので、部品の形・位置にも依存する。
#[derive(Resource, Default, Deref)]
pub struct ChartSchedule(Schedule);

/// 再生位置でのセンサーの ON / OFF (チャートのセンサー順)
#[derive(Resource, Default)]
pub struct SensorStates {
    pub signals: Vec<SensorSignal>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SensorSignal {
    pub id: u32,
    pub name: String,
    pub on: bool,
}

impl SensorStates {
    pub fn is_on(&self, id: u32) -> bool {
        self.signals.iter().any(|s| s.id == id && s.on)
    }
}

/// サイクル全体でのセンサーの ON / OFF (チャートにセンサーの波形を描くため)。
/// センサー id → (時刻, ON/OFF) の並び。時刻表が変わったときだけ作り直す。
#[derive(Resource, Default)]
pub struct SensorTimeline {
    pub samples: HashMap<u32, Vec<(f64, bool)>>,
}

/// 1ステップあたり、センサーの波形を何点で調べるか
const TIMELINE_SAMPLES_PER_STEP: usize = 24;

/// 再生位置と再生状態
#[derive(Resource, Clone, Copy, PartialEq)]
pub struct Playback {
    /// サイクル開始からの時刻 (秒)
    pub time: f64,
    pub playing: bool,
    /// 再生速度の倍率
    pub speed: f64,
    /// サイクルの最後まで行ったら先頭に戻って繰り返す
    pub looping: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            time: 0.0,
            playing: false,
            speed: 1.0,
            looping: true,
        }
    }
}

/// 時刻 `time` における部品の位置 (設計位置からの剛体変換)。動かない部品は含まない。
#[derive(Resource, Default)]
pub struct SimulationPose {
    pub time: f64,
    /// 時刻 `time` で実行中のステップ番号 (0 始まり)
    pub step: Option<usize>,
    pub parts: HashMap<u32, RigidPose>,
}

/// 再生位置が原点 (時刻 0) にあり、部品が設計位置にあるとき true。
/// ギズモでの編集はこのときだけ許可する (動いた位置を設計値として書き戻さないため)。
pub fn at_design_pose(playback: Res<Playback>) -> bool {
    !playback.playing && playback.time == 0.0
}

/// 設計位置の部品を、センサーの検知対象にする (アクチュエータが動かす部品はその動きも付ける)
fn design_targets(model: &Model, chart: &TimingChart) -> Vec<DesignTarget> {
    model
        .parts
        .iter()
        .map(|part| DesignTarget {
            shape: part_box(part),
            actuator: chart
                .actuators
                .iter()
                .find(|a| a.parts.contains(&part.id))
                .map(|a| a.id),
        })
        .collect()
}

fn part_box(part: &BoxPart) -> TargetBox {
    // BoxPart は X → Y → Z の順に回している (Rz * Ry * Rx)
    let [x, y, z] = part.rotation_deg.map(f64::to_radians);
    TargetBox {
        id: part.id,
        center: DVec3::from_array(part.position),
        rotation: DQuat::from_rotation_z(z) * DQuat::from_rotation_y(y) * DQuat::from_rotation_x(x),
        half_size: DVec3::from_array(part.size) / 2.0,
    }
}

/// 部品以外の検知対象 (コンベア上のワークなど) を、時刻 t の位置で返す。
/// 時刻表の t より前の部分だけから決まるものなら、インターロックの待ちにも使える。
fn extra_targets_at(_schedule: &Schedule, _t: f64) -> Vec<TargetBox> {
    Vec::new()
}

fn update_schedule(
    chart: Res<TimingChart>,
    model: Res<Model>,
    mut schedule: ResMut<ChartSchedule>,
    mut timeline: ResMut<SensorTimeline>,
) {
    if !(chart.is_changed() || model.is_changed()) {
        return;
    }
    let targets = design_targets(&model, &chart);
    let new_schedule = sensor::schedule_with_sensors(&chart, |schedule, t| {
        (targets.clone(), extra_targets_at(schedule, t))
    });

    // センサーの波形: 各ステップを細かく区切って調べる
    let mut samples: HashMap<u32, Vec<(f64, bool)>> = HashMap::new();
    for span in &new_schedule.steps {
        for k in 0..=TIMELINE_SAMPLES_PER_STEP {
            let t =
                span.start + (span.end - span.start) * k as f64 / TIMELINE_SAMPLES_PER_STEP as f64;
            let extra = extra_targets_at(&new_schedule, t);
            let states = sensor::sensor_states_at(&chart, &new_schedule, t, &targets, &extra);
            for (sensor, on) in chart.sensors.iter().zip(states) {
                samples.entry(sensor.id).or_default().push((t, on));
            }
        }
    }
    timeline.samples = samples;
    schedule.0 = new_schedule;
}

fn advance_playback(time: Res<Time>, schedule: Res<ChartSchedule>, mut playback: ResMut<Playback>) {
    if !playback.playing {
        return;
    }
    let cycle = schedule.cycle_time;
    if cycle <= 0.0 {
        playback.playing = false;
        return;
    }
    let next = playback.time + time.delta_secs_f64() * playback.speed;
    if next < cycle {
        playback.time = next;
    } else if playback.looping && schedule.stop.is_none() {
        playback.time = next % cycle;
    } else {
        playback.time = cycle;
        playback.playing = false;
    }
}

fn update_pose(
    chart: Res<TimingChart>,
    schedule: Res<ChartSchedule>,
    playback: Res<Playback>,
    mut pose: ResMut<SimulationPose>,
) {
    if !(chart.is_changed() || schedule.is_changed() || playback.is_changed()) {
        return;
    }
    let t = playback.time;
    pose.time = t;
    pose.step = schedule.step_at(t);
    pose.parts = pose::part_poses(&chart, &schedule, t);
}

fn update_sensor_states(
    chart: Res<TimingChart>,
    model: Res<Model>,
    schedule: Res<ChartSchedule>,
    pose: Res<SimulationPose>,
    mut states: ResMut<SensorStates>,
) {
    let t = pose.time;
    let targets = design_targets(&model, &chart);
    let extra = extra_targets_at(&schedule, t);
    let on = sensor::sensor_states_at(&chart, &schedule, t, &targets, &extra);
    let signals: Vec<SensorSignal> = chart
        .sensors
        .iter()
        .zip(on)
        .map(|(sensor, on)| SensorSignal {
            id: sensor.id,
            name: sensor.name.clone(),
            on,
        })
        .collect();
    if states.signals != signals {
        states.signals = signals;
    }
}

/// 部品のルートエンティティの Transform を、再生中の位置に合わせる
fn apply_pose_to_parts(pose: Res<SimulationPose>, mut roots: Query<(&PartRoot, &mut Transform)>) {
    for (root, mut transform) in &mut roots {
        let p = pose.parts.get(&root.id).copied().unwrap_or_default();
        let wanted = Transform {
            translation: p.translation.as_vec3(),
            rotation: p.rotation.as_quat(),
            scale: Vec3::ONE,
        };
        transform.set_if_neq(wanted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OrbitCamera;

    fn demo() -> (Model, TimingChart) {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        demo::load_demo(&mut model, &mut chart, &mut OrbitCamera::default());
        (model, chart)
    }

    fn schedule_of(model: &Model, chart: &TimingChart) -> Schedule {
        let targets = design_targets(model, chart);
        sensor::schedule_with_sensors(chart, |_, _| (targets.clone(), Vec::new()))
    }

    #[test]
    fn demo_runs_through_its_interlocks() {
        let (model, chart) = demo();
        let schedule = schedule_of(&model, &chart);
        assert!(schedule.stop.is_none());
        assert!(schedule.steps.iter().all(|s| s.wait() == 0.0));

        // 下降中はリフタ上昇端が OFF、テーブルを回すと治具確認が OFF になる
        let targets = design_targets(&model, &chart);
        let states = |t| sensor::sensor_states_at(&chart, &schedule, t, &targets, &[]);
        assert_eq!(states(0.0), vec![true, true]);
        assert_eq!(states(schedule.steps[0].end - 0.1), vec![false, true]);
        assert_eq!(states(schedule.steps[5].end), vec![true, false]);
    }

    #[test]
    fn demo_stops_when_lifter_sensor_is_forced_off() {
        let (model, mut chart) = demo();
        chart.sensors[0].force = Some(false);
        let schedule = schedule_of(&model, &chart);
        // X軸を動かす S3 で止まる
        assert_eq!(schedule.stop.map(|s| s.step), Some(2));
    }
}
