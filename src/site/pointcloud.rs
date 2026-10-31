//! 点群 (ドメイン): 間引き・LOD・設備との干渉と距離
//!
//! 既存の工場を計測した数千万点の点群でも扱えるよう、読み込みながら「ボクセル (小さな立方体)
//! 1つに1点」に間引く。点の数が上限を超えたらボクセルを2倍にして間引き直すので、
//! メモリは上限の点数ぶんしか使わない。
//!
//! 表示の LOD (遠くは粗く): 点をばらばらの順に並べ替えておくと、先頭から何点取っても
//! 全体から均等に選んだことになる。床を区画 (タイル) に分け、カメラから遠い区画ほど
//! 先頭の少しだけを描く。
//!
//! 干渉と距離: 点群を格子で引けるようにしておき、部品の近くの点だけを調べる。
//! 部品の内側にある点が「干渉」、外側でいちばん近い点までが「距離」。

use std::collections::HashMap;

use glam::{DMat4, DVec3, Vec3};

use crate::simulation::{Pose, TriMesh, geometry::Aabb};

/// 間引きの途中の状態 (読み込みながら1点ずつ渡す)
pub struct Decimator {
    /// いまのボクセルの一辺 (モデルの単位)
    pub voxel: f64,
    budget: usize,
    cells: HashMap<[i64; 3], usize>,
    points: Vec<DVec3>,
    colors: Vec<[u8; 3]>,
    has_color: bool,
    /// 渡された点の数 (間引く前)
    pub seen: u64,
}

impl Decimator {
    /// `voxel` は最小の点の間隔、`budget` は残す点の数の上限
    pub fn new(voxel: f64, budget: usize) -> Self {
        Self {
            voxel: voxel.max(1e-9),
            budget: budget.max(1),
            cells: HashMap::new(),
            points: Vec::new(),
            colors: Vec::new(),
            has_color: false,
            seen: 0,
        }
    }

    fn key(&self, p: DVec3) -> [i64; 3] {
        (p / self.voxel).floor().as_i64vec3().to_array()
    }

    pub fn push(&mut self, p: DVec3, color: Option<[u8; 3]>) {
        self.seen += 1;
        if !p.is_finite() {
            return;
        }
        let key = self.key(p);
        if self.cells.contains_key(&key) {
            return;
        }
        self.cells.insert(key, self.points.len());
        self.points.push(p);
        self.has_color |= color.is_some();
        self.colors.push(color.unwrap_or([200, 200, 200]));
        if self.points.len() > self.budget {
            self.coarsen();
        }
    }

    /// ボクセルを2倍にして間引き直す
    fn coarsen(&mut self) {
        while self.points.len() > self.budget * 3 / 4 {
            self.voxel *= 2.0;
            self.cells.clear();
            let mut kept = 0;
            for i in 0..self.points.len() {
                let key = self.key(self.points[i]);
                if self.cells.contains_key(&key) {
                    continue;
                }
                self.cells.insert(key, kept);
                self.points.swap(kept, i);
                self.colors.swap(kept, i);
                kept += 1;
            }
            self.points.truncate(kept);
            self.colors.truncate(kept);
        }
    }

    /// 間引いた点群にする。点はばらばらの順に並べ替え、区画に分ける
    pub fn finish(self) -> Result<PointCloud, String> {
        if self.points.is_empty() {
            return Err("点が1つもありませんでした".into());
        }
        let min = self.points.iter().copied().reduce(DVec3::min).unwrap_or_default();
        let max = self.points.iter().copied().reduce(DVec3::max).unwrap_or_default();
        let center = (min + max) / 2.0;
        let mut order: Vec<usize> = (0..self.points.len()).collect();
        shuffle(&mut order);
        let points: Vec<Vec3> = order
            .iter()
            .map(|&i| (self.points[i] - center).as_vec3())
            .collect();
        let colors = self
            .has_color
            .then(|| order.iter().map(|&i| self.colors[i]).collect());
        let mut cloud = PointCloud {
            points,
            colors,
            source_points: self.seen,
            voxel: self.voxel,
            tiles: Vec::new(),
            grid: CloudGrid::default(),
        };
        cloud.build_index();
        Ok(cloud)
    }
}

