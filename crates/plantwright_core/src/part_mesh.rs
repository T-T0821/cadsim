//! 部品1つ分の表示用メッシュ (f32): 面をまとめた三角形・稜線・選択用の BVH・簡略形状
//!
//! 面ごとに別のメッシュ (エンティティ) にすると、数千部品で数十万の描画命令になって重い。
//! そこで部品の面をすべて1つのメッシュにまとめ、どの三角形がどの面かは表で持つ。
//! 同じ形状の部品はこのメッシュを共有するので、Bevy がまとめて (インスタンシングで) 描ける。
//!
//! Bevy に依存しない計算だけを置き、Bevy の `Mesh` への変換は viewport.rs が行う。

use glam::{DVec3, Vec3};
use std::collections::HashMap;

use crate::bvh::{Bounds, Bvh, ray_triangle};
use crate::kernel::FaceMesh;

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
    /// 面メッシュ (f64) と稜線から作る
    pub fn new(faces: &[FaceMesh], edges: &[Vec<DVec3>]) -> PartMesh {
        PartMesh::from_faces(
            faces.iter().enumerate().map(|(i, poly)| (i as u32, poly)),
            edges,
        )
    }

    /// 面の番号を指定して作る (形状を簡略化して面が抜けても、元の番号のままにするため)
    pub fn with_face_ids(faces: &[(u32, FaceMesh)], edges: &[Vec<DVec3>]) -> PartMesh {
        PartMesh::from_faces(faces.iter().map(|(i, poly)| (*i, poly)), edges)
    }

    fn from_faces<'a>(
        faces: impl Iterator<Item = (u32, &'a FaceMesh)>,
        edges: &[Vec<DVec3>],
    ) -> PartMesh {
        let mut positions = Vec::new();
        let mut normals = Vec::new();
        let mut triangle_face = Vec::new();
        for (face, poly) in faces {
            for tri in &poly.triangles {
                let [a, b, c] = tri.map(|v| poly.positions[v as usize].as_vec3());
                let flat = (b - a).cross(c - a).normalize_or_zero();
                if flat == Vec3::ZERO {
                    continue; // つぶれた三角形 (簡略化で頂点が重なったものなど)
                }
                for (&v, p) in tri.iter().zip([a, b, c]) {
                    // 曲面はカーネルが付けた頂点法線を使う。法線が三角形の裏を向いているときや、
                    // 法線が無いときは平らな法線にする。
                    let n = poly
                        .normals
                        .get(v as usize)
                        .map(|n| n.as_vec3().normalize_or_zero())
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
            .map(|line| line.iter().map(|p| p.as_vec3()).collect())
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

    /// 面の向き (面の三角形の法線を面積で重み付けして平均したもの)。
    /// 曲面では平均した向きになる。面が無ければ None
    pub fn face_normal(&self, face: usize) -> Option<Vec3> {
        let mut sum = Vec3::ZERO;
        for (i, _) in self
            .triangle_face
            .iter()
            .enumerate()
            .filter(|&(_, &f)| f as usize == face)
        {
            let [a, b, c] = self.triangle(i);
            // 外積の長さは三角形の面積の2倍なので、そのまま重みになる
            sum += (b - a).cross(c - a);
        }
        sum.try_normalize()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Brep;
    use glam::DMat4;

    /// 箱 (2 x 1 x 1、6面) の表示メッシュ
    fn box_mesh() -> PartMesh {
        let solid = Brep::cuboid(DVec3::new(2.0, 1.0, 1.0))
            .unwrap()
            .transformed(DMat4::from_translation(DVec3::new(-1.0, -0.5, -0.5)))
            .unwrap();
        let meshed = solid.tessellate(0.01, 25.0);
        let faces: Vec<FaceMesh> = meshed.faces.into_iter().filter_map(|f| f.mesh).collect();
        PartMesh::new(&faces, &meshed.edges)
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
        assert!(mesh.face_normal(face).unwrap().distance(Vec3::Y) < 1e-5);
        assert_eq!(mesh.face_normal(999), None);
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
