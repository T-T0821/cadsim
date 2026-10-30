//! 幾何計算の基本部品 (すべて f64)
//!
//! - `Pose`: 部品の姿勢 (回転 + 平行移動)
//! - `Aabb`: 軸に平行な外接箱。重い計算の前のふるい分けに使う
//! - 三角形どうしの交差・最短距離、レイと三角形の交差
//!
//! Bevy や truck には依存させず、単体テストできる純粋な関数だけを置く。

use bevy::math::{DMat4, DQuat, DVec3};

/// 3頂点の三角形。頂点の並びは反時計回りが表 (外向き法線) とする。
pub type Triangle = [DVec3; 3];

// ---------------------------------------------------------------------------
// 姿勢
// ---------------------------------------------------------------------------

/// 剛体の姿勢。部品のローカル座標 → ワールド座標の変換を表す。
///
/// タイミングチャートで部品を動かすときは、形状 (メッシュ) はそのままで
/// この姿勢だけを毎フレーム書き換える想定。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub rotation: DQuat,
    pub translation: DVec3,
}

impl Default for Pose {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        rotation: DQuat::IDENTITY,
        translation: DVec3::ZERO,
    };

    /// 配置行列から姿勢を取り出す (拡大縮小は無視する)
    pub fn from_matrix(m: DMat4) -> Self {
        let (_, rotation, translation) = m.to_scale_rotation_translation();
        Self {
            rotation,
            translation,
        }
    }

    pub fn apply(&self, p: DVec3) -> DVec3 {
        self.rotation * p + self.translation
    }

    pub fn apply_triangle(&self, t: &Triangle) -> Triangle {
        t.map(|p| self.apply(p))
    }

    pub fn inverse(&self) -> Pose {
        let rotation = self.rotation.inverse();
        Pose {
            rotation,
            translation: -(rotation * self.translation),
        }
    }

    /// `self ∘ inner`: 先に `inner` を、次に `self` を適用する変換。
    pub fn compose(&self, inner: &Pose) -> Pose {
        Pose {
            rotation: self.rotation * inner.rotation,
            translation: self.rotation * inner.translation + self.translation,
        }
    }
}

// ---------------------------------------------------------------------------
// 外接箱
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: DVec3,
    pub max: DVec3,
}

impl Aabb {
    /// 何も含まない箱。`grow` で点を足していく出発点。
    pub const EMPTY: Aabb = Aabb {
        min: DVec3::INFINITY,
        max: DVec3::NEG_INFINITY,
    };

    pub fn from_points(points: impl IntoIterator<Item = DVec3>) -> Self {
        let mut aabb = Self::EMPTY;
        for p in points {
            aabb.grow(p);
        }
        aabb
    }

    pub fn grow(&mut self, p: DVec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn expanded(&self, margin: f64) -> Aabb {
        Aabb {
            min: self.min - DVec3::splat(margin),
            max: self.max + DVec3::splat(margin),
        }
    }

    pub fn diagonal(&self) -> f64 {
        (self.max - self.min).length()
    }

    pub fn overlaps(&self, other: &Aabb) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }

    /// 2つの箱のすき間 (重なっていれば 0)。最短距離の下限として使える。
    pub fn gap(&self, other: &Aabb) -> f64 {
        let separation = (other.min - self.max)
            .max(self.min - other.max)
            .max(DVec3::ZERO);
        separation.length()
    }

    pub fn intersection(&self, other: &Aabb) -> Option<Aabb> {
        let aabb = Aabb {
            min: self.min.max(other.min),
            max: self.max.min(other.max),
        };
        aabb.min.cmple(aabb.max).all().then_some(aabb)
    }

