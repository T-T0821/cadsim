//! 描画の質の設定 ([`RenderQuality`]) を JSON ファイルに読み書きする
//!
//! 見た目とミッションの設定 (`preferences.json`) と同じフォルダの `render.json` に置く。
//! Bevy・egui に依存しない。読めないときは標準の設定で始める。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use cadsim_core::render_quality::RenderQuality;

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct RenderRecord {
    quality: Option<String>,
}

pub fn to_json(quality: RenderQuality) -> String {
    serde_json::to_string_pretty(&RenderRecord {
        quality: Some(quality.key().to_owned()),
    })
    .unwrap_or_default()
}

/// 壊れた JSON や知らない段階の名前は標準の設定にする
pub fn from_json(json: &str) -> RenderQuality {
    serde_json::from_str::<RenderRecord>(json)
        .ok()
        .and_then(|r| r.quality)
        .and_then(|key| RenderQuality::from_key(&key))
        .unwrap_or_default()
}

/// 設定ファイルの場所。ホームフォルダが分からなければ None (保存しない)
pub fn default_path() -> Option<PathBuf> {
    crate::preferences_file::default_path().map(|p| p.with_file_name("render.json"))
}

pub fn load(path: &Path) -> RenderQuality {
    std::fs::read_to_string(path)
        .map(|json| from_json(&json))
        .unwrap_or_default()
}

pub fn save(path: &Path, quality: RenderQuality) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, to_json(quality))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        for q in RenderQuality::ALL {
            assert_eq!(from_json(&to_json(q)), q);
        }
    }

    #[test]
    fn broken_or_unknown_uses_default() {
        for json in ["", "{", "{}", r#"{"quality": 3}"#, r#"{"quality": "ultra"}"#] {
            assert_eq!(from_json(json), RenderQuality::default(), "{json}");
        }
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("cadsim-render-test-{}", std::process::id()));
        let path = dir.join("render.json");
        save(&path, RenderQuality::Low).unwrap();
        assert_eq!(load(&path), RenderQuality::Low);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load(&path), RenderQuality::default());
    }
}
