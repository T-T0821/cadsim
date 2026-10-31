//! タイミングチャートの値から、各部品の位置・姿勢 (剛体変換, f64) を求める。
//!
//! 部品の設計上の位置 (BoxPart の position / rotation) を原点とし、
//! そこからどれだけ動いたかを [`RigidPose`] で表す。
//! 親アクチュエータに載っている装置は「自分の動き → 親の動き」の順に変換を重ねる。

use std::collections::HashMap;

use glam::{DMat4, DQuat, DVec3};
use truck_modeling::Matrix4;

use super::chart::Schedule;
use super::chart::{Actuator, ActuatorKind, TimingChart};

/// 回転してから平行移動する剛体変換 (f64)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidPose {
    pub rotation: DQuat,
    pub translation: DVec3,
}

impl Default for RigidPose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl RigidPose {
    pub const IDENTITY: Self = Self {
        rotation: DQuat::IDENTITY,
        translation: DVec3::ZERO,
    };

    pub fn from_translation(translation: DVec3) -> Self {
        Self {
            rotation: DQuat::IDENTITY,
            translation,
        }
    }

    /// `pivot` を通る軸まわりの回転
    pub fn from_rotation_about(pivot: DVec3, rotation: DQuat) -> Self {
        Self {
            rotation,
            translation: pivot - rotation * pivot,
        }
    }

    /// `self * other`: 先に other を適用し、次に self を適用する
    pub fn then_apply_to(self, other: RigidPose) -> RigidPose {
        RigidPose {
            rotation: self.rotation * other.rotation,
            translation: self.rotation * other.translation + self.translation,
        }
    }

    // 干渉チェックなど、動いた後の形状を扱う処理から使う
    #[allow(dead_code)]
    pub fn transform_point(&self, p: DVec3) -> DVec3 {
        self.rotation * p + self.translation
    }

    pub fn to_dmat4(self) -> DMat4 {
        DMat4::from_rotation_translation(self.rotation, self.translation)
    }

    /// truck の形状に適用するための行列。
    /// 干渉チェックでは `builder::transformed(&solid, pose.to_truck_matrix())` で
    /// 再生中の位置にある B-rep を得られる。
    #[allow(dead_code)]
    pub fn to_truck_matrix(self) -> Matrix4 {
        let c = self.to_dmat4().to_cols_array();
        // cgmath (truck) の Matrix4::new も列優先で並べる
        Matrix4::new(
            c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7], c[8], c[9], c[10], c[11], c[12], c[13],
            c[14], c[15],
        )
    }
}

/// 設計位置での動く向き (直動は移動方向、回転は回転軸の単位ベクトル)
pub fn direction(actuator: &Actuator) -> DVec3 {
    let direction = DVec3::from_array(actuator.axis.unit_vector());
    if actuator.reversed {
        -direction
    } else {
        direction
    }
}

/// 1台のアクチュエータ自身の動き (親の動きは含まない)
fn local_motion(actuator: &Actuator, value: f64) -> RigidPose {
    let direction = direction(actuator);
    let value = actuator.clamp(value);
    match actuator.kind {
        ActuatorKind::Cylinder => RigidPose::from_translation(direction * value * actuator.stroke),
        ActuatorKind::LinearAxis => RigidPose::from_translation(direction * value),
        ActuatorKind::RotaryTable => RigidPose::from_rotation_about(
            DVec3::from_array(actuator.pivot),
            DQuat::from_axis_angle(direction, value.to_radians()),
        ),
    }
}

/// 時刻 t における、各アクチュエータの「親の動きも含めた」変換
pub fn actuator_poses(chart: &TimingChart, schedule: &Schedule, t: f64) -> HashMap<u32, RigidPose> {
    actuator_poses_from(chart, |a| schedule.value_at(a, t))
}

/// 各アクチュエータの値から、「親の動きも含めた」変換を求める (順運動学)。
/// `values` にないアクチュエータは原点 (0) にあるものとする。
pub fn actuator_poses_for(
    chart: &TimingChart,
    values: &HashMap<u32, f64>,
) -> HashMap<u32, RigidPose> {
    actuator_poses_from(chart, |a| values.get(&a.id).copied().unwrap_or(0.0))
}

