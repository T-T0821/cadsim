//! 形状カーネル (SMLib) ならではのモデリング操作
//!
//! - 作り変える: フィレット (角を丸める)・面取り・シェル (中空にする)・オフセット (太らせる・やせさせる)・
//!   フィレットの削除。穴あけと同じく、箱とモデリングで作った部品が対象で、元に戻すための
//!   [`ModelChange`] を返す。
//! - 作る: 回転体・パイプ・スイープ。できた形は [`add_solid`](super::add_solid) で部品にする。
//! - 調べる: 質量特性 (体積・重さ・重心・慣性モーメント) と断面。STEP から取り込んだ部品も対象。
//!
//! 辺と面の番号は、部品の表示メッシュ (`ImportedShape::edges` / `faces`、箱は表示用に
//! テッセレーションしたもの) の番号。寸法やクリックで選んだ番号をそのまま渡せる。

use glam::{DMat4, DQuat, DVec2, DVec3};

use super::features::{self, Feature};
use super::{
    SolidStore, display_tolerance, feature_result, history_of, node_solid, normalize_box,
    replace_shape,
};
use crate::history::ModelChange;
use crate::kernel::{Brep, MassProperties};
use crate::model::{Model, NodeId, NodeKind, StepSource};
use crate::shape::TESSELLATION_ANGLE_DEG;

/// 表示メッシュの辺の番号 → カーネルの辺の番号。表示では点が1つしかない辺を飛ばしている
fn kernel_edges(solid: &Brep, display: &[usize]) -> Result<Vec<usize>, String> {
    let meshed = solid.tessellate(display_tolerance(solid), TESSELLATION_ANGLE_DEG);
    let shown: Vec<usize> = (0..meshed.edges.len())
        .filter(|&e| meshed.edges[e].len() >= 2)
        .collect();
    display
        .iter()
        .map(|&d| {
            shown
                .get(d)
                .copied()
                .ok_or_else(|| format!("辺 {d} が見つかりません"))
        })
        .collect()
}

/// 表示メッシュの面の番号 → カーネルの面の番号。表示ではメッシュにできなかった面を飛ばしている
fn kernel_faces(solid: &Brep, display: &[usize]) -> Result<Vec<usize>, String> {
    let meshed = solid.tessellate(display_tolerance(solid), TESSELLATION_ANGLE_DEG);
    let shown: Vec<usize> = (0..meshed.faces.len())
        .filter(|&f| meshed.faces[f].mesh.is_some())
        .collect();
    display
        .iter()
        .map(|&d| {
            shown
                .get(d)
                .copied()
                .ok_or_else(|| format!("面 {d} が見つかりません"))
        })
        .collect()
}

/// 部品 `id` の形を、`edit` が作る編集 ([`Feature`]) で作り変え、編集履歴に残す。失敗したら元に戻す。
/// `edit` には今の形を渡す (表示メッシュの番号を辺・面の印に直すため)
fn modify(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    what: &str,
    edit: impl FnOnce(&Brep) -> Result<Feature, String>,
) -> Result<ModelChange, String> {
    node_solid(model, store, id)?;
    let before = model.snapshot(id);
    normalize_box(model, id);
    let result = node_solid(model, store, id).and_then(|solid| {
        let mut history = history_of(model, store, id)?;
        history.features.push(edit(&solid)?);
        feature_result(&history, &solid)
    });
    let (result, history) = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(b) = &before {
                model.restore(b);
            }
            return Err(e);
        }
    };
    replace_shape(model, store, id, &result, &history)?;
    let name = model.get(id).map(|n| n.name.clone()).unwrap_or_default();
    Ok(ModelChange {
        label: format!("{name} を{what}"),
        changes: vec![(before, model.snapshot(id))],
    })
}

/// 部品の辺 `edges` (表示メッシュの番号) を半径 `radius` で丸める
pub fn fillet_edges(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    edges: &[usize],
    radius: f64,
) -> Result<ModelChange, String> {
    modify(model, store, id, "丸める", |solid| {
        fillet_feature(solid, edges, radius)
    })
}

/// 辺 `edges` (表示メッシュの番号) を半径 `radius` で丸める編集。形は [`Feature::apply`] で作る
pub(super) fn fillet_feature(
    solid: &Brep,
    edges: &[usize],
    radius: f64,
) -> Result<Feature, String> {
    if edges.is_empty() {
        return Err("丸める辺を選んでください".into());
    }
    if radius <= 0.0 {
        return Err("半径は 0 より大きくしてください".into());
    }
    Ok(Feature::Fillet {
        edges: features::edge_keys(solid, &kernel_edges(solid, edges)?)?,
        radius,
    })
}

