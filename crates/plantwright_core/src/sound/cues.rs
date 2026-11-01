//! いまの設備の状態から、鳴らす音を決める
//!
//! - アクチュエータが動いている間: 直動軸はサーボの音、回転テーブル (ロボットの関節も) はモーターの音
//! - エアシリンダが動き始めたとき: 排気音を 1 回
//! - コンベアが回っている間: ベルトの音
//! - 停止の警報が鳴っている間: 警報 (位置を持たず、正面で鳴らす)
//! - 干渉が起きたとき: 「ピピッ」を 1 回
//!
//! 動いているかどうかは、タイミングチャートの時刻ではなくアクチュエータの値の変化で見る。
//! こうすると、チャートの再生でも PLC 連携でもスクラブでも同じように鳴る。

use std::collections::{BTreeSet, HashMap};

use glam::DVec3;

use super::settings::SoundSettings;
use super::spatial::{Listener, Placement, place};
use super::synth::SoundKind;
use crate::motion::ActuatorKind;

/// 鳴らし続ける音の見分け方。同じキーの音は、毎フレーム大きさと向きだけを変える。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VoiceKey {
    Actuator(u32),
    Conveyor(u32),
    Alarm,
}

/// 鳴らし続ける音 1 つ
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voice {
    pub key: VoiceKey,
    pub kind: SoundKind,
    /// 音量 (設定・距離・種類の大きさをすべて掛けたもの) と左右
    pub placement: Placement,
}

