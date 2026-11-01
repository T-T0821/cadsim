//! PLC 連携のユースケース: 1スキャン分の「設備 → 入力 → PLC → 出力 → 設備を動かす」
//!
//! 実機との通信は [`PlcLink`] (ポート) として受け取り、具体的な方式 (Modbus TCP など) は知らない。

use std::collections::HashMap;

use cadsim_core::line::{Direction, Signal, Value};

use super::domain::io::{AddressPlan, IoTable, IoValues, LineSignal, Point, line_points};
use super::domain::io_csv::{self, CsvReport};
use super::domain::log::PlcLog;
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
    /// 記録した PLC のログ (CSV) を流す
    LogReplay,
    /// 部品に付けたスクリプトが出力を決める ([`PlcSession::scan_with`])
    Script,
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
    /// 読み込んだ PLC のログと再生位置
    pub log: Option<LogPlayer>,
    plant: Plant,
    /// I/O 表の後ろに足すライン連携の信号
    line: Vec<LineSignal>,
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
        if let Some(player) = &mut self.log {
            player.time = 0.0;
            player.playing = false;
        }
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
        let table =
            IoTable::from_chart(chart, &self.plan).with_line_signals(&self.line, &self.plan);
        let changed = table != self.table;
        if changed {
            self.table = table;
        }
        changed
    }

    /// ライン連携の信号表を受け取り、設備 → PLC の値を I/O に書く。
    /// 信号の並びが変わったら I/O 表を作り直して true を返す。
    pub fn set_line_signals(&mut self, signals: &[Signal], chart: &TimingChart) -> bool {
        for s in signals {
            match (s.direction, s.value) {
                (Direction::ToPlc, Value::Bit(on)) => {
                    self.io.set_bit(Point::LineInput(s.address), on)
                }
                (Direction::ToPlc, Value::Word(v)) => {
                    self.io.set_word(Point::LineInputWord(s.address), v as f64)
                }
                (Direction::FromPlc, _) => {}
            }
        }
        let line = line_points(signals);
        if line == self.line {
            return false;
        }
        self.line = line;
        self.refresh_table(chart)
    }

    /// PLC が書いたライン連携の PLC → 設備 の信号 (名前, 値)。
    /// 外部の PLC とつないでいるとき、ログを再生しているとき、部品スクリプトのときだけ返す (内蔵の模擬 PLC は
    /// ライン連携の信号を書かないので、そのときは画面での切り替えに任せる)。
    pub fn line_outputs(&self) -> Option<Vec<(String, bool)>> {
        let drives = match self.mode {
            PlcMode::External => self.link.is_some(),
            PlcMode::LogReplay => self.log.is_some(),
            // 部品スクリプトは I/O 表の名前でライン連携の信号も書ける
            PlcMode::Script => true,
            PlcMode::Off | PlcMode::Simulated => false,
        };
        drives.then(|| {
            self.line
                .iter()
                .filter(|(_, p)| matches!(p, Point::LineOutput(_)))
                .map(|(name, p)| (name.clone(), self.io.bit(*p)))
                .collect()
        })
    }

    /// I/O 対応表を CSV の表にする (Excel で編集して読み戻せる)
    pub fn export_io_rows(&self) -> Vec<Vec<String>> {
        io_csv::to_rows(&self.table)
    }

    /// 編集した I/O 対応表 (CSV の表) を読み、番号・スケール・OPC UA ノードを信号ごとに反映する
    pub fn import_io_rows(
        &mut self,
        rows: &[Vec<String>],
        chart: &TimingChart,
    ) -> Result<CsvReport, String> {
        let report = io_csv::apply_rows(rows, &self.table, &mut self.plan)?;
        self.refresh_table(chart);
        Ok(report)
    }

    /// PLC のログ (CSV) を読み込み、先頭から再生できるようにする
    pub fn load_log(&mut self, text: &str) -> Result<&PlcLog, String> {
        let log = PlcLog::parse(text, &self.table)?;
        self.plant.reset();
        self.io = IoValues::default();
        Ok(&self.log.insert(LogPlayer::new(log)).log)
    }

    /// ログの再生位置を `time` 秒に移す。設備は先頭から動かし直して、その時刻の姿にする。
    pub fn seek_log(&mut self, chart: &TimingChart, time: f64) {
        const STEP: f64 = 0.01;
        let Some(player) = &mut self.log else {
            return;
        };
        player.time = time.clamp(0.0, player.log.duration());
        let target = player.time;
        self.plant.reset();
        self.io = IoValues::default();
        let mut t = 0.0;
        while t < target {
            let dt = STEP.min(target - t);
            player.log.apply_at(t, &mut self.io);
            self.plant.take_commands(chart, &self.io);
            self.plant.advance(chart, dt);
            t += dt;
        }
        self.plant.report(chart, &mut self.io);
    }

    /// 1スキャン分を進め、アクチュエータの現在値 (id → 値) を返す。`dt` は前回からの秒数。
    pub fn scan(&mut self, chart: &TimingChart, dt: f64) -> &HashMap<u32, f64> {
        self.plant.report(chart, &mut self.io);
        // 設備を動かす時間。ログ再生では再生速度を掛け、止めている間は動かさない。
        let mut plant_dt = dt;
        match self.mode {
            PlcMode::Off | PlcMode::Script => {}
            PlcMode::Simulated => self.sequencer.scan(chart, &mut self.io, dt),
            PlcMode::External => {
                if let Some(link) = &mut self.link {
                    link.exchange(&self.table, &mut self.io);
                }
            }
            PlcMode::LogReplay => match &mut self.log {
                Some(player) => {
                    plant_dt = player.advance(dt);
                    player.log.apply_at(player.time, &mut self.io);
                }
                None => plant_dt = 0.0,
            },
        }
        self.plant.take_commands(chart, &self.io);
        self.plant.advance(chart, plant_dt);
        self.plant.values()
    }

    /// PLC の代わりに `control` が出力を決める1スキャン (部品スクリプト)。
    /// `control` は I/O 一覧・入出力の値・アクチュエータの現在値を受け取り、出力を書く。
    pub fn scan_with(
        &mut self,
        chart: &TimingChart,
        dt: f64,
        control: impl FnOnce(&IoTable, &mut IoValues, &HashMap<u32, f64>),
    ) -> &HashMap<u32, f64> {
        self.plant.report(chart, &mut self.io);
        control(&self.table, &mut self.io, self.plant.values());
        self.plant.take_commands(chart, &self.io);
        self.plant.advance(chart, dt);
        self.plant.values()
    }

    /// 設備を原点に戻し、出力をすべて OFF にする (部品スクリプトを最初から動かすとき)
    pub fn reset_plant(&mut self) {
        self.plant.reset();
        self.io = IoValues::default();
    }

    /// 模擬 PLC が運転中なら、実行中のステップ番号
    pub fn running_step(&self) -> Option<usize> {
        (self.mode == PlcMode::Simulated && self.sequencer.running).then_some(self.sequencer.step)
    }
}

