//! 形状の簡略化 (デフィーチャリング): 小さな穴とフィレットを消して、表示用の三角形を減らす
//!
//! 大規模アセンブリでは、ボルト穴やフィレット (角の丸み) が三角形の大半を占める。
//! 配置の確認には要らないので、表示用のメッシュからだけ消す (干渉チェックや計測は元の形状で行う)。
//! Bevy に依存しない計算だけを置く。
//!
//! - **穴**: 平面の面にある小さな内側の輪郭 (穴の縁) を見つけ、その穴の内壁・底の面を消して、
//!   縁の輪郭を外した平面を張り直す (B-rep のトポロジーを書き換えてからテッセレーションし直す)。
//!   内壁の法線が穴の軸を向いているもの (凹) だけを穴とみなし、ボスやピン (凸) は残す。
//! - **フィレット**: 2つの平面の間をなめらかにつなぐ細い曲面を見つけ、消す。両側の平面の縁を
//!   2平面の交線まで延ばすので、角が尖った形になる (メッシュの頂点を動かす)。
//!   フィレットどうしが集まる角の小さな曲面も一緒に消す。
//!
//! 面の番号は簡略化していない表示メッシュと同じ番号で返すので、簡略表示に切り替えても
//! クリックした面の番号 (選択) は変わらない。

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::math::DVec3;
use truck_meshalgo::prelude::*;
use truck_topology::compress::{CompressedEdgeIndex, CompressedShell};

use crate::part_mesh::PartMesh;
use crate::step_import::{Brep, brep_tolerance};

/// どこまで小さいものを消すか (モデルの単位)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DefeatureOptions {
    /// この直径以下の穴を消す
    pub max_hole_diameter: f64,
    /// この半径以下のフィレットを消す
    pub max_fillet_radius: f64,
}

/// 簡略化で消したものの数
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DefeatureStats {
    pub holes: usize,
    pub fillets: usize,
}

impl std::ops::AddAssign for DefeatureStats {
    fn add_assign(&mut self, other: Self) {
        self.holes += other.holes;
        self.fillets += other.fillets;
    }
}

/// 簡略化した表示用メッシュ
pub struct DefeaturedMesh {
    /// (簡略化していない表示での面番号, 面のメッシュ)。消した面は入らない。
    pub faces: Vec<(u32, PolygonMesh)>,
    /// 稜線のポリライン
    pub edges: Vec<Vec<Point3>>,
    pub stats: DefeatureStats,
    /// 簡略化していない表示での面の数 (テッセレーションできなかった面は数えない)
    pub face_count: usize,
}

/// 部品の B-rep から簡略化した表示用メッシュを作る。消すものがなければ None。
/// 面の番号は `step_import` が作る表示メッシュ (`ImportedShape::mesh`) と同じ並び。
pub fn defeature_shape(
    breps: &[Brep],
    options: &DefeatureOptions,
) -> Option<(PartMesh, DefeatureStats)> {
    let tol = brep_tolerance(breps);
    let mut faces = Vec::new();
    let mut edges = Vec::new();
    let mut stats = DefeatureStats::default();
    let mut offset = 0;
    for brep in breps {
        let mesh = defeature(brep, tol, options);
        faces.extend(
            mesh.faces
                .into_iter()
                .map(|(face, poly)| (face + offset as u32, poly)),
        );
        edges.extend(mesh.edges);
        stats += mesh.stats;
        offset += mesh.face_count;
    }
    (stats != DefeatureStats::default()).then(|| (PartMesh::with_face_ids(&faces, &edges), stats))
}

/// 平面とみなす許容差 (部品の大きさに対する比)
const PLANAR_TOLERANCE: f64 = 1e-6;
/// 1つの穴とみなす面の数の上限 (これより多くつながっていたら穴ではない)
const MAX_HOLE_FACES: usize = 64;

type MeshedShell = CompressedShell<Point3, PolylineCurve<Point3>, Option<PolygonMesh>>;