/// 決まった種で並べ替える (同じファイルなら毎回同じ表示になるように)
fn shuffle(items: &mut [usize]) {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    for i in (1..items.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state % (i as u64 + 1)) as usize;
        items.swap(i, j);
    }
}

/// 表示の区画。`indices` はばらばらの順のままなので、先頭から何点取っても均等になる
pub struct Tile {
    pub min: Vec3,
    pub max: Vec3,
    pub indices: Vec<u32>,
}

/// 点を格子で引くための索引 (点群の座標系)
#[derive(Default)]
pub struct CloudGrid {
    cell: f32,
    cells: HashMap<[i32; 3], Vec<u32>>,
}

/// 区画1つあたりの点の数の目安
const POINTS_PER_TILE: usize = 60_000;
/// LOD の段数 (1, 1/4, 1/16, 1/64)
pub const LOD_LEVELS: usize = 4;

pub struct PointCloud {
    /// 読み込んだ座標の真ん中から見た位置 (測量座標のように大きな値でも f32 で足りるように)
    pub points: Vec<Vec3>,
    pub colors: Option<Vec<[u8; 3]>>,
    /// 間引く前の点の数
    pub source_points: u64,
    /// 間引きに使ったボクセルの一辺
    pub voxel: f64,
    pub tiles: Vec<Tile>,
    grid: CloudGrid,
}

impl PointCloud {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        self.tiles.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), t| (lo.min(t.min), hi.max(t.max)),
        )
    }

    fn build_index(&mut self) {
        let (min, max) = self
            .points
            .iter()
            .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), p| {
                (lo.min(*p), hi.max(*p))
            });
        let size = (max - min).max(Vec3::splat(1e-6));
        // 床 (X, Z) を区画に分ける
        let tiles_wanted = (self.points.len() / POINTS_PER_TILE).max(1) as f32;
        let aspect = size.x / size.z;
        let nx = (tiles_wanted * aspect).sqrt().ceil().clamp(1.0, 64.0) as usize;
        let nz = (tiles_wanted / nx as f32).ceil().clamp(1.0, 64.0) as usize;
        let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); nx * nz];
        for (i, p) in self.points.iter().enumerate() {
            let tx = (((p.x - min.x) / size.x) * nx as f32).floor().clamp(0.0, nx as f32 - 1.0)
                as usize;
            let tz = (((p.z - min.z) / size.z) * nz as f32).floor().clamp(0.0, nz as f32 - 1.0)
                as usize;
            buckets[tz * nx + tx].push(i as u32);
        }
        self.tiles = buckets
            .into_iter()
            .filter(|b| !b.is_empty())
            .map(|indices| {
                let (lo, hi) = indices.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), &i| {
                        let p = self.points[i as usize];
                        (lo.min(p), hi.max(p))
                    },
                );
                Tile {
                    min: lo,
                    max: hi,
                    indices,
                }
            })
            .collect();

        // 干渉の判定用の格子。1マスに平均 30 点くらい
        let volume = size.x * size.y.max(size.x * 1e-3) * size.z;
        let cell = (volume * 30.0 / self.points.len() as f32).cbrt().max(1e-6);
        let mut cells: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        for (i, p) in self.points.iter().enumerate() {
            cells
                .entry((*p / cell).floor().as_ivec3().to_array())
                .or_default()
                .push(i as u32);
        }
        self.grid = CloudGrid { cell, cells };
    }

    /// 点群の座標系で、箱 `min`〜`max` に入る点
    fn points_in(&self, min: Vec3, max: Vec3, mut f: impl FnMut(u32)) {
        let g = &self.grid;
        if g.cells.is_empty() {
            return;
        }
        let lo = (min / g.cell).floor().as_ivec3();
        let hi = (max / g.cell).floor().as_ivec3();
        let count = (hi - lo + 1).as_i64vec3();
        if count.x * count.y * count.z > g.cells.len() as i64 {
            // 箱が点群より広い: すべてのマスを見る方が速い
            for indices in g.cells.values() {
                indices.iter().copied().for_each(&mut f);
            }
            return;
        }
        for x in lo.x..=hi.x {
            for y in lo.y..=hi.y {
                for z in lo.z..=hi.z {
                    if let Some(indices) = g.cells.get(&[x, y, z]) {
                        indices.iter().copied().for_each(&mut f);
                    }
                }
            }
        }
    }
}

