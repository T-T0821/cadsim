//! 動画の書き出し: 設定ウィンドウと画面のキャプチャ (`recording`)、紹介動画のカメラワーク (`camera_path`)
//!
//! 撮る時刻の計算と、MP4 (H.264) / GIF / PNG 連番へのエンコードは `plantwright_core::video`。

pub mod camera_path;
pub mod recording;
