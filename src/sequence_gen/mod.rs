//! タイミングチャート・ラダーの自動生成と、動作の説明
//!
//! - 説明文から、生成 AI でステップ・インターロック・装置の下書きを作り、タイミングチャートに反映する
//! - タイミングチャートから、三菱 PLC 向けのステップラダー (命令リスト・GX Works 用 CSV) と I/O 表を作る (AI は使わない)
//! - チャート・ラダー・PLC のログを生成 AI に読ませ、動作を文章で説明させる
//!
//! 層の分け方 (docs/architecture.md):
//! - `spec`: 動作の下書きの型と、チャートとの変換 (ドメイン、Bevy なし)
//! - `ladder`: チャート → ラダーと I/O 表 (ドメイン、Bevy なし)
//! - `log_events`: PLC のログ → 信号の変化の一覧 (ドメイン、Bevy なし)
//! - `usecase`: 生成 AI への問い合わせの組み立てと返事の読み取り (ユースケース、Bevy なし)
//! - `json`: 下書きと JSON の変換 (アダプター)
//! - このファイルと `ui`: Bevy のリソースと egui のウィンドウ
//!
//! チャートの型 (`TimingChart`) がまだアプリ側にあるので、ここもアプリ側に置いている。
//! チャートが `cadsim_core` に移ったら、`spec`・`ladder`・`log_events`・`usecase` も一緒に移す。

mod json;
pub mod ladder;
mod log_events;
pub mod spec;
mod ui;
mod usecase;

use bevy::prelude::*;
use bevy_egui::EguiPrimaryContextPass;

use crate::assistant::job::LlmJob;
use log_events::LogSummary;
use spec::{ApplyMode, SequenceSpec};

pub struct SequenceGenPlugin;

impl Plugin for SequenceGenPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SequenceGen>()
            .add_systems(EguiPrimaryContextPass, ui::sequence_gen_window);
    }
}

/// ウィンドウのタブ
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Generate,
    Ladder,
    Explain,
}

/// 説明させる材料の種類
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum SourceKind {
    #[default]
    Chart,
    Ladder,
    Log,
}

/// 「チャート・ラダーの自動生成」ウィンドウの状態
#[derive(Resource, Default)]
pub struct SequenceGen {
    tab: Tab,
    description: String,
    mode: ApplyMode,
    generating: Option<LlmJob<SequenceSpec>>,
    /// 生成 AI が作った下書き (反映するまでチャートは変えない)
    draft: Option<SequenceSpec>,
    source: SourceKind,
    ladder_text: String,
    log: Option<(String, LogSummary)>,
    explaining: Option<LlmJob<String>>,
    explanation: String,
    notice: Option<String>,
}

impl SequenceGen {
    /// 終わった問い合わせの結果を受け取る
    fn poll_jobs(&mut self) {
        if let Some(result) = self.generating.as_ref().and_then(LlmJob::poll) {
            self.generating = None;
            match result {
                Ok(spec) => {
                    self.notice = None;
                    self.draft = Some(spec);
                }
                Err(e) => self.notice = Some(e.to_string()),
            }
        }
        if let Some(result) = self.explaining.as_ref().and_then(LlmJob::poll) {
            self.explaining = None;
            match result {
                Ok(text) => {
                    self.notice = None;
                    self.explanation = text;
                }
                Err(e) => self.notice = Some(e.to_string()),
            }
        }
    }
}