/// B-rep のシェルから小さな穴とフィレットを消し、表示用のメッシュを作る。
/// `tol` はテッセレーションの許容誤差。
pub fn defeature<C, S>(
    shell: &CompressedShell<Point3, C, S>,
    tol: f64,
    options: &DefeatureOptions,
) -> DefeaturedMesh
where
    C: Clone,
    S: Clone,
    CompressedShell<Point3, C, S>: RobustMeshableShape<MeshedShape = MeshedShell>,
{
    let meshed = shell.robust_triangulation(tol);
    // 元の面の番号 → 表示での面の番号 (テッセレーションできなかった面は表示に入らない)
    let mut display = Vec::with_capacity(meshed.faces.len());
    let mut face_count = 0;
    for face in &meshed.faces {
        display.push(face.surface.is_some().then_some(face_count as u32));
        face_count += usize::from(face.surface.is_some());
    }
    let (shell, kept, holes) = remove_small_holes(shell, &meshed, options.max_hole_diameter);
    let meshed = if holes > 0 {
        shell.robust_triangulation(tol)
    } else {
        meshed
    };
    let mut result = collapse_fillets(&shell, &meshed, options.max_fillet_radius);
    result.faces = result
        .faces
        .into_iter()
        .filter_map(|(face, poly)| Some((display[kept[face as usize]]?, poly)))
        .collect();
    result.stats.holes = holes;
    result.face_count = face_count;
    result
}

// ---------------------------------------------------------------------------
// 共通: 面と辺のつながり・平面の判定
// ---------------------------------------------------------------------------

/// 辺 → その辺を境界に持つ面
fn edge_faces<C, S>(shell: &CompressedShell<Point3, C, S>) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); shell.edges.len()];
    for (f, face) in shell.faces.iter().enumerate() {
        for e in face.boundaries.iter().flatten() {
            if !out[e.index].contains(&f) {
                out[e.index].push(f);
            }
        }
    }
    out
}

fn dvec(p: &Point3) -> DVec3 {
    DVec3::new(p.x, p.y, p.z)
}

/// 面の向きを反映した三角形 (3頂点) の並び
fn triangles(poly: &PolygonMesh, flip: bool) -> Vec<[DVec3; 3]> {
    let positions = poly.positions();
    poly.faces()
        .triangle_iter()
        .map(|t| {
            let [a, b, c] = t.map(|v| dvec(&positions[v.pos]));
            if flip { [a, c, b] } else { [a, b, c] }
        })
        .collect()
}

fn triangle_normal([a, b, c]: &[DVec3; 3]) -> DVec3 {
    (b - a).cross(c - a)
}

/// 面が平面なら (単位法線, 原点からの距離) を返す
fn plane_of(poly: &PolygonMesh, flip: bool, scale: f64) -> Option<(DVec3, f64)> {
    let tris = triangles(poly, flip);
    let normal = tris
        .iter()
        .map(triangle_normal)
        .reduce(|a, b| a + b)?
        .try_normalize()?;
    let origin = tris.first()?[0];
    let d = normal.dot(origin);
    let eps = scale * PLANAR_TOLERANCE + 1e-9;
    tris.iter()
        .flatten()
        .all(|p| (normal.dot(*p) - d).abs() <= eps)
        .then_some((normal, d))
}

/// 外接箱のいちばん長い辺
fn extent(points: impl IntoIterator<Item = DVec3>) -> f64 {
    let mut min = DVec3::splat(f64::MAX);
    let mut max = DVec3::splat(f64::MIN);
    for p in points {
        min = min.min(p);
        max = max.max(p);
    }
    if min.x > max.x {
        0.0
    } else {
        (max - min).max_element()
    }
}

/// 輪郭 (辺の並び) の点
fn loop_points(meshed: &MeshedShell, edges: &[CompressedEdgeIndex]) -> Vec<DVec3> {
    edges
        .iter()
        .flat_map(|e| meshed.edges[e.index].curve.iter().map(dvec))
        .collect()
}

/// メッシュ全体の大きさ (平面判定の許容差に使う)
fn shell_scale(meshed: &MeshedShell) -> f64 {
    extent(meshed.vertices.iter().map(dvec)).max(1e-9)
}

// ---------------------------------------------------------------------------
// 穴
// ---------------------------------------------------------------------------

/// 小さな穴の縁 (平面の面の内側の輪郭)
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Rim {
    face: usize,
    boundary: usize,
}

