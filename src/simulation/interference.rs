//! 干渉チェック本体: 部品の組ごとに「干渉 / 接触 / 近接 / 問題なし」を判定する
//!
//! 判定は3段階で、軽いものから順に行う。
//!
//! 1. 外接箱 (AABB) で遠い組を捨てる
//! 2. BVH で近い三角形どうしだけを調べ、相手を貫く交線があれば「干渉」
//! 3. 貫く交線がなければ、内側に入り込んでいないか (内包・面一の重なり) を調べ、
//!    それでもなければ最短距離を測って「接触」「近接」に分ける
//!
//! 干渉している部分の立体 (体積と表示用メッシュ) は重いので、[`super::overlap`] で
//! 必要なときだけ作る。

use std::cell::Cell;

use bevy::math::DVec3;

use super::geometry::{
    Aabb, Pose, Triangle, TriangleContact, closest_points_on_triangles, intersect_triangles,
    triangle_centroid, triangle_normal,
};
use super::mesh::{TriMesh, for_each_triangle_pair};
use super::overlap::{OverlapRegion, overlap_region};

/// 内包チェックで調べる点の数の上限 (部品ごと)
const CONTAINMENT_SAMPLES: usize = 64;

/// 判定の設定値。単位はモデルの長さ単位。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CheckSettings {
    /// これ以下の距離は「触れている」とみなす
    pub tolerance: f64,
    /// クリアランスがこれ未満の組を「近接」として警告する
    pub clearance_warning: f64,
    /// 干渉部分の立体 (体積・表示用メッシュ) も計算するか
    pub compute_overlap: bool,
}

impl Default for CheckSettings {
    fn default() -> Self {
        Self {
            tolerance: 1e-4,
            clearance_warning: 0.1,
            compute_overlap: true,
        }
    }
}

/// 判定対象の部品1つ
pub struct Body<'a> {
    pub id: u32,
    pub mesh: &'a TriMesh,
    /// ローカル → ワールド
    pub pose: Pose,
}

impl Body<'_> {
    fn world_aabb(&self) -> Aabb {
        self.mesh.aabb().transformed(&self.pose)
    }

    /// ワールド座標の点がこの部品の内側か
    pub fn contains(&self, world: DVec3) -> bool {
        self.mesh.contains(self.pose.inverse().apply(world))
    }

    /// ワールド座標の点に最も近い表面上の点と、そこの法線 (どちらもワールド座標)
    pub fn closest_point(&self, world: DVec3) -> Option<(DVec3, DVec3)> {
        let (q, n) = self.mesh.closest_point(self.pose.inverse().apply(world))?;
        Some((self.pose.apply(q), self.pose.rotation * n))
    }
}

/// 2部品の関係
#[derive(Clone, Debug, PartialEq)]
pub enum PairStatus {
    /// 食い込んでいる。`overlap` は計算した場合のみ。
    Interfering { overlap: Option<OverlapRegion> },
    /// 面や辺が触れている (距離が許容差以下)
    Contact,
    /// 離れているが、クリアランスが警告値未満
    Near { clearance: f64 },
}

/// 2部品の判定結果 (座標はすべてワールド)
#[derive(Clone, Debug, PartialEq)]
pub struct PairResult {
    pub a: u32,
    pub b: u32,
    pub status: PairStatus,
    /// 表面どうしの交線 (干渉時の輪郭線として描く)
    pub curves: Vec<[DVec3; 2]>,
    /// 最も近い2点 (接触・近接時)
    pub closest: Option<[DVec3; 2]>,
}

/// すべての組を調べ、問題のある組 (干渉・接触・近接) だけを返す。
///
/// 全部の組 (n² / 2) を調べると数千部品では重いので、外接箱を X 方向に並べて
/// 重なりうる組だけを拾う (sweep and prune)。結果の並びは全部の組を順に調べたときと同じ。
pub fn check_all(bodies: &[Body], settings: &CheckSettings) -> Vec<PairResult> {
    let reach = settings.clearance_warning.max(settings.tolerance);
    let aabbs: Vec<Aabb> = bodies.iter().map(Body::world_aabb).collect();
    let mut order: Vec<usize> = (0..bodies.len()).collect();
    order.sort_by(|&a, &b| aabbs[a].min.x.total_cmp(&aabbs[b].min.x));

    let mut results = Vec::new();
    for (k, &i) in order.iter().enumerate() {
        for &j in &order[k + 1..] {
            // X 方向で離れていれば、それより右の箱はもっと離れている
            if aabbs[j].min.x - aabbs[i].max.x >= reach {
                break;
            }
            if aabbs[i].gap(&aabbs[j]) >= reach {
                continue;
            }
            let (a, b) = (i.min(j), i.max(j));
            if let Some(result) = check_pair(&bodies[a], &bodies[b], settings) {
                results.push(((a, b), result));
            }
        }
    }
    results.sort_by_key(|(pair, _)| *pair);
    results.into_iter().map(|(_, result)| result).collect()
}

