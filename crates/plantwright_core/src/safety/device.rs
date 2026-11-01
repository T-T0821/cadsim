//! 安全機器のデータと、侵入の検知・停止信号の保持 (ラッチ)
//!
//! - ライトカーテン: 縦に張った光の面。物が光軸を遮ると検知する
//! - エリアセンサ: 床の上の検知範囲 (レーザースキャナなど)。範囲に入ると検知する
//! - 安全柵: 人が入れないようにする柵。検知はしないが、危険源からの距離を確かめる
//! - 非常停止ボタン: 押すと停止。押している間はリセットできない
//!
//! 検知した瞬間に停止信号を出し、リセットするまで止めたままにする (ラッチ)。
//! 侵入が続いている間や非常停止が押されている間はリセットできない。
//!
//! Bevy に依存しない純粋なデータと計算だけを置く。

use std::collections::BTreeSet;

use glam::{DVec2, DVec3};

use crate::floor::geom::{Rect2, Zone};
use crate::math::Aabb;

/// 安全機器の種類
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeviceKind {
    LightCurtain,
    AreaSensor,
    Fence,
    EmergencyStop,
}

impl DeviceKind {
    pub const ALL: [DeviceKind; 4] = [
        DeviceKind::LightCurtain,
        DeviceKind::AreaSensor,
        DeviceKind::Fence,
        DeviceKind::EmergencyStop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            DeviceKind::LightCurtain => "ライトカーテン",
            DeviceKind::AreaSensor => "エリアセンサ",
            DeviceKind::Fence => "安全柵",
            DeviceKind::EmergencyStop => "非常停止ボタン",
        }
    }

    /// 侵入を検知する機器か
    pub fn detects(self) -> bool {
        matches!(self, DeviceKind::LightCurtain | DeviceKind::AreaSensor)
    }
}

/// 配置した安全機器 1つ。長さは mm、時間は秒。
#[derive(Clone, Debug, PartialEq)]
pub struct SafetyDevice {
    pub id: u32,
    pub name: String,
    pub kind: DeviceKind,
    /// 床の上の中心 (x, z) と下端の高さ (y)
    pub position: [f64; 3],
    /// Y 軸まわりの向き (度)
    pub yaw_deg: f64,
    /// 幅 (ローカル X)、高さ (Y)、奥行き (ローカル Z)。
    /// ライトカーテンは「光の面の幅 × 高さ」、エリアセンサは「範囲の幅 × 検知する高さ × 奥行き」、
    /// 柵は「長さ × 高さ × 厚み」。
    pub size: [f64; 3],
    /// 検知してから機械が止まるまでの時間 (センサの応答 + 機械の停止時間)
    pub stop_time: f64,
    /// ライトカーテンの検出能力 (光軸の間隔から決まる、検知できる最小の物の太さ)
    pub resolution: f64,
    /// 安全柵: 危険源 (動作範囲) から離すべき距離
    pub required_clearance: f64,
}

impl SafetyDevice {
    /// 種類ごとの標準的な大きさで作る
    pub fn new(id: u32, kind: DeviceKind, floor: [f64; 2]) -> Self {
        let (size, base) = match kind {
            DeviceKind::LightCurtain => ([600.0, 1500.0, 30.0], 300.0),
            DeviceKind::AreaSensor => ([2000.0, 300.0, 1500.0], 0.0),
            DeviceKind::Fence => ([2000.0, 1800.0, 50.0], 0.0),
            DeviceKind::EmergencyStop => ([80.0, 1100.0, 80.0], 0.0),
        };
        Self {
            id,
            name: format!("{} {id}", kind.label()),
            kind,
            position: [floor[0], base, floor[1]],
            yaw_deg: 0.0,
            size,
            stop_time: 0.2,
            resolution: 30.0,
            required_clearance: 850.0,
        }
    }

    /// 床の上の形 (向き付きの長方形)
    pub fn footprint(&self) -> Rect2 {
        Rect2 {
            center: DVec2::new(self.position[0], self.position[2]),
            half: DVec2::new(self.size[0], self.size[2]) * 0.5,
            yaw: self.yaw_deg.to_radians(),
        }
    }

    /// 機器の形 (床の長方形 + 高さの範囲)
    pub fn zone(&self) -> Zone {
        Zone {
            footprint: self.footprint(),
            y_min: self.position[1],
            y_max: self.position[1] + self.size[1],
        }
    }

    /// 侵入を検知する範囲。検知しない機器は None。
    pub fn detection_zone(&self) -> Option<Zone> {
        self.kind.detects().then(|| self.zone())
    }

