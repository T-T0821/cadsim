//! 一人称で工場を歩く (歩行モード) の計算
//!
//! 作業者の目の高さにカメラを置き、床の上を歩いて回る。障害物は「床の上の向きの付いた長方形 +
//! 高さの範囲」([`Zone`]。部品の外接箱や安全柵) で、体 (縦の円柱) が入り込まないように押し戻すので、
//! 壁や設備に沿って滑るように歩ける。
//! 低い段 (パレットや床板) は乗り越え、頭より上のもの (梁・天井クレーン) はくぐる。
//!
//! 歩きながら次のことを測る ([`survey`])。
//! - 通路幅: 左右の障害物までの距離の和 (労働安全衛生規則の 80 cm と比べる)
//! - 前方の障害物までの距離
//! - 肩の位置から手の届く範囲にある部品
//!
//! 座標は Bevy と同じ Y 上向き、床は Y = 0。長さはモデルの単位 (mm など) で、
//! 体の寸法は [`WalkBody::for_unit`] で mm からモデルの単位に直す。

use std::f64::consts::FRAC_PI_2;

use glam::{DVec2, DVec3};

use crate::floor::geom::{Rect2, Zone};
use crate::math::Aabb;

/// 床の高さ (モデルの単位)
pub const FLOOR_Y: f64 = 0.0;

/// 歩く人の体の寸法と歩く速さ (モデルの単位)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WalkBody {
    /// 床から目までの高さ
    pub eye_height: f64,
    /// 床から頭のてっぺんまで (これより上のものはくぐれる)
    pub head_height: f64,
    /// 床から肩までの高さ (手の届く範囲の中心)
    pub shoulder_height: f64,
    /// 体の半径 (上から見た円)
    pub radius: f64,
    /// 乗り越えられる段の高さ
    pub step_height: f64,
    /// 手の届く距離 (肩から)
    pub reach: f64,
    /// 歩く速さ (単位/秒)
    pub walk_speed: f64,
    /// Shift で走るときの倍率
    pub run_factor: f64,
    /// 通路幅の目安。これより狭いと注意を出す
    pub min_aisle: f64,
}

impl WalkBody {
    /// 日本人の成人の平均くらいの体を、モデルの単位 (1 単位 = `unit_mm` mm) で作る
    pub fn for_unit(unit_mm: f64) -> Self {
        let mm = |v: f64| v / unit_mm.max(1e-9);
        Self {
            eye_height: mm(1600.0),
            head_height: mm(1750.0),
            shoulder_height: mm(1400.0),
            radius: mm(250.0),
            step_height: mm(200.0),
            reach: mm(650.0),
            walk_speed: mm(1300.0),
            run_factor: 3.0,
            // 労働安全衛生規則 第 543 条: 機械の間の通路は 80 cm 以上
            min_aisle: mm(800.0),
        }
    }
}

/// 歩いている人の位置と向き
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Walker {
    /// 足元 (体の中心の真下)
    pub feet: DVec3,
    /// 左右の向き (ラジアン)。0 で -Z を向き、正で左へ回る (カメラのヨーと同じ)
    pub yaw: f64,
    /// 上下の向き (ラジアン)。正で下を見る (カメラのピッチと同じ)
    pub pitch: f64,
}

impl Walker {
    pub fn eye(&self, body: &WalkBody) -> DVec3 {
        self.feet + DVec3::Y * body.eye_height
    }

    /// 床に沿った前の向き (単位ベクトル)
    pub fn forward(&self) -> DVec3 {
        DVec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos())
    }

    /// 床に沿った右の向き (単位ベクトル)
    pub fn right(&self) -> DVec3 {
        DVec3::new(self.yaw.cos(), 0.0, -self.yaw.sin())
    }
}

/// 1 フレーム分の操作
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WalkInput {
    /// 前へ +1、後ろへ -1
    pub forward: f64,
    /// 右へ +1、左へ -1
    pub right: f64,
    /// 走る
    pub run: bool,
    /// 左へ回る角度 (ラジアン)
    pub turn: f64,
    /// 下を向く角度 (ラジアン)
    pub look_down: f64,
}

