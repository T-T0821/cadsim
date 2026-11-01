//! 内蔵の模擬 PLC: タイミングチャートのステップを、I/O のやりとりで順に実行する
//!
//! 実機の PLC のステップラダー (SFC) と同じく、
//! 1. 今のステップの動作を出力に書く (シリンダは出 ON/OFF、軸は指令位置)
//! 2. 入力 (出端・戻端・位置決め完了) で全動作の完了を確かめる
//! 3. 完了してからタイマー (dwell) が過ぎたら次のステップへ進む
//!
//! を1スキャンごとに繰り返す。実機がなくても PLC 連携の流れを試せる。
//! 時刻表どおりに動くチャート再生と違い、設備が遅れれば次のステップも待つ。

use super::io::{IoValues, Point};
use plantwright_core::motion::chart::Action;
use plantwright_core::motion::chart::ActuatorKind;
use plantwright_core::motion::chart::TimingChart;

/// 位置決め完了とみなす、現在位置と指令位置の差
const POSITION_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepSequencer {
    /// 運転中なら true。停止中は出力をそのまま保つ (手動で切り替えられる)。
    pub running: bool,
    /// 最後のステップの後、先頭に戻って続ける
    pub looping: bool,
    /// 実行中のステップ番号 (0 始まり)
    pub step: usize,
    /// 今のステップの動作が完了してから経った時間 (秒)。未完了なら None。
    done_for: Option<f64>,
}

impl StepSequencer {
    pub fn new() -> Self {
        Self {
            looping: true,
            ..Default::default()
        }
    }

    /// 先頭のステップに戻して止める
    pub fn reset(&mut self) {
        *self = Self {
            looping: self.looping,
            ..Self::new()
        };
    }

    /// 1スキャン分の処理。`dt` は前回のスキャンからの時間 (秒)。
    pub fn scan(&mut self, chart: &TimingChart, io: &mut IoValues, dt: f64) {
        if !self.running {
            return;
        }
        let Some(step) = chart.steps.get(self.step) else {
            self.reset();
            return;
        };

        for action in &step.actions {
            write_command(chart, action, io);
        }

        let all_done = step.actions.iter().all(|a| action_done(chart, a, io));
        if !all_done {
            self.done_for = None;
            return;
        }
        let elapsed = self.done_for.map_or(0.0, |t| t + dt);
        if elapsed < step.dwell {
            self.done_for = Some(elapsed);
            return;
        }

        self.done_for = None;
        self.step += 1;
        if self.step >= chart.steps.len() {
            self.step = 0;
            self.running = self.looping;
        }
    }
}

/// 動作の指令を出力に書く
fn write_command(chart: &TimingChart, action: &Action, io: &mut IoValues) {
    let Some(actuator) = chart.actuator(action.actuator) else {
        return;
    };
    let id = actuator.id;
    match actuator.kind {
        ActuatorKind::Cylinder => io.set_bit(Point::CylinderExtend(id), action.target >= 0.5),
        ActuatorKind::LinearAxis | ActuatorKind::RotaryTable => {
            io.set_word(Point::AxisTarget(id), action.target)
        }
    }
}

/// 入力を見て、動作が完了しているか確かめる
fn action_done(chart: &TimingChart, action: &Action, io: &IoValues) -> bool {
    let Some(actuator) = chart.actuator(action.actuator) else {
        // 消えたアクチュエータへの動作は待たない
        return true;
    };
    let id = actuator.id;
    match actuator.kind {
        ActuatorKind::Cylinder if action.target >= 0.5 => io.bit(Point::CylinderExtended(id)),
        ActuatorKind::Cylinder => io.bit(Point::CylinderRetracted(id)),
        ActuatorKind::LinearAxis | ActuatorKind::RotaryTable => {
            // 位置決め完了は前の指令位置のものかもしれないので、現在位置も確かめる
            io.bit(Point::AxisInPosition(id))
                && (io.word(Point::AxisPosition(id)) - action.target).abs() <= POSITION_TOLERANCE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::plc::domain::plant::Plant;
    use plantwright_core::motion::chart::Step;

    /// シリンダを出して戻すだけのチャート
    fn out_and_back() -> (TimingChart, u32) {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(id).unwrap().default_duration = 0.5;
        for (target, dwell) in [(1.0, 0.2), (0.0, 0.0)] {
            chart.steps.push(Step {
                actions: vec![Action {
                    actuator: id,
                    target,
                    duration: 0.5,
                }],
                dwell,
                ..Default::default()
            });
        }
        (chart, id)
    }

    /// 模擬 PLC と設備モデルを `seconds` 秒ぶん回す (1スキャン 10ms)
    fn run(
        chart: &TimingChart,
        plc: &mut StepSequencer,
        plant: &mut Plant,
        io: &mut IoValues,
        seconds: f64,
    ) {
        let dt = 0.01;
        for _ in 0..(seconds / dt).round() as usize {
            plant.report(chart, io);
            plc.scan(chart, io, dt);
            plant.take_commands(chart, io);
            plant.advance(chart, dt);
        }
    }

    #[test]
    fn waits_for_end_switch_and_dwell_before_next_step() {
        let (chart, id) = out_and_back();
        let mut plc = StepSequencer::new();
        plc.running = true;
        let mut plant = Plant::default();
        let mut io = IoValues::default();

        run(&chart, &mut plc, &mut plant, &mut io, 0.3);
        assert!(io.bit(Point::CylinderExtend(id)));
        assert_eq!(plc.step, 0, "出端に着くまでは次へ進まない");

        run(&chart, &mut plc, &mut plant, &mut io, 0.35);
        assert_eq!(plc.step, 0, "出端に着いてもタイマーの間は待つ");

        run(&chart, &mut plc, &mut plant, &mut io, 0.1);
        assert_eq!(plc.step, 1);
        assert!(!io.bit(Point::CylinderExtend(id)));
    }

    #[test]
    fn stops_after_last_step_when_not_looping() {
        let (chart, _) = out_and_back();
        let mut plc = StepSequencer::new();
        plc.looping = false;
        plc.running = true;
        let mut plant = Plant::default();
        let mut io = IoValues::default();
        run(&chart, &mut plc, &mut plant, &mut io, 3.0);
        assert!(!plc.running);
        assert_eq!(plc.step, 0);
    }

    #[test]
    fn stalled_actuator_holds_the_sequence() {
        let (chart, id) = out_and_back();
        let mut plc = StepSequencer::new();
        plc.running = true;
        let mut io = IoValues::default();
        // 設備を動かさない (エア切れ・センサー故障の再現)。出端が入らないので進まない。
        for _ in 0..100 {
            plc.scan(&chart, &mut io, 0.01);
        }
        assert_eq!(plc.step, 0);
        assert!(io.bit(Point::CylinderExtend(id)));
    }
}
