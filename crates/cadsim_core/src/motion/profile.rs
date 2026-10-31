//! 動作の速度の付け方 (加減速のパターン) と、電子カム表
//!
//! どのパターンも「時間の進み具合 x (0〜1)」を「位置の進み具合 s (0〜1)」に変える関数。
//!
//! - 台形加減速: 決めた割合の時間で一定の加速度で加速し、一定速度で進み、同じ時間で減速する
//! - サイクロイド: 加速度が 0 から始まり 0 で終わる (s = x - sin 2πx / 2π)。始めと終わりの衝撃が小さい
//! - 電子カム: 表の点 (時間 %, 位置 %) を、行き過ぎない 3 次の曲線 (PCHIP) でつなぐ

/// 電子カム表 (始点 (0, 0) と終点 (1, 1) は表に書かなくても通る)
#[derive(Clone, Debug, PartialEq)]
pub struct CamTable {
    pub id: u32,
    pub name: String,
    /// (時間の進み具合, 位置の進み具合)。どちらも 0〜1。
    pub points: Vec<[f64; 2]>,
}

impl CamTable {
    /// 始点と終点を足して時間の順に並べた点
    pub fn knots(&self) -> Vec<[f64; 2]> {
        let mut knots: Vec<[f64; 2]> = self
            .points
            .iter()
            .filter(|p| p[0] > 1e-9 && p[0] < 1.0 - 1e-9)
            .copied()
            .collect();
        knots.push([0.0, 0.0]);
        knots.push([1.0, 1.0]);
        knots.sort_by(|a, b| a[0].total_cmp(&b[0]));
        knots.dedup_by(|b, a| (b[0] - a[0]).abs() < 1e-9);
        knots
    }

    /// 時間の進み具合 `x` での位置の進み具合
    pub fn eval(&self, x: f64) -> f64 {
        pchip(&self.knots(), x.clamp(0.0, 1.0))
    }
}

/// 点の列を PCHIP (区分 3 次エルミート、傾きを Fritsch–Carlson で決める) でつないだ曲線の値。
/// 点と点の間で値が行き過ぎない (単調な区間は単調のまま)。
fn pchip(knots: &[[f64; 2]], x: f64) -> f64 {
    let n = knots.len();
    if n == 0 {
        return x;
    }
    if n == 1 {
        return knots[0][1];
    }
    let h: Vec<f64> = knots.windows(2).map(|w| w[1][0] - w[0][0]).collect();
    let delta: Vec<f64> = knots
        .windows(2)
        .zip(&h)
        .map(|(w, h)| (w[1][1] - w[0][1]) / h)
        .collect();
    // 各点での傾き
    let mut slope = vec![0.0; n];
    slope[0] = delta[0];
    slope[n - 1] = delta[n - 2];
    for i in 1..n - 1 {
        if delta[i - 1] * delta[i] <= 0.0 {
            slope[i] = 0.0;
        } else {
            // 重み付き調和平均
            let w1 = 2.0 * h[i] + h[i - 1];
            let w2 = h[i] + 2.0 * h[i - 1];
            slope[i] = (w1 + w2) / (w1 / delta[i - 1] + w2 / delta[i]);
        }
    }
    let i = knots.windows(2).position(|w| x <= w[1][0]).unwrap_or(n - 2);
    let t = (x - knots[i][0]) / h[i];
    let (t2, t3) = (t * t, t * t * t);
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    h00 * knots[i][1] + h10 * h[i] * slope[i] + h01 * knots[i + 1][1] + h11 * h[i] * slope[i + 1]
}

/// 台形加減速。`accel` は加速 (と減速) にかける時間の割合 (0〜0.5)。
pub fn trapezoid(x: f64, accel: f64) -> f64 {
    let r = accel.clamp(1e-6, 0.5);
    let v = 1.0 / (1.0 - r);
    if x < r {
        0.5 * v / r * x * x
    } else if x <= 1.0 - r {
        0.5 * v * r + v * (x - r)
    } else {
        1.0 - 0.5 * v / r * (1.0 - x) * (1.0 - x)
    }
}

/// サイクロイド曲線
pub fn cycloid(x: f64) -> f64 {
    use std::f64::consts::TAU;
    x - (TAU * x).sin() / TAU
}

/// 位置の曲線 `f` の、時間の進み具合 `x` での速度 (傾き)。数値微分。
pub fn velocity(f: impl Fn(f64) -> f64, x: f64) -> f64 {
    let h = 1e-4;
    let (a, b) = ((x - h).max(0.0), (x + h).min(1.0));
    (f(b) - f(a)) / (b - a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trapezoid_has_constant_speed_in_the_middle() {
        let r = 0.25;
        assert!(trapezoid(0.0, r).abs() < 1e-12);
        assert!((trapezoid(1.0, r) - 1.0).abs() < 1e-12);
        assert!((trapezoid(0.5, r) - 0.5).abs() < 1e-12);
        // 等速部分の速さは 1 / (1 - r)
        let v = velocity(|x| trapezoid(x, r), 0.5);
        assert!((v - 1.0 / 0.75).abs() < 1e-6);
        // 加速の途中と終わりで値と傾きがつながる
        assert!((velocity(|x| trapezoid(x, r), r) - v).abs() < 1e-3);
        assert!(velocity(|x| trapezoid(x, r), 0.0).abs() < 1e-3);
    }

    #[test]
    fn cycloid_starts_and_ends_at_rest() {
        assert!(cycloid(0.0).abs() < 1e-12);
        assert!((cycloid(1.0) - 1.0).abs() < 1e-12);
        assert!(velocity(cycloid, 0.0).abs() < 1e-3);
        assert!(velocity(cycloid, 1.0).abs() < 1e-3);
        assert!((velocity(cycloid, 0.5) - 2.0).abs() < 1e-6);
    }

    #[test]
    fn cam_passes_through_its_points_without_overshoot() {
        let cam = CamTable {
            id: 1,
            name: "停留付き".into(),
            // 前半で 80% まで進み、後半はその位置で少し止まってから残りを進む
            points: vec![[0.4, 0.8], [0.7, 0.8]],
        };
        assert!(cam.eval(0.0).abs() < 1e-12);
        assert!((cam.eval(0.4) - 0.8).abs() < 1e-12);
        assert!((cam.eval(0.55) - 0.8).abs() < 1e-12, "停留区間は動かない");
        assert!((cam.eval(1.0) - 1.0).abs() < 1e-12);
        // どこでも 0〜1 に収まり、時間とともに減らない
        let mut last = 0.0;
        for k in 0..=100 {
            let s = cam.eval(k as f64 / 100.0);
            assert!((-1e-12..=1.0 + 1e-12).contains(&s));
            assert!(s >= last - 1e-12);
            last = s;
        }
    }

    #[test]
    fn empty_cam_is_linear() {
        let cam = CamTable {
            id: 1,
            name: String::new(),
            points: Vec::new(),
        };
        assert!((cam.eval(0.3) - 0.3).abs() < 1e-12);
    }
}