/// 一度だけ鳴らす音
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OneShot {
    pub kind: SoundKind,
    pub placement: Placement,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SoundPlan {
    pub voices: Vec<Voice>,
    pub one_shots: Vec<OneShot>,
}

/// 音を出す設備 (アクチュエータ) の位置
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Emitter {
    pub id: u32,
    pub kind: ActuatorKind,
    pub position: DVec3,
}

/// 1 フレーム分の設備の状態
#[derive(Clone, Debug, Default)]
pub struct MachineFrame {
    pub actuators: Vec<Emitter>,
    /// 動いているアクチュエータ
    pub moving: BTreeSet<u32>,
    /// このフレームで動き始めたアクチュエータ
    pub started: Vec<u32>,
    /// コンベアの id と位置
    pub conveyors: Vec<(u32, DVec3)>,
    pub conveyors_running: bool,
    /// 停止の警報を鳴らす
    pub alarm: bool,
    /// このフレームで干渉が起きた
    pub interference_began: bool,
}

/// 同時に鳴らし続ける音の数の上限 (警報は別)。多すぎると濁って聞き分けられない。
pub const MAX_VOICES: usize = 8;

pub fn plan_sounds(
    frame: &MachineFrame,
    listener: &Listener,
    settings: &SoundSettings,
) -> SoundPlan {
    let level = |kind: SoundKind, placement: Placement| Placement {
        gain: placement.gain * kind.base_gain() * settings.gain(kind.category()),
        pan: placement.pan,
    };
    let mut voices = Vec::new();
    for emitter in &frame.actuators {
        if !frame.moving.contains(&emitter.id) {
            continue;
        }
        let kind = match emitter.kind {
            ActuatorKind::Cylinder => continue,
            ActuatorKind::LinearAxis => SoundKind::ServoWhine,
            ActuatorKind::RotaryTable => SoundKind::MotorHum,
        };
        voices.push(Voice {
            key: VoiceKey::Actuator(emitter.id),
            kind,
            placement: level(kind, place(listener, emitter.position)),
        });
    }
    if frame.conveyors_running {
        for &(id, position) in &frame.conveyors {
            let kind = SoundKind::ConveyorRumble;
            voices.push(Voice {
                key: VoiceKey::Conveyor(id),
                kind,
                placement: level(kind, place(listener, position)),
            });
        }
    }
    // 大きく聞こえるものから残す
    voices.retain(|v| v.placement.gain > 0.0);
    voices.sort_by(|a, b| {
        b.placement
            .gain
            .total_cmp(&a.placement.gain)
            .then(a.key.cmp(&b.key))
    });
    voices.truncate(MAX_VOICES);
    if frame.alarm {
        let placement = level(SoundKind::Alarm, Placement::CENTER);
        if placement.gain > 0.0 {
            voices.push(Voice {
                key: VoiceKey::Alarm,
                kind: SoundKind::Alarm,
                placement,
            });
        }
    }

    let mut one_shots = Vec::new();
    let positions: HashMap<u32, &Emitter> = frame.actuators.iter().map(|e| (e.id, e)).collect();
    for id in &frame.started {
        let Some(emitter) = positions.get(id) else {
            continue;
        };
        if emitter.kind == ActuatorKind::Cylinder {
            let kind = SoundKind::AirExhaust;
            one_shots.push(OneShot {
                kind,
                placement: level(kind, place(listener, emitter.position)),
            });
        }
    }
    if frame.interference_began {
        let kind = SoundKind::WarningBeep;
        one_shots.push(OneShot {
            kind,
            placement: level(kind, Placement::CENTER),
        });
    }
    one_shots.retain(|s| s.placement.gain > 0.0);
    SoundPlan { voices, one_shots }
}

/// アクチュエータの値の変化から、動いているか・動き始めたかを見る。
///
/// 値が変わってから [`MotionTracker::HOLD`] 秒は動いているとみなす (描画が速くて
/// 同じ値のフレームが挟まっても音が途切れないように)。
#[derive(Clone, Debug, Default)]
pub struct MotionTracker {
    /// アクチュエータ → (前の値, 最後に動いた時刻)
    last: HashMap<u32, (f64, Option<f64>)>,
}

impl MotionTracker {
    pub const HOLD: f64 = 0.12;
    const EPSILON: f64 = 1e-9;

    /// いまの値を伝え、このフレームで動き始めたアクチュエータを返す。
    /// `now` は実時間 (秒)。初めて見たアクチュエータは動き始めとはみなさない。
    pub fn observe(&mut self, now: f64, values: impl IntoIterator<Item = (u32, f64)>) -> Vec<u32> {
        let mut started = Vec::new();
        let mut seen = BTreeSet::new();
        for (id, value) in values {
            seen.insert(id);
            match self.last.get_mut(&id) {
                None => {
                    self.last.insert(id, (value, None));
                }
                Some((previous, moved_at)) => {
                    if (value - *previous).abs() > Self::EPSILON {
                        let was_moving = moved_at.is_some_and(|t| now - t <= Self::HOLD);
                        if !was_moving {
                            started.push(id);
                        }
                        *moved_at = Some(now);
                    }
                    *previous = value;
                }
            }
        }
        self.last.retain(|id, _| seen.contains(id));
        started.sort_unstable();
        started
    }

    /// 動いているアクチュエータ
    pub fn moving(&self, now: f64) -> BTreeSet<u32> {
        self.last
            .iter()
            .filter(|(_, (_, moved_at))| moved_at.is_some_and(|t| now - t <= Self::HOLD))
            .map(|(&id, _)| id)
            .collect()
    }

    /// 値をすべて忘れる (プロジェクトを開き直したときなど、次の値で鳴らさないように)
    pub fn clear(&mut self) {
        self.last.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener() -> Listener {
        Listener {
            position: DVec3::new(0.0, 0.0, 10.0),
            right: DVec3::X,
            reference_distance: 10.0,
        }
    }

    fn emitter(id: u32, kind: ActuatorKind, x: f64) -> Emitter {
        Emitter {
            id,
            kind,
            position: DVec3::new(x, 0.0, 0.0),
        }
    }

    #[test]
    fn moving_axes_hum_and_cylinders_hiss_once() {
        let frame = MachineFrame {
            actuators: vec![
                emitter(1, ActuatorKind::LinearAxis, 0.0),
                emitter(2, ActuatorKind::RotaryTable, 0.0),
                emitter(3, ActuatorKind::Cylinder, 5.0),
                emitter(4, ActuatorKind::LinearAxis, 0.0),
            ],
            moving: [1, 2, 3].into(),
            started: vec![3],
            ..Default::default()
        };
        let plan = plan_sounds(&frame, &listener(), &SoundSettings::default());
        let kinds: Vec<_> = plan.voices.iter().map(|v| (v.key, v.kind)).collect();
        assert!(kinds.contains(&(VoiceKey::Actuator(1), SoundKind::ServoWhine)));
        assert!(kinds.contains(&(VoiceKey::Actuator(2), SoundKind::MotorHum)));
        // シリンダは鳴らし続けない。止まっている 4 は鳴らない
        assert_eq!(plan.voices.len(), 2);
        assert_eq!(plan.one_shots.len(), 1);
        assert_eq!(plan.one_shots[0].kind, SoundKind::AirExhaust);
        // 右にあるシリンダの音は右から聞こえる
        assert!(plan.one_shots[0].placement.pan > 0.0);
    }

    #[test]
    fn conveyors_sound_only_while_running() {
        let mut frame = MachineFrame {
            conveyors: vec![(7, DVec3::ZERO)],
            ..Default::default()
        };
        let settings = SoundSettings::default();
        assert!(
            plan_sounds(&frame, &listener(), &settings)
                .voices
                .is_empty()
        );
        frame.conveyors_running = true;
        let plan = plan_sounds(&frame, &listener(), &settings);
        assert_eq!(plan.voices[0].key, VoiceKey::Conveyor(7));
    }

    #[test]
    fn alarm_is_centered_and_not_capped() {
        let frame = MachineFrame {
            actuators: (0..20)
                .map(|i| emitter(i, ActuatorKind::RotaryTable, i as f64))
                .collect(),
            moving: (0..20).collect(),
            alarm: true,
            interference_began: true,
            ..Default::default()
        };
        let plan = plan_sounds(&frame, &listener(), &SoundSettings::default());
        assert_eq!(plan.voices.len(), MAX_VOICES + 1);
        let alarm = plan.voices.last().unwrap();
        assert_eq!(alarm.key, VoiceKey::Alarm);
        assert_eq!(alarm.placement.pan, 0.0);
        // 近いもの (x が小さいもの) が残る
        assert!(plan.voices.iter().any(|v| v.key == VoiceKey::Actuator(0)));
        assert!(!plan.voices.iter().any(|v| v.key == VoiceKey::Actuator(19)));
        assert_eq!(plan.one_shots[0].kind, SoundKind::WarningBeep);
    }

    #[test]
    fn mute_plans_nothing() {
        let frame = MachineFrame {
            actuators: vec![emitter(1, ActuatorKind::Cylinder, 0.0)],
            moving: [1].into(),
            started: vec![1],
            conveyors: vec![(2, DVec3::ZERO)],
            conveyors_running: true,
            alarm: true,
            interference_began: true,
        };
        let settings = SoundSettings {
            muted: true,
            ..SoundSettings::default()
        };
        assert_eq!(
            plan_sounds(&frame, &listener(), &settings),
            SoundPlan::default()
        );
    }

    #[test]
    fn category_volume_scales_gain() {
        let frame = MachineFrame {
            alarm: true,
            ..Default::default()
        };
        let loud = plan_sounds(&frame, &listener(), &SoundSettings::default());
        let quiet = plan_sounds(
            &frame,
            &listener(),
            &SoundSettings {
                alarm: 0.5,
                ..SoundSettings::default()
            },
        );
        let g = |p: &SoundPlan| p.voices[0].placement.gain;
        assert!((g(&quiet) - g(&loud) * 0.5).abs() < 1e-6);
    }

    #[test]
    fn tracker_reports_start_once_and_holds_briefly() {
        let mut t = MotionTracker::default();
        assert!(
            t.observe(0.0, [(1, 0.0)]).is_empty(),
            "初めて見た値では鳴らさない"
        );
        assert!(t.observe(0.016, [(1, 0.0)]).is_empty());
        assert_eq!(t.observe(0.032, [(1, 0.1)]), vec![1]);
        assert_eq!(t.observe(0.048, [(1, 0.2)]), Vec::<u32>::new());
        // 同じ値のフレームが挟まっても、少しの間は動いているとみなす
        assert!(t.observe(0.064, [(1, 0.2)]).is_empty());
        assert!(t.moving(0.064).contains(&1));
        assert!(t.observe(0.07, [(1, 0.3)]).is_empty(), "動き続けている");
        // しばらく止まると止まったとみなし、また動けば動き始め
        assert!(t.moving(0.5).is_empty());
        assert_eq!(t.observe(0.5, [(1, 0.4)]), vec![1]);
    }

    #[test]
    fn tracker_forgets_removed_actuators() {
        let mut t = MotionTracker::default();
        t.observe(0.0, [(1, 0.0), (2, 0.0)]);
        t.observe(0.1, [(1, 0.0)]);
        // 2 が戻ってきても、初めて見た扱い
        assert!(t.observe(0.2, [(1, 0.0), (2, 5.0)]).is_empty());
    }
}
