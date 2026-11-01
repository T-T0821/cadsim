//! 音の設定 ([`SoundSettings`]) を JSON ファイルに読み書きする
//!
//! 見た目とミッションの設定 (`preferences.json`) と同じフォルダの `sound.json` に置く。
//! Bevy・egui に依存しない。読めないときは標準の設定で始める。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use cadsim_core::sound::SoundSettings;

#[derive(Serialize, Deserialize)]
#[serde(default)]
struct SoundRecord {
    muted: bool,
    master: f32,
    machine: f32,
    air: f32,
    alarm: f32,
}

impl Default for SoundRecord {
    fn default() -> Self {
        Self::from_settings(&SoundSettings::default())
    }
}

impl SoundRecord {
    fn from_settings(s: &SoundSettings) -> Self {
        Self {
            muted: s.muted,
            master: s.master,
            machine: s.machine,
            air: s.air,
            alarm: s.alarm,
        }
    }

    fn into_settings(self) -> SoundSettings {
        SoundSettings {
            muted: self.muted,
            master: self.master,
            machine: self.machine,
            air: self.air,
            alarm: self.alarm,
        }
        .sanitized()
    }
}

pub fn to_json(settings: &SoundSettings) -> String {
    serde_json::to_string_pretty(&SoundRecord::from_settings(settings)).unwrap_or_default()
}

/// 壊れた JSON は標準の設定にする。足りない項目は標準の値で埋める。
pub fn from_json(json: &str) -> SoundSettings {
    serde_json::from_str::<SoundRecord>(json)
        .unwrap_or_default()
        .into_settings()
}

/// 設定ファイルの場所。ホームフォルダが分からなければ None (保存しない)
pub fn default_path() -> Option<PathBuf> {
    crate::preferences_file::default_path().map(|p| p.with_file_name("sound.json"))
}

pub fn load(path: &Path) -> SoundSettings {
    std::fs::read_to_string(path)
        .map(|json| from_json(&json))
        .unwrap_or_default()
}

pub fn save(path: &Path, settings: &SoundSettings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, to_json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let settings = SoundSettings {
            muted: true,
            master: 0.25,
            machine: 0.5,
            air: 0.0,
            alarm: 0.75,
        };
        assert_eq!(from_json(&to_json(&settings)), settings);
    }

    #[test]
    fn broken_or_partial_file_uses_defaults() {
        for json in ["", "{", "{}", r#"{"master": "loud"}"#] {
            assert_eq!(from_json(json), SoundSettings::default(), "{json}");
        }
        let partial = from_json(r#"{"muted": true, "alarm": 5.0}"#);
        assert!(partial.muted);
        assert_eq!(partial.alarm, 1.0);
        assert_eq!(partial.master, SoundSettings::default().master);
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("cadsim-sound-test-{}", std::process::id()));
        let path = dir.join("sound.json");
        let settings = SoundSettings {
            muted: true,
            ..Default::default()
        };
        save(&path, &settings).unwrap();
        assert!(load(&path).muted);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!load(&path).muted);
    }
}
