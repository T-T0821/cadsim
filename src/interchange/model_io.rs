//! 中間表現 [`Scene`] とアプリのモデル (アセンブリの木) の相互変換
//!
//! モデルの座標は「モデルの単位」(STEP なら mm など) なので、メートルの `Scene` と行き来するときに
//! 位置とメッシュを拡大縮小する。回転と拡大縮小の比率はそのまま。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::math::{DAffine3, DMat4, DVec3};
use truck_meshalgo::prelude::*;

use super::{Animation, NodeTransform, Scene, SceneNode, TriMesh};
use crate::model::{Model, NodeId, NodeKind};
use crate::step_import::{ImportNode, ImportedFile, ImportedShape};

/// 箱を書き出すときのテッセレーションの許容誤差 (モデル単位)
const BOX_TESSELLATION_TOL: f64 = 0.005;
/// これより鋭く折れている辺を稜線として描く
const FEATURE_ANGLE_DEG: f64 = 30.0;

/// 単位の名前 → 1単位のメートル
pub fn meters_per_unit(unit: &str) -> f64 {
    match unit {
        "µm" | "um" => 1e-6,
        "mm" => 1e-3,
        "cm" => 1e-2,
        "km" => 1e3,
        "in" | "inch" => 0.0254,
        "ft" => 0.3048,
        _ => 1.0,
    }
}

// ---------------------------------------------------------------------------
// Scene → モデル
// ---------------------------------------------------------------------------

/// 読み込んだシーンを、STEP と同じ取り込み用の木にする。
/// `unit` はいまのモデルの単位で、シーンをその単位に合わせる。
/// アニメーションのトラックは、木を深さ優先 (親が先) でたどった順の番号に付け替える。
pub fn scene_to_import(scene: &Scene, file_name: &str, unit: &str) -> ImportedFile {
    let to_model = 1.0 / meters_per_unit(unit);
    let children = scene.children();
    let roots: Vec<usize> = (0..scene.nodes.len())
        .filter(|&i| scene.nodes[i].parent.is_none())
        .collect();

    // シーンの番号 → 取り込み順の番号
    let mut order: HashMap<usize, usize> = HashMap::new();
    let mut counter = 0;
    // ルートが1つのアセンブリならそれを、そうでなければファイル名のアセンブリをルートにする
    let root = if let [only] = roots[..]
        && scene.nodes[only].mesh.is_none()
    {
        build_node(scene, &children, only, to_model, &mut order, &mut counter)
    } else {
        counter = 1;
        ImportNode {
            name: file_name.to_owned(),
            product: file_name.to_owned(),
            placement: DMat4::IDENTITY,
            children: roots
                .iter()
                .map(|&r| build_node(scene, &children, r, to_model, &mut order, &mut counter))
                .collect(),
            shape: None,
        }
    };

    let animations = scene
        .animations
        .iter()
        .map(|a| Animation {
            name: a.name.clone(),
            tracks: a
                .tracks
                .iter()
                .filter_map(|t| {
                    let mut t = t.clone();
                    t.node = *order.get(&t.node)?;
                    for key in &mut t.keys {
                        key.1.translation *= to_model;
                    }
                    Some(t)
                })
                .collect(),
        })
        .collect();

    ImportedFile {
        root,
        unit: unit.to_owned(),
        warnings: Vec::new(),
        animations,
    }
}

fn build_node(
    scene: &Scene,
    children: &[Vec<usize>],
    index: usize,
    to_model: f64,
    order: &mut HashMap<usize, usize>,
    counter: &mut usize,
) -> ImportNode {
    order.insert(index, *counter);
    *counter += 1;
    let node = &scene.nodes[index];
    let mut transform = node.transform;
    transform.translation *= to_model;
    ImportNode {
        name: node.name.clone(),
        product: node.name.clone(),
        placement: DMat4::from(transform.to_affine()),
        children: children[index]
            .iter()
            .map(|&c| build_node(scene, children, c, to_model, order, counter))
            .collect(),
        shape: node
            .mesh
            .as_ref()
            .filter(|m| !m.is_empty())
            .map(|m| Arc::new(mesh_shape(m, to_model, node.color))),
    }
}

