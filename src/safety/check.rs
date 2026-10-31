//! 動作範囲 (危険源) と、安全機器の距離の確認
//!
//! - 動作範囲: タイミングチャートの1サイクルの間に、アクチュエータが動かす部品が通る範囲。
//!   サイクルを細かく区切った各時刻の外接箱を足し合わせる。
//! - 距離の確認: 安全柵・ライトカーテンから、いちばん近い動作範囲までの床の上の距離を測り、
//!   必要な距離 (柵は設定値、ライトカーテンは ISO 13855 の S) と比べる。
//!
//! Bevy に依存しない純粋な計算だけを置く。

use bevy::math::DVec3;

use super::device::{DeviceKind, SafetyDevice};
use crate::floor::geom::Rect2;
use crate::math::Aabb;
use crate::motion::{RigidPose, Schedule, TimingChart, part_poses};

/// サイクルを区切る数 (動作の始まりと終わりの時刻はこれとは別に必ず調べる)
const SAMPLES_PER_CYCLE: usize = 96;

/// 1台のアクチュエータの動作範囲
#[derive(Clone, Debug, PartialEq)]
pub struct Envelope {
    pub actuator: u32,
    pub name: String,
    pub bounds: Aabb,
}

/// アクチュエータごとの動作範囲。`part_bounds` はノードの設計位置での外接箱を返す。
pub fn motion_envelopes(
    chart: &TimingChart,
    schedule: &Schedule,
    part_bounds: impl Fn(u32) -> Option<Aabb>,
) -> Vec<Envelope> {
    let times = sample_times(schedule);
    let poses: Vec<_> = times
        .iter()
        .map(|&t| part_poses(chart, schedule, t))
        .collect();
    chart
        .actuators
        .iter()
        .filter_map(|actuator| {
            let bounds = actuator
                .parts
                .iter()
                .filter_map(|&part| {
                    let design = part_bounds(part)?;
                    poses
                        .iter()
                        .map(|p| transform_aabb(&design, p.get(&part).copied().unwrap_or_default()))
                        .reduce(Aabb::union)
                })
                .reduce(Aabb::union)?;
            Some(Envelope {
                actuator: actuator.id,
                name: actuator.name.clone(),
                bounds,
            })
        })
        .collect()
}

fn sample_times(schedule: &Schedule) -> Vec<f64> {
    let cycle = schedule.cycle_time.max(0.0);
    let mut times: Vec<f64> = (0..=SAMPLES_PER_CYCLE)
        .map(|i| cycle * i as f64 / SAMPLES_PER_CYCLE as f64)
        .collect();
    for segment in schedule.segments.values().flatten() {
        times.push(segment.start);
        times.push(segment.start + segment.duration);
    }
    times.sort_by(f64::total_cmp);
    times.dedup();
    times
}

fn transform_aabb(aabb: &Aabb, pose: RigidPose) -> Aabb {
    let points = aabb.corners().map(|p| pose.transform_point(p));
    let mut out = Aabb {
        min: DVec3::splat(f64::INFINITY),
        max: DVec3::splat(f64::NEG_INFINITY),
    };
    for p in points {
        out.min = out.min.min(p);
        out.max = out.max.max(p);
    }
    out
}

/// 機器 1つの距離の確認結果
#[derive(Clone, Debug, PartialEq)]
pub struct Clearance {
    pub device: u32,
    /// いちばん近い動作範囲のアクチュエータ
    pub actuator: u32,
    /// 床の上の距離 (mm)
    pub distance: f64,
    /// 必要な距離 (mm)
    pub required: f64,
}

impl Clearance {
    pub fn is_ok(&self) -> bool {
        self.distance + 1e-9 >= self.required
    }
}

