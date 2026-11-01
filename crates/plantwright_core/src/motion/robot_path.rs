//! ロボットの移動 1 つ分の経路を決め、途中の問題 (届かない・特異点) を調べる
//!
//! - 関節補間 (MoveJ): 教示点の逆運動学の解へ、各関節を同じ時間で同時に動かす。
//!   TCP の通り道は曲線になる。
//! - 直線補間 (MoveL): TCP を教示点まで直線で動かす。経路を細かく区切った点ごとに
//!   逆運動学を解き、関節をその点から点へ等速でつなぐ。区切りの時刻は S 字の速度の付け方で
//!   決めるので、TCP は直線の上を滑らかに加速・減速する。
//!
//! どちらも経路を区切った点で TCP の位置と動かしやすさ ([`kinematics::dexterity`]) を調べ、
//! 3D ビューの経路の表示と、チャートの警告に使う。特異点の警告は直線補間だけに出す
//! (関節補間は関節の値を直接つなぐので、特異点の近くを通っても関節が急に回ることはない)。

use std::collections::HashMap;

use glam::DVec3;

use super::chart::{Easing, MoveKind, Robot, TeachPoint, TimingChart};
use super::kinematics;

/// 直線補間で経路を区切る数
const LINEAR_DIVISIONS: usize = 24;
/// 関節補間の経路を調べるときに区切る数
const CHECK_DIVISIONS: usize = 16;
/// 動かしやすさがこれより小さい点は特異点の近くとみなす
pub const SINGULAR_DEXTERITY: f64 = 0.05;
/// 直線補間で、隣り合う区切りの間に回転関節がこれより大きく回ったら、
/// 特異点をまたいで姿勢が入れ替わったとみなす (度)
const FLIP_DEGREES: f64 = 30.0;

/// 経路の上の 1 点
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PathSample {
    /// その時点の TCP の位置 (モデル座標)
    pub position: [f64; 3],
    /// 動かしやすさ (0〜1、小さいほど特異点に近い)
    pub dexterity: f64,
    /// 直線補間で、その点に TCP が届いているか (関節補間では常に true)
    pub reached: bool,
}

/// 関節 1 つの、移動の中での動き (時刻は移動の長さに対する 0〜1 の比)
#[derive(Clone, Debug, PartialEq)]
pub struct JointPath {
    pub actuator: u32,
    /// (区切りの終わりの時刻の比, その時点の値)。時刻の比の昇順で、最後は 1.0。
    pub waypoints: Vec<(f64, f64)>,
    /// 区切りの中の速度の付け方。None ならアクチュエータの設定に従う。
    pub easing: Option<Easing>,
}

/// ロボットの移動 1 つ分の計画
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlannedMove {
    pub joints: Vec<JointPath>,
    /// 移動を終えたときの関節の値
    pub end_values: HashMap<u32, f64>,
    /// 教示点に届かなかった距離 (モデル単位)
    pub position_error: f64,
    /// 工具の向きのずれ (ラジアン)
    pub angle_error: f64,
    /// 教示点に届いたか
    pub reached: bool,
    /// 経路の途中もすべて届いたか (関節補間では `reached` と同じ)
    pub path_reached: bool,
    /// 経路の上で最小の動かしやすさ
    pub min_dexterity: f64,
    /// (直線補間で) 特異点の近くを通るか、特異点をまたいで姿勢が大きく入れ替わるか
    pub singular: bool,
    /// 経路を区切った点 (始点を含む)
    pub samples: Vec<PathSample>,
}

/// `start` (移動を始めるときの全アクチュエータの値) から教示点 `point` への移動を計画する
pub fn plan_move(
    chart: &TimingChart,
    robot: &Robot,
    point: &TeachPoint,
    kind: MoveKind,
    start: &HashMap<u32, f64>,
) -> PlannedMove {
    match kind {
        MoveKind::Joint => plan_joint(chart, robot, point, start),
        MoveKind::Linear => plan_linear(chart, robot, point, start),
    }
}

fn plan_joint(
    chart: &TimingChart,
    robot: &Robot,
    point: &TeachPoint,
    start: &HashMap<u32, f64>,
) -> PlannedMove {
    let solution = kinematics::solve_ik(chart, robot, point, start);
    let mut ids: Vec<u32> = solution.values.keys().copied().collect();
    ids.sort_unstable();
    let joints = ids
        .iter()
        .map(|&id| JointPath {
            actuator: id,
            waypoints: vec![(1.0, solution.values[&id])],
            easing: None,
        })
        .collect();

    // 各関節はアクチュエータの速度の付け方で同時に動くので、それに合わせて途中の姿勢を求める
    let begin = |id: u32| start.get(&id).copied().unwrap_or(0.0);
    let samples: Vec<PathSample> = (0..=CHECK_DIVISIONS)
        .map(|k| {
            let f = k as f64 / CHECK_DIVISIONS as f64;
            let mut values = start.clone();
            for &id in &ids {
                let easing = chart.actuator(id).map_or(Easing::Linear, |a| a.easing);
                let from = begin(id);
                let progress = easing.apply_with(f, &chart.cams);
                values.insert(id, from + (solution.values[&id] - from) * progress);
            }
            sample(chart, robot, &values, true)
        })
        .collect();

    let mut end_values = start.clone();
    end_values.extend(solution.values.iter().map(|(&k, &v)| (k, v)));
    let min_dexterity = min_dexterity(&samples);
    PlannedMove {
        joints,
        end_values,
        position_error: solution.position_error,
        angle_error: solution.angle_error,
        reached: solution.reached,
        path_reached: solution.reached,
        min_dexterity,
        // 関節補間は関節の値を直接つなぐので、特異点を通っても関節が暴れない
        singular: false,
        samples,
    }
}

