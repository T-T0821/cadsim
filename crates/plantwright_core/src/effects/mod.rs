//! 状態のエフェクト: 設備の状態を光・煙・火花で 3D ビューに出す
//!
//! - 詰まり: 赤く脈打つ光と、床に広がる波紋
//! - 故障: 赤い光と波紋に、黒い煙
//! - 人待ち: ゆっくり点滅する黄色い光
//! - 溶接中: ちらつく青白い光と火花 (名前に「溶接」「weld」を含む工程が加工しているとき)
//! - 安全停止: 止めた安全機器の上で速く点滅する赤い光と波紋
//! - 干渉: ぶつかっている所に小さな火花
//!
//! ここでは「どこに・どの見た目を出すか」と粒の動きだけを決める。描くのは UI 層 (`src/effects`)。
//! 座標はモデルの単位 (mm) で、Y が上。

pub mod particles;

use glam::DVec3;

use crate::flow::model::{ElementId, FlowModel};
use crate::flow::sim::State;
use crate::math::Aabb;
use crate::safety::device::SafetyDevice;

pub use particles::{Particle, ParticleField, ParticleKind};

/// エフェクトの設定 (表示メニューで切り替える)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectSettings {
    /// すべてのエフェクトをまとめてオン・オフする
    pub enabled: bool,
    /// 光と波紋
    pub glow: bool,
    /// 煙
    pub smoke: bool,
    /// 火花
    pub sparks: bool,
    /// 一度に出す粒 (煙と火花) の上限。重くならないように抑える。
    pub max_particles: usize,
}

impl Default for EffectSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            glow: true,
            smoke: true,
            sparks: true,
            max_particles: 800,
        }
    }
}

/// エフェクトを出すきっかけ
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Cue {
    /// 詰まり (加工は終わったが送り先がいっぱい)
    Jam,
    /// 故障で停止中
    Fault,
    /// 人待ち
    NoOperator,
    /// 溶接の工程が加工中
    Welding,
    /// 安全機器が止めた
    SafetyStop,
    /// 部品どうしがぶつかっている
    Collision,
}

impl Cue {
    pub const ALL: [Cue; 6] = [
        Cue::Jam,
        Cue::Fault,
        Cue::NoOperator,
        Cue::Welding,
        Cue::SafetyStop,
        Cue::Collision,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Cue::Jam => "詰まり",
            Cue::Fault => "故障",
            Cue::NoOperator => "人待ち",
            Cue::Welding => "溶接",
            Cue::SafetyStop => "安全停止",
            Cue::Collision => "干渉",
        }
    }

    /// このきっかけの見た目。設定で切ったものは出さない。
    pub fn look(self, settings: &EffectSettings) -> Look {
        let mut look = match self {
            Cue::Jam => Look {
                glow: Some(Glow::pulse([1.0, 0.12, 0.08], 1.2, 0.35)),
                ripple: true,
                ..Look::NONE
            },
            Cue::Fault => Look {
                glow: Some(Glow::pulse([1.0, 0.05, 0.02], 0.8, 0.25)),
                ripple: true,
                smoke_per_sec: 9.0,
                ..Look::NONE
            },
            Cue::NoOperator => Look {
                glow: Some(Glow::pulse([1.0, 0.8, 0.1], 2.0, 0.15)),
                ..Look::NONE
            },
            Cue::Welding => Look {
                glow: Some(Glow {
                    rgb: [0.55, 0.75, 1.0],
                    period: 0.0,
                    min: 0.3,
                    flicker: true,
                }),
                sparks_per_sec: 140.0,
                ..Look::NONE
            },
            Cue::SafetyStop => Look {
                glow: Some(Glow::pulse([1.0, 0.1, 0.05], 0.5, 0.0)),
                ripple: true,
                ..Look::NONE
            },
            Cue::Collision => Look {
                sparks_per_sec: 50.0,
                ..Look::NONE
            },
        };
        if !settings.enabled {
            return Look::NONE;
        }
        if !settings.glow {
            look.glow = None;
            look.ripple = false;
        }
        if !settings.smoke {
            look.smoke_per_sec = 0.0;
        }
        if !settings.sparks {
            look.sparks_per_sec = 0.0;
        }
        look
    }
}

