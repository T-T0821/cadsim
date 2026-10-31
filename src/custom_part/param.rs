//! 自作部品の公開パラメータ
//!
//! 部品を置いたあとで変えられる値 (シリンダのストローク、動作時間、可動範囲、センサーの検出距離)。
//! どの値もタイミングチャートのアクチュエータ・センサーの項目を指す。Bevy に依存しない。

use std::collections::HashMap;

use crate::motion::{ActuatorKind, TimingChart};

/// パラメータが変える項目。中の数値はアクチュエータ・センサーの id。
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ParamTarget {
    /// シリンダのストローク
    Stroke(u32),
    /// 動作時間 (秒)
    Duration(u32),
    /// 可動範囲の下限
    LimitMin(u32),
    /// 可動範囲の上限
    LimitMax(u32),
    /// センサーの光軸の長さ・検出距離
    SensorRange(u32),
}

/// 公開パラメータ1つ
#[derive(Clone, PartialEq, Debug)]
pub struct PartParam {
    /// 画面に出す名前 (例: "ストローク")
    pub label: String,
    pub target: ParamTarget,
    /// 今の値 (保存した部品では既定値)
    pub value: f64,
}

/// 置いた自作部品のしるし。部品の一番上のノードに付ける。
#[derive(Clone, PartialEq, Debug, Default)]
pub struct CustomInstance {
    /// ライブラリでの部品名
    pub part: String,
    /// この部品のアクチュエータ・センサーの名前の頭 (例: "プッシャー 1/")。
    /// 部品のスクリプトは、頭を付けない名前でこの部品の関節・信号を指せる。
    pub prefix: String,
    pub params: Vec<PartParam>,
}

impl CustomInstance {
    /// `index` 番目のパラメータを `value` にしてチャートに書く。チャートが変わったら true。
    pub fn set(&mut self, chart: &mut TimingChart, index: usize, value: f64) -> bool {
        let Some(param) = self.params.get_mut(index) else {
            return false;
        };
        let changed = param.target.write(chart, value);
        param.value = param.target.read(chart).unwrap_or(value);
        changed
    }
}

impl ParamTarget {
    /// 公開するかどうかを選ぶ画面の既定の名前
    pub fn default_label(self, chart: &TimingChart) -> String {
        let actuator = |id| chart.actuator(id).map_or("?", |a| a.name.as_str());
        let sensor = |id: u32| {
            chart
                .sensors
                .iter()
                .find(|s| s.id == id)
                .map_or("?", |s| s.name.as_str())
        };
        match self {
            ParamTarget::Stroke(id) => format!("{} のストローク", actuator(id)),
            ParamTarget::Duration(id) => format!("{} の動作時間", actuator(id)),
            ParamTarget::LimitMin(id) => format!("{} の下限", actuator(id)),
            ParamTarget::LimitMax(id) => format!("{} の上限", actuator(id)),
            ParamTarget::SensorRange(id) => format!("{} の検出距離", sensor(id)),
        }
    }

    /// 値の単位 (表示用)。長さはモデルの単位 `unit`
    pub fn unit(self, chart: &TimingChart, unit: &str) -> String {
        match self {
            ParamTarget::Duration(_) => "秒".into(),
            ParamTarget::LimitMin(id) | ParamTarget::LimitMax(id)
                if chart
                    .actuator(id)
                    .is_some_and(|a| a.kind == ActuatorKind::RotaryTable) =>
            {
                "°".into()
            }
            _ => unit.into(),
        }
    }

    /// チャートでの今の値。指している項目がなければ None。
    pub fn read(self, chart: &TimingChart) -> Option<f64> {
        match self {
            ParamTarget::Stroke(id) => chart.actuator(id).map(|a| a.stroke),
            ParamTarget::Duration(id) => chart.actuator(id).map(|a| a.default_duration),
            ParamTarget::LimitMin(id) => chart.actuator(id)?.limits.map(|l| l[0]),
            ParamTarget::LimitMax(id) => chart.actuator(id)?.limits.map(|l| l[1]),
            ParamTarget::SensorRange(id) => {
                chart.sensors.iter().find(|s| s.id == id).map(|s| s.range)
            }
        }
    }

