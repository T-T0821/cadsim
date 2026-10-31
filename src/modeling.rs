//! モデリング操作 (ドメイン + ユースケース): 円柱・押し出し・穴あけ・ブーリアン
//!
//! 形状は truck の B-rep (`truck_modeling::Solid`, f64) で作る。ブーリアン (和・差・積) は
//! `truck_shapeops` を使う。作った形状はモデルの「部品」として加え、プロジェクトファイルには
//! B-rep を JSON にして埋め込む ([`ModeledSource`])。開き直すとそこから形状を作り直すので、
//! 続けて穴をあけたりブーリアンをかけたりできる。
//!
//! 演算できるのは箱とここで作った部品だけ。STEP などから取り込んだ部品は、
//! 曲線・曲面の種類が違うので対象外 (エラーで知らせる)。

use std::f64::consts::TAU;
use std::sync::Arc;

use glam::{DMat4, DVec2, DVec3};
use truck_meshalgo::prelude::*;
use truck_modeling::{Solid, Wire, builder};

use crate::history::ModelChange;
use crate::math::euler_deg_to_quat;
use crate::model::{BoxPart, Model, ModeledSource, NodeId, NodeKind, StepSource};
use crate::part_mesh::PartMesh;
use crate::step_import::{ImportNode, ImportedFile, ImportedShape, mesh_stats, next_shape_id};

/// ブーリアンの許容誤差 (形状の大きさに対する割合)
const BOOLEAN_TOL_RATIO: f64 = 2e-4;

/// `ModeledSource` (B-rep を JSON で持つ) と truck の `Solid` の変換
pub trait ModeledSourceExt: Sized {
    fn new(name: String, solid: &Solid) -> Result<Self, String>;
    fn solid(&self) -> Result<Solid, String>;
    fn to_imported(&self) -> Result<ImportedFile, String>;
}

impl ModeledSourceExt for ModeledSource {
    fn new(name: String, solid: &Solid) -> Result<Self, String> {
        let json =
            serde_json::to_string(solid).map_err(|e| format!("形状を保存できません: {e}"))?;
        Ok(Self {
            id: next_shape_id(),
            name,
            json,
        })
    }

    fn solid(&self) -> Result<Solid, String> {
        serde_json::from_str(&self.json).map_err(|e| format!("形状を読めませんでした: {e}"))
    }

    /// モデルに取り込める形 (部品1つの木) にする
    fn to_imported(&self) -> Result<ImportedFile, String> {
        let shape = shape_from_solid(&self.solid()?);
        Ok(ImportedFile {
            root: ImportNode {
                name: self.name.clone(),
                product: self.name.clone(),
                placement: DMat4::IDENTITY,
                children: Vec::new(),
                shape: Some(Arc::new(shape)),
            },
            unit: String::new(),
            warnings: Vec::new(),
            animations: Vec::new(),
        })
    }
}

fn matrix(m: DMat4) -> Matrix4 {
    Matrix4::from(m.to_cols_array_2d())
}

/// 底面の中心を原点に置いた、Y 方向に立つ円柱
pub fn cylinder(radius: f64, height: f64) -> Result<Solid, String> {
    if radius <= 0.0 || height <= 0.0 {
        return Err("半径と高さは 0 より大きくしてください".into());
    }
    // 4つの 1/4 円でつくる。X・Z の端に頂点が来るので、メッシュの外接箱が実寸どおりになる
    let at = |angle: f64| Point3::new(radius * angle.cos(), 0.0, -radius * angle.sin());
    let vertices: Vec<_> = (0..4)
        .map(|i| builder::vertex(at(TAU * i as f64 / 4.0)))
        .collect();
    let circle: Wire = (0..4)
        .map(|i| {
            let transit = at(TAU * (i as f64 + 0.5) / 4.0);
            builder::circle_arc(&vertices[i], &vertices[(i + 1) % 4], transit)
        })
        .collect();
    let disk =
        builder::try_attach_plane(&[circle]).map_err(|e| format!("円を作れません: {e:?}"))?;
    let disk = oriented(disk, Vector3::unit_y());
    Ok(builder::tsweep(&disk, Vector3::new(0.0, height, 0.0)))
}