/// 上下に見られる角度の限度 (真上・真下の少し手前)
const PITCH_LIMIT: f64 = FRAC_PI_2 - 0.01;
/// 1 回の押し戻しで動かす部分の長さ (体の半径に対する割合)。速く動いても壁を抜けないようにする
const SUBSTEP_RATIO: f64 = 0.5;
/// 押し戻しの繰り返し回数 (角に挟まれたとき)
const RESOLVE_ITERATIONS: usize = 4;

/// 軸に平行な外接箱を障害物の形にする
pub fn zone_from_aabb(aabb: &Aabb) -> Zone {
    Zone {
        footprint: Rect2::from_aabb(aabb),
        y_min: aabb.min.y,
        y_max: aabb.max.y,
    }
}

/// 体の高さの範囲 (足元 + 段の高さ 〜 頭) にかかる障害物か
fn blocks(zone: &Zone, feet_y: f64, body: &WalkBody) -> bool {
    zone.y_max > feet_y + body.step_height && zone.y_min < feet_y + body.head_height
}

fn flat(v: DVec3) -> DVec2 {
    DVec2::new(v.x, v.z)
}

/// 床の座標を長方形のローカル座標 (中心が原点、軸がローカル X / Z) にする
fn to_local(rect: &Rect2, p: DVec2) -> DVec2 {
    let [ax, az] = rect.axes();
    let d = p - rect.center;
    DVec2::new(d.dot(ax), d.dot(az))
}

/// ローカル座標の向きを床の座標の向きにする
fn dir_to_world(rect: &Rect2, v: DVec2) -> DVec2 {
    let [ax, az] = rect.axes();
    ax * v.x + az * v.y
}

/// 上から見た長方形の中で `p` にいちばん近い点 (ローカル座標で返す) と、`p` のローカル座標
fn closest_local(rect: &Rect2, p: DVec2) -> (DVec2, DVec2) {
    let local = to_local(rect, p);
    (local.clamp(-rect.half, rect.half), local)
}

/// `p` が上から見た長方形の中 (縁を含む) にあるか
fn inside(rect: &Rect2, p: DVec2) -> bool {
    let local = to_local(rect, p).abs();
    local.x <= rect.half.x + 1e-9 && local.y <= rect.half.y + 1e-9
}

/// 体 (中心 `p`) が長方形にめり込んでいれば、外へ出す移動 (床の座標) を返す
fn push_out(rect: &Rect2, p: DVec2, radius: f64) -> Option<DVec2> {
    let (closest, local) = closest_local(rect, p);
    let d = local - closest;
    let dist = d.length();
    if dist >= radius {
        return None;
    }
    if dist > 1e-12 {
        return Some(dir_to_world(rect, d / dist * (radius - dist)));
    }
    // 中心が長方形の中: いちばん近い辺から外へ出す
    let h = rect.half;
    let exits = [
        (local.x + h.x + radius, DVec2::NEG_X),
        (h.x - local.x + radius, DVec2::X),
        (local.y + h.y + radius, DVec2::NEG_Y),
        (h.y - local.y + radius, DVec2::Y),
    ];
    let (depth, dir) = exits
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("4 辺ある");
    Some(dir_to_world(rect, dir * depth))
}

/// その場所の足元の高さ (床か、乗り越えられる低い箱の上)
fn ground_at(p: DVec2, feet_y: f64, body: &WalkBody, obstacles: &[Zone]) -> f64 {
    obstacles
        .iter()
        .filter(|z| z.y_max <= feet_y + body.step_height && z.y_max > FLOOR_Y)
        .filter(|z| inside(&z.footprint, p))
        .map(|z| z.y_max)
        .fold(FLOOR_Y, f64::max)
}

/// 体が障害物にめり込んでいるか
pub fn collides(feet: DVec3, body: &WalkBody, obstacles: &[Zone]) -> bool {
    let p = flat(feet);
    obstacles.iter().any(|z| {
        let (closest, local) = closest_local(&z.footprint, p);
        // 押し出した直後のちょうど触れている状態は、丸め誤差があってもめり込みに数えない
        blocks(z, feet.y, body)
            && closest.distance_squared(local) < (body.radius * (1.0 - 1e-6)).powi(2)
    })
}