    /// チャートに値を書く。項目が変わったら true。
    pub fn write(self, chart: &mut TimingChart, value: f64) -> bool {
        let before = self.read(chart);
        match self {
            ParamTarget::Stroke(id) => {
                if let Some(a) = chart.actuator_mut(id) {
                    a.stroke = value.max(0.0);
                }
            }
            ParamTarget::Duration(id) => {
                if let Some(a) = chart.actuator_mut(id) {
                    a.default_duration = value.max(0.01);
                }
            }
            ParamTarget::LimitMin(id) | ParamTarget::LimitMax(id) => {
                if let Some(limits) = chart.actuator_mut(id).and_then(|a| a.limits.as_mut()) {
                    let i = usize::from(matches!(self, ParamTarget::LimitMax(_)));
                    limits[i] = value;
                    // 下限が上限を超えないようにする
                    if limits[0] > limits[1] {
                        limits[1 - i] = value;
                    }
                }
            }
            ParamTarget::SensorRange(id) => {
                if let Some(s) = chart.sensors.iter_mut().find(|s| s.id == id) {
                    s.range = value.max(0.0);
                }
            }
        }
        self.read(chart) != before
    }

    /// 部品を置いたときの id の付け替え
    pub fn remap(self, actuators: &HashMap<u32, u32>, sensors: &HashMap<u32, u32>) -> Option<Self> {
        let a = |id: u32| actuators.get(&id).copied();
        Some(match self {
            ParamTarget::Stroke(id) => ParamTarget::Stroke(a(id)?),
            ParamTarget::Duration(id) => ParamTarget::Duration(a(id)?),
            ParamTarget::LimitMin(id) => ParamTarget::LimitMin(a(id)?),
            ParamTarget::LimitMax(id) => ParamTarget::LimitMax(a(id)?),
            ParamTarget::SensorRange(id) => ParamTarget::SensorRange(*sensors.get(&id)?),
        })
    }
}

/// 公開できる項目の候補 (アクチュエータ・センサーの並び順)
pub fn candidates(chart: &TimingChart, actuators: &[u32], sensors: &[u32]) -> Vec<ParamTarget> {
    let mut out = Vec::new();
    for a in chart.actuators.iter().filter(|a| actuators.contains(&a.id)) {
        if a.kind == ActuatorKind::Cylinder {
            out.push(ParamTarget::Stroke(a.id));
        }
        out.push(ParamTarget::Duration(a.id));
        if a.limits.is_some() {
            out.push(ParamTarget::LimitMin(a.id));
            out.push(ParamTarget::LimitMax(a.id));
        }
    }
    for s in chart.sensors.iter().filter(|s| sensors.contains(&s.id)) {
        out.push(ParamTarget::SensorRange(s.id));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::SensorKind;

    #[test]
    fn candidates_follow_actuator_kind_and_limits() {
        let mut chart = TimingChart::default();
        let cyl = chart.add_actuator(ActuatorKind::Cylinder);
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.actuator_mut(axis).unwrap().limits = Some([0.0, 100.0]);
        let sensor = chart.add_sensor(SensorKind::Proximity);
        let list = candidates(&chart, &[cyl, axis], &[sensor]);
        assert_eq!(
            list,
            vec![
                ParamTarget::Stroke(cyl),
                ParamTarget::Duration(cyl),
                ParamTarget::Duration(axis),
                ParamTarget::LimitMin(axis),
                ParamTarget::LimitMax(axis),
                ParamTarget::SensorRange(sensor),
            ]
        );
        // 選んでいないものは出さない
        assert_eq!(candidates(&chart, &[], &[]), vec![]);
    }

    #[test]
    fn write_clamps_and_keeps_limits_ordered() {
        let mut chart = TimingChart::default();
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.actuator_mut(axis).unwrap().limits = Some([0.0, 100.0]);
        assert!(ParamTarget::LimitMin(axis).write(&mut chart, 150.0));
        assert_eq!(chart.actuator(axis).unwrap().limits, Some([150.0, 150.0]));
        assert!(ParamTarget::Duration(axis).write(&mut chart, -1.0));
        assert_eq!(chart.actuator(axis).unwrap().default_duration, 0.01);
        assert!(!ParamTarget::Duration(axis).write(&mut chart, 0.01));
    }

    #[test]
    fn remap_drops_targets_that_were_not_copied() {
        let actuators = HashMap::from([(1, 10)]);
        let sensors = HashMap::from([(5, 50)]);
        assert_eq!(
            ParamTarget::Stroke(1).remap(&actuators, &sensors),
            Some(ParamTarget::Stroke(10))
        );
        assert_eq!(
            ParamTarget::SensorRange(5).remap(&actuators, &sensors),
            Some(ParamTarget::SensorRange(50))
        );
        assert_eq!(ParamTarget::Duration(2).remap(&actuators, &sensors), None);
    }
}
