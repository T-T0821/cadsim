//! 「ヘルプ → ライセンス」のウィンドウ。
//!
//! 第三者のソフトウェアのライセンス一覧 (`assets/licenses/THIRD_PARTY_LICENSES.txt`) を
//! 実行ファイルに埋め込んで見せる。exe 1 つで配っても著作権表示が付いて回るようにするため。
//! 一覧は `tools/licenses/generate.sh` で Cargo.lock から作る (docs/licenses.md)。

use bevy_egui::egui;
use std::sync::OnceLock;

const TEXT: &str = include_str!("../../assets/licenses/THIRD_PARTY_LICENSES.txt");

/// 開いているかどうかを egui のメモリに持つ (設計データではないので Resource にしない)
fn open_id() -> egui::Id {
    egui::Id::new("licenses_window_open")
}

/// 一覧の行 (数万行あるので、見えている行だけを描く)
fn lines() -> &'static [&'static str] {
    static LINES: OnceLock<Vec<&'static str>> = OnceLock::new();
    LINES.get_or_init(|| TEXT.lines().collect())
}

pub fn open(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(open_id(), true));
}

pub fn show(ctx: &egui::Context) {
    let mut open = ctx.data(|d| d.get_temp::<bool>(open_id()).unwrap_or(false));
    if !open {
        return;
    }
    egui::Window::new("ライセンス")
        .open(&mut open)
        .default_size([640.0, 480.0])
        .resizable(true)
        .show(ctx, |ui| {
            ui.label(
                "plantwright は MIT License と Apache License 2.0 のデュアルライセンスです。\n\
                 次は plantwright が使っている第三者のソフトウェア・フォント・サンプル形状のライセンスです。",
            );
            if ui
                .button("📋 全文をコピー")
                .on_hover_text("ライセンス一覧の全文をクリップボードにコピーする")
                .clicked()
            {
                ui.ctx().copy_text(TEXT.to_owned());
            }
            ui.separator();
            let lines = lines();
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::both().auto_shrink(false).show_rows(
                ui,
                row_height,
                lines.len(),
                |ui, range| {
                    for line in &lines[range] {
                        ui.label(egui::RichText::new(*line).monospace());
                    }
                },
            );
        });
    ctx.data_mut(|d| d.insert_temp(open_id(), open));
}
