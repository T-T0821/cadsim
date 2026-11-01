//! ミッションの設定 ([`Preferences`]) を JSON ファイルに読み書きする
//!
//! 置き場所は OS ごとの設定フォルダ:
//! - Windows: `%APPDATA%\plantwright\preferences.json`
//! - macOS: `~/Library/Application Support/plantwright/preferences.json`
//! - その他: `$XDG_CONFIG_HOME/plantwright/preferences.json` (なければ `~/.config/plantwright/…`)
//!
//! 旧名 (cadsim) のころの設定フォルダがあり、新しいフォルダがまだなければ、
//! 初めて場所を求めたときに中身 (AI の設定・自作部品なども) を新しいフォルダへ写す。
//! 旧フォルダは消さずに残す。
//!
//! Bevy・egui に依存しない。読めないときは標準の設定で始める。

use std::path::{Path, PathBuf};
use std::sync::Once;

use serde::{Deserialize, Serialize};

use crate::gamify::{Preferences, Progress};

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct PreferencesRecord {
    // 以前の版が書いた "theme" (見た目の切り替え) は、知らない項目として読み飛ばす
    show_missions: Option<bool>,
    completed_missions: Vec<String>,
    skipped_missions: Vec<String>,
    xp: u32,
    welcomed: bool,
}

impl PreferencesRecord {
    fn from_prefs(prefs: &Preferences) -> Self {
        Self {
            show_missions: Some(prefs.show_missions),
            completed_missions: prefs
                .progress
                .completed_keys()
                .into_iter()
                .map(String::from)
                .collect(),
            skipped_missions: prefs
                .progress
                .skipped_keys()
                .into_iter()
                .map(String::from)
                .collect(),
            xp: prefs.progress.xp(),
            welcomed: prefs.welcomed,
        }
    }

    fn into_prefs(self) -> Preferences {
        Preferences {
            show_missions: self.show_missions.unwrap_or(true),
            progress: Progress::restore(
                self.completed_missions.iter().map(String::as_str),
                self.xp,
            )
            .with_skipped(self.skipped_missions.iter().map(String::as_str)),
            welcomed: self.welcomed,
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

/// 設定ファイルを置くフォルダを差し替える環境変数。画面の自動確認で、
/// 使う人の設定 (見た目・ミッションの進み・自作部品・AI の設定) に左右されないようにする
pub const CONFIG_DIR_ENV: &str = "PLANTWRIGHT_CONFIG_DIR";

/// 設定ファイルの場所。ホームフォルダが分からなければ None (保存しない)
pub fn default_path() -> Option<PathBuf> {
    let env = |key| {
        std::env::var_os(key)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(dir) = env(CONFIG_DIR_ENV) {
        return Some(dir.join("preferences.json"));
    }
    let dir = if cfg!(windows) {
        env("APPDATA")
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|home| home.join("Library/Application Support"))
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|home| home.join(".config")))
    }?;
    let new_dir = dir.join(APP_DIR);
    static MIGRATE: Once = Once::new();
    MIGRATE.call_once(|| {
        // 写せなくても設定が標準に戻るだけなので、起動は止めない
        let _ = migrate_legacy_dir(&dir.join(LEGACY_APP_DIR), &new_dir);
    });
    Some(new_dir.join("preferences.json"))
}

/// OS の設定フォルダの中の、このアプリのフォルダ名
const APP_DIR: &str = "plantwright";
/// 旧名のころのフォルダ名
const LEGACY_APP_DIR: &str = "cadsim";

/// 旧名のころの設定フォルダ `old` の中身を `new` へ写す。写したら true。
/// `new` がすでにあるか `old` がなければ何もしない。
/// 途中で失敗しても半端なフォルダを残さないよう、隣の一時フォルダに写してから名前を変える。
pub fn migrate_legacy_dir(old: &Path, new: &Path) -> std::io::Result<bool> {
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    let staging = new.with_extension("migrating");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let copied = copy_dir(old, &staging).and_then(|()| std::fs::rename(&staging, new));
    if copied.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    copied.map(|()| true)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
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
    use crate::gamify::{MissionId, Observation};

    #[test]
    fn round_trip() {
        let mut prefs = Preferences {
            show_missions: false,
            welcomed: true,
            ..Default::default()
        };
        prefs.progress.skip(MissionId::RunFlow);
        prefs.progress.observe(&Observation::default());
        prefs.progress.observe(&Observation {
            selected: true,
            ..Default::default()
        });
        let back = from_json(&to_json(&prefs));
        assert!(!back.show_missions);
        assert!(back.progress.is_done(MissionId::SelectPart));
        assert_eq!(back.progress.xp(), prefs.progress.xp());
        assert!(back.progress.is_skipped(MissionId::RunFlow));
        assert!(back.welcomed);
    }

    #[test]
    fn old_theme_setting_is_ignored() {
        let prefs = from_json(r#"{"theme": "plain", "show_missions": false, "xp": 0}"#);
        assert!(!prefs.show_missions);
    }

    #[test]
    fn broken_or_empty_file_uses_defaults() {
        for json in ["", "{", "{}", r#"{"show_missions": 3}"#] {
            let prefs = from_json(json);
            assert!(prefs.show_missions);
            assert_eq!(prefs.progress.done_count(), 0);
            assert!(!prefs.welcomed);
        }
    }

    #[test]
    fn save_and_load_file() {
        let dir =
            std::env::temp_dir().join(format!("plantwright-prefs-test-{}", std::process::id()));
        let path = dir.join("nested").join("preferences.json");
        let prefs = Preferences {
            show_missions: false,
            ..Default::default()
        };
        save(&path, &prefs).unwrap();
        assert!(!load(&path).show_missions);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(load(&path).show_missions);
    }

    #[test]
    fn migrates_legacy_config_dir_once() {
        let root =
            std::env::temp_dir().join(format!("plantwright-migrate-test-{}", std::process::id()));
        let old = root.join("cadsim");
        let new = root.join("plantwright");
        std::fs::create_dir_all(old.join("parts").join("clamp")).unwrap();
        std::fs::write(old.join("ai.json"), "{}").unwrap();
        std::fs::write(old.join("parts").join("clamp").join("part.json"), "x").unwrap();

        assert!(migrate_legacy_dir(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("ai.json")).unwrap(), "{}");
        assert!(new.join("parts").join("clamp").join("part.json").is_file());
        // 旧フォルダは残し、一時フォルダは残さない
        assert!(old.join("ai.json").is_file());
        assert!(!root.join("plantwright.migrating").exists());

        // 新しいフォルダがあれば上書きしない
        std::fs::write(new.join("ai.json"), "new").unwrap();
        assert!(!migrate_legacy_dir(&old, &new).unwrap());
        assert_eq!(std::fs::read_to_string(new.join("ai.json")).unwrap(), "new");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn no_legacy_dir_does_nothing() {
        let root =
            std::env::temp_dir().join(format!("plantwright-migrate-none-{}", std::process::id()));
        assert!(!migrate_legacy_dir(&root.join("cadsim"), &root.join("plantwright")).unwrap());
        assert!(!root.join("plantwright").exists());
    }
}
