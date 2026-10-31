//! 部品表 (BOM): 数える計算は `cadsim_core::bom`、書き出しは `cadsim_io::formats::bom_out`

pub use cadsim_core::bom::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;
    use crate::model::StepSource;
    use crate::step_import::import_step;

    #[test]
    fn same_products_are_counted_together() {
        let file = import_step(
            include_str!("../assets/sample_assembly.step"),
            "sample_assembly.step",
        )
        .unwrap();
        let mut model = Model::default();
        model.add_imported(file.root, file.unit, StepSource::Sample);
        model.add_box([100.0, 50.0, 20.0], [0.0; 3]);
        model.add_box([100.0, 50.0, 20.0], [500.0, 0.0, 0.0]);
        model.add_box([10.0, 10.0, 10.0], [900.0, 0.0, 0.0]);
        let bom = make_bom(&model);
        // サンプルは 7 部品。ブラケットの立ち上がりは左右で同じ製品
        assert_eq!(bom.total_parts(), 10);
        let boxes: Vec<&BomRow> = bom.rows.iter().filter(|r| r.kind == "箱").collect();
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].quantity, 2);
        assert_eq!(boxes[0].size, [100.0, 50.0, 20.0]);
        assert!((boxes[0].volume - 100_000.0).abs() < 1e-6);
        assert!(bom.rows.iter().any(|r| r.quantity >= 2 && r.kind == "部品"));
        // 番号は 1 から順
        for (i, row) in bom.rows.iter().enumerate() {
            assert_eq!(row.number, i + 1);
        }
        // 構成表には全ノード。部品には番号が付く
        assert!(
            bom.tree
                .iter()
                .any(|t| t.level == 0 && t.bom_number.is_none())
        );
        assert_eq!(
            bom.tree.iter().filter(|t| t.bom_number.is_some()).count(),
            10
        );
    }

    #[test]
    fn standard_parts_are_counted_as_whole_parts() {
        use crate::library::{self, PartKind};
        use bevy::math::DVec3;
        let mut model = Model::default();
        library::add_standard(&mut model, PartKind::AirCylinder, DVec3::ZERO);
        library::add_standard(&mut model, PartKind::AirCylinder, DVec3::X * 500.0);
        let bom = make_bom(&model);
        // 本体 (チューブ・ロッドなど) は数えず、同じ寸法のシリンダ2本で1行
        assert_eq!(bom.rows.len(), 1);
        assert_eq!(bom.rows[0].quantity, 2);
        assert_eq!(bom.rows[0].kind, "標準部品");
        assert!(bom.rows[0].size.iter().all(|&v| v > 0.0));
    }
}