/// きっかけ 1 つの見た目
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub glow: Option<Glow>,
    /// 床に広がる波紋
    pub ripple: bool,
    /// 1 秒に出す煙の粒
    pub smoke_per_sec: f64,
    /// 1 秒に出す火花の粒
    pub sparks_per_sec: f64,
}

impl Look {
    pub const NONE: Look = Look {
        glow: None,
        ripple: false,
        smoke_per_sec: 0.0,
        sparks_per_sec: 0.0,
    };

    pub fn is_empty(&self) -> bool {
        self.glow.is_none()
            && !self.ripple
            && self.smoke_per_sec <= 0.0
            && self.sparks_per_sec <= 0.0
    }
}

/// 設備を包む光
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    /// 色 (sRGB, 0〜1)
    pub rgb: [f32; 3],
    /// 脈打つ周期 (秒)。0 なら脈打たない
    pub period: f64,
    /// いちばん暗いときの強さ (0〜1)
    pub min: f32,
    /// 溶接のアークのように不規則にちらつく
    pub flicker: bool,
}

impl Glow {
    fn pulse(rgb: [f32; 3], period: f64, min: f32) -> Self {
        Self {
            rgb,
            period,
            min,
            flicker: false,
        }
    }

    /// 時刻 `t` (秒) での強さ (0〜1)
    pub fn strength(&self, t: f64) -> f32 {
        let wave = if self.flicker {
            // 1/30 秒ごとに変わる、決まった並びの乱れ
            let step = (t * 30.0).floor() as i64 as u64;
            unit_hash(step)
        } else if self.period > 0.0 {
            0.5 - 0.5 * (t / self.period * std::f64::consts::TAU).cos()
        } else {
            1.0
        };
        self.min + (1.0 - self.min) * wave as f32
    }
}

/// 床に広がる波紋の、時刻 `t` での広がり (0〜1)。1 周期 1.5 秒。
pub fn ripple_phase(t: f64) -> f64 {
    (t / RIPPLE_PERIOD).rem_euclid(1.0)
}

const RIPPLE_PERIOD: f64 = 1.5;

/// エフェクトを出す場所の見分け方。同じ場所なら粒の出し方を引き継ぐ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum EmitterKey {
    /// 工程シミュレーションの要素
    Element(u32),
    /// 安全機器
    Device(u32),
    /// 干渉している部品の組
    Collision(u32, u32),
}

/// エフェクトを出す場所 1 つ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub key: EmitterKey,
    pub cue: Cue,
    /// 光の中心
    pub center: DVec3,
    /// 光の半径。煙や火花の大きさもこれに合わせる
    pub radius: f64,
    /// 煙と火花が出る所 (設備の上面の中心)
    pub top: DVec3,
    /// 波紋を描く床の高さ
    pub floor_y: f64,
}

/// 光の半径の下限 (mm)。小さな部品でも見えるように。
const MIN_RADIUS: f64 = 120.0;

impl Emitter {
    /// 設備の外接箱から作る
    pub fn around(key: EmitterKey, cue: Cue, bounds: Aabb) -> Self {
        let center = bounds.center();
        let size = bounds.size();
        let radius = (size.length() / 2.0).max(MIN_RADIUS);
        Self {
            key,
            cue,
            center,
            radius,
            top: DVec3::new(center.x, bounds.max.y, center.z),
            floor_y: bounds.min.y.min(0.0),
        }
    }

    /// 1 点 (ぶつかっている所など) から作る
    pub fn at(key: EmitterKey, cue: Cue, point: DVec3, radius: f64) -> Self {
        Self {
            key,
            cue,
            center: point,
            radius: radius.max(MIN_RADIUS / 2.0),
            top: point,
            floor_y: point.y.min(0.0),
        }
    }

    /// 粒の大きさ・速さの倍率 (半径 400 mm の設備を 1 とする)
    pub fn scale(&self) -> f64 {
        (self.radius / 400.0).clamp(0.4, 3.0)
    }
}

/// 工程の状態から出すエフェクト。`welding` は溶接の工程か ([`is_welding`])。
pub fn element_cue(state: State, welding: bool) -> Option<Cue> {
    match state {
        State::Blocked => Some(Cue::Jam),
        State::Down => Some(Cue::Fault),
        State::NoOperator => Some(Cue::NoOperator),
        State::Working if welding => Some(Cue::Welding),
        State::Working | State::Waiting => None,
    }
}

