//! ロボット (関節を載せ先でつないだ直列リンク) の順運動学と逆運動学
//!
//! 関節の動きは「設計位置 (全関節 0) で定義した軸」を、根元側の関節の動きで運んでから
//! 重ねる (指数積 / Product of Exponentials と同じ形)。順運動学は [`pose::actuator_poses_for`]
//! がそのまま担い、ここではヤコビアンと逆運動学を足す。
//!
//! 逆運動学は減衰最小二乗法 (Damped Least Squares)。特異姿勢の近くでも暴れにくく、
//! 関節の値を初期値から少しずつ動かすので、直前の姿勢に近い解が選ばれる。
//! 各反復で可動範囲に収めるので、範囲外の解は選ばない (届かなければ `reached = false`)。

use std::collections::HashMap;

use bevy::math::DVec3;

use super::chart::{ActuatorKind, Robot, TeachPoint, TimingChart};
use super::pose::{self, RigidPose};

/// 逆運動学の反復回数の上限
const MAX_ITERATIONS: usize = 300;
/// 減衰係数 (ロボットの大きさ [`reach`] に対する比)。大きいほど特異姿勢の近くで安定するが、収束は遅くなる。
const DAMPING: f64 = 0.05;
/// 1回の反復で動かす量の上限 (回転はラジアン、直動は [`reach`] に対する比)
const MAX_STEP: f64 = 0.2;
/// これより近ければ教示点に届いたとみなす (モデル単位)
pub const POSITION_TOLERANCE: f64 = 1e-3;
/// これより小さければ工具の向きが合ったとみなす (ラジアン、約 0.5°)
pub const ANGLE_TOLERANCE: f64 = 0.0087;

/// 先端の関節 `tip` から載せ先をたどった関節の並び (根元が先頭)。
/// シリンダなど、逆運動学で動かさないアクチュエータも含む。
pub fn chain(chart: &TimingChart, tip: u32) -> Vec<u32> {
    let mut ids = Vec::new();
    let mut current = chart.actuator(tip).map(|a| a.id);
    // 壊れたデータで無限ループしないよう、たどる回数に上限を付ける
    for _ in 0..chart.actuators.len() {
        let Some(id) = current else { break };
        ids.push(id);
        current = chart.actuator(id).and_then(|a| a.mounted_on);
    }
    ids.reverse();
    ids
}

/// ロボットの関節のうち、逆運動学で動かすもの (回転テーブル・直動軸)
pub fn joints(chart: &TimingChart, robot: &Robot) -> Vec<u32> {
    chain(chart, robot.tip)
        .into_iter()
        .filter(|id| chart.actuator(*id).is_some_and(|a| a.is_joint()))
        .collect()
}

/// ロボットの大きさの目安 (設計位置での、回転関節の中心から TCP までの最大距離)。
/// モデルの単位 (mm など) によらず同じように解けるよう、直動の移動量や向きの誤差の重みをこれで合わせる。
pub fn reach(chart: &TimingChart, robot: &Robot) -> f64 {
    let tcp = DVec3::from_array(robot.tcp);
    chain(chart, robot.tip)
        .iter()
        .filter_map(|id| chart.actuator(*id))
        .filter(|a| a.kind == ActuatorKind::RotaryTable)
        .map(|a| (tcp - DVec3::from_array(a.pivot)).length())
        .fold(0.0, f64::max)
        .max(1.0)
}

/// 工具先端 (TCP) の位置と工具軸の向き
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tcp {
    pub position: DVec3,
    pub tool_axis: DVec3,
}

/// 各アクチュエータの変換から TCP を求める
pub fn tcp(robot: &Robot, poses: &HashMap<u32, RigidPose>) -> Tcp {
    let pose = poses.get(&robot.tip).copied().unwrap_or_default();
    Tcp {
        position: pose.transform_point(DVec3::from_array(robot.tcp)),
        tool_axis: (pose.rotation * DVec3::from_array(robot.tool_axis)).normalize_or_zero(),
    }
}

