//! 作業者と AGV の見た目 (3D の形) を、単純な形 (箱・球・円柱・カプセル) の組み合わせで作る
//!
//! - 作業者: 頭・ヘルメット・首・胴 (安全ベスト)・腰・腕・手・脚・靴。歩いている間は手足を振り、
//!   作業中は腕を前に出して手元を動かす。
//! - AGV: 車体・天板・黄色の帯・前後のバンパー・駆動輪とキャスター・ヘッドライト・テールランプ・
//!   レーザーセンサー・状態ランプ。荷を積んでいれば天板に箱を載せる。
//!
//! 形の数と並びは毎回同じで、毎フレーム変わるのは位置・向き・色・見える/見えないだけ。
//! 表示側は最初に形ごとのメッシュを作り、あとは姿勢を写すだけでよい。
//!
//! 座標は床が y = 0 の Y 上向き、床の点 (x, y) は 3D の (x, 0, y)。長さは mm、時間は秒。
//! Bevy に依存しない純粋な計算だけを置く。

use std::f64::consts::{FRAC_PI_2, PI, TAU};

use glam::{DQuat, DVec2, DVec3};

use super::operator_plan::Activity;

/// 形の種類と寸法 (mm)。中心が原点、円柱とカプセルの軸は Y。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Box {
        size: DVec3,
    },
    Sphere {
        radius: f64,
    },
    Cylinder {
        radius: f64,
        height: f64,
    },
    /// `length` は両端の半球の中心の間の長さ
    Capsule {
        radius: f64,
        length: f64,
    },
}

/// 塗り分け。色そのものは表示側が決める。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Paint {
    Skin,
    Hair,
    Helmet,
    /// 安全ベスト (選んでいる作業者は `Highlight`)
    Vest,
    Highlight,
    /// ベストの反射帯
    Reflective,
    Shirt,
    Pants,
    Shoe,
    Eye,
    AgvBody,
    AgvDeck,
    /// 車体の黄色の帯 (止まっているときは `StripeAlert`)
    Stripe,
    StripeAlert,
    Bumper,
    Tire,
    Hub,
    Headlight,
    Taillight,
    Sensor,
    LampGreen,
    LampAmber,
    LampRed,
    LampOff,
    Cargo,
}

/// 形 1つと、その置き方
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FigurePart {
    pub shape: Shape,
    pub paint: Paint,
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
    pub visible: bool,
}

impl FigurePart {
    fn new(shape: Shape, paint: Paint, translation: DVec3) -> Self {
        Self {
            shape,
            paint,
            translation,
            rotation: DQuat::IDENTITY,
            scale: DVec3::ONE,
            visible: true,
        }
    }

    fn rotated(mut self, rotation: DQuat) -> Self {
        self.rotation = rotation;
        self
    }

    fn scaled(mut self, scale: DVec3) -> Self {
        self.scale = scale;
        self
    }

