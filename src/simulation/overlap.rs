//! 干渉している部分 (2部品の共通部分 A∩B) の表面メッシュと体積を求める
//!
//! A∩B の表面は次の2つでできている。
//!
//! - A の表面のうち、B の内側にある部分
//! - B の表面のうち、A の内側にある部分
//!
//! そこで両方の表面の三角形を「相手の内側か」で振り分ける。相手の表面をまたぐ
//! 三角形は4分割をくり返して細かくし、境界付近だけ解像度を上げる。
//! 両者の面がぴったり重なる場所 (同じ床に置いた箱の底面など) は A 側だけから取る。
//!
//! 境界は分割の細かさ分だけギザギザになるので、輪郭は交線 (`PairResult::curves`)
//! で補って描く。体積も同じ精度の近似値。

use bevy::math::DVec3;

use super::geometry::{
    Aabb, Triangle, closest_points_on_triangles, longest_edge, signed_volume, subdivide,
    triangle_aabb, triangle_centroid, triangle_normal,
};
use super::interference::Body;

/// 共通部分の外接箱の対角線を、この数で割った長さまで細かくする
const RESOLUTION: f64 = 48.0;
/// 細分割で増やす三角形の上限 (重すぎる形状で止まらないように)
const MAX_TRIANGLES: usize = 200_000;

/// 干渉部分の立体 (ワールド座標、外向き)
#[derive(Clone, Debug, PartialEq)]
pub struct OverlapRegion {
    pub triangles: Vec<Triangle>,
    /// 体積 (近似値)
    pub volume: f64,
}

pub fn overlap_region(a: &Body, b: &Body, tol: f64) -> OverlapRegion {
    let region = a
        .mesh
        .aabb()
        .transformed(&a.pose)
        .intersection(&b.mesh.aabb().transformed(&b.pose))
        .unwrap_or(Aabb::EMPTY);
    let mut builder = Builder {
        tol,
        min_edge: (region.diagonal() / RESOLUTION).max(tol * 10.0),
        region: region.expanded(tol),
        triangles: Vec::new(),
    };
    builder.collect(a, b, Side::A);
    builder.collect(b, a, Side::B);
    let volume = builder.triangles.iter().map(signed_volume).sum();
    OverlapRegion {
        triangles: builder.triangles,
        volume,
    }
}

/// いま振り分けている表面が A と B のどちらのものか
#[derive(Clone, Copy, PartialEq)]
enum Side {
    A,
    B,
}

/// 表面上の点が相手の部品に対してどこにあるか
enum Location {
    Inside,
    Outside,
    /// 相手の表面上。`same_direction` は両者の法線が同じ向き (面一) か。
    OnSurface {
        same_direction: bool,
    },
}

struct Builder {
    tol: f64,
    min_edge: f64,
    /// 両部品の外接箱の共通部分。ここから外れる三角形は最初から捨てる。
    region: Aabb,
    triangles: Vec<Triangle>,
}

impl Builder {
    /// `owner` の表面のうち `other` の内側にある部分を集める。
    fn collect(&mut self, owner: &Body, other: &Body, side: Side) {
        for local in owner.mesh.triangles() {
            let world = owner.pose.apply_triangle(local);
            self.classify(&world, other, side);
        }
    }

    fn classify(&mut self, t: &Triangle, other: &Body, side: Side) {
        if !self.region.overlaps(&triangle_aabb(t)) {
            return;
        }
        let can_split = longest_edge(t) > self.min_edge && self.triangles.len() < MAX_TRIANGLES;
        if can_split && self.touches_surface(t, other) {
            for child in subdivide(t) {
                self.classify(&child, other, side);
            }
            return;
        }
        let keep = match self.locate(t, other) {
            Location::Inside => true,
            Location::Outside => false,
            // 面一の部分は二重にならないよう A 側だけから取る
            Location::OnSurface { same_direction } => same_direction && side == Side::A,
        };
        if keep {
            self.triangles.push(*t);
        }
    }

    /// 三角形の中心が相手の内側・外側・表面上のどれか
    fn locate(&self, t: &Triangle, other: &Body) -> Location {
        let center = triangle_centroid(t);
        match other.closest_point(center) {
            Some((q, normal)) if q.distance(center) <= self.tol => Location::OnSurface {
                same_direction: normal.dot(triangle_normal(t)) > 0.0,
            },
            _ if other.contains(center) => Location::Inside,
            _ => Location::Outside,
        }
    }

    /// 三角形が相手の表面と交わる・接するか (= 内外がまざっている可能性がある)
    fn touches_surface(&self, t: &Triangle, other: &Body) -> bool {
        let to_local = other.pose.inverse();
        let local = to_local.apply_triangle(t);
        let mut touches = false;
        other
            .mesh
            .for_each_in(&triangle_aabb(&local).expanded(self.tol), |candidate| {
                if !touches {
                    let (pa, pb) = closest_points_on_triangles(&local, candidate);
                    touches = pa.distance(pb) <= self.tol;
                }
            });
        touches
    }
}

/// 表示用に、三角形の頂点をまとめて f32 にする
pub fn to_f32_positions(triangles: &[Triangle]) -> Vec<[f32; 3]> {
    triangles
        .iter()
        .flatten()
        .map(|p: &DVec3| p.as_vec3().to_array())
        .collect()
}