/// 関節の値から TCP を求める (順運動学)
pub fn tcp_for(chart: &TimingChart, robot: &Robot, values: &HashMap<u32, f64>) -> Tcp {
    tcp(robot, &pose::actuator_poses_for(chart, values))
}

#[derive(Clone, Debug, Default)]
pub struct IkSolution {
    /// 逆運動学で動かす関節の値 (回転は度、直動はモデル単位)
    pub values: HashMap<u32, f64>,
    /// 届かなかった距離 (モデル単位)
    pub position_error: f64,
    /// 工具の向きのずれ (ラジアン)。向きを指定しないときは 0。
    pub angle_error: f64,
    pub reached: bool,
}

/// 教示点へ TCP を持っていく関節の値を求める
pub fn solve_ik(
    chart: &TimingChart,
    robot: &Robot,
    point: &TeachPoint,
    seed: &HashMap<u32, f64>,
) -> IkSolution {
    solve_ik_to(
        chart,
        robot,
        DVec3::from_array(point.position),
        point.approach.map(DVec3::from_array),
        seed,
    )
}

/// TCP を `target` へ、(指定があれば) 工具軸を `approach` へ向ける関節の値を求める。
/// `seed` は初期値 (ないアクチュエータは 0)。逆運動学で動かさないアクチュエータは `seed` のまま。
///
/// まず `seed` から解き、届かなければ (可動範囲の端で行き止まったときなど) 初期値を変えて
/// 何度か解き直す。どれでも届かなければ、`seed` からできるだけ近づけた姿勢を返す。
pub fn solve_ik_to(
    chart: &TimingChart,
    robot: &Robot,
    target: DVec3,
    approach: Option<DVec3>,
    seed: &HashMap<u32, f64>,
) -> IkSolution {
    let joints: Vec<u32> = joints(chart, robot);
    let approach = approach.and_then(|a| a.try_normalize());
    let mut best = descend(chart, robot, &joints, target, approach, seed.clone());
    // 初期値の候補: 可動範囲の中の 1/2, 1/4, 3/4 の位置 (関節ごとに互い違い)
    for k in [0.5, 0.25, 0.75, 0.1, 0.9] {
        if best.reached {
            break;
        }
        let mut values = seed.clone();
        for (i, &id) in joints.iter().enumerate() {
            let actuator = chart.actuator(id).expect("関節はチャートにある");
            let k = if i % 2 == 0 { k } else { 1.0 - k };
            let [lo, hi] = actuator.limits.unwrap_or(match actuator.kind {
                ActuatorKind::RotaryTable => [-180.0, 180.0],
                _ => [-1.0, 1.0],
            });
            values.insert(id, lo + k * (hi - lo));
        }
        let candidate = descend(chart, robot, &joints, target, approach, values);
        // 届かないときは、初期値から近づけた姿勢のほうが自然なので、届いた解だけ採用する
        if candidate.reached {
            best = candidate;
        }
    }
    best
}

/// `seed` から少しずつ動かせる範囲だけで解く (初期値を変えた解き直しはしない)。
/// 直線補間の経路のように、直前の姿勢から飛ばずにつながった解がほしいときに使う。
pub fn solve_ik_near(
    chart: &TimingChart,
    robot: &Robot,
    target: DVec3,
    approach: Option<DVec3>,
    seed: &HashMap<u32, f64>,
) -> IkSolution {
    let joints: Vec<u32> = joints(chart, robot);
    let approach = approach.and_then(|a| a.try_normalize());
    descend(chart, robot, &joints, target, approach, seed.clone())
}