/// 部品の辺 `edges` (表示メッシュの番号) を、両側の面に沿って `distance` ずつ面取りする
pub fn chamfer_edges(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    edges: &[usize],
    distance: f64,
) -> Result<ModelChange, String> {
    modify(model, store, id, "面取りする", |solid| {
        chamfer_feature(solid, edges, distance)
    })
}

/// 辺 `edges` (表示メッシュの番号) を `distance` ずつ面取りする編集
pub(super) fn chamfer_feature(
    solid: &Brep,
    edges: &[usize],
    distance: f64,
) -> Result<Feature, String> {
    if edges.is_empty() {
        return Err("面取りする辺を選んでください".into());
    }
    if distance <= 0.0 {
        return Err("面取りの大きさは 0 より大きくしてください".into());
    }
    Ok(Feature::Chamfer {
        edges: features::edge_keys(solid, &kernel_edges(solid, edges)?)?,
        distance,
    })
}

/// 部品を厚み `thickness` の中空にする。`open_faces` (表示メッシュの番号) の面は開ける
pub fn shell(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    thickness: f64,
    open_faces: &[usize],
) -> Result<ModelChange, String> {
    modify(model, store, id, "中空にする", |solid| {
        shell_feature(solid, thickness, open_faces)
    })
}

/// 厚み `thickness` の中空にする編集。`open_faces` (表示メッシュの番号) の面は開ける
pub(super) fn shell_feature(
    solid: &Brep,
    thickness: f64,
    open_faces: &[usize],
) -> Result<Feature, String> {
    if thickness <= 0.0 {
        return Err("肉厚は 0 より大きくしてください".into());
    }
    Ok(Feature::Shell {
        open_faces: features::face_keys(solid, &kernel_faces(solid, open_faces)?)?,
        thickness,
    })
}

/// 部品の面を `distance` だけ外へずらす (負なら内へ)
pub fn offset(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
    distance: f64,
) -> Result<ModelChange, String> {
    let what = if distance >= 0.0 {
        "太らせる"
    } else {
        "やせさせる"
    };
    modify(model, store, id, what, |_| Ok(Feature::Offset { distance }))
}

/// 部品のフィレットを消して角を尖らせる
pub fn remove_fillets(
    model: &mut Model,
    store: &dyn SolidStore,
    id: NodeId,
) -> Result<ModelChange, String> {
    modify(model, store, id, "角を尖らせる", |_| {
        Ok(Feature::RemoveFillets)
    })
}

/// 断面 (r, y) を Y 軸のまわりに `angle_deg` 度回した回転体。r は軸からの距離 (0 以上)
pub fn revolve(profile: &[DVec2], angle_deg: f64) -> Result<Brep, String> {
    if profile.iter().any(|p| p.x < 0.0) {
        return Err("断面は軸の片側 (r が 0 以上) に描いてください".into());
    }
    let points: Vec<DVec3> = profile.iter().map(|p| DVec3::new(p.x, p.y, 0.0)).collect();
    Brep::revolve(&points, DVec3::ZERO, DVec3::Y, angle_deg)
}

/// 点 `path` を通る、半径 `radius` の管 (配管・ケーブル)
pub fn pipe(path: &[DVec3], radius: f64) -> Result<Brep, String> {
    Brep::pipe(path, radius)
}

/// 断面 (u, v) を、平面上の折れ線 `path` に沿って動かした形。
/// 断面は道筋の始まりの向きに垂直な面に置き、中心を始点に合わせる
pub fn sweep(profile: &[DVec2], path: &[DVec3]) -> Result<Brep, String> {
    let [start, next, ..] = path else {
        return Err("道筋の点を2つ以上指定してください".into());
    };
    let dir = (*next - *start)
        .try_normalize()
        .ok_or("道筋の最初の2点が同じです")?;
    let rotation = DQuat::from_rotation_arc(DVec3::Z, dir);
    let to_world = DMat4::from_rotation_translation(rotation, *start);
    let points: Vec<DVec3> = profile
        .iter()
        .map(|p| to_world.transform_point3(DVec3::new(p.x, p.y, 0.0)))
        .collect();
    Brep::sweep(&points, path)
}

