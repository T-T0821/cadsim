//! 部品1つ分の表示用メッシュ (f32): 面をまとめた三角形・稜線・選択用の BVH・簡略形状
//!
//! 面ごとに別のメッシュ (エンティティ) にすると、数千部品で数十万の描画命令になって重い。
//! そこで部品の面をすべて1つのメッシュにまとめ、どの三角形がどの面かは表で持つ。
//! 同じ形状の部品はこのメッシュを共有するので、Bevy がまとめて (インスタンシングで) 描ける。
//!
//! Bevy に依存しない計算だけを置き、Bevy の `Mesh` への変換は viewport.rs が行う。

use bevy::math::Vec3;
use std::collections::HashMap;
use truck_meshalgo::prelude::*;
use truck_topology::Shell;

use crate::bvh::{Bounds, Bvh, ray_triangle};

pub struct PartMesh {
    /// 三角形ごとに3頂点ずつ並べた位置と法線 (頂点は共有しない)
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    /// 三角形ごとの面番号
    pub triangle_face: Vec<u32>,
    /// 稜線のポリライン
    pub edges: Vec<Vec<Vec3>>,
    pub bounds: Bounds,
    /// 遠くから見たとき用の粗いメッシュ (位置, 法線)。簡略化しても減らない形状では None。
    pub coarse: Option<(Vec<Vec3>, Vec<Vec3>)>,
    /// 三角形の BVH (クリックで面を引くため)
    bvh: Bvh,
}

/// 簡略形状を作るときの格子の分割数 (部品のいちばん長い辺をこの数に分ける)
const COARSE_CELLS: u32 = 10;

impl PartMesh {
    /// truck の面メッシュ (f64) と稜線から作る
    pub fn new(faces: &[PolygonMesh], edges: &[Vec<Point3>]) -> PartMesh {
        PartMesh::from_faces(
            faces.iter().enumerate().map(|(i, poly)| (i as u32, poly)),
            edges,
        )
    }

    /// 面の番号を指定して作る (形状を簡略化して面が抜けても、元の番号のままにするため)
    pub fn with_face_ids(faces: &[(u32, PolygonMesh)], edges: &[Vec<Point3>]) -> PartMesh {
        PartMesh::from_faces(faces.iter().map(|(i, poly)| (*i, poly)), edges)
    }

    fn from_faces<'a>(
        faces: impl Iterator<Item = (u32, &'a PolygonMesh)>,
        edges: &[Vec<Point3>],
    ) -> PartMesh {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut triangle_face = Vec::new();
        for (face, poly) in faces {
            let points = poly.positions();
            let poly_normals = poly.normals();
            for tri in poly.faces().triangle_iter() {
                let [a, b, c] = tri.map(|v| to_vec3(&points[v.pos]));
                let flat = (b - a).cross(c - a).normalize_or_zero();
                if flat == Vec3::ZERO {
                    continue; // つぶれた三角形 (簡略化で頂点が重なったものなど)
                }
                for (v, p) in tri.iter().zip([a, b, c]) {
                    // 曲面は truck が付けた頂点法線を使う。面の向きを反転して法線が
                    // 三角形の裏を向いているときや、法線が無いときは平らな法線にする。
                    let n = v
                        .nor
                        .and_then(|i| poly_normals.get(i))
                        .map(|n| to_vec3(&Point3::new(n.x, n.y, n.z)).normalize_or_zero())
                        .filter(|n| n.dot(flat) > 0.0)
                        .unwrap_or(flat);
                    positions.push(p);
                    normals.push(n);
                }
                triangle_face.push(face);
            }
        }
        let edges: Vec<Vec<Vec3>> = edges
            .iter()
            .map(|line| line.iter().map(to_vec3).collect())
            .collect();
        PartMesh::from_triangles(positions, normals, triangle_face, edges)
    }

