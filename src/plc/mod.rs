//! PLC 連携 (仮想立ち上げ)
//!
//! タイミングチャートの時刻表の代わりに、PLC の出力でアクチュエータを動かし、
//! 端のスイッチや現在位置を PLC の入力に返す。層の分け方は docs/architecture.md に合わせている。
//!
//! - `domain`: I/O の割り付け表、設備モデル、内蔵の模擬 PLC (Bevy なし)
//! - `usecase`: 1スキャン分の処理と、外部 PLC との通信のポート [`usecase::PlcLink`] (Bevy なし)
//! - `adapter`: 通信方式の実装 (Modbus TCP、OPC UA)
//! - このファイルと `ui`: Bevy のシステムと egui の画面
//!
//! 部品の位置はチャート再生と同じ `SimulationPose` に書くので、表示や干渉チェックはそのまま使える。

mod adapter;
mod domain;
mod ui;
mod usecase;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use crate::history::{TrackDocExt, TrackedDoc};
use crate::motion::{
    MotionSet, SensorStates, SimulationPose, TimingChart, actuator_poses_for, part_poses_for,
};
use crate::scripting::ScriptRun;
pub use adapter::record::PlcRecord;
use adapter::record::PlcSettings;
pub use domain::io::{AddressPlan, IoTable, IoValues, Point};
#[cfg(test)]
pub use domain::plant::Plant;
use usecase::{PlcMode, PlcSession};

pub struct PlcPlugin;

impl Plugin for PlcPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Plc>()
            .track_doc::<PlcPlanDoc>()
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
    protocol: Protocol,
    modbus: adapter::modbus::ModbusSettings,
    opcua: adapter::opcua::OpcUaSettings,
    /// 画面に出すお知らせ (CSV の読み込み結果など)
    notice: Option<String>,
}

impl Plc {
    /// プロジェクトファイルに書く設定 (I/O の割り付けと外部 PLC の接続先)。初期値のままなら None
    pub fn record(&self) -> Option<PlcRecord> {
        adapter::record::to_record(&PlcSettings {
            plan: self.session.plan.clone(),
            use_opcua: self.protocol == Protocol::OpcUa,
            modbus: self.modbus.clone(),
            opcua: self.opcua.clone(),
        })
    }

    /// 開いた (または新規の) プロジェクトの設定に置き換える。PLC 連携は止める。
    pub fn restore(&mut self, record: Option<&PlcRecord>) {
        let settings = adapter::record::from_record(record);
        self.session.set_mode(PlcMode::Off);
        self.session.plan = settings.plan;
        self.protocol = if settings.use_opcua {
            Protocol::OpcUa
        } else {
            Protocol::ModbusTcp
        };
        self.modbus = settings.modbus;
        self.opcua = settings.opcua;
        self.notice = None;
    }
}

/// I/O の割り付けの編集 (先頭番号の変更、CSV の読み込み) を元に戻す履歴に載せる
struct PlcPlanDoc;

impl TrackedDoc for PlcPlanDoc {
    type Value = AddressPlan;
    const LABEL: &'static str = "PLC の I/O 割り付けを編集";

    fn read(world: &World) -> AddressPlan {
        world.resource::<Plc>().session.plan.clone()
    }

    fn write(world: &mut World, value: AddressPlan) {
        world.resource_mut::<Plc>().session.plan = value;
    }

    fn same(a: &AddressPlan, b: &AddressPlan) -> bool {
        a == b
    }
}

impl Plc {
    /// ライン連携の信号を I/O 表に載せて設備 → PLC の値を書き、PLC が書いた
    /// PLC → 設備 の信号を返す (PLC が書いていないモードなら None)。
    /// 1つ目の戻り値は I/O 表が作り直されたか。
    pub(crate) fn exchange_line(
        &mut self,
        signals: &[cadsim_core::line::Signal],
        chart: &TimingChart,
    ) -> (bool, Option<Vec<(String, bool)>>) {
        let changed = self.session.set_line_signals(signals, chart);
        (changed, self.session.line_outputs())
    }
}

/// 外部の PLC との通信方式
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Protocol {
    #[default]
    ModbusTcp,
    OpcUa,
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
    sensors: Res<SensorStates>,
    mut scripts: ResMut<ScriptRun>,
    mut plc: ResMut<Plc>,
    mut pose: ResMut<SimulationPose>,
) {
    let dt = time.delta_secs_f64();
    let values = if plc.session.mode == PlcMode::Script {
        // 部品スクリプトが PLC の代わりに出力を決める。止めている間は出力をそのまま保つ。
        let sensors: Vec<(String, bool)> = sensors
            .signals
            .iter()
            .map(|s| (s.name.clone(), s.on))
            .collect();
        let controller = scripts.running_mut();
        plc.session.scan_with(&chart, dt, |table, io, values| {
            if let Some(c) = controller {
                c.scan(&chart, table, io, values, &sensors, dt);
            }
        })
    } else {
        plc.session.scan(&chart, dt)
    };
    // ロボットの表示 (TCP・骨格) も PLC の値に合わせるため、関節の値と変換も入れる
    pose.actuators = actuator_poses_for(&chart, values);
    pose.parts = part_poses_for(&chart, &pose.actuators);
    pose.values = values.clone();
    pose.time = 0.0;
    pose.step = plc.session.running_step();
}
