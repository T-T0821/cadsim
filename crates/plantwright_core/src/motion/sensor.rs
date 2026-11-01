//! センサー (光電センサー・近接センサー) と、その検知判定
//!
//! - 光電センサー: 投光位置から決まった向きに光軸 (線分) を出し、光軸が物体の
//!   直方体と交わったら ON (透過形で遮光されたら ON、と考える)。
//! - 近接センサー: 検出面の中心から検出距離以内に物体があれば ON。
//!
//! 検知対象は [`TargetBox`] (向きを持つ直方体) で渡す。部品・ワークどちらも同じ形にできる。
//! センサーはアクチュエータに載せられ、載せた先と一緒に動く。
//! そのとき、同じアクチュエータが動かす部品 (センサーを取り付けている部品) は検知しない。

use std::collections::HashMap;

use glam::{DQuat, DVec3};

use super::chart::{Axis, Interlock, Schedule, Step, TimingChart};
use super::pose::{RigidPose, actuator_poses};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SensorKind {
    /// 光電センサー (光軸を遮ると ON)
    Photoelectric,
    /// 近接センサー (検出距離以内に物体があると ON)
    Proximity,
}

impl SensorKind {
    pub const ALL: [SensorKind; 2] = [SensorKind::Photoelectric, SensorKind::Proximity];

    pub fn label(self) -> &'static str {
        match self {
            SensorKind::Photoelectric => "光電センサー",
            SensorKind::Proximity => "近接センサー",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    pub id: u32,
    pub name: String,
    pub kind: SensorKind,
    /// 投光位置 (光電) / 検出面の中心 (近接)。モデル座標で、載せ先が原点にあるときの位置。
    pub position: [f64; 3],
    /// 光軸の向き (光電センサーだけ使う)
    pub axis: Axis,
    /// true なら光軸を軸のマイナス方向に出す
    pub reversed: bool,
    /// 光軸の長さ (光電) / 検出距離 (近接)
    pub range: f64,
    /// このセンサーを取り付けたアクチュエータ。そのアクチュエータと一緒に動く。
    pub mounted_on: Option<u32>,
    /// 強制 ON / OFF (動作確認用)。`None` なら検知結果どおり。
    pub force: Option<bool>,
}

impl Sensor {
    pub fn new(id: u32, kind: SensorKind) -> Self {
        // 既定値は mm で考えた長さ (光軸 100 mm、検出距離 20 mm)
        let range = match kind {
            SensorKind::Photoelectric => 100.0,
            SensorKind::Proximity => 20.0,
        };
        Self {
            id,
            name: format!("{} {id}", kind.label()),
            kind,
            position: [0.0; 3],
            axis: Axis::X,
            reversed: false,
            range,
            mounted_on: None,
            force: None,
        }
    }

    /// 載せ先の動き `mount` を受けたときの、投光位置 (検出面の中心) と光軸の向き
    pub fn placement(&self, mount: RigidPose) -> (DVec3, DVec3) {
        let mut direction = DVec3::from_array(self.axis.unit_vector());
        if self.reversed {
            direction = -direction;
        }
        (
            mount.transform_point(DVec3::from_array(self.position)),
            mount.rotation * direction,
        )
    }

    /// 物体が検知範囲に入っているか (強制 ON / OFF は見ない)
    pub fn detects(&self, mount: RigidPose, target: &TargetBox) -> bool {
        let (origin, direction) = self.placement(mount);
        match self.kind {
            SensorKind::Photoelectric => {
                target.intersects_segment(origin, origin + direction * self.range)
            }
            SensorKind::Proximity => target.distance_to(origin) <= self.range,
        }
    }
}

/// 検知対象の直方体 (中心・向き・各軸方向の半分の寸法)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TargetBox {
    /// 部品の id (ワークなど部品でないものは任意の値)
    pub id: u32,
    pub center: DVec3,
    pub rotation: DQuat,
    pub half_size: DVec3,
}

impl TargetBox {
    /// 動きを加えた後の直方体
    pub fn moved(&self, pose: RigidPose) -> TargetBox {
        TargetBox {
            center: pose.transform_point(self.center),
            rotation: pose.rotation * self.rotation,
            ..*self
        }
    }

    /// ワールド座標の点を、直方体の中心・向きを基準にした座標に直す
    fn local_point(&self, p: DVec3) -> DVec3 {
        self.rotation.inverse() * (p - self.center)
    }

    /// 点から直方体 (の表面または内部) までの距離。内部なら 0。
    pub fn distance_to(&self, p: DVec3) -> f64 {
        let local = self.local_point(p);
        let outside = (local.abs() - self.half_size).max(DVec3::ZERO);
        outside.length()
    }

