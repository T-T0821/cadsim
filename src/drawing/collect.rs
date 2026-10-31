//! モデルから図面の入力を集める (ユースケース)
//!
//! 表示中の部品の形状 (面の三角形と稜線) と、寸法を付けるまとまりを取り出す。
//! まとまりは最上位のノード。最上位がアセンブリ1つだけなら、その子を1台ずつの設備とみなす。

use glam::DVec3;
use truck_meshalgo::prelude::*;

use super::{DrawingInput, DrawingItem, DrawingPart};
use crate::model::{Model, NodeId, NodeKind};
use crate::site::underlay::unit_in_mm;

/// 箱のテッセレーションの許容誤差 (モデル単位)
const BOX_TOL: f64 = 0.005;

fn to_dvec(p: &Point3) -> DVec3 {
    DVec3::new(p.x, p.y, p.z)
}

fn polygon_triangles(poly: &PolygonMesh) -> Vec<[DVec3; 3]> {
    let positions = poly.positions();
    poly.faces()
        .triangle_iter()
        .map(|tri| tri.map(|v| to_dvec(&positions[v.pos])))
        .collect()
}

/// 部品の形状 (部品の座標系)
fn part_geometry(kind: &NodeKind) -> Option<(Vec<Vec<[DVec3; 3]>>, Vec<Vec<DVec3>>)> {
    match kind {
        NodeKind::Assembly { .. } => None,
        NodeKind::Box(part) => {
            let meshed = part.build_solid().triangulation(BOX_TOL);
            let shell = meshed.boundaries().first()?.clone();
            let edges = shell
                .edge_iter()
                .map(|edge| edge.curve().iter().map(to_dvec).collect())
                .collect();
            let faces = shell
                .face_iter()
                .map(|face| polygon_triangles(&face.surface().unwrap_or_default()))
                .collect();
            Some((faces, edges))
        }
        NodeKind::Imported { shape, .. } => Some((
            shape.faces.iter().map(polygon_triangles).collect(),
            shape
                .edges
                .iter()
                .map(|line| line.iter().map(to_dvec).collect())
                .collect(),
        )),
    }
}

/// 寸法と名前を付けるまとまり
fn layout_nodes(model: &Model) -> Vec<NodeId> {
    let shown: Vec<NodeId> = model
        .roots
        .iter()
        .copied()
        .filter(|&r| model.is_shown(r))
        .collect();
    if let [only] = shown.as_slice()
        && let Some(node) = model.get(*only)
        && !node.kind.is_part()
    {
        return node
            .children
            .iter()
            .copied()
            .filter(|&c| model.is_shown(c))
            .collect();
    }
    shown
}

/// 表示中のものから図面の入力を作る。何も表示されていなければ None
pub fn drawing_input(model: &Model, title: String, date: String) -> Option<DrawingInput> {
    let mut parts = Vec::new();
    for &root in &model.roots {
        for id in model.subtree(root) {
            if !model.is_shown(id) {
                continue;
            }
            let Some((faces, edges)) = model.get(id).and_then(|n| part_geometry(&n.kind)) else {
                continue;
            };
            parts.push(DrawingPart {
                faces,
                edges,
                world: model.world_matrix(id),
            });
        }
    }
    let items: Vec<DrawingItem> = layout_nodes(model)
        .into_iter()
        .filter_map(|id| {
            let bounds = model.world_bounds(id)?;
            Some(DrawingItem {
                name: model.get(id)?.name.clone(),
                min: bounds.min,
                max: bounds.max,
            })
        })
        .collect();
    if parts.is_empty() || items.is_empty() {
        return None;
    }
    Some(DrawingInput {
        title,
        unit: model.unit.clone(),
        unit_mm: unit_in_mm(&model.unit),
        parts,
        items,
        date,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drawing::{DrawingOptions, Layer, make_sheet};

    #[test]
    fn boxes_become_a_drawing() {
        let mut model = Model::default();
        model.add_box([1000.0, 500.0, 600.0], [0.0, 250.0, 0.0]);
        model.add_box([400.0, 800.0, 400.0], [1500.0, 400.0, 0.0]);
        let input = drawing_input(&model, "ライン".into(), "2026-10-31".into()).unwrap();
        assert_eq!(input.parts.len(), 2);
        assert_eq!(input.items.len(), 2);
        let sheet = make_sheet(&input, &DrawingOptions::default());
        assert!(sheet.lines.iter().any(|l| l.0 == Layer::Outline));
        // 全体の幅 -500 〜 1700 = 2200
        assert!(sheet.texts.iter().any(|t| t.text == "2200"), "{:?}", sheet.texts.iter().map(|t| &t.text).collect::<Vec<_>>());
        assert!(drawing_input(&Model::default(), String::new(), String::new()).is_none());
    }
}
