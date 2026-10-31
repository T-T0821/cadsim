//! 設備側のモデル: PLC の出力でアクチュエータを動かし、センサーの状態を PLC の入力に返す
//!
//! 実機のシリンダやサーボの代わりに、指令された位置へ一定速度で近づく単純なモデルを使う。
//! 速度はタイミングチャートの動作時間から求めるので、チャート再生と同じテンポで動く。

use std::collections::HashMap;

use super::io::{IoValues, Point};
use crate::motion::{Actuator, ActuatorKind, TimingChart};

/// 位置が一致しているとみなす許容差 (シリンダは 0..1、直動はモデル単位、回転は度)
const TOLERANCE: f64 = 1e-6;

/// 設備側の状態 (アクチュエータごとの現在値と指令値)
#[derive(Clone, Debug, Default)]
pub struct Plant {
    /// 現在値。シリンダは 0 = 戻端 / 1 = 出端、直動は移動量、回転は角度。
    values: HashMap<u32, f64>,
    targets: HashMap<u32, f64>,
}

impl Plant {
    /// アクチュエータの現在値 (id → 値)。部品の位置の計算に使う。
    pub fn values(&self) -> &HashMap<u32, f64> {
        &self.values
    }

    /// すべてのアクチュエータを原点に戻す
    pub fn reset(&mut self) {
        self.values.clear();
        self.targets.clear();
    }

    /// PLC の出力を指令値として受け取る
    pub fn take_commands(&mut self, chart: &TimingChart, io: &IoValues) {
        for actuator in &chart.actuators {
            let id = actuator.id;
            let target = match actuator.kind {
                ActuatorKind::Cylinder => {
                    if io.bit(Point::CylinderExtend(id)) {
                        1.0
                    } else {
                        0.0
                    }
                }
                ActuatorKind::LinearAxis | ActuatorKind::RotaryTable => {
                    io.word(Point::AxisTarget(id))
                }
            };
            self.targets.insert(id, target);
        }
    }

    /// `dt` 秒ぶん、各アクチュエータを指令値へ近づける
    pub fn advance(&mut self, chart: &TimingChart, dt: f64) {
        for actuator in &chart.actuators {
            let target = self.targets.get(&actuator.id).copied().unwrap_or(0.0);
            let value = self.values.entry(actuator.id).or_insert(0.0);
            let max_step = travel_speed(chart, actuator) * dt;
            *value += (target - *value).clamp(-max_step, max_step);
        }
    }

    /// 現在値から、PLC の入力 (端のスイッチ・現在位置) を作る
    pub fn report(&self, chart: &TimingChart, io: &mut IoValues) {
        for actuator in &chart.actuators {
            let id = actuator.id;
            let value = self.values.get(&id).copied().unwrap_or(0.0);
            match actuator.kind {
                ActuatorKind::Cylinder => {
                    io.set_bit(Point::CylinderExtended(id), value >= 1.0 - TOLERANCE);
                    io.set_bit(Point::CylinderRetracted(id), value <= TOLERANCE);
                }
                ActuatorKind::LinearAxis | ActuatorKind::RotaryTable => {
                    let target = self.targets.get(&id).copied().unwrap_or(0.0);
                    io.set_word(Point::AxisPosition(id), value);
                    io.set_bit(
                        Point::AxisInPosition(id),
                        (value - target).abs() <= TOLERANCE,
                    );
                }
            }
        }
    }
}

/// アクチュエータの速度 (値 / 秒)。
/// シリンダは「出端まで default_duration 秒」、直動・回転はチャート内で一番速い動作に合わせる。
pub fn travel_speed(chart: &TimingChart, actuator: &Actuator) -> f64 {
    let fallback = 1.0 / actuator.default_duration.max(0.01);
    if actuator.kind == ActuatorKind::Cylinder {
        return fallback;
    }
    let mut value = 0.0;
    let mut fastest: f64 = 0.0;
    for step in &chart.steps {
        if let Some(action) = step.action(actuator.id) {
            if action.duration > 0.0 {
                fastest = fastest.max((action.target - value).abs() / action.duration);
            }
            value = action.target;
        }
    }
    if fastest > 0.0 { fastest } else { fallback }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Action, chart::Step};

    #[test]
    fn cylinder_reaches_end_switch_after_its_duration() {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(id).unwrap().default_duration = 0.5;
        let mut plant = Plant::default();
        let mut io = IoValues::default();
        io.set_bit(Point::CylinderExtend(id), true);
        plant.take_commands(&chart, &io);

        plant.advance(&chart, 0.25);
        plant.report(&chart, &mut io);
        assert!(!io.bit(Point::CylinderExtended(id)));
        assert!(!io.bit(Point::CylinderRetracted(id)));

        plant.advance(&chart, 0.25);
        plant.report(&chart, &mut io);
        assert!(io.bit(Point::CylinderExtended(id)));
    }

    #[test]
    fn axis_speed_follows_the_chart() {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.steps.push(Step {
            actions: vec![Action {
                actuator: id,
                target: 4.0,
                duration: 2.0,
            }],
            ..Default::default()
        });
        let actuator = chart.actuator(id).unwrap();
        assert_eq!(travel_speed(&chart, actuator), 2.0);

        let mut plant = Plant::default();
        let mut io = IoValues::default();
        io.set_word(Point::AxisTarget(id), 4.0);
        plant.take_commands(&chart, &io);
        plant.advance(&chart, 1.0);
        plant.report(&chart, &mut io);
        assert_eq!(io.word(Point::AxisPosition(id)), 2.0);
        assert!(!io.bit(Point::AxisInPosition(id)));
        plant.advance(&chart, 1.5);
        plant.report(&chart, &mut io);
        assert_eq!(io.word(Point::AxisPosition(id)), 4.0);
        assert!(io.bit(Point::AxisInPosition(id)));
    }
}
