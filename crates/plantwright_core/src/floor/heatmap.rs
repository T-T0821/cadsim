//! 人と AGV の動線のヒートマップ
//!
//! 床をマス目に区切り、作業者と AGV がそれぞれのマスを何回通ったか (入った回数) と、
//! 何秒いたかを数える。混み合う場所 (多く通る・長く止まる) や、歩く距離の長さを見つけるために使う。
//!
//! 位置はフレームごとに渡される。倍速で 1フレームに何マスも進んだときも、
//! 前の位置から今の位置までの線分をたどって、間のマスも通ったことにする。
//! 時刻が戻ったとき (最初から再生し直したとき) は、記録を消して数え直す。
//!
//! Bevy に依存しない純粋なデータと計算だけを置く。長さは mm、時間は秒、床の座標は (x, z)。

use std::collections::HashMap;

use glam::DVec2;

/// マス目の大きさの既定 (mm)
pub const DEFAULT_CELL: f64 = 500.0;
/// マス目の大きさの範囲 (mm)
pub const MIN_CELL: f64 = 100.0;
pub const MAX_CELL: f64 = 2000.0;
/// 1フレームでこれより遠くへ動いたら、歩いたのではなく置き直したものとして間をたどらない (mm)
const TELEPORT_DISTANCE: f64 = 10_000.0;

/// 床の上を動くものの種類
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoverKind {
    Person,
    Agv,
}

impl MoverKind {
    fn index(self) -> usize {
        match self {
            MoverKind::Person => 0,
            MoverKind::Agv => 1,
        }
    }
}

/// 動くもの 1つ (作業者の id、または AGV の番号)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MoverId {
    pub kind: MoverKind,
    pub id: u32,
}

impl MoverId {
    pub fn person(id: u32) -> Self {
        Self {
            kind: MoverKind::Person,
            id,
        }
    }

    pub fn agv(id: u32) -> Self {
        Self {
            kind: MoverKind::Agv,
            id,
        }
    }
}

/// どれを数えるか
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoverFilter {
    #[default]
    All,
    People,
    Agvs,
}

impl MoverFilter {
    pub const ALL: [MoverFilter; 3] = [MoverFilter::All, MoverFilter::People, MoverFilter::Agvs];

    pub fn label(self) -> &'static str {
        match self {
            MoverFilter::All => "人と AGV",
            MoverFilter::People => "人だけ",
            MoverFilter::Agvs => "AGV だけ",
        }
    }

    fn includes(self, kind: MoverKind) -> bool {
        match self {
            MoverFilter::All => true,
            MoverFilter::People => kind == MoverKind::Person,
            MoverFilter::Agvs => kind == MoverKind::Agv,
        }
    }
}

/// 何で色を付けるか
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeatMetric {
    /// マスに入った回数
    #[default]
    Passes,
    /// マスにいた時間 (止まっている・待っている場所が濃くなる)
    Dwell,
}

impl HeatMetric {
    pub const ALL: [HeatMetric; 2] = [HeatMetric::Passes, HeatMetric::Dwell];

    pub fn label(self) -> &'static str {
        match self {
            HeatMetric::Passes => "通った回数",
            HeatMetric::Dwell => "いた時間",
        }
    }

    /// 値を表示用の文字にする
    pub fn format(self, value: f64) -> String {
        match self {
            HeatMetric::Passes => format!("{value:.0} 回"),
            HeatMetric::Dwell => format!("{value:.1} 秒"),
        }
    }
}

/// 1マスの集計 (添字は [人, AGV])
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct CellStats {
    passes: [u32; 2],
    seconds: [f64; 2],
}

impl CellStats {
    fn value(&self, filter: MoverFilter, metric: HeatMetric) -> f64 {
        [MoverKind::Person, MoverKind::Agv]
            .into_iter()
            .filter(|&k| filter.includes(k))
            .map(|k| match metric {
                HeatMetric::Passes => self.passes[k.index()] as f64,
                HeatMetric::Dwell => self.seconds[k.index()],
            })
            .sum()
    }
}

