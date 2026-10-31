//! ロボットの移動の最短時間と、サイクルタイムの見積もり
//!
//! 各関節 (直線補間なら TCP) が、速度の上限まで一定の時間で加速し、一定の時間で減速する
//! 台形の速度で動くとして、移動にかかる最短の時間を求める。距離が短くて上限の速度まで
//! 届かないときは三角形の速度になる。

use super::chart::{ActuatorKind, MoveKind, Robot, RobotSpeed, Schedule, SolvedMove, TimingChart};
use super::kinematics;

/// 距離 `distance` を、最高速度 `max_speed` まで `accel_time` 秒で加速・減速して動く最短時間
pub fn trapezoid_time(distance: f64, max_speed: f64, accel_time: f64) -> f64 {
    let distance = distance.abs();
    if distance <= 0.0 || max_speed <= 0.0 {
        return 0.0;
    }
    let accel_time = accel_time.max(0.0);
    // 加速と減速だけで進む距離 (最高速度に届いてすぐ減速する場合)
    if distance >= max_speed * accel_time {
        distance / max_speed + accel_time
    } else {
        // 三角形: 加速度 a = v / ta で、半分の距離を加速して半分を減速する
        2.0 * (distance * accel_time / max_speed).sqrt()
    }
}

/// TCP と直動関節の最高速度 (モデル単位/秒)
pub fn linear_speed(chart: &TimingChart, robot: &Robot) -> f64 {
    robot
        .speed
        .linear
        .unwrap_or_else(|| 2.0 * kinematics::reach(chart, robot))
}

/// ロボットの移動 1 つの、速度の上限で動いたときの最短時間 (秒)
pub fn min_move_time(chart: &TimingChart, robot: &Robot, solved: &SolvedMove) -> f64 {
    let RobotSpeed {
        joint, accel_time, ..
    } = robot.speed;
    let linear = linear_speed(chart, robot);
    match solved.kind {
        MoveKind::Joint => solved
            .joint_travel
            .iter()
            .map(|&(id, travel)| {
                let rotary = chart
                    .actuator(id)
                    .is_some_and(|a| a.kind == ActuatorKind::RotaryTable);
                let speed = if rotary { joint } else { linear };
                trapezoid_time(travel, speed, accel_time)
            })
            .fold(0.0, f64::max),
        MoveKind::Linear => trapezoid_time(path_length(solved), linear, accel_time),
    }
}

/// TCP の通り道の長さ (モデル単位)
pub fn path_length(solved: &SolvedMove) -> f64 {
    solved
        .path
        .windows(2)
        .map(|w| {
            let [a, b] = [w[0].position, w[1].position];
            ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2) + (b[2] - a[2]).powi(2)).sqrt()
        })
        .sum()
}

/// サイクルタイムの見積もり
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CycleEstimate {
    /// チャートどおりの各ステップの時間 (秒。条件待ちは含まない)
    pub steps: Vec<f64>,
    /// ロボットの移動をすべて最短にしたときの各ステップの時間 (秒)
    pub fastest_steps: Vec<f64>,
}

impl CycleEstimate {
    pub fn current(&self) -> f64 {
        self.steps.iter().sum()
    }

    pub fn fastest(&self) -> f64 {
        self.fastest_steps.iter().sum()
    }
}

/// ステップごとに、チャートどおりの時間と、ロボットの移動を最短にしたときの時間を求める。
/// 開始条件 (インターロック) の待ち時間は含めない。
pub fn estimate_cycle(chart: &TimingChart, schedule: &Schedule) -> CycleEstimate {
    let mut estimate = CycleEstimate::default();
    for (index, step) in chart.steps.iter().enumerate() {
        let actions = step
            .actions
            .iter()
            .map(|a| a.duration.max(0.0))
            .fold(0.0, f64::max);
        let moves = schedule
            .moves
            .iter()
            .filter(|m| m.step == index)
            .filter_map(|m| Some(min_move_time(chart, chart.robot(m.robot)?, m)))
            .fold(0.0, f64::max);
        let dwell = step.dwell.max(0.0);
        estimate.steps.push(step.duration());
        estimate.fastest_steps.push(actions.max(moves) + dwell);
    }
    estimate
}