fn plan_linear(
    chart: &TimingChart,
    robot: &Robot,
    point: &TeachPoint,
    start: &HashMap<u32, f64>,
) -> PlannedMove {
    let joint_ids = kinematics::joints(chart, robot);
    let from = kinematics::tcp_for(chart, robot, start);
    let target = DVec3::from_array(point.position);
    let approach = point
        .approach
        .and_then(|a| DVec3::from_array(a).try_normalize());

    let mut values = start.clone();
    let mut samples = vec![sample(chart, robot, &values, true)];
    let mut waypoints: Vec<(f64, HashMap<u32, f64>)> = Vec::new();
    let mut flipped = false;
    let mut last = kinematics::IkSolution::default();
    for k in 1..=LINEAR_DIVISIONS {
        let s = k as f64 / LINEAR_DIVISIONS as f64;
        let position = from.position.lerp(target, s);
        // 工具の向きは、始点の向きから教示点の向きへ少しずつ回す
        let axis = approach.map(|d| slerp(from.tool_axis, d, s));
        last = kinematics::solve_ik_near(chart, robot, position, axis, &values);
        for &id in &joint_ids {
            let rotary = chart
                .actuator(id)
                .is_some_and(|a| a.kind == super::chart::ActuatorKind::RotaryTable);
            let before = values.get(&id).copied().unwrap_or(0.0);
            let after = last.values.get(&id).copied().unwrap_or(before);
            flipped |= rotary && (after - before).abs() > FLIP_DEGREES;
        }
        values.extend(last.values.iter().map(|(&id, &v)| (id, v)));
        samples.push(sample(chart, robot, &values, last.reached));
        waypoints.push((time_for_progress(s), values.clone()));
    }

    let joints = joint_ids
        .iter()
        .map(|&id| JointPath {
            actuator: id,
            waypoints: waypoints
                .iter()
                .map(|(t, v)| (*t, v.get(&id).copied().unwrap_or(0.0)))
                .collect(),
            // 区切りの中は等速。全体の加減速は区切りの時刻で付けてある。
            easing: Some(Easing::Linear),
        })
        .collect();
    let path_reached = samples.iter().all(|s| s.reached);
    let min_dexterity = min_dexterity(&samples);
    PlannedMove {
        joints,
        end_values: values,
        position_error: last.position_error,
        angle_error: last.angle_error,
        reached: last.reached,
        path_reached,
        min_dexterity,
        singular: flipped || min_dexterity < SINGULAR_DEXTERITY,
        samples,
    }
}

fn sample(
    chart: &TimingChart,
    robot: &Robot,
    values: &HashMap<u32, f64>,
    reached: bool,
) -> PathSample {
    PathSample {
        position: kinematics::tcp_for(chart, robot, values)
            .position
            .to_array(),
        dexterity: kinematics::dexterity(chart, robot, values),
        reached,
    }
}

fn min_dexterity(samples: &[PathSample]) -> f64 {
    samples
        .iter()
        .map(|s| s.dexterity)
        .fold(f64::INFINITY, f64::min)
        .min(1.0)
}

/// 向き `a` から `b` へ、比 `t` だけ回した向き (単位ベクトル)
fn slerp(a: DVec3, b: DVec3, t: f64) -> DVec3 {
    let angle = a.angle_between(b);
    if angle < 1e-9 {
        return b;
    }
    let axis = a
        .cross(b)
        .try_normalize()
        .unwrap_or_else(|| a.any_orthonormal_vector());
    glam::DQuat::from_axis_angle(axis, angle * t) * a
}

