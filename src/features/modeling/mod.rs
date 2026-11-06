//! モデリング操作 (円柱・押し出し・穴あけ・ブーリアン)
//!
//! 計算と変更の作り方は `plantwright_core::modeling`、B-rep を JSON に詰めるのは `plantwright_core::modeling::record`。
//! ここは JSON の詰め方を渡し、元に戻す履歴に積むだけ。
//!
//! 穴あけは 3D ビューで面をクリックして置く道具 ([`HoleTool`]) で行う。道具の状態と寸法の計算は
//! `plantwright_core::modeling::hole_tool`、マウスの読み方は `systems`、3D の線と文字は `view`。

pub mod panel;
pub mod systems;
pub mod view;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use glam::DVec3;
use plantwright_core::kernel::Brep;

use plantwright_core::modeling::hole_tool::HoleTool;
use plantwright_core::modeling::record::JsonSolids;
use plantwright_core::modeling::*;

use crate::document::history::History;
use crate::document::import::ImportJobs;
use crate::ui::font::install_japanese_font;
use crate::viewport::camera::keyboard_free;
use crate::viewport::gizmo::transform_gizmo_system;
use plantwright_core::model::Model;
use plantwright_core::model::NodeId;

pub struct ModelingPlugin;

impl Plugin for ModelingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoleTool>()
            .init_resource::<panel::ModelingUi>()
            .init_gizmo_group::<view::HoleGizmos>()
            .add_systems(Startup, view::configure_gizmos)
            .add_systems(
                Update,
                (
                    systems::hole_keys.run_if(keyboard_free),
                    systems::hole_input,
                    view::draw_preview,
                )
                    .chain(),
            )
            .add_systems(
                EguiPrimaryContextPass,
                view::draw_labels
                    .after(install_japanese_font)
                    // UI がビューの領域とカーソルの位置を決めた後で描く
                    .after(transform_gizmo_system),
            );
    }
}

/// 穴あけの道具を使っていない (クリックで部品を選ぶ・ギズモを出す条件)
pub fn hole_tool_off(tool: Res<HoleTool>) -> bool {
    !tool.active
}

/// 部品の形状を、部品の座標系の B-rep で取り出す (箱とモデリングで作った部品だけ)
pub fn node_solid(model: &Model, id: NodeId) -> Result<Brep, String> {
    plantwright_core::modeling::node_solid(model, &JsonSolids, id)
}

/// 新しい部品として加える (最上位、位置 `position`)。加えたノードの id を返す
pub fn add_solid(
    model: &mut Model,
    name: String,
    solid: &Brep,
    position: DVec3,
) -> Result<NodeId, String> {
    plantwright_core::modeling::add_solid(model, &JsonSolids, name, solid, position)
}

/// 部品に穴をあける。元に戻せるよう履歴に積む
pub fn drill_hole(
    model: &mut Model,
    history: &mut History,
    id: NodeId,
    hole: &HoleSpec,
) -> Result<(), String> {
    let change = plantwright_core::modeling::drill_hole(model, &JsonSolids, id, hole)?;
    history.push(change);
    Ok(())
}

/// 置きかけの穴を確定する (Enter かパネルのボタン)。うまくいけば道具をやめ、
/// 失敗したら穴はそのまま残してエラーを出す
pub fn confirm_hole(
    tool: &mut HoleTool,
    model: &mut Model,
    history: &mut History,
    jobs: &mut ImportJobs,
) {
    let Some(draft) = tool.draft else {
        return;
    };
    match drill_hole(model, history, draft.node, &draft.spec()) {
        Ok(()) => {
            tool.finish();
            jobs.messages.push("穴をあけました".into());
        }
        Err(e) => tool.error = Some(e),
    }
}

/// 部品 `a` と `b` のブーリアン。結果は `a` に入れ、`b` は消す。元に戻せるよう履歴に積む
pub fn apply_boolean(
    model: &mut Model,
    history: &mut History,
    a: NodeId,
    b: NodeId,
    op: BooleanOp,
) -> Result<(), String> {
    let change = plantwright_core::modeling::apply_boolean(model, &JsonSolids, a, b, op)?;
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