/// 面の向きを `normal` にそろえる (押し出す向きと逆なら裏返す)
fn oriented(mut face: truck_modeling::Face, normal: Vector3) -> truck_modeling::Face {
    use truck_modeling::ParametricSurface3D;
    let surface = face.oriented_surface();
    let n = surface.normal(0.5, 0.5);
    if n.dot(normal) < 0.0 {
        face.invert();
    }
    face
}

/// 床の上の多角形 (上から見た X, Z ではなく、図面と同じ「右 +X・上 -Z」の (u, v)) を Y 方向に押し出す
pub fn extrude(outline: &[DVec2], height: f64) -> Result<Solid, String> {
    let mut points: Vec<DVec2> = Vec::new();
    for &p in outline {
        if points.last().is_none_or(|q| q.distance(p) > 1e-9) {
            points.push(p);
        }
    }
    if points.len() > 2 && points[0].distance(*points.last().unwrap_or(&points[0])) < 1e-9 {
        points.pop();
    }
    if points.len() < 3 {
        return Err("頂点を3つ以上指定してください".into());
    }
    if height <= 0.0 {
        return Err("高さは 0 より大きくしてください".into());
    }
    let area: f64 = (0..points.len())
        .map(|i| points[i].perp_dot(points[(i + 1) % points.len()]))
        .sum::<f64>()
        / 2.0;
    if area.abs() < 1e-12 {
        return Err("面積のない形は押し出せません".into());
    }
    let vertices: Vec<_> = points
        .iter()
        .map(|p| builder::vertex(Point3::new(p.x, 0.0, -p.y)))
        .collect();
    let wire: truck_modeling::Wire = (0..vertices.len())
        .map(|i| builder::line(&vertices[i], &vertices[(i + 1) % vertices.len()]))
        .collect();
    let face = builder::try_attach_plane(&[wire]).map_err(|_| {
        "多角形の面を作れませんでした (辺が交差していないか確かめてください)".to_owned()
    })?;
    let face = oriented(face, Vector3::unit_y());
    Ok(builder::tsweep(&face, Vector3::new(0.0, height, 0.0)))
}

/// "0,0; 100,0; 100,50" のような文字から多角形の頂点を読む
pub fn parse_outline(text: &str) -> Result<Vec<DVec2>, String> {
    text.split([';', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let nums: Vec<f64> = pair
                .split([',', ' ', '\t'])
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse::<f64>()
                        .map_err(|_| format!("数として読めません: {s}"))
                })
                .collect::<Result<_, _>>()?;
            match nums.as_slice() {
                [x, y] => Ok(DVec2::new(*x, *y)),
                _ => Err(format!("「x, y」の形で書いてください: {pair}")),
            }
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BooleanOp {
    Union,
    Subtract,
    Intersect,
}

impl BooleanOp {
    pub fn label(self) -> &'static str {
        match self {
            BooleanOp::Union => "和 (くっつける)",
            BooleanOp::Subtract => "差 (くり抜く)",
            BooleanOp::Intersect => "積 (重なりだけ残す)",
        }
    }
}

/// 2つの形状のブーリアン。`tol` は許容誤差
pub fn boolean(a: &Solid, b: &Solid, op: BooleanOp, tol: f64) -> Result<Solid, String> {
    let failed = || {
        "演算できませんでした。面や辺がぴったり重なっているときは、少しずらしてから試してください"
            .to_owned()
    };
    match op {
        BooleanOp::Union => truck_shapeops::or(a, b, tol).ok_or_else(failed),
        BooleanOp::Intersect => truck_shapeops::and(a, b, tol).ok_or_else(failed),
        BooleanOp::Subtract => {
            let mut inverted = b.clone();
            inverted.not();
            truck_shapeops::and(a, &inverted, tol).ok_or_else(failed)
        }
    }
}

/// B-rep を表示用のメッシュと稜線にする
pub fn shape_from_solid(solid: &Solid) -> ImportedShape {
    let compressed = solid.compress();
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for shell in &compressed.boundaries {
        for p in &shell.vertices {
            min = min.min(DVec3::new(p.x, p.y, p.z));
            max = max.max(DVec3::new(p.x, p.y, p.z));
        }
    }
    let size = if min.x <= max.x {
        (max - min).length()
    } else {
        1.0
    };
    let tol = (size * 0.0005).max(1e-6);
    let mut faces = Vec::new();
    let mut edges = Vec::new();
    for shell in &compressed.boundaries {
        let meshed = shell.robust_triangulation(tol);
        for face in meshed.faces {
            if let Some(mut poly) = face.surface {
                if !face.orientation {
                    poly.invert();
                }
                faces.push(poly);
            }
        }
        edges.extend(meshed.edges.into_iter().map(|e| e.curve.0));
    }
    let mut stats = mesh_stats(&faces);
    stats.edge_count = edges.len();
    let mesh = Arc::new(PartMesh::new(&faces, &edges));
    ImportedShape {
        id: next_shape_id(),
        breps: Vec::new(),
        faces,
        edges,
        color: None,
        stats,
        mesh,
    }
}

