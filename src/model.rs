//! 設計データ (中身は `cadsim_core::model`) を Bevy の Resource で包んだものと、選択状態

use bevy::prelude::*;

pub use cadsim_core::model::{
    BoxPart, DEFAULT_PART_COLOR, ModeledSource, NodeId, NodeKind, StepSource,
};

/// 設計データ。`cadsim_core::model::Model` を Bevy の Resource にするための包み
#[derive(Resource, Default, Deref, DerefMut)]
pub struct Model(pub cadsim_core::model::Model);

impl std::borrow::Borrow<cadsim_core::model::Model> for Model {
    fn borrow(&self) -> &cadsim_core::model::Model {
        &self.0
    }
}

impl std::borrow::BorrowMut<cadsim_core::model::Model> for Model {
    fn borrow_mut(&mut self) -> &mut cadsim_core::model::Model {
        &mut self.0
    }
}

/// いま選んでいるもの
#[derive(Resource, Default)]
pub struct Selection {
    /// 選択中のノード (部品でもアセンブリでもよい)
    pub node: Option<NodeId>,
    /// ビューで面をクリックしたときの面番号
    pub face: Option<usize>,
    /// ツリーの行にカーソルが乗っているノード (ビューで薄く強調する)
    pub hovered: Option<NodeId>,
    /// ビューで選んだとき、ツリーをそこまで開いてスクロールする
    pub reveal_in_tree: bool,
}

impl Selection {
    pub fn select(&mut self, node: NodeId) {
        self.node = Some(node);
        self.face = None;
    }

    pub fn clear(&mut self) {
        self.node = None;
        self.face = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step_import::{ImportNode, import_step};
    use bevy::math::{DMat4, DVec3};

    fn sample_model() -> (Model, NodeId) {
        let file = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_assembly.step",
        )
        .unwrap();
        let mut model = Model::default();
        let root = model.add_imported(file.root, file.unit, StepSource::Sample);
        (model, root)
    }

    fn find(model: &Model, name: &str) -> NodeId {
        model
            .roots
            .iter()
            .flat_map(|&root| model.subtree(root))
            .find(|&id| model.get(id).is_some_and(|n| n.name == name))
            .unwrap()
    }

    #[test]
    fn replace_geometry_keeps_ids_and_lists_unmatched() {
        let (mut model, root) = sample_model();
        let moved = DMat4::from_translation(DVec3::new(500.0, 0.0, 0.0));
        model.get_mut(root).unwrap().placement = moved;
        let knob = find(&model, "Knob_1");
        let shaft = find(&model, "Shaft_1");
        let left = find(&model, "Bracket_L");
        let upright_l = model.get(left).unwrap().children[0];

        // 取り込み直した版: Knob の名前が変わり、Shaft がなくなり、Bracket_L が少し動いた
        let mut new = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_v2.step",
        )
        .unwrap()
        .root;
        new.children.retain(|c| c.name != "Shaft_1");
        for child in &mut new.children {
            match child.name.as_str() {
                "Knob_1" => child.name = "Knob_rev2".into(),
                "Bracket_L" => {
                    child.placement = DMat4::from_translation(DVec3::Y) * child.placement
                }
                _ => {}
            }
        }
        let left_placement = new
            .children
            .iter()
            .find(|c| c.name == "Bracket_L")
            .unwrap()
            .placement;

