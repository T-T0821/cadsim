//! 境界ボリューム階層 (BVH): 外接箱の木で、レイが当たりうるものだけを調べる
//!
//! クリックでの選択に使う。部品の数千個の外接箱 (上の階層) と、部品1つの数万個の三角形
//! (下の階層) のどちらにも同じ木を使う。全件を順に調べる代わりに、レイが通らない枝を
//! 丸ごと飛ばすので、部品や三角形が増えてもほぼ log(n) で済む。
//!
//! 木の構築と、近い枝から順に開くたどり方は parry の [`ParryBvh`] に任せる。
//! 外から見える型 ([`Bounds`]・[`Bvh::cast`]) は f32 のまま。

use std::cell::RefCell;

use glam::Vec3;
use parry3d_f64::bounding_volume::Aabb;
use parry3d_f64::partitioning::{Bvh as ParryBvh, BvhBuildStrategy};
use parry3d_f64::query::Ray;

/// 軸に平行な箱 (f32)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds {
        min: Vec3::splat(f32::MAX),
        max: Vec3::splat(f32::MIN),
    };

    pub fn from_points(points: impl IntoIterator<Item = Vec3>) -> Bounds {
        points.into_iter().fold(Bounds::EMPTY, |b, p| Bounds {
            min: b.min.min(p),
            max: b.max.max(p),
        })
    }

    pub fn union(self, other: Bounds) -> Bounds {
        Bounds {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    /// レイ `origin + t * dir` が箱に入る t (0 以上 `max_t` 未満)。当たらなければ None。
    #[cfg(test)]
    fn ray_entry(&self, origin: Vec3, inv_dir: Vec3, max_t: f32) -> Option<f32> {
        let t1 = (self.min - origin) * inv_dir;
        let t2 = (self.max - origin) * inv_dir;
        let near = t1.min(t2).max_element().max(0.0);
        let far = t1.max(t2).min_element().min(max_t);
        // NaN (箱の面上を平行に通るレイ) は当たらない扱いになる
        (near <= far).then_some(near)
    }
}

#[derive(Clone, Default)]
pub struct Bvh {
    /// 葉の番号は `build` に渡した要素の添字
    tree: ParryBvh,
}

impl std::fmt::Debug for Bvh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bvh").finish_non_exhaustive()
    }
}

impl Bvh {
    /// 要素の外接箱から木を作る
    pub fn build(items: &[Bounds]) -> Bvh {
        let leaves: Vec<Aabb> = items
            .iter()
            .map(|b| Aabb::new(b.min.as_dvec3(), b.max.as_dvec3()))
            .collect();
        Bvh {
            tree: ParryBvh::from_leaves(BvhBuildStrategy::Binned, &leaves),
        }
    }

    /// レイにいちばん近く当たる要素とその t を探す。
    ///
    /// `hit(要素, いまの最短 t)` は要素そのもの (三角形や部品) との当たりを調べ、
    /// いまの最短より近ければその t を返す。外接箱が当たっても中身が外れることはあるので、
    /// 近い枝から順に調べ、それまでの最短より遠い枝は開かない。
    pub fn cast(
        &self,
        origin: Vec3,
        dir: Vec3,
        hit: impl FnMut(u32, f32) -> Option<f32>,
    ) -> Option<(u32, f32)> {
        let ray = Ray::new(origin.as_dvec3(), dir.as_dvec3());
        // parry は Fn を受け取るので、呼び出し側の FnMut は RefCell に入れて渡す
        let hit = RefCell::new(hit);
        let (item, t) = self.tree.cast_ray(&ray, f64::MAX, |item, best| {
            let best = if best >= f64::from(f32::MAX) {
                f32::INFINITY
            } else {
                best as f32
            };
            (hit.borrow_mut())(item, best)
                .filter(|&t| t < best)
                .map(f64::from)
        })?;
        Some((item, t as f32))
    }
}

/// レイと三角形の交点の t (Möller–Trumbore)。裏からでも当たりとする。
pub fn ray_triangle(origin: Vec3, dir: Vec3, [a, b, c]: [Vec3; 3]) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < f32::EPSILON * e1.length_squared().max(e2.length_squared()) {
        return None; // レイが三角形と平行か、三角形がつぶれている
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_box_at(center: Vec3) -> Bounds {
        Bounds {
            min: center - Vec3::splat(0.5),
            max: center + Vec3::splat(0.5),
        }
    }

    /// 格子に並べた箱をすべて順に調べた結果と、BVH の結果が一致する
    #[test]
    fn cast_matches_brute_force() {
        let boxes: Vec<Bounds> = (0..1000)
            .map(|i| {
                let (x, y, z) = (i % 10, (i / 10) % 10, i / 100);
                unit_box_at(Vec3::new(x as f32, y as f32, z as f32) * 2.0)
            })
            .collect();
        let bvh = Bvh::build(&boxes);
        let inv = |d: Vec3| d.recip();
        for (origin, dir) in [
            (Vec3::new(-5.0, 0.2, 0.1), Vec3::X),
            (Vec3::new(30.0, 30.0, 30.0), Vec3::new(-1.0, -1.0, -1.0)),
            (Vec3::new(4.1, 50.0, 6.2), Vec3::NEG_Y),
            (Vec3::new(-5.0, -5.0, -5.0), Vec3::new(1.0, 0.0, 0.0)), // 外れる
        ] {
            let mut tested = 0;
            let hit = bvh.cast(origin, dir, |i, max_t| {
                tested += 1;
                boxes[i as usize].ray_entry(origin, inv(dir), max_t)
            });
            let brute = boxes
                .iter()
                .enumerate()
                .filter_map(|(i, b)| Some((i as u32, b.ray_entry(origin, inv(dir), f32::MAX)?)))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            assert_eq!(hit, brute, "{origin} {dir}");
            assert!(tested < 100, "調べた数が多すぎる: {tested}");
        }
    }

    #[test]
    fn empty_tree_hits_nothing() {
        assert_eq!(
            Bvh::build(&[]).cast(Vec3::ZERO, Vec3::X, |_, _| Some(0.0)),
            None
        );
    }

    #[test]
    fn triangle_hit_and_miss() {
        let tri = [Vec3::ZERO, Vec3::X, Vec3::Y];
        let t = ray_triangle(Vec3::new(0.2, 0.2, 5.0), Vec3::NEG_Z, tri);
        assert_eq!(t, Some(5.0));
        assert_eq!(
            ray_triangle(Vec3::new(0.8, 0.8, 5.0), Vec3::NEG_Z, tri),
            None
        );
        // 後ろ向きのレイは当たらない
        assert_eq!(ray_triangle(Vec3::new(0.2, 0.2, 5.0), Vec3::Z, tri), None);
    }
}
