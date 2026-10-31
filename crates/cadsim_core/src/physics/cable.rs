//! ケーブル・ホースの紐モデル (ドメイン。Bevy に依存しない)
//!
//! 両端を部品に付けた紐を、点を鎖のようにつないだもの (位置ベースの動力学) で表す。
//! 重力で垂れ、部品の外接箱に当たると押し出され、可動部に引かれると一緒に動く。
//! 毎刻みの後に、曲げ半径・長さの不足・当たっている部品を調べる。

use glam::DVec3;

use super::world::Iso;

/// 紐の端を付ける場所
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CableAnchor {
    /// 付ける部品 (ノード id)。None ならワールドに固定。
    pub node: Option<u32>,
    /// 部品の座標系 (node が None ならワールド座標) での位置
    pub local: DVec3,
}

/// ケーブルかホースか (表示の名前と既定値だけが違う)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CableKind {
    #[default]
    Cable,
    Hose,
}

impl CableKind {
    pub const ALL: [CableKind; 2] = [CableKind::Cable, CableKind::Hose];

    pub fn label(self) -> &'static str {
        match self {
            CableKind::Cable => "ケーブル",
            CableKind::Hose => "ホース",
        }
    }
}

/// ケーブル・ホース1本の設計値 (長さはモデル単位)
#[derive(Clone, Debug, PartialEq)]
pub struct Cable {
    pub name: String,
    pub kind: CableKind,
    pub start: CableAnchor,
    pub end: CableAnchor,
    pub length: f64,
    pub diameter: f64,
    /// 許容する最小の曲げ半径。これより小さく曲がったら知らせる。
    pub min_bend_radius: f64,
}

impl Cable {
    /// 点の間隔。細かいほど滑らかだが重い。直径の 2 倍を目安に、8〜48 区間に収める。
    pub fn segments(&self) -> usize {
        let wanted = (self.length / (self.diameter * 2.0).max(1e-9)).ceil();
        (wanted as usize).clamp(8, 48)
    }
}

/// 紐が当たる箱 (部品の外接箱をワールドに置いたもの)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obstacle {
    pub node: u32,
    /// 箱の中心と向き
    pub pose: Iso,
    pub half: DVec3,
}

impl Obstacle {
    /// 半径 `radius` の玉が箱に食い込んでいたら、押し出した位置を返す
    fn push_out(&self, p: DVec3, radius: f64) -> Option<DVec3> {
        let local = self.pose.rotation.inverse() * (p - self.pose.translation);
        let reach = self.half + DVec3::splat(radius);
        let depth = reach - local.abs();
        if depth.min_element() <= 0.0 {
            return None;
        }
        // いちばん浅い面から押し出す
        let mut out = local;
        let axis = if depth.x <= depth.y && depth.x <= depth.z {
            0
        } else if depth.y <= depth.z {
            1
        } else {
            2
        };
        out[axis] = reach[axis].copysign(if local[axis] == 0.0 { 1.0 } else { local[axis] });
        Some(self.pose.apply(out))
    }
}

/// 拘束を解く回数。多いほど伸びにくい。
const ITERATIONS: usize = 24;
/// 速度の減衰 (1 刻みあたりに残す割合)。紐が揺れ続けないようにする。
const DAMPING: f64 = 0.98;
/// 曲げにくさ (0 = 自由に折れる, 1 = まっすぐに戻ろうとする)
const BEND_STIFFNESS: f64 = 0.05;

/// 紐の点の状態
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CableState {
    pub points: Vec<DVec3>,
    previous: Vec<DVec3>,
}

/// 紐の状態を調べた結果
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CableReport {
    /// いちばんきつく曲がったところの半径
    pub min_bend_radius: f64,
    /// 許容より小さく曲がった点の番号
    pub over_bent: Vec<usize>,
    /// 両端の距離 ÷ 長さ。1 を超えたら長さが足りない (引っ張られている)。
    pub stretch: f64,
    /// 当たっている部品 (ノード id, 番号順)
    pub touching: Vec<u32>,
}

impl CableReport {
    pub fn is_taut(&self) -> bool {
        self.stretch > 0.999
    }

    pub fn is_ok(&self) -> bool {
        self.over_bent.is_empty() && !self.is_taut()
    }
}

impl CableState {
    /// 両端を結んだ線の上に点を並べる (余った長さは次の刻みから垂れる)
    pub fn new(cable: &Cable, start: DVec3, end: DVec3) -> Self {
        let n = cable.segments();
        let points: Vec<DVec3> = (0..=n)
            .map(|i| start.lerp(end, i as f64 / n as f64))
            .collect();
        Self {
            previous: points.clone(),
            points,
        }
    }

    /// 設計値が変わって点の数が合わなくなったか
    pub fn fits(&self, cable: &Cable) -> bool {
        self.points.len() == cable.segments() + 1
    }

