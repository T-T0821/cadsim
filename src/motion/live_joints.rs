//! 実機のロボットから受け取った関節角度を、モデルの関節の値に直す
//!
//! 受け取る角度は根元から順の 6 つ (ラジアン)。ロボットの回転関節に根元から順に割り当て、
//! 度に直して、関節ごとの向き (反転) とずれ (オフセット) を足す。直動の関節には割り当てない。

use std::collections::HashMap;

use super::kinematics;
use plantwright_core::motion::chart::ActuatorKind;
use plantwright_core::motion::chart::Robot;
use plantwright_core::motion::chart::TimingChart;

/// 受け取る角度の数
pub const JOINT_COUNT: usize = 6;

/// 実機の角度とモデルの関節の値の対応 (関節ごと)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct JointMap {
    /// 実機の 0° のとき、モデルの関節が何度か
    pub offset_deg: [f64; JOINT_COUNT],
    /// 回る向きが逆
    pub invert: [bool; JOINT_COUNT],
}

/// 実機の角度から求めたモデルの関節の値
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveJoints {
    /// 関節 (アクチュエータ id) → 値 (度)
    pub values: HashMap<u32, f64>,
    /// 割り当てた関節 (実機の何番目の角度か、の順)
    pub assigned: Vec<u32>,
    /// 動ける範囲を外れた関節
    pub out_of_limits: Vec<u32>,
}

/// ロボットの回転関節 (根元から順)。実機の角度はこの順に割り当てる。
pub fn rotary_joints(chart: &TimingChart, robot: &Robot) -> Vec<u32> {
    kinematics::joints(chart, robot)
        .into_iter()
        .filter(|id| {
            chart
                .actuator(*id)
                .is_some_and(|a| a.kind == ActuatorKind::RotaryTable)
        })
        .collect()
}

/// 実機の角度 `q` (ラジアン) をモデルの関節の値に直す。範囲外の値もそのまま使い、一覧に入れる。
pub fn live_joint_values(
    chart: &TimingChart,
    robot: &Robot,
    q: &[f64; JOINT_COUNT],
    map: &JointMap,
) -> LiveJoints {
    let mut live = LiveJoints::default();
    for (i, id) in rotary_joints(chart, robot)
        .into_iter()
        .take(JOINT_COUNT)
        .enumerate()
    {
        let sign = if map.invert[i] { -1.0 } else { 1.0 };
        let value = sign * q[i].to_degrees() + map.offset_deg[i];
        if let Some([lo, hi]) = chart.actuator(id).and_then(|a| a.limits)
            && !(lo..=hi).contains(&value)
        {
            live.out_of_limits.push(id);
        }
        live.values.insert(id, value);
        live.assigned.push(id);
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use plantwright_core::motion::chart::Axis;

    /// 根元の回転関節 → 直動軸 → 手首の回転関節
    fn arm() -> (TimingChart, u32, [u32; 3]) {
        let mut chart = TimingChart::default();
        let base = chart.add_actuator(ActuatorKind::RotaryTable);
        let slide = chart.add_actuator(ActuatorKind::LinearAxis);
        let wrist = chart.add_actuator(ActuatorKind::RotaryTable);
        chart.actuator_mut(base).unwrap().axis = Axis::Y;
        chart.actuator_mut(slide).unwrap().mounted_on = Some(base);
        {
            let w = chart.actuator_mut(wrist).unwrap();
            w.mounted_on = Some(slide);
            w.limits = Some([-90.0, 90.0]);
        }
        let robot = chart.add_robot(wrist);
        (chart, robot, [base, slide, wrist])
    }

    #[test]
    fn angles_go_to_rotary_joints_from_the_base() {
        let (chart, robot, [base, slide, wrist]) = arm();
        let robot = chart.robot(robot).unwrap();
        assert_eq!(rotary_joints(&chart, robot), vec![base, wrist]);
        let q = [
            std::f64::consts::FRAC_PI_2,
            std::f64::consts::FRAC_PI_4,
            1.0,
            1.0,
            1.0,
            1.0,
        ];
        let live = live_joint_values(&chart, robot, &q, &JointMap::default());
        assert!((live.values[&base] - 90.0).abs() < 1e-9);
        assert!((live.values[&wrist] - 45.0).abs() < 1e-9);
        assert!(!live.values.contains_key(&slide), "直動には割り当てない");
        assert_eq!(live.assigned, vec![base, wrist]);
        assert!(live.out_of_limits.is_empty());
    }

    #[test]
    fn invert_offset_and_limits() {
        let (chart, robot, [base, _, wrist]) = arm();
        let robot = chart.robot(robot).unwrap();
        let mut map = JointMap::default();
        map.invert[0] = true;
        map.offset_deg[0] = 10.0;
        map.offset_deg[1] = 80.0;
        let q = [
            std::f64::consts::FRAC_PI_6,
            20f64.to_radians(),
            0.0,
            0.0,
            0.0,
            0.0,
        ];
        let live = live_joint_values(&chart, robot, &q, &map);
        assert!((live.values[&base] - (-30.0 + 10.0)).abs() < 1e-9);
        assert!((live.values[&wrist] - 100.0).abs() < 1e-9);
        assert_eq!(live.out_of_limits, vec![wrist]);
    }
}