/// 部品の形状を、部品の座標系の B-rep で取り出す (箱とモデリングで作った部品だけ)
pub fn node_solid(model: &Model, id: NodeId) -> Result<Solid, String> {
    let node = model.get(id).ok_or("部品が見つかりません")?;
    match &node.kind {
        NodeKind::Box(part) => Ok(part.build_solid()),
        NodeKind::Imported {
            source: StepSource::Modeled(source),
            ..
        } => source.solid(),
        NodeKind::Imported { .. } => Err(format!(
            "「{}」は取り込んだ部品なので演算できません。箱か、作成メニューで作った部品を選んでください",
            node.name
        )),
        NodeKind::Assembly { .. } => Err("アセンブリではなく部品を選んでください".into()),
    }
}

fn tolerance_for(solid: &Solid) -> f64 {
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for v in solid.vertex_iter() {
        let p = v.point();
        min = min.min(DVec3::new(p.x, p.y, p.z));
        max = max.max(DVec3::new(p.x, p.y, p.z));
    }
    ((max - min).length() * BOOLEAN_TOL_RATIO).max(1e-6)
}

/// 新しい部品として加える (最上位、位置 `position`)。加えたノードの id を返す
pub fn add_solid(
    model: &mut Model,
    name: String,
    solid: &Solid,
    position: DVec3,
) -> Result<NodeId, String> {
    let source = Arc::new(ModeledSource::new(name.clone(), solid)?);
    let shape = Arc::new(shape_from_solid(solid));
    Ok(model.add_node(
        None,
        name.clone(),
        DMat4::from_translation(position),
        NodeKind::Imported {
            product: name,
            shape,
            source: StepSource::Modeled(source),
        },
    ))
}

/// 部品 `id` の形を `solid` (部品の座標系) に置き換える。箱は配置を部品の置き場所に移す
fn replace_shape(model: &mut Model, id: NodeId, solid: &Solid) -> Result<(), String> {
    let node = model.get(id).ok_or("部品が見つかりません")?;
    let name = match &node.kind {
        NodeKind::Imported { product, .. } => product.clone(),
        _ => node.name.clone(),
    };
    let source = Arc::new(ModeledSource::new(name.clone(), solid)?);
    let shape = Arc::new(shape_from_solid(solid));
    let node = model.get_mut(id).ok_or("部品が見つかりません")?;
    node.kind = NodeKind::Imported {
        product: name,
        shape,
        source: StepSource::Modeled(source),
    };
    model.dirty.insert(id);
    Ok(())
}

/// 箱は寸法と配置をパラメータで持つので、原点を中心にした形と配置に分けてから演算する
fn normalize_box(model: &mut Model, id: NodeId) {
    let Some(node) = model.get(id) else { return };
    let NodeKind::Box(part) = &node.kind else {
        return;
    };
    let local = DMat4::from_rotation_translation(
        euler_deg_to_quat(part.rotation_deg),
        DVec3::from(part.position),
    );
    let centered = BoxPart {
        size: part.size,
        position: [0.0; 3],
        rotation_deg: [0.0; 3],
    };
    let placement = node.placement * local;
    if let Some(node) = model.get_mut(id) {
        node.placement = placement;
        node.kind = NodeKind::Box(centered);
    }
}

/// 穴の向き
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HoleAxis {
    X,
    Y,
    Z,
}

impl HoleAxis {
    pub fn label(self) -> &'static str {
        match self {
            HoleAxis::X => "X (左右)",
            HoleAxis::Y => "Y (上下)",
            HoleAxis::Z => "Z (前後)",
        }
    }

    fn dir(self) -> DVec3 {
        match self {
            HoleAxis::X => DVec3::X,
            HoleAxis::Y => DVec3::Y,
            HoleAxis::Z => DVec3::Z,
        }
    }
}

