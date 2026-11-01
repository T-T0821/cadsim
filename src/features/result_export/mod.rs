//! シミュレーション結果の CSV 出力 (メニューの「ファイル > シミュレーション結果 (CSV)」)
//!
//! | ファイル | 層 | 役割 |
//! | --- | --- | --- |
//! | `report/` | ユースケース | チャートと形状から CSV の文字列を作る (Bevy に依存しない) |
//! | `report/csv.rs` | | CSV の書式 (BOM 付き UTF-8, CRLF) |
//! | `report/cycle_time.rs` | | ステップごとの時刻と所要時間 |
//! | `report/interference.rs` | | 1サイクルを動かしながら干渉チェックし、状態の続いた時間帯にまとめる |
//! | `plugin.rs` | UI・フレームワーク | メニューからの要求を受け、保存ダイアログ → 別スレッドで作成・書き込み → ステータスバーに結果 |

mod plugin;
mod report;

pub use plugin::{ResultExport, ResultExportPlugin, ResultKind};