    /// `dt` 秒進める。`start` / `end` はこの刻みでの両端の位置。
    pub fn step(
        &mut self,
        cable: &Cable,
        start: DVec3,
        end: DVec3,
        obstacles: &[Obstacle],
        gravity: DVec3,
        dt: f64,
    ) {
        if !self.fits(cable) {
            *self = Self::new(cable, start, end);
        }
        let n = self.points.len() - 1;
        let rest = cable.length / n as f64;
        let radius = cable.diameter / 2.0;

        // 慣性と重力 (Verlet 積分)
        for (p, prev) in self.points.iter_mut().zip(self.previous.iter_mut()) {
            let velocity = (*p - *prev) * DAMPING;
            *prev = *p;
            *p += velocity + gravity * dt * dt;
        }

        for _ in 0..ITERATIONS {
            self.points[0] = start;
            self.points[n] = end;
            // 隣どうしの距離を区間の長さに保つ
            for i in 0..n {
                let (a, b) = (self.points[i], self.points[i + 1]);
                let delta = b - a;
                let length = delta.length();
                if length < 1e-12 {
                    continue;
                }
                let correction = delta * ((length - rest) / length);
                let (wa, wb) = weights(i, i + 1, n);
                self.points[i] += correction * wa;
                self.points[i + 1] -= correction * wb;
            }
            // 1 つ飛ばしの点を少しだけまっすぐに引き、折れ曲がりをなまらせる
            for i in 0..n.saturating_sub(1) {
                let (a, c) = (self.points[i], self.points[i + 2]);
                let delta = c - a;
                let length = delta.length();
                if length < 1e-12 || length >= 2.0 * rest {
                    continue;
                }
                let correction = delta * ((length - 2.0 * rest) / length) * BEND_STIFFNESS;
                let (wa, wc) = weights(i, i + 2, n);
                self.points[i] += correction * wa;
                self.points[i + 2] -= correction * wc;
            }
            // 部品に食い込んだ点を外へ出す。端の近くは、端を埋め込んだ部品の中にあってよい。
            for i in 1..n {
                for obstacle in obstacles {
                    if anchored_near(cable, obstacle, start, end, i, n) {
                        continue;
                    }
                    if let Some(out) = obstacle.push_out(self.points[i], radius) {
                        self.points[i] = out;
                    }
                }
            }
        }
        self.points[0] = start;
        self.points[n] = end;
    }

    /// 曲げ半径・長さ・当たっている部品を調べる
    pub fn report(&self, cable: &Cable, obstacles: &[Obstacle]) -> CableReport {
        let n = self.points.len().saturating_sub(1);
        let mut min_bend_radius = f64::INFINITY;
        let mut over_bent = Vec::new();
        for i in 1..n {
            let r = bend_radius(self.points[i - 1], self.points[i], self.points[i + 1]);
            min_bend_radius = min_bend_radius.min(r);
            if r < cable.min_bend_radius {
                over_bent.push(i);
            }
        }
        let span = match (self.points.first(), self.points.last()) {
            (Some(a), Some(b)) => a.distance(*b),
            _ => 0.0,
        };
        // 押し出した後の点は表面ちょうどにあるので、少しだけ広げて当たりとみなす
        let radius = cable.diameter / 2.0 * 1.05 + 1e-9;
        let mut touching: Vec<u32> = obstacles
            .iter()
            .filter(|o| {
                (1..n).any(|i| {
                    !anchored_near(cable, o, self.points[0], self.points[n], i, n)
                        && o.push_out(self.points[i], radius).is_some()
                })
            })
            .map(|o| o.node)
            .collect();
        touching.sort_unstable();
        touching.dedup();
        CableReport {
            min_bend_radius,
            over_bent,
            stretch: if cable.length > 0.0 {
                span / cable.length
            } else {
                f64::INFINITY
            },
            touching,
        }
    }
}

/// 拘束で動かす割合。端は固定なので動かさず、相手が全部動く。
fn weights(a: usize, b: usize, n: usize) -> (f64, f64) {
    match (a == 0 || a == n, b == 0 || b == n) {
        (true, true) => (0.0, 0.0),
        (true, false) => (0.0, 1.0),
        (false, true) => (1.0, 0.0),
        (false, false) => (0.5, 0.5),
    }
}

/// 点 i が端のすぐ近く (端から 2 区間以内) にあり、その端が `obstacle` に付いているか埋まっているか。
/// 継手は部品に埋め込んで付けるので、そこから出るまでの紐は部品と当たってよい。
fn anchored_near(
    cable: &Cable,
    obstacle: &Obstacle,
    start: DVec3,
    end: DVec3,
    i: usize,
    n: usize,
) -> bool {
    let embedded = |anchor: &CableAnchor, point: DVec3| {
        anchor.node == Some(obstacle.node)
            || obstacle.push_out(point, cable.diameter / 2.0).is_some()
    };
    (i <= 2 && embedded(&cable.start, start)) || (i + 2 >= n && embedded(&cable.end, end))
}

