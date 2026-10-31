//! 床の上 (XZ 平面) の2次元の幾何計算
//!
//! 安全機器の検知範囲や柵は「床の上の向きの付いた長方形 + 高さの範囲」で表す。
//! 上方向は +Y。床の座標は (x, z) を `DVec2` の (x, y) に入れる。
//!
//! Bevy に依存しない純粋な計算だけを置く。

use bevy::math::{DVec2, DVec3};

use crate::math::Aabb;

/// 3D の点を床の座標 (x, z) にする
pub fn to_floor(p: DVec3) -> DVec2 {
    DVec2::new(p.x, p.z)
}

/// 床の上の、向きの付いた長方形
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect2 {
    pub center: DVec2,
    /// 長方形のローカル X / Z 方向の半分の長さ
    pub half: DVec2,
    /// Y 軸まわりの回転 (ラジアン)。Bevy と同じく上から見て反時計回りが正。
    pub yaw: f64,
}

impl Rect2 {
    /// 軸に平行な外接箱の床への投影
    pub fn from_aabb(aabb: &Aabb) -> Self {
        let min = to_floor(aabb.min);
        let max = to_floor(aabb.max);
        Self {
            center: (min + max) * 0.5,
            half: (max - min) * 0.5,
            yaw: 0.0,
        }
    }

    /// ローカル X / Z の単位ベクトル (床の座標)
    pub fn axes(&self) -> [DVec2; 2] {
        // Y 軸まわりに yaw 回すと、X 軸 (1,0,0) は (cos, 0, -sin)、Z 軸 (0,0,1) は (sin, 0, cos) になる
        let (s, c) = self.yaw.sin_cos();
        [DVec2::new(c, -s), DVec2::new(s, c)]
    }

    pub fn corners(&self) -> [DVec2; 4] {
        let [ax, az] = self.axes();
        let (x, z) = (ax * self.half.x, az * self.half.y);
        [
            self.center - x - z,
            self.center + x - z,
            self.center + x + z,
            self.center - x + z,
        ]
    }

    /// 重なっているか (分離軸判定)
    pub fn overlaps(&self, other: &Rect2) -> bool {
        let a = self.corners();
        let b = other.corners();
        self.axes().into_iter().chain(other.axes()).all(|axis| {
            let (a_min, a_max) = project(&a, axis);
            let (b_min, b_max) = project(&b, axis);
            a_min <= b_max + 1e-9 && b_min <= a_max + 1e-9
        })
    }

    /// 2つの長方形の最短距離 (重なっていれば 0)
    pub fn distance(&self, other: &Rect2) -> f64 {
        if self.overlaps(other) {
            return 0.0;
        }
        let a = self.corners();
        let b = other.corners();
        let mut best = f64::INFINITY;
        for i in 0..4 {
            for j in 0..4 {
                let d = segment_distance(a[i], a[(i + 1) % 4], b[j], b[(j + 1) % 4]);
                best = best.min(d);
            }
        }
        best
    }
}

fn project(points: &[DVec2; 4], axis: DVec2) -> (f64, f64) {
    points
        .iter()
        .map(|p| p.dot(axis))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| {
            (lo.min(v), hi.max(v))
        })
}

/// 点と線分の最短距離
pub fn point_segment_distance(p: DVec2, a: DVec2, b: DVec2) -> f64 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 <= 0.0 {
        0.0
    } else {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    };
    p.distance(a + ab * t)
}

/// 線分と線分の最短距離 (交わっていれば 0)
pub fn segment_distance(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> f64 {
    if segments_cross(a, b, c, d) {
        return 0.0;
    }
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}

/// 2つの線分が交わるか (端の接触も含む)
pub fn segments_cross(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let d1 = (b - a).perp_dot(c - a);
    let d2 = (b - a).perp_dot(d - a);
    let d3 = (d - c).perp_dot(a - c);
    let d4 = (d - c).perp_dot(b - c);
    if d1 == 0.0 && d2 == 0.0 {
        // 同じ直線上: 線分の範囲が重なるか
        let axis = b - a;
        let (t0, t1) = (0.0, axis.length_squared());
        let (u0, u1) = ((c - a).dot(axis), (d - a).dot(axis));
        return u0.min(u1) <= t1 && u0.max(u1) >= t0;
    }
    d1 * d2 <= 0.0 && d3 * d4 <= 0.0
}

/// 床の長方形に高さの範囲を付けた立体 (検知範囲や柵の形)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Zone {
    pub footprint: Rect2,
    /// 下端と上端の高さ (Y)
    pub y_min: f64,
    pub y_max: f64,
}

impl Zone {
    /// 軸に平行な外接箱と重なるか
    pub fn intersects(&self, aabb: &Aabb) -> bool {
        aabb.min.y <= self.y_max
            && aabb.max.y >= self.y_min
            && self.footprint.overlaps(&Rect2::from_aabb(aabb))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aabb(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb {
            min: DVec3::from_array(min),
            max: DVec3::from_array(max),
        }
    }

    #[test]
    fn rotated_rect_overlap_uses_true_shape() {
        // 45° 回した細長い長方形は、外接箱なら重なる位置でも実際には離れている
        let thin = Rect2 {
            center: DVec2::ZERO,
            half: DVec2::new(10.0, 0.5),
            yaw: 45f64.to_radians(),
        };
        let corner = Rect2::from_aabb(&aabb([5.0, 0.0, 5.0], [6.0, 1.0, 6.0]));
        assert!(!thin.overlaps(&corner));
        let on_line = Rect2::from_aabb(&aabb([4.0, 0.0, -5.0], [6.0, 1.0, -3.0]));
        assert!(thin.overlaps(&on_line));
    }

    #[test]
    fn distance_between_separated_rects() {
        let a = Rect2::from_aabb(&aabb([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]));
        let b = Rect2::from_aabb(&aabb([4.0, 0.0, 0.0], [5.0, 1.0, 1.0]));
        assert!((a.distance(&b) - 3.0).abs() < 1e-9);
        let c = Rect2::from_aabb(&aabb([0.5, 0.0, 0.5], [2.0, 1.0, 2.0]));
        assert_eq!(a.distance(&c), 0.0);
    }

    #[test]
    fn yaw_turns_x_axis_like_bevy() {
        let r = Rect2 {
            center: DVec2::ZERO,
            half: DVec2::new(2.0, 0.1),
            yaw: 90f64.to_radians(),
        };
        // Y まわり +90° で X 軸は -Z に向く
        let [ax, _] = r.axes();
        assert!((ax - DVec2::new(0.0, -1.0)).length() < 1e-12);
    }

    #[test]
    fn zone_checks_height_range() {
        let zone = Zone {
            footprint: Rect2::from_aabb(&aabb([0.0, 0.0, 0.0], [1.0, 0.0, 1.0])),
            y_min: 0.0,
            y_max: 1.0,
        };
        assert!(zone.intersects(&aabb([0.5, 0.5, 0.5], [2.0, 2.0, 2.0])));
        assert!(!zone.intersects(&aabb([0.5, 1.5, 0.5], [2.0, 2.0, 2.0])));
    }
}
