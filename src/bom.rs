//! 部品表 (BOM) (ドメイン + ユースケース)
//!
//! 表示 / 非表示にかかわらず、モデルのすべての部品を「同じ品物」ごとにまとめて数える。
//! 取り込んだ部品は「どのファイルの、どの製品か」、箱は寸法が同じものを同じ品物とみなす。
//! あわせて、アセンブリの木をそのまま並べた構成表も作る。
//! 書き出し (CSV / Excel) は `formats::bom_out` が受け持つ。

use std::collections::BTreeMap;

use crate::drawing::format_length;
use crate::file_io::source_name;
use crate::math::{Aabb, split_placement};
use crate::model::{Model, NodeId, NodeKind};

/// 部品表の1行 (品物1種類)
#[derive(Clone, Debug, PartialEq)]
pub struct BomRow {
    pub number: usize,
    /// 品名 (製品名。箱は「箱」)
    pub name: String,
    /// 種類 (箱 / 部品) と出どころ (取り込んだファイル名)
    pub kind: String,
    pub source: String,
    /// 部品の座標系での外接箱の大きさ (幅 × 高さ × 奥行)
    pub size: [f64; 3],
    pub quantity: usize,
    /// 1個あたりの体積
    pub volume: f64,
    /// 使われているアセンブリ (重複なし)
    pub used_in: Vec<String>,
}

/// 構成表の1行 (ツリーの1ノード)
#[derive(Clone, Debug, PartialEq)]
pub struct TreeRow {
    /// 深さ (最上位が 0)
    pub level: usize,
    pub name: String,
    pub kind: String,
    /// 部品なら部品表の番号
    pub bom_number: Option<usize>,
    /// ワールドでの位置
    pub position: [f64; 3],
    pub visible: bool,
}

pub struct Bom {
    pub unit: String,
    pub rows: Vec<BomRow>,
    pub tree: Vec<TreeRow>,
}

impl Bom {
    pub fn total_parts(&self) -> usize {
        self.rows.iter().map(|r| r.quantity).sum()
    }
}

/// 同じ品物かを見分ける鍵
fn item_key(model: &Model, id: NodeId) -> Option<(String, String, String, [f64; 3], f64)> {
    let node = model.get(id)?;
    let m = model.measure(id);
    let local = match &node.kind {
        // 標準部品は部品全体で1つの品物 (本体は数えない)
        NodeKind::Assembly { .. } | NodeKind::StandardBody { .. } => return None,
        NodeKind::Box(part) => part.size,
        NodeKind::Imported { shape, .. } => (shape.stats.max - shape.stats.min).to_array(),
        NodeKind::Standard(_) => standard_size(model, id),
    };
    let size = local.map(|v| (v * 1000.0).round() / 1000.0);
    Some(match &node.kind {
        NodeKind::Box(_) => ("箱".into(), "箱".into(), String::new(), size, m.volume),
        NodeKind::Imported {
            product, source, ..
        } => (
            product.clone(),
            "部品".into(),
            source_name(source),
            size,
            m.volume,
        ),
        // 種類と寸法パラメータが同じものを同じ品物とみなす
        NodeKind::Standard(spec) => (
            spec.kind.label().into(),
            "標準部品".into(),
            spec.kind
                .param_specs()
                .iter()
                .zip(&spec.params.0)
                .map(|(p, v)| format!("{} {}", p.label, format_length(*v)))
                .collect::<Vec<_>>()
                .join(" / "),
            size,
            m.volume,
        ),
        NodeKind::Assembly { .. } | NodeKind::StandardBody { .. } => return None,
    })
}

/// 標準部品の、部品の座標系での外接箱の大きさ (本体をまとめたもの)
fn standard_size(model: &Model, id: NodeId) -> [f64; 3] {
    model
        .get(id)
        .into_iter()
        .flat_map(|node| &node.children)
        .filter_map(|&child| {
            let node = model.get(child)?;
            let NodeKind::StandardBody { shape, .. } = &node.kind else {
                return None;
            };
            Some(
                Aabb {
                    min: shape.stats.min,
                    max: shape.stats.max,
                }
                .transformed(node.placement),
            )
        })
        .reduce(Aabb::union)
        .map_or([0.0; 3], |b| (b.max - b.min).to_array())
}

pub fn make_bom(model: &Model) -> Bom {
    // 鍵 → 行。最初に現れた順に番号を振る
    let mut order: Vec<String> = Vec::new();
    let mut rows: BTreeMap<String, BomRow> = BTreeMap::new();
    let mut numbers: BTreeMap<NodeId, String> = BTreeMap::new();
    let mut tree = Vec::new();
    let mut tree_ids = Vec::new();

    for &root in &model.roots {
        for id in model.subtree(root) {
            let Some(node) = model.get(id) else { continue };
            let level = model.ancestors(id).len() - 1;
            let (translation, _) = split_placement(model.world_matrix(id));
            if let Some((name, kind, source, size, volume)) = item_key(model, id) {
                let key = format!(
                    "{kind}\u{1}{source}\u{1}{name}\u{1}{}",
                    size.map(format_length).join("x")
                );
                let parent = node
                    .parent
                    .and_then(|p| model.get(p))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "(最上位)".into());
                let row = rows.entry(key.clone()).or_insert_with(|| {
                    order.push(key.clone());
                    BomRow {
                        number: 0,
                        name,
                        kind,
                        source,
                        size,
                        quantity: 0,
                        volume,
                        used_in: Vec::new(),
                    }
                });
                row.quantity += 1;
                if !row.used_in.contains(&parent) {
                    row.used_in.push(parent);
                }
                numbers.insert(id, key);
            }
            tree_ids.push(id);
            tree.push(TreeRow {
                level,
                name: node.name.clone(),
                kind: node.kind.label().into(),
                bom_number: None,
                position: translation
                    .to_array()
                    .map(|v| (v * 1e6).round() / 1e6 + 0.0),
                visible: model.is_shown(id),
            });
        }
    }
    let rows: Vec<BomRow> = order
        .iter()
        .enumerate()
        .map(|(i, key)| {
            let mut row = rows.remove(key).unwrap();
            row.number = i + 1;
            row
        })
        .collect();
    for (row, id) in tree.iter_mut().zip(tree_ids) {
        row.bom_number = numbers
            .get(&id)
            .and_then(|key| order.iter().position(|k| k == key))
            .map(|i| i + 1);
    }
    Bom {
        unit: model.unit.clone(),
        rows,
        tree,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
