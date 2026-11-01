//! plantwright のロゴ: ウィンドウのアイコン (タスクバー・Alt+Tab) と、画面に出すロゴの画像
//!
//! 元の絵は assets/icon/ の SVG。ここでは書き出し済みの PNG を実行ファイルに埋め込んで使う。
//! Mac ではウィンドウのアイコンは .app の .icns が使われ、ここでの設定は効かない。

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bevy::winit::WINIT_WINDOWS;
use bevy_egui::egui;

/// アプリのアイコン (紺の角丸つき, 256px)
const ICON_PNG: &[u8] = include_bytes!("../../assets/icon/plantwright-icon-256.png");
/// 背景なしのマーク (128px)。ようこそ画面などに出す
const MARK_PNG: &[u8] = include_bytes!("../../assets/icon/plantwright-mark-128.png");

/// ウィンドウができたらアイコンを付ける
pub struct LogoPlugin;

impl Plugin for LogoPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, set_window_icon);
    }
}

/// PNG を RGBA の画素と幅・高さにする
fn decode_rgba(png: &[u8]) -> (Vec<u8>, u32, u32) {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("埋め込みのロゴ画像が読めない")
        .into_rgba8();
    let (width, height) = image.dimensions();
    (image.into_raw(), width, height)
}

/// メインのウィンドウに一度だけアイコンを付ける。winit のウィンドウはメインスレッドでしか触れない
fn set_window_icon(
    mut done: Local<bool>,
    window: Query<Entity, With<PrimaryWindow>>,
    _main_thread: NonSendMarker,
) {
    if *done {
        return;
    }
    let Ok(entity) = window.single() else {
        return;
    };
    WINIT_WINDOWS.with_borrow(|windows| {
        let Some(winit_window) = windows.get_window(entity) else {
            return;
        };
        let (rgba, width, height) = decode_rgba(ICON_PNG);
        match winit::window::Icon::from_rgba(rgba, width, height) {
            Ok(icon) => winit_window.set_window_icon(Some(icon)),
            Err(e) => warn!("ウィンドウのアイコンを作れませんでした: {e}"),
        }
        *done = true;
    });
}

/// 背景なしのマークを egui のテクスチャにする。呼んだ側で持っておき、毎フレーム作り直さない
pub fn mark_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let (rgba, width, height) = decode_rgba(MARK_PNG);
    let image = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
    ctx.load_texture("plantwright-mark", image, egui::TextureOptions::LINEAR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_logos_decode() {
        let (rgba, w, h) = decode_rgba(ICON_PNG);
        assert_eq!((w, h), (256, 256));
        assert_eq!(rgba.len(), 256 * 256 * 4);
        let (_, w, h) = decode_rgba(MARK_PNG);
        assert_eq!((w, h), (128, 128));
    }

    #[test]
    fn icon_is_accepted_by_winit() {
        let (rgba, w, h) = decode_rgba(ICON_PNG);
        assert!(winit::window::Icon::from_rgba(rgba, w, h).is_ok());
    }
}
