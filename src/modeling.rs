//! モデリング操作 (円柱・押し出し・穴あけ・ブーリアン)
//!
//! 計算と変更の作り方は `cadsim_core::modeling`、B-rep を JSON に詰めるのは `cadsim_io::modeled`。
//! ここは JSON の詰め方を渡し、元に戻す履歴に積むだけ。

use glam::DVec3;
use truck_modeling::Solid;

pub use cadsim_core::modeling::*;
use cadsim_io::modeled::JsonSolids;
pub use cadsim_io::modeled::ModeledSourceExt;

use crate::history::History;
use crate::model::{Model, NodeId};

/// 部品の形状を、部品の座標系の B-rep で取り出す (箱とモデリングで作った部品だけ)
pub fn node_solid(model: &Model, id: NodeId) -> Result<Solid, String> {
    cadsim_core::modeling::node_solid(model, &JsonSolids, id)
}

/// 新しい部品として加える (最上位、位置 `position`)。加えたノードの id を返す
pub fn add_solid(
    model: &mut Model,
    name: String,
    solid: &Solid,
    position: DVec3,
) -> Result<NodeId, String> {
    cadsim_core::modeling::add_solid(model, &JsonSolids, name, solid, position)
}

/// 部品に穴をあける。元に戻せるよう履歴に積む
pub fn drill_hole(
    model: &mut Model,
    history: &mut History,
    id: NodeId,
    hole: &HoleSpec,
) -> Result<(), String> {
    let change = cadsim_core::modeling::drill_hole(model, &JsonSolids, id, hole)?;
    history.push(change);
    Ok(())
}

/// 部品 `a` と `b` のブーリアン。結果は `a` に入れ、`b` は消す。元に戻せるよう履歴に積む
pub fn apply_boolean(
    model: &mut Model,
    history: &mut History,
    a: NodeId,
    b: NodeId,
    op: BooleanOp,
) -> Result<(), String> {
    let change = cadsim_core::modeling::apply_boolean(model, &JsonSolids, a, b, op)?;
    history.push(change);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drilling_is_recorded_in_history() {
        let mut model = Model::default();
        let mut history = History::default();
        let id = model.add_box([100.0, 40.0, 100.0], [0.0; 3]);
        let hole = HoleSpec {
            center: DVec3::ZERO,
            axis: HoleAxis::Y,
            diameter: 20.0,
            depth: 0.0,
        };
        drill_hole(&mut model, &mut history, id, &hole).unwrap();
        assert!(history.undo_label().unwrap().contains("穴"));
    }
}