    /// 姿勢で動かした後の外接箱 (8隅を変換して囲み直すので少し大きめになる)。
    pub fn transformed(&self, pose: &Pose) -> Aabb {
        let (lo, hi) = (self.min, self.max);
        Aabb::from_points((0..8).map(|i| {
            let pick = |bit: usize, a: f64, b: f64| if i & bit == 0 { a } else { b };
            pose.apply(DVec3::new(
                pick(1, lo.x, hi.x),
                pick(2, lo.y, hi.y),
                pick(4, lo.z, hi.z),
            ))
        }))
    }
}

pub fn triangle_aabb(t: &Triangle) -> Aabb {
    Aabb::from_points(*t)
}

// ---------------------------------------------------------------------------
// 三角形の基本量
// ---------------------------------------------------------------------------

/// 単位法線 (つぶれた三角形なら 0 ベクトル)
pub fn triangle_normal(t: &Triangle) -> DVec3 {
    (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero()
}

pub fn triangle_centroid(t: &Triangle) -> DVec3 {
    (t[0] + t[1] + t[2]) / 3.0
}

pub fn longest_edge(t: &Triangle) -> f64 {
    (t[1] - t[0])
        .length()
        .max((t[2] - t[1]).length())
        .max((t[0] - t[2]).length())
}

/// 原点と三角形がつくる四面体の符号付き体積。閉じたメッシュで合計すると体積になる。
pub fn signed_volume(t: &Triangle) -> f64 {
    t[0].dot(t[1].cross(t[2])) / 6.0
}

/// いちばん長い辺の中点で2つに分ける (向きは元の三角形と同じ)。
///
/// 4分割と違い、円柱の側面のような細長い三角形でも形がつぶれず、
/// 分割の数が「境界の長さ」に比例する程度で済む。
pub fn bisect(t: &Triangle) -> [Triangle; 2] {
    // 頂点を回して、いちばん長い辺を a-b にする (回しても向きは変わらない)
    let edge = |i: usize| t[i].distance_squared(t[(i + 1) % 3]);
    let first = (0..3)
        .max_by(|&i, &j| edge(i).total_cmp(&edge(j)))
        .unwrap_or(0);
    let [a, b, c] = [t[first], t[(first + 1) % 3], t[(first + 2) % 3]];
    let m = (a + b) * 0.5;
    [[a, m, c], [m, b, c]]
}

// ---------------------------------------------------------------------------
// 三角形どうしの交差
// ---------------------------------------------------------------------------

/// 2つの三角形の交わり方
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TriangleContact {
    /// 離れている
    Separate,
    /// 同一平面上にある (重なっていても「接触」扱いで、交線は出さない)
    Coplanar,
    /// 交わっていて、交線 `segment` を持つ。
    /// `proper` は「互いに相手の平面を貫いている」= 食い込んでいることを表す。
    /// 面が接しているだけ・辺や角が触れているだけなら false。
    Crossing { segment: [DVec3; 2], proper: bool },
}

/// 三角形 `a` と `b` の交差を調べる。`eps` 以内の距離は「平面上」とみなす。
pub fn intersect_triangles(a: &Triangle, b: &Triangle, eps: f64) -> TriangleContact {
    let na = triangle_normal(a);
    let nb = triangle_normal(b);
    if na == DVec3::ZERO || nb == DVec3::ZERO {
        return TriangleContact::Separate;
    }
    // 相手の平面からの符号付き距離
    let dist_b = b.map(|p| na.dot(p - a[0]));
    let dist_a = a.map(|p| nb.dot(p - b[0]));
    if one_side(&dist_b, eps) || one_side(&dist_a, eps) {
        return TriangleContact::Separate;
    }
    let line_dir = na.cross(nb);
    if dist_b.iter().all(|d| d.abs() <= eps) || line_dir.length_squared() < 1e-24 {
        return TriangleContact::Coplanar;
    }
    let line_dir = line_dir.normalize();

    // それぞれの三角形が相手の平面で切られる区間 (交線方向への射影で表す)
    let (Some(span_a), Some(span_b)) = (
        plane_section(a, &dist_a, line_dir, eps),
        plane_section(b, &dist_b, line_dir, eps),
    ) else {
        return TriangleContact::Separate;
    };
    // 2つの区間の重なりが交線
    let start = if span_a.lo.0 > span_b.lo.0 {
        span_a.lo
    } else {
        span_b.lo
    };
    let end = if span_a.hi.0 < span_b.hi.0 {
        span_a.hi
    } else {
        span_b.hi
    };
    if start.0 > end.0 + eps {
        return TriangleContact::Separate;
    }
    // 角どうしが1点で触れているだけ (交線の長さが 0) の場合も食い込みではない
    let proper = straddles(&dist_a, eps) && straddles(&dist_b, eps) && end.0 - start.0 > eps;
    TriangleContact::Crossing {
        segment: [start.1, end.1],
        proper,
    }
}

/// 全頂点が平面の片側 (`eps` より外) にある
fn one_side(dist: &[f64; 3], eps: f64) -> bool {
    dist.iter().all(|&d| d > eps) || dist.iter().all(|&d| d < -eps)
}

/// 平面の両側に頂点がある (平面を貫いている)
fn straddles(dist: &[f64; 3], eps: f64) -> bool {
    dist.iter().any(|&d| d > eps) && dist.iter().any(|&d| d < -eps)
}

/// 交線方向への射影値と、その点
struct Span {
    lo: (f64, DVec3),
    hi: (f64, DVec3),
}

/// 三角形と平面の交わり (線分) を、交線方向 `dir` に射影した区間で返す。
fn plane_section(t: &Triangle, dist: &[f64; 3], dir: DVec3, eps: f64) -> Option<Span> {
    let mut span: Option<Span> = None;
    let mut add = |p: DVec3| {
        let s = dir.dot(p);
        match &mut span {
            None => {
                span = Some(Span {
                    lo: (s, p),
                    hi: (s, p),
                })
            }
            Some(span) => {
                if s < span.lo.0 {
                    span.lo = (s, p);
                }
                if s > span.hi.0 {
                    span.hi = (s, p);
                }
            }
        }
    };
    for i in 0..3 {
        let j = (i + 1) % 3;
        let (di, dj) = (dist[i], dist[j]);
        if di.abs() <= eps {
            add(t[i]);
        }
        if (di > eps && dj < -eps) || (di < -eps && dj > eps) {
            add(t[i] + (t[j] - t[i]) * (di / (di - dj)));
        }
    }
    span
}

// ---------------------------------------------------------------------------
// 最短距離
// ---------------------------------------------------------------------------

/// 点 `p` に最も近い三角形上の点 (Ericson『Real-Time Collision Detection』5.1.5)
pub fn closest_point_on_triangle(p: DVec3, t: &Triangle) -> DVec3 {
    let [a, b, c] = *t;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// 線分 p1-q1 と p2-q2 の最近点の組 (Ericson 5.1.9)
pub fn closest_points_on_segments(p1: DVec3, q1: DVec3, p2: DVec3, q2: DVec3) -> (DVec3, DVec3) {
    let (d1, d2, r) = (q1 - p1, q2 - p2, p1 - p2);
    let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
    const EPS: f64 = 1e-18;
    let (s, t) = if a <= EPS && e <= EPS {
        (0.0, 0.0)
    } else if a <= EPS {
        (0.0, (f / e).clamp(0.0, 1.0))
    } else {
        let c = d1.dot(r);
        if e <= EPS {
            ((-c / a).clamp(0.0, 1.0), 0.0)
        } else {
            let b = d1.dot(d2);
            let denom = a * e - b * b;
            let mut s = if denom > EPS {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t = (b * s + f) / e;
            if t < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            }
            (s, t)
        }
    };
    (p1 + d1 * s, p2 + d2 * t)
}

/// 2つの三角形の最近点の組。交わっている場合は交点 (距離 0) を返す。
pub fn closest_points_on_triangles(a: &Triangle, b: &Triangle) -> (DVec3, DVec3) {
    if let TriangleContact::Crossing { segment, .. } = intersect_triangles(a, b, 0.0) {
        return (segment[0], segment[0]);
    }
    // 交わっていなければ、最近点は「頂点と相手の面」か「辺と辺」のどれか
    let mut best = (a[0], b[0]);
    let mut best_d = f64::INFINITY;
    let mut consider = |pa: DVec3, pb: DVec3| {
        let d = pa.distance_squared(pb);
        if d < best_d {
            best_d = d;
            best = (pa, pb);
        }
    };
    for &p in a {
        consider(p, closest_point_on_triangle(p, b));
    }
    for &p in b {
        consider(closest_point_on_triangle(p, a), p);
    }
    for i in 0..3 {
        for j in 0..3 {
            let (pa, pb) = closest_points_on_segments(a[i], a[(i + 1) % 3], b[j], b[(j + 1) % 3]);
            consider(pa, pb);
        }
    }
    best
}

// ---------------------------------------------------------------------------
// レイ
// ---------------------------------------------------------------------------

/// レイが三角形に当たった位置。`u`, `v` は重心座標 (辺の近くかどうかの判定に使う)。
#[derive(Clone, Copy, Debug)]
pub struct RayHit {
    pub t: f64,
    pub u: f64,
    pub v: f64,
}

impl RayHit {
    /// 辺や頂点のすぐ近くに当たった (内外判定でダブルカウントの恐れがある)
    pub fn is_near_edge(&self, eps: f64) -> bool {
        self.u < eps || self.v < eps || self.u + self.v > 1.0 - eps
    }
}

/// Möller–Trumbore 法。裏表どちらから当たっても返す。
pub fn ray_triangle(origin: DVec3, dir: DVec3, t: &Triangle) -> Option<RayHit> {
    let (e1, e2) = (t[1] - t[0], t[2] - t[0]);
    let pvec = dir.cross(e2);
    let det = e1.dot(pvec);
    if det.abs() < 1e-14 {
        return None;
    }
    let inv = 1.0 / det;
    let tvec = origin - t[0];
    let u = tvec.dot(pvec) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let qvec = tvec.cross(e1);
    let v = dir.dot(qvec) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some(RayHit {
        t: e2.dot(qvec) * inv,
        u,
        v,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> Triangle {
        [a.into(), b.into(), c.into()]
    }

    #[test]
    fn crossing_triangles_give_segment() {
        let a = tri([-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [0.0, 0.0, 1.0]);
        let b = tri([0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -2.0]);
        match intersect_triangles(&a, &b, 1e-9) {
            TriangleContact::Crossing { segment, proper } => {
                assert!(proper);
                assert!(segment[0].y.abs() < 1e-12 && segment[0].x.abs() < 1e-12);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn touching_triangles_are_not_proper() {
        // b は a の平面に下から触れているだけ
        let a = tri([-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [0.0, 0.0, 1.0]);
        let b = tri([-0.5, 0.0, 0.0], [0.5, 0.0, 0.0], [0.0, -1.0, 0.0]);
        match intersect_triangles(&a, &b, 1e-9) {
            TriangleContact::Crossing { proper, .. } => assert!(!proper),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn distance_between_parallel_triangles() {
        let a = tri([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        let b = a.map(|p| p + DVec3::Y * 0.25);
        let (pa, pb) = closest_points_on_triangles(&a, &b);
        assert!((pa.distance(pb) - 0.25).abs() < 1e-12);
    }

    #[test]
    fn pose_inverse_roundtrip() {
        let pose = Pose {
            rotation: DQuat::from_rotation_y(0.7),
            translation: DVec3::new(1.0, 2.0, 3.0),
        };
        let p = DVec3::new(0.3, -0.2, 5.0);
        assert!(pose.inverse().apply(pose.apply(p)).distance(p) < 1e-12);
    }
}
