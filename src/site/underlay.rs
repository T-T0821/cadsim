//! 平面図の下敷き (ドメイン)
//!
//! DXF / PDF / 画像から読んだ平面図を、床 (Y = 高さ) の上に敷く。
//! 図面の座標 (u, v) は「上面ビューで右が +u、上が +v」の2次元座標で、
//! 縮尺・回転・原点の位置を掛けてワールド座標の (X, Z) に置く。
//! 上面ビューは画面の上が -Z なので、v はワールドの -Z 方向になる。
//!
//! 縮尺合わせは「下敷きの上の2点と、その実際の距離」から縮尺を決め直す。
//! Bevy にもファイル形式にも依存しない。

use std::path::PathBuf;
use std::sync::Arc;

use glam::{DVec2, DVec3};

/// 読み込んだ平面図の中身 (図面の座標系)
#[derive(Default)]
pub struct PlanContent {
    /// 線 (折れ線)。円や円弧は折れ線に直してある。
    pub polylines: Vec<Vec<DVec2>>,
    /// 画像 (スキャンした図面や写真)。
    pub image: Option<PlanImage>,
    /// 図面の座標の単位 1 が何 mm か (分からなければ None)
    pub unit_mm: Option<f64>,
}

/// 下敷きの画像。`min`〜`max` の長方形に貼る (図面の座標)。
pub struct PlanImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8、上の行から順
    pub rgba: Vec<u8>,
    pub min: DVec2,
    pub max: DVec2,
}

impl PlanContent {
    /// 図面の座標での範囲 (min, max)。何もなければ None。
    pub fn bounds(&self) -> Option<(DVec2, DVec2)> {
        let points = self.polylines.iter().flatten().copied().chain(
            self.image
                .iter()
                .flat_map(|image| [image.min, image.max]),
        );
        points.fold(None, |acc, p| match acc {
            None => Some((p, p)),
            Some((min, max)) => Some((min.min(p), max.max(p))),
        })
    }

    pub fn segment_count(&self) -> usize {
        self.polylines
            .iter()
            .map(|l| l.len().saturating_sub(1))
            .sum()
    }
}

/// 床に敷いた平面図1枚
#[derive(Clone)]
pub struct Underlay {
    /// 表示名 (ファイル名)
    pub name: String,
    pub source: PathBuf,
    pub content: Arc<PlanContent>,
    /// 図面の座標 1 がモデルの単位でいくつか
    pub scale: f64,
    /// 図面の原点 (0, 0) を置くワールドの (X, Z)
    pub origin: [f64; 2],
    /// 上から見て反時計回りの回転 (度)
    pub rotation_deg: f64,
    /// 敷く高さ (ワールドの Y)
    pub elevation: f64,
    /// 0 (透明) 〜 1 (不透明)
    pub opacity: f32,
    pub visible: bool,
}

impl Underlay {
    /// 読み込んだ図面を、モデルの単位に合わせて原点に敷く
    pub fn new(name: String, source: PathBuf, content: PlanContent, model_unit_mm: f64) -> Self {
        // 単位の分からない図面 (画像・PDF) は仮に 1 = 1 mm とし、縮尺合わせで直してもらう
        let unit_mm = content.unit_mm.unwrap_or(1.0);
        Self {
            name,
            source,
            content: Arc::new(content),
            scale: unit_mm / model_unit_mm,
            origin: [0.0, 0.0],
            rotation_deg: 0.0,
            elevation: 0.0,
            opacity: 0.8,
            visible: true,
        }
    }

    /// 図面の座標 → ワールド座標
    pub fn to_world(&self, p: DVec2) -> DVec3 {
        let r = DVec2::from_angle(self.rotation_deg.to_radians()).rotate(p * self.scale);
        DVec3::new(
            self.origin[0] + r.x,
            self.elevation,
            self.origin[1] - r.y,
        )
    }

    /// ワールド座標 (高さは無視) → 図面の座標
    pub fn to_plan(&self, world: DVec3) -> DVec2 {
        let r = DVec2::new(world.x - self.origin[0], self.origin[1] - world.z);
        DVec2::from_angle(-self.rotation_deg.to_radians()).rotate(r) / self.scale
    }

    /// ワールドでの範囲 (X, Z の min / max)
    pub fn world_bounds(&self) -> Option<(DVec3, DVec3)> {
        let (min, max) = self.content.bounds()?;
        let corners = [
            min,
            DVec2::new(max.x, min.y),
            max,
            DVec2::new(min.x, max.y),
        ]
        .map(|c| self.to_world(c));
        let lo = corners.iter().copied().reduce(DVec3::min)?;
        let hi = corners.iter().copied().reduce(DVec3::max)?;
        Some((lo, hi))
    }

    /// ワールドの点 `world` から `reach` 以内にある、いちばん近い線の頂点 (ワールド)。
    /// 縮尺合わせで図面の角を正確に拾うために使う。
    pub fn nearest_vertex(&self, world: DVec3, reach: f64) -> Option<DVec3> {
        let target = self.to_plan(world);
        let reach_plan = reach / self.scale;
        let mut best = reach_plan * reach_plan;
        let mut found = None;
        for &p in self.content.polylines.iter().flatten() {
            let d = p.distance_squared(target);
            if d <= best {
                best = d;
                found = Some(p);
            }
        }
        found.map(|p| self.to_world(p))
    }

