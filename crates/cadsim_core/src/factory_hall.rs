//! 3D ビューの背景に置く「工場の建屋」の形を決める (ドメイン)
//!
//! コンクリートの床・通路の白線ならぬ黄線・歩行者通路・壁・柱・高窓・メートル単位のグリッドを、
//! モデルの外接箱と設定から計算する。設計データではなく見た目の背景なので、プロジェクトには保存しない。
//!
//! 長さは mm。上方向は +Y。床の座標 (x, z) は `DVec2` の (x, y) に入れる。
//! Bevy に依存しない。表示 (メッシュ・マテリアル) はアプリ側の `view::factory_hall`。

use glam::{DVec2, DVec3};

use crate::math::Aabb;

/// グリッドの太い線と床の目地の間隔 (mm)。自動の広さはこの倍数に揃える
pub const MAJOR_STEP: f64 = 5_000.0;
/// グリッドの細い線の間隔 (mm)
pub const MINOR_STEP: f64 = 1_000.0;
/// 自動の広さで、モデルの外側に空ける幅 (mm)。周りの通路がモデルに重ならないようにする
pub const AUTO_MARGIN: f64 = 6_000.0;
/// 自動の広さの最小 (mm)
pub const AUTO_MIN_SIZE: DVec2 = DVec2::new(30_000.0, 20_000.0);
/// 建屋の一辺の上限 (mm)。遠くに部品が1つあっても床が際限なく広がらないようにする
pub const MAX_SIZE: f64 = 300_000.0;
/// 壁の高さ (mm)
pub const WALL_HEIGHT: f64 = 8_000.0;
/// 腰壁 (壁の下の色を変えた部分) の高さ (mm)
pub const DADO_HEIGHT: f64 = 1_500.0;
/// 高窓の下端と上端 (mm)
pub const WINDOW_BOTTOM: f64 = 5_200.0;
pub const WINDOW_TOP: f64 = 6_800.0;
/// 柱の太さ (正方形の一辺, mm) と、いちばん広い柱の間隔 (mm)
pub const COLUMN_SIZE: f64 = 400.0;
pub const MAX_COLUMN_SPAN: f64 = 10_000.0;
/// 柱の根元に塗る黄色の帯の高さ (mm)
pub const COLUMN_GUARD_HEIGHT: f64 = 1_000.0;

/// 床の線の配置 (壁からの距離, mm)。壁際から 歩行者通路 → 黄線 → フォークリフトの通路 → 黄線
const WALKWAY: (f64, f64) = (600.0, 1_600.0);
const INNER_LINE_WIDTH: f64 = 100.0;
const AISLE_WIDTH: f64 = 3_000.0;

/// 建屋の広さの選び方
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HallSize {
    /// モデルに合わせて広げる
    Auto,
    /// 幅 (X) × 奥行き (Z) を m で決める
    Fixed { width_m: u32, depth_m: u32 },
}

impl HallSize {
    /// メニューに並べる広さ
    pub const PRESETS: [HallSize; 5] = [
        HallSize::Auto,
        HallSize::Fixed {
            width_m: 20,
            depth_m: 15,
        },
        HallSize::Fixed {
            width_m: 40,
            depth_m: 30,
        },
        HallSize::Fixed {
            width_m: 60,
            depth_m: 40,
        },
        HallSize::Fixed {
            width_m: 100,
            depth_m: 60,
        },
    ];

    pub fn label(self) -> String {
        match self {
            HallSize::Auto => "自動 (モデルに合わせる)".into(),
            HallSize::Fixed { width_m, depth_m } => format!("{width_m} m × {depth_m} m"),
        }
    }
}

/// 建屋の背景の設定
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HallSettings {
    /// 床・通路の線・メートルのグリッドを出す
    pub show: bool,
    /// 壁・柱・高窓も出す
    pub walls: bool,
    pub size: HallSize,
}

impl Default for HallSettings {
    fn default() -> Self {
        Self {
            show: true,
            walls: true,
            size: HallSize::Auto,
        }
    }
}

/// 床の上の、軸に平行な長方形 (mm)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloorRect {
    pub min: DVec2,
    pub max: DVec2,
}