/// 3 点を通る円の半径。まっすぐなら無限大。
pub fn bend_radius(a: DVec3, b: DVec3, c: DVec3) -> f64 {
    let cross = (b - a).cross(c - a).length();
    if cross < 1e-12 {
        return f64::INFINITY;
    }
    a.distance(b) * b.distance(c) * c.distance(a) / (2.0 * cross)
}

#[cfg(test)]
mod tests {
    use super::*;

    const G: DVec3 = DVec3::new(0.0, -9810.0, 0.0);
    const DT: f64 = 1.0 / 120.0;

    fn cable(length: f64) -> Cable {
        Cable {
            name: "c".into(),
            kind: CableKind::Cable,
            start: CableAnchor {
                node: None,
                local: DVec3::ZERO,
            },
            end: CableAnchor {
                node: None,
                local: DVec3::new(400.0, 0.0, 0.0),
            },
            length,
            diameter: 10.0,
            min_bend_radius: 30.0,
        }
    }

    fn settle(state: &mut CableState, c: &Cable, a: DVec3, b: DVec3, obstacles: &[Obstacle]) {
        for _ in 0..600 {
            state.step(c, a, b, obstacles, G, DT);
        }
    }

    #[test]
    fn bend_radius_of_a_quarter_circle() {
        let r = bend_radius(
            DVec3::new(10.0, 0.0, 0.0),
            DVec3::new(0.0, 10.0, 0.0),
            DVec3::new(-10.0, 0.0, 0.0),
        );
        assert!((r - 10.0).abs() < 1e-9);
        assert!(bend_radius(DVec3::ZERO, DVec3::X, DVec3::X * 2.0).is_infinite());
    }

    #[test]
    fn a_slack_cable_sags_and_keeps_its_length() {
        let c = cable(600.0);
        let (a, b) = (DVec3::ZERO, DVec3::new(400.0, 0.0, 0.0));
        let mut state = CableState::new(&c, a, b);
        settle(&mut state, &c, a, b, &[]);
        let lowest = state
            .points
            .iter()
            .map(|p| p.y)
            .fold(f64::INFINITY, f64::min);
        assert!(lowest < -150.0, "lowest = {lowest}");
        let length: f64 = state.points.windows(2).map(|w| w[0].distance(w[1])).sum();
        assert!((length - 600.0).abs() / 600.0 < 0.03, "length = {length}");
        let report = state.report(&c, &[]);
        assert!(!report.is_taut());
        assert!(report.touching.is_empty());
    }

    #[test]
    fn a_short_cable_is_reported_as_taut() {
        let c = cable(300.0);
        let (a, b) = (DVec3::ZERO, DVec3::new(400.0, 0.0, 0.0));
        let mut state = CableState::new(&c, a, b);
        settle(&mut state, &c, a, b, &[]);
        let report = state.report(&c, &[]);
        assert!(report.is_taut() && !report.is_ok());
    }

    #[test]
    fn the_cable_drapes_over_a_box_instead_of_passing_through() {
        let c = cable(900.0);
        let (a, b) = (DVec3::new(0.0, 100.0, 0.0), DVec3::new(400.0, 100.0, 0.0));
        let block = Obstacle {
            node: 7,
            pose: Iso::from_translation(DVec3::new(200.0, -100.0, 0.0)),
            half: DVec3::new(50.0, 100.0, 200.0),
        };
        let mut state = CableState::new(&c, a, b);
        settle(&mut state, &c, a, b, &[block]);
        // 箱の上面 (y = 0) より下に、箱の幅の中で入り込んだ点はない
        for p in &state.points {
            if (p.x - 200.0).abs() < 45.0 && p.z.abs() < 195.0 {
                assert!(p.y >= -1e-6 + 5.0 - 0.5, "point inside the box: {p}");
            }
        }
        let report = state.report(&c, &[block]);
        assert_eq!(report.touching, vec![7]);
        // 箱の角で折れるので、曲げ半径が小さくなる
        assert!(report.min_bend_radius < 200.0);
    }

    #[test]
    fn a_tight_fold_is_reported() {
        let c = Cable {
            min_bend_radius: 1000.0,
            ..cable(600.0)
        };
        let (a, b) = (DVec3::ZERO, DVec3::new(100.0, 0.0, 0.0));
        let mut state = CableState::new(&c, a, b);
        settle(&mut state, &c, a, b, &[]);
        let report = state.report(&c, &[]);
        assert!(!report.over_bent.is_empty());
    }

    #[test]
    fn changing_the_length_rebuilds_the_points() {
        let mut c = cable(100.0);
        let mut state = CableState::new(&c, DVec3::ZERO, DVec3::X * 50.0);
        c.length = 2000.0;
        assert!(!state.fits(&c));
        state.step(&c, DVec3::ZERO, DVec3::X * 50.0, &[], G, DT);
        assert!(state.fits(&c));
    }
}