/// 小さな穴を消したシェルと、新しい面 → 元の面の番号の表、消した穴の数を返す
fn remove_small_holes<C: Clone, S: Clone>(
    shell: &CompressedShell<Point3, C, S>,
    meshed: &MeshedShell,
    max_diameter: f64,
) -> (CompressedShell<Point3, C, S>, Vec<usize>, usize) {
    let identity: Vec<usize> = (0..shell.faces.len()).collect();
    if max_diameter <= 0.0 {
        return (shell.clone(), identity, 0);
    }
    let scale = shell_scale(meshed);
    let edge_faces = edge_faces(shell);

    // 平面の面にある、小さな内側の輪郭をすべて集める (いちばん大きい輪郭が外周)
    let mut rims: HashMap<usize, Rim> = HashMap::new(); // 辺 → 縁
    let mut rim_list = Vec::new();
    for (f, face) in shell.faces.iter().enumerate() {
        if face.boundaries.len() < 2 {
            continue;
        }
        let Some(poly) = meshed.faces[f].surface.as_ref() else {
            continue;
        };
        if plane_of(poly, !face.orientation, scale).is_none() {
            continue;
        }
        let sizes: Vec<f64> = face
            .boundaries
            .iter()
            .map(|b| extent(loop_points(meshed, b)))
            .collect();
        let outer = (0..sizes.len())
            .max_by(|&a, &b| sizes[a].total_cmp(&sizes[b]))
            .unwrap_or(0);
        for (b, &size) in sizes.iter().enumerate() {
            if b != outer && size <= max_diameter {
                let rim = Rim {
                    face: f,
                    boundary: b,
                };
                rim_list.push(rim);
                for e in &face.boundaries[b] {
                    rims.insert(e.index, rim);
                }
            }
        }
    }

    let rim_faces: HashSet<usize> = rim_list.iter().map(|r| r.face).collect();
    let outward = outward_sign(shell, meshed);
    let mut removed_faces: HashSet<usize> = HashSet::new();
    let mut removed_rims: HashSet<Rim> = HashSet::new();
    let mut holes = 0;
    for rim in rim_list {
        if removed_rims.contains(&rim) {
            continue; // 貫通穴の反対側の縁としてもう消した
        }
        let Some((region, reached)) = hole_region(shell, &edge_faces, &rims, &rim_faces, rim)
        else {
            continue;
        };
        if region.iter().any(|f| removed_faces.contains(f))
            || !is_small_hole(shell, meshed, rim, &region, max_diameter, outward)
        {
            continue;
        }
        removed_faces.extend(region);
        removed_rims.extend(reached);
        holes += 1;
    }
    if holes == 0 {
        return (shell.clone(), identity, 0);
    }

    let mut out = CompressedShell {
        vertices: shell.vertices.clone(),
        edges: shell.edges.clone(),
        faces: Vec::new(),
    };
    let mut kept = Vec::new();
    for (f, face) in shell.faces.iter().enumerate() {
        if removed_faces.contains(&f) {
            continue;
        }
        let mut face = face.clone();
        let mut b = 0;
        face.boundaries.retain(|_| {
            let keep = !removed_rims.contains(&Rim {
                face: f,
                boundary: b,
            });
            b += 1;
            keep
        });
        out.faces.push(face);
        kept.push(f);
    }
    (out, kept, holes)
}

/// 縁から穴の内側の面をたどる。ほかの縁 (貫通穴の反対側) でも止まる。
/// 穴の面の集まりと、たどり着いた縁を返す。穴らしくなければ None。
fn hole_region<C, S>(
    shell: &CompressedShell<Point3, C, S>,
    edge_faces: &[Vec<usize>],
    rims: &HashMap<usize, Rim>,
    rim_faces: &HashSet<usize>,
    start: Rim,
) -> Option<(Vec<usize>, Vec<Rim>)> {
    let mut region = Vec::new();
    let mut seen = HashSet::new();
    let mut reached = vec![start];
    let mut queue = VecDeque::new();
    for e in &shell.faces[start.face].boundaries[start.boundary] {
        for &f in &edge_faces[e.index] {
            if f != start.face && seen.insert(f) {
                queue.push_back(f);
            }
        }
    }
    while let Some(f) = queue.pop_front() {
        region.push(f);
        if region.len() > MAX_HOLE_FACES {
            return None;
        }
        for e in shell.faces[f].boundaries.iter().flatten() {
            if let Some(&rim) = rims.get(&e.index)
                && rim.face != f
            {
                // ほかの平面の縁に着いた: そこが穴の出口 (その平面は残す)
                if !reached.contains(&rim) {
                    reached.push(rim);
                }
                continue;
            }
            for &g in &edge_faces[e.index] {
                // 縁を通らずに縁のある平面 (消してはいけない面) に着いた: 穴ではない
                if rim_faces.contains(&g) {
                    return None;
                }
                if seen.insert(g) {
                    queue.push_back(g);
                }
            }
        }
    }
    (!region.is_empty()).then_some((region, reached))
}