/// 2部品を調べる。問題がなければ None。
pub fn check_pair(a: &Body, b: &Body, settings: &CheckSettings) -> Option<PairResult> {
    let tol = settings.tolerance;
    let reach = settings.clearance_warning.max(tol);

    // 1. 外接箱で遠い組を捨てる
    if a.world_aabb().gap(&b.world_aabb()) >= reach {
        return None;
    }

    // 2. 交線を集める (計算は a のローカル座標で行い、最後にワールドへ戻す)
    let b_to_a = a.pose.inverse().compose(&b.pose);
    let mut curves = Vec::new();
    let mut penetrating = false;
    for_each_triangle_pair(
        a.mesh,
        b.mesh,
        &b_to_a,
        |box_a, box_b| box_a.expanded(tol).overlaps(box_b),
        |ta, tb| {
            if let TriangleContact::Crossing { segment, proper } = intersect_triangles(ta, tb, tol)
            {
                penetrating |= proper;
                if proper {
                    curves.push(segment.map(|p| a.pose.apply(p)));
                }
            }
        },
    );

    // 3. 貫いていなくても、まるごと内側にある・面一で重なっている場合がある
    if penetrating || is_inside_of(a, b, tol) || is_inside_of(b, a, tol) {
        let overlap = settings.compute_overlap.then(|| overlap_region(a, b, tol));
        return Some(PairResult {
            a: a.id,
            b: b.id,
            status: PairStatus::Interfering { overlap },
            curves,
            closest: None,
        });
    }

    // 4. 最短距離で接触・近接を分ける
    let [pa, pb] = closest_points(a, b, &b_to_a, reach)?;
    let clearance = pa.distance(pb);
    let status = if clearance <= tol {
        PairStatus::Contact
    } else if clearance < settings.clearance_warning {
        PairStatus::Near { clearance }
    } else {
        return None;
    };
    Some(PairResult {
        a: a.id,
        b: b.id,
        status,
        curves: Vec::new(),
        closest: Some([a.pose.apply(pa), a.pose.apply(pb)]),
    })
}

/// `inner` の表面のすぐ内側の点が `outer` の内側に入っているか。
///
/// 表面から許容差の2倍だけ内側へずらした点を使うので、面が接しているだけなら
/// 「外」、完全に入り込んでいたり面一で重なっていたりすれば「内」になる。
fn is_inside_of(inner: &Body, outer: &Body, tol: f64) -> bool {
    let Some(region) = inner
        .world_aabb()
        .intersection(&outer.world_aabb().expanded(tol))
    else {
        return false;
    };
    let candidates: Vec<&Triangle> = inner
        .mesh
        .triangles()
        .iter()
        .filter(|t| region.overlaps(&Aabb::from_points(t.map(|p| inner.pose.apply(p)))))
        .collect();
    let stride = candidates.len().div_ceil(CONTAINMENT_SAMPLES).max(1);
    candidates.iter().step_by(stride).any(|t| {
        let inward = -triangle_normal(t) * (2.0 * tol);
        let sample = inner.pose.apply(triangle_centroid(t) + inward);
        outer.contains(sample)
            && outer
                .closest_point(sample)
                .is_some_and(|(q, _)| q.distance(sample) > tol)
    })
}

