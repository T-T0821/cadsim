//! タイミングチャートの動きを、書き出し用のアニメーション (glTF / USD などのキーフレーム) にする。
//!
//! 1サイクルを一定間隔で標本化し、動くノードごとに「親から見た配置」のキーを作る。
//! アセンブリに割り当てた動きは、シーンの親子関係でそのまま子孫に伝わる。

use std::collections::HashMap;

use bevy::math::{DAffine3, DMat4};

use super::{SimulationPose, pose::part_poses};
use plantwright_core::formats::Animation;
use plantwright_core::formats::NodeTransform;
use plantwright_core::formats::Track;
use plantwright_core::formats::model_io::meters_per_unit;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;
use plantwright_core::motion::chart::Schedule;
use plantwright_core::motion::chart::TimingChart;

/// 1秒あたりのキーの数
const KEYS_PER_SECOND: f64 = 30.0;

/// チャートの1サイクルをアニメーションにする。`index_of` はモデルのノード → シーンの番号、
/// `scene_parents` はシーンの各ノードの親。動くノードがなければ None。
pub fn chart_animation(
    chart: &TimingChart,
    schedule: &Schedule,
    model: &Model,
    index_of: &HashMap<NodeId, usize>,
    scene_parents: &[Option<usize>],
) -> Option<Animation> {
    let cycle = schedule.cycle_time;
    if cycle <= 0.0 || chart.actuators.is_empty() {
        return None;
    }
    let mut node_of = vec![None; scene_parents.len()];
    for (&id, &index) in index_of {
        if let Some(slot) = node_of.get_mut(index) {
            *slot = Some(id);
        }
    }
    let to_meters = meters_per_unit(&model.unit);
    let count = (cycle * KEYS_PER_SECOND).ceil().max(1.0) as usize;
    let times: Vec<f64> = (0..=count)
        .map(|i| cycle * i as f64 / count as f64)
        .collect();
    let poses: Vec<SimulationPose> = times
        .iter()
        .map(|&t| SimulationPose {
            time: t,
            step: None,
            parts: part_poses(chart, schedule, t),
            ..Default::default()
        })
        .collect();

    // ノードの動き (自分か祖先に割り当てられたもの)
    let motion =
        |pose: &SimulationPose, index: usize| node_of[index].and_then(|id| pose.pose_of(model, id));
    // ワールドでの配置 (モデルの単位)
    let world = |pose: &SimulationPose, index: usize| {
        let Some(id) = node_of[index] else {
            return DMat4::IDENTITY;
        };
        let design = model.world_matrix(id);
        pose.pose_of(model, id)
            .map_or(design, |p| p.to_dmat4() * design)
    };

    let mut tracks = Vec::new();
    for (index, &parent) in scene_parents.iter().enumerate() {
        // 親と同じ動きなら、シーンの親子関係で付いていくのでキーは要らない
        let moves_alone = poses
            .iter()
            .any(|pose| motion(pose, index) != parent.and_then(|p| motion(pose, p)));
        if !moves_alone {
            continue;
        }
        let keys = times
            .iter()
            .zip(&poses)
            .map(|(&t, pose)| {
                let parent_world = parent.map_or(DMat4::IDENTITY, |p| world(pose, p));
                let local = parent_world.inverse() * world(pose, index);
                let mut transform = NodeTransform::from_affine(DAffine3::from_mat4(local));
                transform.translation *= to_meters;
                (t, transform)
            })
            .collect();
        tracks.push(Track { node: index, keys });
    }
    (!tracks.is_empty()).then(|| Animation {
        name: "タイミングチャート".into(),
        tracks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::demo::load_demo;
    use plantwright_core::formats::model_io::model_to_scene;

    #[test]
    fn demo_cycle_becomes_keyframes() {
        let mut model = Model::default();
        let mut chart = TimingChart::default();
        load_demo(&mut model, &mut chart, &mut Default::default());
        let schedule = chart.schedule();
        let (scene, index_of) = model_to_scene(&model, &[]);
        let parents: Vec<_> = scene.nodes.iter().map(|n| n.parent).collect();
        let animation = chart_animation(&chart, &schedule, &model, &index_of, &parents).unwrap();

        // 割り当てた部品 (キャリッジ・ロッド・チャック本体・固定爪・可動爪) だけにキーができる
        let moving: Vec<&str> = animation
            .tracks
            .iter()
            .map(|t| scene.nodes[t.node].name.as_str())
            .collect();
        assert_eq!(moving.len(), 5, "{moving:?}");
        assert!(moving.contains(&"チャック本体"));
        assert!(!moving.contains(&"供給コンベア"));
        assert!((animation.duration() - schedule.cycle_time).abs() < 1e-9);

        // どの時刻でも、キーの姿勢 (メートル) がシミュレーションの位置と一致する
        let to_meters = meters_per_unit(&model.unit);
        for track in &animation.tracks {
            let id = *index_of.iter().find(|(_, i)| **i == track.node).unwrap().0;
            let n = track.keys.len();
            for &t in &[
                track.keys[0].0,
                track.keys[n * 2 / 5].0,
                track.keys[n - 1].0,
            ] {
                let pose = SimulationPose {
                    time: t,
                    step: None,
                    parts: part_poses(&chart, &schedule, t),
                    ..Default::default()
                };
                let expected =
                    pose.pose_of(&model, id).unwrap().to_dmat4() * model.world_matrix(id);
                let got = track.sample(t).unwrap();
                let want = expected.w_axis.truncate() * to_meters;
                assert!((got.translation - want).length() < 1e-6, "{t}: {got:?}");
            }
        }
    }

    #[test]
    fn no_animation_without_actuators() {
        let model = Model::default();
        let chart = TimingChart::default();
        let schedule = chart.schedule();
        assert!(chart_animation(&chart, &schedule, &model, &HashMap::new(), &[]).is_none());
    }
}