    /// 非常停止ボタンの押しボタンの位置 (柱の上)
    pub fn button_position(&self) -> DVec3 {
        DVec3::new(
            self.position[0],
            self.position[1] + self.size[1],
            self.position[2],
        )
    }

    /// ISO 13855 による、検知面から危険源までに必要な最小距離 S (mm)。
    /// ライトカーテンとエリアセンサだけ。
    pub fn safety_distance(&self) -> Option<f64> {
        match self.kind {
            DeviceKind::LightCurtain => {
                Some(light_curtain_distance(self.stop_time, self.resolution))
            }
            DeviceKind::AreaSensor => Some(area_sensor_distance(self.stop_time, self.position[1])),
            _ => None,
        }
    }
}

/// 配置した安全機器の一覧
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SafetyLayout {
    pub devices: Vec<SafetyDevice>,
    pub next_id: u32,
}

impl SafetyLayout {
    /// 機器を足して id を返す
    pub fn add(&mut self, kind: DeviceKind, floor: [f64; 2]) -> u32 {
        self.next_id = self.next_id.max(1);
        let id = self.next_id;
        self.next_id += 1;
        self.devices.push(SafetyDevice::new(id, kind, floor));
        id
    }

    pub fn get(&self, id: u32) -> Option<&SafetyDevice> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut SafetyDevice> {
        self.devices.iter_mut().find(|d| d.id == id)
    }

    pub fn remove(&mut self, id: u32) {
        self.devices.retain(|d| d.id != id);
    }
}

/// 垂直に張ったライトカーテン (検出能力 d ≤ 40 mm) の最小距離。
/// S = 2000·T + 8(d − 14)。500 mm を超えるときは K = 1600 で計算し直し、500 mm を下限にする。
/// d > 40 mm (腕・体の検知) では C = 850 mm とする。
pub fn light_curtain_distance(stop_time: f64, resolution: f64) -> f64 {
    let t_ms = stop_time.max(0.0) * 1000.0;
    let c = if resolution <= 40.0 {
        (8.0 * (resolution - 14.0)).max(0.0)
    } else {
        850.0
    };
    let s = 2.0 * t_ms + c;
    if s <= 500.0 {
        s.max(100.0)
    } else {
        (1.6 * t_ms + c).max(500.0)
    }
}

/// 床に水平な検知範囲 (エリアセンサ) の最小距離。
/// S = 1600·T + (1200 − 0.4·H)。H は検知面の高さ (mm)、C は 850 mm を下限にする。
pub fn area_sensor_distance(stop_time: f64, height: f64) -> f64 {
    let t_ms = stop_time.max(0.0) * 1000.0;
    let c = (1200.0 - 0.4 * height.max(0.0)).max(850.0);
    1.6 * t_ms + c
}

/// 侵入を検知させうる物 (動いている部品・作業者・AGV など)
#[derive(Clone, Debug, PartialEq)]
pub struct Intruder {
    pub label: String,
    pub bounds: Aabb,
}

/// 1つの機器が検知した (または押された) こと
#[derive(Clone, Debug, PartialEq)]
pub struct Detection {
    pub device: u32,
    /// 何が入ったか ("作業者 1"、"非常停止" など)
    pub cause: String,
}

/// 検知範囲と侵入物を照らし合わせる。
/// `blocked` は手動で遮光したことにする機器、`pressed` は押されている非常停止ボタン。
pub fn detect(
    devices: &[SafetyDevice],
    intruders: &[Intruder],
    blocked: &BTreeSet<u32>,
    pressed: &BTreeSet<u32>,
) -> Vec<Detection> {
    let mut detections = Vec::new();
    for device in devices {
        if device.kind == DeviceKind::EmergencyStop {
            if pressed.contains(&device.id) {
                detections.push(Detection {
                    device: device.id,
                    cause: "非常停止".into(),
                });
            }
            continue;
        }
        let Some(zone) = device.detection_zone() else {
            continue;
        };
        if blocked.contains(&device.id) {
            detections.push(Detection {
                device: device.id,
                cause: "遮光テスト".into(),
            });
        }
        if let Some(intruder) = intruders.iter().find(|i| zone.intersects(&i.bounds)) {
            detections.push(Detection {
                device: device.id,
                cause: intruder.label.clone(),
            });
        }
    }
    detections
}

/// 停止信号を保持する (ラッチ)。リセットされるまで止めたまま。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SafetyMonitor {
    /// 停止させた原因 (最初に検知したものから順に)
    pub causes: Vec<Detection>,
}