    /// 線分 a-b が直方体と交わるか (スラブ法)
    pub fn intersects_segment(&self, a: DVec3, b: DVec3) -> bool {
        let start = self.local_point(a);
        let delta = self.local_point(b) - start;
        let (mut enter, mut exit) = (0.0f64, 1.0f64);
        for axis in 0..3 {
            let (s, d, h) = (start[axis], delta[axis], self.half_size[axis]);
            if d.abs() < 1e-12 {
                // この軸に平行: 板の外にあれば交わらない
                if s.abs() > h {
                    return false;
                }
                continue;
            }
            let t1 = (-h - s) / d;
            let t2 = (h - s) / d;
            enter = enter.max(t1.min(t2));
            exit = exit.min(t1.max(t2));
            if enter > exit {
                return false;
            }
        }
        true
    }
}

/// 設計位置の物体。`actuator` が動かしていれば、その動きと一緒に動く。
#[derive(Clone, Copy, Debug)]
pub struct DesignTarget {
    pub shape: TargetBox,
    pub actuator: Option<u32>,
}

/// 時刻 t における全センサーの ON / OFF (チャートのセンサー順)。
///
/// `targets` は設計位置の部品 (アクチュエータに付いていれば一緒に動かす)、
/// `extra` はすでにワールド位置になっている物体 (コンベア上のワークなど)。
pub fn sensor_states_at(
    chart: &TimingChart,
    schedule: &Schedule,
    t: f64,
    targets: &[DesignTarget],
    extra: &[TargetBox],
) -> Vec<bool> {
    sensor_states(chart, &actuator_poses(chart, schedule, t), targets, extra)
}

/// アクチュエータの動き (`poses`: アクチュエータ id → 親の動きも含めた変換) が
/// わかっているときの、全センサーの ON / OFF。PLC が位置を決めているときに使う。
pub fn sensor_states(
    chart: &TimingChart,
    poses: &HashMap<u32, RigidPose>,
    targets: &[DesignTarget],
    extra: &[TargetBox],
) -> Vec<bool> {
    let pose_of = |actuator: Option<u32>| {
        actuator
            .and_then(|id| poses.get(&id).copied())
            .unwrap_or_default()
    };
    let moved: Vec<(TargetBox, Option<u32>)> = targets
        .iter()
        .map(|target| {
            (
                target.shape.moved(pose_of(target.actuator)),
                target.actuator,
            )
        })
        .collect();

    chart
        .sensors
        .iter()
        .map(|sensor| {
            if let Some(forced) = sensor.force {
                return forced;
            }
            let mount = pose_of(sensor.mounted_on);
            let on_own_mount = |actuator: Option<u32>| {
                sensor.mounted_on.is_some() && actuator == sensor.mounted_on
            };
            moved
                .iter()
                .filter(|(_, actuator)| !on_own_mount(*actuator))
                .map(|(shape, _)| shape)
                .chain(extra)
                .any(|shape| sensor.detects(mount, shape))
        })
        .collect()
}

/// インターロック付きでチャートを時刻表に変換する。
/// `targets_at(途中までの時刻表, t)` で時刻 t の検知対象を返す。
pub fn schedule_with_sensors(
    chart: &TimingChart,
    mut targets_at: impl FnMut(&Schedule, f64) -> (Vec<DesignTarget>, Vec<TargetBox>),
) -> Schedule {
    chart.schedule_with(|schedule, t, step: &Step| {
        let (targets, extra) = targets_at(schedule, t);
        let states = sensor_states_at(chart, schedule, t, &targets, &extra);
        unmet_interlocks(chart, step, &states)
    })
}

