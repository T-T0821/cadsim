//! 生成 AI の設定 (API キーとモデル) を JSON ファイルに読み書きする
//!
//! 置き場所は見た目の設定 (`preferences.json`) と同じフォルダの `ai.json`。
//! API キーはプロジェクトファイル (.plantwright) には書かないので、ファイルを人に渡しても漏れない。
//! 設定がなければ環境変数 `ANTHROPIC_API_KEY` を使う。
//!
//! Bevy・egui に依存しない。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 生成 AI の設定
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AiSettings {
    /// Claude API のキー ("sk-ant-…")。空なら環境変数を使う。
    pub api_key: String,
    /// モデルの id。空なら既定のモデル。
    pub model: String,
}

impl AiSettings {
    /// 実際に使うキー (設定が空なら環境変数 `ANTHROPIC_API_KEY`)
    pub fn effective_key(&self) -> String {
        Self::pick_key(&self.api_key, std::env::var("ANTHROPIC_API_KEY").ok())
    }

    fn pick_key(saved: &str, env: Option<String>) -> String {
        if saved.trim().is_empty() {
            env.unwrap_or_default().trim().to_string()
        } else {
            saved.trim().to_string()
        }
    }

    /// 画面に出すキーの状態 ("設定済み (…abcd)" など)。キーそのものは出さない。
    pub fn key_status(&self) -> String {
        let key = self.effective_key();
        if key.is_empty() {
            return "未設定".into();
        }
        let tail: String = key
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let source = if self.api_key.trim().is_empty() {
            "環境変数"
        } else {
            "設定済み"
        };
        format!("{source} (…{tail})")
    }
}

/// 設定ファイルの場所。ホームフォルダが分からなければ None (保存しない)
pub fn default_path() -> Option<PathBuf> {
    crate::preferences_file::default_path().map(|p| p.with_file_name("ai.json"))
}

/// 読めない・壊れているときは空の設定
pub fn load(path: &Path) -> AiSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

pub fn save(path: &Path, settings: &AiSettings) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(settings).unwrap_or_default();
    std::fs::write(path, json)?;
    restrict_permissions(path);
    Ok(())
}

/// API キーが入るので、Unix では自分だけが読めるようにする
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_key_wins_over_environment() {
        assert_eq!(AiSettings::pick_key(" abc ", Some("env".into())), "abc");
        assert_eq!(AiSettings::pick_key("", Some(" env ".into())), "env");
        assert_eq!(AiSettings::pick_key("", None), "");
    }

    #[test]
    fn key_status_hides_the_key() {
        let s = AiSettings {
            api_key: "sk-ant-secret-1234".into(),
            model: String::new(),
        };
        let status = s.key_status();
        assert!(status.contains("1234"));
        assert!(!status.contains("secret"));
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("plantwright-ai-test-{}", std::process::id()));
        let path = dir.join("ai.json");
        let s = AiSettings {
            api_key: "k".into(),
            model: "claude-sonnet-5-5".into(),
        };
        save(&path, &s).unwrap();
        assert_eq!(load(&path), s);
        std::fs::write(&path, "{").unwrap();
        assert_eq!(load(&path), AiSettings::default());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
