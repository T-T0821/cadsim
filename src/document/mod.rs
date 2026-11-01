//! 土台: 設計データの Resource・選択・元に戻す・プロジェクトの開く/保存・ファイルの取り込みと書き出し・ダイアログ
//!
//! 設計データの型 (`Model`) と計算は `plantwright_core`。ここは Bevy とのつなぎだけを持つ。

pub mod command_line;
pub mod dialog;
pub mod history;
pub mod import;
pub mod project;
pub mod scene_asset;
pub mod selection;