/// 動くもの 1つの、前のフレームの位置と歩いた距離
#[derive(Clone, Copy, Debug)]
struct Track {
    pos: DVec2,
    cell: (i32, i32),
    distance: f64,
}

/// 混み合う場所 1つ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hotspot {
    /// マスの中心 (x, z)
    pub center: DVec2,
    pub value: f64,
}

/// 動くもの 1つの集計
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoverTotal {
    pub mover: MoverId,
    /// 記録を始めてから動いた距離 (mm)
    pub distance: f64,
}

/// 色を付けた画像 (左上から右へ、行は z の小さい方から)
#[derive(Clone, Debug, PartialEq)]
pub struct HeatImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// 画像の左上の角の床の位置 (x, z)
    pub min: DVec2,
    /// 画像の右下の角の床の位置 (x, z)
    pub max: DVec2,
}

/// 動線のヒートマップ
#[derive(Clone, Debug)]
pub struct TrafficHeatmap {
    cell: f64,
    cells: HashMap<(i32, i32), CellStats>,
    tracks: HashMap<MoverId, Track>,
    time: Option<f64>,
    /// 記録した時間の長さ (秒)
    recorded: f64,
}

impl Default for TrafficHeatmap {
    fn default() -> Self {
        Self::new(DEFAULT_CELL)
    }
}

impl TrafficHeatmap {
    pub fn new(cell: f64) -> Self {
        Self {
            cell: cell.clamp(MIN_CELL, MAX_CELL),
            cells: HashMap::new(),
            tracks: HashMap::new(),
            time: None,
            recorded: 0.0,
        }
    }

    /// マス目の大きさ (mm)
    pub fn cell(&self) -> f64 {
        self.cell
    }

