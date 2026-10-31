//! 3D 表示 (UI 層): ビューポート、カメラ、ギズモ、動画の撮影
//!
//! 判断や計算は `cadsim_core` に置き、ここでは入力を集めて結果を画面に写すだけにする。

pub mod camera;
pub mod gizmo;
pub mod recording;
pub mod viewport;