/// 2部品の最近点の組 (a のローカル座標)。`reach` より離れていれば None。
fn closest_points(a: &Body, b: &Body, b_to_a: &Pose, reach: f64) -> Option<[DVec3; 2]> {
    let mut best: Option<[DVec3; 2]> = None;
    // 2つのクロージャから読み書きするので Cell に入れる
    let best_d = Cell::new(reach);
    for_each_triangle_pair(
        a.mesh,
        b.mesh,
        b_to_a,
        // 今の最良値より遠い箱の組は調べなくてよい
        |box_a, box_b| box_a.gap(box_b) < best_d.get(),
        |ta, tb| {
            let (pa, pb) = closest_points_on_triangles(ta, tb);
            let d = pa.distance(pb);
            if d < best_d.get() {
                best_d.set(d);
                best = Some([pa, pb]);
            }
        },
    );
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simulation::mesh::tests::box_mesh;
    use bevy::math::DQuat;

    /// 組を絞り込んだ結果が、全部の組を順に調べた結果と同じ
    #[test]
    fn check_all_matches_all_pairs() {
        let meshes: Vec<TriMesh> = (0..40)
            .map(|i| {
                let (x, z) = ((i % 8) as f64 * 1.5, (i / 8) as f64 * 1.5);
                // 少し大きめにして、隣どうしが重なったり接したりするようにする
                box_mesh([x, 0.0, z], [1.4 + (i % 3) as f64 * 0.1, 1.0, 1.0])
            })
            .collect();
        let bodies: Vec<Body> = meshes
            .iter()
            .enumerate()
            .map(|(i, mesh)| Body {
                id: i as u32,
                mesh,
                pose: Pose::IDENTITY,
            })
            .collect();
        let settings = CheckSettings::default();
        let mut brute = Vec::new();
        for (i, a) in bodies.iter().enumerate() {
            for b in &bodies[i + 1..] {
                brute.extend(check_pair(a, b, &settings).map(|r| (r.a, r.b)));
            }
        }
        let fast: Vec<(u32, u32)> = check_all(&bodies, &settings)
            .iter()
            .map(|r| (r.a, r.b))
            .collect();
        assert!(!brute.is_empty());
        assert_eq!(fast, brute);
    }

    fn status(a: &TriMesh, b: &TriMesh) -> Option<PairStatus> {
        let body = |id, mesh| Body {
            id,
            mesh,
            pose: Pose::IDENTITY,
        };
        check_pair(&body(1, a), &body(2, b), &CheckSettings::default()).map(|r| r.status)
    }

    #[test]
    fn overlapping_boxes_interfere_with_volume() {
        let a = box_mesh([0.0; 3], [2.0, 2.0, 2.0]);
        let b = box_mesh([1.5, 0.5, 0.0], [2.0, 2.0, 2.0]);
        let Some(PairStatus::Interfering {
            overlap: Some(overlap),
        }) = status(&a, &b)
        else {
            panic!("expected interference");
        };
        // 重なりは 0.5 x 1.5 x 2 = 1.5
        assert!((overlap.volume - 1.5).abs() < 0.02, "{}", overlap.volume);
    }

    #[test]
    fn boxes_on_same_floor_interfere() {
        // 底面が同じ平面にある (CAD でよくある配置)
        let a = box_mesh([0.0, 0.5, 0.0], [2.0, 1.0, 1.0]);
        let b = box_mesh([1.0, 0.5, 0.0], [1.0, 1.0, 1.0]);
        let Some(PairStatus::Interfering {
            overlap: Some(overlap),
        }) = status(&a, &b)
        else {
            panic!("expected interference");
        };
        assert!((overlap.volume - 0.5).abs() < 0.02, "{}", overlap.volume);
    }

    #[test]
    fn identical_boxes_interfere() {
        let a = box_mesh([0.0; 3], [1.0, 1.0, 1.0]);
        let b = box_mesh([0.0; 3], [1.0, 1.0, 1.0]);
        assert!(matches!(
            status(&a, &b),
            Some(PairStatus::Interfering { .. })
        ));
    }

    #[test]
    fn contained_box_interferes() {
        let a = box_mesh([0.0; 3], [4.0, 4.0, 4.0]);
        let b = box_mesh([0.3, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let Some(PairStatus::Interfering {
            overlap: Some(overlap),
        }) = status(&a, &b)
        else {
            panic!("expected interference");
        };
        assert!((overlap.volume - 1.0).abs() < 1e-6, "{}", overlap.volume);
    }

    #[test]
    fn stacked_boxes_are_in_contact() {
        let a = box_mesh([0.0, 0.5, 0.0], [1.0, 1.0, 1.0]);
        let b = box_mesh([0.2, 1.5, 0.1], [1.0, 1.0, 1.0]);
        assert_eq!(status(&a, &b), Some(PairStatus::Contact));
    }

    #[test]
    fn near_boxes_report_clearance() {
        let a = box_mesh([0.0; 3], [1.0, 1.0, 1.0]);
        let b = box_mesh([1.05, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let Some(PairStatus::Near { clearance }) = status(&a, &b) else {
            panic!("expected near");
        };
        assert!((clearance - 0.05).abs() < 1e-9);
        let far = box_mesh([3.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert_eq!(status(&a, &far), None);
    }

    #[test]
    fn pose_moves_body() {
        // 同じメッシュでも姿勢 (タイミングチャートでの移動) だけで結果が変わる
        let mesh = box_mesh([0.0; 3], [1.0, 1.0, 1.0]);
        let a = Body {
            id: 1,
            mesh: &mesh,
            pose: Pose::IDENTITY,
        };
        let settings = CheckSettings::default();
        let moved = |x: f64| Body {
            id: 2,
            mesh: &mesh,
            pose: Pose {
                rotation: DQuat::from_rotation_y(0.3),
                translation: DVec3::new(x, 0.0, 0.0),
            },
        };
        assert!(check_pair(&a, &moved(5.0), &settings).is_none());
        assert!(
            check_pair(&a, &moved(0.8), &settings)
                .is_some_and(|r| matches!(r.status, PairStatus::Interfering { .. }))
        );
    }
}
