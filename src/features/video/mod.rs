//! 動画書き出しのアダプター (Bevy 非依存)
//!
//! - `encode`: MP4 (H.264) / GIF / PNG 連番への書き出し。別スレッドで動き、
//!   `plantwright_core::video::VideoSink` を実装する
//! - `mp4`: H.264 を MP4 に入れる最小限のライター
//!
//! 撮る時刻の計算は `plantwright_core::video`、画面のキャプチャと設定画面は `recording` が受け持つ。

pub mod camera_path;
pub mod recording;
