//! ノードの配置とその時間変化 (ファイルから取り込んだアニメーション)

use glam::{DAffine3, DQuat, DVec3};

/// 平行移動・回転・拡大縮小 (この順で親に対して適用される)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeTransform {
    pub translation: DVec3,
    pub rotation: DQuat,
    pub scale: DVec3,
}

impl Default for NodeTransform {
    fn default() -> Self {
        Self {
            translation: DVec3::ZERO,
            rotation: DQuat::IDENTITY,
            scale: DVec3::ONE,
        }
    }
}

impl NodeTransform {
    pub fn to_affine(self) -> DAffine3 {
        DAffine3::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }

    pub fn from_affine(affine: DAffine3) -> Self {
        let (scale, rotation, translation) = affine.to_scale_rotation_translation();
        Self {
            translation,
            rotation,
            scale,
        }
    }

    /// 2つの姿勢の間を補間する (位置と拡大は線形、回転は球面線形)
    pub fn lerp(self, other: Self, t: f64) -> Self {
        Self {
            translation: self.translation.lerp(other.translation, t),
            rotation: self.rotation.slerp(other.rotation, t),
            scale: self.scale.lerp(other.scale, t),
        }
    }
}

/// ノードの姿勢の時間変化
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Animation {
    pub name: String,
    pub tracks: Vec<Track>,
}

/// 1つのノードのキーフレーム。キーの間は線形補間する
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub node: usize,
    /// (時刻 [秒], 親から見た配置)。時刻の昇順
    pub keys: Vec<(f64, NodeTransform)>,
}

impl Track {
    /// 時刻 `time` の姿勢。範囲外は端のキーで止める
    pub fn sample(&self, time: f64) -> Option<NodeTransform> {
        let first = self.keys.first()?;
        if time <= first.0 {
            return Some(first.1);
        }
        let index = self.keys.partition_point(|(t, _)| *t <= time);
        let Some(next) = self.keys.get(index) else {
            return self.keys.last().map(|k| k.1);
        };
        let prev = self.keys[index - 1];
        let span = next.0 - prev.0;
        let t = if span > 0.0 {
            (time - prev.0) / span
        } else {
            1.0
        };
        Some(prev.1.lerp(next.1, t))
    }
}

impl Animation {
    /// 最後のキーの時刻 [秒]
    pub fn duration(&self) -> f64 {
        self.tracks
            .iter()
            .filter_map(|t| t.keys.last().map(|k| k.0))
            .fold(0.0, f64::max)
    }
}