/// 区画をカメラからの距離に応じて何段目の粗さで描くか (0 = すべての点)
pub fn lod_level(tile_min: Vec3, tile_max: Vec3, camera: Vec3, detail: f32) -> usize {
    let size = (tile_max - tile_min).length().max(1e-6);
    let nearest = camera.clamp(tile_min, tile_max);
    let distance = camera.distance(nearest);
    // 区画の大きさの何倍離れているか。4倍ごとに1段粗くする (面積で 1/4 ずつ減らす)
    let ratio = distance / (size * detail.max(0.05));
    if ratio <= 1.0 {
        0
    } else {
        ((ratio.log2() / 1.0).floor() as usize + 1).min(LOD_LEVELS - 1)
    }
}

/// LOD の段の点の割合 (1, 1/4, 1/16, 1/64)
pub fn lod_fraction(level: usize) -> f32 {
    0.25f32.powi(level as i32)
}

/// 床に置いた点群1つ
#[derive(Clone)]
pub struct PlacedCloud {
    pub name: String,
    pub source: std::path::PathBuf,
    pub cloud: std::sync::Arc<PointCloud>,
    /// 点群の真ん中を置くワールドの位置
    pub offset: [f64; 3],
    /// 上から見て反時計回りの回転 (度)
    pub rotation_deg: f64,
    pub visible: bool,
    /// 読み込みの設定 (保存して、開き直すときに同じように読む)
    pub settings: CloudSettings,
}

/// 点群の読み込みの設定
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CloudSettings {
    /// 最小の点の間隔 (モデルの単位)
    pub spacing: f64,
    /// 残す点の数の上限
    pub budget: usize,
    /// 点群の座標の単位 1 が何 mm か (LAS / E57 はメートル)
    pub unit_mm: f64,
    /// Z 軸が上 (LAS / E57 はふつう Z 上)
    pub z_up: bool,
}

impl PlacedCloud {
    /// 点群の真ん中を原点の上、いちばん低い点を床 (Y = 0) に置く
    pub fn new(
        name: String,
        source: std::path::PathBuf,
        cloud: PointCloud,
        settings: CloudSettings,
    ) -> Self {
        let (min, _) = cloud.bounds();
        Self {
            name,
            source,
            offset: [0.0, -(min.y as f64), 0.0],
            rotation_deg: 0.0,
            visible: true,
            cloud: std::sync::Arc::new(cloud),
            settings,
        }
    }

    /// 点群の座標系 (真ん中が原点) → ワールド
    pub fn to_world(&self) -> DMat4 {
        DMat4::from_translation(DVec3::from(self.offset))
            * DMat4::from_rotation_y(self.rotation_deg.to_radians())
    }
}

/// 部品1つと点群の判定結果
#[derive(Clone, Debug, Default)]
pub struct CloudHit {
    pub body: u32,
    /// 部品の内側にある点の数
    pub inside: usize,
    /// 内側の点 (表示用、ワールド、最大 [`MAX_SAMPLES`] 点)
    pub inside_samples: Vec<DVec3>,
    /// 外側でいちばん近い点と、部品の表面の点 (警告距離より近いときだけ)
    pub closest: Option<(DVec3, DVec3)>,
}

impl CloudHit {
    pub fn distance(&self) -> Option<f64> {
        self.closest.map(|(a, b)| a.distance(b))
    }
}