    /// 縮尺合わせ。今の表示で `a`・`b` (ワールド) にある2点の実際の距離が `distance` になるよう
    /// 縮尺を変える。`a` の点はその場に残す。距離が測れないときは false。
    pub fn calibrate(&mut self, a: DVec3, b: DVec3, distance: f64) -> bool {
        let shown = DVec2::new(b.x - a.x, b.z - a.z).length();
        if shown <= f64::EPSILON || distance <= 0.0 || !distance.is_finite() {
            return false;
        }
        let anchor = self.to_plan(a);
        self.scale *= distance / shown;
        self.place(anchor, a);
        true
    }

    /// 図面の点 `plan` がワールドの `world` (X, Z) に来るよう原点をずらす
    pub fn place(&mut self, plan: DVec2, world: DVec3) {
        let now = self.to_world(plan);
        self.origin[0] += world.x - now.x;
        self.origin[1] += world.z - now.z;
    }

    /// 図面上の `a` → `b` の向きがワールドの +X を向くよう、`a` を中心に回す
    pub fn align_to_x(&mut self, a: DVec3, b: DVec3) {
        let dir = DVec2::new(b.x - a.x, a.z - b.z);
        if dir.length_squared() <= f64::EPSILON {
            return;
        }
        let anchor = self.to_plan(a);
        self.rotation_deg = (self.rotation_deg - dir.to_angle().to_degrees()).rem_euclid(360.0);
        self.place(anchor, a);
    }
}

/// モデルの単位 ("mm" など) の 1 が何 mm か。知らない単位は mm とみなす。
pub fn unit_in_mm(unit: &str) -> f64 {
    match unit.to_ascii_lowercase().as_str() {
        "cm" => 10.0,
        "m" => 1000.0,
        "inch" | "in" => 25.4,
        "ft" | "feet" => 304.8,
        "um" | "µm" => 0.001,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Underlay {
        let content = PlanContent {
            polylines: vec![vec![
                DVec2::ZERO,
                DVec2::new(10.0, 0.0),
                DVec2::new(10.0, 10.0),
                DVec2::new(0.0, 10.0),
                DVec2::ZERO,
            ]],
            image: None,
            unit_mm: Some(1000.0),
        };
        Underlay::new("plan.dxf".into(), "plan.dxf".into(), content, 1.0)
    }

    #[test]
    fn plan_units_become_model_units() {
        // m の図面を mm のモデルに敷くと 1000 倍
        let u = square();
        assert_eq!(u.scale, 1000.0);
        let p = u.to_world(DVec2::new(10.0, 10.0));
        // 図面の上 (+v) はワールドの -Z
        assert!((p - DVec3::new(10000.0, 0.0, -10000.0)).length() < 1e-9, "{p}");
        assert!((u.to_plan(p) - DVec2::new(10.0, 10.0)).length() < 1e-9);
    }

    #[test]
    fn calibrate_keeps_first_point() {
        let mut u = square();
        u.origin = [500.0, 200.0];
        u.rotation_deg = 30.0;
        let a = u.to_world(DVec2::new(0.0, 10.0));
        let b = u.to_world(DVec2::new(10.0, 10.0));
        // 図面の 10 が実際は 4000 だった
        assert!(u.calibrate(a, b, 4000.0));
        assert!((u.scale - 400.0).abs() < 1e-9);
        assert!((u.to_world(DVec2::new(0.0, 10.0)) - a).length() < 1e-9);
        let b2 = u.to_world(DVec2::new(10.0, 10.0));
        assert!(((b2 - a).length() - 4000.0).abs() < 1e-6);
        assert!(!u.calibrate(a, a, 100.0));
    }

    #[test]
    fn align_rotates_line_onto_x_axis() {
        let mut u = square();
        u.rotation_deg = 25.0;
        let a = u.to_world(DVec2::ZERO);
        let b = u.to_world(DVec2::new(0.0, 10.0));
        u.align_to_x(a, b);
        let a2 = u.to_world(DVec2::ZERO);
        let b2 = u.to_world(DVec2::new(0.0, 10.0));
        assert!((a2 - a).length() < 1e-9);
        assert!((b2.z - a2.z).abs() < 1e-6 && b2.x > a2.x, "{a2} {b2}");
    }

    #[test]
    fn snaps_to_nearby_vertex() {
        let u = square();
        let corner = u.to_world(DVec2::new(10.0, 0.0));
        let near = corner + DVec3::new(30.0, 0.0, -20.0);
        assert_eq!(u.nearest_vertex(near, 100.0), Some(corner));
        assert_eq!(u.nearest_vertex(near, 10.0), None);
    }

    #[test]
    fn bounds_cover_lines_and_image() {
        let mut content = PlanContent::default();
        content.polylines.push(vec![DVec2::new(-1.0, 2.0), DVec2::new(3.0, 4.0)]);
        content.image = Some(PlanImage {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
            min: DVec2::new(0.0, 0.0),
            max: DVec2::new(5.0, 1.0),
        });
        assert_eq!(
            content.bounds(),
            Some((DVec2::new(-1.0, 0.0), DVec2::new(5.0, 4.0)))
        );
        assert_eq!(content.segment_count(), 1);
    }
}