impl FloorRect {
    pub fn size(&self) -> DVec2 {
        self.max - self.min
    }

    pub fn center(&self) -> DVec2 {
        (self.min + self.max) * 0.5
    }

    /// 各辺を `d` だけ内側へ寄せる
    pub fn shrink(&self, d: f64) -> FloorRect {
        FloorRect {
            min: self.min + DVec2::splat(d),
            max: self.max - DVec2::splat(d),
        }
    }

    pub fn contains(&self, p: DVec2) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }
}

/// 建屋の壁の向き。床の座標の軸で呼ぶ
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    MinX,
    MaxX,
    MinZ,
    MaxZ,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::MinX, Side::MaxX, Side::MinZ, Side::MaxZ];

    fn index(self) -> usize {
        self as usize
    }

    /// 壁の面から建屋の内側へ向かう向き (床の座標)
    pub fn inward(self) -> DVec2 {
        match self {
            Side::MinX => DVec2::X,
            Side::MaxX => DVec2::NEG_X,
            Side::MinZ => DVec2::Y,
            Side::MaxZ => DVec2::NEG_Y,
        }
    }
}

/// どの壁の側にあるか。柱は角で2つの壁に属する
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sides([bool; 4]);

impl Sides {
    pub fn of(sides: &[Side]) -> Self {
        let mut s = Sides::default();
        for side in sides {
            s.0[side.index()] = true;
        }
        s
    }

    pub fn has(self, side: Side) -> bool {
        self.0[side.index()]
    }

    /// `hidden` のどれかに属していれば隠す
    pub fn any_in(self, hidden: Sides) -> bool {
        self.0.iter().zip(hidden.0).any(|(&a, b)| a && b)
    }
}

/// 床に塗った線・面の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkingKind {
    /// 通路の区切りの黄線
    AisleLine,
    /// 壁際の歩行者通路 (緑)
    Walkway,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marking {
    pub rect: FloorRect,
    pub kind: MarkingKind,
}