/// S 字の速度の付け方で、経路の進み具合が `s` になる時刻の比 (二分法で逆算する)
fn time_for_progress(s: f64) -> f64 {
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if Easing::Smooth.apply(mid) < s {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// 関節の動きを、移動の中の時刻の比 `f` (0〜1) で読む。`from` は移動を始めたときの値。
/// (時刻表の [`super::chart::Schedule::value_at`] と同じ読み方。テストで経路を確かめるのに使う)
#[cfg(test)]
fn joint_value_at(path: &JointPath, actuator: &super::chart::Actuator, from: f64, f: f64) -> f64 {
    let easing = path.easing.unwrap_or(actuator.easing);
    let mut value = from;
    let mut previous = 0.0;
    for &(end, target) in &path.waypoints {
        if f <= previous {
            break;
        }
        let length = end - previous;
        let progress = if length <= 0.0 {
            1.0
        } else {
            (f - previous) / length
        };
        value += (target - value) * easing.apply(progress);
        previous = end;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{ActuatorKind, Axis};

    /// 2リンクの平面アーム (Z 軸まわり)。肩は原点、肘は (0, 1, 0)、TCP は (0, 2, 0)。
    fn planar_arm() -> (TimingChart, u32) {
        let mut chart = TimingChart::default();
        let shoulder = chart.add_actuator(ActuatorKind::RotaryTable);
        let elbow = chart.add_actuator(ActuatorKind::RotaryTable);
        for (id, pivot) in [(shoulder, [0.0; 3]), (elbow, [0.0, 1.0, 0.0])] {
            let a = chart.actuator_mut(id).unwrap();
            a.axis = Axis::Z;
            a.pivot = pivot;
        }
        chart.actuator_mut(elbow).unwrap().mounted_on = Some(shoulder);
        let robot = chart.add_robot(elbow);
        chart.robot_mut(robot).unwrap().tcp = [0.0, 2.0, 0.0];
        (chart, robot)
    }

    fn point(position: [f64; 3]) -> TeachPoint {
        TeachPoint {
            id: 1,
            name: "P".into(),
            position,
            approach: None,
        }
    }

    /// 肘を曲げた姿勢から始める (伸び切った特異姿勢を避ける)
    fn bent(chart: &TimingChart) -> HashMap<u32, f64> {
        let ids: Vec<u32> = chart.actuators.iter().map(|a| a.id).collect();
        HashMap::from([(ids[0], 30.0), (ids[1], -60.0)])
    }

    #[test]
    fn linear_move_keeps_the_tcp_on_a_straight_line() {
        let (chart, robot) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let start = bent(&chart);
        let from = kinematics::tcp_for(&chart, robot, &start).position;
        let to = DVec3::new(0.6, 1.2, 0.0);
        let plan = plan_move(
            &chart,
            robot,
            &point(to.to_array()),
            MoveKind::Linear,
            &start,
        );
        assert!(plan.reached && plan.path_reached, "{plan:?}");
        // 途中の時刻で関節の値を読み、TCP が始点と終点を結ぶ直線から離れていないか確かめる
        let line = (to - from).normalize();
        for k in 0..=40 {
            let f = k as f64 / 40.0;
            let mut values = start.clone();
            for path in &plan.joints {
                let actuator = chart.actuator(path.actuator).unwrap();
                values.insert(
                    path.actuator,
                    joint_value_at(path, actuator, start[&path.actuator], f),
                );
            }
            let p = kinematics::tcp_for(&chart, robot, &values).position - from;
            let off_line = (p - line * p.dot(line)).length();
            assert!(off_line < 5e-3, "t={f}: {off_line}");
        }
    }

    #[test]
    fn joint_move_bends_the_tcp_path() {
        let (chart, robot) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let start = bent(&chart);
        let from = kinematics::tcp_for(&chart, robot, &start).position;
        let to = DVec3::new(-1.0, 1.0, 0.0);
        let plan = plan_move(
            &chart,
            robot,
            &point(to.to_array()),
            MoveKind::Joint,
            &start,
        );
        assert!(plan.reached);
        let line = (to - from).normalize();
        let farthest = plan
            .samples
            .iter()
            .map(|s| {
                let p = DVec3::from_array(s.position) - from;
                (p - line * p.dot(line)).length()
            })
            .fold(0.0, f64::max);
        assert!(
            farthest > 0.05,
            "関節補間の経路は直線から外れる: {farthest}"
        );
    }

    #[test]
    fn passing_through_a_stretched_arm_is_singular() {
        let (chart, robot) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let start = bent(&chart);
        // 肩からちょうど腕の長さ (2) の点へ直線で動かすと、終点で腕が伸び切る
        let plan = plan_move(
            &chart,
            robot,
            &point([2.0f64.sqrt(), 2.0f64.sqrt(), 0.0]),
            MoveKind::Linear,
            &start,
        );
        assert!(plan.singular, "{}", plan.min_dexterity);
        // 曲げたまま届く点なら特異ではない
        let plan = plan_move(
            &chart,
            robot,
            &point([0.6, 1.2, 0.0]),
            MoveKind::Linear,
            &start,
        );
        assert!(!plan.singular, "{}", plan.min_dexterity);
    }

    #[test]
    fn linear_move_through_an_unreachable_region_is_reported() {
        let (chart, robot) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let start = bent(&chart);
        // 原点の反対側への直線は、肩の真上 (届かない内側) を通らないが、遠い点は途中から届かない
        let plan = plan_move(
            &chart,
            robot,
            &point([3.0, 0.5, 0.0]),
            MoveKind::Linear,
            &start,
        );
        assert!(!plan.path_reached);
        assert!(!plan.reached);
    }

    #[test]
    fn time_for_progress_inverts_the_s_curve() {
        for s in [0.0, 0.1, 0.5, 0.9, 1.0] {
            let t = time_for_progress(s);
            assert!((Easing::Smooth.apply(t) - s).abs() < 1e-9);
        }
    }
}