/// 減衰最小二乗法で `values` から少しずつ動かして解く
fn descend(
    chart: &TimingChart,
    robot: &Robot,
    joints: &[u32],
    target: DVec3,
    approach: Option<DVec3>,
    mut values: HashMap<u32, f64>,
) -> IkSolution {
    for &id in joints {
        let actuator = chart.actuator(id).expect("関節はチャートにある");
        let v = values.entry(id).or_insert(0.0);
        *v = actuator.clamp(*v);
    }
    let scale = reach(chart, robot);

    for _ in 0..MAX_ITERATIONS {
        let poses = pose::actuator_poses_for(chart, &values);
        let current = tcp(robot, &poses);
        let position_error = target - current.position;
        let angular_error = approach.map(|d| rotation_toward(current.tool_axis, d));
        let angle = approach.map_or(0.0, |d| current.tool_axis.angle_between(d));
        if position_error.length() < 1e-7 * scale && angle < 1e-7 {
            break;
        }

        // 行は位置 3 + (向きを指定したとき) 角速度 3
        let rows = if approach.is_some() { 6 } else { 3 };
        let mut columns = jacobian(chart, joints, &poses, current.position, scale);
        let mut error = [0.0; 6];
        error[..3].copy_from_slice(&position_error.to_array());
        if let Some(w) = angular_error {
            error[3..].copy_from_slice(&(w * scale).to_array());
        }

        let Some(mut step) = damped_least_squares(&columns, &error[..rows], rows, DAMPING * scale)
        else {
            break;
        };
        // 可動範囲の端にいて、さらに外へ動こうとする関節は止めて、残りの関節で解き直す
        let mut locked = false;
        for ((&id, column), delta) in joints.iter().zip(&mut columns).zip(&step) {
            let actuator = chart.actuator(id).expect("関節はチャートにある");
            let v = values[&id];
            if let Some([lo, hi]) = actuator.limits
                && ((v <= lo && *delta < 0.0) || (v >= hi && *delta > 0.0))
            {
                *column = [0.0; 6];
                locked = true;
            }
        }
        if locked {
            let Some(retry) = damped_least_squares(&columns, &error[..rows], rows, DAMPING * scale)
            else {
                break;
            };
            step = retry;
        }
        // 上限に対する比がいちばん大きい関節に合わせて、全体を縮める
        let largest = joints.iter().zip(&step).fold(0.0f64, |m, (&id, v)| {
            let rotary = chart
                .actuator(id)
                .is_some_and(|a| a.kind == ActuatorKind::RotaryTable);
            let limit = if rotary { MAX_STEP } else { MAX_STEP * scale };
            m.max(v.abs() / limit)
        });
        if largest > 1.0 {
            for v in &mut step {
                *v /= largest;
            }
        }
        let mut moved = false;
        for (&id, delta) in joints.iter().zip(step) {
            let actuator = chart.actuator(id).expect("関節はチャートにある");
            let delta = if actuator.kind == ActuatorKind::RotaryTable {
                delta.to_degrees()
            } else {
                delta
            };
            let v = values.get_mut(&id).expect("初期値を入れてある");
            let next = actuator.clamp(*v + delta);
            moved |= (next - *v).abs() > 1e-12;
            *v = next;
        }
        if !moved {
            // 可動範囲の端で止まった
            break;
        }
    }

    // 最後の更新を反映した誤差で判定する
    let current = tcp_for(chart, robot, &values);
    let errors = (
        (target - current.position).length(),
        approach.map_or(0.0, |d| current.tool_axis.angle_between(d)),
    );
    IkSolution {
        values: joints
            .iter()
            .map(|id| (*id, values[id]))
            .collect::<HashMap<_, _>>(),
        position_error: errors.0,
        angle_error: errors.1,
        reached: errors.0 < POSITION_TOLERANCE && errors.1 < ANGLE_TOLERANCE,
    }
}

/// ヤコビアン: 各関節を単位量 (ラジアン / モデル単位) 動かしたときの TCP の速度を、関節ごとの列にする。
/// 列の先頭 3 つは並進速度、残り 3 つは角速度を `scale` 倍したもの (大きさをそろえるため)。
fn jacobian(
    chart: &TimingChart,
    joints: &[u32],
    poses: &HashMap<u32, RigidPose>,
    tcp_position: DVec3,
    scale: f64,
) -> Vec<[f64; 6]> {
    joints
        .iter()
        .map(|&id| {
            let actuator = chart.actuator(id).expect("関節はチャートにある");
            let parent = actuator
                .mounted_on
                .and_then(|p| poses.get(&p).copied())
                .unwrap_or_default();
            let axis = parent.rotation * pose::direction(actuator);
            let (linear, angular) = match actuator.kind {
                ActuatorKind::RotaryTable => {
                    let pivot = parent.transform_point(DVec3::from_array(actuator.pivot));
                    (axis.cross(tcp_position - pivot), axis)
                }
                _ => (axis, DVec3::ZERO),
            };
            let angular = angular * scale;
            [
                linear.x, linear.y, linear.z, angular.x, angular.y, angular.z,
            ]
        })
        .collect()
}