/// ステップの開始条件のうち、`states` (チャートのセンサー順の ON / OFF) で成立していないもの。
/// 消えたセンサーを指す条件は成立しないものとする。
pub fn unmet_interlocks(chart: &TimingChart, step: &Step, states: &[bool]) -> Vec<Interlock> {
    step.interlocks
        .iter()
        .copied()
        .filter(|interlock| {
            let state = chart
                .sensors
                .iter()
                .position(|s| s.id == interlock.sensor)
                .map(|index| states[index]);
            state != Some(interlock.on)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{Action, ActuatorKind, Easing};

    fn unit_box(center: DVec3) -> TargetBox {
        TargetBox {
            id: 1,
            center,
            rotation: DQuat::IDENTITY,
            half_size: DVec3::splat(0.5),
        }
    }

    #[test]
    fn beam_hits_box_only_when_it_reaches() {
        let target = unit_box(DVec3::new(2.0, 0.0, 0.0));
        assert!(target.intersects_segment(DVec3::ZERO, DVec3::new(1.6, 0.0, 0.0)));
        assert!(!target.intersects_segment(DVec3::ZERO, DVec3::new(1.4, 0.0, 0.0)));
        // 横を通り過ぎる光軸
        assert!(!target.intersects_segment(DVec3::new(0.0, 0.6, 0.0), DVec3::new(4.0, 0.6, 0.0)));
    }

    #[test]
    fn rotated_box_is_checked_in_its_own_frame() {
        // 45° 回すと、角が x = 2 - 0.707 まで出てくる
        let target = TargetBox {
            rotation: DQuat::from_rotation_z(45f64.to_radians()),
            ..unit_box(DVec3::new(2.0, 0.0, 0.0))
        };
        assert!(target.intersects_segment(DVec3::ZERO, DVec3::new(1.35, 0.0, 0.0)));
        assert!(
            (target.distance_to(DVec3::new(1.0, 0.0, 0.0)) - (1.0 - 0.5f64.sqrt())).abs() < 1e-9
        );
    }

    #[test]
    fn proximity_sensor_detects_within_range() {
        let mut sensor = Sensor::new(1, SensorKind::Proximity);
        sensor.range = 0.2;
        let near = unit_box(DVec3::new(0.0, 0.65, 0.0));
        let far = unit_box(DVec3::new(0.0, 0.75, 0.0));
        assert!(sensor.detects(RigidPose::IDENTITY, &near));
        assert!(!sensor.detects(RigidPose::IDENTITY, &far));
    }

    /// シリンダが部品を前に出すと、光軸を遮って ON になる
    fn cylinder_and_beam() -> (TimingChart, Vec<DesignTarget>) {
        let mut chart = TimingChart::default();
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        {
            let c = chart.actuator_mut(cylinder).unwrap();
            c.axis = Axis::X;
            c.stroke = 1.0;
            c.easing = Easing::Linear;
        }
        let sensor = chart.add_sensor(SensorKind::Photoelectric);
        {
            let s = chart.sensor_mut(sensor).unwrap();
            s.position = [1.0, -1.0, 0.0];
            s.axis = Axis::Y;
            s.range = 2.0;
        }
        chart.steps.push(Step {
            actions: vec![Action {
                actuator: cylinder,
                target: 1.0,
                duration: 1.0,
            }],
            ..Default::default()
        });
        chart.steps.push(Step {
            interlocks: vec![Interlock { sensor, on: true }],
            ..Default::default()
        });
        let targets = vec![DesignTarget {
            shape: TargetBox {
                half_size: DVec3::splat(0.2),
                ..unit_box(DVec3::ZERO)
            },
            actuator: Some(cylinder),
        }];
        (chart, targets)
    }

    #[test]
    fn sensor_follows_the_moving_part() {
        let (chart, targets) = cylinder_and_beam();
        let schedule = chart.schedule();
        let at = |t| sensor_states_at(&chart, &schedule, t, &targets, &[])[0];
        assert!(!at(0.0));
        assert!(at(1.0));
    }

    #[test]
    fn interlock_on_moved_part_passes() {
        let (chart, targets) = cylinder_and_beam();
        let schedule = schedule_with_sensors(&chart, |_, _| (targets.clone(), Vec::new()));
        assert!(schedule.stop.is_none());
        assert_eq!(schedule.steps[1].wait(), 0.0);
    }

    #[test]
    fn forced_off_sensor_stops_the_cycle() {
        let (mut chart, targets) = cylinder_and_beam();
        chart.sensors[0].force = Some(false);
        let schedule = schedule_with_sensors(&chart, |_, _| (targets.clone(), Vec::new()));
        assert_eq!(schedule.stop.map(|s| s.step), Some(1));
    }

    #[test]
    fn sensor_ignores_parts_on_its_own_mount() {
        let (mut chart, targets) = cylinder_and_beam();
        let cylinder = chart.actuators[0].id;
        chart.sensors[0].mounted_on = Some(cylinder);
        let schedule = chart.schedule();
        assert!(!sensor_states_at(&chart, &schedule, 1.0, &targets, &[])[0]);
    }

    #[test]
    fn extra_targets_are_detected() {
        let (chart, _) = cylinder_and_beam();
        let schedule = chart.schedule();
        let work = unit_box(DVec3::new(1.0, 0.0, 0.0));
        assert!(sensor_states_at(&chart, &schedule, 0.0, &[], &[work])[0]);
    }
}