/// 穴の指定 (ワールド座標)
#[derive(Clone, Copy, Debug)]
pub struct HoleSpec {
    /// 穴の中心線が通る点。深さを指定するときは入口の中心
    pub center: DVec3,
    pub axis: HoleAxis,
    pub diameter: f64,
    /// 0 なら貫通。正なら軸の + 向きから、負なら - 向きから掘る深さ
    pub depth: f64,
}

/// 部品に穴をあける。元に戻せるよう履歴に積む
pub fn drill_hole(
    model: &mut Model,
    history: &mut crate::history::History,
    id: NodeId,
    hole: &HoleSpec,
) -> Result<(), String> {
    if hole.diameter <= 0.0 {
        return Err("穴の直径は 0 より大きくしてください".into());
    }
    node_solid(model, id)?;
    let before = model.snapshot(id);
    normalize_box(model, id);
    let solid = node_solid(model, id)?;
    let world = model.world_matrix(id);
    let to_local = world.inverse();
    let bounds = model.world_bounds(id).ok_or("部品の大きさが分かりません")?;
    let dir = hole.axis.dir();
    // 貫通なら部品の外まで、深さ指定なら入口から深さぶん。
    // 面がぴったり重なると演算できないので、外側へ少しはみ出させる
    let margin = bounds.size().max_element() * 0.05 + hole.diameter;
    let (start, drill_dir, length) = if hole.depth == 0.0 {
        let lo = bounds.min.dot(dir) - margin;
        let hi = bounds.max.dot(dir) + margin;
        (
            hole.center + dir * (lo - hole.center.dot(dir)),
            dir,
            hi - lo,
        )
    } else if hole.depth > 0.0 {
        // + 側の入口から - 向きに掘る
        (hole.center + dir * margin, -dir, hole.depth + margin)
    } else {
        (hole.center - dir * margin, dir, -hole.depth + margin)
    };
    let tool = oriented_cylinder(hole.diameter / 2.0, length, start, drill_dir)?;
    let tool = builder::transformed(&tool, matrix(to_local));
    let result = boolean(&solid, &tool, BooleanOp::Subtract, tolerance_for(&solid));
    let result = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(b) = &before {
                model.restore(b);
            }
            return Err(e);
        }
    };
    replace_shape(model, id, &result)?;
    let name = model.get(id).map(|n| n.name.clone()).unwrap_or_default();
    history.push(ModelChange {
        label: format!("{name} に穴をあける"),
        changes: vec![(before, model.snapshot(id))],
    });
    Ok(())
}

/// `start` から `dir` 方向へ長さ `length` の円柱 (ワールド)
fn oriented_cylinder(radius: f64, length: f64, start: DVec3, dir: DVec3) -> Result<Solid, String> {
    let base = cylinder(radius, length)?;
    let rotation = glam::DQuat::from_rotation_arc(DVec3::Y, dir.normalize());
    let m = DMat4::from_rotation_translation(rotation, start);
    Ok(builder::transformed(&base, matrix(m)))
}