/// TCP の動かしやすさ (0〜1)。TCP の位置を動かすヤコビアンの、最小と最大の特異値の比。
///
/// 腕が伸び切ったときや、TCP が旋回軸の上に来たときなど、ある向きへ動かせなくなる
/// 特異姿勢に近いほど 0 に近づく。関節が 3 つ未満のロボットは、関節の数だけの向きで比べる。
pub fn dexterity(chart: &TimingChart, robot: &Robot, values: &HashMap<u32, f64>) -> f64 {
    let joints = joints(chart, robot);
    if joints.is_empty() {
        return 0.0;
    }
    let scale = reach(chart, robot);
    let poses = pose::actuator_poses_for(chart, values);
    let position = tcp(robot, &poses).position;
    // 回転関節の列は reach で割って、直動関節 (単位量で単位の速度) と大きさをそろえる
    let columns: Vec<DVec3> = jacobian(chart, &joints, &poses, position, scale)
        .iter()
        .zip(&joints)
        .map(|(c, &id)| {
            let linear = DVec3::new(c[0], c[1], c[2]);
            let rotary = chart
                .actuator(id)
                .is_some_and(|a| a.kind == ActuatorKind::RotaryTable);
            if rotary { linear / scale } else { linear }
        })
        .collect();
    // 特異値の 2 乗 = グラム行列の固有値。関節が 3 つ以上なら J Jᵀ (3×3)、少なければ Jᵀ J。
    let n = columns.len().min(3);
    let mut gram = [[0.0; 3]; 3];
    for (i, row) in gram.iter_mut().enumerate().take(n) {
        for (j, cell) in row.iter_mut().enumerate().take(n) {
            *cell = if columns.len() >= 3 {
                columns.iter().map(|c| c[i] * c[j]).sum()
            } else {
                columns[i].dot(columns[j])
            };
        }
    }
    let eigen = symmetric_eigenvalues(gram, n);
    let largest = eigen.iter().copied().fold(0.0, f64::max);
    if largest <= 1e-24 {
        return 0.0;
    }
    let smallest = eigen.iter().copied().fold(f64::INFINITY, f64::min).max(0.0);
    (smallest / largest).sqrt()
}