/// アクチュエータごとの値 (`value_of`) から、「親の動きも含めた」変換を求める。
/// 値の出どころは時刻表でも PLC でもジョグでもよい。
fn actuator_poses_from(
    chart: &TimingChart,
    value_of: impl Fn(&Actuator) -> f64,
) -> HashMap<u32, RigidPose> {
    let local: HashMap<u32, RigidPose> = chart
        .actuators
        .iter()
        .map(|a| (a.id, local_motion(a, value_of(a))))
        .collect();

    let mut world = HashMap::new();
    for actuator in &chart.actuators {
        // 自分 → 親 → 親の親 … の順に変換を重ねる。循環していても止まるよう回数を制限する。
        let mut pose = local[&actuator.id];
        let mut parent = actuator.mounted_on;
        for _ in 0..chart.actuators.len() {
            let Some(id) = parent else { break };
            let Some(parent_pose) = local.get(&id) else {
                break;
            };
            pose = parent_pose.then_apply_to(pose);
            parent = chart.actuator(id).and_then(|a| a.mounted_on);
        }
        world.insert(actuator.id, pose);
    }
    world
}

/// 時刻 t における各部品の変換 (動かない部品は含まない)
pub fn part_poses(chart: &TimingChart, schedule: &Schedule, t: f64) -> HashMap<u32, RigidPose> {
    part_poses_for(chart, &actuator_poses(chart, schedule, t))
}

/// 各アクチュエータの変換から、各部品の変換を求める (動かない部品は含まない)
pub fn part_poses_for(
    chart: &TimingChart,
    actuator_poses: &HashMap<u32, RigidPose>,
) -> HashMap<u32, RigidPose> {
    let mut parts = HashMap::new();
    for actuator in &chart.actuators {
        for &part in &actuator.parts {
            parts.insert(part, actuator_poses[&actuator.id]);
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{Action, Axis, Easing, Step};
    use truck_modeling::{EuclideanSpace, Point3, Transform as _};

    #[test]
    fn cylinder_on_axis_moves_with_it() {
        let mut chart = TimingChart::default();
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        {
            let c = chart.actuator_mut(cylinder).unwrap();
            c.mounted_on = Some(axis);
            c.axis = Axis::Y;
            c.reversed = true;
            c.stroke = 0.5;
            c.parts = vec![7];
        }
        for a in &mut chart.actuators {
            a.easing = Easing::Linear;
        }
        chart.steps.push(Step {
            actions: vec![
                Action {
                    actuator: axis,
                    target: 2.0,
                    duration: 1.0,
                },
                Action {
                    actuator: cylinder,
                    target: 1.0,
                    duration: 1.0,
                },
            ],
            ..Default::default()
        });
        let schedule = chart.schedule();
        let pose = part_poses(&chart, &schedule, 1.0)[&7];
        assert!((pose.translation - DVec3::new(2.0, -0.5, 0.0)).length() < 1e-12);
    }

    #[test]
    fn rotary_table_rotates_about_pivot() {
        let pose = RigidPose::from_rotation_about(
            DVec3::new(1.0, 0.0, 0.0),
            DQuat::from_rotation_y(90f64.to_radians()),
        );
        let moved = pose.transform_point(DVec3::new(2.0, 0.0, 0.0));
        assert!((moved - DVec3::new(1.0, 0.0, -1.0)).length() < 1e-12);
    }

    #[test]
    fn truck_matrix_matches_pose() {
        let pose = RigidPose {
            rotation: DQuat::from_rotation_z(0.7),
            translation: DVec3::new(1.0, 2.0, 3.0),
        };
        let p = DVec3::new(0.3, -1.0, 2.0);
        let expected = pose.transform_point(p);
        let got =
            pose.to_truck_matrix()
                .transform_point(Point3::from_vec(truck_modeling::Vector3::new(
                    p.x, p.y, p.z,
                )));
        assert!((got.x - expected.x).abs() < 1e-12);
        assert!((got.y - expected.y).abs() < 1e-12);
        assert!((got.z - expected.z).abs() < 1e-12);
    }
}