/// 壁に付いた長方形の板 (腰壁・上の壁・高窓)。壁の面に沿って `from`〜`to`、高さ `bottom`〜`top` (床から)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WallPanel {
    pub side: Side,
    pub kind: PanelKind,
    /// 壁の面の上の両端 (床の座標)
    pub from: DVec2,
    pub to: DVec2,
    pub bottom: f64,
    pub top: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelKind {
    Dado,
    Upper,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Column {
    /// 中心 (床の座標)
    pub center: DVec2,
    pub sides: Sides,
}

/// グリッドの線の位置と、太い線かどうか
pub type GridLines = Vec<(f64, bool)>;

/// グリッドの線
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HallGrid {
    pub rect: FloorRect,
    pub minor: f64,
    pub major: f64,
}

impl HallGrid {
    /// 線の位置 (X 方向に並ぶ線の x と、Z 方向に並ぶ線の z)。太い線なら true
    pub fn lines(&self) -> (GridLines, GridLines) {
        let along = |lo: f64, hi: f64| {
            let first = (lo / self.minor).ceil() as i64;
            let last = (hi / self.minor).floor() as i64;
            let per_major = (self.major / self.minor).round().max(1.0) as i64;
            (first..=last)
                .map(|i| (i as f64 * self.minor, i.rem_euclid(per_major) == 0))
                .collect::<Vec<_>>()
        };
        (
            along(self.rect.min.x, self.rect.max.x),
            along(self.rect.min.y, self.rect.max.y),
        )
    }
}

/// 建屋の形。これをもとにアプリ側がメッシュを作る
#[derive(Clone, Debug, PartialEq)]
pub struct HallLayout {
    /// 床の広さ (壁の内側)
    pub floor: FloorRect,
    /// 床の高さ (y)
    pub floor_y: f64,
    pub grid: HallGrid,
    pub markings: Vec<Marking>,
    /// 壁を出さないときは空
    pub panels: Vec<WallPanel>,
    pub columns: Vec<Column>,
}

impl HallLayout {
    /// 床の点 → 3D の点 (床の高さに `lift` を足す)
    pub fn to_world(&self, p: DVec2, lift: f64) -> DVec3 {
        DVec3::new(p.x, self.floor_y + lift, p.y)
    }
}

/// モデルの外接箱 `scene` と設定から建屋の形を決める
pub fn layout(scene: Option<Aabb>, settings: &HallSettings) -> HallLayout {
    let floor = floor_rect(scene, settings.size);
    let floor_y = scene.map_or(0.0, |b| b.min.y.min(0.0));
    let markings = markings(floor);
    let (panels, columns) = if settings.walls {
        (wall_panels(floor), columns(floor))
    } else {
        (Vec::new(), Vec::new())
    };
    HallLayout {
        floor,
        floor_y,
        grid: grid(floor),
        markings,
        panels,
        columns,
    }
}

/// 床の広さ。自動なら 5 m の倍数に揃えるので、部品を少し動かしたくらいでは変わらない
fn floor_rect(scene: Option<Aabb>, size: HallSize) -> FloorRect {
    let footprint = scene.map(|b| FloorRect {
        min: DVec2::new(b.min.x, b.min.z),
        max: DVec2::new(b.max.x, b.max.z),
    });
    match size {
        HallSize::Auto => {
            let (mut min, mut max) = match footprint {
                Some(f) => (
                    ((f.min - AUTO_MARGIN) / MAJOR_STEP).floor() * MAJOR_STEP,
                    ((f.max + AUTO_MARGIN) / MAJOR_STEP).ceil() * MAJOR_STEP,
                ),
                None => (-AUTO_MIN_SIZE * 0.5, AUTO_MIN_SIZE * 0.5),
            };
            // 小さすぎれば両側へ 5 m ずつ広げる (広げる量が奇数段なら大きい側へ)
            for axis in 0..2 {
                let short = AUTO_MIN_SIZE[axis] - (max[axis] - min[axis]);
                if short > 0.0 {
                    let steps = (short / MAJOR_STEP).ceil();
                    let low = (steps / 2.0).floor();
                    min[axis] -= low * MAJOR_STEP;
                    max[axis] += (steps - low) * MAJOR_STEP;
                }
            }
            clamp_size(FloorRect { min, max })
        }
        HallSize::Fixed { width_m, depth_m } => {
            let size = DVec2::new(width_m.max(1) as f64, depth_m.max(1) as f64) * 1000.0;
            let center = footprint.map_or(DVec2::ZERO, |f| f.center());
            let min = ((center - size * 0.5) / MINOR_STEP).round() * MINOR_STEP;
            clamp_size(FloorRect {
                min,
                max: min + size,
            })
        }
    }
}

fn clamp_size(rect: FloorRect) -> FloorRect {
    let size = rect.size();
    if size.x <= MAX_SIZE && size.y <= MAX_SIZE {
        return rect;
    }
    let half = size.min(DVec2::splat(MAX_SIZE)) * 0.5;
    let center = (rect.center() / MAJOR_STEP).round() * MAJOR_STEP;
    FloorRect {
        min: center - half,
        max: center + half,
    }
}

/// 広い建屋では細い線を 5 m、太い線を 25 m にして、遠目に線が潰れないようにする
fn grid(floor: FloorRect) -> HallGrid {
    let big = floor.size().max_element() > 120_000.0;
    HallGrid {
        rect: floor,
        minor: if big { MAJOR_STEP } else { MINOR_STEP },
        major: if big { MAJOR_STEP * 5.0 } else { MAJOR_STEP },
    }
}

/// 壁際をぐるりと回る歩行者通路と、フォークリフトの通路の黄線
fn markings(floor: FloorRect) -> Vec<Marking> {
    let mut out = Vec::new();
    let inner_lines = [WALKWAY.1, WALKWAY.1 + INNER_LINE_WIDTH + AISLE_WIDTH];
    // 通路が床の半分を超える狭い建屋では線を引かない
    let deepest = inner_lines[1] + INNER_LINE_WIDTH;
    if floor.size().min_element() < deepest * 2.0 + 2_000.0 {
        return out;
    }
    ring(floor, WALKWAY.0, WALKWAY.1, MarkingKind::Walkway, &mut out);
    for at in inner_lines {
        ring(
            floor,
            at,
            at + INNER_LINE_WIDTH,
            MarkingKind::AisleLine,
            &mut out,
        );
    }
    out
}

/// 壁から `inner`〜`outer` の距離の帯を、重ならない4つの長方形で
fn ring(floor: FloorRect, inner: f64, outer: f64, kind: MarkingKind, out: &mut Vec<Marking>) {
    let a = floor.shrink(inner);
    let b = floor.shrink(outer);
    let rects = [
        // Z の小さい側と大きい側は端から端まで
        (DVec2::new(a.min.x, a.min.y), DVec2::new(a.max.x, b.min.y)),
        (DVec2::new(a.min.x, b.max.y), DVec2::new(a.max.x, a.max.y)),
        // X の両側はその間
        (DVec2::new(a.min.x, b.min.y), DVec2::new(b.min.x, b.max.y)),
        (DVec2::new(b.max.x, b.min.y), DVec2::new(a.max.x, b.max.y)),
    ];
    out.extend(rects.map(|(min, max)| Marking {
        rect: FloorRect { min, max },
        kind,
    }));
}

/// 壁の両端 (床の座標)。内側から見て左 → 右
fn wall_ends(floor: FloorRect, side: Side) -> (DVec2, DVec2) {
    let (lo, hi) = (floor.min, floor.max);
    match side {
        Side::MinZ => (DVec2::new(hi.x, lo.y), DVec2::new(lo.x, lo.y)),
        Side::MaxZ => (DVec2::new(lo.x, hi.y), DVec2::new(hi.x, hi.y)),
        Side::MinX => (DVec2::new(lo.x, lo.y), DVec2::new(lo.x, hi.y)),
        Side::MaxX => (DVec2::new(hi.x, hi.y), DVec2::new(hi.x, lo.y)),
    }
}

/// 壁に沿った柱の位置 (0〜1)。柱の間隔が `MAX_COLUMN_SPAN` を超えないよう等分する
fn column_stations(length: f64) -> Vec<f64> {
    let spans = (length / MAX_COLUMN_SPAN).ceil().max(1.0) as usize;
    (0..=spans).map(|i| i as f64 / spans as f64).collect()
}

fn wall_panels(floor: FloorRect) -> Vec<WallPanel> {
    let mut out = Vec::new();
    for side in Side::ALL {
        let (from, to) = wall_ends(floor, side);
        let panel = |kind, from, to, bottom, top| WallPanel {
            side,
            kind,
            from,
            to,
            bottom,
            top,
        };
        out.push(panel(PanelKind::Dado, from, to, 0.0, DADO_HEIGHT));
        out.push(panel(PanelKind::Upper, from, to, DADO_HEIGHT, WALL_HEIGHT));
        // 柱と柱の間に高窓を1つずつ。柱から少し離す
        let length = from.distance(to);
        let stations = column_stations(length);
        let dir = (to - from) / length;
        let gap = COLUMN_SIZE * 0.5 + 600.0;
        for pair in stations.windows(2) {
            let (a, b) = (pair[0] * length + gap, pair[1] * length - gap);
            if b - a > 1_000.0 {
                out.push(panel(
                    PanelKind::Window,
                    from + dir * a,
                    from + dir * b,
                    WINDOW_BOTTOM,
                    WINDOW_TOP,
                ));
            }
        }
    }
    out
}

fn columns(floor: FloorRect) -> Vec<Column> {
    let half = COLUMN_SIZE * 0.5;
    let mut out: Vec<Column> = Vec::new();
    for side in Side::ALL {
        let (from, to) = wall_ends(floor, side);
        let length = from.distance(to);
        let dir = (to - from) / length;
        for t in column_stations(length) {
            // 柱は壁の内側に接して立てる。角の柱は両方の壁から内側へ
            let along = (t * length).clamp(half, length - half);
            let center = from + dir * along + side.inward() * half;
            if let Some(existing) = out
                .iter_mut()
                .find(|c| c.center.distance(center) < COLUMN_SIZE)
            {
                existing.sides.0[side.index()] = true;
            } else {
                out.push(Column {
                    center,
                    sides: Sides::of(&[side]),
                });
            }
        }
    }
    out
}

/// カメラの位置 `eye` から見て手前にある壁 (カメラが壁の外にいる側)。
/// その壁と柱を隠すと、建屋の外から覗いても中の装置が見える
pub fn hidden_sides(floor: FloorRect, eye: DVec3) -> Sides {
    let p = DVec2::new(eye.x, eye.z);
    let mut s = Sides::default();
    s.0[Side::MinX.index()] = p.x < floor.min.x;
    s.0[Side::MaxX.index()] = p.x > floor.max.x;
    s.0[Side::MinZ.index()] = p.y < floor.min.y;
    s.0[Side::MaxZ.index()] = p.y > floor.max.y;
    s
}

// ---------------------------------------------------------------------------
// コンクリートの床の模様
// ---------------------------------------------------------------------------

/// 床の模様1枚が覆う広さ (mm)。目地 (グリッドの太い線) と同じ 5 m
pub const CONCRETE_TILE: f64 = MAJOR_STEP;

/// コンクリートの模様 (sRGB の RGBA)。`size` × `size` の画像から 1×1 までの縮小 (ミップマップ) を
/// 大きい順に並べて返す。端と端がつながるので、床に何枚並べても継ぎ目が見えない。
/// 遠くで細かい粒がちらつかないよう、縮小は 2×2 の平均で作る。
pub fn concrete_texture(size: u32) -> Vec<Vec<u8>> {
    let size = size.next_power_of_two().max(1);
    let n = size as usize;
    let mut base = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f64 / n as f64, y as f64 / n as f64);
            // 大きなむら + 中くらいのむら + 細かい粒
            let blotch = fbm(u, v, 3, 4, 11);
            let detail = fbm(u, v, 24, 2, 23);
            let grain = hash(x as i64, y as i64, 37) - 0.5;
            let shade = 0.0 + (blotch - 0.5) * 26.0 + (detail - 0.5) * 10.0 + grain * 8.0;
            let i = (y * n + x) * 4;
            let base_rgb = [118.0, 120.0, 121.0];
            for c in 0..3 {
                base[i + c] = (base_rgb[c] + shade).round().clamp(0.0, 255.0) as u8;
            }
            base[i + 3] = 255;
        }
    }
    let mut levels = vec![base];
    let mut n = n;
    while n > 1 {
        let prev = levels.last().expect("1枚目がある");
        let m = n / 2;
        let mut next = vec![0u8; m * m * 4];
        for y in 0..m {
            for x in 0..m {
                for c in 0..4 {
                    let at = |xx: usize, yy: usize| prev[(yy * n + xx) * 4 + c] as u32;
                    let sum = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[(y * m + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        levels.push(next);
        n = m;
    }
    levels
}

/// 0〜1 の擬似乱数 (格子点ごとに決まる)
fn hash(x: i64, y: i64, seed: u64) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 29;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// 周期 `period` の格子で滑らかにつないだ値ノイズ (0〜1)。端がつながる
fn value_noise(u: f64, v: f64, period: i64, seed: u64) -> f64 {
    let (x, y) = (u * period as f64, v * period as f64);
    let (x0, y0) = (x.floor(), y.floor());
    let (fx, fy) = (x - x0, y - y0);
    let smooth = |t: f64| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (smooth(fx), smooth(fy));
    let at = |i: i64, j: i64| hash(i.rem_euclid(period), j.rem_euclid(period), seed);
    let (i, j) = (x0 as i64, y0 as i64);
    let top = at(i, j) * (1.0 - sx) + at(i + 1, j) * sx;
    let bottom = at(i, j + 1) * (1.0 - sx) + at(i + 1, j + 1) * sx;
    top * (1.0 - sy) + bottom * sy
}

/// 周期を倍々にしたノイズを重ねる (0〜1)
fn fbm(u: f64, v: f64, period: i64, octaves: u32, seed: u64) -> f64 {
    let (mut sum, mut weight, mut total) = (0.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += value_noise(u, v, period << o, seed + o as u64) * weight;
        total += weight;
        weight *= 0.5;
    }
    sum / total
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
    fn empty_scene_gets_minimum_hall_around_origin() {
        let l = layout(None, &HallSettings::default());
        assert_eq!(l.floor.size(), AUTO_MIN_SIZE);
        assert_eq!(l.floor.center(), DVec2::ZERO);
        assert_eq!(l.floor_y, 0.0);
    }

    #[test]
    fn auto_hall_surrounds_scene_with_margin_on_major_steps() {
        let scene = aabb([-12_300.0, 0.0, 2_000.0], [31_000.0, 2_500.0, 9_000.0]);
        let f = layout(Some(scene), &HallSettings::default()).floor;
        assert!(f.min.x <= -12_300.0 - AUTO_MARGIN && f.max.x >= 31_000.0 + AUTO_MARGIN);
        assert!(f.min.y <= 2_000.0 - AUTO_MARGIN && f.max.y >= 9_000.0 + AUTO_MARGIN);
        for v in [f.min.x, f.min.y, f.max.x, f.max.y] {
            assert_eq!(v % MAJOR_STEP, 0.0, "{v} は 5 m の倍数");
        }
        assert!(f.size().y >= AUTO_MIN_SIZE.y);
    }

    #[test]
    fn auto_hall_does_not_move_for_small_part_motion() {
        let settings = HallSettings::default();
        let a = layout(Some(aabb([0.0; 3], [8_000.0, 1_000.0, 3_000.0])), &settings);
        let b = layout(
            Some(aabb([150.0, 0.0, 80.0], [8_150.0, 1_000.0, 3_080.0])),
            &settings,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn small_part_sits_inside_minimum_hall() {
        let scene = aabb([-100.0, 0.0, -50.0], [100.0, 300.0, 50.0]);
        let f = layout(Some(scene), &HallSettings::default()).floor;
        assert_eq!(f.size(), AUTO_MIN_SIZE);
        assert!(f.contains(DVec2::ZERO));
    }

    #[test]
    fn fixed_size_is_centered_on_scene() {
        let settings = HallSettings {
            size: HallSize::Fixed {
                width_m: 40,
                depth_m: 30,
            },
            ..HallSettings::default()
        };
        let scene = aabb([10_000.0, 0.0, 0.0], [20_000.0, 1_000.0, 4_000.0]);
        let f = layout(Some(scene), &settings).floor;
        assert_eq!(f.size(), DVec2::new(40_000.0, 30_000.0));
        assert_eq!(f.center(), DVec2::new(15_000.0, 2_000.0));
    }

    #[test]
    fn huge_scene_is_clamped() {
        let scene = aabb([0.0; 3], [2_000_000.0, 100.0, 1_000.0]);
        let f = layout(Some(scene), &HallSettings::default()).floor;
        assert!(f.size().x <= MAX_SIZE);
    }

    #[test]
    fn floor_follows_parts_below_zero() {
        let scene = aabb([0.0, -800.0, 0.0], [1_000.0, 500.0, 1_000.0]);
        assert_eq!(
            layout(Some(scene), &HallSettings::default()).floor_y,
            -800.0
        );
    }

    #[test]
    fn markings_stay_inside_floor_and_do_not_overlap() {
        let l = layout(None, &HallSettings::default());
        assert_eq!(l.markings.len(), 12);
        for m in &l.markings {
            assert!(l.floor.contains(m.rect.min) && l.floor.contains(m.rect.max));
            assert!(m.rect.size().min_element() > 0.0);
        }
        for (i, a) in l.markings.iter().enumerate() {
            for b in &l.markings[i + 1..] {
                let overlap =
                    a.rect.max.cmpgt(b.rect.min).all() && b.rect.max.cmpgt(a.rect.min).all();
                assert!(!overlap, "{a:?} と {b:?} が重なる");
            }
        }
    }

    #[test]
    fn tiny_fixed_hall_skips_markings() {
        let settings = HallSettings {
            size: HallSize::Fixed {
                width_m: 8,
                depth_m: 8,
            },
            ..HallSettings::default()
        };
        assert!(layout(None, &settings).markings.is_empty());
    }

    #[test]
    fn columns_are_inside_walls_and_corner_columns_belong_to_two_walls() {
        let l = layout(None, &HallSettings::default());
        let inner = l.floor.shrink(COLUMN_SIZE * 0.5 - 1e-6);
        for c in &l.columns {
            assert!(inner.contains(c.center), "{c:?}");
        }
        let corners = l
            .columns
            .iter()
            .filter(|c| Side::ALL.iter().filter(|&&s| c.sides.has(s)).count() == 2)
            .count();
        assert_eq!(corners, 4);
        // 30 m の壁は 10 m 間隔で 4 本、20 m の壁は 3 本。角を共有して 4 + 4 + 1 + 1 = 10
        assert_eq!(l.columns.len(), 10);
    }

    #[test]
    fn windows_sit_between_columns_on_every_wall() {
        let l = layout(None, &HallSettings::default());
        for side in Side::ALL {
            let windows = l
                .panels
                .iter()
                .filter(|p| p.side == side && p.kind == PanelKind::Window)
                .count();
            assert!(windows >= 2, "{side:?}: {windows}");
        }
    }

    #[test]
    fn walls_can_be_turned_off() {
        let settings = HallSettings {
            walls: false,
            ..HallSettings::default()
        };
        let l = layout(None, &settings);
        assert!(l.panels.is_empty() && l.columns.is_empty());
        assert!(!l.markings.is_empty());
    }

    #[test]
    fn grid_lines_are_on_meters_with_major_every_five() {
        let l = layout(None, &HallSettings::default());
        let (xs, zs) = l.grid.lines();
        assert_eq!(xs.len(), 31);
        assert_eq!(zs.len(), 21);
        assert!(xs.iter().all(|(x, _)| x % 1_000.0 == 0.0));
        assert!(
            xs.iter()
                .filter(|(_, major)| *major)
                .all(|(x, _)| x % 5_000.0 == 0.0)
        );
        assert_eq!(xs.iter().filter(|(_, major)| *major).count(), 7);
    }

    #[test]
    fn big_hall_uses_coarser_grid() {
        let settings = HallSettings {
            size: HallSize::Fixed {
                width_m: 200,
                depth_m: 100,
            },
            ..HallSettings::default()
        };
        let g = layout(None, &settings).grid;
        assert_eq!((g.minor, g.major), (5_000.0, 25_000.0));
    }

    #[test]
    fn walls_facing_camera_outside_are_hidden() {
        let floor = layout(None, &HallSettings::default()).floor;
        let inside = hidden_sides(floor, DVec3::new(0.0, 2_000.0, 0.0));
        assert_eq!(inside, Sides::default());
        let outside = hidden_sides(floor, DVec3::new(40_000.0, 9_000.0, -30_000.0));
        assert_eq!(outside, Sides::of(&[Side::MaxX, Side::MinZ]));
        let corner = Column {
            center: DVec2::ZERO,
            sides: Sides::of(&[Side::MaxX, Side::MaxZ]),
        };
        assert!(corner.sides.any_in(outside));
        assert!(!corner.sides.any_in(Sides::of(&[Side::MinX])));
    }

    #[test]
    fn concrete_texture_has_full_mip_chain_and_tiles() {
        let levels = concrete_texture(64);
        assert_eq!(levels.len(), 7);
        for (i, level) in levels.iter().enumerate() {
            let n = 64 >> i;
            assert_eq!(level.len(), n * n * 4);
        }
        let base = &levels[0];
        let px = |x: usize, y: usize| base[(y * 64 + x) * 4] as i32;
        // 右端と左端、下端と上端は大きく違わない (模様がつながる)
        for y in 0..64 {
            assert!((px(63, y) - px(0, y)).abs() < 24);
        }
        for x in 0..64 {
            assert!((px(x, 63) - px(x, 0)).abs() < 24);
        }
        // 中間の灰色で、不透明
        let last = levels.last().unwrap();
        assert!((90..150).contains(&last[0]));
        assert_eq!(last[3], 255);
    }
}