    fn from_triangles(
        positions: Vec<Vec3>,
        normals: Vec<Vec3>,
        triangle_face: Vec<u32>,
        edges: Vec<Vec<Vec3>>,
    ) -> PartMesh {
        let triangle_bounds: Vec<Bounds> = positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| Bounds::from_points(*t))
            .collect();
        let bounds = Bounds::from_points(positions.iter().chain(edges.iter().flatten()).copied());
        let mut mesh = PartMesh {
            bvh: Bvh::build(&triangle_bounds),
            positions,
            normals,
            triangle_face,
            edges,
            bounds,
            coarse: None,
        };
        mesh.coarse = mesh.simplified(COARSE_CELLS);
        mesh
    }

    pub fn triangle_count(&self) -> usize {
        self.triangle_face.len()
    }

    fn triangle(&self, index: usize) -> [Vec3; 3] {
        [0, 1, 2].map(|k| self.positions[index * 3 + k])
    }

    /// 部品の座標系のレイ `origin + t * dir` が最初に当たる面と t
    pub fn ray_cast(&self, origin: Vec3, dir: Vec3) -> Option<(usize, f32)> {
        let (tri, t) = self.bvh.cast(origin, dir, |i, _| {
            ray_triangle(origin, dir, self.triangle(i as usize))
        })?;
        Some((self.triangle_face[tri as usize] as usize, t))
    }

    /// 1つの面だけの三角形 (選択した面の強調表示に使う)
    pub fn face_triangles(&self, face: usize) -> (Vec<Vec3>, Vec<Vec3>) {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        for (i, _) in self
            .triangle_face
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f as usize == face)
        {
            positions.extend_from_slice(&self.positions[i * 3..i * 3 + 3]);
            normals.extend_from_slice(&self.normals[i * 3..i * 3 + 3]);
        }
        (positions, normals)
    }

    /// 遠くから見たとき用の粗いメッシュ (頂点クラスタリング)。
    ///
    /// 部品の外接箱を `cells` 分割した格子で、同じマスに入った頂点を1つにまとめる。
    /// 3頂点が別々のマスに残った三角形だけを残すので、細かい丸みやボルト穴が消える。
    /// 三角形があまり減らない (箱のような単純な) 形状では None を返し、元のメッシュを使う。
    pub fn simplified(&self, cells: u32) -> Option<(Vec<Vec3>, Vec<Vec3>)> {
        let size = self.bounds.max - self.bounds.min;
        let cell = size.max_element() / cells as f32;
        if self.triangle_count() < 64 || cell <= 0.0 {
            return None;
        }
        let key = |p: Vec3| ((p - self.bounds.min) / cell).floor().as_ivec3();

        // マスごとに頂点の平均を代表点にする
        let mut sums: HashMap<_, (Vec3, u32)> = HashMap::new();
        for &p in &self.positions {
            let entry = sums.entry(key(p)).or_insert((Vec3::ZERO, 0));
            entry.0 += p;
            entry.1 += 1;
        }
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for tri in self.positions.as_chunks::<3>().0 {
            let keys = [key(tri[0]), key(tri[1]), key(tri[2])];
            if keys[0] == keys[1] || keys[1] == keys[2] || keys[0] == keys[2] {
                continue; // つぶれて線か点になった
            }
            // 同じマスの組の三角形は1つで足りる (向きは保ったまま並びをそろえて比べる)
            let first = (0..3).min_by_key(|&i| keys[i].to_array()).unwrap_or(0);
            let canonical = [0, 1, 2].map(|k| keys[(first + k) % 3]);
            if !seen.insert(canonical) {
                continue;
            }
            let [a, b, c] = keys.map(|k| {
                let (sum, n) = sums[&k];
                sum / n as f32
            });
            let n = (b - a).cross(c - a).normalize_or_zero();
            if n == Vec3::ZERO {
                continue;
            }
            positions.extend([a, b, c]);
            normals.extend([n; 3]);
        }
        // 半分以上減らないなら簡略化する意味がない
        (positions.len() * 2 <= self.positions.len() && !positions.is_empty())
            .then_some((positions, normals))
    }
}

