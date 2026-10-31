//! ロボットの移動中の干渉: 移動ごとに時刻を細かく区切って部品を動かし、
//! ロボットに載った部品と、それ以外の部品の食い込みを調べる。
//!
//! ロボットの腕どうし (関節のところで重なりがち) は調べない。
//! 画面の干渉チェックは表示中の時刻だけなので、経路の途中はここで調べ直す。

use std::{collections::HashMap, sync::Arc};

use bevy::math::DMat4;

use super::chart::{Schedule, TimingChart};
use super::pose::{RigidPose, part_poses};
use crate::simulation::{
    Pose, TriMesh,
    interference::{Body, CheckSettings, PairStatus, check_all},
};

/// 調べる部品1つ
pub struct PathBody {
    pub id: u32,
    pub name: String,
    /// 部品ローカル座標の形状
    pub mesh: Arc<TriMesh>,
    /// 設計上の配置 (ローカル → ワールド)
    pub placement: DMat4,
    /// ルートから自分までのノード (アクチュエータの動きは、自分にいちばん近いものを使う)
    pub ancestors: Vec<u32>,
}

impl PathBody {
    fn pose(&self, motion: &HashMap<u32, RigidPose>) -> Pose {
        let moved = self.ancestors.iter().rev().find_map(|id| motion.get(id));
        match moved {
            Some(m) => Pose::from_matrix(m.to_dmat4() * self.placement),
            None => Pose::from_matrix(self.placement),
        }
    }
}

/// 移動中に見つかった干渉 1 件 (同じ移動・同じ部品の組はまとめる)
#[derive(Clone, Debug, PartialEq)]
pub struct PathHit {
    /// 移動を置いたステップ (0 始まり)
    pub step: usize,
    pub point: u32,
    /// ロボットに載った部品
    pub robot_part: String,
    /// ぶつかった相手の部品
    pub other_part: String,
    /// 干渉していた最初と最後のサンプル時刻 (秒)
    pub start: f64,
    pub end: f64,
}

/// ロボット `robot` の移動すべてを `interval` 秒ごとに調べ、干渉を移動の順に返す
pub fn robot_path_interference(
    bodies: &[PathBody],
    chart: &TimingChart,
    schedule: &Schedule,
    settings: &CheckSettings,
    robot: u32,
    interval: f64,
) -> Vec<PathHit> {
    let Some(r) = chart.robot(robot) else {
        return Vec::new();
    };
    let robot_nodes: Vec<u32> = chart
        .robot_actuators(r)
        .into_iter()
        .filter_map(|id| chart.actuator(id))
        .flat_map(|a| a.parts.iter().copied())
        .collect();
    let on_robot: Vec<bool> = bodies
        .iter()
        .map(|b| b.ancestors.iter().any(|id| robot_nodes.contains(id)))
        .collect();
    // 体積は使わないので計算しない (速くする)
    let settings = CheckSettings {
        compute_overlap: false,
        ..*settings
    };
    let interval = interval.max(1e-3);

    let mut hits: Vec<PathHit> = Vec::new();
    for solved in schedule.moves.iter().filter(|m| m.robot == robot) {
        let count = (solved.duration / interval).ceil().max(1.0) as usize;
        for k in 0..=count {
            let t = solved.start + solved.duration * k as f64 / count as f64;
            let poses = part_poses(chart, schedule, t);
            let inputs: Vec<Body> = bodies
                .iter()
                .map(|body| Body {
                    id: body.id,
                    mesh: &body.mesh,
                    pose: body.pose(&poses),
                })
                .collect();
            for pair in check_all(&inputs, &settings) {
                if !matches!(pair.status, PairStatus::Interfering { .. }) {
                    continue;
                }
                let index = |id: u32| bodies.iter().position(|b| b.id == id);
                let (Some(a), Some(b)) = (index(pair.a), index(pair.b)) else {
                    continue;
                };
                // ロボットの部品とそれ以外の組だけを見る
                let (mine, other) = match (on_robot[a], on_robot[b]) {
                    (true, false) => (a, b),
                    (false, true) => (b, a),
                    _ => continue,
                };
                let (robot_part, other_part) = (&bodies[mine].name, &bodies[other].name);
                match hits.iter_mut().find(|h| {
                    h.step == solved.step
                        && &h.robot_part == robot_part
                        && &h.other_part == other_part
                }) {
                    Some(hit) => hit.end = t,
                    None => hits.push(PathHit {
                        step: solved.step,
                        point: solved.point,
                        robot_part: robot_part.clone(),
                        other_part: other_part.clone(),
                        start: t,
                        end: t,
                    }),
                }
            }
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::{ActuatorKind, Axis, MoveKind, RobotMove, Step};
    use bevy::math::DVec3;

    /// 原点を中心にした、辺の長さ `size` の立方体
    fn cube(size: f64) -> Arc<TriMesh> {
        let corner = |x: usize, y: usize, z: usize| {
            (DVec3::new(x as f64, y as f64, z as f64) - DVec3::splat(0.5)) * size
        };
        let faces = [
            [(0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)],
            [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)],
            [(0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)],
            [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)],
            [(0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)],
            [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)],
        ];
        let mut triangles = Vec::new();
        for face in faces {
            let [a, b, c, d] = face.map(|(x, y, z)| corner(x, y, z));
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
        Arc::new(TriMesh::new(triangles))
    }

    /// Z 軸まわりに回る 1 関節のアーム (部品 1)。+X 側に柱 (部品 2) があり、
    /// -90° 回すと腕の先が柱に当たる。
    #[test]
    fn arm_hitting_a_post_is_reported_once_per_move() {
        let mut chart = TimingChart::default();
        let joint = chart.add_actuator(ActuatorKind::RotaryTable);
        {
            let a = chart.actuator_mut(joint).unwrap();
            a.axis = Axis::Z;
            a.parts = vec![1];
        }
        let robot = chart.add_robot(joint);
        let point = {
            let r = chart.robot_mut(robot).unwrap();
            r.tcp = [0.0, 2.0, 0.0];
            r.add_point("横".into(), [2.0, 0.0, 0.0], None)
        };
        chart.steps = vec![Step {
            moves: vec![RobotMove {
                robot,
                point,
                duration: 1.0,
                kind: MoveKind::Joint,
            }],
            ..Default::default()
        }];
        let schedule = chart.schedule();
        let bodies = vec![
            PathBody {
                id: 1,
                name: "腕".into(),
                mesh: cube(0.4),
                placement: DMat4::from_translation(DVec3::new(0.0, 1.8, 0.0)),
                ancestors: vec![1],
            },
            PathBody {
                id: 2,
                name: "柱".into(),
                mesh: cube(0.4),
                placement: DMat4::from_translation(DVec3::new(1.8, 0.0, 0.0)),
                ancestors: vec![2],
            },
        ];
        let hits = robot_path_interference(
            &bodies,
            &chart,
            &schedule,
            &CheckSettings::default(),
            robot,
            0.05,
        );
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!(hits[0].robot_part, "腕");
        assert_eq!(hits[0].other_part, "柱");
        assert!(hits[0].end > 0.9, "移動の終わりで当たっている: {hits:?}");
    }
}