/// ロボットの移動の動作時間を、すべて最短時間 (小数第 2 位で切り上げ) にする
pub fn set_fastest_durations(chart: &mut TimingChart, schedule: &Schedule) {
    let times: Vec<(usize, u32, f64)> = schedule
        .moves
        .iter()
        .filter_map(|m| {
            let robot = chart.robot(m.robot)?;
            Some((m.step, m.robot, round_up(min_move_time(chart, robot, m))))
        })
        .collect();
    for (step, robot, time) in times {
        if let Some(m) = chart
            .steps
            .get_mut(step)
            .and_then(|s| s.robot_move_mut(robot))
        {
            m.duration = time;
        }
    }
}

/// 小数第 2 位で切り上げる (0.01 秒刻み)
pub fn round_up(seconds: f64) -> f64 {
    (seconds * 100.0 - 1e-9).ceil().max(0.0) / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{Action, Axis, RobotMove, Step};

    #[test]
    fn trapezoid_and_triangle_profiles() {
        // 100 を 50/秒、加速 0.5 秒: 加速と減速で 25 進むので台形。100/50 + 0.5 = 2.5 秒
        assert!((trapezoid_time(100.0, 50.0, 0.5) - 2.5).abs() < 1e-12);
        // 10 だけなら最高速度に届かない三角形: 2√(10 × 0.5 / 50) ≈ 0.632 秒
        assert!((trapezoid_time(10.0, 50.0, 0.5) - 2.0 * 0.1f64.sqrt()).abs() < 1e-12);
        // 境目 (25) で両方の式が一致する
        let a = trapezoid_time(25.0, 50.0, 0.5);
        assert!((a - 1.0).abs() < 1e-12);
        assert_eq!(trapezoid_time(0.0, 50.0, 0.5), 0.0);
    }

    /// 1 関節 (Z 軸まわり) のアーム。TCP は (0, 1, 0)。
    fn one_joint() -> (TimingChart, u32, u32) {
        let mut chart = TimingChart::default();
        let joint = chart.add_actuator(ActuatorKind::RotaryTable);
        chart.actuator_mut(joint).unwrap().axis = Axis::Z;
        let robot = chart.add_robot(joint);
        let point = {
            let r = chart.robot_mut(robot).unwrap();
            r.tcp = [0.0, 1.0, 0.0];
            r.speed.joint = 90.0;
            r.speed.accel_time = 0.0;
            r.add_point("横".into(), [-1.0, 0.0, 0.0], None)
        };
        (chart, robot, point)
    }

    #[test]
    fn joint_move_time_follows_the_joint_speed() {
        let (mut chart, robot, point) = one_joint();
        let cylinder = chart.add_actuator(ActuatorKind::Cylinder);
        chart.steps = vec![Step {
            moves: vec![RobotMove {
                robot,
                point,
                duration: 3.0,
                kind: MoveKind::Joint,
            }],
            actions: vec![Action {
                actuator: cylinder,
                target: 1.0,
                duration: 0.5,
            }],
            dwell: 0.2,
            ..Default::default()
        }];
        let schedule = chart.schedule();
        // 90° を 90°/秒 で 1 秒
        let solved = &schedule.moves[0];
        let time = min_move_time(&chart, chart.robot(robot).unwrap(), solved);
        assert!((time - 1.0).abs() < 1e-3, "{time}");

        let estimate = estimate_cycle(&chart, &schedule);
        assert!((estimate.current() - 3.2).abs() < 1e-9);
        assert!((estimate.fastest() - 1.2).abs() < 1e-3);

        set_fastest_durations(&mut chart, &schedule);
        assert!((chart.steps[0].moves[0].duration - 1.0).abs() < 0.011);
    }

    #[test]
    fn round_up_to_hundredths() {
        assert_eq!(round_up(1.0), 1.0);
        assert_eq!(round_up(1.001), 1.01);
        assert_eq!(round_up(0.0), 0.0);
    }
}
