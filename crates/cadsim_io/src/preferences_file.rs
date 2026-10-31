//! 見た目とミッションの設定 ([`Preferences`]) を JSON ファイルに読み書きする
//!
//! 置き場所は OS ごとの設定フォルダ:
//! - Windows: `%APPDATA%\cadsim\preferences.json`
//! - macOS: `~/Library/Application Support/cadsim/preferences.json`
//! - その他: `$XDG_CONFIG_HOME/cadsim/preferences.json` (なければ `~/.config/cadsim/…`)
//!
//! Bevy・egui に依存しない。読めないときは標準の設定で始める。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use cadsim_core::gamify::{Preferences, Progress, ThemeKind};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct PreferencesRecord {
    /// "playful" か "plain"
    theme: String,
    show_missions: Option<bool>,
    completed_missions: Vec<String>,
    xp: u32,
}

impl PreferencesRecord {
    fn from_prefs(prefs: &Preferences) -> Self {
        Self {
            theme: match prefs.theme {
                ThemeKind::Playful => "playful",
                ThemeKind::Plain => "plain",
            }
            .into(),
            show_missions: Some(prefs.show_missions),
            completed_missions: prefs
                .progress
                .completed_keys()
                .into_iter()
                .map(String::from)
                .collect(),
            xp: prefs.progress.xp(),
        }
    }

    fn into_prefs(self) -> Preferences {
        Preferences {
            theme: match self.theme.as_str() {
                "plain" => ThemeKind::Plain,
                _ => ThemeKind::Playful,
            },
            show_missions: self.show_missions.unwrap_or(true),
            progress: Progress::restore(
                self.completed_missions.iter().map(String::as_str),
                self.xp,
            ),
        }
    }
}

pub fn to_json(prefs: &Preferences) -> String {
    serde_json::to_string_pretty(&PreferencesRecord::from_prefs(prefs)).unwrap_or_default()
}

/// 壊れた JSON や知らない項目は無視して、読める分だけ使う
pub fn from_json(json: &str) -> Preferences {
    serde_json::from_str::<PreferencesRecord>(json)
        .unwrap_or_default()
        .into_prefs()
}

/// 設定ファイルの場所。ホームフォルダが分からなければ None (保存しない)
pub fn default_path() -> Option<PathBuf> {
    let env = |key| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let dir = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|home| home.join(".config")))
    }?;
    Some(dir.join("cadsim").join("preferences.json"))
}

pub fn load(path: &Path) -> Preferences {
    std::fs::read_to_string(path)
        .map(|json| from_json(&json))
        .unwrap_or_default()
}

pub fn save(path: &Path, prefs: &Preferences) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, to_json(prefs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadsim_core::gamify::{MissionId, Observation};

    #[test]
    fn round_trip() {
        let mut prefs = Preferences {
            theme: ThemeKind::Plain,
            show_missions: false,
            ..Default::default()
        };
        prefs.progress.observe(&Observation::default());
        prefs.progress.observe(&Observation {
            selected: true,
            ..Default::default()
        });
        let back = from_json(&to_json(&prefs));
        assert_eq!(back.theme, ThemeKind::Plain);
        assert!(!back.show_missions);
        assert!(back.progress.is_done(MissionId::SelectPart));
        assert_eq!(back.progress.xp(), prefs.progress.xp());
    }

    #[test]
    fn broken_or_empty_file_uses_defaults() {
        for json in ["", "{", "{}", r#"{"theme": 3}"#] {
            let prefs = from_json(json);
            assert_eq!(prefs.theme, ThemeKind::Playful);
            assert!(prefs.show_missions);
            assert_eq!(prefs.progress.done_count(), 0);
        }
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("cadsim-prefs-test-{}", std::process::id()));
        let path = dir.join("nested").join("preferences.json");
        let prefs = Preferences {
            theme: ThemeKind::Plain,
            ..Default::default()
        };
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).theme, ThemeKind::Plain);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(load(&path).theme, ThemeKind::Playful);
    }
}
