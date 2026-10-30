//! タイミングチャートのデータ (ステップベース / PLC 風)
//!
//! - アクチュエータ (シリンダ・直動軸・回転テーブル) が部品を動かす。
//! - シーケンスはステップの並び。1つのステップには「どのアクチュエータを、どこまで、何秒で」
//!   動かすかの動作 (Action) を複数置ける。同じステップの動作は同時に始まる。
//! - ステップ内の動作がすべて完了し、さらにタイマー (dwell) が経過したら次のステップへ進む。
//!
//! 再生は [`TimingChart::schedule`] で「何秒から何秒まで、どの値へ動くか」の時刻表に
//! 変換してから行う。時刻表は純粋な計算なので、Bevy に依存せずテストできる。

use std::collections::HashMap;

use bevy::prelude::Resource;

/// アクチュエータの種類
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ActuatorKind {
    /// エアシリンダ。値は 0 = 戻り端 / 1 = 出端。移動量は stroke。
    Cylinder,
    /// 直動軸 (サーボ・電動スライダ)。値は原点からの移動量 (モデル単位)。
    LinearAxis,
    /// 回転テーブル・インデックス。値は原点からの角度 (度)。
    RotaryTable,
}

impl ActuatorKind {
    pub const ALL: [ActuatorKind; 3] = [
        ActuatorKind::Cylinder,
        ActuatorKind::LinearAxis,
        ActuatorKind::RotaryTable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ActuatorKind::Cylinder => "シリンダ",
            ActuatorKind::LinearAxis => "直動軸",
            ActuatorKind::RotaryTable => "回転テーブル",
        }
    }

    /// 値の単位 (UI 表示用)
    pub fn unit(self) -> &'static str {
        match self {
            ActuatorKind::Cylinder => "",
            ActuatorKind::LinearAxis => "",
            ActuatorKind::RotaryTable => "°",
        }
    }
}

/// 動く方向 (直動は移動方向、回転は回転軸)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    pub fn label(self) -> &'static str {
        match self {
            Axis::X => "X",
            Axis::Y => "Y",
            Axis::Z => "Z",
        }
    }

    pub fn unit_vector(self) -> [f64; 3] {
        match self {
            Axis::X => [1.0, 0.0, 0.0],
            Axis::Y => [0.0, 1.0, 0.0],
            Axis::Z => [0.0, 0.0, 1.0],
        }
    }
}

/// 動き始めと止まり際の速度の付け方
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Easing {
    /// 一定速度
    Linear,
    /// 加減速あり (S字)
    Smooth,
}

impl Easing {
    pub fn label(self) -> &'static str {
        match self {
            Easing::Linear => "等速",
            Easing::Smooth => "S字加減速",
        }
    }

    /// 0..1 の進み具合を、速度の付け方に合わせた 0..1 に変換する
    pub fn apply(self, x: f64) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match self {
            Easing::Linear => x,
            Easing::Smooth => x * x * (3.0 - 2.0 * x),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Actuator {
    pub id: u32,
    pub name: String,
    pub kind: ActuatorKind,
    pub axis: Axis,
    /// true なら軸のマイナス方向に動く (回転なら逆回り)
    pub reversed: bool,
    /// シリンダのストローク (モデル単位)。シリンダ以外では使わない。
    pub stroke: f64,
    /// 回転の中心 (モデル座標)。回転テーブル以外では使わない。
    pub pivot: [f64; 3],
    /// この装置が載っている親アクチュエータ。親が動くと、この装置ごと一緒に動く。
    pub mounted_on: Option<u32>,
    /// このアクチュエータが動かす部品の id
    pub parts: Vec<u32>,
    pub easing: Easing,
    /// 新しく動作を置いたときの動作時間 (秒)
    pub default_duration: f64,
}

impl Actuator {
    pub fn new(id: u32, kind: ActuatorKind) -> Self {
        let (axis, stroke, default_duration) = match kind {
            ActuatorKind::Cylinder => (Axis::Y, 0.5, 0.4),
            ActuatorKind::LinearAxis => (Axis::X, 0.0, 1.0),
            ActuatorKind::RotaryTable => (Axis::Y, 0.0, 0.8),
        };
        Self {
            id,
            name: format!("{} {id}", kind.label()),
            kind,
            axis,
            reversed: false,
            stroke,
            pivot: [0.0; 3],
            mounted_on: None,
            parts: Vec::new(),
            easing: Easing::Smooth,
            default_duration,
        }
    }

    /// チャート上の値を表示用の文字にする (シリンダは出端 / 戻り端)
    pub fn format_value(&self, value: f64) -> String {
        match self.kind {
            ActuatorKind::Cylinder if value >= 0.5 => "出".into(),
            ActuatorKind::Cylinder => "戻".into(),
            ActuatorKind::LinearAxis => format!("{value:.2}"),
            ActuatorKind::RotaryTable => format!("{value:.0}°"),
        }
    }
}

/// 1つのステップで1つのアクチュエータに出す指令
#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub actuator: u32,
    /// 動かした先の値 (シリンダは 0 / 1、直動は移動量、回転は角度)
    pub target: f64,
    /// 動作にかかる時間 (秒)
    pub duration: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    /// ステップの説明 ("ワーク把持" など)
    pub comment: String,
    pub actions: Vec<Action>,
    /// 動作完了後に待つ時間 (秒)。タイマー待ちや確認時間に使う。
    pub dwell: f64,
}