    fn shown(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    /// 点 a から点 b へ伸びるカプセル (腕や脚)。長さは骨の長さ `length` に固定し、
    /// 計算の誤差で形 (メッシュ) が毎フレーム変わらないようにする
    fn limb(a: DVec3, b: DVec3, length: f64, radius: f64, paint: Paint) -> Self {
        let rotation = match (b - a).try_normalize() {
            Some(d) => DQuat::from_rotation_arc(DVec3::Y, d),
            None => DQuat::IDENTITY,
        };
        Self::new(Shape::Capsule { radius, length }, paint, (a + b) / 2.0).rotated(rotation)
    }

    /// 床の位置と向きに置き直す
    fn placed(mut self, origin: DVec3, rotation: DQuat) -> Self {
        self.translation = origin + rotation * self.translation;
        self.rotation = rotation * self.rotation;
        self
    }
}

/// 床の向き (x, y) を、ローカルの +X をその向きに回す回転にする
pub fn heading_rotation(heading: DVec2) -> DQuat {
    let h = heading.normalize_or(DVec2::X);
    DQuat::from_rotation_y((-h.y).atan2(h.x))
}

fn floor(p: DVec2) -> DVec3 {
    DVec3::new(p.x, 0.0, p.y)
}

// ---------------------------------------------------------------------------
// 作業者

/// 作業者の姿勢を決めるもの
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PersonPose {
    /// 床の位置
    pub position: DVec2,
    /// 向いている方向 (床の向き)
    pub heading: DVec2,
    /// いまの作業
    pub activity: Activity,
    /// 歩いた距離 (mm)。手足を振る位相に使う
    pub walked: f64,
    /// 時刻 (秒)。作業中の手の動きに使う
    pub time: f64,
    /// 選んでいる作業者 (ベストの色を変える)
    pub selected: bool,
}

/// 1歩 × 2 (左右で 1周) の長さ (mm)
pub const STRIDE: f64 = 1300.0;

const HIP_Y: f64 = 880.0;
const HIP_HALF_WIDTH: f64 = 95.0;
const THIGH: f64 = 420.0;
const SHIN: f64 = 400.0;
const SHOULDER_Y: f64 = 1390.0;
const SHOULDER_HALF_WIDTH: f64 = 195.0;
const UPPER_ARM: f64 = 290.0;
const FOREARM: f64 = 260.0;
const HEAD_Y: f64 = 1580.0;
const HEAD_RADIUS: f64 = 105.0;

/// 脚 1本の付け根の角度 (前へ振るとき正) とひざの曲げ (正)
fn leg_angles(phase: f64, amount: f64) -> (f64, f64) {
    let swing = (phase * TAU).sin();
    // 足を前へ戻している間 (前へ振り出す前半) にひざを曲げる
    let knee = (phase * TAU).cos().max(0.0);
    (
        swing * 25f64.to_radians() * amount,
        knee * 40f64.to_radians() * amount,
    )
}

/// 関節から、角度 (前へ正) の方向へ長さだけ下へ伸ばした先
fn swing_down(from: DVec3, angle: f64, length: f64) -> DVec3 {
    from + DVec3::new(angle.sin(), -angle.cos(), 0.0) * length
}

/// 作業者の形。ローカルでは前が +X、左右が ±Z。
pub fn person_parts(pose: &PersonPose) -> Vec<FigurePart> {
    let walking = matches!(pose.activity, Activity::Walk);
    let working = matches!(pose.activity, Activity::Work);
    let amount = if walking { 1.0 } else { 0.0 };
    let phase = (pose.walked / STRIDE).rem_euclid(1.0);
    let vest = if pose.selected {
        Paint::Highlight
    } else {
        Paint::Vest
    };

    // 脚: 左は位相そのまま、右は半周ずらす。腰の高さで関節を求めてから、
    // 低い方の足が床に着くように体ごと下げる (歩くと自然に上下する)
    let legs = [(1.0, 0.0), (-1.0, 0.5)].map(|(side, offset)| {
        let (thigh, knee_bend) = leg_angles(phase + offset, amount);
        let hip = DVec3::new(0.0, HIP_Y, side * HIP_HALF_WIDTH);
        let knee = swing_down(hip, thigh, THIGH);
        let ankle = swing_down(knee, thigh - knee_bend, SHIN);
        (hip, knee, ankle)
    });
    let rest_ankle = HIP_Y - THIGH - SHIN;
    let lowest = legs.iter().map(|l| l.2.y).fold(f64::INFINITY, f64::min);
    let up = DVec3::Y * (rest_ankle - lowest).min(0.0);

    let mut parts = Vec::with_capacity(32);
    for (hip, knee, ankle) in legs {
        let (hip, knee, ankle) = (hip + up, knee + up, ankle + up);
        parts.push(FigurePart::limb(hip, knee, THIGH, 62.0, Paint::Pants));
        parts.push(FigurePart::limb(knee, ankle, SHIN, 52.0, Paint::Pants));
        // 靴は足首より少し前
        let foot = DVec3::new(ankle.x + 45.0, (ankle.y - 25.0).max(35.0), ankle.z);
        parts.push(FigurePart::new(
            Shape::Box {
                size: DVec3::new(250.0, 70.0, 105.0),
            },
            Paint::Shoe,
            foot,
        ));
    }

    // 腰・胴・ベストの反射帯
    parts.push(
        FigurePart::new(
            Shape::Capsule {
                radius: 100.0,
                length: 180.0,
            },
            Paint::Pants,
            DVec3::new(0.0, HIP_Y + 30.0, 0.0) + up,
        )
        .rotated(DQuat::from_rotation_x(FRAC_PI_2))
        .scaled(DVec3::new(1.0, 1.0, 0.9)),
    );
    let lean = if walking {
        DQuat::from_rotation_z(-4f64.to_radians())
    } else {
        DQuat::IDENTITY
    };
    let torso_center = DVec3::new(0.0, 1170.0, 0.0) + up;
    parts.push(
        FigurePart::new(
            Shape::Capsule {
                radius: 150.0,
                length: 260.0,
            },
            vest,
            torso_center,
        )
        .rotated(lean)
        .scaled(DVec3::new(0.78, 1.0, 1.25)),
    );
    for y in [-60.0, 40.0] {
        parts.push(
            FigurePart::new(
                Shape::Cylinder {
                    radius: 152.0,
                    height: 26.0,
                },
                Paint::Reflective,
                torso_center + lean * DVec3::new(0.0, y, 0.0),
            )
            .rotated(lean)
            .scaled(DVec3::new(0.79, 1.0, 1.26)),
        );
    }

    // 腕と手: 歩くときは脚と逆に振る。作業中は前に出して手元を動かす
    for (side, offset) in [(1.0, 0.5), (-1.0, 0.0)] {
        let shoulder = DVec3::new(0.0, SHOULDER_Y, side * SHOULDER_HALF_WIDTH) + up;
        let (upper, elbow_bend) = if working {
            let wiggle = ((pose.time * 1.6 + offset) * TAU).sin() * 10f64.to_radians();
            (55f64.to_radians() + wiggle, 50f64.to_radians() - wiggle)
        } else {
            let swing = ((phase + offset) * TAU).sin() * 22f64.to_radians() * amount;
            (swing, 12f64.to_radians() + swing.max(0.0) * 0.8)
        };
        // 腕は体の外へ少し開く
        let out = DVec3::Z * side * 25.0;
        let elbow = swing_down(shoulder, upper, UPPER_ARM) + out;
        let wrist = swing_down(elbow, upper + elbow_bend, FOREARM) + out * 0.3;
        parts.push(FigurePart::limb(
            shoulder,
            elbow,
            UPPER_ARM,
            48.0,
            Paint::Shirt,
        ));
        parts.push(FigurePart::limb(elbow, wrist, FOREARM, 42.0, Paint::Skin));
        parts.push(FigurePart::new(
            Shape::Sphere { radius: 48.0 },
            Paint::Skin,
            swing_down(wrist, upper + elbow_bend, 30.0),
        ));
    }

    // 首・頭・目・ヘルメット
    parts.push(FigurePart::new(
        Shape::Cylinder {
            radius: 50.0,
            height: 110.0,
        },
        Paint::Skin,
        DVec3::new(0.0, 1460.0, 0.0) + up,
    ));
    let head = DVec3::new(0.0, HEAD_Y, 0.0) + up;
    parts.push(
        FigurePart::new(
            Shape::Sphere {
                radius: HEAD_RADIUS,
            },
            Paint::Skin,
            head,
        )
        .scaled(DVec3::new(1.0, 1.1, 0.95)),
    );
    for side in [1.0, -1.0] {
        parts.push(FigurePart::new(
            Shape::Sphere { radius: 13.0 },
            Paint::Eye,
            head + DVec3::new(HEAD_RADIUS * 0.9, 15.0, side * 38.0),
        ));
    }
    parts.push(
        FigurePart::new(
            Shape::Sphere { radius: 122.0 },
            Paint::Helmet,
            head + DVec3::new(-5.0, 45.0, 0.0),
        )
        .scaled(DVec3::new(1.0, 0.72, 1.0)),
    );
    // ヘルメットのつば (前に出す)
    parts.push(
        FigurePart::new(
            Shape::Box {
                size: DVec3::new(80.0, 14.0, 180.0),
            },
            Paint::Helmet,
            head + DVec3::new(HEAD_RADIUS + 15.0, 40.0, 0.0),
        )
        .rotated(DQuat::from_rotation_z(-8f64.to_radians())),
    );
    // 後ろ髪 (ヘルメットの下からのぞく)
    parts.push(
        FigurePart::new(
            Shape::Sphere { radius: 95.0 },
            Paint::Hair,
            head + DVec3::new(-25.0, -5.0, 0.0),
        )
        .scaled(DVec3::new(0.9, 0.8, 1.05)),
    );

    let origin = floor(pose.position);
    let rotation = heading_rotation(pose.heading);
    parts
        .into_iter()
        .map(|p| p.placed(origin, rotation))
        .collect()
}

// ---------------------------------------------------------------------------
// AGV

/// AGV の状態 (ランプの色と帯の色を決める)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgvStatus {
    /// 仕事がなく止まっている
    Idle,
    /// 走っている
    Moving,
    /// 積み込み・荷降ろし中
    Handling,
    /// 交差点や前の車を待って止まっている
    Blocked,
}

