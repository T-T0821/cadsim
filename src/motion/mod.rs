//! タイミングチャート (ステップベース) からの動作シミュレーション
//!
//! - `chart`: アクチュエータとステップのデータ、時刻表への変換 (Bevy 非依存)
//! - `pose`: 時刻 t における部品の位置・姿勢 (f64 の剛体変換)
//! - `ui`: タイミングチャートの編集画面と再生コントロール
//! - `demo`: 動作確認用のサンプル装置 (ピック & プレース)
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
mod pose;
mod ui;

use std::collections::HashMap;

use bevy::prelude::*;

pub use chart::{Action, ActuatorKind, Axis, Schedule, Step, TimingChart};
#[cfg(test)]
pub use demo::load_demo;
pub use export::chart_animation;
pub use pose::{RigidPose, part_poses};
pub use ui::{MotionUi, show_panel};

use crate::model::{Model, NodeId};

pub struct MotionPlugin;

impl Plugin for MotionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimingChart>()
            .init_resource::<ChartSchedule>()
            .init_resource::<Playback>()
            .init_resource::<SimulationPose>()
            .init_resource::<ui::ChartUiState>()
            .add_systems(
                Update,
                (update_schedule, advance_playback, update_pose)
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

/// 時刻 `time` における動き (設計位置からのワールド座標での剛体変換)。
/// キーはアクチュエータに割り当てたノード (部品またはアセンブリ)。動かないノードは含まない。
#[derive(Resource, Default)]
pub struct SimulationPose {
    pub time: f64,
    /// 時刻 `time` で実行中のステップ番号 (0 始まり)
    pub step: Option<usize>,
    pub parts: HashMap<NodeId, RigidPose>,
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
