//! 土台: 3D 表示。ビューポート・カメラ・ギズモ・クリックでの選択・画質・工場の建屋・一人称で歩く・キーのショートカット
//!
//! 判断や計算は `plantwright_core` に置き、ここでは入力を集めて結果を画面に写すだけにする。

pub mod camera;
pub mod convert;
pub mod defeature;
pub mod factory_hall;
pub mod gizmo;
pub mod parts;
pub mod picking;
pub mod render_quality;
pub mod shortcuts;
pub mod walk;