/// 表示に使う干渉点の数の上限
pub const MAX_SAMPLES: usize = 2000;
/// 1つの部品で調べる点の数の上限 (これを超えたら間を飛ばして調べる)
const MAX_CANDIDATES: usize = 400_000;

/// 部品 (メッシュと姿勢) が点群とぶつかっていないか、どこまで近いかを調べる
pub fn check_body(
    placed: &PlacedCloud,
    body: u32,
    mesh: &TriMesh,
    pose: &Pose,
    warning: f64,
) -> Option<CloudHit> {
    let cloud = &placed.cloud;
    let to_world = placed.to_world();
    let to_cloud = to_world.inverse();
    let world_box = mesh.aabb().transformed(pose).expanded(warning.max(0.0));
    // ワールドの箱を点群の座標系に写す (角を写して外接箱をとる)
    let corners = (0..8).map(|i| {
        let p = DVec3::new(
            if i & 1 == 0 { world_box.min.x } else { world_box.max.x },
            if i & 2 == 0 { world_box.min.y } else { world_box.max.y },
            if i & 4 == 0 { world_box.min.z } else { world_box.max.z },
        );
        to_cloud.transform_point3(p).as_vec3()
    });
    let (lo, hi) = corners.fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    );
    let mut candidates = Vec::new();
    cloud.points_in(lo, hi, |i| candidates.push(i));
    if candidates.is_empty() {
        return None;
    }
    let stride = candidates.len().div_ceil(MAX_CANDIDATES);
    let to_body = pose.inverse();
    let mut hit = CloudHit {
        body,
        ..Default::default()
    };
    let mut best = warning;
    for &i in candidates.iter().step_by(stride) {
        let local = cloud.points[i as usize].as_dvec3();
        let world = to_world.transform_point3(local);
        if !aabb_contains(&world_box, world) {
            continue;
        }
        let in_body = to_body.apply(world);
        if mesh.contains(in_body) {
            hit.inside += stride;
            if hit.inside_samples.len() < MAX_SAMPLES {
                hit.inside_samples.push(world);
            }
        } else if let Some((surface, _)) = mesh.closest_point(in_body) {
            let d = surface.distance(in_body);
            if d < best {
                best = d;
                hit.closest = Some((world, pose.apply(surface)));
            }
        }
    }
    (hit.inside > 0 || hit.closest.is_some()).then_some(hit)
}