/// 三角形メッシュを表示用の形状にする。B-rep は持たない。
fn mesh_shape(mesh: &TriMesh, scale: f64, color: Option<[f32; 3]>) -> ImportedShape {
    let positions: Vec<Point3> = mesh
        .positions
        .iter()
        .map(|p| {
            let p = *p * scale;
            Point3::new(p.x, p.y, p.z)
        })
        .collect();
    let edges = feature_edges(&positions, &mesh.triangles);
    let faces = Faces::from_iter(mesh.triangles.iter().map(|t| t.map(|i| i as usize)));
    let poly = PolygonMesh::new(
        StandardAttributes {
            positions,
            ..Default::default()
        },
        faces,
    );
    ImportedShape::new(Vec::new(), vec![poly], edges, color)
}

/// 面が折れている辺と、穴の縁の辺を線分で返す (メッシュには B-rep の稜線がないので代わりに描く)
fn feature_edges(positions: &[Point3], triangles: &[[u32; 3]]) -> Vec<Vec<Point3>> {
    // 同じ位置の頂点をまとめる (面ごとに頂点を分けたメッシュでも辺がつながるように)
    let mut weld: HashMap<[i64; 3], usize> = HashMap::new();
    let quantum = 1e-9_f64.max(bounds_size(positions) * 1e-7);
    let ids: Vec<usize> = positions
        .iter()
        .map(|p| {
            let key = [p.x, p.y, p.z].map(|c| (c / quantum).round() as i64);
            let next = weld.len();
            *weld.entry(key).or_insert(next)
        })
        .collect();

    let normal = |t: &[u32; 3]| {
        let [a, b, c] = t.map(|i| positions[i as usize]);
        (b - a).cross(c - a).normalize()
    };
    // 辺 (小さい番号, 大きい番号) → (元の頂点2つ, 隣の面の法線)
    let mut edges: HashMap<(usize, usize), (usize, usize, Vec<Vector3>)> = HashMap::new();
    for t in triangles {
        let n = normal(t);
        for k in 0..3 {
            let (a, b) = (t[k] as usize, t[(k + 1) % 3] as usize);
            let key = (ids[a].min(ids[b]), ids[a].max(ids[b]));
            if key.0 == key.1 {
                continue;
            }
            edges.entry(key).or_insert((a, b, Vec::new())).2.push(n);
        }
    }
    let cos_limit = FEATURE_ANGLE_DEG.to_radians().cos();
    let mut seen = HashSet::new();
    edges
        .into_values()
        .filter(|(_, _, normals)| match normals[..] {
            // 法線が求まらない (潰れた三角形) ときも描く
            [a, b] => a.dot(b) < cos_limit || a.x.is_nan() || b.x.is_nan(),
            _ => true, // 縁 (面が1つ) や3枚以上が集まる辺
        })
        .filter(|(a, b, _)| seen.insert((*a, *b)))
        .map(|(a, b, _)| vec![positions[a], positions[b]])
        .collect()
}

