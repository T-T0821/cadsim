//! PLC 連携 (仮想立ち上げ)
//!
//! タイミングチャートの時刻表の代わりに、PLC の出力でアクチュエータを動かし、
//! 端のスイッチや現在位置を PLC の入力に返す。層の分け方は docs/architecture.md に合わせている。
//!
//! - `domain`: I/O の割り付け表、設備モデル、内蔵の模擬 PLC (Bevy なし)
//! - `usecase`: 1スキャン分の処理と、外部 PLC との通信のポート [`usecase::PlcLink`] (Bevy なし)
//! - `adapter`: 通信方式の実装 (Modbus TCP)
//! - このファイルと `ui`: Bevy のシステムと egui の画面
//!
//! 部品の位置はチャート再生と同じ `SimulationPose` に書くので、表示や干渉チェックはそのまま使える。

mod adapter;
mod domain;
mod ui;
mod usecase;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use crate::motion::{MotionSet, SimulationPose, TimingChart, part_poses_from_values};
use usecase::PlcSession;

pub struct PlcPlugin;

impl Plugin for PlcPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Plc>()
            .add_systems(EguiPrimaryContextPass, ui::plc_window)
            .add_systems(
                Update,
                (update_io_table, run_plc.run_if(plc_active))
                    .chain()
                    .in_set(MotionSet::Drive),
            );
    }
}

/// PLC 連携の状態 (ユースケースの [`PlcSession`] と、画面だけが使う設定)
#[derive(Resource, Default)]
pub struct Plc {
    session: PlcSession,
    modbus: adapter::modbus::ModbusSettings,
}

/// PLC がアクチュエータを動かしているとき true。
/// このあいだはチャートの時刻表での再生と、ギズモでの編集を止める。
pub fn plc_active(plc: Res<Plc>) -> bool {
    plc.session.is_active()
}

fn update_io_table(chart: Res<TimingChart>, mut plc: ResMut<Plc>) {
    // 変わらないフレームで Plc を「変更あり」にしないよう、先に比べる
    if plc.bypass_change_detection().session.refresh_table(&chart) {
        plc.set_changed();
    }
}

fn run_plc(
    time: Res<Time>,
    chart: Res<TimingChart>,
    mut plc: ResMut<Plc>,
    mut pose: ResMut<SimulationPose>,
) {
    let values = plc.session.scan(&chart, time.delta_secs_f64());
    pose.parts = part_poses_from_values(&chart, values);
    pose.time = 0.0;
    pose.step = plc.session.running_step();
}