fn aabb_contains(b: &Aabb, p: DVec3) -> bool {
    p.cmpge(b.min).all() && p.cmple(b.max).all()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_cloud(n: usize, spacing: f64) -> PointCloud {
        // 床 (Y = 0) に n × n の格子の点
        let mut d = Decimator::new(spacing * 0.5, 1_000_000);
        for i in 0..n {
            for k in 0..n {
                d.push(DVec3::new(i as f64 * spacing, 0.0, k as f64 * spacing), None);
            }
        }
        d.finish().unwrap()
    }

    #[test]
    fn decimation_keeps_one_point_per_voxel() {
        let mut d = Decimator::new(10.0, 1_000_000);
        for i in 0..1000 {
            d.push(DVec3::new(i as f64 * 0.1, 0.0, 0.0), Some([1, 2, 3]));
        }
        // 0〜99.9 を 10 ずつのボクセルに分けると 10 点
        let cloud = d.finish().unwrap();
        assert_eq!(cloud.points.len(), 10);
        assert_eq!(cloud.source_points, 1000);
        assert_eq!(cloud.colors.as_ref().unwrap()[0], [1, 2, 3]);
    }

    #[test]
    fn budget_coarsens_voxels() {
        let mut d = Decimator::new(1.0, 1000);
        for i in 0..200 {
            for k in 0..200 {
                d.push(DVec3::new(i as f64, 0.0, k as f64), None);
            }
        }
        let cloud = d.finish().unwrap();
        assert!(cloud.points.len() <= 1000, "{}", cloud.points.len());
        assert!(cloud.points.len() > 200);
        assert!(cloud.voxel >= 8.0);
        // 区画と索引にすべての点が入っている
        let in_tiles: usize = cloud.tiles.iter().map(|t| t.indices.len()).sum();
        assert_eq!(in_tiles, cloud.points.len());
    }

    #[test]
    fn far_tiles_are_coarser() {
        let lo = Vec3::ZERO;
        let hi = Vec3::splat(10.0);
        assert_eq!(lod_level(lo, hi, Vec3::splat(5.0), 1.0), 0);
        assert_eq!(lod_level(lo, hi, Vec3::new(1000.0, 0.0, 0.0), 1.0), LOD_LEVELS - 1);
        let near = lod_level(lo, hi, Vec3::new(40.0, 0.0, 0.0), 1.0);
        let far = lod_level(lo, hi, Vec3::new(80.0, 0.0, 0.0), 1.0);
        assert!(near <= far && far > 0);
        assert_eq!(lod_fraction(2), 1.0 / 16.0);
    }

    fn box_mesh(center: DVec3, size: DVec3) -> TriMesh {
        let h = size / 2.0;
        let v = |x: f64, y: f64, z: f64| center + DVec3::new(x * h.x, y * h.y, z * h.z);
        let quads = [
            [v(-1., -1., 1.), v(1., -1., 1.), v(1., 1., 1.), v(-1., 1., 1.)],
            [v(1., -1., -1.), v(-1., -1., -1.), v(-1., 1., -1.), v(1., 1., -1.)],
            [v(-1., 1., 1.), v(1., 1., 1.), v(1., 1., -1.), v(-1., 1., -1.)],
            [v(-1., -1., -1.), v(1., -1., -1.), v(1., -1., 1.), v(-1., -1., 1.)],
            [v(1., -1., 1.), v(1., -1., -1.), v(1., 1., -1.), v(1., 1., 1.)],
            [v(-1., -1., -1.), v(-1., -1., 1.), v(-1., 1., 1.), v(-1., 1., -1.)],
        ];
        TriMesh::new(
            quads
                .iter()
                .flat_map(|q| [[q[0], q[1], q[2]], [q[0], q[2], q[3]]])
                .collect(),
        )
    }

    #[test]
    fn body_on_the_floor_hits_points() {
        // 1000 × 1000 の床に 10 間隔の点。床に埋まる箱は干渉、浮いた箱は距離
        let cloud = grid_cloud(101, 10.0);
        let placed = PlacedCloud::new(
            "floor.ply".into(),
            "floor.ply".into(),
            cloud,
            CloudSettings {
                spacing: 5.0,
                budget: 1_000_000,
                unit_mm: 1.0,
                z_up: false,
            },
        );
        let mesh = box_mesh(DVec3::ZERO, DVec3::splat(100.0));
        // 箱の中心を床の高さに置く → 下半分が床を突き抜ける
        let sunk = Pose::from_matrix(DMat4::from_translation(DVec3::new(0.0, 0.0, 0.0)));
        let hit = check_body(&placed, 1, &mesh, &sunk, 50.0).unwrap();
        // 100 × 100 の断面に 10 間隔の点 → 9 × 9 くらい
        assert!((60..=121).contains(&hit.inside), "{}", hit.inside);
        // 床から 30 浮かせると干渉はなく、距離は 30
        let floating = Pose::from_matrix(DMat4::from_translation(DVec3::new(3.0, 80.0, 3.0)));
        let hit = check_body(&placed, 1, &mesh, &floating, 50.0).unwrap();
        assert_eq!(hit.inside, 0);
        assert!((hit.distance().unwrap() - 30.0).abs() < 1e-3, "{:?}", hit.distance());
        // 警告距離より遠ければ何も返さない
        let far = Pose::from_matrix(DMat4::from_translation(DVec3::new(0.0, 500.0, 0.0)));
        assert!(check_body(&placed, 1, &mesh, &far, 50.0).is_none());
    }
}