/// 縁から続く面が、縁の太さに収まる穴か。内壁の法線が穴の軸の方を向いている (凹) ものだけを
/// 穴とし、ボスやピン (凸) は残す。面が軸から離れて広がっていたら穴ではない。
fn is_small_hole<C, S>(
    shell: &CompressedShell<Point3, C, S>,
    meshed: &MeshedShell,
    rim: Rim,
    region: &[usize],
    max_diameter: f64,
    outward: f64,
) -> bool {
    let scale = shell_scale(meshed);
    let Some((axis, _)) = meshed.faces[rim.face]
        .surface
        .as_ref()
        .and_then(|poly| plane_of(poly, !shell.faces[rim.face].orientation, scale))
    else {
        return false;
    };
    let points = loop_points(meshed, &shell.faces[rim.face].boundaries[rim.boundary]);
    if points.is_empty() {
        return false;
    }
    let center = points.iter().sum::<DVec3>() / points.len() as f64;
    let mut score = 0.0;
    for &f in region {
        let Some(poly) = meshed.faces[f].surface.as_ref() else {
            continue;
        };
        for tri in triangles(poly, !shell.faces[f].orientation) {
            let p = (tri[0] + tri[1] + tri[2]) / 3.0;
            // 軸に垂直な方向で、三角形から軸へ向かうベクトル
            let to_axis = center - p;
            let to_axis = to_axis - axis * to_axis.dot(axis);
            if to_axis.length() > max_diameter {
                return false;
            }
            score += triangle_normal(&tri).dot(to_axis);
        }
    }
    score * outward > 0.0
}

/// `triangles` の向きが外向きなら 1、内向きなら -1。truck のメッシュが面の向きを
/// 反映済みかどうかに左右されないよう、シェル全体の符号付き体積で確かめる。
fn outward_sign<C, S>(shell: &CompressedShell<Point3, C, S>, meshed: &MeshedShell) -> f64 {
    let mut volume = 0.0;
    for (face, m) in shell.faces.iter().zip(&meshed.faces) {
        let Some(poly) = m.surface.as_ref() else {
            continue;
        };
        for [a, b, c] in triangles(poly, !face.orientation) {
            volume += a.dot(b.cross(c));
        }
    }
    if volume < 0.0 { -1.0 } else { 1.0 }
}

// ---------------------------------------------------------------------------
// フィレット
// ---------------------------------------------------------------------------

/// 曲面と平面が接しているとみなす法線の向きの差 (cos)
const TANGENT_COS: f64 = 0.999;

/// 曲面の法線 (面の向きを反映した単位ベクトル)。truck が付けた頂点の法線は曲面の正確な法線
/// なので、粗いテッセレーションでも接する平面の法線と一致する。無ければ三角形の法線を使う。
fn surface_normals(poly: &PolygonMesh, flip: bool) -> Vec<DVec3> {
    let sign = if flip { -1.0 } else { 1.0 };
    let vertex: Vec<DVec3> = poly
        .normals()
        .iter()
        .filter_map(|n| (DVec3::new(n.x, n.y, n.z) * sign).try_normalize())
        .collect();
    if !vertex.is_empty() {
        return vertex;
    }
    triangles(poly, flip)
        .iter()
        .filter_map(|t| triangle_normal(t).try_normalize())
        .collect()
}

/// 2平面の交線 (点と単位方向)
#[derive(Clone, Copy)]
struct Line {
    point: DVec3,
    dir: DVec3,
}