/// ログの再生位置と再生状態
#[derive(Clone, Debug, PartialEq)]
pub struct LogPlayer {
    pub log: PlcLog,
    /// ログの先頭からの秒数
    pub time: f64,
    pub playing: bool,
    /// 再生速度の倍率
    pub speed: f64,
    pub looping: bool,
}

impl LogPlayer {
    fn new(log: PlcLog) -> Self {
        Self {
            log,
            time: 0.0,
            playing: false,
            speed: 1.0,
            looping: false,
        }
    }

    /// `dt` 秒ぶん再生位置を進め、設備を動かす時間を返す。最後まで行ったら止めるか先頭へ戻る。
    fn advance(&mut self, dt: f64) -> f64 {
        if !self.playing {
            return 0.0;
        }
        let step = dt * self.speed;
        self.time += step;
        let end = self.log.duration();
        if self.time >= end {
            if self.looping && end > 0.0 {
                self.time %= end;
            } else {
                self.time = end;
                self.playing = false;
            }
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::Step;
    use crate::motion::{Action, ActuatorKind};
    use crate::plc::domain::io::Point;
    use cadsim_core::flow::model::sample_line;
    use cadsim_core::line::{LineInputs, SignalSources, line_signals};

    /// 外部 PLC のふり: ライン連携の「運転」を OFF にし、届いた「排出数」を覚える
    struct LinePlc {
        run: u16,
        output: u16,
        seen_output: std::sync::Arc<std::sync::Mutex<f64>>,
    }

    impl PlcLink for LinePlc {
        fn exchange(&mut self, _table: &IoTable, io: &mut IoValues) {
            *self.seen_output.lock().unwrap() = io.word(Point::LineInputWord(self.output));
            io.set_bit(Point::LineOutput(self.run), false);
        }
        fn status(&self) -> LinkStatus {
            LinkStatus::Connected { cycle_ms: 0.0 }
        }
    }

    #[test]
    fn line_signals_join_the_io_table_and_are_exchanged_with_the_plc() {
        let model = sample_line();
        let chart = TimingChart::default();
        let signals = line_signals(
            &model,
            None,
            &SignalSources::default(),
            &LineInputs::default(),
        );
        let address = |name: &str| signals.iter().find(|s| s.name == name).unwrap().address;
        let mut session = PlcSession::default();
        assert!(session.set_line_signals(&signals, &chart));
        assert!(!session.set_line_signals(&signals, &chart));
        assert_eq!(session.table.points.len(), signals.len());
        assert!(session.table.address_conflicts().is_empty());

        // 模擬 PLC ではライン連携の出力は画面に任せる
        session.set_mode(PlcMode::Simulated);
        assert_eq!(session.line_outputs(), None);

        session.set_mode(PlcMode::External);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(-1.0));
        session.connect(Box::new(LinePlc {
            run: address("ライン.運転"),
            output: address("ライン.排出数"),
            seen_output: seen.clone(),
        }));
        session.set_line_signals(&signals, &chart);
        session.scan(&chart, 0.01);
        assert_eq!(*seen.lock().unwrap(), 0.0);
        let outputs = session.line_outputs().unwrap();
        assert!(outputs.contains(&("ライン.運転".to_string(), false)));
    }

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
    fn log_replay_moves_the_actuator_and_seeks() {
        let (chart, id) = one_cylinder();
        let mut session = PlcSession::default();
        session.refresh_table(&chart);
        session.set_mode(PlcMode::LogReplay);
        let name = session
            .table
            .get(Point::CylinderExtend(id))
            .unwrap()
            .name
            .clone();
        session
            .load_log(&format!("Time,{name}\n0,0\n0.1,1\n2.0,1\n"))
            .unwrap();

        // 止めている間は動かない
        assert_eq!(session.scan(&chart, 0.5)[&id], 0.0);
        session.log.as_mut().unwrap().playing = true;
        for _ in 0..40 {
            session.scan(&chart, 0.02);
        }
        assert_eq!(session.scan(&chart, 0.0)[&id], 1.0);

        // 先頭近くへ戻すと、設備もその時刻の姿に戻る
        session.seek_log(&chart, 0.05);
        assert_eq!(session.scan(&chart, 0.0)[&id], 0.0);
        session.seek_log(&chart, 1.0);
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