/// 名前から溶接の工程かを見分ける (「溶接」「ウェルド」「weld」を含む)
pub fn is_welding(name: &str) -> bool {
    let lower = name.to_lowercase();
    ["溶接", "ウェルド", "ｳｪﾙﾄﾞ", "weld"]
        .iter()
        .any(|w| lower.contains(w))
}

/// 干渉している所の目印: 交線の端点の平均と、その広がり (半径)
pub fn collision_point(curves: &[[DVec3; 2]]) -> Option<(DVec3, f64)> {
    if curves.is_empty() {
        return None;
    }
    let points = curves.iter().flatten();
    let n = (curves.len() * 2) as f64;
    let center = points.clone().copied().sum::<DVec3>() / n;
    let radius = points.map(|p| p.distance(center)).fold(0.0, f64::max);
    Some((center, radius))
}

/// 工程シミュレーションの要素ごとのエフェクトを集める。
/// `state_of` は今の状態、`bounds` は 3D の部品の外接箱 (見えていない部品は None を返す)。
pub fn flow_emitters(
    flow: &FlowModel,
    state_of: impl Fn(ElementId) -> Option<State>,
    bounds: impl Fn(u32) -> Option<Aabb>,
) -> Vec<Emitter> {
    flow.elements
        .iter()
        .filter_map(|e| {
            let part = e.part?;
            let cue = element_cue(state_of(e.id)?, is_welding(&e.name))?;
            Some(Emitter::around(
                EmitterKey::Element(e.id),
                cue,
                bounds(part)?,
            ))
        })
        .collect()
}

/// 安全停止の回転灯を出す場所: 止めた機器の上
pub fn safety_beacon(device: &SafetyDevice) -> Emitter {
    let top = device.position[1] + device.size[1];
    let point = DVec3::new(device.position[0], top + BEACON_LIFT, device.position[2]);
    Emitter::at(
        EmitterKey::Device(device.id),
        Cue::SafetyStop,
        point,
        BEACON_RADIUS,
    )
}

/// 回転灯を機器の上端からどれだけ上に出すか (mm)
const BEACON_LIFT: f64 = 200.0;
const BEACON_RADIUS: f64 = 180.0;