/// AGV の姿勢を決めるもの
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AgvPose {
    pub position: DVec2,
    pub heading: DVec2,
    /// 車体の長さ・幅・高さ (mm)
    pub length: f64,
    pub width: f64,
    pub height: f64,
    pub status: AgvStatus,
    /// 荷を積んでいる
    pub loaded: bool,
    /// 走った距離 (mm)。車輪の回転に使う
    pub distance: f64,
    /// 時刻 (秒)。ランプの点滅に使う
    pub time: f64,
}

const WHEEL_RADIUS: f64 = 75.0;
const CASTER_RADIUS: f64 = 38.0;
const CLEARANCE: f64 = 25.0;

/// 状態ランプの色。作業中は 1 秒に 2 回点滅する
pub fn agv_lamp(status: AgvStatus, time: f64) -> Paint {
    match status {
        AgvStatus::Idle => Paint::LampOff,
        AgvStatus::Moving => Paint::LampGreen,
        AgvStatus::Blocked => Paint::LampRed,
        AgvStatus::Handling => {
            if (time * 2.0).rem_euclid(1.0) < 0.5 {
                Paint::LampAmber
            } else {
                Paint::LampOff
            }
        }
    }
}

/// AGV の形。ローカルでは前が +X、左右が ±Z。
pub fn agv_parts(pose: &AgvPose) -> Vec<FigurePart> {
    let l = pose.length.max(200.0);
    let w = pose.width.max(200.0);
    let h = pose.height.max(120.0);
    let (hl, hw) = (l / 2.0, w / 2.0);
    let body_bottom = CLEARANCE + 20.0;
    let deck = 35.0;
    let body_top = h - deck;
    let body_h = body_top - body_bottom;
    let mut parts = Vec::with_capacity(24);

    // 車体・天板・帯
    parts.push(FigurePart::new(
        Shape::Box {
            size: DVec3::new(l, body_h, w),
        },
        Paint::AgvBody,
        DVec3::new(0.0, body_bottom + body_h / 2.0, 0.0),
    ));
    parts.push(FigurePart::new(
        Shape::Box {
            size: DVec3::new(l - 30.0, deck, w - 30.0),
        },
        Paint::AgvDeck,
        DVec3::new(0.0, h - deck / 2.0, 0.0),
    ));
    let stripe = if pose.status == AgvStatus::Blocked {
        Paint::StripeAlert
    } else {
        Paint::Stripe
    };
    parts.push(FigurePart::new(
        Shape::Box {
            size: DVec3::new(l + 4.0, body_h * 0.18, w + 4.0),
        },
        stripe,
        DVec3::new(0.0, body_bottom + body_h * 0.62, 0.0),
    ));

    // 前後のバンパー
    for dir in [1.0, -1.0] {
        parts.push(FigurePart::new(
            Shape::Box {
                size: DVec3::new(40.0, body_h * 0.35, w - 60.0),
            },
            Paint::Bumper,
            DVec3::new(dir * (hl + 20.0), body_bottom + body_h * 0.2, 0.0),
        ));
    }

    // 駆動輪 (左右の中央) とハブ。ハブの板が回って見える
    let spin = pose.distance / WHEEL_RADIUS;
    let axle = DQuat::from_rotation_x(FRAC_PI_2);
    for side in [1.0, -1.0] {
        let center = DVec3::new(0.0, WHEEL_RADIUS, side * (hw - 20.0));
        parts.push(
            FigurePart::new(
                Shape::Cylinder {
                    radius: WHEEL_RADIUS,
                    height: 45.0,
                },
                Paint::Tire,
                center,
            )
            .rotated(axle),
        );
        parts.push(
            FigurePart::new(
                Shape::Box {
                    size: DVec3::new(WHEEL_RADIUS * 1.5, 22.0, 6.0),
                },
                Paint::Hub,
                center + DVec3::Z * side * 25.0,
            )
            .rotated(DQuat::from_rotation_z(-spin)),
        );
    }
    // 四隅のキャスター
    for x in [1.0, -1.0] {
        for z in [1.0, -1.0] {
            parts.push(
                FigurePart::new(
                    Shape::Cylinder {
                        radius: CASTER_RADIUS,
                        height: 30.0,
                    },
                    Paint::Tire,
                    DVec3::new(x * (hl - 110.0), CASTER_RADIUS, z * (hw - 90.0)),
                )
                .rotated(axle),
            );
        }
    }

    // ヘッドライト・テールランプ
    let light_y = body_bottom + body_h * 0.42;
    for side in [1.0, -1.0] {
        parts.push(FigurePart::new(
            Shape::Box {
                size: DVec3::new(12.0, 40.0, 110.0),
            },
            Paint::Headlight,
            DVec3::new(hl + 4.0, light_y, side * (hw - 110.0)),
        ));
        parts.push(FigurePart::new(
            Shape::Box {
                size: DVec3::new(12.0, 40.0, 90.0),
            },
            Paint::Taillight,
            DVec3::new(-hl - 4.0, light_y, side * (hw - 100.0)),
        ));
    }

    // 前の上のレーザーセンサーと、後ろの角の状態ランプ
    parts.push(FigurePart::new(
        Shape::Cylinder {
            radius: 55.0,
            height: 70.0,
        },
        Paint::Sensor,
        DVec3::new(hl - 90.0, h + 35.0, 0.0),
    ));
    parts.push(FigurePart::new(
        Shape::Cylinder {
            radius: 38.0,
            height: 90.0,
        },
        agv_lamp(pose.status, pose.time),
        DVec3::new(-hl + 80.0, h + 45.0, hw - 80.0),
    ));

    // 積んでいる荷
    let cargo = 300.0;
    parts.push(
        FigurePart::new(
            Shape::Box {
                size: DVec3::new(l * 0.55, cargo, w * 0.7),
            },
            Paint::Cargo,
            DVec3::new(-l * 0.05, h + cargo / 2.0, 0.0),
        )
        .shown(pose.loaded),
    );

    let origin = floor(pose.position);
    let rotation = heading_rotation(pose.heading);
    parts
        .into_iter()
        .map(|p| p.placed(origin, rotation))
        .collect()
}