/// 対称行列 (先頭 n×n) の固有値をヤコビ法で求める
#[allow(clippy::needless_range_loop)]
fn symmetric_eigenvalues(mut a: [[f64; 3]; 3], n: usize) -> Vec<f64> {
    for _ in 0..50 {
        // いちばん大きい非対角成分を消す回転をかける
        let mut p = 0;
        let mut q = 0;
        let mut off = 0.0;
        for i in 0..n {
            for j in i + 1..n {
                if a[i][j].abs() > off {
                    off = a[i][j].abs();
                    p = i;
                    q = j;
                }
            }
        }
        if off < 1e-15 {
            break;
        }
        let theta = 0.5 * (2.0 * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (sin, cos) = theta.sin_cos();
        for row in a.iter_mut().take(n) {
            let (x, y) = (row[p], row[q]);
            row[p] = cos * x - sin * y;
            row[q] = sin * x + cos * y;
        }
        for k in 0..n {
            let (x, y) = (a[p][k], a[q][k]);
            a[p][k] = cos * x - sin * y;
            a[q][k] = sin * x + cos * y;
        }
    }
    (0..n).map(|i| a[i][i]).collect()
}

/// 向き `from` を `to` に回す微小回転 (回転軸 × 角度の正弦)。真逆のときは直交する軸で回す。
fn rotation_toward(from: DVec3, to: DVec3) -> DVec3 {
    let w = from.cross(to);
    if w.length() < 1e-9 && from.dot(to) < 0.0 {
        from.any_orthonormal_vector()
    } else {
        w
    }
}

/// dq = Jᵀ (J Jᵀ + λ² I)⁻¹ e。`columns` はヤコビアンの列 (関節ごと)、使うのは先頭 `rows` 行。
fn damped_least_squares(
    columns: &[[f64; 6]],
    error: &[f64],
    rows: usize,
    damping: f64,
) -> Option<Vec<f64>> {
    let mut a = [[0.0; 7]; 6];
    for (i, row) in a.iter_mut().enumerate().take(rows) {
        for (j, cell) in row.iter_mut().enumerate().take(rows) {
            *cell = columns.iter().map(|c| c[i] * c[j]).sum::<f64>();
        }
        row[i] += damping * damping;
        row[rows] = error[i];
    }
    let y = solve_linear(&mut a[..rows], rows)?;
    Some(
        columns
            .iter()
            .map(|c| (0..rows).map(|i| c[i] * y[i]).sum())
            .collect(),
    )
}

/// 拡大係数行列 (n 行 × n+1 列) の連立一次方程式を、部分ピボット付きガウスの消去法で解く
fn solve_linear(a: &mut [[f64; 7]], n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        let pivot = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[pivot][col].abs() < 1e-15 {
            return None;
        }
        a.swap(col, pivot);
        let (upper, lower) = a.split_at_mut(col + 1);
        let pivot_row = &upper[col];
        for row in lower.iter_mut().take(n - col - 1) {
            let factor = row[col] / pivot_row[col];
            for (cell, p) in row.iter_mut().zip(pivot_row).skip(col).take(n + 1 - col) {
                *cell -= factor * p;
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let sum: f64 = (row + 1..n).map(|k| a[row][k] * x[k]).sum();
        x[row] = (a[row][n] - sum) / a[row][row];
    }
    Some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{Axis, RobotMove, Step};

    /// 2リンクの平面アーム (Z 軸まわりに回る肩と肘)。肩は原点、肘は (0, 1, 0)、TCP は (0, 2, 0)。
    fn planar_arm() -> (TimingChart, u32, u32, u32) {
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
        (chart, robot, shoulder, elbow)
    }

    #[test]
    fn forward_kinematics_composes_joints() {
        let (chart, robot, shoulder, elbow) = planar_arm();
        let values = HashMap::from([(shoulder, -90.0), (elbow, 90.0)]);
        // 肩を -90° で腕が +X 方向に倒れ、肘で +90° 戻ると先端は +Y を向く
        let tcp = tcp_for(&chart, chart.robot(robot).unwrap(), &values);
        assert!((tcp.position - DVec3::new(1.0, 1.0, 0.0)).length() < 1e-12);
        assert!((tcp.tool_axis - DVec3::Y).length() < 1e-12);
    }

    #[test]
    fn dexterity_drops_to_zero_when_the_arm_is_stretched() {
        let (chart, robot, shoulder, elbow) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let stretched = HashMap::from([(shoulder, 20.0), (elbow, 0.0)]);
        assert!(dexterity(&chart, robot, &stretched) < 1e-9);
        let bent = HashMap::from([(shoulder, 20.0), (elbow, 90.0)]);
        assert!(dexterity(&chart, robot, &bent) > 0.3);
    }

    #[test]
    fn ik_reaches_a_point_and_matches_fk() {
        let (chart, robot, _, _) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let target = DVec3::new(1.2, 0.8, 0.0);
        let solution = solve_ik_to(&chart, robot, target, None, &HashMap::new());
        assert!(solution.reached, "{solution:?}");
        let tcp = tcp_for(&chart, robot, &solution.values);
        assert!((tcp.position - target).length() < POSITION_TOLERANCE);
    }

    #[test]
    fn ik_reports_unreachable_points() {
        let (chart, robot, _, _) = planar_arm();
        let robot = chart.robot(robot).unwrap();
        let solution = solve_ik_to(
            &chart,
            robot,
            DVec3::new(3.0, 0.0, 0.0),
            None,
            &HashMap::new(),
        );
        assert!(!solution.reached);
        // 届かなくても、できるだけ近づいて腕を伸ばしきる (残りは 3 - 2 = 1。伸びきった特異姿勢の近くは収束が遅いので 1% まで許す)
        assert!((solution.position_error - 1.0).abs() < 1e-2);
    }

    #[test]
    fn ik_respects_joint_limits() {
        let (mut chart, robot, shoulder, elbow) = planar_arm();
        chart.actuator_mut(elbow).unwrap().limits = Some([0.0, 90.0]);
        let robot = chart.robot(robot).unwrap().clone();
        // 肘を曲げる向きが1つに限られる位置
        let solution = solve_ik_to(
            &chart,
            &robot,
            DVec3::new(1.0, 1.0, 0.0),
            None,
            &HashMap::new(),
        );
        assert!(solution.reached, "{solution:?}");
        assert!((0.0..=90.0).contains(&solution.values[&elbow]));
        assert!((solution.values[&shoulder] + 90.0).abs() < 1e-2);
    }

    #[test]
    fn ik_aligns_tool_axis() {
        // 平面3リンク: 肩・肘・手首 (すべて Z 軸まわり)
        let (mut chart, _, _, elbow) = planar_arm();
        let wrist = chart.add_actuator(ActuatorKind::RotaryTable);
        {
            let a = chart.actuator_mut(wrist).unwrap();
            a.axis = Axis::Z;
            a.pivot = [0.0, 2.0, 0.0];
            a.mounted_on = Some(elbow);
        }
        let robot = chart.add_robot(wrist);
        chart.robot_mut(robot).unwrap().tcp = [0.0, 2.5, 0.0];
        let robot = chart.robot(robot).unwrap();
        let target = DVec3::new(1.5, 0.5, 0.0);
        let solution = solve_ik_to(&chart, robot, target, Some(-DVec3::Y), &HashMap::new());
        assert!(solution.reached, "{solution:?}");
        let tcp = tcp_for(&chart, robot, &solution.values);
        assert!((tcp.tool_axis + DVec3::Y).length() < 1e-2);
    }

    #[test]
    fn prismatic_joint_carries_the_arm() {
        // 直動軸 (走行軸) の上に回転関節: 腕だけでは届かない位置へ走行軸で近づく
        let mut chart = TimingChart::default();
        let track = chart.add_actuator(ActuatorKind::LinearAxis);
        let arm = chart.add_actuator(ActuatorKind::RotaryTable);
        {
            let a = chart.actuator_mut(arm).unwrap();
            a.axis = Axis::Z;
            a.mounted_on = Some(track);
        }
        chart.actuator_mut(track).unwrap().limits = Some([0.0, 5.0]);
        let robot = chart.add_robot(arm);
        chart.robot_mut(robot).unwrap().tcp = [0.0, 1.0, 0.0];
        let robot = chart.robot(robot).unwrap();
        let solution = solve_ik_to(
            &chart,
            robot,
            DVec3::new(4.0, 0.0, 0.0),
            None,
            &HashMap::new(),
        );
        assert!(solution.reached, "{solution:?}");
        assert!(solution.values[&track] > 2.9);
    }

    #[test]
    fn robot_moves_expand_into_joint_segments() {
        let (mut chart, robot, shoulder, elbow) = planar_arm();
        let point = chart
            .robot_mut(robot)
            .unwrap()
            .add_point("P1".into(), [1.0, 1.0, 0.0], None);
        chart.steps.push(Step {
            moves: vec![RobotMove {
                robot,
                point,
                duration: 2.0,
                kind: Default::default(),
            }],
            dwell: 0.5,
            ..Default::default()
        });
        let schedule = chart.schedule();
        assert_eq!(schedule.cycle_time, 2.5);
        assert!(schedule.solved_move(0, robot).unwrap().reached);
        let values = schedule.values_at(&chart, 2.0);
        let tcp = tcp_for(&chart, chart.robot(robot).unwrap(), &values);
        assert!((tcp.position - DVec3::new(1.0, 1.0, 0.0)).length() < POSITION_TOLERANCE);
        assert!(schedule.segments.contains_key(&shoulder));
        assert!(schedule.segments.contains_key(&elbow));
    }
}