        let report = model
            .replace_geometry(root, new, StepSource::File("v2.step".into()))
            .unwrap();
        assert_eq!(report.unmatched, vec![shaft]);
        assert!(report.added.is_empty());
        // ID はそのまま、名前と配置は新しい版になる
        assert_eq!(model.get(knob).unwrap().name, "Knob_rev2");
        assert_eq!(model.get(left).unwrap().placement, left_placement);
        assert_eq!(model.get(left).unwrap().children[0], upright_l);
        // 一番上の配置はユーザーが置いた位置のまま
        assert_eq!(model.get(root).unwrap().placement, moved);
        // 古い Shaft は後ろに残る
        assert_eq!(model.get(root).unwrap().children.last(), Some(&shaft));
        assert_eq!(model.measure(root).part_count, 7);
        assert!(matches!(
            &model.get(knob).unwrap().kind,
            NodeKind::Imported { source: StepSource::File(p), .. } if p.ends_with("v2.step")
        ));
    }

    #[test]
    fn replace_geometry_adds_new_parts_under_matching_parent() {
        let (mut model, root) = sample_model();
        let before = model.subtree(root).len();
        let mut new = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_v2.step",
        )
        .unwrap()
        .root;
        let extra = ImportNode {
            name: "Sensor_1".into(),
            product: "Sensor".into(),
            placement: DMat4::IDENTITY,
            children: Vec::new(),
            shape: new.children[0].shape.clone(),
        };
        let bracket = new
            .children
            .iter_mut()
            .find(|c| c.name == "Bracket_R")
            .unwrap();
        bracket.children.push(extra);
        let report = model
            .replace_geometry(root, new, StepSource::Sample)
            .unwrap();
        assert_eq!(report.added.len(), 1);
        assert!(report.unmatched.is_empty());
        assert_eq!(model.subtree(root).len(), before + 1);
        let added = model.get(report.added[0]).unwrap();
        assert_eq!(added.name, "Sensor_1");
        assert_eq!(added.parent, Some(find(&model, "Bracket_R")));
    }

    #[test]
    fn assembly_bounds_cover_all_parts() {
        let (model, root) = sample_model();
        // STEP の Z 上は Y 上に回して取り込む
        let size = model.world_bounds(root).unwrap().size();
        // X: 軸 -130..130 とノブ 16 → 276 / 高さ: ベース下端 -20 〜 ノブ上端 70 + 22
        // 円柱の外接箱はメッシュから求めるので少しだけ小さくなる
        assert!((size.x - 276.0).abs() < 1e-6, "{size}");
        assert!((size.y - 112.0).abs() < 0.1, "{size}");
        assert!((size.z - 100.0).abs() < 1e-6, "{size}");
        assert_eq!(model.measure(root).part_count, 7);
    }

    #[test]
    fn hiding_assembly_hides_children() {
        let (mut model, _) = sample_model();
        let bracket = find(&model, "Bracket_L");
        let upright = model.get(bracket).unwrap().children[0];
        assert!(model.is_shown(upright));
        model.get_mut(bracket).unwrap().visible = false;
        assert!(!model.is_shown(upright));
    }

    #[test]
    fn remove_subtree() {
        let (mut model, root) = sample_model();
        model.dirty.clear();
        let bracket = find(&model, "Bracket_R");
        model.remove(bracket);
        assert_eq!(model.measure(root).part_count, 5);
        assert_eq!(model.dirty.len(), 3);
    }

    #[test]
    fn visit_parts_matches_per_node_queries() {
        let (mut model, root) = sample_model();
        let bracket = find(&model, "Bracket_L");
        model.get_mut(bracket).unwrap().visible = false;
        model.get_mut(bracket).unwrap().color = Some([1.0, 0.0, 0.0]);
        let mut count = 0;
        model.visit_parts(|v| {
            count += 1;
            assert_eq!(v.path, model.ancestors(v.id).as_slice());
            assert!(v.world.abs_diff_eq(model.world_matrix(v.id), 1e-12));
            assert_eq!(v.shown, model.is_shown(v.id));
            assert_eq!(v.color, model.display_color(v.id));
        });
        assert_eq!(count, model.measure(root).part_count);
    }

    #[test]
    fn color_inherits_from_assembly() {
        let (mut model, _) = sample_model();
        let bracket = find(&model, "Bracket_L");
        let upright = model.get(bracket).unwrap().children[0];
        assert_eq!(model.display_color(upright), [0.4, 0.62, 0.48]);
        model.get_mut(bracket).unwrap().color = Some([1.0, 0.0, 0.0]);
        assert_eq!(model.display_color(upright), [1.0, 0.0, 0.0]);
    }
}
