//! PLC 連携のユースケース: 1スキャン分の「設備 → 入力 → PLC → 出力 → 設備を動かす」
//!
//! 実機との通信は [`PlcLink`] (ポート) として受け取り、具体的な方式 (Modbus TCP など) は知らない。

use std::collections::HashMap;

use super::domain::io::{AddressPlan, IoTable, IoValues};
use super::domain::plant::Plant;
use super::domain::sequencer::StepSequencer;
use crate::motion::TimingChart;

/// 何がアクチュエータを動かすか
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PlcMode {
    /// PLC 連携なし (タイミングチャートの時刻表で再生)
    #[default]
    Off,
    /// 内蔵の模擬 PLC
    Simulated,
    /// 通信でつないだ外部の PLC
    External,
}

/// 外部の PLC との通信方式 (ポート)。
/// Modbus TCP のほか、OPC UA などを足すときもこれを実装すれば残りはそのまま使える。
pub trait PlcLink: Send + Sync {
    /// 入力 (設備 → PLC) を渡し、届いている出力 (PLC → 設備) を `io` に書く。
    /// 通信は別スレッドで行い、ここでは待たない。
    fn exchange(&mut self, table: &IoTable, io: &mut IoValues);
    fn status(&self) -> LinkStatus;
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum LinkStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected {
        /// 1回の読み書きにかかった時間
        cycle_ms: f64,
    },
    Error(String),
}

/// PLC 連携の状態一式
#[derive(Default)]
pub struct PlcSession {
    pub mode: PlcMode,
    pub plan: AddressPlan,
    pub table: IoTable,
    pub io: IoValues,
    pub sequencer: StepSequencer,
    plant: Plant,
    /// 外部の PLC とつないでいる間だけ Some
    link: Option<Box<dyn PlcLink>>,
}

impl PlcSession {
    pub fn is_active(&self) -> bool {
        self.mode != PlcMode::Off
    }

    /// モードを切り替える。どのモードに入るときも設備は原点から始める。
    pub fn set_mode(&mut self, mode: PlcMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.link = None;
        self.plant.reset();
        self.io = IoValues::default();
        self.sequencer = StepSequencer::new();
    }

    pub fn connect(&mut self, link: Box<dyn PlcLink>) {
        self.link = Some(link);
    }

    pub fn disconnect(&mut self) {
        self.link = None;
        self.io.clear_outputs();
    }

    pub fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    pub fn link_status(&self) -> LinkStatus {
        self.link
            .as_ref()
            .map_or(LinkStatus::Disconnected, |l| l.status())
    }

    /// 模擬 PLC を先頭のステップに戻し、出力をすべて OFF にする
    pub fn reset_sequence(&mut self) {
        self.sequencer.reset();
        self.io.clear_outputs();
    }

    /// アクチュエータの増減や番号の割り付けに合わせて I/O 一覧を作り直す。変わったら true。
    pub fn refresh_table(&mut self, chart: &TimingChart) -> bool {
        let table = IoTable::from_chart(chart, &self.plan);
        let changed = table != self.table;
        if changed {
            self.table = table;
        }
        changed
    }

    /// 1スキャン分を進め、アクチュエータの現在値 (id → 値) を返す。`dt` は前回からの秒数。
    pub fn scan(&mut self, chart: &TimingChart, dt: f64) -> &HashMap<u32, f64> {
        self.plant.report(chart, &mut self.io);
        match self.mode {
            PlcMode::Off => {}
            PlcMode::Simulated => self.sequencer.scan(chart, &mut self.io, dt),
            PlcMode::External => {
                if let Some(link) = &mut self.link {
                    link.exchange(&self.table, &mut self.io);
                }
            }
        }
        self.plant.take_commands(chart, &self.io);
        self.plant.advance(chart, dt);
        self.plant.values()
    }

    /// 模擬 PLC が運転中なら、実行中のステップ番号
    pub fn running_step(&self) -> Option<usize> {
        (self.mode == PlcMode::Simulated && self.sequencer.running).then_some(self.sequencer.step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::Step;
    use crate::motion::{Action, ActuatorKind};
    use crate::plc::domain::io::Point;

    /// 出力をそのまま返すだけの外部 PLC (シリンダを出し続ける)
    struct AlwaysExtend(u32);

    impl PlcLink for AlwaysExtend {
        fn exchange(&mut self, _table: &IoTable, io: &mut IoValues) {
            io.set_bit(Point::CylinderExtend(self.0), true);
        }
        fn status(&self) -> LinkStatus {
            LinkStatus::Connected { cycle_ms: 0.0 }
        }
    }

    fn one_cylinder() -> (TimingChart, u32) {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(id).unwrap().default_duration = 0.5;
        chart.steps.push(Step {
            actions: vec![Action {
                actuator: id,
                target: 1.0,
                duration: 0.5,
            }],
            ..Default::default()
        });
        (chart, id)
    }

    #[test]
    fn external_plc_output_moves_the_actuator() {
        let (chart, id) = one_cylinder();
        let mut session = PlcSession::default();
        session.set_mode(PlcMode::External);
        session.refresh_table(&chart);
        session.connect(Box::new(AlwaysExtend(id)));
        for _ in 0..30 {
            session.scan(&chart, 0.02);
        }
        assert_eq!(session.scan(&chart, 0.02)[&id], 1.0);
        assert!(session.io.bit(Point::CylinderExtended(id)));
    }

    #[test]
    fn switching_mode_returns_plant_to_origin() {
        let (chart, id) = one_cylinder();
        let mut session = PlcSession::default();
        session.set_mode(PlcMode::Simulated);
        session.sequencer.running = true;
        for _ in 0..10 {
            session.scan(&chart, 0.02);
        }
        assert!(session.scan(&chart, 0.0)[&id] > 0.0);
        session.set_mode(PlcMode::Off);
        assert!(session.scan(&chart, 0.0)[&id] == 0.0);
    }
}
