//! 入力 (UI 層): キーボードのショートカット、ファイルのダイアログとドラッグ&ドロップ、コマンドライン引数
//!
//! 入力を集めてコマンドに変えるだけにし、実際の処理は各機能の Resource や内側の層に任せる。

pub mod command_line;
pub mod file_dialog;
pub mod shortcuts;

use bevy::prelude::*;

/// 入力のプラグインをまとめたもの
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((shortcuts::ShortcutsPlugin, file_dialog::FileDialogPlugin));
    }
}