fn bounds_size(positions: &[Point3]) -> f64 {
    let (lo, hi) = positions.iter().fold(
        (DVec3::splat(f64::MAX), DVec3::splat(f64::MIN)),
        |(lo, hi), p| {
            let p = DVec3::new(p.x, p.y, p.z);
            (lo.min(p), hi.max(p))
        },
    );
    if lo.x <= hi.x {
        (hi - lo).length()
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// モデル → Scene
// ---------------------------------------------------------------------------

/// `roots` 以下 (空ならモデル全体) をシーンにする。非表示のノードも含める。
/// 戻り値の対応表は、モデルのノード → シーンの番号 (アニメーションの書き出しに使う)。
pub fn model_to_scene(model: &Model, roots: &[NodeId]) -> (Scene, HashMap<NodeId, usize>) {
    let to_meters = meters_per_unit(&model.unit);
    let mut scene = Scene::default();
    let mut index_of = HashMap::new();
    let roots = if roots.is_empty() {
        model.roots.clone()
    } else {
        roots.to_vec()
    };
    for root in roots {
        // 選んだノードがどこかの子でも、ワールドでの位置で書き出す
        let parent_world = model
            .get(root)
            .and_then(|n| n.parent)
            .map_or(DMat4::IDENTITY, |p| model.world_matrix(p));
        for id in model.subtree(root) {
            let Some(node) = model.get(id) else { continue };
            let mut placement = node.placement;
            if id == root {
                placement = parent_world * placement;
            }
            let mut transform = NodeTransform::from_affine(DAffine3::from_mat4(placement));
            transform.translation *= to_meters;
            let parent = if id == root {
                None
            } else {
                node.parent.and_then(|p| index_of.get(&p).copied())
            };
            let mesh = node_mesh(&node.kind).map(|mut m| {
                for p in &mut m.positions {
                    *p *= to_meters;
                }
                m
            });
            let color = node.kind.is_part().then(|| model.display_color(id));
            index_of.insert(id, scene.nodes.len());
            scene.nodes.push(SceneNode {
                name: node.name.clone(),
                parent,
                transform,
                mesh,
                color,
            });
        }
    }
    (scene, index_of)
}

/// 部品の形状を1つの三角形メッシュにする (部品の座標系、モデル単位)
fn node_mesh(kind: &NodeKind) -> Option<TriMesh> {
    let polys: Vec<PolygonMesh> = match kind {
        NodeKind::Assembly { .. } | NodeKind::Standard(_) => return None,
        NodeKind::Box(part) => {
            let meshed = part.build_solid().triangulation(BOX_TESSELLATION_TOL);
            meshed.boundaries()[0]
                .face_iter()
                .filter_map(|face| {
                    let mut poly = face.surface()?;
                    if !face.orientation() {
                        poly.invert();
                    }
                    Some(poly)
                })
                .collect()
        }
        NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. } => {
            shape.faces.clone()
        }
    };
    let mut mesh = TriMesh::default();
    for poly in &polys {
        let offset = mesh.positions.len() as u32;
        mesh.positions
            .extend(poly.positions().iter().map(|p| DVec3::new(p.x, p.y, p.z)));
        mesh.triangles.extend(
            poly.faces()
                .triangle_iter()
                .map(|t| t.map(|v| v.pos as u32 + offset)),
        );
    }
    Some(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StepSource;
    use cadsim_io::interchange::test_util::sample_scene;

    #[test]
    fn scene_round_trips_through_model() {
        let scene = sample_scene();
        let file = scene_to_import(&scene, "line.glb", "mm");
        // ルートが1つのアセンブリなので、それがそのまま木のルートになる
        assert_eq!(file.root.name, "ライン");
        // 1 m は 1000 mm
        assert!((file.root.placement.w_axis.x - 1000.0).abs() < 1e-9);
        assert_eq!(file.animations[0].tracks[0].node, 2);

        let mut model = Model::default();
        let root = model.add_imported(file.root, file.unit, StepSource::File("line.glb".into()));
        assert_eq!(model.measure(root).part_count, 2);

        let (back, index_of) = model_to_scene(&model, &[]);
        assert_eq!(back.nodes.len(), 3);
        assert_eq!(index_of[&root], 0);
        let t = back.nodes[2].transform;
        assert!(t.translation.abs_diff_eq(DVec3::new(0.0, 1.0, 0.0), 1e-9));
        assert!(t.scale.abs_diff_eq(DVec3::new(0.5, 2.0, 0.5), 1e-9));
        super::super::test_util::assert_same_shape(&back.merged_mesh(), &scene.merged_mesh(), 1e-9);
        assert_eq!(back.nodes[1].color, Some([0.2, 0.4, 0.8]));
    }

    #[test]
    fn boxes_export_as_meshes() {
        let mut model = Model::default();
        model.add_box([2000.0, 1000.0, 500.0], [0.0, 500.0, 0.0]);
        let (scene, _) = model_to_scene(&model, &[]);
        let mesh = scene.nodes[0].mesh.as_ref().unwrap();
        assert_eq!(mesh.triangles.len(), 12);
        let max = mesh.positions.iter().fold(DVec3::MIN, |a, b| a.max(*b));
        assert!(max.abs_diff_eq(DVec3::new(1.0, 1.0, 0.25), 1e-9));
    }

    #[test]
    fn cube_has_twelve_feature_edges() {
        let mesh = super::super::test_util::cube();
        let shape = mesh_shape(&mesh, 1.0, None);
        // 立方体の面の対角線は平らなので描かない
        assert_eq!(shape.edges.len(), 12);
    }
}
