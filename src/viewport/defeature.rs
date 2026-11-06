//! 形状の簡略化 (デフィーチャリング): 小さな穴とフィレットを消して、表示用の三角形を減らす
//!
//! 大規模アセンブリでは、ボルト穴やフィレット (角の丸み) が三角形の大半を占める。
//! 配置の確認には要らないので、表示用のメッシュからだけ消す (干渉チェックや計測は元の形状で行う)。
//! Bevy に依存しない計算だけを置く。
//!
//! - **穴**: 平面の面にある小さな内側の輪郭 (穴の縁) を見つけ、その穴の内壁・底の面を消して、
//!   縁の輪郭を外した平面を張り直す (縁を外した輪郭で平面を三角形に分け直す)。
//!   内壁の法線が穴の軸を向いているもの (凹) だけを穴とみなし、ボスやピン (凸) は残す。
//! - **フィレット**: 2つの平面の間をなめらかにつなぐ細い曲面を見つけ、消す。両側の平面の縁を
//!   2平面の交線まで延ばすので、角が尖った形になる (メッシュの頂点を動かす)。
//!   フィレットどうしが集まる角の小さな曲面も一緒に消す。
//!
//! 面の番号は簡略化していない表示メッシュと同じ番号で返すので、簡略表示に切り替えても
//! クリックした面の番号 (選択) は変わらない。

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::math::DVec3;

use plantwright_core::kernel::{Brep, FaceMesh, MeshedBrep, MeshedFace, triangulate_planar};
use plantwright_core::part_mesh::PartMesh;
use plantwright_core::shape::{TESSELLATION_ANGLE_DEG, brep_tolerance};

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
    pub faces: Vec<(u32, FaceMesh)>,
    /// 稜線のポリライン
    pub edges: Vec<Vec<DVec3>>,
    pub stats: DefeatureStats,
    /// 簡略化していない表示での面の数 (テッセレーションできなかった面は数えない)
    pub face_count: usize,
}

/// 部品の B-rep から簡略化した表示用メッシュを作る。消すものがなければ None。
/// 面の番号は STEP の読み込みが作る表示メッシュ (`ImportedShape::mesh`) と同じ並び。
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
        let mesh = defeature(&brep.tessellate(tol, TESSELLATION_ANGLE_DEG), options);
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

/// メッシュにした B-rep から小さな穴とフィレットを消し、表示用のメッシュを作る
pub fn defeature(meshed: &MeshedBrep, options: &DefeatureOptions) -> DefeaturedMesh {
    // 元の面の番号 → 表示での面の番号 (テッセレーションできなかった面は表示に入らない)
    let mut display = Vec::with_capacity(meshed.faces.len());
    let mut face_count = 0;
    for face in &meshed.faces {
        display.push(face.mesh.is_some().then_some(face_count as u32));
        face_count += usize::from(face.mesh.is_some());
    }
    let (filled, holes) = remove_small_holes(meshed, options.max_hole_diameter);
    let mut result = collapse_fillets(filled.as_ref().unwrap_or(meshed), options.max_fillet_radius);
    result.faces = result
        .faces
        .into_iter()
        .filter_map(|(face, poly)| Some((display[face as usize]?, poly)))
        .collect();
    result.stats.holes = holes;
    result.face_count = face_count;
    result
}

// ---------------------------------------------------------------------------
// 共通: 面と辺のつながり・平面の判定
// ---------------------------------------------------------------------------

/// 辺 → その辺を輪郭に持つ面
fn edge_faces(meshed: &MeshedBrep) -> Vec<Vec<usize>> {
    let mut out = vec![Vec::new(); meshed.edges.len()];
    for (f, face) in meshed.faces.iter().enumerate() {
        for &e in face.loops.iter().flatten() {
            if !out[e].contains(&f) {
                out[e].push(f);
            }
        }
    }
    out
}

fn triangle_normal([a, b, c]: &[DVec3; 3]) -> DVec3 {
    (b - a).cross(c - a)
}

