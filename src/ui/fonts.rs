//! 日本語フォントの読み込み
//!
//! egui の内蔵フォントには日本語が無いので、OS に入っているフォントを探して
//! 「足りない文字だけ使う」予備フォントとして登録する。

use bevy::log::{info, warn};
use bevy_egui::egui::{
    self, FontData, FontFamily,
    epaint::text::{FontInsert, FontPriority, InsertFontFamily},
};

/// 探すフォント (見つかった最初の1つを使う)
const CANDIDATES: &[&str] = &[
    // Windows
    "C:\\Windows\\Fonts\\YuGothM.ttc",
    "C:\\Windows\\Fonts\\meiryo.ttc",
    "C:\\Windows\\Fonts\\msgothic.ttc",
    // macOS
    "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/Library/Fonts/Arial Unicode.ttf",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/ipafont-gothic/ipagp.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
];

pub fn install_japanese_font(ctx: &egui::Context) {
    let Some((path, bytes)) = CANDIDATES
        .iter()
        .find_map(|path| Some((*path, std::fs::read(path).ok()?)))
    else {
        warn!("日本語フォントが見つかりませんでした。UI の日本語が表示されません。");
        return;
    };
    info!("日本語フォント: {path}");
    let families = [FontFamily::Proportional, FontFamily::Monospace]
        .into_iter()
        .map(|family| InsertFontFamily {
            family,
            priority: FontPriority::Lowest,
        })
        .collect();
    ctx.add_font(FontInsert::new(
        "japanese",
        FontData::from_owned(bytes),
        families,
    ));
}