    /// 記録した時間の長さ (秒)
    pub fn recorded_seconds(&self) -> f64 {
        self.recorded
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// 記録を消す (マス目の大きさはそのまま)
    pub fn clear(&mut self) {
        *self = Self::new(self.cell);
    }

    /// マス目の大きさを変える。数え方が変わるので記録は消す
    pub fn set_cell(&mut self, cell: f64) {
        *self = Self::new(cell);
    }

    fn cell_of(&self, p: DVec2) -> (i32, i32) {
        (
            (p.x / self.cell).floor() as i32,
            (p.y / self.cell).floor() as i32,
        )
    }

    fn cell_center(&self, (i, j): (i32, i32)) -> DVec2 {
        DVec2::new((i as f64 + 0.5) * self.cell, (j as f64 + 0.5) * self.cell)
    }

    /// 時刻 `time` の位置を記録する。時刻が進んでいなければ何もしない。
    /// 時刻が戻ったら、最初から再生し直したものとして記録を消してから数え始める。
    /// 前のフレームにいて今回いないもの (消した作業者など) は追うのをやめる。
    pub fn observe(&mut self, time: f64, positions: &[(MoverId, DVec2)]) {
        let dt = match self.time {
            Some(prev) if time < prev => {
                self.clear();
                0.0
            }
            Some(prev) if time == prev => return,
            Some(prev) => time - prev,
            None => 0.0,
        };
        self.time = Some(time);
        self.recorded += dt;
        let mut tracks = HashMap::with_capacity(positions.len());
        for &(mover, pos) in positions {
            let track = match self.tracks.get(&mover).copied() {
                Some(prev) => self.walk(mover.kind, prev, pos, dt),
                None => {
                    let cell = self.cell_of(pos);
                    self.cells.entry(cell).or_default().passes[mover.kind.index()] += 1;
                    Track {
                        pos,
                        cell,
                        distance: 0.0,
                    }
                }
            };
            tracks.insert(mover, track);
        }
        self.tracks = tracks;
    }

    /// 前の位置から今の位置まで線分をたどり、入ったマスを数え、時間を割り振る
    fn walk(&mut self, kind: MoverKind, prev: Track, pos: DVec2, dt: f64) -> Track {
        let k = kind.index();
        let length = prev.pos.distance(pos);
        if length > TELEPORT_DISTANCE {
            // 置き直した: 間はたどらず、新しい場所から数え直す
            let cell = self.cell_of(pos);
            let stats = self.cells.entry(cell).or_default();
            stats.passes[k] += 1;
            stats.seconds[k] += dt;
            return Track {
                pos,
                cell,
                distance: prev.distance,
            };
        }
        // マスの 1/4 ごとに点を取る (斜めに角をかすめるマスも拾えるくらい細かく)
        let steps = ((length / (self.cell / 4.0)).ceil() as usize).max(1);
        let share = dt / steps as f64;
        let mut cell = prev.cell;
        for s in 1..=steps {
            let p = prev.pos.lerp(pos, s as f64 / steps as f64);
            let c = self.cell_of(p);
            let stats = self.cells.entry(c).or_default();
            if c != cell {
                stats.passes[k] += 1;
                cell = c;
            }
            stats.seconds[k] += share;
        }
        Track {
            pos,
            cell,
            distance: prev.distance + length,
        }
    }

    /// 位置 `p` のマスの値
    pub fn value_at(&self, p: DVec2, filter: MoverFilter, metric: HeatMetric) -> f64 {
        self.cells
            .get(&self.cell_of(p))
            .map_or(0.0, |c| c.value(filter, metric))
    }

    /// いちばん大きいマスの値
    pub fn max_value(&self, filter: MoverFilter, metric: HeatMetric) -> f64 {
        self.cells
            .values()
            .map(|c| c.value(filter, metric))
            .fold(0.0, f64::max)
    }

    /// 値の大きい順にマスを `count` 個 (値が 0 のマスは除く)
    pub fn hotspots(&self, filter: MoverFilter, metric: HeatMetric, count: usize) -> Vec<Hotspot> {
        let mut spots: Vec<_> = self
            .cells
            .iter()
            .map(|(&c, s)| (c, s.value(filter, metric)))
            .filter(|&(_, v)| v > 0.0)
            .collect();
        // 同じ値なら位置で並べ、毎回同じ順にする
        spots.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        spots
            .into_iter()
            .take(count)
            .map(|(c, value)| Hotspot {
                center: self.cell_center(c),
                value,
            })
            .collect()
    }

    /// 動くものごとの、記録を始めてから動いた距離 (種類 → id の順)
    pub fn totals(&self) -> Vec<MoverTotal> {
        let mut totals: Vec<_> = self
            .tracks
            .iter()
            .map(|(&mover, t)| MoverTotal {
                mover,
                distance: t.distance,
            })
            .collect();
        totals.sort_by_key(|t| (t.mover.kind.index(), t.mover.id));
        totals
    }

    /// 値の入ったマスの範囲 (マスの番号、両端を含む)
    fn cell_range(
        &self,
        filter: MoverFilter,
        metric: HeatMetric,
    ) -> Option<((i32, i32), (i32, i32))> {
        let mut range: Option<((i32, i32), (i32, i32))> = None;
        for (&(i, j), s) in &self.cells {
            if s.value(filter, metric) <= 0.0 {
                continue;
            }
            range = Some(match range {
                None => ((i, j), (i, j)),
                Some((lo, hi)) => ((lo.0.min(i), lo.1.min(j)), (hi.0.max(i), hi.1.max(j))),
            });
        }
        range
    }

    /// 1マス = 1画素の、床に重ねる画像。値のないマスは透明。
    /// 周りに 1マスずつ透明な縁を付ける (なめらかに拡大したときに端が切れないように)。
    pub fn render(&self, filter: MoverFilter, metric: HeatMetric) -> Option<HeatImage> {
        let ((i0, j0), (i1, j1)) = self.cell_range(filter, metric)?;
        let (i0, j0, i1, j1) = (i0 - 1, j0 - 1, i1 + 1, j1 + 1);
        let width = (i1 - i0 + 1) as u32;
        let height = (j1 - j0 + 1) as u32;
        let max = self.max_value(filter, metric);
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for (&(i, j), s) in &self.cells {
            let v = s.value(filter, metric);
            if v <= 0.0 {
                continue;
            }
            let px = ((j - j0) as u32 * width + (i - i0) as u32) as usize * 4;
            rgba[px..px + 4].copy_from_slice(&heat_color(v / max));
        }
        Some(HeatImage {
            width,
            height,
            rgba,
            min: DVec2::new(i0 as f64, j0 as f64) * self.cell,
            max: DVec2::new((i1 + 1) as f64, (j1 + 1) as f64) * self.cell,
        })
    }

    /// 書き出し用の画像。白地に 1 m の方眼、`lines` (通路や歩行経路) を灰色の線で描き、上に色を重ねる。
    /// 範囲は値の入ったマスと `lines` を含む。長い辺がおよそ `long_side` 画素になるよう拡大する。
    pub fn render_export(
        &self,
        filter: MoverFilter,
        metric: HeatMetric,
        lines: &[(DVec2, DVec2)],
        long_side: u32,
    ) -> Option<HeatImage> {
        // 範囲 (mm)
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        if let Some(((i0, j0), (i1, j1))) = self.cell_range(filter, metric) {
            lo = lo.min(DVec2::new(i0 as f64, j0 as f64) * self.cell);
            hi = hi.max(DVec2::new((i1 + 1) as f64, (j1 + 1) as f64) * self.cell);
        }
        for &(a, b) in lines {
            lo = lo.min(a).min(b);
            hi = hi.max(a).max(b);
        }
        if !lo.is_finite() || !hi.is_finite() {
            return None;
        }
        // 1マスぶんの余白を付ける
        lo -= DVec2::splat(self.cell);
        hi += DVec2::splat(self.cell);
        let size = hi - lo;
        let mm_per_px = (size.x.max(size.y) / long_side.max(16) as f64).max(1.0);
        let width = (size.x / mm_per_px).ceil().max(1.0) as u32;
        let height = (size.y / mm_per_px).ceil().max(1.0) as u32;
        let mut img = Canvas::new(width, height, [255, 255, 255, 255]);
        // 1 m の方眼
        let grid = [232, 232, 236, 255];
        let mut x = (lo.x / 1000.0).ceil() * 1000.0;
        while x < hi.x {
            let px = ((x - lo.x) / mm_per_px) as i64;
            img.line((px, 0), (px, height as i64 - 1), grid);
            x += 1000.0;
        }
        let mut z = (lo.y / 1000.0).ceil() * 1000.0;
        while z < hi.y {
            let py = ((z - lo.y) / mm_per_px) as i64;
            img.line((0, py), (width as i64 - 1, py), grid);
            z += 1000.0;
        }
        let to_px = |p: DVec2| {
            let q = (p - lo) / mm_per_px;
            (q.x.round() as i64, q.y.round() as i64)
        };
        for &(a, b) in lines {
            img.line(to_px(a), to_px(b), [120, 120, 128, 255]);
        }
        // 色: 画素の中心が入るマスの値
        let max = self.max_value(filter, metric);
        if max > 0.0 {
            for py in 0..height {
                for px in 0..width {
                    let p = lo + (DVec2::new(px as f64, py as f64) + 0.5) * mm_per_px;
                    let v = self.value_at(p, filter, metric);
                    if v > 0.0 {
                        img.blend(px, py, heat_color(v / max));
                    }
                }
            }
        }
        Some(HeatImage {
            width,
            height,
            rgba: img.rgba,
            min: lo,
            max: lo + DVec2::new(width as f64, height as f64) * mm_per_px,
        })
    }
}

/// 0〜1 の値の色。少ないほど青く薄く、多いほど赤く濃い (青 → 水色 → 緑 → 黄 → 赤)
pub fn heat_color(t: f64) -> [u8; 4] {
    const STOPS: [(f64, [f64; 3]); 5] = [
        (0.0, [40.0, 90.0, 230.0]),
        (0.25, [30.0, 200.0, 230.0]),
        (0.5, [60.0, 200.0, 80.0]),
        (0.75, [250.0, 210.0, 40.0]),
        (1.0, [230.0, 40.0, 30.0]),
    ];
    let t = if t.is_finite() {
        t.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let k = STOPS
        .windows(2)
        .position(|w| t <= w[1].0)
        .unwrap_or(STOPS.len() - 2);
    let (t0, c0) = STOPS[k];
    let (t1, c1) = STOPS[k + 1];
    let f = (t - t0) / (t1 - t0);
    let ch = |i: usize| (c0[i] + (c1[i] - c0[i]) * f).round() as u8;
    // 1回だけ通ったマスも見えるよう、薄くても半分ほどの濃さにする
    let alpha = (130.0 + 110.0 * t).round() as u8;
    [ch(0), ch(1), ch(2), alpha]
}

/// 書き出し用の簡単な描画先
struct Canvas {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32, fill: [u8; 4]) -> Self {
        Self {
            width,
            height,
            rgba: fill.repeat((width * height) as usize),
        }
    }

    fn put(&mut self, x: i64, y: i64, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        self.rgba[i..i + 4].copy_from_slice(&color);
    }

    /// 不透明な地の上に、透明度付きの色を重ねる
    fn blend(&mut self, x: u32, y: u32, color: [u8; 4]) {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let a = color[3] as f64 / 255.0;
        for (dst, &src) in self.rgba[i..i + 3].iter_mut().zip(&color[..3]) {
            let base = *dst as f64;
            *dst = (base + (src as f64 - base) * a).round() as u8;
        }
    }

    /// 2点を結ぶ線 (ブレゼンハム)
    fn line(&mut self, (mut x0, mut y0): (i64, i64), (x1, y1): (i64, i64), color: [u8; 4]) {
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx + dy;
        loop {
            self.put(x0, y0, color);
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, z: f64) -> DVec2 {
        DVec2::new(x, z)
    }

    #[test]
    fn walking_across_cells_counts_each_cell_once() {
        let mut map = TrafficHeatmap::new(1000.0);
        let a = MoverId::person(1);
        map.observe(0.0, &[(a, p(500.0, 500.0))]);
        // 1フレームで 3マス先まで進んでも、間の 2マスも通ったことになる
        map.observe(1.0, &[(a, p(3500.0, 500.0))]);
        for x in [500.0, 1500.0, 2500.0, 3500.0] {
            assert_eq!(
                map.value_at(p(x, 500.0), MoverFilter::All, HeatMetric::Passes),
                1.0,
                "x = {x}"
            );
        }
        assert_eq!(
            map.value_at(p(4500.0, 500.0), MoverFilter::All, HeatMetric::Passes),
            0.0
        );
        // 1秒を通ったマスに割り振る (合計 1秒)
        let total: f64 = [500.0, 1500.0, 2500.0, 3500.0]
            .iter()
            .map(|&x| map.value_at(p(x, 500.0), MoverFilter::All, HeatMetric::Dwell))
            .sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert_eq!(map.totals()[0].distance, 3000.0);
    }

    #[test]
    fn going_back_and_forth_adds_passes() {
        let mut map = TrafficHeatmap::new(1000.0);
        let a = MoverId::person(1);
        for (t, x) in [(0.0, 500.0), (1.0, 1500.0), (2.0, 500.0), (3.0, 1500.0)] {
            map.observe(t, &[(a, p(x, 0.0))]);
        }
        assert_eq!(
            map.value_at(p(500.0, 0.0), MoverFilter::All, HeatMetric::Passes),
            2.0
        );
        assert_eq!(
            map.value_at(p(1500.0, 0.0), MoverFilter::All, HeatMetric::Passes),
            2.0
        );
    }

    #[test]
    fn standing_still_adds_dwell_but_not_passes() {
        let mut map = TrafficHeatmap::new(500.0);
        let agv = MoverId::agv(0);
        for t in 0..=10 {
            map.observe(t as f64, &[(agv, p(100.0, 100.0))]);
        }
        assert_eq!(
            map.value_at(p(100.0, 100.0), MoverFilter::All, HeatMetric::Passes),
            1.0
        );
        assert!(
            (map.value_at(p(100.0, 100.0), MoverFilter::All, HeatMetric::Dwell) - 10.0).abs()
                < 1e-9
        );
        assert_eq!(map.recorded_seconds(), 10.0);
    }

    #[test]
    fn filters_separate_people_and_agvs() {
        let mut map = TrafficHeatmap::new(1000.0);
        let person = MoverId::person(1);
        let agv = MoverId::agv(0);
        map.observe(0.0, &[(person, p(500.0, 500.0)), (agv, p(500.0, 500.0))]);
        map.observe(1.0, &[(person, p(500.0, 500.0)), (agv, p(1500.0, 500.0))]);
        let here = p(500.0, 500.0);
        assert_eq!(
            map.value_at(here, MoverFilter::All, HeatMetric::Passes),
            2.0
        );
        assert_eq!(
            map.value_at(here, MoverFilter::People, HeatMetric::Passes),
            1.0
        );
        assert_eq!(
            map.value_at(here, MoverFilter::Agvs, HeatMetric::Passes),
            1.0
        );
        assert_eq!(
            map.value_at(p(1500.0, 500.0), MoverFilter::People, HeatMetric::Passes),
            0.0
        );
        let hot = map.hotspots(MoverFilter::All, HeatMetric::Passes, 1);
        assert_eq!(
            hot,
            vec![Hotspot {
                center: here,
                value: 2.0
            }]
        );
    }

    #[test]
    fn rewinding_time_starts_over() {
        let mut map = TrafficHeatmap::new(1000.0);
        let a = MoverId::person(1);
        map.observe(5.0, &[(a, p(0.0, 0.0))]);
        map.observe(6.0, &[(a, p(2000.0, 0.0))]);
        assert!(map.max_value(MoverFilter::All, HeatMetric::Passes) > 0.0);
        map.observe(0.0, &[(a, p(0.0, 0.0))]);
        // 最初の位置だけが数えられ、巻き戻しの移動は線としてたどらない
        assert_eq!(
            map.value_at(p(1500.0, 0.0), MoverFilter::All, HeatMetric::Passes),
            0.0
        );
        assert_eq!(
            map.value_at(p(500.0, 0.0), MoverFilter::All, HeatMetric::Passes),
            1.0
        );
        assert_eq!(map.totals()[0].distance, 0.0);
        assert_eq!(map.recorded_seconds(), 0.0);
    }

    #[test]
    fn same_time_is_ignored_and_teleports_are_not_traced() {
        let mut map = TrafficHeatmap::new(1000.0);
        let a = MoverId::person(1);
        map.observe(0.0, &[(a, p(500.0, 500.0))]);
        map.observe(0.0, &[(a, p(5500.0, 500.0))]);
        assert_eq!(
            map.value_at(p(5500.0, 500.0), MoverFilter::All, HeatMetric::Passes),
            0.0
        );
        map.observe(1.0, &[(a, p(50_500.0, 500.0))]);
        assert_eq!(
            map.value_at(p(20_500.0, 500.0), MoverFilter::All, HeatMetric::Passes),
            0.0
        );
        assert_eq!(
            map.value_at(p(50_500.0, 500.0), MoverFilter::All, HeatMetric::Passes),
            1.0
        );
    }

    #[test]
    fn removed_movers_are_forgotten() {
        let mut map = TrafficHeatmap::new(1000.0);
        map.observe(
            0.0,
            &[
                (MoverId::person(1), p(0.0, 0.0)),
                (MoverId::agv(0), p(0.0, 0.0)),
            ],
        );
        map.observe(1.0, &[(MoverId::agv(0), p(0.0, 0.0))]);
        assert_eq!(map.totals().len(), 1);
        assert_eq!(map.totals()[0].mover, MoverId::agv(0));
    }

    #[test]
    fn render_places_cells_with_transparent_border() {
        let mut map = TrafficHeatmap::new(1000.0);
        let a = MoverId::person(1);
        map.observe(0.0, &[(a, p(500.0, 500.0))]);
        map.observe(1.0, &[(a, p(1500.0, 500.0))]);
        map.observe(2.0, &[(a, p(500.0, 500.0))]);
        assert!(map.render(MoverFilter::Agvs, HeatMetric::Passes).is_none());
        let img = map.render(MoverFilter::All, HeatMetric::Passes).unwrap();
        // 2 × 1 マスに縁を付けて 4 × 3
        assert_eq!((img.width, img.height), (4, 3));
        assert_eq!(img.min, p(-1000.0, -1000.0));
        assert_eq!(img.max, p(3000.0, 2000.0));
        let px = |x: u32, y: u32| {
            let i = ((y * img.width + x) * 4) as usize;
            [
                img.rgba[i],
                img.rgba[i + 1],
                img.rgba[i + 2],
                img.rgba[i + 3],
            ]
        };
        assert_eq!(px(0, 0)[3], 0);
        // 2回入ったマスが最大 (赤)、1回のマスはその半分
        assert_eq!(px(1, 1), heat_color(1.0));
        assert_eq!(px(2, 1), heat_color(0.5));
    }

    #[test]
    fn export_includes_lines_and_heat() {
        let mut map = TrafficHeatmap::new(500.0);
        let a = MoverId::agv(0);
        map.observe(0.0, &[(a, p(0.0, 0.0))]);
        map.observe(1.0, &[(a, p(2000.0, 0.0))]);
        let lines = [(p(0.0, 0.0), p(0.0, 4000.0))];
        let img = map
            .render_export(MoverFilter::All, HeatMetric::Passes, &lines, 200)
            .unwrap();
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        assert!(img.width.max(img.height) <= 201);
        // 範囲は通路の線を含む
        assert!(img.min.y < 0.0 && img.max.y > 4000.0);
        // 画像はすべて不透明で、白以外の画素 (色や線) がある
        assert!(img.rgba.chunks(4).all(|c| c[3] == 255));
        assert!(img.rgba.chunks(4).any(|c| c[0] > 200 && c[1] < 100));
        assert!(
            TrafficHeatmap::default()
                .render_export(MoverFilter::All, HeatMetric::Passes, &[], 200)
                .is_none()
        );
    }

    #[test]
    fn heat_color_runs_blue_to_red() {
        let cold = heat_color(0.0);
        let hot = heat_color(1.0);
        assert!(cold[2] > cold[0]);
        assert!(hot[0] > hot[2]);
        assert!(hot[3] > cold[3]);
        assert_eq!(heat_color(f64::NAN), cold);
        assert_eq!(heat_color(2.0), hot);
    }

    #[test]
    fn changing_cell_size_clears() {
        let mut map = TrafficHeatmap::new(50.0);
        assert_eq!(map.cell(), MIN_CELL);
        map.observe(0.0, &[(MoverId::person(1), p(0.0, 0.0))]);
        map.set_cell(1000.0);
        assert!(map.is_empty());
        assert_eq!(map.cell(), 1000.0);
    }
}
