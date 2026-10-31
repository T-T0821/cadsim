//! タイミングチャート (ステップベース) からの動作シミュレーション
//!
//! - `chart`: アクチュエータとステップのデータ、時刻表への変換 (Bevy 非依存)
//! - `pose`: 時刻 t における部品の位置・姿勢 (f64 の剛体変換、順運動学)
//! - `kinematics`: ロボット (関節の直列リンク) のヤコビアンと逆運動学
//! - `teach`: 3D ビューでクリックした面から教示点の位置と工具の向きを決める
//! - `robot_time`: ロボットの移動の最短時間 (台形の速度) とサイクルタイムの見積もり
//! - `melfa`: ロボットの動きを MELFA-BASIC VI のプログラムにする
//! - `robot_path`: ロボットの移動の経路 (関節補間・直線補間) と、途中の届かない点・特異点の判定
//! - `sensor`: 光電・近接センサーの検知判定と、インターロック付きの時刻表 (Bevy 非依存)
//! - `ui`: タイミングチャートの編集画面と再生コントロール
//! - `targets`: モデルの部品を、センサーが検知する直方体に変換する (Bevy 非依存)
//! - `sensor_view`: 3D ビューにセンサーの光軸・検出範囲を描く
//! - `robot_ui`: ロボットの設定・ジョグ・教示点の欄 (右のプロパティ欄) と、3D ビューへの TCP・教示点の表示
//! - `demo`: 動作確認用のサンプル装置 (ガントリーのピック & プレース、4軸ロボット)
//! - `export`: 1サイクルの動きを書き出し用のアニメーション (glTF / USD) にする
//!
//! 設計データ (Model) は書き換えず、再生中の動きは [`SimulationPose`] に出す。
//! 3D 表示 (viewport.rs) は部品の配置にこの動きを重ねて描く。
//! アクチュエータにはアセンブリも割り当てられ、その場合は子孫の部品がまとめて動く。
//!
//! ## 干渉チェックとのつなぎ方
//!
//! 再生中の部品の位置は [`SimulationPose`] に入る。干渉チェックのシステムを
//! `.after(MotionSet::Pose)` で登録し、`SimulationPose` が変わったフレームだけ
//! `builder::transformed(&part.build_solid(), pose.to_truck_matrix())` で動いた後の
//! B-rep を作って判定すればよい (動かない部品は `parts` に入っていない)。

pub mod chart;
mod demo;
mod export;
use cadsim_core::motion::kinematics;
mod live_joints;
mod melfa;
use cadsim_core::motion::path_interference;
use cadsim_core::motion::pose;
use cadsim_core::motion::profile;
use cadsim_core::motion::robot_path;
use cadsim_core::motion::robot_time;
mod robot_ui;
use cadsim_core::motion::sensor;
mod sensor_view;
mod targets;
mod teach;
mod ui;
mod ur_ui;

use std::collections::HashMap;

use bevy::prelude::*;

pub use cadsim_core::motion::pose::{
    RigidPose, actuator_poses, actuator_poses_for, part_poses, part_poses_for,
};
pub use chart::{Action, Actuator, ActuatorKind, Axis, Schedule, Step, TimingChart};
#[cfg(test)]
pub use demo::load_demo;
pub use export::chart_animation;
pub use robot_ui::{show_robot_section, teach_tool_off};
pub use ui::{MotionUi, show_panel};

use crate::history::{TrackDocExt, TrackedDoc};
use crate::model::{Model, NodeId};
use crate::plc::plc_active;
use targets::{design_targets, extra_targets_at};

/// タイミングチャートの編集を元に戻す履歴に載せる
struct ChartDoc;

impl TrackedDoc for ChartDoc {
    type Value = TimingChart;
    const LABEL: &'static str = "タイミングチャートを編集";

    fn read(world: &World) -> TimingChart {
        world.resource::<TimingChart>().clone()
    }

    fn write(world: &mut World, mut value: TimingChart) {
        let mut chart = world.resource_mut::<TimingChart>();
        // センサーの強制 ON / OFF は動作確認用なので、戻さずに今の状態を残す
        for sensor in &mut value.sensors {
            sensor.force = chart
                .sensors
                .iter()
                .find(|s| s.id == sensor.id)
                .and_then(|s| s.force);
        }
        *chart = value;
    }

    fn same(a: &TimingChart, b: &TimingChart) -> bool {
        // 強制 ON / OFF の切り替えは履歴に載せない
        a.actuators == b.actuators
            && a.steps == b.steps
            && a.next_actuator_id == b.next_actuator_id
            && a.next_sensor_id == b.next_sensor_id
            && a.sensors.len() == b.sensors.len()
            && a.sensors.iter().zip(&b.sensors).all(|(x, y)| {
                let mut y = y.clone();
                y.force = x.force;
                *x == y
            })
    }
}

pub struct MotionPlugin;

impl Plugin for MotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimingChart>()
            .track_doc::<ChartDoc>()
            .init_resource::<ChartSchedule>()
            .init_resource::<Playback>()
            .init_resource::<SimulationPose>()
            .init_resource::<SensorStates>()
            .init_resource::<SensorTimeline>()
            .init_resource::<JogPose>()
            .init_resource::<ui::ChartUiState>()
            .configure_sets(Update, MotionSet::Sensors.after(MotionSet::Pose))
            .init_resource::<robot_ui::RobotUiState>()
            .configure_sets(Update, MotionSet::Drive.in_set(MotionSet::Pose))
            .add_systems(
                Update,
                (
                    update_schedule,
                    advance_playback,
                    clear_jog_on_playback,
                    // 実機 (UR) とつないでいる間は、受け取った角度をジョグの値にする
                    ur_ui::follow_ur,
                    // PLC 連携中は時刻表ではなく PLC が位置を決める (MotionSet::Drive)
                    update_pose.run_if(not(plc_active)),
                )
                    .chain()
                    .in_set(MotionSet::Pose),
            )
            .add_systems(
                Update,
                (update_sensor_states, sensor_view::draw_sensors)
                    .chain()
                    .in_set(MotionSet::Sensors),
            )
            .add_systems(Update, robot_ui::draw_robots.after(MotionSet::Pose))
            .add_systems(
                Update,
                robot_ui::check_path_interference.after(MotionSet::Pose),
            )
            .add_systems(
                Update,
                robot_ui::place_teach_point
                    .run_if(crate::camera::pointer_in_3d_view)
                    .after(MotionSet::Pose),
            )
            .add_systems(Update, robot_ui::export_melfa);
    }
}

