//! 生成 AI (Claude) を使う機能の共通部分
//!
//! - このファイルと `panel`: 設定を持つ Resource と「AI の設定」ウィンドウ
//! - `help`: 「AI に聞く」ウィンドウ (使い方を会話で質問する。中身は `plantwright_core::assistant::help`)
//!
//! 問い合わせ口のトレイト (`LlmClient`)・Claude API での実装・設定ファイル・別スレッドでの問い合わせ (`LlmJob`) は
//! `plantwright_core::assistant`。各機能は [`AiAssistant::client`] で呼び出し口を受け取り、
//! `plantwright_core::assistant::job::LlmJob` で別スレッドに投げる。

pub mod help;
mod panel;

use std::path::PathBuf;
use std::sync::Arc;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use plantwright_core::assistant::claude::ClaudeClient;
use plantwright_core::assistant::llm::LlmClient;
use plantwright_core::assistant::settings_file::AiSettings;

pub struct AssistantPlugin;

impl Plugin for AssistantPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(AiAssistant::load())
            .init_resource::<help::HelpChat>()
            .add_systems(
                EguiPrimaryContextPass,
                (panel::settings_window, help::help_window),
            );
    }
}

/// 生成 AI の設定と、設定ウィンドウの状態
#[derive(Resource, Default)]
pub struct AiAssistant {
    pub settings: AiSettings,
    path: Option<PathBuf>,
    /// 「AI の設定」ウィンドウを出しているか
    pub settings_open: bool,
    /// 設定ウィンドウで入力中のキー (保存するまで反映しない)
    draft_key: String,
    notice: Option<String>,
}

impl AiAssistant {
    fn load() -> Self {
        let path = plantwright_core::assistant::settings_file::default_path();
        let settings = path
            .as_deref()
            .map(plantwright_core::assistant::settings_file::load)
            .unwrap_or_default();
        Self {
            settings,
            path,
            ..Default::default()
        }
    }

    /// API キーが (設定か環境変数で) 使えるか
    pub fn has_key(&self) -> bool {
        !self.settings.effective_key().is_empty()
    }

    /// 今の設定で Claude API を呼ぶ口
    pub fn client(&self) -> Arc<dyn LlmClient> {
        Arc::new(ClaudeClient::new(
            self.settings.effective_key(),
            self.settings.model.clone(),
        ))
    }

    pub fn toggle_settings(&mut self) {
        self.settings_open = !self.settings_open;
    }

    fn save(&mut self) {
        self.notice = Some(match &self.path {
            Some(path) => {
                match plantwright_core::assistant::settings_file::save(path, &self.settings) {
                    Ok(()) => "保存しました".into(),
                    Err(e) => format!("保存できませんでした: {e}"),
                }
            }
            None => "設定フォルダが見つからないため、この起動中だけ使います".into(),
        });
    }
}