impl Step {
    pub fn action(&self, actuator: u32) -> Option<&Action> {
        self.actions.iter().find(|a| a.actuator == actuator)
    }

    pub fn action_mut(&mut self, actuator: u32) -> Option<&mut Action> {
        self.actions.iter_mut().find(|a| a.actuator == actuator)
    }

    /// 全動作が完了するまでの時間 + タイマー
    pub fn duration(&self) -> f64 {
        let motion = self
            .actions
            .iter()
            .map(|a| a.duration.max(0.0))
            .fold(0.0, f64::max);
        motion + self.dwell.max(0.0)
    }
}

/// タイミングチャート全体 (アクチュエータ一覧 + ステップの並び)
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct TimingChart {
    pub actuators: Vec<Actuator>,
    pub steps: Vec<Step>,
    pub next_actuator_id: u32,
}

impl TimingChart {
    pub fn add_actuator(&mut self, kind: ActuatorKind) -> u32 {
        self.next_actuator_id = self.next_actuator_id.max(1);
        let id = self.next_actuator_id;
        self.next_actuator_id += 1;
        self.actuators.push(Actuator::new(id, kind));
        id
    }

    pub fn actuator(&self, id: u32) -> Option<&Actuator> {
        self.actuators.iter().find(|a| a.id == id)
    }

    pub fn actuator_mut(&mut self, id: u32) -> Option<&mut Actuator> {
        self.actuators.iter_mut().find(|a| a.id == id)
    }

    /// アクチュエータを消す。そのアクチュエータへの動作と、載せていた子の親設定も外す。
    pub fn remove_actuator(&mut self, id: u32) {
        self.actuators.retain(|a| a.id != id);
        for actuator in &mut self.actuators {
            if actuator.mounted_on == Some(id) {
                actuator.mounted_on = None;
            }
        }
        for step in &mut self.steps {
            step.actions.retain(|a| a.actuator != id);
        }
    }

    /// `id` の親をたどった先に `ancestor` があるか (載せ替えで循環しないための確認)
    pub fn is_mounted_under(&self, id: u32, ancestor: u32) -> bool {
        let mut current = self.actuator(id).and_then(|a| a.mounted_on);
        // 壊れたデータで無限ループしないよう、たどる回数に上限を付ける
        for _ in 0..self.actuators.len() {
            match current {
                Some(parent) if parent == ancestor => return true,
                Some(parent) => current = self.actuator(parent).and_then(|a| a.mounted_on),
                None => return false,
            }
        }
        false
    }

    /// ステップの並びを時刻表に変換する
    pub fn schedule(&self) -> Schedule {
        let mut schedule = Schedule::default();
        let mut time = 0.0;
        for step in &self.steps {
            for action in &step.actions {
                schedule
                    .segments
                    .entry(action.actuator)
                    .or_default()
                    .push(Segment {
                        start: time,
                        duration: action.duration.max(0.0),
                        target: action.target,
                    });
            }
            let end = time + step.duration();
            schedule.steps.push(StepSpan { start: time, end });
            time = end;
        }
        schedule.cycle_time = time;
        schedule
    }
}