/// 再生中の部品位置が決まるシステムのまとまり。
/// 干渉チェックなど、動いた後の位置を使う処理は `.after(MotionSet::Pose)` で登録する。
/// センサーの ON / OFF ([`SensorStates`]) を使う処理は `.after(MotionSet::Sensors)` で登録する。
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MotionSet {
    Pose,
    Sensors,
    /// 時刻表以外 (PLC 連携) で `SimulationPose` を書くシステム。`Pose` の中で動く。
    Drive,
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

/// 時刻 `time` における動き (設計位置からのワールド座標での剛体変換)。
/// キーはアクチュエータに割り当てたノード (部品またはアセンブリ)。動かないノードは含まない。
#[derive(Resource, Default)]
pub struct SimulationPose {
    pub time: f64,
    /// 時刻 `time` で実行中のステップ番号 (0 始まり)
    pub step: Option<usize>,
    pub parts: HashMap<NodeId, RigidPose>,
    /// 各アクチュエータ (関節) の値と、親の動きも含めた変換
    pub values: HashMap<u32, f64>,
    pub actuators: HashMap<u32, RigidPose>,
}

/// ロボットのジョグ (手動操作) で一時的に動かしている関節の値。
/// 空でない間は、チャートの値の代わりにこの値で部品を動かす。再生やスクラブで空に戻る。
#[derive(Resource, Default, PartialEq)]
pub struct JogPose {
    pub values: HashMap<u32, f64>,
}

impl SimulationPose {
    /// ノードの動き。自分に割り当てがなければ、いちばん近い祖先 (アセンブリ) の動きを使う。
    pub fn pose_of(&self, model: &Model, node: NodeId) -> Option<RigidPose> {
        if self.parts.is_empty() {
            return None;
        }
        model
            .ancestors(node)
            .iter()
            .rev()
            .find_map(|id| self.parts.get(id).copied())
    }
}

/// 再生位置が原点 (時刻 0) にあり、部品が設計位置にあるとき true。
/// ギズモでの編集はこのときだけ許可する (動いた位置を設計値として書き戻さないため)。
pub fn at_design_pose(playback: Res<Playback>, jog: Res<JogPose>) -> bool {
    !playback.playing && playback.time == 0.0 && jog.values.is_empty()
}

/// 再生位置が動いたら、ジョグをやめてチャートどおりの位置に戻す
fn clear_jog_on_playback(playback: Res<Playback>, mut jog: ResMut<JogPose>) {
    if playback.is_changed() && !playback.is_added() && !jog.values.is_empty() {
        jog.values.clear();
    }
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
    jog: Res<JogPose>,
    mut pose: ResMut<SimulationPose>,
) {
    if !(chart.is_changed() || schedule.is_changed() || playback.is_changed() || jog.is_changed()) {
        return;
    }
    let t = playback.time;
    pose.time = t;
    pose.step = schedule.step_at(t);
    let mut values = schedule.values_at(&chart, t);
    values.extend(jog.values.iter().map(|(&id, &v)| (id, v)));
    pose.actuators = pose::actuator_poses_for(&chart, &values);
    pose.parts = pose::part_poses_for(&chart, &pose.actuators);
    pose.values = values;
}

/// いま表示している各アクチュエータの動き (アクチュエータ id → 変換)。
/// アクチュエータの動きは、それが動かすノードのどれかの動きと同じ。
fn actuator_poses_of(chart: &TimingChart, pose: &SimulationPose) -> HashMap<u32, RigidPose> {
    chart
        .actuators
        .iter()
        .filter_map(|a| {
            let p = a.parts.iter().find_map(|node| pose.parts.get(node))?;
            Some((a.id, *p))
        })
        .collect()
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
    // 位置は SimulationPose から取る (時刻表でも PLC 連携でも、表示と同じ位置で検知する)
    let on = sensor::sensor_states(&chart, &actuator_poses_of(&chart, &pose), &targets, &extra);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> (Model, TimingChart) {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        demo::load_demo(&mut model, &mut chart, &mut Default::default());
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

        // [リフタ上昇端, チャック閉確認]: 下げるとリフタ上昇端が OFF、チャックを閉じると閉確認が ON
        let targets = design_targets(&model, &chart);
        let states = |t| sensor::sensor_states_at(&chart, &schedule, t, &targets, &[]);
        assert_eq!(states(0.0), vec![true, false]);
        assert_eq!(states(schedule.steps[2].end), vec![false, true]);
        assert_eq!(states(schedule.steps[3].end), vec![true, true]);
        assert_eq!(states(schedule.steps[6].end), vec![false, false]);
    }

    #[test]
    fn demo_stops_when_lifter_sensor_is_forced_off() {
        let (model, mut chart) = demo();
        chart.sensors[0].force = Some(false);
        let schedule = schedule_of(&model, &chart);
        // X軸を動かす S5 で止まる
        assert_eq!(schedule.stop.map(|s| s.step), Some(4));
    }
}