/// 面が平面なら (単位法線, 原点からの距離) を返す
fn plane_of(poly: &FaceMesh, scale: f64) -> Option<(DVec3, f64)> {
    let normal = poly
        .triangle_points()
        .map(|t| triangle_normal(&t))
        .reduce(|a, b| a + b)?
        .try_normalize()?;
    let d = normal.dot(*poly.positions.first()?);
    let eps = scale * PLANAR_TOLERANCE + 1e-9;
    poly.positions
        .iter()
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
fn loop_points(meshed: &MeshedBrep, edges: &[usize]) -> Vec<DVec3> {
    edges
        .iter()
        .flat_map(|&e| meshed.edges[e].iter().copied())
        .collect()
}

/// 輪郭を1周する点の列 (辺のポリラインを向きをそろえてつなぐ)
fn loop_polygon(meshed: &MeshedBrep, edges: &[usize]) -> Vec<DVec3> {
    let lines: Vec<&Vec<DVec3>> = edges
        .iter()
        .map(|&e| &meshed.edges[e])
        .filter(|l| !l.is_empty())
        .collect();
    let mut out: Vec<DVec3> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let (first, last) = (line[0], line[line.len() - 1]);
        // 前の辺の終わりにつながる向きにする。最初の辺は次の辺につながる向き
        let reversed = match out.last() {
            Some(&end) => end.distance_squared(last) < end.distance_squared(first),
            None => lines.get(i + 1).is_some_and(|next| {
                let ends = [next[0], next[next.len() - 1]];
                let d = |p: DVec3| {
                    ends.iter()
                        .map(|q| p.distance_squared(*q))
                        .fold(f64::MAX, f64::min)
                };
                d(first) < d(last)
            }),
        };
        let points: Vec<DVec3> = if reversed {
            line.iter().rev().copied().collect()
        } else {
            line.to_vec()
        };
        let skip = usize::from(out.last() == points.first());
        out.extend(points.into_iter().skip(skip));
    }
    if out.len() > 1 && out.first() == out.last() {
        out.pop();
    }
    out
}

/// メッシュ全体の大きさ (平面判定の許容差に使う)
fn shell_scale(meshed: &MeshedBrep) -> f64 {
    extent(meshed.edges.iter().flatten().copied()).max(1e-9)
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

/// 小さな穴を消したメッシュ (面の番号は元のまま、消した面はメッシュなし) と、消した穴の数を返す。
/// 消すものが無ければ None
fn remove_small_holes(meshed: &MeshedBrep, max_diameter: f64) -> (Option<MeshedBrep>, usize) {
    if max_diameter <= 0.0 {
        return (None, 0);
    }
    let scale = shell_scale(meshed);
    let edge_faces = edge_faces(meshed);

    // 平面の面にある、小さな内側の輪郭をすべて集める (いちばん大きい輪郭が外周)
    let mut rims: HashMap<usize, Rim> = HashMap::new(); // 辺 → 縁
    let mut rim_list = Vec::new();
    for (f, face) in meshed.faces.iter().enumerate() {
        if face.loops.len() < 2 {
            continue;
        }
        let Some(poly) = face.mesh.as_ref() else {
            continue;
        };
        if plane_of(poly, scale).is_none() {
            continue;
        }
        let sizes: Vec<f64> = face
            .loops
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
                for &e in &face.loops[b] {
                    rims.insert(e, rim);
                }
            }
        }
    }

    let rim_faces: HashSet<usize> = rim_list.iter().map(|r| r.face).collect();
    let outward = outward_sign(meshed);
    let mut removed_faces: HashSet<usize> = HashSet::new();
    let mut removed_rims: HashSet<Rim> = HashSet::new();
    let mut holes = 0;
    for rim in rim_list {
        if removed_rims.contains(&rim) {
            continue; // 貫通穴の反対側の縁としてもう消した
        }
        let Some((region, reached)) = hole_region(meshed, &edge_faces, &rims, &rim_faces, rim)
        else {
            continue;
        };
        if region.iter().any(|f| removed_faces.contains(f))
            || !is_small_hole(meshed, rim, &region, max_diameter, outward)
        {
            continue;
        }
        removed_faces.extend(region);
        removed_rims.extend(reached);
        holes += 1;
    }
    if holes == 0 {
        return (None, 0);
    }

    let mut out = MeshedBrep {
        faces: Vec::with_capacity(meshed.faces.len()),
        edges: meshed.edges.clone(),
    };
    for (f, face) in meshed.faces.iter().enumerate() {
        if removed_faces.contains(&f) {
            out.faces.push(MeshedFace::default());
            continue;
        }
        let loops: Vec<Vec<usize>> = face
            .loops
            .iter()
            .enumerate()
            .filter(|&(b, _)| {
                !removed_rims.contains(&Rim {
                    face: f,
                    boundary: b,
                })
            })
            .map(|(_, l)| l.clone())
            .collect();
        let mesh = if loops.len() == face.loops.len() {
            face.mesh.clone()
        } else {
            refill_plane(meshed, face, &loops)
        };
        out.faces.push(MeshedFace { mesh, loops });
    }
    (Some(out), holes)
}