/// 形の上端 (表示の確認やテストに使う)。
pub fn top_of(parts: &[FigurePart]) -> f64 {
    parts
        .iter()
        .filter(|p| p.visible)
        .map(|p| p.translation.y + vertical_half_extent(p))
        .fold(f64::NEG_INFINITY, f64::max)
}

/// 形の下端 (表示の確認やテストに使う)
pub fn bottom_of(parts: &[FigurePart]) -> f64 {
    parts
        .iter()
        .filter(|p| p.visible)
        .map(|p| p.translation.y - vertical_half_extent(p))
        .fold(f64::INFINITY, f64::min)
}

/// 形の Y 方向の半分の高さ (回転と拡大を入れた正確な値)
fn vertical_half_extent(p: &FigurePart) -> f64 {
    // ローカルの軸ごとの Y 成分から求める
    let axis = |v: DVec3| (p.rotation * v).y.abs();
    match p.shape {
        Shape::Box { size } => {
            let s = size * p.scale / 2.0;
            axis(DVec3::X) * s.x + axis(DVec3::Y) * s.y + axis(DVec3::Z) * s.z
        }
        Shape::Sphere { radius } => {
            let s = p.scale * radius;
            // 楕円体の Y 方向の半径
            let r = p.rotation.inverse() * DVec3::Y;
            ((r.x * s.x).powi(2) + (r.y * s.y).powi(2) + (r.z * s.z).powi(2)).sqrt()
        }
        Shape::Cylinder { radius, height } => {
            let s = p.scale;
            let ay = axis(DVec3::Y);
            ay * height / 2.0 * s.y + (1.0 - ay * ay).max(0.0).sqrt() * radius * s.x.max(s.z)
        }
        Shape::Capsule { radius, length } => {
            let s = p.scale;
            axis(DVec3::Y) * length / 2.0 * s.y + radius * s.max_element()
        }
    }
}