/// 1つの動作を時刻で表したもの
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub start: f64,
    pub duration: f64,
    pub target: f64,
}

#[derive(Clone, Copy, Debug)]
pub struct StepSpan {
    pub start: f64,
    pub end: f64,
}

/// ステップを時刻に展開した結果
#[derive(Clone, Debug, Default)]
pub struct Schedule {
    /// アクチュエータごとの動作 (開始時刻の早い順)
    pub segments: HashMap<u32, Vec<Segment>>,
    /// 各ステップの開始・終了時刻
    pub steps: Vec<StepSpan>,
    /// 1サイクルの時間 (秒)
    pub cycle_time: f64,
}

impl Schedule {
    /// 時刻 t でのアクチュエータの値。サイクル開始時は全アクチュエータ 0 (原点) とする。
    pub fn value_at(&self, actuator: &Actuator, t: f64) -> f64 {
        let Some(segments) = self.segments.get(&actuator.id) else {
            return 0.0;
        };
        let mut value = 0.0;
        for segment in segments {
            if t < segment.start {
                break;
            }
            let progress = if segment.duration <= 0.0 {
                1.0
            } else {
                (t - segment.start) / segment.duration
            };
            value += (segment.target - value) * actuator.easing.apply(progress);
        }
        value
    }

    /// サイクル終了時の値 (原点に戻っていないアクチュエータを見つけるのに使う)
    pub fn final_value(&self, actuator: &Actuator) -> f64 {
        self.segments
            .get(&actuator.id)
            .and_then(|s| s.last())
            .map_or(0.0, |s| s.target)
    }

    /// 時刻 t が含まれるステップの番号
    pub fn step_at(&self, t: f64) -> Option<usize> {
        self.steps
            .iter()
            .position(|s| t >= s.start && t < s.end)
            .or_else(|| {
                (!self.steps.is_empty() && t >= self.cycle_time).then(|| self.steps.len() - 1)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chart_with_cylinder() -> (TimingChart, u32) {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(id).unwrap().easing = Easing::Linear;
        (chart, id)
    }

    #[test]
    fn next_step_waits_for_the_slowest_action() {
        let mut chart = TimingChart::default();
        let a = chart.add_actuator(ActuatorKind::Cylinder);
        let b = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.steps.push(Step {
            actions: vec![
                Action {
                    actuator: a,
                    target: 1.0,
                    duration: 0.4,
                },
                Action {
                    actuator: b,
                    target: 2.0,
                    duration: 1.0,
                },
            ],
            dwell: 0.5,
            ..Default::default()
        });
        chart.steps.push(Step {
            actions: vec![Action {
                actuator: a,
                target: 0.0,
                duration: 0.4,
            }],
            ..Default::default()
        });
        let schedule = chart.schedule();
        assert_eq!(schedule.steps[1].start, 1.5);
        assert_eq!(schedule.cycle_time, 1.9);
    }

    #[test]
    fn value_ramps_between_steps() {
        let (mut chart, id) = chart_with_cylinder();
        for target in [1.0, 0.0] {
            chart.steps.push(Step {
                actions: vec![Action {
                    actuator: id,
                    target,
                    duration: 1.0,
                }],
                ..Default::default()
            });
        }
        let schedule = chart.schedule();
        let actuator = chart.actuator(id).unwrap();
        let at = |t| schedule.value_at(actuator, t);
        assert_eq!(at(0.0), 0.0);
        assert!((at(0.5) - 0.5).abs() < 1e-12);
        assert_eq!(at(1.0), 1.0);
        assert!((at(1.25) - 0.75).abs() < 1e-12);
        assert_eq!(at(5.0), 0.0);
    }

    #[test]
    fn mounting_cycles_are_detected() {
        let mut chart = TimingChart::default();
        let a = chart.add_actuator(ActuatorKind::LinearAxis);
        let b = chart.add_actuator(ActuatorKind::Cylinder);
        chart.actuator_mut(b).unwrap().mounted_on = Some(a);
        assert!(chart.is_mounted_under(b, a));
        assert!(!chart.is_mounted_under(a, b));
    }
}
