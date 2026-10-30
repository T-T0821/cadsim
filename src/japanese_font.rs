//! egui で日本語を表示するためのフォント設定
//!
//! egui の標準フォントには日本語の字形がないので、Noto Sans JP を実行ファイルに埋め込み、
//! 標準フォントの「予備」として登録する。英数字は今までどおり標準フォントで描かれ、
//! 標準フォントにない文字 (かな・漢字など) だけ Noto Sans JP で描かれる。
//!
//! `include_bytes!` で実行ファイルに埋め込むので、配布時にフォントファイルを同梱しなくてよい。
//! フォントは `tools/subset_jp_font.py` で必要な文字だけに絞ってある (ライセンスは assets/fonts/OFL.txt)。

use bevy::prelude::*;
use bevy_egui::{
    EguiContexts,
    egui::epaint::text::{FontData, FontFamily, FontInsert, FontPriority, InsertFontFamily},
};

const FONT_NAME: &str = "NotoSansJP";
const FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/NotoSansJP-Subset.otf");

/// 最初のフレームで一度だけ、egui に日本語フォントを追加する。
pub fn install_japanese_font(mut contexts: EguiContexts, mut installed: Local<bool>) -> Result {
    if *installed {
        return Ok(());
    }
    let fallback = |family| InsertFontFamily {
        family,
        priority: FontPriority::Lowest,
    };
    contexts.ctx_mut()?.add_font(FontInsert::new(
        FONT_NAME,
        FontData::from_static(FONT_BYTES),
        vec![
            fallback(FontFamily::Proportional),
            fallback(FontFamily::Monospace),
        ],
    ));
    *installed = true;
    Ok(())
}
