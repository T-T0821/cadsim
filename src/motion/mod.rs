//! タイミングチャート (ステップベース) からの動作シミュレーション
//!
//! - `chart`: アクチュエータとステップのデータ、時刻表への変換 (Bevy 非依存)
//! - `pose`: 時刻 t における部品の位置・姿勢 (f64 の剛体変換)
//! - `ui`: タイミングチャートの編集画面と再生コントロール
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
mod ui;

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

pub use chart::{Schedule, TimingChart};
pub use pose::RigidPose;

use crate::PartRoot;

pub struct MotionPlugin;

impl Plugin for MotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimingChart>()
            .init_resource::<ChartSchedule>()
            .init_resource::<Playback>()
            .init_resource::<SimulationPose>()
            .init_resource::<ui::ChartUiState>()
            .add_systems(EguiPrimaryContextPass, ui::timing_chart_window)
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
            );
    }
}

/// 再生中の部品位置が決まるシステムのまとまり。
/// 干渉チェックなど、動いた後の位置を使う処理は `.after(MotionSet::Pose)` で登録する。
#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum MotionSet {
    Pose,
}

/// チャートを時刻に展開したもの。チャートが編集されたときだけ作り直す。
#[derive(Resource, Default, Deref)]
pub struct ChartSchedule(Schedule);

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

fn update_schedule(chart: Res<TimingChart>, mut schedule: ResMut<ChartSchedule>) {
    if chart.is_changed() {
        schedule.0 = chart.schedule();
    }
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
    } else if playback.looping {
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