/// 障害物から体を押し出す (角に挟まれたときのために何回か繰り返す)
fn resolve(mut p: DVec2, feet_y: f64, body: &WalkBody, obstacles: &[Zone]) -> DVec2 {
    for _ in 0..RESOLVE_ITERATIONS {
        let mut moved = false;
        for b in obstacles.iter().filter(|b| blocks(b, feet_y, body)) {
            if let Some(push) = push_out(&b.footprint, p, body.radius) {
                p += push;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    p
}

/// 1 フレーム歩く。壁にぶつかったら壁に沿って滑る
pub fn step(walker: &mut Walker, input: &WalkInput, body: &WalkBody, obstacles: &[Zone], dt: f64) {
    walker.yaw += input.turn;
    walker.pitch = (walker.pitch + input.look_down).clamp(-PITCH_LIMIT, PITCH_LIMIT);

    let wish = walker.forward() * input.forward + walker.right() * input.right;
    let len = wish.length();
    if len < 1e-9 || dt <= 0.0 {
        return;
    }
    let speed = body.walk_speed * if input.run { body.run_factor } else { 1.0 };
    // 斜めに歩いても速くならないようにする
    let travel = flat(wish / len.max(1.0)) * speed * dt;
    let substep = (body.radius * SUBSTEP_RATIO).max(1e-9);
    let n = ((travel.length() / substep).ceil() as usize).clamp(1, 64);
    let mut p = flat(walker.feet);
    let mut y = walker.feet.y;
    for _ in 0..n {
        p = resolve(p + travel / n as f64, y, body, obstacles);
        y = ground_at(p, y, body, obstacles);
    }
    walker.feet = DVec3::new(p.x, y, p.y);
}

/// `start` の近くで体が入る場所を探して立つ。見つからなければ `start` のまま
pub fn place(start: DVec3, yaw: f64, body: &WalkBody, obstacles: &[Zone]) -> Walker {
    let at = |p: DVec2| {
        let y = ground_at(p, FLOOR_Y, body, obstacles);
        DVec3::new(p.x, y, p.y)
    };
    let origin = flat(start);
    let mut feet = at(origin);
    // 中心から輪を広げながら探す (体の半径ずつ、輪 1 周を体の幅ずつ)
    'search: for ring in 0..200 {
        let r = ring as f64 * body.radius;
        let count = ((std::f64::consts::TAU * r) / body.radius).ceil().max(1.0) as usize;
        for i in 0..count {
            let a = i as f64 / count as f64 * std::f64::consts::TAU;
            let candidate = at(origin + DVec2::new(a.cos(), a.sin()) * r);
            if !collides(candidate, body, obstacles) {
                feet = candidate;
                break 'search;
            }
        }
    }
    Walker {
        feet,
        yaw,
        pitch: 0.0,
    }
}

/// 上から見た半直線 (`origin` から `dir` 向き) が長方形に当たる距離
fn ray_rect(rect: &Rect2, origin: DVec2, dir: DVec2) -> Option<f64> {
    let origin = to_local(rect, origin);
    let [ax, az] = rect.axes();
    let dir = DVec2::new(dir.dot(ax), dir.dot(az));
    let (min, max) = (-rect.half, rect.half);
    let mut t_near = f64::NEG_INFINITY;
    let mut t_far = f64::INFINITY;
    for (o, d, lo, hi) in [
        (origin.x, dir.x, min.x, max.x),
        (origin.y, dir.y, min.y, max.y),
    ] {
        if d.abs() < 1e-12 {
            if o < lo || o > hi {
                return None;
            }
        } else {
            let (a, b) = ((lo - o) / d, (hi - o) / d);
            t_near = t_near.max(a.min(b));
            t_far = t_far.min(a.max(b));
        }
    }
    (t_near <= t_far && t_far >= 0.0).then_some(t_near.max(0.0))
}

/// 体の中心から `dir` の向きで、いちばん近い障害物までの距離 (`max_range` より遠ければ None)
fn distance_to_wall(
    walker: &Walker,
    dir: DVec3,
    body: &WalkBody,
    obstacles: &[Zone],
    max_range: f64,
) -> Option<f64> {
    let origin = flat(walker.feet);
    let dir = flat(dir).normalize_or_zero();
    obstacles
        .iter()
        .filter(|b| blocks(b, walker.feet.y, body))
        .filter_map(|b| ray_rect(&b.footprint, origin, dir))
        .filter(|&t| t <= max_range)
        .min_by(f64::total_cmp)
}

/// 歩いている場所で測ったこと
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Survey {
    /// 左の障害物までの距離 (体の中心から)
    pub left: Option<f64>,
    /// 右の障害物までの距離
    pub right: Option<f64>,
    /// 前の障害物までの距離
    pub ahead: Option<f64>,
    /// 通路幅 (左右の障害物の間)。どちらかが測れる範囲になければ None
    pub aisle_width: Option<f64>,
    /// 通路幅が目安 ([`WalkBody::min_aisle`]) より狭い
    pub aisle_narrow: bool,
    /// 手の届く範囲にある障害物の番号 (近い順)
    pub in_reach: Vec<usize>,
}

/// 左右・前の障害物までの距離と、手の届く部品を測る。`max_range` より遠い障害物は数えない
pub fn survey(walker: &Walker, body: &WalkBody, obstacles: &[Zone], max_range: f64) -> Survey {
    let wall = |dir| distance_to_wall(walker, dir, body, obstacles, max_range);
    let left = wall(-walker.right());
    let right = wall(walker.right());
    let ahead = wall(walker.forward());
    let aisle_width = left.zip(right).map(|(l, r)| l + r);

    let shoulder = walker.feet + DVec3::Y * body.shoulder_height;
    let mut in_reach: Vec<(usize, f64)> = obstacles
        .iter()
        .enumerate()
        // 足元の床板やパレットは「手が届く部品」に数えない
        .filter(|(_, z)| z.y_max > walker.feet.y + body.step_height)
        .map(|(i, z)| {
            let (closest, local) = closest_local(&z.footprint, flat(shoulder));
            let dy = shoulder.y - shoulder.y.clamp(z.y_min, z.y_max);
            (i, (closest - local).extend(dy).length())
        })
        .filter(|&(_, d)| d <= body.reach)
        .collect();
    in_reach.sort_by(|a, b| a.1.total_cmp(&b.1));

    Survey {
        left,
        right,
        ahead,
        aisle_width,
        aisle_narrow: aisle_width.is_some_and(|w| w < body.min_aisle),
        in_reach: in_reach.into_iter().map(|(i, _)| i).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> WalkBody {
        WalkBody::for_unit(1.0)
    }

    fn boxed(min: [f64; 3], max: [f64; 3]) -> Zone {
        zone_from_aabb(&Aabb {
            min: DVec3::from(min),
            max: DVec3::from(max),
        })
    }

    fn walker_at(x: f64, z: f64) -> Walker {
        Walker {
            feet: DVec3::new(x, 0.0, z),
            yaw: 0.0,
            pitch: 0.0,
        }
    }

    fn forward() -> WalkInput {
        WalkInput {
            forward: 1.0,
            ..Default::default()
        }
    }

    #[test]
    fn body_scales_with_unit() {
        let m = WalkBody::for_unit(1000.0);
        assert!((m.eye_height - 1.6).abs() < 1e-12);
        assert!((m.min_aisle - 0.8).abs() < 1e-12);
        assert!((body().eye_height - 1600.0).abs() < 1e-12);
    }

    #[test]
    fn walks_forward_along_minus_z() {
        let mut w = walker_at(0.0, 0.0);
        step(&mut w, &forward(), &body(), &[], 1.0);
        assert!((w.feet.z + 1300.0).abs() < 1e-6, "{:?}", w.feet);
        assert!(w.feet.x.abs() < 1e-6);
        assert!((w.eye(&body()).y - 1600.0).abs() < 1e-9);
    }

    #[test]
    fn diagonal_is_not_faster() {
        let mut w = walker_at(0.0, 0.0);
        let input = WalkInput {
            forward: 1.0,
            right: 1.0,
            ..Default::default()
        };
        step(&mut w, &input, &body(), &[], 1.0);
        assert!((w.feet.length() - 1300.0).abs() < 1e-6);
    }

    #[test]
    fn running_is_faster() {
        let mut w = walker_at(0.0, 0.0);
        let input = WalkInput {
            forward: 1.0,
            run: true,
            ..Default::default()
        };
        step(&mut w, &input, &body(), &[], 1.0);
        assert!((w.feet.z + 3900.0).abs() < 1e-6);
    }

    #[test]
    fn stops_at_wall_and_does_not_tunnel() {
        // 前 1 m に厚さ 10 mm の壁。走って 1 秒で 3.9 m 進もうとしても抜けない
        let wall = boxed([-5000.0, 0.0, -1010.0], [5000.0, 3000.0, -1000.0]);
        let mut w = walker_at(0.0, 0.0);
        let input = WalkInput {
            forward: 1.0,
            run: true,
            ..Default::default()
        };
        step(&mut w, &input, &body(), &[wall], 1.0);
        assert!(
            (w.feet.z - (-1000.0 + 250.0)).abs() < 1e-6,
            "壁の手前で体の半径だけ離れて止まる: {:?}",
            w.feet
        );
    }

    #[test]
    fn slides_along_wall() {
        // 斜めに壁へ向かうと、壁に沿った分だけ進む
        let wall = boxed([-5000.0, 0.0, -1010.0], [5000.0, 3000.0, -1000.0]);
        let mut w = walker_at(0.0, -700.0);
        let input = WalkInput {
            forward: 1.0,
            right: 1.0,
            ..Default::default()
        };
        step(&mut w, &input, &body(), &[wall], 1.0);
        assert!((w.feet.z + 750.0).abs() < 1e-6);
        assert!(w.feet.x > 800.0, "右へ滑って進む: {:?}", w.feet);
    }

    #[test]
    fn steps_over_low_box_and_ducks_under_beam() {
        let pallet = boxed([-500.0, 0.0, -1500.0], [500.0, 150.0, -1000.0]);
        let beam = boxed([-5000.0, 2000.0, -3000.0], [5000.0, 2300.0, -2800.0]);
        let mut w = walker_at(0.0, 0.0);
        // パレットの上で立ち止まる
        step(&mut w, &forward(), &body(), &[pallet, beam], 1.0);
        assert!(
            (w.feet.y - 150.0).abs() < 1e-9,
            "パレットに乗る: {:?}",
            w.feet
        );
        // 降りて梁の下をくぐる
        step(&mut w, &forward(), &body(), &[pallet, beam], 2.0);
        assert!(w.feet.z < -3000.0, "{:?}", w.feet);
        assert_eq!(w.feet.y, 0.0);
    }

    #[test]
    fn rotated_fence_blocks_along_its_length() {
        // 長さ 4 m・厚さ 50 mm の柵を 45° 回して、前 1 m の所に置く
        let fence = Zone {
            footprint: Rect2 {
                center: DVec2::new(0.0, -1000.0),
                half: DVec2::new(2000.0, 25.0),
                yaw: std::f64::consts::FRAC_PI_4,
            },
            y_min: 0.0,
            y_max: 1800.0,
        };
        let mut w = walker_at(0.0, 0.0);
        step(&mut w, &forward(), &body(), &[fence], 2.0);
        assert!(!collides(w.feet, &body(), &[fence]));
        // 斜めの柵に沿って滑るので、まっすぐ前より横にずれる
        assert!(w.feet.x.abs() > 100.0, "{:?}", w.feet);
        let s = survey(&walker_at(0.0, 0.0), &body(), &[fence], 10_000.0);
        let ahead = s.ahead.unwrap();
        // 中心線は z = -1000 で、厚みの半分 25 mm を 45° で見ると √2 倍手前に来る
        assert!(
            (ahead - (1000.0 - 25.0 * 2f64.sqrt())).abs() < 1e-6,
            "{ahead}"
        );
    }

    #[test]
    fn tall_box_blocks() {
        let machine = boxed([-500.0, 0.0, -1500.0], [500.0, 1200.0, -1000.0]);
        let mut w = walker_at(0.0, 0.0);
        step(&mut w, &forward(), &body(), &[machine], 2.0);
        assert!((w.feet.z + 750.0).abs() < 1e-6);
    }

    #[test]
    fn turning_and_looking() {
        let mut w = walker_at(0.0, 0.0);
        let input = WalkInput {
            turn: FRAC_PI_2,
            look_down: 10.0,
            ..Default::default()
        };
        step(&mut w, &input, &body(), &[], 0.016);
        // 左へ 90° 回ると -X を向く
        assert!((w.forward() - DVec3::NEG_X).length() < 1e-9);
        assert!((w.right() - DVec3::NEG_Z).length() < 1e-9);
        assert!(w.pitch < FRAC_PI_2, "真下を越えない");
    }

    #[test]
    fn place_moves_out_of_obstacle() {
        let machine = boxed([-1000.0, 0.0, -1000.0], [1000.0, 1500.0, 1000.0]);
        let w = place(DVec3::ZERO, 0.0, &body(), &[machine]);
        assert!(!collides(w.feet, &body(), &[machine]));
        assert!(
            w.feet.truncate().length() < 1600.0,
            "近くに立つ: {:?}",
            w.feet
        );
        // 空いていればその場所に立つ
        let w = place(DVec3::new(3000.0, 500.0, 0.0), 0.3, &body(), &[machine]);
        assert_eq!(w.feet, DVec3::new(3000.0, 0.0, 0.0));
        assert_eq!(w.yaw, 0.3);
    }

    #[test]
    fn measures_aisle_between_two_machines() {
        // X = -400 と X = 500 の間の通路 (幅 900 mm) を -Z 向きに歩いている
        let left = boxed([-2000.0, 0.0, -3000.0], [-400.0, 1500.0, 3000.0]);
        let right = boxed([500.0, 0.0, -3000.0], [2000.0, 1500.0, 3000.0]);
        let end = boxed([-2000.0, 0.0, -5000.0], [2000.0, 1500.0, -4000.0]);
        let s = survey(&walker_at(0.0, 0.0), &body(), &[left, right, end], 10_000.0);
        assert_eq!(s.left, Some(400.0));
        assert_eq!(s.right, Some(500.0));
        assert_eq!(s.aisle_width, Some(900.0));
        assert!(!s.aisle_narrow);
        assert_eq!(s.ahead, Some(4000.0));
        assert_eq!(s.in_reach, vec![0, 1], "左右とも手が届く (近い順)");

        // 幅 700 mm の通路は狭い
        let right = boxed([300.0, 0.0, -3000.0], [2000.0, 1500.0, 3000.0]);
        let s = survey(&walker_at(0.0, 0.0), &body(), &[left, right], 10_000.0);
        assert_eq!(s.aisle_width, Some(700.0));
        assert!(s.aisle_narrow);
        assert_eq!(s.in_reach, vec![1, 0], "近い順に手が届く");

        // 広い通路の真ん中では届かない
        let right = boxed([1000.0, 0.0, -3000.0], [2000.0, 1500.0, 3000.0]);
        let left = boxed([-2000.0, 0.0, -3000.0], [-1000.0, 1500.0, 3000.0]);
        let s = survey(&walker_at(0.0, 0.0), &body(), &[left, right], 10_000.0);
        assert!(s.in_reach.is_empty());
    }

    #[test]
    fn survey_ignores_far_and_floor_items() {
        let far = boxed([5000.0, 0.0, -100.0], [6000.0, 1500.0, 100.0]);
        let floor_plate = boxed([-3000.0, -20.0, -3000.0], [3000.0, 0.0, 3000.0]);
        let s = survey(&walker_at(0.0, 0.0), &body(), &[far, floor_plate], 3000.0);
        assert_eq!(s.right, None);
        assert_eq!(s.aisle_width, None);
        assert!(s.in_reach.is_empty());
    }
}
