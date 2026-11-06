//! モデリング操作 (円柱・押し出し・穴あけ・ブーリアン) と、形を調べる機能 (断面・質量・修復の案内)
//!
//! 計算と変更の作り方は `plantwright_core::modeling`、B-rep を JSON に詰めるのは `plantwright_core::modeling::record`。
//! ここは JSON の詰め方を渡し、元に戻す履歴に積むだけ。
//!
//! 穴あけは 3D ビューで面をクリックして置く道具 ([`HoleTool`]) で行う。道具の状態と寸法の計算は
//! `plantwright_core::modeling::hole_tool`、マウスの読み方は `systems`、3D の線と文字は `view`。
//!
//! 調べる機能も同じ並び: 断面 ([`SectionTool`]、平面を 3D でドラッグ)、質量・重心 ([`MassCache`]、
//! プロパティの欄と 3D の球)、STEP の修復の案内 ([`RepairNotice`]、ステータスバー)。

pub mod panel;
pub mod systems;
pub mod view;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;
use glam::DVec3;
use plantwright_core::kernel::Brep;

use plantwright_core::modeling::extrude_tool::ExtrudeTool;
use plantwright_core::modeling::features::{Base, Feature, ShapeHistory};
use plantwright_core::modeling::hole_tool::HoleTool;
use plantwright_core::modeling::mass::MassCache;
use plantwright_core::modeling::record::JsonSolids;
use plantwright_core::modeling::repair::RepairNotice;
use plantwright_core::modeling::section_tool::SectionTool;
use plantwright_core::modeling::*;
use plantwright_core::pointer::PointerLock;

use crate::document::history::History;
use crate::document::import::ImportJobs;
use crate::document::selection::Selection;
use crate::ui::font::install_japanese_font;
use crate::viewport::camera::keyboard_free;
use crate::viewport::gizmo::transform_gizmo_system;
use plantwright_core::model::{Model, NodeId, NodeKind, StepSource};

pub struct ModelingPlugin;

impl Plugin for ModelingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HoleTool>()
            .init_resource::<ExtrudeTool>()
            .init_resource::<SectionTool>()
            .init_resource::<MassCache>()
            .init_resource::<RepairNotice>()
            .init_resource::<panel::ModelingUi>()
            .init_gizmo_group::<view::HoleGizmos>()
            .init_gizmo_group::<view::SectionGizmos>()
            .add_systems(Startup, view::configure_gizmos)
            .add_systems(
                Update,
                (
                    systems::hole_keys.run_if(keyboard_free),
                    systems::hole_input,
                    view::draw_preview,
                    extrude_lock,
                    systems::extrude_keys.run_if(keyboard_free),
                    systems::extrude_input,
                    view::draw_extrude_preview,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                (
                    systems::repair_notice,
                    systems::section_keys.run_if(keyboard_free),
                    systems::section_input,
                    systems::section_refresh,
                    view::draw_section,
                    view::draw_centroid,
                )
                    .chain(),
            )
            .add_systems(
                EguiPrimaryContextPass,
                (view::draw_labels, view::draw_section_labels)
                    .after(install_japanese_font)
                    // UI がビューの領域とカーソルの位置を決めた後で描く
                    .after(transform_gizmo_system),
            );
    }
}

/// 穴あけ・断面の道具を使っていない (クリックで部品を選ぶ・ギズモを出す条件)
pub fn hole_tool_off(tool: Res<HoleTool>, section: Res<SectionTool>) -> bool {
    !tool.active && !section.active
}

/// 押し出しの道具を使っている間は、クリックを道具のものにする (選択・ギズモ・右クリックメニューを止める)
fn extrude_lock(tool: Res<ExtrudeTool>, mut lock: ResMut<PointerLock>) {
    if tool.active {
        if lock.held_by() != Some("extrude") {
            lock.hold("extrude");
        }
    } else if lock.held_by() == Some("extrude") {
        lock.release("extrude");
    }
}

/// 描いた形を押し出して部品にする (Enter かパネルのボタン)。うまくいけば道具をやめて選び、
/// 失敗したら形はそのまま残してエラーを出す
pub fn confirm_extrude(
    tool: &mut ExtrudeTool,
    model: &mut Model,
    history: &mut History,
    selection: &mut Selection,
    jobs: &mut ImportJobs,
) {
    if !tool.closed {
        return;
    }
    let (outline, position) = tool.outline();
    let name = format!("押し出し {}", model.roots.len() + 1);
    let result =
        extrude(&outline, tool.height).and_then(|solid| add_solid(model, name, &solid, position));
    match result {
        Ok(id) => {
            let label = format!("{} を追加", model.path_label(id));
            history.record_added(model, id, label);
            selection.select(id);
            selection.reveal_in_tree = true;
            tool.finish();
            jobs.messages.push("押し出しの形を作りました".into());
        }
        Err(e) => tool.error = Some(e),
    }
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

/// 部品の形の編集履歴 (モデリングで作った部品だけ)。出発点を覚えていなければ None
pub fn shape_history(model: &Model, id: NodeId) -> Option<ShapeHistory> {
    match &model.get(id)?.kind {
        NodeKind::Imported {
            source: StepSource::Modeled(source),
            ..
        } => Some(source.history()).filter(ShapeHistory::editable),
        _ => None,
    }
}

/// 編集履歴の `index` 番目を `feature` に差し替えて形を作り直す。元に戻せるよう履歴に積む
pub fn edit_feature(
    model: &mut Model,
    history: &mut History,
    id: NodeId,
    index: usize,
    feature: Feature,
) -> Result<(), String> {
    let change = plantwright_core::modeling::edit_feature(model, &JsonSolids, id, index, feature)?;
    history.push(change);
    Ok(())
}

/// 編集履歴の `index` 番目を消して形を作り直す。元に戻せるよう履歴に積む
pub fn remove_feature(
    model: &mut Model,
    history: &mut History,
    id: NodeId,
    index: usize,
) -> Result<(), String> {
    let change = plantwright_core::modeling::remove_feature(model, &JsonSolids, id, index)?;
    history.push(change);
    Ok(())
}

/// 編集履歴の出発点 (箱の寸法) を直して形を作り直す。元に戻せるよう履歴に積む
pub fn edit_base(
    model: &mut Model,
    history: &mut History,
    id: NodeId,
    base: Base,
) -> Result<(), String> {
    let change = plantwright_core::modeling::edit_base(model, &JsonSolids, id, base)?;
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
