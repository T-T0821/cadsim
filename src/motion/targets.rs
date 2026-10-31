//! モデルの部品を、センサーの検知対象 (向きつきの直方体) に変換する
//!
//! Bevy に依存しない純粋な計算。部品の形は直方体で近似する
//! (箱はそのまま、取り込んだ部品は部品座標系での外接箱)。

use bevy::math::{DQuat, DVec3};

use super::chart::{Schedule, TimingChart};
use super::sensor::{DesignTarget, TargetBox};
use crate::math::euler_deg_to_quat;
use crate::model::{Model, NodeId, NodeKind};

/// 設計位置の部品を、センサーの検知対象にする。
/// アクチュエータが動かす部品 (またはそれを含むアセンブリ) には、そのアクチュエータを付ける。
pub fn design_targets(model: &Model, chart: &TimingChart) -> Vec<DesignTarget> {
    let actuator_of = |id: NodeId| {
        model.ancestors(id).iter().rev().find_map(|node| {
            chart
                .actuators
                .iter()
                .find(|a| a.parts.contains(node))
                .map(|a| a.id)
        })
    };
    model
        .roots
        .iter()
        .flat_map(|&root| model.subtree(root))
        .filter_map(|id| {
            Some(DesignTarget {
                shape: part_box(model, id)?,
                actuator: actuator_of(id),
            })
        })
        .collect()
}

/// 部品をワールド座標の直方体にする。箱はそのまま、取り込んだ部品は部品座標系での外接箱。
fn part_box(model: &Model, id: NodeId) -> Option<TargetBox> {
    let (center, rotation, half_size) = match &model.get(id)?.kind {
        NodeKind::Assembly { .. } => return None,
        NodeKind::Box(part) => (
            DVec3::from(part.position),
            euler_deg_to_quat(part.rotation_deg),
            DVec3::from(part.size) / 2.0,
        ),
        NodeKind::Imported { shape, .. } => (
            (shape.stats.min + shape.stats.max) / 2.0,
            DQuat::IDENTITY,
            (shape.stats.max - shape.stats.min) / 2.0,
        ),
    };
    // 配置は剛体 (+ 一様な拡大) とみなす
    let (scale, world_rotation, _) = model.world_matrix(id).to_scale_rotation_translation();
    Some(TargetBox {
        id,
        center: model.world_matrix(id).transform_point3(center),
        rotation: world_rotation * rotation,
        half_size: half_size * scale,
    })
}

/// 部品以外の検知対象 (コンベア上のワークなど) を、時刻 t の位置で返す。
/// 時刻表の t より前の部分だけから決まるものなら、インターロックの待ちにも使える。
pub fn extra_targets_at(_schedule: &Schedule, _t: f64) -> Vec<TargetBox> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::motion::chart::ActuatorKind;

    #[test]
    fn box_part_becomes_world_box_with_its_actuator() {
        let mut model = Model::default();
        let id = model.add_box([200.0, 100.0, 50.0], [10.0, 20.0, 30.0]);
        let mut chart = TimingChart::default();
        let axis = chart.add_actuator(ActuatorKind::LinearAxis);
        chart.actuator_mut(axis).unwrap().parts = vec![id];

        let targets = design_targets(&model, &chart);
        assert_eq!(targets.len(), 1);
        let target = targets[0];
        assert_eq!(target.actuator, Some(axis));
        assert!((target.shape.center - DVec3::new(10.0, 20.0, 30.0)).length() < 1e-9);
        assert!((target.shape.half_size - DVec3::new(100.0, 50.0, 25.0)).length() < 1e-9);
    }
}