/// 歩く向きを変えるのにかける時間 (秒)
const TURN_TIME: f64 = 0.35;

/// 角度 a から b へ、近い回りで t (0〜1) だけ進めた向き
fn turn_toward(a: DVec2, b: DVec2, t: f64) -> DVec2 {
    let (aa, ab) = (a.y.atan2(a.x), b.y.atan2(b.x));
    let mut d = (ab - aa).rem_euclid(TAU);
    if d > PI {
        d -= TAU;
    }
    let angle = aa + d * t.clamp(0.0, 1.0);
    DVec2::new(angle.cos(), angle.sin())
}

impl super::operator_plan::OperatorPlan {
    /// 区間 i の歩く向き (歩かない区間や長さ 0 の歩行は None)
    fn walk_direction(&self, i: usize) -> Option<DVec2> {
        let s = self.segments.get(i)?;
        (s.activity == Activity::Walk)
            .then(|| (s.to - s.from).try_normalize())
            .flatten()
    }

    /// 区間 i より前で、最後に歩いた向き (サイクルの最後から回り込んで探す)
    fn previous_direction(&self, i: usize) -> Option<DVec2> {
        let n = self.segments.len();
        (1..=n).find_map(|k| self.walk_direction((i + n - k) % n))
    }

    /// 時刻 t (サイクルを繰り返す) で向いている方向。歩いていれば進む向き、
    /// 止まっていれば最後に歩いた向き。向きを変えるときは少しずつ回る。
    pub fn heading_at(&self, t: f64) -> DVec2 {
        let local = self.cycle_time_of(t);
        let Some(i) = self
            .segments
            .iter()
            .position(|s| local < s.end)
            .or_else(|| (!self.segments.is_empty()).then(|| self.segments.len() - 1))
        else {
            return DVec2::X;
        };
        let before = self.previous_direction(i);
        match self.walk_direction(i) {
            Some(dir) => {
                let s = &self.segments[i];
                let k = (local - s.start) / TURN_TIME;
                match before {
                    Some(prev) if k < 1.0 => turn_toward(prev, dir, k),
                    _ => dir,
                }
            }
            None => before.unwrap_or(DVec2::X),
        }
    }

