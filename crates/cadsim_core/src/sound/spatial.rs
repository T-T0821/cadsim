//! 聞く位置から見た、音の大きさと左右の振り分け
//!
//! カメラの位置で聞く。カメラが見ている点 (注視点) までの距離を基準にして、
//! それより遠い音は小さく、近い音はそのままの大きさにする。こうすると、拡大して見ている
//! 設備の音が大きく聞こえ、どの倍率で見ても全体の音量が大きく変わらない。

use glam::DVec3;

/// 聞く位置と向き
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub position: DVec3,
    /// 画面の右の向き (長さ 1)
    pub right: DVec3,
    /// この距離までは小さくしない (注視点までの距離)
    pub reference_distance: f64,
}

/// 1 つの音の聞こえ方
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// 0〜1
    pub gain: f32,
    /// -1 (左) 〜 1 (右)
    pub pan: f32,
}

impl Placement {
    /// 位置を持たない音 (警報など): 正面でそのままの大きさ
    pub const CENTER: Placement = Placement {
        gain: 1.0,
        pan: 0.0,
    };

    /// 左右のスピーカーに掛ける倍率。真ん中で両方 1/√2 になる (等パワー)。
    pub fn channel_gains(&self) -> (f32, f32) {
        let angle = (self.pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        (self.gain * angle.cos(), self.gain * angle.sin())
    }
}

/// 遠くても完全には消さない (どこかで動いていることは聞こえるようにする)
const MIN_GAIN: f64 = 0.08;
/// 左右に振り切らない (片方の耳だけで鳴ると聞きにくい)
const MAX_PAN: f64 = 0.8;

pub fn place(listener: &Listener, emitter: DVec3) -> Placement {
    let offset = emitter - listener.position;
    let distance = offset.length();
    let reference = listener.reference_distance.max(1e-6);
    let gain = if distance <= reference {
        1.0
    } else {
        (reference / distance).max(MIN_GAIN)
    };
    let pan = if distance > 1e-9 {
        (offset.dot(listener.right) / distance).clamp(-1.0, 1.0) * MAX_PAN
    } else {
        0.0
    };
    Placement {
        gain: gain as f32,
        pan: pan as f32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener() -> Listener {
        Listener {
            position: DVec3::ZERO,
            right: DVec3::X,
            reference_distance: 10.0,
        }
    }

    #[test]
    fn near_sounds_keep_full_gain_far_sounds_fade() {
        let l = listener();
        assert_eq!(place(&l, DVec3::new(0.0, 0.0, -5.0)).gain, 1.0);
        let far = place(&l, DVec3::new(0.0, 0.0, -20.0));
        assert!((far.gain - 0.5).abs() < 1e-6);
        let very_far = place(&l, DVec3::new(0.0, 0.0, -1e6));
        assert!((very_far.gain as f64 - MIN_GAIN).abs() < 1e-6);
    }

    #[test]
    fn right_side_pans_right() {
        let l = listener();
        assert!(place(&l, DVec3::new(10.0, 0.0, 0.0)).pan > 0.7);
        assert!(place(&l, DVec3::new(-10.0, 0.0, 0.0)).pan < -0.7);
        assert_eq!(place(&l, DVec3::new(0.0, 0.0, -10.0)).pan, 0.0);
        assert_eq!(place(&l, DVec3::ZERO).pan, 0.0);
    }

    #[test]
    fn channel_gains_are_equal_power() {
        let (l, r) = Placement::CENTER.channel_gains();
        assert!((l - r).abs() < 1e-6);
        assert!((l * l + r * r - 1.0).abs() < 1e-5);
        let (l, r) = Placement {
            gain: 1.0,
            pan: 1.0,
        }
        .channel_gains();
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
    }
}