/// 安全柵とライトカーテンについて、いちばん近い動作範囲までの距離を確かめる
pub fn check_clearances(devices: &[SafetyDevice], envelopes: &[Envelope]) -> Vec<Clearance> {
    devices
        .iter()
        .filter_map(|device| {
            let required = match device.kind {
                DeviceKind::Fence => device.required_clearance,
                DeviceKind::LightCurtain => device.safety_distance()?,
                _ => return None,
            };
            let footprint = device.footprint();
            envelopes
                .iter()
                .map(|e| Clearance {
                    device: device.id,
                    actuator: e.actuator,
                    distance: footprint.distance(&Rect2::from_aabb(&e.bounds)),
                    required,
                })
                .min_by(|a, b| a.distance.total_cmp(&b.distance))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::{Action, ActuatorKind, Axis, Step};

    /// 部品 `part` を1台のアクチュエータで `target` まで動かし、原点に戻すチャート
    fn chart_with(kind: ActuatorKind, part: u32, target: f64) -> TimingChart {
        let mut chart = TimingChart::default();
        let id = chart.add_actuator(kind);
        let a = chart.actuator_mut(id).unwrap();
        a.axis = match kind {
            ActuatorKind::RotaryTable => Axis::Y,
            _ => Axis::X,
        };
        a.parts = vec![part];
        for target in [target, 0.0] {
            chart.steps.push(Step {
                actions: vec![Action {
                    actuator: id,
                    target,
                    duration: 1.0,
                }],
                ..Default::default()
            });
        }
        chart
    }

    fn linear_axis_chart(part: u32, stroke: f64) -> TimingChart {
        chart_with(ActuatorKind::LinearAxis, part, stroke)
    }

    fn rotary_chart(part: u32, degrees: f64) -> TimingChart {
        chart_with(ActuatorKind::RotaryTable, part, degrees)
    }

    fn unit_box_at(x: f64) -> Aabb {
        Aabb {
            min: DVec3::new(x, 0.0, 0.0),
            max: DVec3::new(x + 100.0, 100.0, 100.0),
        }
    }

    #[test]
    fn envelope_covers_whole_stroke() {
        // 部品 7 を X 方向に 1000 動かして戻す
        let chart = linear_axis_chart(7, 1000.0);
        let schedule = chart.schedule();
        let envelopes =
            motion_envelopes(&chart, &schedule, |id| (id == 7).then(|| unit_box_at(0.0)));
        assert_eq!(envelopes.len(), 1);
        let b = envelopes[0].bounds;
        assert!((b.min.x - 0.0).abs() < 1e-9);
        assert!((b.max.x - 1100.0).abs() < 1e-9);
    }

    #[test]
    fn envelope_of_rotary_table_includes_quarter_turn() {
        // 原点まわりに 90° 回す: X 側の箱が -Z 側まで届く
        let chart = rotary_chart(7, 90.0);
        let schedule = chart.schedule();
        let envelopes = motion_envelopes(&chart, &schedule, |_| Some(unit_box_at(500.0)));
        let b = envelopes[0].bounds;
        assert!(b.min.z < -590.0, "{b:?}");
        assert!(b.max.x > 590.0, "{b:?}");
    }

    #[test]
    fn fence_too_close_to_envelope_is_flagged() {
        let envelopes = vec![Envelope {
            actuator: 1,
            name: "X軸".into(),
            bounds: Aabb {
                min: DVec3::new(0.0, 0.0, 0.0),
                max: DVec3::new(1000.0, 500.0, 500.0),
            },
        }];
        // 柵は z = 1000 の位置に X 方向に長く置く (厚み 50) → 範囲の端 z = 500 から 475
        let mut fence = SafetyDevice::new(1, DeviceKind::Fence, [500.0, 1000.0]);
        fence.required_clearance = 850.0;
        let result = &check_clearances(&[fence.clone()], &envelopes)[0];
        assert!((result.distance - 475.0).abs() < 1e-9);
        assert!(!result.is_ok());

        fence.position[2] = 1500.0;
        assert!(check_clearances(&[fence], &envelopes)[0].is_ok());
    }

    #[test]
    fn light_curtain_uses_safety_distance() {
        let envelopes = vec![Envelope {
            actuator: 1,
            name: "X軸".into(),
            bounds: unit_box_at(0.0),
        }];
        let curtain = SafetyDevice::new(1, DeviceKind::LightCurtain, [50.0, 600.0]);
        let result = &check_clearances(std::slice::from_ref(&curtain), &envelopes)[0];
        assert_eq!(result.required, curtain.safety_distance().unwrap());
    }
}
