//! 紹介動画のカメラワークをプロジェクトファイル (.plantwright) に書く形 (Bevy に依存しない)
//!
//! 視点は「動画の秒数・注視点・ヨー / ピッチ (度)・距離」で書く。角度は人が読みやすいよう度にする。

use crate::camera_path::{CameraKey, CameraPath, CameraPose, Interpolation};
use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CameraPathRecord {
    /// つなぎ方 ("smooth" / "linear")。知らない名前ならなめらかにする
    #[serde(default = "default_interpolation")]
    pub interpolation: String,
    #[serde(default)]
    pub keys: Vec<CameraKeyRecord>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CameraKeyRecord {
    /// 動画の頭からの秒数
    pub time: f64,
    pub focus: [f32; 3],
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub distance: f32,
}

fn default_interpolation() -> String {
    Interpolation::default().key().into()
}

/// 視点がなければ None (ファイルに書かない)
pub fn to_record(path: &CameraPath) -> Option<CameraPathRecord> {
    if path.is_empty() {
        return None;
    }
    Some(CameraPathRecord {
        interpolation: path.interpolation.key().into(),
        keys: path
            .keys()
            .iter()
            .map(|k| CameraKeyRecord {
                time: k.time,
                focus: k.pose.focus.to_array(),
                yaw_deg: k.pose.yaw.to_degrees(),
                pitch_deg: k.pose.pitch.to_degrees(),
                distance: k.pose.distance,
            })
            .collect(),
    })
}

/// 壊れた値 (距離が 0 以下・数でない) の視点は読み飛ばす
pub fn from_record(record: Option<&CameraPathRecord>) -> CameraPath {
    let Some(record) = record else {
        return CameraPath::default();
    };
    let keys = record
        .keys
        .iter()
        .filter(|k| {
            k.time.is_finite()
                && k.focus.iter().all(|v| v.is_finite())
                && k.yaw_deg.is_finite()
                && k.pitch_deg.is_finite()
                && k.distance.is_finite()
                && k.distance > 0.0
        })
        .map(|k| CameraKey {
            time: k.time,
            pose: CameraPose {
                focus: Vec3::from_array(k.focus),
                yaw: k.yaw_deg.to_radians(),
                pitch: k.pitch_deg.to_radians(),
                distance: k.distance,
            },
        })
        .collect();
    let interpolation = Interpolation::from_key(&record.interpolation).unwrap_or_default();
    CameraPath::from_keys(keys, interpolation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let mut path = CameraPath::turntable(
            CameraPose {
                focus: Vec3::new(100.0, 20.0, -5.0),
                yaw: 0.3,
                pitch: 0.4,
                distance: 2500.0,
            },
            8.0,
        );
        path.interpolation = Interpolation::Linear;
        let record = to_record(&path).unwrap();
        let json = serde_json::to_string(&record).unwrap();
        let back: CameraPathRecord = serde_json::from_str(&json).unwrap();
        let restored = from_record(Some(&back));
        assert_eq!(restored.len(), path.len());
        assert_eq!(restored.interpolation, Interpolation::Linear);
        for (a, b) in restored.keys().iter().zip(path.keys()) {
            assert_eq!(a.time, b.time);
            assert!(a.pose.focus.distance(b.pose.focus) < 1e-3);
            assert!((a.pose.yaw - b.pose.yaw).abs() < 1e-5);
            assert!((a.pose.pitch - b.pose.pitch).abs() < 1e-5);
        }
    }

    #[test]
    fn empty_path_is_not_written() {
        assert!(to_record(&CameraPath::default()).is_none());
        assert!(from_record(None).is_empty());
    }

    #[test]
    fn broken_keys_are_skipped() {
        let record: CameraPathRecord = serde_json::from_str(
            r#"{"interpolation":"???","keys":[
                {"time":0,"focus":[0,0,0],"yaw_deg":0,"pitch_deg":10,"distance":5},
                {"time":2,"focus":[0,0,0],"yaw_deg":90,"pitch_deg":10,"distance":0}
            ]}"#,
        )
        .unwrap();
        let path = from_record(Some(&record));
        assert_eq!(path.len(), 1);
        assert_eq!(path.interpolation, Interpolation::Smooth);
    }
}
