//! 視点 (カメラの位置と注視点) の受け渡し

use glam::Vec3;

/// 参加者の視点。カメラの位置 `eye` から注視点 `focus` を見ている
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewPose {
    pub eye: [f32; 3],
    pub focus: [f32; 3],
    /// 平行投影か
    pub orthographic: bool,
}

impl ViewPose {
    pub fn eye(&self) -> Vec3 {
        Vec3::from(self.eye)
    }

    pub fn focus(&self) -> Vec3 {
        Vec3::from(self.focus)
    }

    /// 見ている向き (単位ベクトル)。位置と注視点が同じなら -Z
    pub fn direction(&self) -> Vec3 {
        (self.focus() - self.eye())
            .try_normalize()
            .unwrap_or(Vec3::NEG_Z)
    }

    /// 送り直すほど動いたか。位置・注視点が注視点までの距離の 0.1% を超えて動いたか、
    /// 投影が変わったら true
    pub fn moved_from(&self, other: &ViewPose) -> bool {
        let scale = self.eye().distance(self.focus()).max(1e-6) * 1e-3;
        self.orthographic != other.orthographic
            || self.eye().distance(other.eye()) > scale
            || self.focus().distance(other.focus()) > scale
    }
}

/// 注視点を回るカメラ (アプリの `OrbitCamera`) の角度に直す。戻り値は (yaw, pitch, 距離)。
///
/// カメラは `focus + Ry(yaw)·Rx(-pitch)·(0, 0, 距離)` にいる。
pub fn orbit_angles(pose: &ViewPose) -> (f32, f32, f32) {
    let offset = pose.eye() - pose.focus();
    let distance = offset.length();
    if distance < 1e-9 {
        return (0.0, 0.0, 1e-3);
    }
    let pitch = (offset.y / distance).clamp(-1.0, 1.0).asin();
    let yaw = offset.x.atan2(offset.z);
    (yaw, pitch, distance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{EulerRot, Quat};

    #[test]
    fn angles_round_trip_through_the_orbit_camera_formula() {
        let focus = Vec3::new(1.0, 2.0, -3.0);
        for (yaw, pitch, distance) in [
            (0.8f32, 0.5f32, 8.0f32),
            (-2.5, -0.3, 0.4),
            (3.0, 1.2, 120.0),
        ] {
            let rotation = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0);
            let eye = focus + rotation * Vec3::new(0.0, 0.0, distance);
            let pose = ViewPose {
                eye: eye.into(),
                focus: focus.into(),
                orthographic: false,
            };
            let (y, p, d) = orbit_angles(&pose);
            assert!((y - yaw).abs() < 1e-4, "{y} vs {yaw}");
            assert!((p - pitch).abs() < 1e-4, "{p} vs {pitch}");
            assert!((d - distance).abs() < 1e-3 * distance);
        }
    }

    #[test]
    fn small_moves_are_not_resent() {
        let a = ViewPose {
            eye: [0.0, 0.0, 10.0],
            focus: [0.0; 3],
            orthographic: false,
        };
        let mut b = a;
        b.eye[0] = 0.001;
        assert!(!b.moved_from(&a));
        b.eye[0] = 0.5;
        assert!(b.moved_from(&a));
        let mut c = a;
        c.orthographic = true;
        assert!(c.moved_from(&a));
    }
}