/// 縁を外した輪郭で平面を三角形に分け直す。向きは元の面に合わせる
fn refill_plane(meshed: &MeshedBrep, face: &MeshedFace, loops: &[Vec<usize>]) -> Option<FaceMesh> {
    let polygons: Vec<Vec<DVec3>> = loops.iter().map(|l| loop_polygon(meshed, l)).collect();
    let mut mesh = triangulate_planar(&polygons)?;
    let (want, _) = plane_of(face.mesh.as_ref()?, shell_scale(meshed))?;
    let have: DVec3 = mesh.triangle_points().map(|t| triangle_normal(&t)).sum();
    if have.dot(want) < 0.0 {
        mesh.invert();
    }
    // 平面の正確な法線
    mesh.normals = vec![want; mesh.positions.len()];
    Some(mesh)
}

/// 縁から穴の内側の面をたどる。ほかの縁 (貫通穴の反対側) でも止まる。
/// 穴の面の集まりと、たどり着いた縁を返す。穴らしくなければ None。
fn hole_region(
    meshed: &MeshedBrep,
    edge_faces: &[Vec<usize>],
    rims: &HashMap<usize, Rim>,
    rim_faces: &HashSet<usize>,
    start: Rim,
) -> Option<(Vec<usize>, Vec<Rim>)> {
    let mut region = Vec::new();
    let mut seen = HashSet::new();
    let mut reached = vec![start];
    let mut queue = VecDeque::new();
    for &e in &meshed.faces[start.face].loops[start.boundary] {
        for &f in &edge_faces[e] {
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
        for &e in meshed.faces[f].loops.iter().flatten() {
            if let Some(&rim) = rims.get(&e)
                && rim.face != f
            {
                // ほかの平面の縁に着いた: そこが穴の出口 (その平面は残す)
                if !reached.contains(&rim) {
                    reached.push(rim);
                }
                continue;
            }
            for &g in &edge_faces[e] {
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
fn is_small_hole(
    meshed: &MeshedBrep,
    rim: Rim,
    region: &[usize],
    max_diameter: f64,
    outward: f64,
) -> bool {
    let scale = shell_scale(meshed);
    let Some((axis, _)) = meshed.faces[rim.face]
        .mesh
        .as_ref()
        .and_then(|poly| plane_of(poly, scale))
    else {
        return false;
    };
    let points = loop_points(meshed, &meshed.faces[rim.face].loops[rim.boundary]);
    if points.is_empty() {
        return false;
    }
    let center = points.iter().sum::<DVec3>() / points.len() as f64;
    let mut score = 0.0;
    for &f in region {
        let Some(poly) = meshed.faces[f].mesh.as_ref() else {
            continue;
        };
        for tri in poly.triangle_points() {
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

/// 面のメッシュの向きが外向きなら 1、内向きなら -1。全体の符号付き体積で確かめる
fn outward_sign(meshed: &MeshedBrep) -> f64 {
    let volume: f64 = meshed
        .faces
        .iter()
        .filter_map(|f| f.mesh.as_ref())
        .flat_map(|m| m.triangle_points())
        .map(|[a, b, c]| a.dot(b.cross(c)))
        .sum();
    if volume < 0.0 { -1.0 } else { 1.0 }
}

// ---------------------------------------------------------------------------
// フィレット
// ---------------------------------------------------------------------------

/// 曲面と平面が接しているとみなす法線の向きの差 (cos)
const TANGENT_COS: f64 = 0.999;

/// 曲面の法線 (面の外向きの単位ベクトル)。カーネルが付けた頂点の法線は曲面の正確な法線
/// なので、粗いテッセレーションでも接する平面の法線と一致する。無ければ三角形の法線を使う。
fn surface_normals(poly: &FaceMesh) -> Vec<DVec3> {
    let vertex: Vec<DVec3> = poly
        .normals
        .iter()
        .filter_map(|n| n.try_normalize())
        .collect();
    if vertex.len() == poly.positions.len() && !vertex.is_empty() {
        // 念のため三角形の向きと比べ、逆なら反転する
        let agree: f64 = poly
            .triangles
            .iter()
            .map(|t| {
                let tri = t.map(|i| poly.positions[i as usize]);
                let n: DVec3 = t.iter().map(|&i| vertex[i as usize]).sum();
                triangle_normal(&tri).dot(n)
            })
            .sum();
        let sign = if agree < 0.0 { -1.0 } else { 1.0 };
        return vertex.into_iter().map(|n| n * sign).collect();
    }
    poly.triangle_points()
        .filter_map(|t| triangle_normal(&t).try_normalize())
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
fn collapse_fillets(meshed: &MeshedBrep, max_radius: f64) -> DefeaturedMesh {
    let scale = shell_scale(meshed);
    let edge_faces = edge_faces(meshed);
    let planes: Vec<Option<(DVec3, f64)>> = meshed
        .faces
        .iter()
        .map(|face| plane_of(face.mesh.as_ref()?, scale))
        .collect();
    let neighbors = |f: usize| -> Vec<usize> {
        let mut out: Vec<usize> = meshed.faces[f]
            .loops
            .iter()
            .flatten()
            .flat_map(|&e| edge_faces[e].iter().copied())
            .filter(|&g| g != f)
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    };

    // フィレット: 曲面で、接する平面がちょうど2つあり、その交線から近いもの
    let mut fillet_lines: HashMap<usize, Line> = HashMap::new();
    if max_radius > 0.0 {
        for (f, face) in meshed.faces.iter().enumerate() {
            if planes[f].is_some() {
                continue;
            }
            let Some(poly) = face.mesh.as_ref() else {
                continue;
            };
            let normals = surface_normals(poly);
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
                .positions
                .iter()
                .map(|p| line.distance(*p))
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
    for (f, face) in meshed.faces.iter().enumerate() {
        if planes[f].is_some() || fillet_lines.contains_key(&f) {
            continue;
        }
        let Some(poly) = face.mesh.as_ref() else {
            continue;
        };
        let around = neighbors(f);
        let touches_fillet = around.iter().any(|g| fillet_lines.contains_key(g));
        let all_simple = around
            .iter()
            .all(|g| fillet_lines.contains_key(g) || planes[*g].is_some());
        let small = extent(poly.positions.iter().copied()) <= max_radius * 2.0;
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
        for &e in meshed.faces[f].loops.iter().flatten() {
            for &p in &meshed.edges[e] {
                moves.entry(key(p)).or_insert_with(|| line.closest(p));
            }
        }
    }
    let moved = |p: DVec3| -> DVec3 { moves.get(&key(p)).copied().unwrap_or(p) };

    let removed = |f: usize| fillet_lines.contains_key(&f) || corners.contains(&f);
    let mut faces = Vec::new();
    for (f, face) in meshed.faces.iter().enumerate() {
        if removed(f) {
            continue;
        }
        let Some(poly) = face.mesh.as_ref() else {
            continue;
        };
        let mut poly = poly.clone();
        if !moves.is_empty() {
            for p in &mut poly.positions {
                *p = moved(*p);
            }
        }
        faces.push((f as u32, poly));
    }

    // 稜線: 消した面どうしの境目と、消した穴の縁 (どの面にも使われていない辺) は捨て、
    // 残りは点を移す。つぶれて点になった線も捨てる。
    let mut edges = Vec::new();
    for (e, faces_of_edge) in edge_faces.iter().enumerate() {
        if faces_of_edge
            .iter()
            .all(|&f| removed(f) || meshed.faces[f].mesh.is_none())
        {
            continue;
        }
        let line: Vec<DVec3> = meshed.edges[e].iter().map(|&p| moved(p)).collect();
        let length: f64 = line.windows(2).map(|w| w[0].distance(w[1])).sum();
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
    use bevy::math::{DMat4, Vec3};
    use plantwright_core::kernel::BooleanKind;
    use plantwright_core::shape::{ImportedShape, mesh_stats, tessellate_breps};

    fn triangle_count(mesh: &DefeaturedMesh) -> usize {
        mesh.faces.iter().map(|(_, p)| p.triangles.len()).sum()
    }

    fn volume(mesh: &DefeaturedMesh) -> f64 {
        let polys: Vec<FaceMesh> = mesh.faces.iter().map(|(_, p)| p.clone()).collect();
        mesh_stats(&polys).volume
    }

    fn defeature_at(solid: &Brep, tol: f64, options: &DefeatureOptions) -> DefeaturedMesh {
        defeature(&solid.tessellate(tol, TESSELLATION_ANGLE_DEG), options)
    }

    fn cuboid(min: DVec3, max: DVec3) -> Brep {
        Brep::cuboid(max - min)
            .unwrap()
            .transformed(DMat4::from_translation(min))
            .unwrap()
    }

    /// `center` を通り Y 方向に伸びる円柱 (y0..y1)
    fn y_cylinder(x: f64, z: f64, radius: f64, y0: f64, y1: f64) -> Brep {
        let m = DMat4::from_translation(DVec3::new(x, y0, z)) * DMat4::from_rotation_x(-FRAC_PI_2);
        Brep::cylinder_z(radius, y1 - y0)
            .unwrap()
            .transformed(m)
            .unwrap()
    }

    /// Z 方向に伸びる円柱 (z0..z1)
    fn z_cylinder(x: f64, y: f64, radius: f64, z0: f64, z1: f64) -> Brep {
        Brep::cylinder_z(radius, z1 - z0)
            .unwrap()
            .transformed(DMat4::from_translation(DVec3::new(x, y, z0)))
            .unwrap()
    }

    fn minus(a: Brep, b: &Brep) -> Brep {
        a.boolean(b, BooleanKind::Difference).unwrap()
    }

    use std::f64::consts::FRAC_PI_2;

    /// 100 x 10 x 60 の板に、直径 `d` の貫通穴を2つあける
    fn plate_with_holes(d: f64) -> Brep {
        let plate = cuboid(DVec3::ZERO, DVec3::new(100.0, 10.0, 60.0));
        let plate = minus(plate, &y_cylinder(25.0, 30.0, d / 2.0, -1.0, 11.0));
        minus(plate, &y_cylinder(75.0, 30.0, d / 2.0, -1.0, 11.0))
    }

    const OFF: DefeatureOptions = DefeatureOptions {
        max_hole_diameter: 0.0,
        max_fillet_radius: 0.0,
    };

    #[test]
    fn small_through_holes_are_filled() {
        let solid = plate_with_holes(12.0);
        let options = DefeatureOptions {
            max_hole_diameter: 20.0,
            max_fillet_radius: 0.0,
        };
        let original = defeature_at(&solid, 0.05, &OFF);
        let simplified = defeature_at(&solid, 0.05, &options);
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
        let options = DefeatureOptions {
            max_hole_diameter: 20.0,
            max_fillet_radius: 0.0,
        };
        assert_eq!(
            defeature_at(&plate_with_holes(30.0), 0.05, &options)
                .stats
                .holes,
            0
        );
    }

    /// 角 (x = w, y = 0 の辺) を半径 `r` で丸めるために削る部分 (z0..z1)
    fn round_off(corner_x: f64, corner_y: f64, sx: f64, sy: f64, r: f64, z0: f64, z1: f64) -> Brep {
        // 角の r x r の正方形から、丸みの円柱を引いたもの
        let (x0, y0) = (corner_x - sx * r, corner_y - sy * r);
        let square = cuboid(
            DVec3::new(x0.min(corner_x), y0.min(corner_y), z0 - 1.0),
            DVec3::new(x0.max(corner_x), y0.max(corner_y), z1 + 1.0),
        );
        minus(square, &z_cylinder(x0, y0, r, z0 - 2.0, z1 + 2.0))
    }

    /// 断面が 40 x 30 で、1つの角を半径 `r` で丸めた長さ 100 の棒
    fn rounded_bar(r: f64) -> Brep {
        let bar = cuboid(DVec3::ZERO, DVec3::new(40.0, 30.0, 100.0));
        minus(bar, &round_off(40.0, 0.0, 1.0, -1.0, r, 0.0, 100.0))
    }

    #[test]
    fn fillet_is_collapsed_to_sharp_corner() {
        let solid = rounded_bar(5.0);
        let on = DefeatureOptions {
            max_hole_diameter: 0.0,
            max_fillet_radius: 10.0,
        };
        let original = defeature_at(&solid, 0.01, &OFF);
        let simplified = defeature_at(&solid, 0.01, &on);
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
        let options = DefeatureOptions {
            max_hole_diameter: 0.0,
            max_fillet_radius: 10.0,
        };
        assert_eq!(
            defeature_at(&rounded_bar(15.0), 0.01, &options)
                .stats
                .fillets,
            0
        );
    }

    /// 角を丸めた (半径 10) 200 x 15 x 120 の板 (XZ 平面、厚みは Y) に、
    /// 直径 12 の穴を4つと直径 50 の穴を1つあけた部品
    fn bracket() -> Brep {
        let (w, t, d, r) = (200.0, 15.0, 120.0, 10.0);
        // XY 平面に置き、Z を厚みにして作ってから回す
        let mut plate = cuboid(DVec3::ZERO, DVec3::new(w, d, t));
        for (cx, cy, sx, sy) in [
            (0.0, 0.0, -1.0, -1.0),
            (w, 0.0, 1.0, -1.0),
            (w, d, 1.0, 1.0),
            (0.0, d, -1.0, 1.0),
        ] {
            plate = minus(plate, &round_off(cx, cy, sx, sy, r, 0.0, t));
        }
        for (x, y, dia) in [
            (30.0, 30.0, 12.0),
            (170.0, 30.0, 12.0),
            (30.0, 90.0, 12.0),
            (170.0, 90.0, 12.0),
            (100.0, 60.0, 50.0),
        ] {
            plate = minus(plate, &z_cylinder(x, y, dia / 2.0, -1.0, t + 1.0));
        }
        // X 軸まわりに 90° 回すと (x, y, z) → (x, -z, y)。厚みを 0..t の Y にする
        let m =
            DMat4::from_translation(DVec3::new(0.0, t, 0.0)) * DMat4::from_rotation_x(FRAC_PI_2);
        plate.transformed(m).unwrap()
    }

    /// 表示と同じ経路 (部品の形状の表示メッシュ) でも簡略化でき、面の番号が表示と合う
    #[test]
    fn part_shape_is_defeatured() {
        let solid = bracket();
        let breps = vec![solid];
        let (faces, edges) = tessellate_breps(&breps, brep_tolerance(&breps));
        let shape = ImportedShape::new(breps, faces, edges, None);

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
        let above = Vec3::new(60.0, 50.0, 60.0);
        let top = shape.mesh.ray_cast(above, Vec3::NEG_Y).unwrap();
        let simplified_top = mesh.ray_cast(above, Vec3::NEG_Y).unwrap();
        assert_eq!(top.0, simplified_top.0);
        // 小さな穴があった場所にも当たるようになる (大きな穴は残る)
        let at_hole = Vec3::new(30.0, 50.0, 30.0);
        assert!(shape.mesh.ray_cast(at_hole, Vec3::NEG_Y).is_none());
        assert!(mesh.ray_cast(at_hole, Vec3::NEG_Y).is_some());
        let at_big_hole = Vec3::new(100.0, 50.0, 60.0);
        assert!(mesh.ray_cast(at_big_hole, Vec3::NEG_Y).is_none());

        // 消すものが無い条件なら None
        let nothing = DefeatureOptions {
            max_hole_diameter: 5.0,
            max_fillet_radius: 1.0,
        };
        assert!(defeature_shape(&shape.breps, &nothing).is_none());
    }
}