/// 0〜1 の決まった値 (同じ入力なら同じ値)
fn unit_hash(x: u64) -> f64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb {
            min: DVec3::from(min),
            max: DVec3::from(max),
        }
    }

    #[test]
    fn states_map_to_cues() {
        assert_eq!(element_cue(State::Blocked, false), Some(Cue::Jam));
        assert_eq!(element_cue(State::Down, true), Some(Cue::Fault));
        assert_eq!(element_cue(State::NoOperator, false), Some(Cue::NoOperator));
        assert_eq!(element_cue(State::Working, true), Some(Cue::Welding));
        assert_eq!(element_cue(State::Working, false), None);
        assert_eq!(element_cue(State::Waiting, true), None);
    }

    #[test]
    fn welding_is_found_by_name() {
        assert!(is_welding("スポット溶接 1"));
        assert!(is_welding("Robot Welding Cell"));
        assert!(is_welding("ウェルド工程"));
        assert!(!is_welding("組立"));
        assert!(!is_welding("検査"));
    }

    #[test]
    fn settings_switch_parts_of_the_look() {
        let all = EffectSettings::default();
        let fault = Cue::Fault.look(&all);
        assert!(fault.glow.is_some() && fault.ripple && fault.smoke_per_sec > 0.0);
        assert!(Cue::Welding.look(&all).sparks_per_sec > 0.0);

        let off = EffectSettings {
            enabled: false,
            ..all
        };
        for cue in Cue::ALL {
            assert!(cue.look(&off).is_empty(), "{cue:?}");
        }

        let no_smoke = EffectSettings {
            smoke: false,
            ..all
        };
        let fault = Cue::Fault.look(&no_smoke);
        assert_eq!(fault.smoke_per_sec, 0.0);
        assert!(fault.glow.is_some());

        let no_glow = EffectSettings { glow: false, ..all };
        let jam = Cue::Jam.look(&no_glow);
        assert!(jam.is_empty());
        let no_sparks = EffectSettings {
            sparks: false,
            ..all
        };
        assert_eq!(Cue::Welding.look(&no_sparks).sparks_per_sec, 0.0);
    }

    #[test]
    fn glow_pulses_between_min_and_one() {
        let glow = Cue::Jam.look(&EffectSettings::default()).glow.unwrap();
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..240 {
            let s = glow.strength(i as f64 / 60.0);
            lo = lo.min(s);
            hi = hi.max(s);
        }
        assert!((lo - glow.min).abs() < 0.02, "{lo}");
        assert!(hi > 0.98, "{hi}");
        // ちらつきも範囲の中で、同じ時刻なら同じ値
        let arc = Cue::Welding.look(&EffectSettings::default()).glow.unwrap();
        for i in 0..100 {
            let t = i as f64 * 0.013;
            let s = arc.strength(t);
            assert!((arc.min..=1.0).contains(&s));
            assert_eq!(s, arc.strength(t));
        }
    }

    #[test]
    fn ripple_repeats() {
        assert!(ripple_phase(0.0).abs() < 1e-9);
        assert!((ripple_phase(0.75) - 0.5).abs() < 1e-9);
        assert!((ripple_phase(1.5 + 0.3) - ripple_phase(0.3)).abs() < 1e-9);
    }

    #[test]
    fn emitter_wraps_the_part() {
        let e = Emitter::around(
            EmitterKey::Element(1),
            Cue::Jam,
            bounds([0.0, 0.0, 0.0], [1000.0, 800.0, 600.0]),
        );
        assert_eq!(e.center, DVec3::new(500.0, 400.0, 300.0));
        assert_eq!(e.top, DVec3::new(500.0, 800.0, 300.0));
        assert!(e.radius > 600.0);
        assert_eq!(e.floor_y, 0.0);
        // 小さな部品でも光が見える大きさにする
        let tiny = Emitter::around(
            EmitterKey::Element(2),
            Cue::Jam,
            bounds([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
        );
        assert_eq!(tiny.radius, MIN_RADIUS);
        assert!(tiny.scale() >= 0.4);
    }

    #[test]
    fn flow_emitters_follow_states_and_parts() {
        use crate::flow::model::KindTag;
        let mut flow = FlowModel::default();
        let mut station = |name: &str, part: Option<u32>| {
            let id = flow.add(KindTag::Process);
            let e = flow.get_mut(id).unwrap();
            e.name = name.into();
            e.part = part;
            id
        };
        let weld = station("溶接", Some(10));
        let press = station("プレス", Some(20));
        station("検査", None);
        let box_ = bounds([0.0, 0.0, 0.0], [500.0, 500.0, 500.0]);
        let found = flow_emitters(
            &flow,
            |id| {
                Some(if id == press {
                    State::Blocked
                } else {
                    State::Working
                })
            },
            |_| Some(box_),
        );
        let cues: Vec<_> = found.iter().map(|e| (e.key, e.cue)).collect();
        assert_eq!(
            cues,
            vec![
                (EmitterKey::Element(weld), Cue::Welding),
                (EmitterKey::Element(press), Cue::Jam),
            ]
        );
        // 部品が見えていなければ出さない
        assert!(flow_emitters(&flow, |_| Some(State::Down), |_| None).is_empty());
    }

    #[test]
    fn beacon_sits_on_top_of_the_device() {
        use crate::safety::device::DeviceKind;
        let d = SafetyDevice::new(3, DeviceKind::LightCurtain, [1000.0, 2000.0]);
        let e = safety_beacon(&d);
        assert_eq!(e.key, EmitterKey::Device(3));
        assert_eq!(e.cue, Cue::SafetyStop);
        assert_eq!(e.center.x, 1000.0);
        assert_eq!(e.center.z, 2000.0);
        assert!(e.center.y > d.position[1] + d.size[1]);
    }

    #[test]
    fn collision_point_is_the_middle_of_the_curves() {
        assert_eq!(collision_point(&[]), None);
        let (c, r) = collision_point(&[
            [DVec3::new(0.0, 0.0, 0.0), DVec3::new(10.0, 0.0, 0.0)],
            [DVec3::new(0.0, 10.0, 0.0), DVec3::new(10.0, 10.0, 0.0)],
        ])
        .unwrap();
        assert_eq!(c, DVec3::new(5.0, 5.0, 0.0));
        assert!((r - 50f64.sqrt()).abs() < 1e-9);
    }
}
