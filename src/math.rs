//! f64 の設計データを f32 の表示データ (Bevy) に橋渡しする。計算そのものは `plantwright_core::math`。

use bevy::math::DMat4;
use bevy::prelude::*;

/// f64 の配置行列を Bevy の Transform (f32) にする
pub fn to_transform(m: DMat4) -> Transform {
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    Transform {
        translation: translation.as_vec3(),
        rotation: rotation.as_quat(),
        scale: scale.as_vec3(),
    }
}