/// 部品の形の B-rep (部品の座標系)。箱・モデリングの部品・STEP の部品
fn part_solids(model: &Model, store: &dyn SolidStore, id: NodeId) -> Result<Vec<Brep>, String> {
    let node = model.get(id).ok_or("部品が見つかりません")?;
    match &node.kind {
        NodeKind::Imported {
            shape,
            source: StepSource::Modeled(_),
            ..
        } if shape.breps.is_empty() => Ok(vec![node_solid(model, store, id)?]),
        NodeKind::Imported { shape, .. } | NodeKind::StandardBody { shape, .. }
            if !shape.breps.is_empty() =>
        {
            Ok(shape.breps.clone())
        }
        NodeKind::Box(_) | NodeKind::Imported { .. } => Ok(vec![node_solid(model, store, id)?]),
        _ => Err(format!("「{}」は B-rep の形を持っていません", node.name)),
    }
}

/// 部品の質量特性 (ワールド座標)。`density` は単位体積あたりの質量 (モデルの単位で)。
/// 部品が拡大縮小されていても、体積と重心はワールドの寸法で測る
pub fn mass_properties(
    model: &Model,
    store: &dyn SolidStore,
    id: NodeId,
    density: f64,
) -> Result<MassProperties, String> {
    let world = model.world_matrix(id);
    let parts: Vec<MassProperties> = part_solids(model, store, id)?
        .iter()
        .map(|b| b.transformed(world)?.mass_properties(density))
        .collect::<Result<_, _>>()
        .map_err(|_| "質量を計算できませんでした (閉じた形だけ計算できます)".to_owned())?;
    Ok(combine(&parts))
}

/// いくつかの形の質量特性を1つにまとめる (慣性モーメントは平行軸の定理で全体の重心まわりに直す)
fn combine(parts: &[MassProperties]) -> MassProperties {
    let mass: f64 = parts.iter().map(|p| p.mass).sum();
    let centroid = if mass > 0.0 {
        parts.iter().map(|p| p.centroid * p.mass).sum::<DVec3>() / mass
    } else {
        DVec3::ZERO
    };
    let mut inertia = DVec3::ZERO;
    let mut products = DVec3::ZERO;
    for p in parts {
        let d = p.centroid - centroid;
        inertia += p.inertia
            + p.mass
                * DVec3::new(
                    d.y * d.y + d.z * d.z,
                    d.z * d.z + d.x * d.x,
                    d.x * d.x + d.y * d.y,
                );
        products += p.products + p.mass * DVec3::new(d.y * d.z, d.z * d.x, d.x * d.y);
    }
    MassProperties {
        area: parts.iter().map(|p| p.area).sum(),
        volume: parts.iter().map(|p| p.volume).sum(),
        mass,
        centroid,
        inertia,
        products,
    }
}