impl Line {
    fn of_planes((n1, d1): (DVec3, f64), (n2, d2): (DVec3, f64)) -> Option<Line> {
        let dir = n1.cross(n2);
        let len2 = dir.length_squared();
        if len2 < 1e-6 {
            return None; // ほぼ平行
        }
        // 両方の平面に乗る点 (交線上で原点に最も近い点)
        let point = (n2.cross(dir) * d1 + dir.cross(n1) * d2) / len2;
        Some(Line {
            point,
            dir: dir / len2.sqrt(),
        })
    }

    fn closest(&self, p: DVec3) -> DVec3 {
        self.point + self.dir * (p - self.point).dot(self.dir)
    }

    fn distance(&self, p: DVec3) -> f64 {
        (p - self.closest(p)).length()
    }
}

/// 面のメッシュからフィレットを消し、両側の平面を交線まで延ばす
fn collapse_fillets<C, S>(
    shell: &CompressedShell<Point3, C, S>,
    meshed: &MeshedShell,
    max_radius: f64,
) -> DefeaturedMesh {
    let scale = shell_scale(meshed);
    let edge_faces = edge_faces(shell);
    let planes: Vec<Option<(DVec3, f64)>> = shell
        .faces
        .iter()
        .zip(&meshed.faces)
        .map(|(face, m)| plane_of(m.surface.as_ref()?, !face.orientation, scale))
        .collect();
    let neighbors = |f: usize| -> Vec<usize> {
        let mut out: Vec<usize> = shell.faces[f]
            .boundaries
            .iter()
            .flatten()
            .flat_map(|e| edge_faces[e.index].iter().copied())
            .filter(|&g| g != f)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };

    // フィレット: 曲面で、接する平面がちょうど2つあり、その交線から近いもの
    let mut fillet_lines: HashMap<usize, Line> = HashMap::new();
    if max_radius > 0.0 {
        for (f, face) in shell.faces.iter().enumerate() {
            if planes[f].is_some() {
                continue;
            }
            let Some(poly) = meshed.faces[f].surface.as_ref() else {
                continue;
            };
            let normals = surface_normals(poly, !face.orientation);
            // 曲面の法線と同じ向きの法線を持つ平面 = この曲面が接している平面
            let tangent: Vec<(DVec3, f64)> = neighbors(f)
                .into_iter()
                .filter_map(|g| planes[g])
                .filter(|(n, _)| normals.iter().any(|m| m.dot(*n) > TANGENT_COS))
                .collect();
            let [a, b] = tangent[..] else {
                continue;
            };
            let Some(line) = Line::of_planes(a, b) else {
                continue;
            };
            // 接点は交線から r·tan(α/2) の位置にある (α は2平面の法線のなす角)。
            // いちばん遠い点を接点とみなして半径を見積もる。
            let farthest = poly
                .positions()
                .iter()
                .map(|p| line.distance(dvec(p)))
                .fold(0.0, f64::max);
            let half_angle = a.0.dot(b.0).clamp(-1.0, 1.0).acos() / 2.0;
            let radius = farthest / half_angle.tan().max(1e-9);
            if radius <= max_radius * 1.05 {
                fillet_lines.insert(f, line);
            }
        }
    }

    // フィレットが集まる角の小さな曲面 (まわりが消すフィレットか平面だけのもの) も消す
    let mut corners: HashSet<usize> = HashSet::new();
    for (f, m) in meshed.faces.iter().enumerate() {
        if planes[f].is_some() || fillet_lines.contains_key(&f) {
            continue;
        }
        let Some(poly) = m.surface.as_ref() else {
            continue;
        };
        let around = neighbors(f);
        let touches_fillet = around.iter().any(|g| fillet_lines.contains_key(g));
        let all_simple = around
            .iter()
            .all(|g| fillet_lines.contains_key(g) || planes[*g].is_some());
        let small = extent(poly.positions().iter().map(dvec)) <= max_radius * 2.0;
        if touches_fillet && all_simple && small {
            corners.insert(f);
        }
    }

    // 消すフィレットの縁の点 → 移す先 (交線上の点)。縁の点は両側の面のメッシュと同じ座標。
    let key = |p: DVec3| {
        let q = scale * 1e-9 + 1e-12;
        (
            (p.x / q).round() as i64,
            (p.y / q).round() as i64,
            (p.z / q).round() as i64,
        )
    };
    let mut moves: HashMap<(i64, i64, i64), DVec3> = HashMap::new();
    for (&f, line) in &fillet_lines {
        for e in shell.faces[f].boundaries.iter().flatten() {
            for p in meshed.edges[e.index].curve.iter() {
                let p = dvec(p);
                moves.entry(key(p)).or_insert_with(|| line.closest(p));
            }
        }
    }
    let moved = |p: &Point3| -> Point3 {
        match moves.get(&key(dvec(p))) {
            Some(q) => Point3::new(q.x, q.y, q.z),
            None => *p,
        }
    };

    let removed = |f: usize| fillet_lines.contains_key(&f) || corners.contains(&f);
    let mut faces = Vec::new();
    for (f, (face, m)) in shell.faces.iter().zip(&meshed.faces).enumerate() {
        if removed(f) {
            continue;
        }
        let Some(poly) = m.surface.as_ref() else {
            continue;
        };
        let mut poly = poly.clone();
        if !moves.is_empty() {
            for p in poly.positions_mut() {
                *p = moved(p);
            }
        }
        if !face.orientation {
            poly.invert();
        }
        faces.push((f as u32, poly));
    }

    // 稜線: 消した面どうしの境目と、消した穴の縁 (どの面にも使われていない辺) は捨て、
    // 残りは点を移す。つぶれて点になった線も捨てる。
    let mut edges = Vec::new();
    for (e, faces_of_edge) in edge_faces.iter().enumerate() {
        if faces_of_edge.iter().all(|&f| removed(f)) {
            continue;
        }
        let line: Vec<Point3> = meshed.edges[e].curve.iter().map(moved).collect();
        let length: f64 = line
            .windows(2)
            .map(|w| dvec(&w[0]).distance(dvec(&w[1])))
            .sum();
        if length > scale * 1e-9 {
            edges.push(line);
        }
    }
    DefeaturedMesh {
        faces,
        edges,
        stats: DefeatureStats {
            holes: 0,
            fillets: fillet_lines.len(),
        },
        face_count: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::step_import::mesh_stats;
    use std::f64::consts::{FRAC_PI_4, TAU};
    use truck_modeling::{Solid, builder};

    fn shell_of(
        solid: &Solid,
    ) -> CompressedShell<Point3, truck_modeling::Curve, truck_modeling::Surface> {
        solid.compress().boundaries.remove(0)
    }

    fn triangle_count(mesh: &DefeaturedMesh) -> usize {
        mesh.faces
            .iter()
            .map(|(_, p)| p.faces().triangle_iter().count())
            .sum()
    }

    fn volume(mesh: &DefeaturedMesh) -> f64 {
        let polys: Vec<PolygonMesh> = mesh.faces.iter().map(|(_, p)| p.clone()).collect();
        mesh_stats(&polys).volume
    }

    /// 100 x 10 x 60 の板に、直径 `d` の貫通穴を2つあける
    fn plate_with_holes(d: f64) -> Solid {
        let v = builder::vertex(Point3::new(0.0, 0.0, 0.0));
        let e = builder::tsweep(&v, Vector3::new(100.0, 0.0, 0.0));
        let f = builder::tsweep(&e, Vector3::new(0.0, 0.0, 60.0));
        let outer = f.boundaries()[0].clone();
        let circle = |x: f64, z: f64| {
            let v = builder::vertex(Point3::new(x + d / 2.0, 0.0, z));
            // +Y まわりの回転は XZ 平面で外周と逆回りなので、そのまま穴になる
            builder::rsweep(&v, Point3::new(x, 0.0, z), Vector3::unit_y(), Rad(TAU))
        };
        let face = builder::try_attach_plane(&[outer, circle(25.0, 30.0), circle(75.0, 30.0)])
            .expect("穴あきの平面");
        builder::tsweep(&face, Vector3::new(0.0, 10.0, 0.0))
    }

    #[test]
    fn small_through_holes_are_filled() {
        let solid = plate_with_holes(12.0);
        let shell = shell_of(&solid);
        let options = DefeatureOptions {
            max_hole_diameter: 20.0,
            max_fillet_radius: 0.0,
        };
        let original = defeature(
            &shell,
            0.05,
            &DefeatureOptions {
                max_hole_diameter: 0.0,
                max_fillet_radius: 0.0,
            },
        );
        let simplified = defeature(&shell, 0.05, &options);
        assert_eq!(simplified.stats.holes, 2);
        assert!(triangle_count(&simplified) * 4 < triangle_count(&original));
        // 穴が埋まって、ただの直方体の体積になる
        assert!(
            (volume(&simplified) - 100.0 * 10.0 * 60.0).abs() < 1.0,
            "{}",
            volume(&simplified)
        );
        // 残った面の番号は元の面の番号 (上面と下面は残る)
        let kept: Vec<u32> = simplified.faces.iter().map(|(f, _)| *f).collect();
        assert!(kept.windows(2).all(|w| w[0] < w[1]));
        assert!(kept.iter().all(|&f| (f as usize) < original.face_count));
        assert_eq!(simplified.face_count, original.face_count);
    }

    #[test]
    fn large_holes_are_kept() {
        let shell = shell_of(&plate_with_holes(30.0));
        let options = DefeatureOptions {
            max_hole_diameter: 20.0,
            max_fillet_radius: 0.0,
        };
        assert_eq!(defeature(&shell, 0.05, &options).stats.holes, 0);
    }

    /// 断面が 40 x 30 で、1つの角を半径 `r` で丸めた長さ 100 の棒
    fn rounded_bar(r: f64) -> Solid {
        let p = |x: f64, y: f64| builder::vertex(Point3::new(x, y, 0.0));
        let (w, h) = (40.0, 30.0);
        let v0 = p(0.0, 0.0);
        let v1 = p(w - r, 0.0);
        let v2 = p(w, r);
        let v3 = p(w, h);
        let v4 = p(0.0, h);
        // 中心 (w - r, r) の 1/4 円。途中の点は 45° の位置。
        let arc = builder::circle_arc(
            &v1,
            &v2,
            Point3::new(w - r + r * FRAC_PI_4.sin(), r - r * FRAC_PI_4.cos(), 0.0),
        );
        let wire: truck_modeling::Wire = vec![
            builder::line(&v0, &v1),
            arc,
            builder::line(&v2, &v3),
            builder::line(&v3, &v4),
            builder::line(&v4, &v0),
        ]
        .into();
        let face = builder::try_attach_plane(&[wire]).expect("断面");
        builder::tsweep(&face, Vector3::new(0.0, 0.0, 100.0))
    }

    #[test]
    fn fillet_is_collapsed_to_sharp_corner() {
        let shell = shell_of(&rounded_bar(5.0));
        let off = DefeatureOptions {
            max_hole_diameter: 0.0,
            max_fillet_radius: 0.0,
        };
        let on = DefeatureOptions {
            max_hole_diameter: 0.0,
            max_fillet_radius: 10.0,
        };
        let original = defeature(&shell, 0.01, &off);
        let simplified = defeature(&shell, 0.01, &on);
        assert_eq!(simplified.stats.fillets, 1);
        assert_eq!(simplified.faces.len(), original.faces.len() - 1);
        assert!(triangle_count(&simplified) < triangle_count(&original));
        // 角が尖って、丸める前の直方体の体積に戻る
        let full = 40.0 * 30.0 * 100.0;
        assert!(
            (volume(&simplified) - full).abs() < full * 1e-6,
            "{}",
            volume(&simplified)
        );
        assert!(volume(&original) < full - 100.0);
        // 尖った角の稜線ができ、つぶれた線は残らない
        assert!(simplified.edges.iter().all(|e| e.len() >= 2));
    }

    #[test]
    fn large_fillet_is_kept() {
        let shell = shell_of(&rounded_bar(15.0));
        let options = DefeatureOptions {
            max_hole_diameter: 0.0,
            max_fillet_radius: 10.0,
        };
        assert_eq!(defeature(&shell, 0.01, &options).stats.fillets, 0);
    }

    /// 角を丸めた (半径 10) 200 x 15 x 120 の板に、直径 12 の穴を4つと直径 50 の穴を1つあけた部品
    fn bracket() -> Solid {
        let (w, d, r) = (200.0, 120.0, 10.0);
        let corners = [(w - r, r), (w - r, d - r), (r, d - r), (r, r)];
        let points = [
            (r, 0.0),
            (w - r, 0.0),
            (w, r),
            (w, d - r),
            (w - r, d),
            (r, d),
            (0.0, d - r),
            (0.0, r),
        ];
        let vertices: Vec<_> = points
            .iter()
            .map(|&(x, z)| builder::vertex(Point3::new(x, 0.0, z)))
            .collect();
        let mut edges = Vec::new();
        for (i, (cx, cz)) in corners.into_iter().enumerate() {
            let (a, b) = (&vertices[2 * i], &vertices[2 * i + 1]);
            let c = &vertices[(2 * i + 2) % 8];
            edges.push(builder::line(a, b));
            // 角の 1/4 円の途中の点は、中心から2つの端点の中点の向きに半径ぶん
            let mid = (b.point().to_vec() + c.point().to_vec()) / 2.0 - Vector3::new(cx, 0.0, cz);
            let mid = Point3::new(cx, 0.0, cz) + mid.normalize() * r;
            edges.push(builder::circle_arc(b, c, mid));
        }
        let mut wires = vec![truck_modeling::Wire::from(edges)];
        for (x, z, dia) in [
            (30.0, 30.0, 12.0),
            (170.0, 30.0, 12.0),
            (30.0, 90.0, 12.0),
            (170.0, 90.0, 12.0),
            (100.0, 60.0, 50.0),
        ] {
            let v = builder::vertex(Point3::new(x + dia / 2.0, 0.0, z));
            let circle = builder::rsweep(&v, Point3::new(x, 0.0, z), Vector3::unit_y(), Rad(TAU));
            wires.push(circle);
        }
        let face = builder::try_attach_plane(&wires).expect("穴あきの板");
        builder::tsweep(&face, Vector3::new(0.0, 15.0, 0.0))
    }

    /// STEP に書き出して読み込んだ部品 (表示と同じ経路) でも簡略化でき、面の番号が表示と合う
    #[test]
    fn imported_step_part_is_defeatured() {
        use truck_stepio::out::{CompleteStepDisplay, StepModel};
        let solid = bracket().compress();
        let text =
            CompleteStepDisplay::new(StepModel::from(&solid), Default::default()).to_string();
        let file = crate::step_import::import_step(&text, "bracket.step").unwrap();
        let shape = find_shape(&file.root).expect("形状がある");

        let options = DefeatureOptions {
            max_hole_diameter: 20.0,
            max_fillet_radius: 15.0,
        };
        let (mesh, stats) = defeature_shape(&shape.breps, &options).expect("簡略化できる");
        assert_eq!(
            stats,
            DefeatureStats {
                holes: 4,
                fillets: 4
            }
        );
        assert!(mesh.triangle_count() < shape.mesh.triangle_count() / 2);
        // 残った面 (上面) の番号は元の表示と同じで、同じ面の三角形を指す
        let top = shape
            .mesh
            .ray_cast(
                bevy::math::Vec3::new(60.0, 50.0, 60.0),
                bevy::math::Vec3::NEG_Y,
            )
            .unwrap();
        let simplified_top = mesh
            .ray_cast(
                bevy::math::Vec3::new(60.0, 50.0, 60.0),
                bevy::math::Vec3::NEG_Y,
            )
            .unwrap();
        assert_eq!(top.0, simplified_top.0);
        // 小さな穴があった場所にも当たるようになる (大きな穴は残る)
        let at_hole = bevy::math::Vec3::new(30.0, 50.0, 30.0);
        assert!(
            shape
                .mesh
                .ray_cast(at_hole, bevy::math::Vec3::NEG_Y)
                .is_none()
        );
        assert!(mesh.ray_cast(at_hole, bevy::math::Vec3::NEG_Y).is_some());
        let at_big_hole = bevy::math::Vec3::new(100.0, 50.0, 60.0);
        assert!(
            mesh.ray_cast(at_big_hole, bevy::math::Vec3::NEG_Y)
                .is_none()
        );

        // 消すものが無い条件なら None
        let nothing = DefeatureOptions {
            max_hole_diameter: 5.0,
            max_fillet_radius: 1.0,
        };
        assert!(defeature_shape(&shape.breps, &nothing).is_none());
    }

    fn find_shape(
        node: &crate::step_import::ImportNode,
    ) -> Option<std::sync::Arc<crate::step_import::ImportedShape>> {
        node.shape
            .clone()
            .or_else(|| node.children.iter().find_map(find_shape))
    }
}