/// テッセレーション済みのシェルの稜線 (ポリライン)。
/// `Shell::edge_iter` は面ごとに稜線を返して隣り合う面で重複するので、圧縮形式から取る。
pub fn shell_edges(
    shell: &Shell<Point3, PolylineCurve<Point3>, Option<PolygonMesh>>,
) -> Vec<Vec<Point3>> {
    shell
        .compress()
        .edges
        .into_iter()
        .map(|edge| edge.curve.0)
        .collect()
}

fn to_vec3(p: &Point3) -> Vec3 {
    Vec3::new(p.x as f32, p.y as f32, p.z as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BoxPart;

    /// 箱 (6面) の表示メッシュ
    fn box_mesh() -> PartMesh {
        let part = BoxPart {
            size: [2.0, 1.0, 1.0],
            position: [0.0; 3],
            rotation_deg: [0.0; 3],
        };
        let meshed = part.build_solid().triangulation(0.01);
        let shell = &meshed.boundaries()[0];
        let faces: Vec<PolygonMesh> = shell
            .face_iter()
            .map(|f| {
                let mut poly = f.surface().unwrap_or_default();
                if !f.orientation() {
                    poly.invert();
                }
                poly
            })
            .collect();
        PartMesh::new(&faces, &shell_edges(shell))
    }

    #[test]
    fn merged_mesh_keeps_face_ids() {
        let mesh = box_mesh();
        let faces: std::collections::HashSet<u32> = mesh.triangle_face.iter().copied().collect();
        assert_eq!(faces.len(), 6);
        assert_eq!(mesh.edges.len(), 12);
        assert!((mesh.bounds.max - Vec3::new(1.0, 0.5, 0.5)).length() < 1e-6);
    }

    /// 上から撃ったレイは上面 (+Y) に当たり、その面だけを取り出せる
    #[test]
    fn ray_cast_finds_top_face() {
        let mesh = box_mesh();
        let (face, t) = mesh
            .ray_cast(Vec3::new(0.3, 5.0, 0.1), Vec3::NEG_Y)
            .unwrap();
        assert!((t - 4.5).abs() < 1e-5);
        let (positions, normals) = mesh.face_triangles(face);
        assert!(positions.iter().all(|p| (p.y - 0.5).abs() < 1e-6));
        assert!(normals.iter().all(|n| n.y > 0.99));
        assert_eq!(mesh.ray_cast(Vec3::new(5.0, 5.0, 0.0), Vec3::NEG_Y), None);
    }

    #[test]
    fn simplified_reduces_round_parts_only() {
        // 箱は三角形が少ないので簡略化しない
        assert!(box_mesh().simplified(8).is_none());

        // 細かく分割した球 (緯度経度) は大きく減る
        let mut positions = Vec::new();
        let (rings, segments) = (40, 80);
        let point = |i: u32, j: u32| {
            let theta = std::f32::consts::PI * i as f32 / rings as f32;
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            )
        };
        for i in 0..rings {
            for j in 0..segments {
                let (a, b, c, d) = (
                    point(i, j),
                    point(i + 1, j),
                    point(i + 1, j + 1),
                    point(i, j + 1),
                );
                positions.extend([a, c, b, a, d, c]);
            }
        }
        let count = positions.len() / 3;
        let sphere =
            PartMesh::from_triangles(positions.clone(), positions, vec![0; count], Vec::new());
        let (coarse, _) = sphere.simplified(8).unwrap();
        assert!(
            coarse.len() / 3 < count / 10,
            "{} -> {}",
            count,
            coarse.len() / 3
        );
        // 形はおおむね保つ (代表点は元の球の内側、中心から遠すぎない)
        assert!(
            coarse
                .iter()
                .all(|p| p.length() <= 1.0 + 1e-5 && p.length() > 0.6)
        );
    }
}