/// 部品を、`point` を通り `normal` に垂直な平面 (ワールド座標) で切った断面の線 (ワールド座標)
pub fn section(
    model: &Model,
    store: &dyn SolidStore,
    id: NodeId,
    point: DVec3,
    normal: DVec3,
) -> Result<Vec<Vec<DVec3>>, String> {
    let world = model.world_matrix(id);
    let mut lines = Vec::new();
    for solid in part_solids(model, store, id)? {
        lines.extend(solid.transformed(world)?.section(point, normal, 48));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::super::tests::MemoryStore;
    use super::*;
    use crate::kernel::FaceMesh;
    use std::f64::consts::PI;

    fn volume(model: &Model, id: NodeId) -> f64 {
        let NodeKind::Imported { shape, .. } = &model.get(id).unwrap().kind else {
            panic!("部品の形がない")
        };
        shape
            .faces
            .iter()
            .flat_map(FaceMesh::triangle_points)
            .map(|[a, b, c]| a.dot(b.cross(c)) / 6.0)
            .sum()
    }

    /// 表示メッシュの辺のうち、両端が条件に合うもの
    fn edge_where(model: &Model, id: NodeId, f: impl Fn(DVec3) -> bool) -> usize {
        let edges = match &model.get(id).unwrap().kind {
            NodeKind::Box(part) => {
                crate::shape::tessellate_breps(&[part.build_solid().unwrap()], 0.005).1
            }
            NodeKind::Imported { shape, .. } => shape.edges.clone(),
            _ => unreachable!(),
        };
        edges.iter().position(|e| e.iter().all(|&p| f(p))).unwrap()
    }

    #[test]
    fn fillet_a_box_edge_and_undo() {
        let store = MemoryStore::default();
        let mut model = Model::default();
        let id = model.add_box([10.0, 10.0, 10.0], [0.0; 3]);
        // 箱の中心は原点。+X と +Y の面の間の辺
        let e = edge_where(&model, id, |p| {
            (p.x - 5.0).abs() < 1e-9 && (p.y - 5.0).abs() < 1e-9
        });
        let change = fillet_edges(&mut model, &store, id, &[e], 2.0).unwrap();
        assert!(change.label.contains("丸める"));
        let expected = 1000.0 - (4.0 - PI) * 10.0;
        assert!(
            (volume(&model, id) - expected).abs() < 0.1,
            "{}",
            volume(&model, id)
        );
        // 元に戻すと箱に戻る
        model.restore(change.changes[0].0.as_ref().unwrap());
        assert!(matches!(model.get(id).unwrap().kind, NodeKind::Box(_)));
        // 大きすぎる半径は失敗し、形は変わらない
        assert!(fillet_edges(&mut model, &store, id, &[e], 50.0).is_err());
        assert!(matches!(model.get(id).unwrap().kind, NodeKind::Box(_)));
    }

    #[test]
    fn chamfer_then_shell() {
        let store = MemoryStore::default();
        let mut model = Model::default();
        let id = model.add_box([10.0, 10.0, 10.0], [0.0; 3]);
        let e = edge_where(&model, id, |p| {
            (p.x - 5.0).abs() < 1e-9 && (p.y - 5.0).abs() < 1e-9
        });
        chamfer_edges(&mut model, &store, id, &[e], 1.0).unwrap();
        assert!((volume(&model, id) - 995.0).abs() < 1e-3);

        let mut model = Model::default();
        let id = model.add_box([10.0, 10.0, 10.0], [0.0; 3]);
        shell(&mut model, &store, id, 1.0, &[]).unwrap();
        assert!((volume(&model, id) - (1000.0 - 512.0)).abs() < 1.0);
    }

    #[test]
    fn revolve_pipe_and_sweep_make_solids() {
        let ring = revolve(
            &[
                DVec2::new(2.0, 0.0),
                DVec2::new(3.0, 0.0),
                DVec2::new(3.0, 1.0),
                DVec2::new(2.0, 1.0),
            ],
            360.0,
        )
        .unwrap();
        let m = ring.mass_properties(1.0).unwrap();
        assert!((m.volume - PI * (9.0 - 4.0)).abs() / m.volume < 1e-2);
        assert!(revolve(&[DVec2::new(-1.0, 0.0)], 360.0).is_err());

        let tube = pipe(&[DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0)], 0.5).unwrap();
        assert!(tube.is_solid());

        let square = [
            DVec2::new(-0.5, -0.5),
            DVec2::new(0.5, -0.5),
            DVec2::new(0.5, 0.5),
            DVec2::new(-0.5, 0.5),
        ];
        let bar = sweep(&square, &[DVec3::ZERO, DVec3::new(10.0, 0.0, 0.0)]).unwrap();
        let v = bar.mass_properties(1.0).unwrap().volume;
        assert!((v - 10.0).abs() < 0.1, "{v}");
    }

    #[test]
    fn mass_and_section_of_a_placed_box() {
        let store = MemoryStore::default();
        let mut model = Model::default();
        let id = model.add_box([2.0, 4.0, 6.0], [10.0, 0.0, 0.0]);
        let m = mass_properties(&model, &store, id, 2.0).unwrap();
        assert!((m.mass - 96.0).abs() < 1e-6);
        assert!((m.centroid - DVec3::new(10.0, 0.0, 0.0)).length() < 1e-6);
        let lines = section(&model, &store, id, DVec3::new(10.0, 0.0, 0.0), DVec3::Y).unwrap();
        let points: Vec<DVec3> = lines.into_iter().flatten().collect();
        assert!(!points.is_empty());
        assert!(points.iter().all(|p| p.y.abs() < 1e-9));
        assert!(points.iter().any(|p| (p.x - 11.0).abs() < 1e-9));
    }

    #[test]
    fn combined_inertia_uses_parallel_axes() {
        let unit = MassProperties {
            area: 1.0,
            volume: 1.0,
            mass: 1.0,
            centroid: DVec3::new(1.0, 0.0, 0.0),
            inertia: DVec3::ZERO,
            products: DVec3::ZERO,
        };
        let other = MassProperties {
            centroid: DVec3::new(-1.0, 0.0, 0.0),
            ..unit
        };
        let m = combine(&[unit, other]);
        assert_eq!(m.mass, 2.0);
        assert_eq!(m.centroid, DVec3::ZERO);
        // 原点から 1 離れた質量 1 が2つ: Iyy = Izz = 2
        assert!((m.inertia - DVec3::new(0.0, 2.0, 2.0)).length() < 1e-12);
    }
}