/// リセットできなかった理由
#[derive(Clone, Debug, PartialEq)]
pub enum ResetBlocked {
    /// まだ検知が続いている
    StillDetected(Vec<Detection>),
}

impl SafetyMonitor {
    /// 停止信号が出ているか
    pub fn is_stopped(&self) -> bool {
        !self.causes.is_empty()
    }

    /// 今回の検知を取り込む。新しく止まったら true。
    pub fn update(&mut self, detections: &[Detection]) -> bool {
        let was_stopped = self.is_stopped();
        for d in detections {
            if !self.causes.iter().any(|c| c.device == d.device) {
                self.causes.push(d.clone());
            }
        }
        !was_stopped && self.is_stopped()
    }

    /// 停止を解除する。検知が続いていれば解除しない。
    pub fn reset(&mut self, detections: &[Detection]) -> Result<(), ResetBlocked> {
        if !detections.is_empty() {
            return Err(ResetBlocked::StillDetected(detections.to_vec()));
        }
        self.causes.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intruder(min: [f64; 3], max: [f64; 3]) -> Intruder {
        Intruder {
            label: "作業者".into(),
            bounds: Aabb {
                min: DVec3::from_array(min),
                max: DVec3::from_array(max),
            },
        }
    }

    #[test]
    fn light_curtain_distance_follows_iso_13855() {
        // T = 0.2 s, d = 14 mm: 2000 × 0.2 + 0 = 400
        assert!((light_curtain_distance(0.2, 14.0) - 400.0).abs() < 1e-9);
        // T = 0.3 s, d = 30 mm: 2000×0.3 + 128 = 728 > 500 → 1600×0.3 + 128 = 608
        assert!((light_curtain_distance(0.3, 30.0) - 608.0).abs() < 1e-9);
        // 速い機械でも 100 mm は取る
        assert!((light_curtain_distance(0.01, 14.0) - 100.0).abs() < 1e-9);
        // K = 1600 で計算し直して 500 を下回るときは 500
        assert!((light_curtain_distance(0.26, 14.0) - 500.0).abs() < 1e-9);
    }

    #[test]
    fn area_sensor_distance_uses_height() {
        // T = 0.5 s, H = 0: 800 + 1200
        assert!((area_sensor_distance(0.5, 0.0) - 2000.0).abs() < 1e-9);
        // 高くしても C は 850 を下回らない
        assert!((area_sensor_distance(0.0, 1000.0) - 850.0).abs() < 1e-9);
    }

    #[test]
    fn light_curtain_detects_crossing_object() {
        let curtain = SafetyDevice::new(1, DeviceKind::LightCurtain, [0.0, 0.0]);
        let devices = vec![curtain];
        let none = BTreeSet::new();
        let through = intruder([-100.0, 800.0, -200.0], [100.0, 1700.0, 200.0]);
        let found = detect(&devices, &[through], &none, &none);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].device, 1);
        // 光の面の手前で止まっていれば検知しない
        let before = intruder([-100.0, 800.0, 100.0], [100.0, 1700.0, 300.0]);
        assert!(detect(&devices, &[before], &none, &none).is_empty());
        // 光の面より下 (足元) を通っても検知しない
        let below = intruder([-100.0, 0.0, -200.0], [100.0, 200.0, 200.0]);
        assert!(detect(&devices, &[below], &none, &none).is_empty());
    }

    #[test]
    fn fence_and_estop_do_not_detect_intrusion() {
        let devices = vec![
            SafetyDevice::new(1, DeviceKind::Fence, [0.0, 0.0]),
            SafetyDevice::new(2, DeviceKind::EmergencyStop, [0.0, 0.0]),
        ];
        let none = BTreeSet::new();
        let body = intruder([-1000.0, 0.0, -1000.0], [1000.0, 2000.0, 1000.0]);
        assert!(detect(&devices, &[body], &none, &none).is_empty());
        let pressed = BTreeSet::from([2]);
        let found = detect(&devices, &[], &none, &pressed);
        assert_eq!(found[0].cause, "非常停止");
    }

    #[test]
    fn stop_latches_until_reset_with_clear_zone() {
        let mut monitor = SafetyMonitor::default();
        let hit = vec![Detection {
            device: 3,
            cause: "作業者".into(),
        }];
        assert!(monitor.update(&hit));
        // 侵入が終わっても止まったまま
        assert!(!monitor.update(&[]));
        assert!(monitor.is_stopped());
        // 侵入が続いている間はリセットできない
        assert!(monitor.reset(&hit).is_err());
        assert!(monitor.is_stopped());
        assert!(monitor.reset(&[]).is_ok());
        assert!(!monitor.is_stopped());
    }
}