    /// 時刻 t までに歩いた距離 (前のサイクルの分も足す)
    pub fn walked_at(&self, t: f64) -> f64 {
        let cycles = if self.cycle_time > 0.0 {
            (t / self.cycle_time).floor().max(0.0)
        } else {
            0.0
        };
        let local = self.cycle_time_of(t);
        let mut walked = cycles * self.walk_distance;
        for s in self
            .segments
            .iter()
            .filter(|s| s.activity == Activity::Walk)
        {
            if local >= s.end {
                walked += s.from.distance(s.to);
            } else if local > s.start && s.end > s.start {
                walked += s.from.distance(s.to) * (local - s.start) / (s.end - s.start);
            }
        }
        walked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::floor::operator_plan::{Operator, OperatorPlan, OperatorTask};

    fn person(activity: Activity, walked: f64) -> PersonPose {
        PersonPose {
            position: DVec2::new(1000.0, 2000.0),
            heading: DVec2::X,
            activity,
            walked,
            time: 0.0,
            selected: false,
        }
    }

    fn agv(status: AgvStatus, loaded: bool) -> AgvPose {
        AgvPose {
            position: DVec2::ZERO,
            heading: DVec2::Y,
            length: 1200.0,
            width: 800.0,
            height: 400.0,
            status,
            loaded,
            distance: 0.0,
            time: 0.0,
        }
    }

    #[test]
    fn heading_rotation_turns_local_x_to_the_floor_direction() {
        for h in [
            DVec2::X,
            DVec2::Y,
            DVec2::new(-1.0, 0.0),
            DVec2::new(1.0, -1.0),
        ] {
            let v = heading_rotation(h) * DVec3::X;
            let h = h.normalize();
            assert!(
                (v - DVec3::new(h.x, 0.0, h.y)).length() < 1e-9,
                "{h:?} -> {v:?}"
            );
        }
    }

    #[test]
    fn person_stands_on_the_floor_and_is_about_1_7_m_tall() {
        for activity in [Activity::Walk, Activity::Work, Activity::Wait] {
            for walked in [0.0, 300.0, 650.0, 1000.0] {
                let parts = person_parts(&person(activity, walked));
                let top = top_of(&parts);
                let bottom = bottom_of(&parts);
                assert!((1600.0..1760.0).contains(&top), "{activity:?} top {top}");
                assert!(
                    (-1.0..30.0).contains(&bottom),
                    "{activity:?} bottom {bottom}"
                );
            }
        }
    }

    #[test]
    fn person_keeps_the_same_parts_every_frame() {
        let a = person_parts(&person(Activity::Walk, 0.0));
        let b = person_parts(&person(Activity::Work, 400.0));
        assert_eq!(a.len(), b.len());
        assert!(a.iter().zip(&b).all(|(a, b)| a.shape == b.shape));
    }

    #[test]
    fn person_is_placed_at_its_floor_position() {
        let parts = person_parts(&person(Activity::Wait, 0.0));
        let head = parts
            .iter()
            .find(|p| {
                p.shape
                    == Shape::Sphere {
                        radius: HEAD_RADIUS,
                    }
            })
            .unwrap();
        assert!((head.translation.x - 1000.0).abs() < 1.0);
        assert!((head.translation.z - 2000.0).abs() < 1.0);
    }

    #[test]
    fn walking_swings_the_legs_and_standing_does_not() {
        // 位相 1/4 で左脚は前、右脚は後ろ
        let feet = |pose: &PersonPose| -> Vec<f64> {
            person_parts(pose)
                .iter()
                .filter(|p| p.paint == Paint::Shoe)
                .map(|p| p.translation.x - pose.position.x)
                .collect()
        };
        let walk = feet(&person(Activity::Walk, STRIDE / 4.0));
        assert!(walk[0] > walk[1] + 200.0, "{walk:?}");
        let stand = feet(&person(Activity::Wait, STRIDE / 4.0));
        assert!((stand[0] - stand[1]).abs() < 1e-9, "{stand:?}");
    }

    #[test]
    fn working_reaches_the_hands_forward() {
        let hands = |activity| -> f64 {
            person_parts(&person(activity, 0.0))
                .iter()
                .filter(|p| p.shape == Shape::Sphere { radius: 48.0 })
                .map(|p| p.translation.x - 1000.0)
                .sum::<f64>()
                / 2.0
        };
        assert!(hands(Activity::Work) > hands(Activity::Wait) + 250.0);
    }

    #[test]
    fn selected_person_wears_the_highlight_vest() {
        let mut pose = person(Activity::Wait, 0.0);
        pose.selected = true;
        let parts = person_parts(&pose);
        assert!(parts.iter().any(|p| p.paint == Paint::Highlight));
        assert!(!parts.iter().any(|p| p.paint == Paint::Vest));
    }

    #[test]
    fn agv_fits_its_size_and_shows_cargo_only_when_loaded() {
        let parts = agv_parts(&agv(AgvStatus::Moving, false));
        assert!(bottom_of(&parts).abs() < 1e-6);
        // センサーとランプの分だけ天板より高い
        let top = top_of(&parts);
        assert!((400.0..520.0).contains(&top), "{top}");
        let loaded = agv_parts(&agv(AgvStatus::Moving, true));
        assert_eq!(parts.len(), loaded.len());
        assert!(top_of(&loaded) >= 400.0 + 300.0 - 1e-6);
        // 前 (+Y の床方向 = 3D の +Z) にセンサーがある
        let sensor = parts.iter().find(|p| p.paint == Paint::Sensor).unwrap();
        assert!(sensor.translation.z > 400.0);
    }

    #[test]
    fn agv_lamp_shows_the_status() {
        assert_eq!(agv_lamp(AgvStatus::Moving, 0.0), Paint::LampGreen);
        assert_eq!(agv_lamp(AgvStatus::Blocked, 0.0), Paint::LampRed);
        assert_eq!(agv_lamp(AgvStatus::Idle, 0.0), Paint::LampOff);
        assert_eq!(agv_lamp(AgvStatus::Handling, 0.1), Paint::LampAmber);
        assert_eq!(agv_lamp(AgvStatus::Handling, 0.3), Paint::LampOff);
        let blocked = agv_parts(&agv(AgvStatus::Blocked, false));
        assert!(blocked.iter().any(|p| p.paint == Paint::StripeAlert));
    }

    fn plan() -> OperatorPlan {
        let mut op = Operator::new(1, [0.0, 0.0]);
        op.walk_speed = 1000.0;
        op.tasks = vec![
            OperatorTask::Walk { to: [3000.0, 0.0] },
            OperatorTask::Wait { duration: 2.0 },
            OperatorTask::Walk {
                to: [3000.0, 4000.0],
            },
        ];
        // 3 s 歩く → 2 s 待つ → 4 s 歩く → 5 s で戻る = 14 s
        OperatorPlan::new(&op)
    }

    #[test]
    fn heading_follows_the_walk_and_keeps_it_while_standing() {
        let plan = plan();
        assert!((plan.heading_at(1.5) - DVec2::X).length() < 1e-9);
        // 待っている間は最後に歩いた向き
        assert!((plan.heading_at(4.0) - DVec2::X).length() < 1e-9);
        // 曲がった直後は途中の向き、少し後には次の向き
        let turning = plan.heading_at(5.1);
        assert!(turning.x > 0.1 && turning.y > 0.1, "{turning:?}");
        assert!((plan.heading_at(6.0) - DVec2::Y).length() < 1e-9);
        // 戻りの歩行は開始位置へ向かう
        let back = plan.heading_at(12.0);
        assert!((back - DVec2::new(-0.6, -0.8)).length() < 1e-9, "{back:?}");
    }

    #[test]
    fn walked_distance_accumulates_over_cycles() {
        let plan = plan();
        assert!((plan.walked_at(1.0) - 1000.0).abs() < 1e-9);
        assert!((plan.walked_at(4.0) - 3000.0).abs() < 1e-9);
        assert!((plan.walked_at(7.0) - 5000.0).abs() < 1e-9);
        assert!((plan.walked_at(14.0 + 1.0) - 13000.0).abs() < 1e-9);
    }

    #[test]
    fn empty_plan_faces_x() {
        let plan = OperatorPlan::default();
        assert_eq!(plan.heading_at(3.0), DVec2::X);
        assert_eq!(plan.walked_at(3.0), 0.0);
    }
}