/// 部品 `a` と `b` のブーリアン。結果は `a` に入れ、`b` は消す。元に戻せるよう履歴に積む
pub fn apply_boolean(
    model: &mut Model,
    history: &mut crate::history::History,
    a: NodeId,
    b: NodeId,
    op: BooleanOp,
) -> Result<(), String> {
    if a == b {
        return Err("違う部品を2つ選んでください".into());
    }
    node_solid(model, a)?;
    let solid_b = node_solid(model, b)?;
    let before_a = model.snapshot(a);
    let before_b = model.snapshot(b);
    normalize_box(model, a);
    let solid_a = node_solid(model, a)?;
    // b を a の座標系に持ってくる
    let b_to_a = model.world_matrix(a).inverse() * model.world_matrix(b);
    let solid_b = builder::transformed(&solid_b, matrix(b_to_a));
    let tol = tolerance_for(&solid_a).min(tolerance_for(&solid_b));
    let result = match boolean(&solid_a, &solid_b, op, tol) {
        Ok(r) => r,
        Err(e) => {
            if let Some(s) = &before_a {
                model.restore(s);
            }
            return Err(e);
        }
    };
    replace_shape(model, a, &result)?;
    let names = [a, b].map(|id| model.get(id).map(|n| n.name.clone()).unwrap_or_default());
    model.remove(b);
    history.push(ModelChange {
        label: format!("{} と {} の{}", names[0], names[1], op.label()),
        changes: vec![(before_a, model.snapshot(a)), (before_b, None)],
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 向きをそろえた三角形から求めた符号付き体積 (外向きなら正)
    fn signed_volume(shape: &ImportedShape) -> f64 {
        let mut volume = 0.0;
        for poly in &shape.faces {
            let positions = poly.positions();
            for tri in poly.faces().triangle_iter() {
                let [a, b, c] = tri.map(|v| {
                    let p = positions[v.pos];
                    DVec3::new(p.x, p.y, p.z)
                });
                volume += a.dot(b.cross(c)) / 6.0;
            }
        }
        volume
    }

    #[test]
    fn cylinder_volume_and_orientation() {
        let solid = cylinder(10.0, 50.0).unwrap();
        let shape = shape_from_solid(&solid);
        let expected = std::f64::consts::PI * 100.0 * 50.0;
        let v = signed_volume(&shape);
        assert!((v - expected).abs() / expected < 0.01, "{v} {expected}");
        assert!((shape.stats.max.y - 50.0).abs() < 1e-9);
        assert!(cylinder(0.0, 1.0).is_err());
    }

    #[test]
    fn extrude_either_winding() {
        let square = parse_outline("0,0; 100,0; 100,50; 0,50").unwrap();
        for outline in [square.clone(), square.iter().rev().copied().collect()] {
            let shape = shape_from_solid(&extrude(&outline, 20.0).unwrap());
            let v = signed_volume(&shape);
            assert!((v - 100_000.0).abs() < 1e-6, "{v}");
            // 図面の上 (+v) はワールドの -Z
            assert!((shape.stats.min.z + 50.0).abs() < 1e-9);
        }
        assert!(parse_outline("0,0; 1").is_err());
        assert!(extrude(&square[..2], 1.0).is_err());
    }

    #[test]
    fn hole_through_box_removes_volume() {
        let mut model = Model::default();
        let mut history = crate::history::History::default();
        let id = model.add_box([100.0, 40.0, 100.0], [200.0, 20.0, 0.0]);
        drill_hole(
            &mut model,
            &mut history,
            id,
            &HoleSpec {
                center: DVec3::new(200.0, 0.0, 0.0),
                axis: HoleAxis::Y,
                diameter: 20.0,
                depth: 0.0,
            },
        )
        .unwrap();
        let NodeKind::Imported { shape, .. } = &model.get(id).unwrap().kind else {
            panic!("穴をあけた箱は形状の部品になる");
        };
        let expected = 100.0 * 40.0 * 100.0 - std::f64::consts::PI * 100.0 * 40.0;
        let v = signed_volume(shape);
        assert!((v - expected).abs() / expected < 0.01, "{v} {expected}");
        // 位置は変わらない
        let b = model.world_bounds(id).unwrap();
        assert!((b.center() - DVec3::new(200.0, 20.0, 0.0)).length() < 1e-6);
        assert!(history.undo_label().unwrap().contains("穴"));
    }

    #[test]
    fn boolean_union_and_save_round_trip() {
        let mut model = Model::default();
        let mut history = crate::history::History::default();
        let a = model.add_box([100.0, 100.0, 100.0], [0.0, 50.0, 0.0]);
        let cyl = cylinder(20.0, 160.0).unwrap();
        let b = add_solid(
            &mut model,
            "円柱".into(),
            &cyl,
            DVec3::new(10.0, -30.0, 7.0),
        )
        .unwrap();
        apply_boolean(&mut model, &mut history, a, b, BooleanOp::Union).unwrap();
        assert!(model.get(b).is_none());
        let NodeKind::Imported { shape, source, .. } = &model.get(a).unwrap().kind else {
            panic!();
        };
        let cube = 1_000_000.0;
        let outside = std::f64::consts::PI * 400.0 * 60.0;
        let v = signed_volume(shape);
        assert!((v - (cube + outside)).abs() / cube < 0.02, "{v}");
        // 保存した JSON から同じ形を作り直せる
        let StepSource::Modeled(source) = source else {
            panic!()
        };
        let again = source.to_imported().unwrap();
        let shape2 = again.root.shape.unwrap();
        assert!((signed_volume(&shape2) - v).abs() < 1.0);
        // 取り込んだ部品は演算できない
        assert!(node_solid(&model, 999).is_err());
    }
}
